//! The relay-network node (`gipny-dht`) inside the app and the agent.
//!
//! Everything here is shared by `Core` and `SessionManager`, which only create
//! a [`Node`], hand [`handler`] to their [`crate::EphemeralRelay`] and call
//! [`join`] when that relay is up and [`maintain`] now and then.
//!
//! * **Storage** is the app's database ([`DbStorage`]): items held for others
//!   survive a restart. They are ciphertext under keys nobody but the two ends
//!   can link to a person.
//! * **Transport** is an anonymous connection to another node's relay: a
//!   [`ClientToRelay::Dht`] as the first frame, never a login ([`I2pTransport`]).
//! * **What we publish** so far: for each accepted contact, a record of where
//!   we collect now, readable by that contact alone.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use gipny_dht::node::{self as dht_node, Connection as DhtConnection, DhtNode, NetError as DhtNetError, NodeConfig, Transport};
use gipny_dht::proto::{DhtEnvelope, DhtResponse, NodeInfo, StoredItem, PROTOCOL_VERSION};
use gipny_dht::store::{admit, Storage, StoreError, StoreLimits, StoreStats};

use crate::crypto::Identity;
use crate::db::{Db, RequestState, TrustLevel};
use crate::net::{DuplexStream, TorNode};
use crate::relay::{recv, send, ClientToRelay, RelayToClient, RELAY_PORT};
use crate::relay_server::DhtHandler;

pub type Node = DhtNode<I2pTransport, DbStorage>;

/// How often a running node checks its table, republishes what it holds and
/// renews our address records (they live two hours).
pub const MAINTAIN_EVERY: Duration = Duration::from_secs(45 * 60);

