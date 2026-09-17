//! A node: answers other nodes, and finds, stores and fetches items across
//! the network (Kademlia-style lookups over a flat table of known nodes —
//! gipny's network is small, and a flat table sorted by distance is the same
//! thing as full buckets until it is not).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use futures::future::{join_all, BoxFuture};

use crate::crypto::{sha256, DhtKey};
use crate::proto::{
    self, node_id, pow_ok, solve_pow, DhtEnvelope, DhtRequest, DhtResponse, NodeInfo, StoredItem,
    PROTOCOL_VERSION,
};
use crate::store::{Storage, StoreStats};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct NetError(pub String);

/// One conversation with another node, opened by [`Transport::open`]. The
/// challenge is what that node sent first; store work is bound to it.
pub trait Connection: Send {
    fn challenge(&self) -> [u8; 32];
    fn call(&mut self, env: DhtEnvelope) -> BoxFuture<'_, Result<DhtResponse, NetError>>;
}

pub trait Transport: Send + Sync + 'static {
    type Conn: Connection + 'static;
    fn open(&self, destination: &str) -> BoxFuture<'_, Result<Self::Conn, NetError>>;
}

pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

pub fn system_clock() -> Clock {
    Arc::new(|| {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
    })
}

#[derive(Clone, Debug)]
pub struct NodeConfig {
    /// Replicas per item, and the size of a lookup's result.
    pub k: usize,
    /// Nodes asked at once during a lookup.
    pub alpha: usize,
    /// Leading zero bits a store's proof of work needs.
    pub pow_difficulty: u32,
    pub max_peers: usize,
    /// One whole exchange with a node, dial included. i2p dials take seconds.
    pub call_timeout: Duration,
    /// Failed exchanges before a node is forgotten; seeds are never forgotten.
    pub max_failures: u32,
    /// Most bytes of items in one `Get` answer.
    pub max_get_bytes: usize,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            k: 8,
            alpha: 3,
            pow_difficulty: 18,
            max_peers: 2048,
            call_timeout: Duration::from_secs(120),
            max_failures: 3,
            max_get_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
struct Peer {
    info: NodeInfo,
    /// Answered us at least once, so `info.stores` is what it said, not a guess.
    confirmed: bool,
    failures: u32,
    seed: bool,
    first_seen_ms: u64,
    last_ok_ms: u64,
}

/// A known node, as the embedding app may want to persist it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownPeer {
    pub info: NodeInfo,
    pub first_seen_ms: u64,
    pub last_ok_ms: u64,
}

fn distance(a: &DhtKey, b: &DhtKey) -> DhtKey {
    let mut d = [0u8; 32];
    for i in 0..32 {
        d[i] = a[i] ^ b[i];
    }
    d
}

pub struct DhtNode<T: Transport, S: Storage> {
    me: RwLock<Option<NodeInfo>>,
    storage: Arc<S>,
    transport: Arc<T>,
    cfg: NodeConfig,
    clock: Clock,
    peers: Mutex<HashMap<String, Peer>>,
}

impl<T: Transport, S: Storage> DhtNode<T, S> {
    pub fn new(transport: Arc<T>, storage: Arc<S>, cfg: NodeConfig, clock: Clock) -> Self {
        Self { me: RwLock::new(None), storage, transport, cfg, clock, peers: Mutex::default() }
    }

    /// Become reachable as `me` (once our relay is up). Until then this node
    /// only asks, and nobody learns about it.
    pub fn set_me(&self, me: Option<NodeInfo>) {
        *self.me.write().unwrap_or_else(|p| p.into_inner()) = me;
    }

