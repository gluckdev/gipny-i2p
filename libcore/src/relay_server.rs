//! Each unlocked device hosts a published I2P terminal. The protected
//! profile database retains its inbound destination key across restarts.
//! Contacts verify the profile owner's signature before transmitting.
//! Queued envelopes and partial uploads use bounded memory in this process.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::sync::{mpsc, Mutex, OwnedSemaphorePermit, RwLock, Semaphore};
use tokio::task::{JoinHandle, JoinSet};

use crate::net::NetError;
#[cfg(test)]
use crate::relay::recv;
use crate::relay::{send, ClientToRelay, RelayError, RelayToClient, ERR_NEEDS_AUTH_V2};

/// Answers one relay-network request, given the connection's challenge and the
/// request's bytes; returns the response's bytes. The node behind it lives in
/// [`crate::dht_client`]; the relay only carries the frames.
pub type DhtHandler = Arc<dyn Fn(&[u8; 32], Vec<u8>) -> Vec<u8> + Send + Sync>;
pub type ViewOnceAccept = Box<dyn FnOnce(Vec<u8>) -> bool + Send>;
pub type ViewOnceHandler = Arc<dyn Fn([u8; 32], crate::view_once::Request) -> Option<(Vec<u8>, ViewOnceAccept)> + Send + Sync>;

/// Requests one anonymous relay-network connection may make, and how long it
/// may sit idle between them.
const DHT_MAX_REQUESTS: usize = 64;
const DHT_IDLE: Duration = Duration::from_secs(60);
/// Streams served at once. Each costs a task and frame buffers before it has
/// proved anything; past this, new streams are closed at once rather than
/// queued, so a flood of idle connections cannot starve the messenger.
const MAX_CLIENTS: usize = 256;
/// How long a new stream may take to send its first frame (auth or a DHT
/// request). An i2p round trip is seconds; a silent stream is let go.
const FIRST_FRAME_DEADLINE: Duration = Duration::from_secs(60);
/// Global inbound frame-allocation budget: bytes held by body buffers while
/// frames are read and deserialized.
const INBOUND_FRAME_BUDGET_BYTES: u32 = 64 * 1024 * 1024;
const INBOUND_FRAME_BUDGET_UNIT_BYTES: u32 = 16 * 1024;

/// Live connections by the signing key they authenticated as.
pub type Connections = Arc<RwLock<HashMap<[u8; 32], Lanes>>>;

/// The connections one key has open at once. Usually one; a client taking a
/// file in parts opens more, and new letters go to the least loaded of them,
/// so its mail comes down several i2p streams at a time. One cursor for all:
/// what went down any of them is not pushed again down another. Catching up
/// on what was stored meanwhile is the first connection's job.
pub struct Lanes {
    senders: Vec<Lane>,
    next: std::sync::atomic::AtomicUsize,
    cursor: Arc<Mutex<Pushed>>,
}

/// What of a key's mail has been handed to one of its connections and not
/// acked. Whoever marks a message first — its deposit, an Ack's sweep, the
/// catch-up — is the one that pushes it. A plain high-water mark was set only
/// after the deposit answered its sender, an i2p write that can take seconds;
/// a sweep in between pushed it too (e2e run 36056012101: parts twice).
#[derive(Default, Debug)]
pub struct Pushed {
    /// Every id above this has been looked at by a sweep or a deposit.
    high: u64,
    ids: std::collections::HashSet<u64>,
}

impl Pushed {
    /// Mark `id` as handed out; false if it already was.
    fn claim(&mut self, id: u64) -> bool {
        self.high = self.high.max(id);
        self.ids.insert(id)
    }

    fn acked(&mut self, id: u64) {
        self.ids.remove(&id);
    }

    fn is_claimed(&self, id: u64) -> bool {
        self.ids.contains(&id)
    }

    fn release_for_retry(&mut self, id: u64) {
        self.ids.remove(&id);
        self.high = self.high.min(id.saturating_sub(1));
    }
}

enum EnqueueOutcome {
    Queued,
    AlreadyClaimed,
    Backpressure,
}

async fn try_enqueue_unclaimed(
    tx: &mpsc::Sender<RelayToClient>,
    cursor: &Arc<Mutex<Pushed>>,
    id: u64,
    blob: Vec<u8>,
) -> EnqueueOutcome {
    let mut c = cursor.lock().await;
    if c.is_claimed(id) {
        return EnqueueOutcome::AlreadyClaimed;
    }
    let pkt = RelayToClient::Incoming { id, from: [0u8; 32], blob };
    match tx.try_send(pkt) {
        Ok(()) => {
            c.claim(id);
            EnqueueOutcome::Queued
        }
        Err(_) => EnqueueOutcome::Backpressure,
    }
}

async fn release_for_retry(cursor: &Arc<Mutex<Pushed>>, ids: &[u64]) {
    if ids.is_empty() {
        return;
    }
    let mut c = cursor.lock().await;
    for id in ids {
        c.release_for_retry(*id);
    }
}

/// One connection of a key: where its pushes go, and how many it was given
/// and has not acked.
struct Lane {
    tx: mpsc::Sender<RelayToClient>,
    unacked: Arc<std::sync::atomic::AtomicUsize>,
}

impl Lane {
    /// Given and not acked, and queued to be written.
    fn load(&self) -> usize {
        self.unacked.load(std::sync::atomic::Ordering::Relaxed) + (self.tx.max_capacity() - self.tx.capacity())
    }
}

impl Lanes {
    /// The live connection with the least outstanding, round the lanes among
    /// equals. Dealt strictly in turn, a connection slow or broken one way
    /// (its pings still coming, its pushes not getting through) took its
    /// share of every file all the same: after a recipient restarted, its
    /// relay took 73 parts and handed out 36 (the laptop, 2026-09-25).
    fn pick(&self) -> Option<mpsc::Sender<RelayToClient>> {
        let live: Vec<&Lane> = self.senders.iter().filter(|l| !l.tx.is_closed()).collect();
        if live.is_empty() {
            return None;
        }
        let least = live.iter().map(|l| l.load()).min()?;
        let even: Vec<&&Lane> = live.iter().filter(|l| l.load() == least).collect();
        let i = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Some(even[i % even.len()].tx.clone())
    }

    fn is_first(&self, tx: &mpsc::Sender<RelayToClient>) -> bool {
        self.senders.iter().find(|l| !l.tx.is_closed()).is_some_and(|l| l.tx.same_channel(tx))
    }
}

/// Maximum number of pending envelopes in one push batch.
const PENDING_LIMIT: usize = 200;
/// How often a connected client is re-offered messages it has not acked.
const PUSH_REFRESH: Duration = Duration::from_secs(30);
/// A client says something at least every 20 s (a ping, if nothing else); one
/// silent this long is gone, though its close never came. Its connection is
/// dropped and what it was given goes to the key's others — kept, it went on
/// taking a share of new mail into nowhere (a recipient restarted with three
/// extra connections open: one close arrived, three did not).
#[cfg(not(test))]
const CLIENT_SILENT: Duration = Duration::from_secs(90);
#[cfg(test)]
const CLIENT_SILENT: Duration = Duration::from_secs(2);
const GC_INTERVAL: Duration = Duration::from_secs(600);
const PUSH_CAPACITY: usize = 512;
/// Heap cost we charge per stored message in addition to ciphertext bytes.
///
/// This keeps quota enforcement robust against many tiny records: every entry
/// costs map/set nodes, ids, timestamps, and Vec bookkeeping even when payload
/// is small.
const MESSAGE_METADATA_BYTES: usize = 96;

#[derive(Clone)]
struct InboundFrameBudget {
    unit_bytes: u32,
    sem: Arc<Semaphore>,
}

impl InboundFrameBudget {
    fn new(total_bytes: u32, unit_bytes: u32) -> Self {
        let unit_bytes = unit_bytes.max(1);
        let permits = total_bytes.max(1).div_ceil(unit_bytes) as usize;
        Self { unit_bytes, sem: Arc::new(Semaphore::new(permits)) }
    }

    fn permits_for_len(&self, len: u32) -> u32 {
        if len == 0 { 0 } else { len.div_ceil(self.unit_bytes) }
    }

    async fn acquire_for_len(&self, len: u32) -> Result<Option<OwnedSemaphorePermit>, RelayError> {
        let permits = self.permits_for_len(len);
        if permits == 0 {
            return Ok(None);
        }
        let permit = self.sem.clone().acquire_many_owned(permits).await
            .map_err(|_| RelayError::Proto("inbound frame budget closed".into()))?;
        Ok(Some(permit))
    }

    #[cfg(test)]
    fn available_units(&self) -> usize {
        self.sem.available_permits()
    }
}

fn inbound_frame_budget() -> &'static InboundFrameBudget {
    static BUDGET: OnceLock<InboundFrameBudget> = OnceLock::new();
    BUDGET.get_or_init(|| InboundFrameBudget::new(INBOUND_FRAME_BUDGET_BYTES, INBOUND_FRAME_BUDGET_UNIT_BYTES))
}

async fn recv_server_frame<R, T>(r: &mut R) -> Result<T, RelayError>
where
    R: AsyncRead + Unpin,
    T: serde::de::DeserializeOwned,
{
    recv_server_frame_with_budget(r, inbound_frame_budget()).await
}

async fn recv_server_frame_with_budget<R, T>(r: &mut R, budget: &InboundFrameBudget) -> Result<T, RelayError>
where
    R: AsyncRead + Unpin,
    T: serde::de::DeserializeOwned,
{
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);
    if len > crate::relay::MAX_FRAME {
        return Err(RelayError::Proto("frame too large".into()));
    }
    let _budget = budget.acquire_for_len(len).await?;
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).await?;
    Ok(bincode::deserialize(&buf)?)
}

fn accounted_message_bytes(blob_len: usize) -> usize {
    blob_len.saturating_add(MESSAGE_METADATA_BYTES)
}

/// Budget for a relay held in RAM.
///
/// Memory quota includes message metadata. Evicted, unacknowledged envelopes
/// can be retried by the sender.
#[derive(Clone, Debug)]
struct MemStoreLimits {
    /// Global message budget including a fixed per-record metadata charge.
    pub max_total_bytes: usize,
    /// Held message bytes for one recipient, so one busy inbox cannot starve
    /// every other.
    pub max_recipient_bytes: usize,
    /// Global record bound, so many tiny messages cannot exhaust maps/sets by
    /// count while staying under byte caps.
    pub max_total_messages: usize,
    /// A prekey bundle is a few hundred bytes; anything far beyond is not one.
    pub max_bundle_bytes: usize,
    pub max_bundles: usize,
    pub message_ttl: Duration,
    pub bundle_ttl: Duration,
    /// Serve this one identity only: hold mail and a prekey bundle for it and
    /// refuse everyone else's. What a relay inside somebody's app is for — it
    /// is that person's inbox, not a public mailbox that strangers can fill
    /// with up to `max_total_bytes` of the host's memory. `None` serves
    /// any identity in multi-recipient store tests.
    pub only_for: Option<[u8; 32]>,
}