/// Seed nodes, comma or whitespace separated: baked in at build time
/// (`GIPNY_DHT_SEEDS`, set from a repository variable in release.yml), plus
/// any named in the same variable at run time — how a server operator, or the
/// e2e run with its own seed, points a client at a network of its own.
pub fn builtin_seeds() -> Vec<String> {
    let runtime = std::env::var("GIPNY_DHT_SEEDS").unwrap_or_default();
    [option_env!("GIPNY_DHT_SEEDS").unwrap_or(""), runtime.as_str()]
        .iter()
        .flat_map(|list| list.split(|c: char| c == ',' || c.is_whitespace()))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether this process holds items for others. Not on Android: the system
/// puts the app to sleep, and a node that is mostly asleep loses what it holds.
pub fn stores_here() -> bool {
    !cfg!(target_os = "android")
}

pub fn new_node(net: Arc<TorNode>, db: Arc<Db>) -> Arc<Node> {
    Arc::new(DhtNode::new(
        Arc::new(I2pTransport { net }),
        Arc::new(DbStorage::new(db)),
        NodeConfig::default(),
        dht_node::system_clock(),
    ))
}

/// What a relay calls for each relay-network request it receives.
pub fn handler(node: &Arc<Node>) -> DhtHandler {
    let node = node.clone();
    Arc::new(move |challenge: &[u8; 32], request: Vec<u8>| {
        let answer = match dht_node::decode_envelope(&request) {
            Some(env) => node.handle(challenge, env),
            None => DhtResponse::Error("undecodable request".into()),
        };
        dht_node::encode_response(&answer)
    })
}

/// Our relay is up at `address`: become a node there, find the network and
/// tell our contacts where we are.
pub async fn join(node: &Arc<Node>, db: &Arc<Db>, identity: &Arc<Identity>, address: &str) {
    node.set_me(Some(NodeInfo { destination: address.to_string(), stores: stores_here(), version: PROTOCOL_VERSION }));
    add_candidates(node, db);
    node.bootstrap().await;
    eprintln!("[dht] joined: {} nodes known", node.peer_count());
    publish_addresses(node, db, identity, address).await;
    // A new address means a new place in the network: what we hold now
    // belongs with the nodes closest to it.
    node.republish().await;
    save_peers(node, db);
}

/// Periodic upkeep; `address` is where we collect now, if our relay is up.
pub async fn maintain(node: &Arc<Node>, db: &Arc<Db>, identity: &Arc<Identity>, address: Option<&str>) {
    if node.peer_count() == 0 {
        add_candidates(node, db);
        node.bootstrap().await;
    }
    node.maintain().await;
    if let Some(address) = address {
        publish_addresses(node, db, identity, address).await;
        node.republish().await;
    }
    save_peers(node, db);
}

/// Where to look for the network: seeds, nodes that answered before, and
/// the relays our contacts collect at.
fn add_candidates(node: &Arc<Node>, db: &Arc<Db>) {
    node.add_candidates(&builtin_seeds(), true);
    if let Ok(saved) = db.dht_peers_load() {
        node.add_candidates(&saved, false);
    }
    if let Ok(contacts) = db.list_contacts() {
        let relays: Vec<String> = contacts.into_iter().filter_map(|c| c.relay_address).collect();
        node.add_candidates(&relays, false);
    }
}

fn save_peers(node: &Arc<Node>, db: &Arc<Db>) {
    if let Err(e) = db.dht_peers_save(&node.known_peers()) {
        eprintln!("[dht] saving the node table failed: {e}");
    }
}

/// For each accepted contact, a record only they can find and read, saying
/// we collect at `address`. One at a time: every copy costs proof of work.
async fn publish_addresses(node: &Arc<Node>, db: &Arc<Db>, identity: &Arc<Identity>, address: &str) {
    let Ok(contacts) = db.list_contacts() else { return };
    let signing = identity.signing_key();
    let me = identity.card();
    let mut published = 0;
    for c in contacts {
        if c.trust == TrustLevel::Blocked || c.request_state != RequestState::None {
            continue;
        }
        let (Ok(their_sign), Ok(their_dh)) = (<[u8; 32]>::try_from(c.identity_sign.as_slice()), <[u8; 32]>::try_from(c.identity_dh.as_slice())) else {
            continue;
        };
        let Some(pair) = gipny_dht::crypto::pair_secret(identity.dh_secret(), &their_dh, &me.sign_pk, &their_sign) else { continue };
        let Some(item) = gipny_dht::items::address_record(&signing, &pair, node.now_ms(), address) else { continue };
        let item = StoredItem { key: item.key, value: item.value, delete_hash: item.delete_hash, expires_at_ms: item.expires_at_ms };
        if node.put(item).await > 0 {
            published += 1;
        }
    }
    eprintln!("[dht] address published for {published} contact(s)");
}

// ── mail through the network ─────────────────────────────────────────────
//
// Everything below works on a pair of identities and needs nothing from the
// session protocol: both sides derive the same secret from the cards they
// already hold. The storing nodes see ciphertext under a key that rolls daily.

/// The secret only these two can compute, from our identity and their card.
pub fn pair_with(identity: &Arc<Identity>, their_sign: &[u8; 32], their_dh: &[u8; 32]) -> Option<zeroize::Zeroizing<[u8; 32]>> {
    gipny_dht::crypto::pair_secret(identity.dh_secret(), their_dh, &identity.card().sign_pk, their_sign)
}

fn to_stored(item: gipny_dht::items::PreparedItem) -> StoredItem {
    StoredItem { key: item.key, value: item.value, delete_hash: item.delete_hash, expires_at_ms: item.expires_at_ms }
}

/// Leave an envelope where this contact collects it. `true` when at least one
/// node took it.
pub async fn put_mail(node: &Arc<Node>, identity: &Arc<Identity>, their_sign: &[u8; 32], their_dh: &[u8; 32], envelope: &[u8]) -> bool {
    let Some(pair) = pair_with(identity, their_sign, their_dh) else { return false };
    let Some(item) = gipny_dht::items::mail(&pair, their_sign, node.now_ms(), envelope) else { return false };
    node.put(to_stored(item)).await > 0
}

/// A first letter to someone who may not know us yet: sealed to their card,
/// so only they can open it, and found under a key any holder of that card
/// can compute.
pub async fn put_intro(node: &Arc<Node>, their_sign: &[u8; 32], their_dh: &[u8; 32], envelope: &[u8]) -> bool {
    let Some(item) = gipny_dht::items::intro(their_sign, their_dh, node.now_ms(), envelope) else { return false };
    node.put(to_stored(item)).await > 0
}

/// One letter found in the network, with what it takes to remove it.
pub struct Letter {
    pub envelope: Vec<u8>,
    pub key: [u8; 32],
    pub delete_token: [u8; 32],
}

/// Letters this contact left for us over the last `days` days.
pub async fn collect_mail(node: &Arc<Node>, identity: &Arc<Identity>, their_sign: &[u8; 32], their_dh: &[u8; 32], days: u32) -> Vec<Letter> {
    let Some(pair) = pair_with(identity, their_sign, their_dh) else { return Vec::new() };
    let me = identity.card().sign_pk;
    let mut out = Vec::new();
    for key in gipny_dht::items::mail_keys_to_poll(&pair, &me, node.now_ms(), days) {
        for item in node.get(&key).await {
            if let Some(opened) = gipny_dht::items::open_mail(&pair, &key, &item.value) {
                out.push(Letter { envelope: opened.envelope, key, delete_token: opened.delete_token });
            }
        }
    }
    out
}

/// First letters addressed to our card, from anyone.
pub async fn collect_intros(node: &Arc<Node>, identity: &Arc<Identity>, days: u32) -> Vec<Letter> {
    let me = identity.card();
    let mut out = Vec::new();
    for key in gipny_dht::items::intro_keys_to_poll(&me.sign_pk, &me.dh_pk, node.now_ms(), days) {
        for item in node.get(&key).await {
            if let Some(opened) = gipny_dht::items::open_intro(identity.dh_secret(), &key, &item.value) {
                out.push(Letter { envelope: opened.envelope, key, delete_token: opened.delete_token });
            }
        }
    }
    out
}

/// Identifies a letter for the "already handled" table.
pub fn letter_hash(envelope: &[u8]) -> [u8; 32] {
    gipny_dht::crypto::sha256(&[envelope])
}

/// Take a letter we have read out of the network. Only the recipient can: the
/// token was inside the sealed value.
pub async fn drop_letter(node: &Arc<Node>, letter: &Letter) {
    node.delete(&letter.key, &letter.delete_token).await;
}

/// Where this contact collects now, as they published it, and when.
pub async fn find_address(node: &Arc<Node>, identity: &Arc<Identity>, their_sign: &[u8; 32], their_dh: &[u8; 32]) -> Option<(String, u64)> {
    let pair = pair_with(identity, their_sign, their_dh)?;
    let key = gipny_dht::items::address_key(&pair, their_sign);
    let values: Vec<Vec<u8>> = node.get(&key).await.into_iter().map(|i| i.value).collect();
    gipny_dht::items::open_address_record(&pair, their_sign, &values, node.now_ms())
}

/// Their prekey bundle, for starting a session while they are away.
pub async fn find_bundle(node: &Arc<Node>, their_sign: &[u8; 32], their_dh: &[u8; 32]) -> Option<Vec<u8>> {
    let key = gipny_dht::items::bundle_key(their_sign, their_dh);
    let values: Vec<Vec<u8>> = node.get(&key).await.into_iter().map(|i| i.value).collect();
    gipny_dht::items::open_bundle_record(their_sign, their_dh, &values, node.now_ms())
}

/// Publish our prekey bundle so a contact can start a session with us while
/// we are offline. Anyone holding our card can read it — it is public by
/// design — but a storing node cannot tell whose it is.
pub async fn publish_bundle(node: &Arc<Node>, identity: &Arc<Identity>, bundle: &[u8]) -> bool {
    let me = identity.card();
    let Some(item) = gipny_dht::items::bundle_record(&identity.signing_key(), &me.dh_pk, node.now_ms(), bundle) else {
        return false;
    };
    node.put(to_stored(item)).await > 0
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct DhtStatus {
    /// Nodes that answered us.
    pub peers: usize,
    /// Items held for others, and their bytes.
    pub items: usize,
    pub bytes: usize,
    /// Reachable as a node (our relay is up), not only asking.
    pub joined: bool,
    pub stores: bool,
    pub seeds: usize,
}

pub fn status(node: &Arc<Node>) -> DhtStatus {
    let stats = node.storage_stats();
    DhtStatus {
        peers: node.peer_count(),
        items: stats.items,
        bytes: stats.bytes,
        joined: node.me().is_some(),
        stores: stores_here(),
        seeds: builtin_seeds().len(),
    }
}

// ── storage ──────────────────────────────────────────────────────────────

pub struct DbStorage {
    db: Arc<Db>,
    limits: StoreLimits,
}

impl DbStorage {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db, limits: StoreLimits::default() }
    }
}

