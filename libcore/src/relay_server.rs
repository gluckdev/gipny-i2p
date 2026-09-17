//! A relay that runs inside a client process.
//!
//! Same protocol as the standalone relay in `core/relay`, ported down to this
//! workspace's dependency majors: that crate cannot be linked as a library,
//! because its libsqlite3-sys is a different major declaring the same
//! `links = "sqlite3"`. The frames are the enums in [`crate::relay`], and the
//! golden tests there pin their bytes to what `core/relay` puts on the wire.
//!
//! Two deliberate differences from the standalone relay:
//!
//! * **Memory only.** [`MemStore`] never touches disk. The keys of the people a
//!   relay serves, their ciphertext, and the record of who got something when
//!   stay out of this user's files entirely and are gone when the app closes.
//! * **A new destination every start**, never written down. There is no stable
//!   LeaseSet whose appearances would trace this user's online hours.
//!
//! So this is a fast path for whoever is online now, not a mailbox. The app
//! starts one for itself at every launch unless an external relay is chosen
//! (`core/src/core.rs`, `run_hosted_relay`). Contacts learn the address of the
//! day from the relay field every message carries, and from the empty
//! announcement sent to each of them once the relay is up; see
//! docs/relay-independence.md for what that does and does not cover.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::{JoinHandle, JoinSet};
use yosemite::{style, DestinationKind, RouterApi, Session, SessionOptions};

use crate::net::{NetError, SESSION_SEQ};
use crate::relay::{recv, send, ClientToRelay, RelayError, RelayToClient, ERR_NEEDS_AUTH_V2};

/// Answers one relay-network request, given the connection's challenge and the
/// request's bytes; returns the response's bytes. The node behind it lives in
/// [`crate::dht_client`]; the relay only carries the frames.
pub type DhtHandler = Arc<dyn Fn(&[u8; 32], Vec<u8>) -> Vec<u8> + Send + Sync>;

/// Requests one anonymous relay-network connection may make, and how long it
/// may sit idle between them.
const DHT_MAX_REQUESTS: usize = 64;
const DHT_IDLE: Duration = Duration::from_secs(60);

/// Live connections by the signing key they authenticated as.
pub type Connections = Arc<RwLock<HashMap<[u8; 32], mpsc::Sender<RelayToClient>>>>;

/// Most messages one push batch carries; core/relay's `PENDING_LIMIT`.
const PENDING_LIMIT: usize = 200;
/// How often a connected client is re-offered messages it has not acked.
const PUSH_REFRESH: Duration = Duration::from_secs(30);
const GC_INTERVAL: Duration = Duration::from_secs(600);
const PUSH_CAPACITY: usize = 512;

/// Budget for a relay held in RAM.
///
/// core/relay keeps up to 50 000 messages per recipient for 14 days — a
/// server's disk budget. Here every byte is the host user's memory, so the caps
/// are bytes, and when one is hit the oldest message goes first. Losing it is
/// recoverable: the sender still holds it unacked and sends it again.
#[derive(Clone, Debug)]
pub struct MemStoreLimits {
    /// All held message bytes together.
    pub max_total_bytes: usize,
    /// Held message bytes for one recipient, so one busy inbox cannot starve
    /// every other.
    pub max_recipient_bytes: usize,
    /// A prekey bundle is a few hundred bytes; anything far beyond is not one.
    pub max_bundle_bytes: usize,
    pub max_bundles: usize,
    pub message_ttl: Duration,
    pub bundle_ttl: Duration,
    /// Serve this one identity only: hold mail and a prekey bundle for it and
    /// refuse everyone else's. What a relay inside somebody's app is for — it
    /// is that person's inbox, not a public mailbox that strangers can fill
    /// with up to `max_total_bytes` of the host's memory. `None` serves
    /// anyone, as a standalone relay does.
    pub only_for: Option<[u8; 32]>,
}