impl Default for MemStoreLimits {
    fn default() -> Self {
        Self {
            max_total_bytes: 96 * 1024 * 1024,
            max_recipient_bytes: 24 * 1024 * 1024,
            max_total_messages: 100_000,
            max_bundle_bytes: 16 * 1024,
            max_bundles: 4096,
            message_ttl: Duration::from_secs(3 * 24 * 3600),
            bundle_ttl: Duration::from_secs(7 * 24 * 3600),
            only_for: None,
        }
    }
}

impl MemStoreLimits {
    /// The defaults, serving `owner` alone.
    pub fn personal(owner: [u8; 32]) -> Self {
        Self { only_for: Some(owner), ..Self::default() }
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("larger than this relay holds")]
    TooLarge,
    #[error("this relay does not serve that recipient")]
    NotServed,
    #[error("invalid message blob")]
    InvalidBlob,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StoreStats {
    pub bundles: usize,
    pub messages: usize,
    pub message_bytes: usize,
}

struct Stored {
    recipient: [u8; 32],
    blob: Vec<u8>,
    at: Instant,
}

#[derive(Default)]
struct Inbox {
    ids: BTreeSet<u64>,
    bytes: usize,
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    /// Keyed by id, which is also arrival order: the first entry is the oldest.
    messages: BTreeMap<u64, Stored>,
    inboxes: HashMap<[u8; 32], Inbox>,
    bundles: HashMap<[u8; 32], (Vec<u8>, Instant)>,
    total_payload_bytes: usize,
    total_quota_bytes: usize,
}

impl Inner {
    fn remove(&mut self, id: u64) -> Option<Stored> {
        let m = self.messages.remove(&id)?;
        self.total_payload_bytes -= m.blob.len();
        self.total_quota_bytes -= accounted_message_bytes(m.blob.len());
        if let Some(inbox) = self.inboxes.get_mut(&m.recipient) {
            inbox.ids.remove(&id);
            inbox.bytes -= m.blob.len();
            if inbox.ids.is_empty() {
                self.inboxes.remove(&m.recipient);
            }
        }
        Some(m)
    }
}

/// In-memory relay storage with the semantics `client_loop` relies on:
/// ids are unique and increasing, `pending_above` returns them in order.
struct MemStore {
    inner: std::sync::Mutex<Inner>,
    limits: MemStoreLimits,
    serving: std::sync::RwLock<Serving>,
    terminal_identity: std::sync::RwLock<Option<Arc<crate::crypto::Identity>>>,
    view_once: std::sync::RwLock<Option<ViewOnceHandler>>,
}

/// Whose mail a store takes.
#[derive(Clone, Copy)]
enum Serving {
    #[cfg(test)]
    Anyone,
    Only([u8; 32]),
    /// Built before its owner is known (tunnels built while the password is
    /// typed): takes nothing from anyone until [`MemStore::claim`].
    Nobody,
}

impl MemStore {
    pub fn new(mut limits: MemStoreLimits) -> Self {
        // A recipient cap above the total cap would let one deposit push the
        // total over while evicting nothing of that recipient's.
        limits.max_recipient_bytes = limits.max_recipient_bytes.min(limits.max_total_bytes);
        limits.max_total_messages = limits.max_total_messages.max(1);
        let serving = match limits.only_for {
            Some(owner) => Serving::Only(owner),
            #[cfg(test)]
            None => Serving::Anyone,
            #[cfg(not(test))]
            None => Serving::Nobody,
        };
        Self { inner: std::sync::Mutex::new(Inner::default()), limits, terminal_identity: Default::default(), serving: std::sync::RwLock::new(serving), view_once: Default::default() }
    }

    /// A personal store whose owner is not known yet: refuses everything.
    #[cfg(test)]
    fn unclaimed(limits: MemStoreLimits) -> Self {
        let store = Self::new(limits);
        *store.serving.write().unwrap_or_else(|p| p.into_inner()) = Serving::Nobody;
        store
    }

    /// From now on serve `owner` alone.
    #[cfg(test)]
    fn claim(&self, owner: [u8; 32]) {
        *self.serving.write().unwrap_or_else(|p| p.into_inner()) = Serving::Only(owner);
    }

    fn serves(&self, pk: &[u8; 32]) -> bool {
        match *self.serving.read().unwrap_or_else(|p| p.into_inner()) {
            #[cfg(test)]
            Serving::Anyone => true,
            Serving::Only(owner) => owner == *pk,
            Serving::Nobody => false,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves plain collections behind, not a
        // broken invariant worth refusing all later requests over.
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn store_bundle(&self, pk: &[u8; 32], bundle: &[u8]) -> Result<(), StoreError> {
        if !self.serves(pk) {
            return Err(StoreError::NotServed);
        }
        if bundle.len() > self.limits.max_bundle_bytes {
            return Err(StoreError::TooLarge);
        }
        let mut g = self.lock();
        if !g.bundles.contains_key(pk) && g.bundles.len() >= self.limits.max_bundles {
            let stalest = g.bundles.iter().min_by_key(|(_, (_, at))| *at).map(|(k, _)| *k);
            if let Some(k) = stalest {
                g.bundles.remove(&k);
            }
        }
        g.bundles.insert(*pk, (bundle.to_vec(), Instant::now()));
        Ok(())
    }

    pub fn get_bundle(&self, pk: &[u8; 32]) -> Option<Vec<u8>> {
        self.lock().bundles.get(pk).map(|(b, _)| b.clone())
    }

    pub fn deposit(&self, to: &[u8; 32], blob: &[u8]) -> Result<u64, StoreError> {
        if !self.serves(to) {
            return Err(StoreError::NotServed);
        }
        if blob.is_empty() {
            return Err(StoreError::InvalidBlob);
        }
        let cost = accounted_message_bytes(blob.len());
        if blob.len() > self.limits.max_recipient_bytes {
            return Err(StoreError::TooLarge);
        }
        if cost > self.limits.max_total_bytes {
            return Err(StoreError::TooLarge);
        }
        let mut g = self.lock();
        g.next_id += 1;
        let id = g.next_id;
        g.messages.insert(id, Stored { recipient: *to, blob: blob.to_vec(), at: Instant::now() });
        g.total_payload_bytes += blob.len();
        g.total_quota_bytes += cost;
        let inbox = g.inboxes.entry(*to).or_default();
        inbox.ids.insert(id);
        inbox.bytes += blob.len();

        // Oldest first, this recipient's own messages before anyone else's. The
        // new message has the highest id and fits both caps on its own, so it is
        // never the one evicted.
        loop {
            let oldest = match g.inboxes.get(to) {
                Some(inbox) if inbox.bytes > self.limits.max_recipient_bytes => inbox.ids.first().copied(),
                _ => None,
            };
            match oldest {
                Some(old) => { g.remove(old); }
                None => break,
            }
        }
        while g.total_quota_bytes > self.limits.max_total_bytes || g.messages.len() > self.limits.max_total_messages {
            let Some(old) = g.messages.keys().next().copied() else { break };
            g.remove(old);
        }
        Ok(id)
    }

    /// Up to [`PENDING_LIMIT`] of `pk`'s messages with id above `cursor`, in order.
    pub fn pending_above(&self, pk: &[u8; 32], cursor: u64) -> Vec<(u64, Vec<u8>)> {
        let g = self.lock();
        let Some(inbox) = g.inboxes.get(pk) else { return Vec::new() };
        inbox
            .ids
            .range(cursor.saturating_add(1)..)
            .take(PENDING_LIMIT)
            .filter_map(|id| g.messages.get(id).map(|m| (*id, m.blob.clone())))
            .collect()
    }

    /// Drop message `id`, but only if it is addressed to `pk`.
    ///
    pub fn ack(&self, pk: &[u8; 32], id: u64) -> bool {
        let mut g = self.lock();
        match g.messages.get(&id) {
            Some(m) if m.recipient == *pk => g.remove(id).is_some(),
            _ => false,
        }
    }

    pub fn gc(&self) {
        self.gc_at(Instant::now());
    }

    fn gc_at(&self, now: Instant) {
        let mut g = self.lock();
        let ttl = self.limits.message_ttl;
        loop {
            let expired = g
                .messages
                .iter()
                .next()
                .filter(|(_, m)| now.saturating_duration_since(m.at) > ttl)
                .map(|(id, _)| *id);
            match expired {
                Some(id) => { g.remove(id); }
                None => break,
            }
        }
        let bundle_ttl = self.limits.bundle_ttl;
        g.bundles.retain(|_, (_, at)| now.saturating_duration_since(*at) <= bundle_ttl);
    }

    pub fn stats(&self) -> StoreStats {
        let g = self.lock();
        StoreStats { bundles: g.bundles.len(), messages: g.messages.len(), message_bytes: g.total_payload_bytes }
    }
}

/// A relay-network connection: request, answer, until the other side is done.
/// It never logs in, so nothing here touches the mailbox or `Connections`.
async fn serve_dht<S>(mut stream: S, challenge: [u8; 32], first: Vec<u8>, dht: Option<DhtHandler>) -> Result<(), RelayError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let Some(dht) = dht else {
        send(&mut stream, &RelayToClient::Error("not a relay-network node".into())).await?;
        return Ok(());
    };
    let mut request = first;
    for served in 1.. {
        let answer = tokio::task::spawn_blocking({
            let dht = dht.clone();
            move || dht(&challenge, request)
        })
        .await
        .map_err(|e| RelayError::Proto(format!("dht handler: {e}")))?;
        send(&mut stream, &RelayToClient::Dht(answer)).await?;
        if served >= DHT_MAX_REQUESTS {
            return Ok(());
        }
        request = match tokio::time::timeout(DHT_IDLE, recv_server_frame::<_, ClientToRelay>(&mut stream)).await {
            Err(_) => return Ok(()),
            Ok(Err(RelayError::Io(e))) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Ok(Err(e)) => return Err(e),
            Ok(Ok(ClientToRelay::Dht(bytes))) => bytes,
            Ok(Ok(_)) => return Err(RelayError::Proto("only relay-network frames after one".into())),
        };
    }
    Ok(())
}

async fn handle_client<S>(
    mut stream: S,
    store: Arc<MemStore>,
    connections: Connections,
    destination_hash: [u8; 32],
    dht: Option<DhtHandler>,
) -> Result<(), RelayError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let mut challenge = [0u8; 32];
    crate::crypto::fill_random(&mut challenge);
    send(&mut stream, &RelayToClient::Challenge(challenge)).await?;