impl Storage for DbStorage {
    fn put(&self, item: StoredItem, now_ms: u64) -> Result<(), StoreError> {
        let item = admit(item, &self.limits, now_ms)?;
        self.db.dht_item_put(&item, &self.limits).map_err(|e| StoreError::Backend(e.to_string()))
    }

    fn get(&self, key: &[u8; 32], now_ms: u64) -> Vec<StoredItem> {
        self.db.dht_items_get(key, now_ms as i64).unwrap_or_default()
    }

    fn delete(&self, key: &[u8; 32], token: &[u8; 32]) -> u32 {
        self.db.dht_item_delete(key, &gipny_dht::items::delete_hash(token)).unwrap_or(0)
    }

    fn gc(&self, now_ms: u64) {
        let _ = self.db.dht_items_gc(now_ms as i64);
    }

    fn all(&self, now_ms: u64) -> Vec<StoredItem> {
        self.db.dht_items_all(now_ms as i64).unwrap_or_default()
    }

    fn stats(&self) -> StoreStats {
        let (items, bytes) = self.db.dht_items_stats().unwrap_or((0, 0));
        StoreStats { items, bytes }
    }
}

// ── transport ────────────────────────────────────────────────────────────

pub struct I2pTransport {
    net: Arc<TorNode>,
}

