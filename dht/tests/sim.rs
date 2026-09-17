//! The network in memory: many nodes, churn, and people who are away.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use futures::future::BoxFuture;
use gipny_dht::crypto::{self, DAY_MS};
use gipny_dht::items;
use gipny_dht::node::{self as dnode, Connection, DhtNode, NetError, NodeConfig, Transport};
use gipny_dht::proto::{DhtEnvelope, DhtRequest, DhtResponse, NodeInfo, StoredItem, PROTOCOL_VERSION};
use gipny_dht::store::{MemStorage, StoreLimits};
use x25519_dalek::{PublicKey, StaticSecret};

type Node = DhtNode<MemTransport, MemStorage>;

struct Net {
    nodes: Mutex<HashMap<String, Arc<Node>>>,
    offline: Mutex<HashSet<String>>,
    clock: Arc<AtomicU64>,
}

impl Net {
    fn new() -> Arc<Self> {
        Arc::new(Self { nodes: Mutex::default(), offline: Mutex::default(), clock: Arc::new(AtomicU64::new(20_000 * DAY_MS)) })
    }
    fn now(&self) -> u64 {
        self.clock.load(Ordering::Relaxed)
    }
}

struct MemTransport {
    net: Arc<Net>,
}

struct MemConn {
    net: Arc<Net>,
    node: Arc<Node>,
    destination: String,
    challenge: [u8; 32],
}

impl Connection for MemConn {
    fn challenge(&self) -> [u8; 32] {
        self.challenge
    }

    fn call(&mut self, env: DhtEnvelope) -> BoxFuture<'_, Result<DhtResponse, NetError>> {
        Box::pin(async move {
            if self.net.offline.lock().unwrap().contains(&self.destination) {
                return Err(NetError("gone".into()));
            }
            // Through the byte encoding, as on the wire.
            let env = dnode::decode_envelope(&dnode::encode_envelope(&env)).expect("envelope round-trips");
            let resp = self.node.handle(&self.challenge, env);
            Ok(dnode::decode_response(&dnode::encode_response(&resp)).expect("response round-trips"))
        })
    }
}

impl Transport for MemTransport {
    type Conn = MemConn;

    fn open(&self, destination: &str) -> BoxFuture<'_, Result<MemConn, NetError>> {
        let destination = destination.to_string();
        Box::pin(async move {
            if self.net.offline.lock().unwrap().contains(&destination) {
                return Err(NetError("unreachable".into()));
            }
            let node = self.net.nodes.lock().unwrap().get(&destination).cloned().ok_or_else(|| NetError("no such destination".into()))?;
            Ok(MemConn { net: self.net.clone(), node, destination, challenge: crypto::random_32() })
        })
    }
}

const DIFFICULTY: u32 = 4;

fn config() -> NodeConfig {
    NodeConfig { pow_difficulty: DIFFICULTY, call_timeout: Duration::from_secs(5), ..NodeConfig::default() }
}

fn new_node(net: &Arc<Net>, destination: Option<&str>, stores: bool) -> Arc<Node> {
    let node = Arc::new(DhtNode::new(
        Arc::new(MemTransport { net: net.clone() }),
        Arc::new(MemStorage::new(StoreLimits::default())),
        config(),
        { let clock = net.clock.clone(); Arc::new(move || clock.load(Ordering::Relaxed)) },
    ));
    if let Some(d) = destination {
        node.set_me(Some(NodeInfo { destination: d.into(), stores, version: PROTOCOL_VERSION }));
        net.nodes.lock().unwrap().insert(d.into(), node.clone());
    }
    node
}

async fn network(net: &Arc<Net>, n: usize) -> Vec<Arc<Node>> {
    let mut nodes = Vec::new();
    for i in 0..n {
        let node = new_node(net, Some(&format!("node-{i}")), true);
        node.add_candidates(&["node-0".to_string()], true);
        nodes.push(node);
    }
    for node in &nodes {
        node.bootstrap().await;
    }
    // A second round, as running nodes keep learning about later arrivals.
    for node in &nodes {
        node.bootstrap().await;
    }
    nodes
}

struct Person {
    signing: SigningKey,
    dh_sk: [u8; 32],
}

impl Person {
    fn new() -> Self {
        Self { signing: SigningKey::from_bytes(&crypto::random_32()), dh_sk: crypto::random_32() }
    }
    fn sign_pk(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }
    fn dh_pk(&self) -> [u8; 32] {
        PublicKey::from(&StaticSecret::from(self.dh_sk)).to_bytes()
    }
    fn pair_with(&self, other: &Person) -> [u8; 32] {
        *crypto::pair_secret(&self.dh_sk, &other.dh_pk(), &self.sign_pk(), &other.sign_pk()).unwrap()
    }
}

fn stored(p: items::PreparedItem) -> StoredItem {
    StoredItem { key: p.key, value: p.value, delete_hash: p.delete_hash, expires_at_ms: p.expires_at_ms }
}

fn holders(nodes: &[Arc<Node>], key: &[u8; 32], net: &Net) -> Vec<String> {
    let offline = net.offline.lock().unwrap().clone();
    nodes.iter()
        .filter_map(|n| n.me().map(|m| (n, m.destination)))
        .filter(|(_, d)| !offline.contains(d))
        .filter(|(n, _)| {
            matches!(n.handle(&[0; 32], DhtEnvelope { from: None, req: DhtRequest::Get { key: *key, skip: 0 } }),
                DhtResponse::Items { items, .. } if !items.is_empty())
        })
        .map(|(_, d)| d)
        .collect()
}