impl Default for MemStoreLimits {
    fn default() -> Self {
        Self {
            max_total_bytes: 96 * 1024 * 1024,
            max_recipient_bytes: 24 * 1024 * 1024,
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
    total_bytes: usize,
}

impl Inner {
    fn remove(&mut self, id: u64) -> Option<Stored> {
        let m = self.messages.remove(&id)?;
        self.total_bytes -= m.blob.len();
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
pub struct MemStore {
    inner: std::sync::Mutex<Inner>,
    limits: MemStoreLimits,
}

impl MemStore {
    pub fn new(mut limits: MemStoreLimits) -> Self {
        // A recipient cap above the total cap would let one deposit push the
        // total over while evicting nothing of that recipient's.
        limits.max_recipient_bytes = limits.max_recipient_bytes.min(limits.max_total_bytes);
        Self { inner: std::sync::Mutex::new(Inner::default()), limits }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves plain collections behind, not a
        // broken invariant worth refusing all later requests over.
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn store_bundle(&self, pk: &[u8; 32], bundle: &[u8]) -> Result<(), StoreError> {
        if self.limits.only_for.is_some_and(|owner| owner != *pk) {
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
        if self.limits.only_for.is_some_and(|owner| owner != *to) {
            return Err(StoreError::NotServed);
        }
        if blob.len() > self.limits.max_recipient_bytes {
            return Err(StoreError::TooLarge);
        }
        let mut g = self.lock();
        g.next_id += 1;
        let id = g.next_id;
        g.messages.insert(id, Stored { recipient: *to, blob: blob.to_vec(), at: Instant::now() });
        g.total_bytes += blob.len();
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
        while g.total_bytes > self.limits.max_total_bytes {
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
    /// core/relay deleted by id alone, so any authenticated client could erase
    /// anyone's mail by walking the ids. An ack is the recipient's to give.
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
        StoreStats { bundles: g.bundles.len(), messages: g.messages.len(), message_bytes: g.total_bytes }
    }
}

/// Serve one client connection until it closes.
///
/// Mirrors `handle_client` in core/relay/src/main.rs, with three fixes carried
/// here and not there: the connection is registered before `AuthOk`, so a
/// message sent the instant a recipient authenticates is pushed rather than
/// waiting for the next refresh; on exit it unregisters only itself, not a newer
/// connection from the same key; and acks are checked against the recipient.
///
/// `destination_hash` is this relay's own ([`crate::relay::destination_hash`]),
/// which an `AuthV2` signature must cover. A client that logs in with plain
/// `Auth` may deposit and fetch bundles and nothing else: its signature could
/// have been lifted from a login to some other relay.
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
        request = match tokio::time::timeout(DHT_IDLE, recv::<_, ClientToRelay>(&mut stream)).await {
            Err(_) => return Ok(()),
            Ok(Err(RelayError::Io(e))) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Ok(Err(e)) => return Err(e),
            Ok(Ok(ClientToRelay::Dht(bytes))) => bytes,
            Ok(Ok(_)) => return Err(RelayError::Proto("only relay-network frames after one".into())),
        };
    }
    Ok(())
}

pub async fn handle_client<S>(
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

    let (sign_pk, signature, signed, owner) = match recv::<_, ClientToRelay>(&mut stream).await? {
        ClientToRelay::Dht(first) => return serve_dht(stream, challenge, first, dht).await,
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
    let cursor = Arc::new(Mutex::new(0u64));
    if !owner {
        let result = match send(&mut stream, &RelayToClient::AuthOk).await {
            Ok(()) => client_loop(&mut stream, &mut push_rx, &push_tx, sign_pk, false, &store, &connections, &cursor).await,
            Err(e) => Err(e),
        };
        return result;
    }
    connections.write().await.insert(sign_pk, push_tx.clone());

    let initial = tokio::spawn({
        let (store, push_tx, cursor) = (store.clone(), push_tx.clone(), cursor.clone());
        async move {
            for (id, blob) in store.pending_above(&sign_pk, 0) {
                {
                    let mut c = cursor.lock().await;
                    if id > *c { *c = id; }
                }
                if push_tx.send(RelayToClient::Incoming { id, from: [0u8; 32], blob }).await.is_err() {
                    break;
                }
            }
        }
    });
    let refresh = tokio::spawn({
        let (store, push_tx, cursor) = (store.clone(), push_tx.clone(), cursor.clone());
        async move {
            let mut tick = tokio::time::interval(PUSH_REFRESH);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            tick.tick().await;
            loop {
                tick.tick().await;
                let cur = *cursor.lock().await;
                for (id, blob) in store.pending_above(&sign_pk, cur) {
                    if push_tx.try_send(RelayToClient::Incoming { id, from: [0u8; 32], blob }).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let result = match send(&mut stream, &RelayToClient::AuthOk).await {
        Ok(()) => client_loop(&mut stream, &mut push_rx, &push_tx, sign_pk, true, &store, &connections, &cursor).await,
        Err(e) => Err(e),
    };

    initial.abort();
    refresh.abort();
    let mut conns = connections.write().await;
    if conns.get(&sign_pk).is_some_and(|tx| tx.same_channel(&push_tx)) {
        conns.remove(&sign_pk);
    }
    result
}

async fn client_loop<S>(
    stream: &mut S,
    push_rx: &mut mpsc::Receiver<RelayToClient>,
    push_tx: &mpsc::Sender<RelayToClient>,
    sign_pk: [u8; 32],
    owner: bool,
    store: &Arc<MemStore>,
    connections: &Connections,
    cursor: &Arc<Mutex<u64>>,
) -> Result<(), RelayError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    loop {
        tokio::select! {
            frame = recv::<_, ClientToRelay>(stream) => {
                match frame? {
                    ClientToRelay::Publish { .. } | ClientToRelay::Ack { .. } if !owner => {
                        send(stream, &RelayToClient::Error(ERR_NEEDS_AUTH_V2.into())).await?;
                    }
                    ClientToRelay::Publish { bundle } => {
                        if let Err(e) = store.store_bundle(&sign_pk, &bundle) {
                            send(stream, &RelayToClient::Error(e.to_string())).await?;
                        }
                    }
                    ClientToRelay::GetBundle { pk } => {
                        let bundle = store.get_bundle(&pk);
                        send(stream, &RelayToClient::Bundle { pk, bundle }).await?;
                    }
                    ClientToRelay::Send { to, blob } => match store.deposit(&to, &blob) {
                        Ok(id) => {
                            send(stream, &RelayToClient::Deposited { id }).await?;
                            let online = connections.read().await.get(&to).cloned();
                            if let Some(tx) = online {
                                let pkt = RelayToClient::Incoming { id, from: [0u8; 32], blob };
                                tokio::spawn(async move { let _ = tx.send(pkt).await; });
                            }
                        }
                        // No Deposited: the sender keeps the message unacked and
                        // tries again elsewhere or later.
                        Err(e) => send(stream, &RelayToClient::Error(e.to_string())).await?,
                    },
                    ClientToRelay::Ack { id } => {
                        store.ack(&sign_pk, id);
                        let cur = *cursor.lock().await;
                        for (mid, blob) in store.pending_above(&sign_pk, cur) {
                            {
                                let mut c = cursor.lock().await;
                                if mid > *c { *c = mid; }
                            }
                            if push_tx.try_send(RelayToClient::Incoming { id: mid, from: [0u8; 32], blob }).is_err() {
                                break;
                            }
                        }
                    }
                    ClientToRelay::Ping => send(stream, &RelayToClient::Pong).await?,
                    ClientToRelay::Auth { .. } | ClientToRelay::AuthV2 { .. } | ClientToRelay::Dht(_) => {}
                }
            }
            push = push_rx.recv() => {
                let Some(msg) = push else { break };
                if let RelayToClient::Incoming { id, .. } = &msg {
                    let mut c = cursor.lock().await;
                    if *id > *c { *c = *id; }
                }
                send(stream, &msg).await?;
            }
        }
    }
    Ok(())
}

/// A running in-process relay: its own publishing SAM session, memory-only
/// storage, and the tasks serving both. Dropping it stops everything and
/// releases the destination.
pub struct EphemeralRelay {
    address: String,
    store: Arc<MemStore>,
    tasks: Vec<JoinHandle<()>>,
}

impl EphemeralRelay {
    /// Generate a fresh destination and start serving it through the router
    /// on `sam_port`.
    ///
    /// This needs its own SAM session: the client's is unpublished, and a relay
    /// must publish a LeaseSet to be reachable at all. It returns once that
    /// session exists, which for a published destination means its tunnels are
    /// built, commonly a minute or two. Run it in a spawned task; never on a
    /// path anything user-facing waits on.
    ///
    /// `dht` answers relay-network requests arriving here; `None` refuses them.
    pub async fn start(sam_port: u16, limits: MemStoreLimits, dht: Option<DhtHandler>) -> Result<Self, NetError> {
        let (address, private_key) = RouterApi::new(sam_port)
            .generate_destination()
            .await
            .map_err(|e| NetError::I2p(format!("relay destination: {e}")))?;
        // Kept in memory only, for rebuilding the session on the same address.
        // It is never written anywhere.
        let private_key = zeroize::Zeroizing::new(private_key);
        let first = open_session(sam_port, &private_key).await?;

        let store = Arc::new(MemStore::new(limits));
        let connections: Connections = Arc::default();

        // The session is owned by this task alone: `accept` holds `&mut` across
        // its await, so sharing it would stall everything else behind a wait for
        // the next caller. Client tasks live in the JoinSet, so aborting this
        // task drops them with it.
        let destination_hash = crate::relay::destination_hash(&address)
            .ok_or_else(|| NetError::I2p("relay destination does not decode".into()))?;
        let accept = tokio::spawn({
            let store = store.clone();
            async move {
                let mut session = Some(first);
                let mut failures = 0u32;
                let mut clients = JoinSet::new();
                loop {
                    let Some(live) = session.as_mut() else {
                        tokio::time::sleep(rebuild_backoff(failures)).await;
                        match open_session(sam_port, &private_key).await {
                            Ok(s) => session = Some(s),
                            Err(e) => {
                                failures += 1;
                                eprintln!("[relay-server] session rebuild failed: {e}");
                            }
                        }
                        continue;
                    };
                    tokio::select! {
                        accepted = live.accept() => match accepted {
                            Ok(stream) => {
                                failures = 0;
                                let (store, connections, dht) = (store.clone(), connections.clone(), dht.clone());
                                clients.spawn(async move {
                                    if let Err(e) = handle_client(stream, store, connections, destination_hash, dht).await {
                                        eprintln!("[relay-server] client gone: {e}");
                                    }
                                });
                            }
                            // yosemite poisons the session's controller on any
                            // reply it cannot parse, and every later accept on it
                            // fails while the destination drops off the router
                            // (e2e run 35076520090, both standalone relays). Drop
                            // it, releasing the destination, and reopen.
                            Err(e) => {
                                failures += 1;
                                eprintln!("[relay-server] accept err: {e}; rebuilding the session");
                                session = None;
                            }
                        },
                        // Reap finished clients so the set does not grow for the
                        // life of the relay.
                        Some(_) = clients.join_next(), if !clients.is_empty() => {}
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

        Ok(Self { address, store, tasks: vec![accept, gc] })
    }

    /// This relay's destination, for handing to whoever should deposit here.
    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn stats(&self) -> StoreStats {
        self.store.stats()
    }
}

/// A publishing STREAM session on `private_key`, under a nickname no other
/// session on the router has: IDs are router-wide, and a rebuild can race the
/// router's teardown of the session it replaces.
async fn open_session(sam_port: u16, private_key: &str) -> Result<Session<style::Stream>, NetError> {
    let seq = SESSION_SEQ.fetch_add(1, Ordering::Relaxed);
    let opts = SessionOptions {
        nickname: format!("gipny-relay-{}-{}", std::process::id(), seq),
        destination: DestinationKind::Persistent { private_key: private_key.to_string() },
        samv3_tcp_port: sam_port,
        publish: true,
        // Payloads are E2E-encrypted and padded to size buckets already.
        gzip: false,
        ..Default::default()
    };
    Session::<style::Stream>::new(opts)
        .await
        .map_err(|e| NetError::I2p(format!("relay SAM session: {e}")))
}

/// 0.5 s doubling to a 30 s ceiling.
fn rebuild_backoff(failures: u32) -> Duration {
    Duration::from_millis(500u64.saturating_mul(1 << failures.min(6)).min(30_000))
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
        let s = MemStore::new(small(10, 10));
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
        assert_eq!(s.stats().messages, 0);
        let big = vec![0u8; MemStoreLimits::default().max_bundle_bytes + 1];
        assert_eq!(s.store_bundle(&A, &big), Err(StoreError::TooLarge));
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
            let (client, server) = tokio::io::duplex(1 << 20);
            let task = tokio::spawn(handle_client(server, self.store.clone(), self.connections.clone(), RIG_DESTINATION, self.dht.clone()));
            (client, task)
        }

        async fn login(&self, who: &Identity) -> DuplexStream {
            let (mut c, _task) = self.open();
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
}