    pub fn me(&self) -> Option<NodeInfo> {
        self.me.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn stores(&self) -> bool {
        self.me().is_some_and(|m| m.stores)
    }

    pub fn now_ms(&self) -> u64 {
        (self.clock)()
    }

    pub fn storage_stats(&self) -> StoreStats {
        self.storage.stats()
    }

    fn peers(&self) -> std::sync::MutexGuard<'_, HashMap<String, Peer>> {
        self.peers.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Addresses to try when nothing better is known: seeds, contacts' relays,
    /// the saved table. `seed` ones are never dropped.
    pub fn add_candidates(&self, destinations: &[String], seed: bool) {
        let now = self.now_ms();
        let mine = self.me().map(|m| m.destination);
        let mut peers = self.peers();
        for d in destinations {
            let d = d.trim();
            if d.is_empty() || mine.as_deref() == Some(d) {
                continue;
            }
            peers.entry(d.to_string())
                .and_modify(|p| p.seed |= seed)
                .or_insert(Peer {
                    info: NodeInfo { destination: d.to_string(), stores: true, version: PROTOCOL_VERSION },
                    confirmed: false,
                    failures: 0,
                    seed,
                    first_seen_ms: now,
                    last_ok_ms: 0,
                });
        }
        self.trim_peers(&mut peers);
    }

    /// Nodes that answered at some point, for saving across restarts.
    pub fn known_peers(&self) -> Vec<KnownPeer> {
        self.peers().values().filter(|p| p.confirmed).map(|p| KnownPeer {
            info: p.info.clone(),
            first_seen_ms: p.first_seen_ms,
            last_ok_ms: p.last_ok_ms,
        }).collect()
    }

    pub fn peer_count(&self) -> usize {
        self.peers().values().filter(|p| p.confirmed).count()
    }

    fn learn(&self, info: &NodeInfo, confirmed: bool) {
        if info.version != PROTOCOL_VERSION || info.destination.is_empty() {
            return;
        }
        if self.me().is_some_and(|m| m.destination == info.destination) {
            return;
        }
        let now = self.now_ms();
        let mut peers = self.peers();
        match peers.get_mut(&info.destination) {
            Some(p) => {
                // What a node says about itself beats what others say about it.
                if confirmed || !p.confirmed {
                    p.info = info.clone();
                }
                if confirmed {
                    p.confirmed = true;
                    p.failures = 0;
                    p.last_ok_ms = now;
                }
            }
            None => {
                peers.insert(info.destination.clone(), Peer {
                    info: info.clone(),
                    confirmed,
                    failures: 0,
                    seed: false,
                    first_seen_ms: now,
                    last_ok_ms: if confirmed { now } else { 0 },
                });
            }
        }
        self.trim_peers(&mut peers);
    }

    fn trim_peers(&self, peers: &mut HashMap<String, Peer>) {
        while peers.len() > self.cfg.max_peers {
            // Hearsay first, then the longest silent.
            let victim = peers.iter()
                .filter(|(_, p)| !p.seed)
                .min_by_key(|(_, p)| (p.confirmed, p.last_ok_ms))
                .map(|(d, _)| d.clone());
            match victim {
                Some(d) => { peers.remove(&d); }
                None => break,
            }
        }
    }

    fn failed(&self, destination: &str) {
        let mut peers = self.peers();
        if let Some(p) = peers.get_mut(destination) {
            p.failures += 1;
            if p.failures >= self.cfg.max_failures && !p.seed {
                peers.remove(destination);
            }
        }
    }

    fn closest(&self, target: &DhtKey, n: usize, storing_only: bool) -> Vec<NodeInfo> {
        let mut list: Vec<NodeInfo> = self.peers().values()
            .filter(|p| !storing_only || p.info.stores)
            .map(|p| p.info.clone())
            .collect();
        list.sort_by_key(|i| distance(&i.id(), target));
        list.truncate(n);
        list
    }

    // ── answering other nodes ────────────────────────────────────────────

    /// Answer one request that arrived on a connection whose challenge was
    /// `challenge`.
    pub fn handle(&self, challenge: &[u8; 32], env: DhtEnvelope) -> DhtResponse {
        if let Some(from) = &env.from {
            // It reached us, but that says nothing about reaching it back.
            self.learn(from, false);
        }
        let now = self.now_ms();
        match env.req {
            DhtRequest::Ping => match self.me() {
                Some(node) => DhtResponse::Pong { node },
                None => DhtResponse::Error("not a node".into()),
            },
            DhtRequest::FindNode { target } => DhtResponse::Nodes { me: self.me(), nodes: self.closest(&target, self.cfg.k, false) },
            DhtRequest::Get { key, skip } => {
                let closer = self.closest(&key, self.cfg.k, true);
                if !self.stores() {
                    return DhtResponse::Items { items: vec![], more: false, closer };
                }
                let all = self.storage.get(&key, now);
                let mut items = Vec::new();
                let mut bytes = 0;
                let mut more = false;
                for it in all.into_iter().skip(skip as usize) {
                    if !items.is_empty() && bytes + it.value.len() > self.cfg.max_get_bytes {
                        more = true;
                        break;
                    }
                    bytes += it.value.len();
                    items.push(it);
                }
                DhtResponse::Items { items, more, closer }
            }
            DhtRequest::Store { item, pow } => {
                if !self.stores() {
                    return DhtResponse::Error("this node does not store".into());
                }
                if !pow_ok(challenge, &item, pow, self.cfg.pow_difficulty) {
                    return DhtResponse::Error("insufficient work".into());
                }
                match self.storage.put(item, now) {
                    Ok(()) => DhtResponse::Stored,
                    Err(e) => DhtResponse::Error(e.to_string()),
                }
            }
            DhtRequest::Delete { key, token } => DhtResponse::Deleted(self.storage.delete(&key, &token)),
        }
    }

    // ── asking other nodes ───────────────────────────────────────────────

    fn envelope(&self, req: DhtRequest) -> DhtEnvelope {
        DhtEnvelope { from: self.me(), req }
    }

    async fn open(&self, destination: &str) -> Option<T::Conn> {
        match tokio::time::timeout(self.cfg.call_timeout, self.transport.open(destination)).await {
            Ok(Ok(c)) => Some(c),
            _ => {
                self.failed(destination);
                None
            }
        }
    }

    async fn exchange(&self, destination: &str, conn: &mut T::Conn, req: DhtRequest) -> Option<DhtResponse> {
        match tokio::time::timeout(self.cfg.call_timeout, conn.call(self.envelope(req))).await {
            Ok(Ok(r)) => Some(r),
            _ => {
                self.failed(destination);
                None
            }
        }
    }

    async fn ask(&self, destination: &str, req: DhtRequest) -> Option<DhtResponse> {
        let mut conn = self.open(destination).await?;
        self.exchange(destination, &mut conn, req).await
    }

    /// Ping every unconfirmed candidate (seeds, contacts' relays, the saved
    /// table), then look ourselves up so the nodes near us learn about us.
    pub async fn bootstrap(&self) {
        let pending: Vec<String> = self.peers().values().filter(|p| !p.confirmed).map(|p| p.info.destination.clone()).collect();
        join_all(pending.iter().map(|d| self.ping(d))).await;
        let target = self.me().map(|m| m.id()).unwrap_or_else(crate::crypto::random_32);
        self.lookup(&target).await;
    }

    pub async fn ping(&self, destination: &str) -> bool {
        match self.ask(destination, DhtRequest::Ping).await {
            Some(DhtResponse::Pong { node }) if node.destination == destination => {
                self.learn(&node, true);
                true
            }
            Some(_) => {
                // Answers, but not as a node: a relay from before the network.
                self.peers().remove(destination);
                false
            }
            None => false,
        }
    }

    /// The `k` nodes closest to `target` that answered.
    pub async fn lookup(&self, target: &DhtKey) -> Vec<NodeInfo> {
        let mut queried: HashSet<String> = HashSet::new();
        let mut answered: HashMap<String, NodeInfo> = HashMap::new();
        loop {
            let batch: Vec<NodeInfo> = self.closest(target, self.cfg.k * 2, false).into_iter()
                .filter(|n| !queried.contains(&n.destination))
                .take(self.cfg.alpha)
                .collect();
            // Done once the k closest we know have all been asked.
            let k_closest_asked = self.closest(target, self.cfg.k, false).iter().all(|n| queried.contains(&n.destination));
            if batch.is_empty() || k_closest_asked {
                break;
            }
            for n in &batch {
                queried.insert(n.destination.clone());
            }
            let replies = join_all(batch.iter().map(|n| async move {
                (n, self.ask(&n.destination, DhtRequest::FindNode { target: *target }).await)
            })).await;
            for (n, reply) in replies {
                if let Some(DhtResponse::Nodes { me, nodes }) = reply {
                    let Some(me) = me.filter(|m| m.destination == n.destination) else {
                        self.peers().remove(&n.destination);
                        continue;
                    };
                    self.learn(&me, true);
                    answered.insert(n.destination.clone(), me);
                    for m in &nodes {
                        self.learn(m, false);
                    }
                }
            }
        }
        let mut out: Vec<NodeInfo> = answered.into_values().collect();
        out.sort_by_key(|i| distance(&i.id(), target));
        out.truncate(self.cfg.k);
        out
    }

    async fn store_at(&self, node: &NodeInfo, item: &StoredItem) -> bool {
        let Some(mut conn) = self.open(&node.destination).await else { return false };
        let challenge = conn.challenge();
        let difficulty = self.cfg.pow_difficulty;
        let work_item = item.clone();
        let Ok(pow) = tokio::task::spawn_blocking(move || solve_pow(&challenge, &work_item, difficulty)).await else {
            return false;
        };
        matches!(
            self.exchange(&node.destination, &mut conn, DhtRequest::Store { item: item.clone(), pow }).await,
            Some(DhtResponse::Stored)
        )
    }

    /// Store an item on the `k` closest storing nodes (and here, if this node
    /// stores). Returns how many copies exist now.
    pub async fn put(&self, item: StoredItem) -> usize {
        let now = self.now_ms();
        let mut copies = 0;
        if self.stores() && self.storage.put(item.clone(), now).is_ok() {
            copies += 1;
        }
        let targets: Vec<NodeInfo> = self.lookup(&item.key).await.into_iter().filter(|n| n.stores).collect();
        let results = join_all(targets.iter().map(|n| self.store_at(n, &item))).await;
        copies + results.into_iter().filter(|ok| *ok).count()
    }

    async fn items_at(&self, node: &NodeInfo, key: &DhtKey) -> Option<Vec<StoredItem>> {
        let mut conn = self.open(&node.destination).await?;
        let mut out = Vec::new();
        loop {
            match self.exchange(&node.destination, &mut conn, DhtRequest::Get { key: *key, skip: out.len() as u32 }).await? {
                DhtResponse::Items { items, more, closer } => {
                    for c in &closer {
                        self.learn(c, false);
                    }
                    let got = items.len();
                    out.extend(items);
                    if !more || got == 0 {
                        return Some(out);
                    }
                }
                _ => return None,
            }
        }
    }

    /// Every distinct item under `key` the closest storing nodes hold.
    pub async fn get(&self, key: &DhtKey) -> Vec<StoredItem> {
        let now = self.now_ms();
        let mut seen: HashSet<[u8; 32]> = HashSet::new();
        let mut out = Vec::new();
        let mut take = |items: Vec<StoredItem>, out: &mut Vec<StoredItem>| {
            for it in items {
                if it.key == *key && it.expires_at_ms > now && seen.insert(sha256(&[&it.value])) {
                    out.push(it);
                }
            }
        };
        if self.stores() {
            take(self.storage.get(key, now), &mut out);
        }
        let targets: Vec<NodeInfo> = self.lookup(key).await.into_iter().filter(|n| n.stores).collect();
        for items in join_all(targets.iter().map(|n| self.items_at(n, key))).await.into_iter().flatten() {
            take(items, &mut out);
        }
        out
    }

    /// Delete mail we opened, wherever the closest nodes hold it.
    pub async fn delete(&self, key: &DhtKey, token: &[u8; 32]) -> u32 {
        let mut n = self.storage.delete(key, token);
        let targets: Vec<NodeInfo> = self.lookup(key).await.into_iter().filter(|n| n.stores).collect();
        for r in join_all(targets.iter().map(|t| self.ask(&t.destination, DhtRequest::Delete { key: *key, token: *token }))).await {
            if let Some(DhtResponse::Deleted(d)) = r {
                n += d;
            }
        }
        n
    }

    /// Hand what we hold to the nodes now closest to it, which after churn
    /// may be nodes that did not exist when it was stored. A node that already
    /// has an item is asked, not sent it again: asking costs no work.
    pub async fn republish(&self) -> usize {
        let now = self.now_ms();
        self.storage.gc(now);
        let mut handed = 0;
        for item in self.storage.all(now) {
            let hash = sha256(&[&item.value]);
            for node in self.lookup(&item.key).await.into_iter().filter(|n| n.stores) {
                let has = self.items_at(&node, &item.key).await
                    .is_some_and(|items| items.iter().any(|i| sha256(&[&i.value]) == hash));
                if !has && self.store_at(&node, &item).await {
                    handed += 1;
                }
            }
        }
        handed
    }

    /// Forget expired items and nodes that stopped answering.
    pub async fn maintain(&self) {
        self.storage.gc(self.now_ms());
        let stale: Vec<String> = self.peers().values()
            .filter(|p| p.confirmed && p.failures > 0)
            .map(|p| p.info.destination.clone())
            .collect();
        join_all(stale.iter().map(|d| self.ping(d))).await;
    }
}

/// Encode/decode helpers for embedding the protocol in a byte transport.
pub fn encode_envelope(env: &DhtEnvelope) -> Vec<u8> {
    proto::encode(env)
}

pub fn decode_envelope(b: &[u8]) -> Option<DhtEnvelope> {
    proto::decode(b)
}

pub fn encode_response(r: &DhtResponse) -> Vec<u8> {
    proto::encode(r)
}

pub fn decode_response(b: &[u8]) -> Option<DhtResponse> {
    proto::decode(b)
}

pub fn id_of(destination: &str) -> DhtKey {
    node_id(destination)
}