#[tokio::test]
async fn mail_outlives_every_node_it_was_first_stored_on() {
    let net = Net::new();
    let mut nodes = network(&net, 30).await;
    let (alice, bob) = (Person::new(), Person::new());

    // Alice is a phone: she asks and stores through others, holds nothing.
    let alice_node = new_node(&net, None, false);
    alice_node.add_candidates(&["node-0".to_string()], true);
    alice_node.bootstrap().await;
    let now = net.now();
    let letter = stored(items::mail(&alice.pair_with(&bob), &bob.sign_pk(), now, b"see you tomorrow").unwrap());
    let copies = alice_node.put(letter.clone()).await;
    assert!(copies >= 6, "stored on {copies} nodes");

    // Every original holder goes away, but not all at once: first most of
    // them, then newcomers join and the survivors hand the letter on, then
    // the rest go too.
    let first = holders(&nodes, &letter.key, &net);
    assert_eq!(first.len(), copies);
    for d in &first[..first.len() - 2] {
        net.offline.lock().unwrap().insert(d.clone());
    }
    let live: Vec<String> = (0..30).map(|i| format!("node-{i}"))
        .filter(|d| !net.offline.lock().unwrap().contains(d)).take(2).collect();
    for i in 30..45 {
        let node = new_node(&net, Some(&format!("node-{i}")), true);
        node.add_candidates(&live, false);
        node.bootstrap().await;
        nodes.push(node);
    }
    let offline = net.offline.lock().unwrap().clone();
    for node in nodes.iter().filter(|n| !offline.contains(&n.me().unwrap().destination)) {
        node.republish().await;
    }
    for d in &first {
        net.offline.lock().unwrap().insert(d.clone());
    }
    let now_holding = holders(&nodes, &letter.key, &net);
    assert!(!now_holding.is_empty(), "nobody holds the letter any more");
    assert!(now_holding.iter().all(|d| !first.contains(d)));

    // Bob comes back a day later, knowing one node that is still up.
    net.clock.fetch_add(DAY_MS, Ordering::Relaxed);
    let later = net.now();
    let bob_node = new_node(&net, None, false);
    bob_node.add_candidates(&["node-40".to_string()], true);
    bob_node.bootstrap().await;
    let pair = bob.pair_with(&alice);
    let mut got = Vec::new();
    for key in items::mail_keys_to_poll(&pair, &bob.sign_pk(), later, 7) {
        for it in bob_node.get(&key).await {
            got.push((key, items::open_mail(&pair, &key, &it.value).expect("opens")));
        }
    }
    assert_eq!(got.len(), 1);
    let (key, mail) = &got[0];
    assert_eq!(mail.envelope, b"see you tomorrow");

    assert!(bob_node.delete(key, &mail.delete_token).await >= 1);
    assert!(bob_node.get(key).await.is_empty(), "deleted everywhere it was found");
}

#[tokio::test]
async fn a_contact_finds_the_address_published_after_a_restart() {
    let net = Net::new();
    let nodes = network(&net, 12).await;
    let (alice, bob) = (Person::new(), Person::new());
    let now = net.now();

    let alice_pair = alice.pair_with(&bob);
    for (t, relay) in [(now, "relay-before-restart"), (now + 60_000, "relay-after-restart")] {
        let rec = stored(items::address_record(&alice.signing, &alice_pair, t, relay).unwrap());
        assert!(nodes[5].put(rec).await >= 6);
    }

    let bob_node = new_node(&net, None, false);
    bob_node.add_candidates(&["node-9".to_string()], true);
    bob_node.bootstrap().await;
    let bob_pair = bob.pair_with(&alice);
    let values: Vec<Vec<u8>> = bob_node.get(&items::address_key(&bob_pair, &alice.sign_pk())).await
        .into_iter().map(|i| i.value).collect();
    let (relay, _) = items::open_address_record(&bob_pair, &alice.sign_pk(), &values, now + 120_000).unwrap();
    assert_eq!(relay, "relay-after-restart");
}

#[tokio::test]
async fn nodes_refuse_what_they_should() {
    let net = Net::new();
    let storing = new_node(&net, Some("storing"), true);
    let phone = new_node(&net, Some("phone"), false);
    let challenge = [1; 32];
    let now = net.now();
    let item = StoredItem { key: [2; 32], value: vec![3; 100], delete_hash: None, expires_at_ms: now + 1000 };

    // No work, no storing.
    let lazy = (0u64..).find(|n| !gipny_dht::proto::pow_ok(&challenge, &item, *n, DIFFICULTY)).unwrap();
    let r = storing.handle(&challenge, DhtEnvelope { from: None, req: DhtRequest::Store { item: item.clone(), pow: lazy } });
    assert!(matches!(r, DhtResponse::Error(_)));
    // Work done for another connection does not count either.
    let other = gipny_dht::proto::solve_pow(&[9; 32], &item, 12);
    if !gipny_dht::proto::pow_ok(&challenge, &item, other, DIFFICULTY) {
        let r = storing.handle(&challenge, DhtEnvelope { from: None, req: DhtRequest::Store { item: item.clone(), pow: other } });
        assert!(matches!(r, DhtResponse::Error(_)));
    }
    let pow = gipny_dht::proto::solve_pow(&challenge, &item, DIFFICULTY);
    let r = storing.handle(&challenge, DhtEnvelope { from: None, req: DhtRequest::Store { item: item.clone(), pow } });
    assert!(matches!(r, DhtResponse::Stored));

    // A phone holds nothing for anyone.
    let r = phone.handle(&challenge, DhtEnvelope { from: None, req: DhtRequest::Store { item, pow } });
    assert!(matches!(r, DhtResponse::Error(_)));

    // A node that is not reachable does not claim to be one.
    let leaf = new_node(&net, None, false);
    assert!(matches!(leaf.handle(&challenge, DhtEnvelope { from: None, req: DhtRequest::Ping }), DhtResponse::Error(_)));
}