    let first = tokio::time::timeout(FIRST_FRAME_DEADLINE, recv_server_frame::<_, ClientToRelay>(&mut stream)).await
        .map_err(|_| RelayError::Proto("no first frame in time".into()))??;
    let terminal_proof = if let ClientToRelay::AuthFramedV1 { sign_pk, nonce, .. } = &first {
        let owner=store.terminal_identity.read().unwrap_or_else(|p|p.into_inner());
        let owner=owner.as_ref().ok_or(RelayError::AuthFailed)?;
        Some(owner.sign(&crate::relay::terminal_proof_message(&destination_hash,sign_pk,&challenge,nonce)))
    } else { None };
    let framed=terminal_proof.is_some();
    let (sign_pk, signature, signed, owner) = match first {
        ClientToRelay::Dht(first) => return serve_dht(stream, challenge, first, dht).await,
        ClientToRelay::AuthFramedV1 { sign_pk, signature, nonce } => {
            (sign_pk, signature, crate::relay::framed_auth_message(&destination_hash, &challenge, &nonce), true)
        }
        ClientToRelay::AuthV2 { sign_pk, signature } => {
            (sign_pk, signature, crate::relay::auth_v2_message(&destination_hash, &challenge), true)
        }
        ClientToRelay::Auth { sign_pk, signature } => (sign_pk, signature, challenge.to_vec(), false),
        _ => return Err(RelayError::Proto("expected Auth first".into())),
    };
    let verified = VerifyingKey::from_bytes(&sign_pk)
        .map(|vk| vk.verify(&signed, &Signature::from_bytes(&signature)).is_ok())
        .unwrap_or(false);
    if !verified {
        send(&mut stream, &RelayToClient::AuthFail).await?;
        return Err(RelayError::AuthFailed);
    }

    let (push_tx, mut push_rx) = mpsc::channel::<RelayToClient>(PUSH_CAPACITY);
    if !owner {
        let cursor = Arc::new(Mutex::new(Pushed::default()));
        let mut given = std::collections::HashSet::new();
        let result = match send(&mut stream, &RelayToClient::AuthOk).await {
            Ok(()) => client_loop(&mut stream, &mut push_rx, &push_tx, sign_pk, false, false, &store, &connections, &cursor, &mut given, &Default::default()).await,
            Err(e) => Err(e),
        };
        return result;
    }
    let unacked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (cursor, first) = {
        let mut conns = connections.write().await;
        let lanes = conns.entry(sign_pk).or_insert_with(|| Lanes {
            senders: Vec::new(),
            next: std::sync::atomic::AtomicUsize::new(0),
            cursor: Arc::new(Mutex::new(Pushed::default())),
        });
        lanes.senders.retain(|l| !l.tx.is_closed());
        let first = lanes.senders.is_empty();
        if first {
            // Nothing live for this key: whatever an earlier connection had
            // been pushed and not acked goes again.
            lanes.cursor = Arc::new(Mutex::new(Pushed::default()));
        }
        lanes.senders.push(Lane { tx: push_tx.clone(), unacked: unacked.clone() });
        (lanes.cursor.clone(), first)
    };

    let initial = tokio::spawn({
        let (store, push_tx, cursor) = (store.clone(), push_tx.clone(), cursor.clone());
        async move {
            // Another connection of this key is catching up already.
            if !first { return; }
            for (id, blob) in store.pending_above(&sign_pk, 0) {
                match try_enqueue_unclaimed(&push_tx, &cursor, id, blob).await {
                    EnqueueOutcome::Queued | EnqueueOutcome::AlreadyClaimed => {}
                    EnqueueOutcome::Backpressure => break,
                }
            }
        }
    });
    let refresh = tokio::spawn({
        let (store, push_tx, cursor, connections) = (store.clone(), push_tx.clone(), cursor.clone(), connections.clone());
        async move {
            let mut tick = tokio::time::interval(PUSH_REFRESH);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            tick.tick().await;
            loop {
                tick.tick().await;
                // One connection of a key sweeps, not each of them.
                let sweeps = connections.read().await.get(&sign_pk).is_some_and(|l| l.is_first(&push_tx));
                if !sweeps { continue; }
                let cur = cursor.lock().await.high;
                for (id, blob) in store.pending_above(&sign_pk, cur) {
                    match try_enqueue_unclaimed(&push_tx, &cursor, id, blob).await {
                        EnqueueOutcome::Queued | EnqueueOutcome::AlreadyClaimed => {}
                        EnqueueOutcome::Backpressure => break,
                    }
                }
            }
        }
    });

    let mut given = std::collections::HashSet::new();
    let authenticated = if let Some(proof) = terminal_proof { RelayToClient::AuthFramedOkV1 { proof } } else { RelayToClient::AuthOk };
    let result = match send(&mut stream, &authenticated).await {
        Ok(()) => client_loop(&mut stream, &mut push_rx, &push_tx, sign_pk, true, framed, &store, &connections, &cursor, &mut given, &unacked).await,
        Err(e) => Err(e),
    };