pub struct I2pConn {
    stream: Pin<Box<dyn DuplexStream>>,
    challenge: [u8; 32],
}

fn net_err(e: impl std::fmt::Display) -> DhtNetError {
    DhtNetError(e.to_string())
}

impl Transport for I2pTransport {
    type Conn = I2pConn;

    fn open(&self, destination: &str) -> BoxFuture<'_, Result<I2pConn, DhtNetError>> {
        let destination = destination.to_string();
        Box::pin(async move {
            // A service dial: another node being away says nothing about our
            // own SAM session.
            let stream = self.net.connect_service(&destination, RELAY_PORT).await.map_err(net_err)?;
            let mut stream = stream.into_inner();
            match recv::<_, RelayToClient>(&mut stream).await.map_err(net_err)? {
                RelayToClient::Challenge(challenge) => Ok(I2pConn { stream, challenge }),
                _ => Err(DhtNetError("expected a challenge".into())),
            }
        })
    }
}

impl DhtConnection for I2pConn {
    fn challenge(&self) -> [u8; 32] {
        self.challenge
    }

    fn call(&mut self, env: DhtEnvelope) -> BoxFuture<'_, Result<DhtResponse, DhtNetError>> {
        Box::pin(async move {
            send(&mut self.stream, &ClientToRelay::Dht(dht_node::encode_envelope(&env))).await.map_err(net_err)?;
            match recv::<_, RelayToClient>(&mut self.stream).await.map_err(net_err)? {
                RelayToClient::Dht(bytes) => dht_node::decode_response(&bytes).ok_or_else(|| DhtNetError("undecodable answer".into())),
                // A relay that is not a node says so; the node drops it.
                RelayToClient::Error(e) => Ok(DhtResponse::Error(e)),
                _ => Err(DhtNetError("unexpected frame".into())),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(key: u8, value: &[u8], expires: u64) -> StoredItem {
        StoredItem { key: [key; 32], value: value.to_vec(), delete_hash: None, expires_at_ms: expires }
    }

    #[test]
    fn db_storage_behaves_like_the_memory_one() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open_plain(&dir.path().join("t.db")).unwrap());
        let s = DbStorage { db: db.clone(), limits: StoreLimits { max_items_per_key: 2, max_total_bytes: 10, ..Default::default() } };
        let now = 1_000;

        s.put(item(1, b"aaa", now + 100), now).unwrap();
        s.put(item(1, b"aaa", now + 100), now).unwrap(); // identical: no-op
        s.put(item(1, b"bbb", now + 100), now).unwrap();
        s.put(item(1, b"ccc", now + 100), now).unwrap(); // evicts "aaa" under the key
        let got: Vec<Vec<u8>> = s.get(&[1; 32], now).into_iter().map(|i| i.value).collect();
        assert_eq!(got, vec![b"bbb".to_vec(), b"ccc".to_vec()]);

        s.put(item(2, b"dddddd", now + 100), now).unwrap(); // 12 bytes > 10: evicts "bbb"
        assert_eq!(s.stats(), StoreStats { items: 2, bytes: 9 });

        assert_eq!(s.put(item(3, b"x", now), now), Err(StoreError::Expired));

        let token = [7u8; 32];
        let mut deletable = item(4, b"z", now + 50);
        deletable.delete_hash = Some(gipny_dht::items::delete_hash(&token));
        s.put(deletable, now).unwrap();
        assert_eq!(s.delete(&[4; 32], &[8; 32]), 0);
        assert_eq!(s.delete(&[4; 32], &token), 1);

        s.gc(now + 100);
        assert!(s.all(now + 100).is_empty());
    }
}