    initial.abort();
    refresh.abort();
    push_rx.close();
    while let Ok(msg) = push_rx.try_recv() {
        if let RelayToClient::Incoming { id, .. } = msg { given.insert(id); }
    }
    let mut failed_redeal = Vec::new();
    {
        let mut conns = connections.write().await;
        if let Some(lanes) = conns.get_mut(&sign_pk) {
            lanes.senders.retain(|l| !l.tx.same_channel(&push_tx) && !l.tx.is_closed());
            if lanes.senders.is_empty() {
                conns.remove(&sign_pk);
            } else {
                failed_redeal = redeal(lanes, &store.pending_above(&sign_pk, 0), &given);
            }
        }
    }
    release_for_retry(&cursor, &failed_redeal).await;
    result
}

/// What a closed connection of a key was given and never acked goes to the
/// others still open. The cursor is past it, so without this it would wait
/// until every connection of the key had closed.
fn redeal(lanes: &Lanes, pending: &[(u64, Vec<u8>)], given: &std::collections::HashSet<u64>) -> Vec<u64> {
    let mut failed = Vec::new();
    for (id, blob) in pending.iter().filter(|(id, _)| given.contains(id)) {
        let Some(tx) = lanes.pick() else {
            failed.push(*id);
            continue;
        };
        let pkt = RelayToClient::Incoming { id: *id, from: [0u8; 32], blob: blob.clone() };
        if tx.try_send(pkt).is_err() {
            failed.push(*id);
        }
    }
    failed
}

async fn client_loop<S>(
    stream: &mut S,
    push_rx: &mut mpsc::Receiver<RelayToClient>,
    push_tx: &mpsc::Sender<RelayToClient>,
    sign_pk: [u8; 32],
    owner: bool,
    framed: bool,
    store: &Arc<MemStore>,
    connections: &Connections,
    cursor: &Arc<Mutex<Pushed>>,
    given: &mut std::collections::HashSet<u64>,
    unacked: &Arc<std::sync::atomic::AtomicUsize>,
) -> Result<(), RelayError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    // One read of a frame is kept across turns of the loop and polled by
    // reference: `select!` must never drop it half-way, or the bytes it had
    // read are gone and the next read takes the middle of a frame for its
    // length ("frame too large", a dropped connection, and everything unacked
    // pushed again — e2e run 36047667974 under a file in parts).
    let (rd, mut wr) = tokio::io::split(stream);
    let mut reading = Box::pin(read_frame::<_, ClientToRelay>(rd));
    let mut offer: Option<([u8; 16], ViewOnceAccept)> = None;
    let mut parts = crate::view_once::PartBuffer::default();
    let mut deposit = crate::relay_deposit::Assembly::default();
    let silent = tokio::time::sleep(CLIENT_SILENT);
    tokio::pin!(silent);
    loop {
        tokio::select! {
            () = &mut silent => {
                crate::debug_trace!("[relay-server] client silent for {CLIENT_SILENT:?}, dropping it");
                break;
            }
            (rd, frame) = &mut reading => {
                reading = Box::pin(read_frame(rd));
                silent.as_mut().reset(tokio::time::Instant::now() + CLIENT_SILENT);
                let frame = match frame? {
                    ClientToRelay::SendPartV1 { to, index, total, bytes } if owner => {
                        match deposit.push(to, index, total, &bytes, std::time::Instant::now()).map_err(|e| RelayError::Proto(e.into()))? {
                            Some(blob) => ClientToRelay::Send { to, blob },
                            None => {
                                let end = tokio::time::Instant::from_std(deposit.deadline().unwrap());
                                silent.as_mut().reset(end.min(tokio::time::Instant::now() + CLIENT_SILENT));
                                send(&mut wr, &RelayToClient::SendPartAckV1 { index }).await?;
                                continue;
                            }
                        }
                    }
                    _ if deposit.deadline().is_some() => return Err(RelayError::Proto("interrupted fragmented deposit".into())),
                    other => other,
                };
                match frame {
                    ClientToRelay::Reserved11 | ClientToRelay::Reserved12 | ClientToRelay::Reserved13 | ClientToRelay::Reserved14 => {
                        send(&mut wr, &RelayToClient::Error("One-time message unavailable".into())).await?;
                    }
                    ClientToRelay::SendPartV1 { .. } => send(&mut wr, &RelayToClient::Error(ERR_NEEDS_AUTH_V2.into())).await?,
                    ClientToRelay::ViewOnceOfferV1(req) => {
                        let handler = store.view_once.read().unwrap_or_else(|e| e.into_inner()).clone();
                        offer = None;
                        parts = Default::default();
                        let ready = if owner && store.serves(&req.to) { handler.and_then(|issue| issue(sign_pk, req)) } else { None };
                        let frame = match ready {
                            Some((public, accept)) => {
                                offer = Some((req.id, accept));
                                RelayToClient::ViewOnceReadyV1 { offer: public }
                            }
                            None => RelayToClient::Error("One-time message unavailable".into()),
                        };
                        send(&mut wr, &frame).await?;
                    }
                    ClientToRelay::ViewOncePartV1 { index, total, bytes } => {
                        let frame = match offer.as_ref().map(|_| parts.push(index, total, &bytes)) {
                            Some(Ok(None)) => RelayToClient::ViewOncePartAckV1 { index },
                            Some(Ok(Some(envelope))) => {
                                match offer.take().and_then(|(id, accept)| accept(envelope).then_some(id)) {
                                    Some(id) => RelayToClient::ViewOnceStoredV1 { id },
                                    None => RelayToClient::Error("One-time message unavailable".into()),
                                }
                            }
                            _ => {
                                offer = None;
                                parts = Default::default();
                                RelayToClient::Error("One-time message unavailable".into())
                            }
                        };
                        send(&mut wr, &frame).await?;
                    }
                    ClientToRelay::Publish { .. } | ClientToRelay::Ack { .. } if !owner => {
                        send(&mut wr, &RelayToClient::Error(ERR_NEEDS_AUTH_V2.into())).await?;
                    }
                    ClientToRelay::Publish { bundle } => {
                        if let Err(e) = store.store_bundle(&sign_pk, &bundle) {
                            send(&mut wr, &RelayToClient::Error(e.to_string())).await?;
                        }
                    }
                    ClientToRelay::GetBundle { pk } => {
                        let bundle = store.get_bundle(&pk);
                        send(&mut wr, &RelayToClient::Bundle { pk, bundle }).await?;
                    }
                    ClientToRelay::Send { to, blob } => match store.deposit(&to, &blob) {
                        Ok(id) => {
                            let online = match connections.read().await.get(&to) {
                                Some(l) => l.pick().map(|tx| (tx, l.cursor.clone())),
                                None => None,
                            };
                            send(&mut wr, &RelayToClient::Deposited { id }).await?;
                            if let Some((tx, to_cursor)) = online {
                                let _ = try_enqueue_unclaimed(&tx, &to_cursor, id, blob).await;
                            }
                        }
                        // No Deposited: the sender keeps the message unacked and
                        // tries again elsewhere or later.
                        Err(e) => send(&mut wr, &RelayToClient::Error(e.to_string())).await?,
                    },
                    ClientToRelay::Ack { id } => {
                        store.ack(&sign_pk, id);
                        if given.remove(&id) {
                            unacked.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                        }
                        let cur = {
                            let mut c = cursor.lock().await;
                            c.acked(id);
                            c.high
                        };
                        for (mid, blob) in store.pending_above(&sign_pk, cur) {
                            match try_enqueue_unclaimed(push_tx, cursor, mid, blob).await {
                                EnqueueOutcome::Queued | EnqueueOutcome::AlreadyClaimed => {}
                                EnqueueOutcome::Backpressure => break,
                            }
                        }
                    }
                    ClientToRelay::Ping => send(&mut wr, &RelayToClient::Pong).await?,
                    ClientToRelay::Auth { .. } | ClientToRelay::AuthV2 { .. } | ClientToRelay::AuthFramedV1 { .. } | ClientToRelay::Dht(_) => {}
                }
            }
            push = push_rx.recv() => {
                let Some(msg) = push else { break };
                if let RelayToClient::Incoming { id, .. } = &msg {
                    if given.insert(*id) {
                        unacked.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                if framed {
                    if let RelayToClient::Incoming { id, from, blob } = &msg {
                        let pieces = crate::relay_deposit::push_parts(blob).map_err(|e| RelayError::Proto(e.into()))?;
                        let total = pieces.len() as u16;
                        let write = async {
                            for (index, bytes) in pieces.enumerate() {
                                send(&mut wr, &RelayToClient::IncomingPartV1 { id: *id, from: *from, index: index as u16, total, bytes }).await?;
                            }
                            Ok::<(), RelayError>(())
                        };
                        tokio::time::timeout(crate::relay_deposit::TIME_LIMIT, write).await.map_err(|_| RelayError::Proto("fragmented push timed out".into()))??;
                        continue;
                    }
                }
                send(&mut wr, &msg).await?;
            }
        }
    }
    Ok(())
}

/// Read one frame, handing the reader back with it (see `client_loop`).
async fn read_frame<R, T>(mut r: R) -> (R, Result<T, RelayError>)
where
    R: AsyncRead + Unpin,
    T: serde::de::DeserializeOwned,
{
    let res = recv_server_frame(&mut r).await;
    (r, res)
}

/// A running in-process relay: its own published destination on the router
/// in this process, memory-only storage, and the tasks serving both. Dropping it stops everything and
/// releases the destination.
pub struct EphemeralRelay {
    address: String,
    store: Arc<MemStore>,
    tasks: Vec<JoinHandle<()>>,
    /// Hops in this relay's inbound tunnels — the leg everyone writing to us
    /// travels. Shared with the accept loop, which reads it every time it
    /// opens a session.
    hops: Arc<AtomicU8>,
    /// Rung to make the accept loop drop the live session and open a new one
    /// at whatever `hops` now says.
    rebuild: Arc<tokio::sync::Notify>,
    /// What [`Self::connect_local`] serves its connections with.
    connections: Connections,
    destination_hash: [u8; 32],
    dht: Option<DhtHandler>,
}

impl EphemeralRelay {
    pub fn set_view_once_handler(&self, handler: ViewOnceHandler) {
        *self.store.view_once.write().unwrap_or_else(|e| e.into_inner()) = Some(handler);
    }
    /// Stop and join the owned accept/GC tasks before releasing this relay.
    /// The accept task drops its destination and aborts its nested client
    /// JoinSet. This does not join those client tasks or stop the global router.
    pub async fn shutdown(mut self) {
        for task in &self.tasks {
            task.abort();
        }
        for task in self.tasks.drain(..) {
            let _ = task.await;
        }
    }

    /// Public address for cards and group invitations, available even before
    /// the incoming tunnels are ready. Private key bytes stay in the vault.
    pub fn personal_address(db:&crate::Db) -> Result<String,NetError> {
        let key=db.terminal_key(i2p_embed::generate_keys).map_err(|_|NetError::I2p("personal terminal key unavailable".into()))?;
        let bytes=key.expose().map_err(|_|NetError::I2p("personal terminal key unavailable".into()))?;
        let key=std::str::from_utf8(&bytes).map_err(|_|NetError::I2p("invalid personal terminal key".into()))?;
        i2p_embed::public_of(key).map_err(|_|NetError::I2p("invalid personal terminal key".into()))
    }

    /// The profile's published terminal reopens with the same I2P private
    /// destination. Its identity proof is ready before the listener starts.
    pub async fn start_personal(
        db: &crate::Db,
        identity: Arc<crate::crypto::Identity>,
        dht: Option<DhtHandler>,
    ) -> Result<Self, NetError> {
        let key = db
            .terminal_key(i2p_embed::generate_keys)
            .map_err(|_| NetError::I2p("personal terminal key unavailable".into()))?;
        let store = MemStore::new(MemStoreLimits::personal(identity.card().sign_pk));
        *store
            .terminal_identity
            .write()
            .unwrap_or_else(|p| p.into_inner()) = Some(identity);
        Self::start_embedded(store, dht, key).await
    }

    async fn start_embedded(store: MemStore, dht: Option<DhtHandler>, private_key: crate::memory::SealedBlob) -> Result<Self, NetError> {
        let router = crate::embedded::running().ok_or(NetError::Closed)?;
        let hops = Arc::new(AtomicU8::new(crate::net::start_hops()));
        let rebuild = Arc::new(tokio::sync::Notify::new());
        let open = |hops: u8| {
            crate::embedded::destination_from_sealed(&router, &private_key, &crate::embedded::destination_options(true, hops))
        };
        let first = open(hops.load(Ordering::Relaxed))?;
        first.ready(Duration::from_secs(600)).await.map_err(|e| NetError::I2p(format!("relay tunnels: {e}")))?;
        let address = first.address().to_string();
        let store = Arc::new(store);
        let connections: Connections = Arc::default();
        let destination_hash = crate::relay::destination_hash(&address)
            .ok_or_else(|| NetError::I2p("relay destination does not decode".into()))?;
        let accept = tokio::spawn({
            let (store, connections, dht, hops, rebuild) = (store.clone(), connections.clone(), dht.clone(), hops.clone(), rebuild.clone());
            let router = router.clone();
            let private_key = private_key.clone();
            async move {
                let mut dest = Some(first);
                let mut clients = JoinSet::new();
                loop {
                    let Some(live) = dest.as_ref() else { break };
                    let mut inbound = live.accept();
                    loop {
                        while clients.try_join_next().is_some() {}
                        tokio::select! {
                            stream = inbound.recv() => {
                                let Some(stream) = stream else { break };
                                if clients.len() >= MAX_CLIENTS {
                                    crate::debug_trace!("[relay-server] {MAX_CLIENTS} streams open; closing a new one");
                                    continue;
                                }
                                let (store, connections) = (store.clone(), connections.clone());
                                let dht = dht.clone();
                                clients.spawn(async move {
                                    if let Err(e) = handle_client(stream, store, connections, destination_hash, dht).await {
                                        crate::debug_trace!("[relay-server] client gone: {e}");
                                    }
                                });
                            }
                            // The tunnel length changed: the old destination goes
                            // first (one key, one LeaseSet), then a new one at the
                            // new length on the same key.
                            () = rebuild.notified() => {
                                dest = None;
                                clients.abort_all();
                                let opts = crate::embedded::destination_options(true, hops.load(Ordering::Relaxed));
                                match crate::embedded::destination_from_sealed(&router, &private_key, &opts) {
                                    Ok(d) => {
                                        let _ = d.ready(Duration::from_secs(600)).await;
                                        dest = Some(d);
                                    }
                                    Err(e) => eprintln!("[relay-server] rebuild failed: {e}"),
                                }
                                break;
                            }
                        }
                    }
                }
            }
        });
        let gc = tokio::spawn({
            let store = store.clone();
            async move {
                let mut tick = tokio::time::interval(GC_INTERVAL);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                tick.tick().await;
                loop {
                    tick.tick().await;
                    store.gc();
                }
            }
        });
        Ok(Self { address, store, tasks: vec![accept, gc], hops, rebuild, connections, destination_hash, dht })
    }

    /// A connection to this relay from inside the process: its owner, the
    /// client in the same app or agent, collecting its own mail.
    ///
    /// The same `handle_client` as for anyone arriving over i2p, over a pipe
    /// instead of two tunnels. Going through i2p to a relay in the same
    /// process cost 10–15 s before the client counted as connected after
    /// every start, and one more pass through the network for every letter
    /// that arrived (e2e run 35955326759: relay-connect 11–13 s, RTT 42 s).
    /// The owner still logs in with `AuthV2` for this relay's address, so
    /// nothing about who may collect changes.
    pub fn connect_local(&self) -> std::pin::Pin<Box<dyn crate::net::DuplexStream>> {
        let (client, server) = tokio::io::duplex(1 << 20);
        let (store, connections) = (self.store.clone(), self.connections.clone());
        let dht = self.dht.clone();
        let destination_hash = self.destination_hash;
        tokio::spawn(async move {
            if let Err(e) = handle_client(server, store, connections, destination_hash, dht).await {
                crate::debug_trace!("[relay-server] local client gone: {e}");
            }
        });
        Box::pin(client)
    }

    /// This relay's destination, for handing to whoever should deposit here.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Hops in the tunnels people reach this relay through.
    pub fn hops(&self) -> u8 {
        self.hops.load(Ordering::Relaxed)
    }

    /// Ask for the tunnels to be rebuilt at a different length.
    ///
    /// Returns immediately; the accept loop drops its session and opens a new
    /// one at the next turn. A published destination has to build tunnels and
    /// republish its LeaseSet before anyone can find it again, so this relay is
    /// unreachable for a minute or two afterwards — letters sent meanwhile wait
    /// at their senders and arrive when it is back.
    pub fn set_hops(&self, hops: u8) {
        let hops = hops.clamp(crate::net::min_hops(), crate::net::DEFAULT_HOPS);
        if self.hops.swap(hops, Ordering::Relaxed) == hops {
            return;
        }
        crate::debug_trace!("[relay-server] rebuilding inbound tunnels at {hops} hops");
        self.rebuild.notify_one();
    }

    pub fn stats(&self) -> StoreStats {
        self.store.stats()
    }
}

impl Drop for EphemeralRelay {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Identity;
    use tokio::io::DuplexStream;

    fn small(total: usize, per_recipient: usize) -> MemStoreLimits {
        MemStoreLimits { max_total_bytes: total, max_recipient_bytes: per_recipient, ..Default::default() }
    }

    const A: [u8; 32] = [0xA; 32];
    const B: [u8; 32] = [0xB; 32];

    #[tokio::test]
    async fn explicit_relay_shutdown_joins_its_owned_tasks() {
        struct Owned(Arc<std::sync::atomic::AtomicUsize>);
        impl Drop for Owned {
            fn drop(&mut self) { self.0.fetch_add(1, Ordering::SeqCst); }
        }
        let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..2 {
            let owned = Owned(dropped.clone());
            let (started, running) = tokio::sync::oneshot::channel();
            tasks.push(tokio::spawn(async move {
                let _owned = owned;
                started.send(()).unwrap();
                std::future::pending::<()>().await;
            }));
            running.await.unwrap();
        }
        let relay = EphemeralRelay {
            address: String::new(),
            store: Arc::new(MemStore::unclaimed(MemStoreLimits::default())),
            tasks,
            hops: Arc::new(AtomicU8::new(3)),
            rebuild: Arc::new(tokio::sync::Notify::new()),
            connections: Arc::default(),
            destination_hash: [0; 32],
            dht: None,
        };
        relay.shutdown().await;
        assert_eq!(dropped.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn ids_increase_and_pending_is_ordered_above_the_cursor() {
        let s = MemStore::new(MemStoreLimits::default());
        let ids: Vec<u64> = (0..5u8).map(|i| s.deposit(&A, &[i]).unwrap()).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
        s.deposit(&B, b"other").unwrap();

        let all = s.pending_above(&A, 0);
        assert_eq!(all.iter().map(|(id, _)| *id).collect::<Vec<_>>(), ids);
        assert_eq!(all[0].1, vec![0]);

        let rest = s.pending_above(&A, ids[2]);
        assert_eq!(rest.iter().map(|(id, _)| *id).collect::<Vec<_>>(), ids[3..].to_vec());
        assert!(s.pending_above(&[0xC; 32], 0).is_empty());
    }

    #[test]
    fn pending_is_capped_per_batch() {
        let s = MemStore::new(MemStoreLimits::default());
        for _ in 0..(PENDING_LIMIT + 7) {
            s.deposit(&A, b"x").unwrap();
        }
        assert_eq!(s.pending_above(&A, 0).len(), PENDING_LIMIT);
    }

    #[test]
    fn only_the_recipient_can_ack() {
        let s = MemStore::new(MemStoreLimits::default());
        let id = s.deposit(&A, b"for a").unwrap();
        assert!(!s.ack(&B, id), "a stranger's ack must not delete");
        assert_eq!(s.pending_above(&A, 0).len(), 1);
        assert!(s.ack(&A, id));
        assert!(s.pending_above(&A, 0).is_empty());
        assert!(!s.ack(&A, id), "acking twice is a no-op");
        assert_eq!(s.stats(), StoreStats { bundles: 0, messages: 0, message_bytes: 0 });
    }

    #[test]
    fn recipient_cap_evicts_that_recipients_oldest_only() {
        let s = MemStore::new(small(1000, 10));
        let b_id = s.deposit(&B, &[0; 4]).unwrap();
        let first = s.deposit(&A, &[1; 4]).unwrap();
        let second = s.deposit(&A, &[2; 4]).unwrap();
        let third = s.deposit(&A, &[3; 4]).unwrap(); // A now 12 > 10
        let a: Vec<u64> = s.pending_above(&A, 0).into_iter().map(|(id, _)| id).collect();
        assert_eq!(a, vec![second, third]);
        assert!(!a.contains(&first));
        assert_eq!(s.pending_above(&B, 0)[0].0, b_id, "another inbox is untouched");
    }

    #[test]
    fn total_cap_evicts_globally_oldest() {
        let cap = accounted_message_bytes(4) * 2 + 1;
        let s = MemStore::new(MemStoreLimits { max_total_bytes: cap, max_recipient_bytes: 10, ..Default::default() });
        let a1 = s.deposit(&A, &[0; 4]).unwrap();
        let b1 = s.deposit(&B, &[0; 4]).unwrap();
        let a2 = s.deposit(&A, &[0; 4]).unwrap(); // total 12 > 10
        assert!(s.pending_above(&A, 0).iter().all(|(id, _)| *id != a1));
        assert_eq!(s.pending_above(&B, 0)[0].0, b1);
        assert_eq!(s.pending_above(&A, 0)[0].0, a2);
        assert_eq!(s.stats().message_bytes, 8);
    }

    #[test]
    fn oversize_is_refused_not_stored() {
        let s = MemStore::new(small(100, 10));
        assert_eq!(s.deposit(&A, &[0; 11]), Err(StoreError::TooLarge));
        assert_eq!(s.deposit(&A, &[]), Err(StoreError::InvalidBlob));
        assert_eq!(s.stats().messages, 0);
        let big = vec![0u8; MemStoreLimits::default().max_bundle_bytes + 1];
        assert_eq!(s.store_bundle(&A, &big), Err(StoreError::TooLarge));
    }

    #[test]
    fn total_message_count_cap_limits_many_small_messages() {
        let s = MemStore::new(MemStoreLimits {
            max_total_bytes: 1 << 20,
            max_recipient_bytes: 1 << 20,
            max_total_messages: 3,
            ..Default::default()
        });
        let ids: Vec<u64> = (0..5).map(|_| s.deposit(&A, b"x").unwrap()).collect();
        let kept: Vec<u64> = s.pending_above(&A, 0).into_iter().map(|(id, _)| id).collect();
        assert_eq!(kept, ids[2..].to_vec(), "oldest entries are evicted first");
        assert_eq!(s.stats().messages, 3);
    }

    #[test]
    fn metadata_counts_toward_total_byte_cap() {
        let cost = accounted_message_bytes(1);
        let s = MemStore::new(MemStoreLimits {
            max_total_bytes: cost * 2 + 1,
            max_recipient_bytes: 1024,
            ..Default::default()
        });
        let first = s.deposit(&A, b"a").unwrap();
        let _second = s.deposit(&A, b"b").unwrap();
        let _third = s.deposit(&A, b"c").unwrap();
        let ids: Vec<u64> = s.pending_above(&A, 0).into_iter().map(|(id, _)| id).collect();
        assert!(!ids.contains(&first), "oldest entry was evicted by metadata-aware byte cap");
        assert_eq!(s.stats().messages, 2);
        assert_eq!(s.stats().message_bytes, 2, "payload stats remain payload-only");
    }

    #[test]
    fn gc_expires_old_messages_and_bundles() {
        let limits = MemStoreLimits::default();
        let s = MemStore::new(limits.clone());
        s.deposit(&A, b"old").unwrap();
        s.store_bundle(&A, b"bundle").unwrap();
        s.gc_at(Instant::now());
        assert_eq!(s.stats().messages, 1);
        s.gc_at(Instant::now() + limits.message_ttl + Duration::from_secs(1));
        assert_eq!(s.stats().messages, 0);
        assert_eq!(s.stats().bundles, 1, "bundles outlive messages");
        s.gc_at(Instant::now() + limits.bundle_ttl + Duration::from_secs(1));
        assert_eq!(s.stats().bundles, 0);
    }

    #[test]
    fn bundle_cap_evicts_the_stalest() {
        let s = MemStore::new(MemStoreLimits { max_bundles: 2, ..Default::default() });
        s.store_bundle(&A, b"a").unwrap();
        s.store_bundle(&B, b"b").unwrap();
        s.store_bundle(&A, b"a2").unwrap(); // refresh, no eviction
        s.store_bundle(&[0xC; 32], b"c").unwrap(); // evicts B, the stalest
        assert_eq!(s.get_bundle(&A), Some(b"a2".to_vec()));
        assert_eq!(s.get_bundle(&B), None);
        assert_eq!(s.stats().bundles, 2);
    }

    // ── the server over an in-memory pipe ────────────────────────────────────

    const RIG_DESTINATION: [u8; 32] = [0x5A; 32];

    struct Rig {
        store: Arc<MemStore>,
        connections: Connections,
        dht: Option<DhtHandler>,
    }

    impl Rig {
        fn new() -> Self {
            Self { store: Arc::new(MemStore::new(MemStoreLimits::default())), connections: Arc::default(), dht: None }
        }

        fn open(&self) -> (DuplexStream, JoinHandle<Result<(), RelayError>>) {
            self.open_with(1 << 20)
        }

        fn open_with(&self, buffer: usize) -> (DuplexStream, JoinHandle<Result<(), RelayError>>) {
            let (client, server) = tokio::io::duplex(buffer);
            let task = tokio::spawn(handle_client(server, self.store.clone(), self.connections.clone(), RIG_DESTINATION, self.dht.clone()));
            (client, task)
        }

        async fn login(&self, who: &Identity) -> DuplexStream {
            self.login_buffered(who, 1 << 20).await
        }

        async fn login_buffered(&self, who: &Identity, buffer: usize) -> DuplexStream {
            let (mut c, _task) = self.open_with(buffer);
            let RelayToClient::Challenge(ch) = recv(&mut c).await.unwrap() else { panic!("no challenge") };
            let signature = who.sign(&crate::relay::auth_v2_message(&RIG_DESTINATION, &ch));
            let auth = ClientToRelay::AuthV2 { sign_pk: who.card().sign_pk, signature };
            send(&mut c, &auth).await.unwrap();
            assert!(matches!(recv::<_, RelayToClient>(&mut c).await.unwrap(), RelayToClient::AuthOk));
            c
        }
    }

    async fn next(c: &mut DuplexStream) -> RelayToClient {
        tokio::time::timeout(Duration::from_secs(5), recv(c)).await.expect("frame within 5s").unwrap()
    }

    #[tokio::test]
    async fn partial_deposit_is_invisible_and_confirmed_ciphertext_reaches_framed_and_legacy_receivers(
    ) {
        let rig = Rig::new();
        let sender = Arc::new(Identity::generate());
        let receiver = Arc::new(Identity::generate());
        let to = receiver.card().sign_pk;
        *rig.store.terminal_identity.write().unwrap() = Some(receiver.clone());
        let mut interrupted = rig.login(&sender).await;
        send(
            &mut interrupted,
            &ClientToRelay::SendPartV1 {
                to,
                index: 0,
                total: 2,
                bytes: vec![4; 128],
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            next(&mut interrupted).await,
            RelayToClient::SendPartAckV1 { index: 0 }
        ));
        assert_eq!(rig.store.stats().messages, 0);
        drop(interrupted);

        // The receiver can still collect malformed opaque items left by an old
        // sender and proceed to the next deposit on that same signed stream.
        let mut legacy = rig.login(&sender).await;
        send(&mut legacy, &ClientToRelay::Send { to, blob: vec![42] })
            .await
            .unwrap();
        assert!(matches!(next(&mut legacy).await, RelayToClient::Deposited { id } if id > 0));
        drop(legacy);
        let address = "ljnfuws2ljnfuws2ljnfuws2ljnfuws2ljnfuws2ljnfuws2ljna.b32.i2p";
        let recipient = crate::relay::connect_local(Box::pin(rig.open().0), address, &receiver)
            .await
            .unwrap();
        async fn incoming(client: &crate::relay::RelayClient) -> (u64, Vec<u8>) {
            let mut rx = client.in_rx.lock().await;
            let item = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap();
            let RelayToClient::Incoming { id, blob, .. } = item else {
                panic!("expected reconstructed deposit")
            };
            (id, blob)
        }
        let (id, tiny) = incoming(&recipient).await;
        assert_eq!(tiny, [42]);
        recipient
            .out_tx
            .send(ClientToRelay::Ack { id })
            .await
            .unwrap();
        let mut stream = rig.open().0;
        assert!(crate::relay::authenticate_terminal(
            &mut stream,
            address,
            &sender,
            &sender.card().sign_pk
        )
        .await
        .is_err());
        let mut stream = rig.open().0;
        crate::relay::authenticate_terminal(&mut stream, address, &sender, &to)
            .await
            .unwrap();
        let client = crate::relay::sender::start(Box::pin(stream), to, address);
        let blob = vec![17; crate::relay_deposit::PART_BYTES * 2 + 5];
        client.out_tx.deposit(to, blob.clone(), None).await.unwrap();
        let (_, collected) = incoming(&recipient).await;
        assert_eq!(collected, blob);
        assert!(client
            .out_tx
            .deposit(sender.card().sign_pk, vec![8, 9], None)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn guarded_outgoing_letter_waits_for_the_pinned_personal_terminal_and_fresh_offer() {
        use crate::view_once::{Offer,QueuedText,Request};
        let address="ljnfuws2ljnfuws2ljnfuws2ljnfuws2ljnfuws2ljnfuws2ljna.b32.i2p";
        let rig=Rig::new();
        let alice=Identity::generate();
        let bob=Arc::new(Identity::generate());
        let peer=bob.card();
        let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
        let request=Request::new(peer.sign_pk,now+60_000);
        let initial=vec![b'z';crate::view_once::MAX_TEXT];
        let queued=QueuedText::wrap(&request.id,&initial).unwrap();
        let releases=std::sync::atomic::AtomicUsize::new(0);
        let expose=|| { releases.fetch_add(1,std::sync::atomic::Ordering::SeqCst); queued.open(&request.id).ok() };
        let attempt=||crate::relay::send_view_once_on(rig.open().0,address,&alice,&peer,request.fresh_attempt(),expose);
        // Unavailable recipient, then a terminal at the expected address that
        // belongs to another profile. Neither can release the sender's buffer.
        assert!(attempt().await.is_err());
        *rig.store.terminal_identity.write().unwrap()=Some(Arc::new(Identity::generate()));
        assert!(attempt().await.is_err());
        *rig.store.terminal_identity.write().unwrap()=Some(bob.clone());
        *rig.store.view_once.write().unwrap()=Some(Arc::new(|_,_|Some((vec![0;50],Box::new(|_|false)))));
        assert!(attempt().await.is_err());
        assert_eq!(releases.load(std::sync::atomic::Ordering::SeqCst),0);
        let unseen=Arc::new(std::sync::Mutex::new(None));
        *rig.store.view_once.write().unwrap()=Some(Arc::new({
            let bob=bob.clone();let sender=alice.card();let unseen=unseen.clone();
            move |from,req|{
                if from!=sender.sign_pk || req.id!=request.id || req.nonce==request.nonce {return None;}
                let (offer,cert)=Offer::issue(&bob,sender.clone(),req,now).ok()?;
                let unseen=unseen.clone();
                Some((cert,Box::new(move|bytes|{
                    let Ok(letter)=offer.accept(&bytes,now) else{return false;};
                    *unseen.lock().unwrap()=Some(letter);
                    true
                })))
            }
        }));
        attempt().await.unwrap();
        assert_eq!(releases.load(std::sync::atomic::Ordering::SeqCst),1);
        assert_eq!(rig.store.stats().messages,0);
        let received=unseen.lock().unwrap().take().unwrap().open(&bob,now).unwrap();
        assert_eq!(&**received,&initial);
    }

    #[tokio::test]
    async fn view_once_personal_relay_consumes_the_authenticated_offer_without_deposit() {
        let rig = Rig::new();
        let sender = Identity::generate();
        let receiver = Arc::new(Identity::generate());
        let req = crate::view_once::Request::new(receiver.card().sign_pk, 5_000);
        let mut c = rig.login(&sender).await;
        send(&mut c, &ClientToRelay::ViewOnceOfferV1(req))
            .await
            .unwrap();
        assert!(matches!(next(&mut c).await, RelayToClient::Error(_)));
        let mailbox = Arc::new(std::sync::Mutex::new(None));
        *rig.store.view_once.write().unwrap() = Some(Arc::new({
            let peer = sender.card();
            let receiver = receiver.clone();
            let mailbox = mailbox.clone();
            move |from, req| {
                if from != peer.sign_pk {
                    return None;
                }
                let (offer, bytes) =
                    crate::view_once::Offer::issue(&receiver, peer.clone(), req, 1000).ok()?;
                let mailbox = mailbox.clone();
                Some((
                    bytes,
                    Box::new(move |payload| {
                        let Ok(letter) = offer.accept(&payload, 2000) else {
                            return false;
                        };
                        *mailbox.lock().unwrap() = Some(letter);
                        true
                    }),
                ))
            }
        }));
        send(&mut c, &ClientToRelay::ViewOnceOfferV1(req))
            .await
            .unwrap();
        let RelayToClient::ViewOnceReadyV1 { offer } = next(&mut c).await else {
            panic!("no certified offer");
        };
        let body = vec![b'x'; crate::view_once::MAX_TEXT];
        let cipher =
            crate::view_once::seal(&sender, &receiver.card(), &req, &offer, &body, 1500).unwrap();
        let parts = crate::view_once::parts(&cipher);
        let total = parts.len() as u16;
        for (i, part) in parts.enumerate() {
            let index = i as u16;
            send(
                &mut c,
                &ClientToRelay::ViewOncePartV1 {
                    index,
                    total,
                    bytes: part.to_vec(),
                },
            )
            .await
            .unwrap();
            let ack = next(&mut c).await;
            if index + 1 == total {
                assert!(matches!(ack,RelayToClient::ViewOnceStoredV1{id} if id==req.id));
            } else {
                assert!(matches!(ack,RelayToClient::ViewOncePartAckV1{index:i} if i==index));
            }
        }
        assert_eq!(rig.store.stats().messages, 0);
        assert_eq!(
            mailbox
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .open(&receiver, 3000)
                .unwrap()
                .as_slice(),
            body
        );
        send(
            &mut c,
            &ClientToRelay::ViewOncePartV1 {
                index: 0,
                total: 2,
                bytes: vec![1; 20],
            },
        )
        .await
        .unwrap();
        assert!(matches!(next(&mut c).await, RelayToClient::Error(_)));
    }

    /// Every `Incoming` a connection has within `wait`, by id.
    async fn drain(c: &mut DuplexStream, wait: Duration) -> Vec<u64> {
        let mut ids = Vec::new();
        while let Ok(Ok(frame)) = tokio::time::timeout(wait, recv::<_, RelayToClient>(c)).await {
            if let RelayToClient::Incoming { id, .. } = frame {
                ids.push(id);
            }
        }
        ids
    }

    #[tokio::test]
    async fn a_second_connection_shares_the_mail_without_doubling_it() {
        let rig = Rig::new();
        let (bob, alice) = (Identity::generate(), Identity::generate());
        let mut b1 = rig.login(&bob).await;
        let mut b2 = rig.login(&bob).await;
        let mut a = rig.login(&alice).await;
        for i in 0..8u8 {
            send(&mut a, &ClientToRelay::Send { to: bob.card().sign_pk, blob: vec![i; 10] }).await.unwrap();
            assert!(matches!(next(&mut a).await, RelayToClient::Deposited { .. }));
        }
        let (got1, got2) = (drain(&mut b1, Duration::from_millis(300)).await, drain(&mut b2, Duration::from_millis(300)).await);
        assert!(!got1.is_empty() && !got2.is_empty(), "dealt round both: {got1:?} {got2:?}");
        let mut all: Vec<u64> = got1.iter().chain(&got2).copied().collect();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 8);
        assert_eq!(got1.len() + got2.len(), 8, "nothing pushed twice");
    }

    #[tokio::test]
    async fn when_the_last_connection_goes_the_unacked_come_again() {
        let rig = Rig::new();
        let (bob, alice) = (Identity::generate(), Identity::generate());
        let mut a = rig.login(&alice).await;
        {
            let mut b1 = rig.login(&bob).await;
            let mut b2 = rig.login(&bob).await;
            for i in 0..4u8 {
                send(&mut a, &ClientToRelay::Send { to: bob.card().sign_pk, blob: vec![i; 10] }).await.unwrap();
                assert!(matches!(next(&mut a).await, RelayToClient::Deposited { .. }));
            }
            let n = drain(&mut b1, Duration::from_millis(300)).await.len() + drain(&mut b2, Duration::from_millis(300)).await.len();
            assert_eq!(n, 4);
            // Neither acked; both go away.
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut b3 = rig.login(&bob).await;
        assert_eq!(drain(&mut b3, Duration::from_millis(300)).await.len(), 4, "all four again, from the start");
    }

    #[tokio::test]
    async fn what_a_closed_connection_left_unacked_goes_to_the_others() {
        let rig = Rig::new();
        let (bob, alice) = (Identity::generate(), Identity::generate());
        let mut b1 = rig.login(&bob).await;
        let mut b2 = rig.login(&bob).await;
        let mut a = rig.login(&alice).await;
        for i in 0..6u8 {
            send(&mut a, &ClientToRelay::Send { to: bob.card().sign_pk, blob: vec![i; 10] }).await.unwrap();
            assert!(matches!(next(&mut a).await, RelayToClient::Deposited { .. }));
        }
        let lost = drain(&mut b1, Duration::from_millis(300)).await;
        let got2 = drain(&mut b2, Duration::from_millis(300)).await;
        assert!(!lost.is_empty() && !got2.is_empty());
        for id in &got2 {
            send(&mut b2, &ClientToRelay::Ack { id: *id }).await.unwrap();
        }
        // b1 goes without acking; b2 stays open all along.
        drop(b1);
        let mut again = drain(&mut b2, Duration::from_millis(500)).await;
        again.sort_unstable();
        let mut lost = lost;
        lost.sort_unstable();
        assert_eq!(again, lost, "b1's share comes to b2, and nothing else twice");
    }

    #[tokio::test]
    async fn acking_while_mail_keeps_coming_pushes_nothing_twice() {
        let rig = Rig::new();
        let (bob, alice) = (Identity::generate(), Identity::generate());
        let mut b = rig.login(&bob).await;
        let mut a = rig.login(&alice).await;
        const N: u8 = 60;
        let sender = tokio::spawn(async move {
            for i in 0..N {
                send(&mut a, &ClientToRelay::Send { to: bob.card().sign_pk, blob: vec![i; 10] }).await.unwrap();
                assert!(matches!(next(&mut a).await, RelayToClient::Deposited { .. }));
            }
        });
        // Bob acks each as it comes, so every deposit races an Ack's sweep.
        let mut got = Vec::new();
        while let Ok(Ok(frame)) = tokio::time::timeout(Duration::from_millis(500), recv::<_, RelayToClient>(&mut b)).await {
            if let RelayToClient::Incoming { id, .. } = frame {
                got.push(id);
                send(&mut b, &ClientToRelay::Ack { id }).await.unwrap();
            }
        }
        sender.await.unwrap();
        let mut unique = got.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), N as usize, "every letter came");
        assert_eq!(got.len(), N as usize, "none came twice: {got:?}");
    }

    #[tokio::test]
    async fn a_deposit_slow_to_confirm_is_not_pushed_twice() {
        let rig = Rig::new();
        let (bob, alice) = (Identity::generate(), Identity::generate());
        let mut b = rig.login(&bob).await;
        // Alice's side takes two Deposited frames and then nothing, as an
        // i2p stream with a full window would: the third deposit waits on
        // its answer, with the letter already stored.
        let a = rig.login_buffered(&alice, 40).await;
        let to = bob.card().sign_pk;
        let (mut ar, mut aw) = tokio::io::split(a);
        let sender = tokio::spawn(async move {
            for i in 0..3u8 {
                send(&mut aw, &ClientToRelay::Send { to, blob: vec![i; 10] }).await.unwrap();
            }
            aw
        });
        let first = match next(&mut b).await { RelayToClient::Incoming { id, .. } => id, f => panic!("{f:?}") };
        tokio::time::sleep(Duration::from_millis(200)).await;
        // Bob's Ack sweeps while the third deposit is still answering Alice.
        send(&mut b, &ClientToRelay::Ack { id: first }).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        for _ in 0..3 {
            let _: RelayToClient = tokio::time::timeout(Duration::from_secs(5), recv(&mut ar)).await.unwrap().unwrap();
        }
        let _aw = sender.await.unwrap();
        let mut got = vec![first];
        got.extend(drain(&mut b, Duration::from_millis(300)).await);
        assert_eq!(got.len(), 3, "each once: {got:?}");
    }

    #[tokio::test]
    async fn a_silent_connection_is_dropped_and_its_share_goes_to_the_others() {
        let rig = Rig::new();
        let (bob, alice) = (Identity::generate(), Identity::generate());
        // b1 goes quiet as a client whose close never arrived; b2 pings.
        let mut b1 = rig.login(&bob).await;
        let mut b2 = rig.login(&bob).await;
        let mut a = rig.login(&alice).await;
        for i in 0..2u8 {
            send(&mut a, &ClientToRelay::Send { to: bob.card().sign_pk, blob: vec![i; 10] }).await.unwrap();
            assert!(matches!(next(&mut a).await, RelayToClient::Deposited { .. }));
        }
        let to_b1 = drain(&mut b1, Duration::from_millis(200)).await;
        let to_b2 = drain(&mut b2, Duration::from_millis(200)).await;
        assert_eq!((to_b1.len(), to_b2.len()), (1, 1), "dealt round both");
        send(&mut b2, &ClientToRelay::Ack { id: to_b2[0] }).await.unwrap();
        let mut got = Vec::new();
        let deadline = tokio::time::Instant::now() + CLIENT_SILENT + Duration::from_secs(1);
        while tokio::time::Instant::now() < deadline {
            send(&mut b2, &ClientToRelay::Ping).await.unwrap();
            got.extend(drain(&mut b2, Duration::from_millis(300)).await);
        }
        assert_eq!(got, to_b1, "b1's letter came to b2 once b1 was dropped");
        // Alice kept quiet too, and was dropped the same way: she comes back.
        let mut a = rig.login(&alice).await;
        for i in 2..5u8 {
            send(&mut a, &ClientToRelay::Send { to: bob.card().sign_pk, blob: vec![i; 10] }).await.unwrap();
            assert!(matches!(next(&mut a).await, RelayToClient::Deposited { .. }));
        }
        assert_eq!(drain(&mut b2, Duration::from_millis(300)).await.len(), 3, "all new mail to the live one");
    }

    #[tokio::test]
    async fn a_connection_that_does_not_ack_stops_getting_new_mail() {
        let rig = Rig::new();
        let (bob, alice) = (Identity::generate(), Identity::generate());
        // b1 takes pushes and never acks (its pushes go nowhere, as down a
        // stream broken one way); b2 acks each as it comes.
        let mut b1 = rig.login(&bob).await;
        let mut b2 = rig.login(&bob).await;
        let mut a = rig.login(&alice).await;
        let mut to_b2 = 0;
        for i in 0..20u8 {
            send(&mut a, &ClientToRelay::Send { to: bob.card().sign_pk, blob: vec![i; 10] }).await.unwrap();
            assert!(matches!(next(&mut a).await, RelayToClient::Deposited { .. }));
            for id in drain(&mut b2, Duration::from_millis(50)).await {
                send(&mut b2, &ClientToRelay::Ack { id }).await.unwrap();
                to_b2 += 1;
            }
        }
        let to_b1 = drain(&mut b1, Duration::from_millis(200)).await.len();
        assert_eq!(to_b1 + to_b2, 20);
        assert!(to_b1 <= 2, "the one not acking got {to_b1} of 20");
    }

    #[tokio::test]
    async fn a_push_while_a_frame_is_half_read_loses_nothing() {
        use tokio::io::AsyncWriteExt;
        let rig = Rig::new();
        let (bob, alice) = (Identity::generate(), Identity::generate());
        let mut b = rig.login(&bob).await;
        let mut a = rig.login(&alice).await;
        // Bob starts a large frame and stops half-way ...
        let frame = bincode::serialize(&ClientToRelay::Publish { bundle: vec![9; 8000] }).unwrap();
        b.write_all(&(frame.len() as u32).to_be_bytes()).await.unwrap();
        b.write_all(&frame[..4000]).await.unwrap();
        b.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        // ... a letter for him arrives and is pushed meanwhile ...
        send(&mut a, &ClientToRelay::Send { to: bob.card().sign_pk, blob: vec![1; 10] }).await.unwrap();
        assert!(matches!(next(&mut a).await, RelayToClient::Deposited { .. }));
        assert!(matches!(next(&mut b).await, RelayToClient::Incoming { .. }));
        // ... and the rest of his frame still parses: the connection lives.
        b.write_all(&frame[4000..]).await.unwrap();
        send(&mut b, &ClientToRelay::Ping).await.unwrap();
        loop {
            match next(&mut b).await {
                RelayToClient::Pong => break,
                RelayToClient::Error(e) => panic!("relay error: {e}"),
                _ => {}
            }
        }
        assert!(rig.store.get_bundle(&bob.card().sign_pk).is_some(), "the half-read Publish arrived whole");
    }

    #[tokio::test]
    async fn inbound_budget_blocks_second_advertised_frame_until_first_releases() {
        use tokio::io::AsyncWriteExt;
        let budget = InboundFrameBudget::new(1024, 1024);
        let (mut w_hold, mut r_hold) = tokio::io::duplex(64);
        let (mut w_wait, mut r_wait) = tokio::io::duplex(64);
        let body = bincode::serialize(&ClientToRelay::Ping).unwrap();
        let len = (body.len() as u32).to_be_bytes();

        // First reader takes the whole budget and blocks on body bytes.
        w_hold.write_all(&len).await.unwrap();
        w_hold.flush().await.unwrap();
        let holder_budget = budget.clone();
        let t_hold = tokio::spawn(async move {
            recv_server_frame_with_budget::<_, ClientToRelay>(&mut r_hold, &holder_budget).await
        });

        tokio::time::timeout(Duration::from_secs(1), async {
            while budget.available_units() != 0 {
                tokio::task::yield_now().await;
            }
        }).await.expect("holder must acquire budget");

        // Second reader has a complete frame ready, but must wait for budget.
        w_wait.write_all(&len).await.unwrap();
        w_wait.write_all(&body).await.unwrap();
        w_wait.flush().await.unwrap();
        let waiter_budget = budget.clone();
        let t_wait = tokio::spawn(async move {
            recv_server_frame_with_budget::<_, ClientToRelay>(&mut r_wait, &waiter_budget).await
        });

        assert!(tokio::time::timeout(Duration::from_millis(100), async {
            while !t_wait.is_finished() {
                tokio::task::yield_now().await;
            }
        }).await.is_err(), "second reader should stay blocked while budget is held");

        t_hold.abort();
        let _ = t_hold.await;

        let got = tokio::time::timeout(Duration::from_secs(1), t_wait).await.unwrap().unwrap().unwrap();
        assert!(matches!(got, ClientToRelay::Ping));
    }

    #[tokio::test]
    async fn inbound_budget_is_released_when_read_times_out() {
        use tokio::io::AsyncWriteExt;
        let budget = InboundFrameBudget::new(1024, 1024);
        let (mut w, mut r) = tokio::io::duplex(64);
        let body = bincode::serialize(&ClientToRelay::Ping).unwrap();
        w.write_all(&(body.len() as u32).to_be_bytes()).await.unwrap();
        w.flush().await.unwrap();

        let timed = tokio::time::timeout(Duration::from_millis(50), recv_server_frame_with_budget::<_, ClientToRelay>(&mut r, &budget)).await;
        assert!(timed.is_err(), "body read should time out");
        assert_eq!(budget.available_units(), 1, "budget must be released after timeout cancellation");

        let (mut w2, mut r2) = tokio::io::duplex(64);
        w2.write_all(&(body.len() as u32).to_be_bytes()).await.unwrap();
        w2.write_all(&body).await.unwrap();
        w2.flush().await.unwrap();
        let got = tokio::time::timeout(Duration::from_secs(1), recv_server_frame_with_budget::<_, ClientToRelay>(&mut r2, &budget)).await.unwrap().unwrap();
        assert!(matches!(got, ClientToRelay::Ping));
    }

    #[tokio::test]
    async fn wrong_signature_is_refused() {
        let rig = Rig::new();
        let (mut c, task) = rig.open();
        let RelayToClient::Challenge(ch) = recv(&mut c).await.unwrap() else { panic!() };
        let me = Identity::generate();
        let other = Identity::generate();
        // Claims my key, signed by someone else.
        send(&mut c, &ClientToRelay::Auth { sign_pk: me.card().sign_pk, signature: other.sign(&ch) }).await.unwrap();
        assert!(matches!(next(&mut c).await, RelayToClient::AuthFail));
        assert!(matches!(task.await.unwrap(), Err(RelayError::AuthFailed)));
        assert!(rig.connections.read().await.is_empty());
    }

    #[tokio::test]
    async fn anything_but_auth_first_is_refused() {
        let rig = Rig::new();
        let (mut c, task) = rig.open();
        let _challenge: RelayToClient = recv(&mut c).await.unwrap();
        send(&mut c, &ClientToRelay::Ping).await.unwrap();
        assert!(matches!(task.await.unwrap(), Err(RelayError::Proto(_))));
    }

    #[tokio::test]
    async fn a_relay_network_connection_is_answered_but_never_logged_in() {
        let mut rig = Rig::new();
        rig.dht = Some(Arc::new(|challenge: &[u8; 32], req: Vec<u8>| [&challenge[..1], &req[..]].concat()));
        let (mut c, task) = rig.open();
        let RelayToClient::Challenge(ch) = recv(&mut c).await.unwrap() else { panic!() };
        send(&mut c, &ClientToRelay::Dht(vec![1, 2])).await.unwrap();
        let RelayToClient::Dht(a) = next(&mut c).await else { panic!("no dht answer") };
        assert_eq!(a, vec![ch[0], 1, 2]);
        send(&mut c, &ClientToRelay::Dht(vec![3])).await.unwrap();
        assert!(matches!(next(&mut c).await, RelayToClient::Dht(_)));
        // The same connection cannot turn into a mailbox one.
        send(&mut c, &ClientToRelay::Ack { id: 1 }).await.unwrap();
        assert!(matches!(task.await.unwrap(), Err(RelayError::Proto(_))));
        assert!(rig.connections.read().await.is_empty());
    }

    #[tokio::test]
    async fn a_relay_without_a_node_refuses_relay_network_requests() {
        let rig = Rig::new();
        let (mut c, task) = rig.open();
        let _challenge: RelayToClient = recv(&mut c).await.unwrap();
        send(&mut c, &ClientToRelay::Dht(vec![1])).await.unwrap();
        assert!(matches!(next(&mut c).await, RelayToClient::Error(_)));
        assert!(task.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn online_recipient_gets_the_message_pushed() {
        let rig = Rig::new();
        let (alice, bob) = (Identity::generate(), Identity::generate());
        let mut b = rig.login(&bob).await;
        let mut a = rig.login(&alice).await;

        let to = bob.card().sign_pk;
        send(&mut a, &ClientToRelay::Send { to, blob: b"hello".to_vec() }).await.unwrap();
        let RelayToClient::Deposited { id } = next(&mut a).await else { panic!("no Deposited") };
        match next(&mut b).await {
            RelayToClient::Incoming { id: got, blob, .. } => {
                assert_eq!(got, id);
                assert_eq!(blob, b"hello");
            }
            other => panic!("expected Incoming, got {other:?}"),
        }
        send(&mut b, &ClientToRelay::Ack { id }).await.unwrap();
        send(&mut b, &ClientToRelay::Ping).await.unwrap();
        assert!(matches!(next(&mut b).await, RelayToClient::Pong));
        assert_eq!(rig.store.stats().messages, 0);
    }

    #[tokio::test]
    async fn a_login_signed_for_another_relay_is_refused() {
        let rig = Rig::new();
        let (mut c, task) = rig.open();
        let RelayToClient::Challenge(ch) = recv(&mut c).await.unwrap() else { panic!() };
        let bob = Identity::generate();
        // A relay that passed us its challenge gets a signature naming itself.
        let signature = bob.sign(&crate::relay::auth_v2_message(&[0x77; 32], &ch));
        send(&mut c, &ClientToRelay::AuthV2 { sign_pk: bob.card().sign_pk, signature }).await.unwrap();
        assert!(matches!(next(&mut c).await, RelayToClient::AuthFail));
        assert!(matches!(task.await.unwrap(), Err(RelayError::AuthFailed)));
    }

    #[tokio::test]
    async fn a_plain_auth_login_may_deposit_but_not_collect() {
        let rig = Rig::new();
        let (alice, bob) = (Identity::generate(), Identity::generate());
        let mut a = rig.login(&alice).await;
        send(&mut a, &ClientToRelay::Send { to: bob.card().sign_pk, blob: b"for bob".to_vec() }).await.unwrap();
        let RelayToClient::Deposited { id } = next(&mut a).await else { panic!() };

        // Bob's signature over a bare challenge, as a relay in the middle
        // would hold it after passing this relay's challenge on.
        let (mut b, _task) = rig.open();
        let RelayToClient::Challenge(ch) = recv(&mut b).await.unwrap() else { panic!() };
        send(&mut b, &ClientToRelay::Auth { sign_pk: bob.card().sign_pk, signature: bob.sign(&ch) }).await.unwrap();
        assert!(matches!(next(&mut b).await, RelayToClient::AuthOk));

        send(&mut b, &ClientToRelay::Ack { id }).await.unwrap();
        assert!(matches!(next(&mut b).await, RelayToClient::Error(e) if e == ERR_NEEDS_AUTH_V2));
        send(&mut b, &ClientToRelay::Publish { bundle: b"forged".to_vec() }).await.unwrap();
        assert!(matches!(next(&mut b).await, RelayToClient::Error(e) if e == ERR_NEEDS_AUTH_V2));
        // Nothing was pushed to it, the mail is still there, and depositing works.
        send(&mut b, &ClientToRelay::Send { to: alice.card().sign_pk, blob: b"hi".to_vec() }).await.unwrap();
        assert!(matches!(next(&mut b).await, RelayToClient::Deposited { .. }));
        assert_eq!(rig.store.stats().messages, 2);
        assert_eq!(rig.store.stats().bundles, 0);
        assert!(!rig.connections.read().await.contains_key(&bob.card().sign_pk));
    }

    #[test]
    fn the_refusal_text_is_the_one_clients_look_for() {
        assert_eq!(StoreError::NotServed.to_string(), crate::relay::ERR_NOT_SERVED);
    }

    #[test]
    fn an_unclaimed_store_takes_nothing_until_claimed() {
        let (owner, stranger) = ([1u8; 32], [2u8; 32]);
        let store = MemStore::unclaimed(MemStoreLimits::default());
        assert_eq!(store.deposit(&owner, b"early"), Err(StoreError::NotServed));
        assert_eq!(store.store_bundle(&owner, b"prekeys"), Err(StoreError::NotServed));
        store.claim(owner);
        assert!(store.deposit(&owner, b"for the owner").is_ok());
        assert!(store.store_bundle(&owner, b"prekeys").is_ok());
        assert_eq!(store.deposit(&stranger, b"not here"), Err(StoreError::NotServed));
    }

    #[tokio::test]
    async fn a_personal_relay_holds_mail_for_its_owner_only() {
        let (owner, alice, stranger) = (Identity::generate(), Identity::generate(), Identity::generate());
        let rig = Rig {
            store: Arc::new(MemStore::new(MemStoreLimits::personal(owner.card().sign_pk))),
            connections: Arc::default(),
            dht: None,
        };
        let mut a = rig.login(&alice).await;

        // Mail for the owner is what it is there for.
        send(&mut a, &ClientToRelay::Send { to: owner.card().sign_pk, blob: b"for the owner".to_vec() }).await.unwrap();
        assert!(matches!(next(&mut a).await, RelayToClient::Deposited { .. }));

        // Mail for anyone else is refused, not stored: the sender keeps it
        // queued for the right relay, and the host's memory is not a mailbox.
        send(&mut a, &ClientToRelay::Send { to: stranger.card().sign_pk, blob: b"parked here".to_vec() }).await.unwrap();
        assert!(matches!(next(&mut a).await, RelayToClient::Error(_)));
        assert_eq!(rig.store.stats().messages, 1);

        // Same for prekey bundles: only the owner publishes one here.
        send(&mut a, &ClientToRelay::Publish { bundle: b"alice's prekeys".to_vec() }).await.unwrap();
        assert!(matches!(next(&mut a).await, RelayToClient::Error(_)));
        let mut o = rig.login(&owner).await;
        assert!(matches!(next(&mut o).await, RelayToClient::Incoming { .. }), "the owner collects on login");
        send(&mut o, &ClientToRelay::Publish { bundle: b"owner's prekeys".to_vec() }).await.unwrap();
        send(&mut o, &ClientToRelay::Ping).await.unwrap();
        assert!(matches!(next(&mut o).await, RelayToClient::Pong));
        assert_eq!(rig.store.stats().bundles, 1);
    }

    #[tokio::test]
    async fn offline_recipient_collects_on_login() {
        let rig = Rig::new();
        let (alice, bob) = (Identity::generate(), Identity::generate());
        let mut a = rig.login(&alice).await;
        send(&mut a, &ClientToRelay::Send { to: bob.card().sign_pk, blob: b"later".to_vec() }).await.unwrap();
        let RelayToClient::Deposited { id } = next(&mut a).await else { panic!() };

        let mut b = rig.login(&bob).await;
        let RelayToClient::Incoming { id: got, blob, .. } = next(&mut b).await else { panic!("no pending push") };
        assert_eq!((got, blob.as_slice()), (id, b"later".as_slice()));
    }

    #[tokio::test]
    async fn bundles_publish_and_fetch() {
        let rig = Rig::new();
        let (alice, bob) = (Identity::generate(), Identity::generate());
        let mut b = rig.login(&bob).await;
        send(&mut b, &ClientToRelay::Publish { bundle: b"bob's prekeys".to_vec() }).await.unwrap();
        send(&mut b, &ClientToRelay::Ping).await.unwrap();
        assert!(matches!(next(&mut b).await, RelayToClient::Pong));

        let mut a = rig.login(&alice).await;
        let pk = bob.card().sign_pk;
        send(&mut a, &ClientToRelay::GetBundle { pk }).await.unwrap();
        let RelayToClient::Bundle { pk: got, bundle } = next(&mut a).await else { panic!() };
        assert_eq!(got, pk);
        assert_eq!(bundle.as_deref(), Some(b"bob's prekeys".as_slice()));
    }

    #[tokio::test]
    async fn an_old_connection_closing_does_not_unregister_the_new_one() {
        let rig = Rig::new();
        let (alice, bob) = (Identity::generate(), Identity::generate());
        let old = rig.login(&bob).await;
        let mut new = rig.login(&bob).await;
        drop(old);
        // Give the old connection's handler time to notice and clean up.
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(rig.connections.read().await.contains_key(&bob.card().sign_pk));

        let mut a = rig.login(&alice).await;
        send(&mut a, &ClientToRelay::Send { to: bob.card().sign_pk, blob: b"still here".to_vec() }).await.unwrap();
        let _deposited = next(&mut a).await;
        assert!(matches!(next(&mut new).await, RelayToClient::Incoming { .. }));
    }

    #[tokio::test]
    async fn backpressure_does_not_claim_the_cursor() {
        let (tx, mut rx) = mpsc::channel(1);
        let cursor = Arc::new(Mutex::new(Pushed::default()));

        assert!(matches!(
            try_enqueue_unclaimed(&tx, &cursor, 1, vec![1]).await,
            EnqueueOutcome::Queued
        ));
        assert!(matches!(
            try_enqueue_unclaimed(&tx, &cursor, 2, vec![2]).await,
            EnqueueOutcome::Backpressure
        ));

        let c = cursor.lock().await;
        assert!(c.is_claimed(1));
        assert!(!c.is_claimed(2));
        drop(c);

        assert!(matches!(rx.recv().await, Some(RelayToClient::Incoming { id: 1, .. })));
    }

    #[test]
    fn releasing_a_claim_rewinds_the_cursor_for_retry() {
        let mut p = Pushed::default();
        assert!(p.claim(10));
        assert_eq!(p.high, 10);
        p.release_for_retry(10);
        assert!(!p.is_claimed(10));
        assert_eq!(p.high, 9);
    }
}
