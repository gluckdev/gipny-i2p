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
    /// Gone the way an i2p destination goes: a dial is never refused, it
    /// just never completes.
    silent: Mutex<HashSet<String>>,
    dials: Mutex<HashMap<String, usize>>,
    clock: Arc<AtomicU64>,
}

impl Net {
    fn new() -> Arc<Self> {
        Arc::new(Self { nodes: Mutex::default(), offline: Mutex::default(), silent: Mutex::default(), dials: Mutex::default(), clock: Arc::new(AtomicU64::new(20_000 * DAY_MS)) })
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
            *self.net.dials.lock().unwrap().entry(destination.clone()).or_default() += 1;
            if self.net.silent.lock().unwrap().contains(&destination) {
                std::future::pending::<()>().await;
            }
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
    NodeConfig { pow_difficulty: DIFFICULTY, dial_timeout: Duration::from_secs(5), call_timeout: Duration::from_secs(5), ..NodeConfig::default() }
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

/// A contact's node that went away silently costs one dial timeout, not one
/// per letter: the e2e run 35947260726 spent 120 s on it for every lookup
/// until it had failed three times, and three letters did not leave in 450 s.
#[tokio::test(start_paused = true)]
async fn an_absent_node_is_waited_for_once() {
    let net = Net::new();
    let _nodes = network(&net, 6).await;
    let alice = new_node(&net, Some("alice"), true);
    alice.add_candidates(&["node-0".to_string()], true);
    alice.bootstrap().await;
    net.silent.lock().unwrap().insert("node-3".into());
    net.dials.lock().unwrap().clear();

    let (a, b) = (Person::new(), Person::new());
    let started = tokio::time::Instant::now();
    for i in 0..5u8 {
        let letter = stored(items::mail(&a.pair_with(&b), &b.sign_pk(), net.now(), &[i; 16]).unwrap());
        assert!(alice.put(letter).await >= 1, "letter {i} was not stored elsewhere");
    }
    let dials = net.dials.lock().unwrap().get("node-3").copied().unwrap_or(0);
    assert_eq!(dials, 1, "the silent node was dialled {dials} times");
    assert!(started.elapsed() < config().dial_timeout * 2, "took {:?}", started.elapsed());
}

/// A letter counts as in the network only once another node holds it; and a
/// node whose only peer failed once still tries it rather than giving up.
/// e2e run 35963073832: three answers "left in the network" in one
/// millisecond, all on their writer, who then closed.
#[tokio::test(start_paused = true)]
async fn a_letter_is_in_the_network_only_when_someone_else_holds_it() {
    let net = Net::new();
    let seed = new_node(&net, Some("seed"), true);
    let bob = new_node(&net, Some("bob"), true);
    bob.add_candidates(&["seed".to_string()], true);
    bob.bootstrap().await;
    let (a, b) = (Person::new(), Person::new());
    let letter = |n: u8| stored(items::mail(&a.pair_with(&b), &b.sign_pk(), net.now(), &[n; 16]).unwrap());

    // The seed is away: our own copy is kept, but nothing is in the network.
    net.offline.lock().unwrap().insert("seed".into());
    assert_eq!(bob.put(letter(1)).await, 0, "a copy only we hold counted as stored");

    // The seed is back. It failed once, and it is the only node we know: it
    // must still be tried.
    net.offline.lock().unwrap().remove("seed");
    assert_eq!(bob.put(letter(2)).await, 1, "the only known node was not tried after one failure");
    let _ = seed;
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

/// A peer that names a hundred addresses nobody has ever heard of, however many
/// the table already holds.
///
/// The honest `handle` path cannot do this any more — a node only hands out
/// what has answered it — so a node that means to fill our table has to lie
/// past us, and this is what that looks like on the wire.
struct FillerTransport {
    liar: String,
    fabrications: Vec<NodeInfo>,
}

struct FillerConn {
    liar: String,
    fabrications: Vec<NodeInfo>,
    challenge: [u8; 32],
}

impl Connection for FillerConn {
    fn challenge(&self) -> [u8; 32] {
        self.challenge
    }

    fn call(&mut self, env: DhtEnvelope) -> BoxFuture<'_, Result<DhtResponse, NetError>> {
        let (liar, fabrications) = (self.liar.clone(), self.fabrications.clone());
        Box::pin(async move {
            match env.req {
                DhtRequest::FindNode { .. } => Ok(DhtResponse::Nodes {
                    me: Some(NodeInfo { destination: liar, stores: true, version: PROTOCOL_VERSION }),
                    nodes: fabrications,
                }),
                _ => Err(NetError("nothing else".into())),
            }
        })
    }
}

impl Transport for FillerTransport {
    type Conn = FillerConn;

    fn open(&self, destination: &str) -> BoxFuture<'_, Result<FillerConn, NetError>> {
        let (liar, fabrications) = (self.liar.clone(), self.fabrications.clone());
        let asked = destination.to_string();
        Box::pin(async move {
            // Only the liar is there. The addresses it named are not, which is
            // the whole point of naming them.
            if asked != liar {
                return Err(NetError("no such destination".into()));
            }
            Ok(FillerConn { liar, fabrications, challenge: crypto::random_32() })
        })
    }
}

fn fabricated(n: usize) -> Vec<NodeInfo> {
    (0..n).map(|i| NodeInfo { destination: format!("ghost-{i}"), stores: true, version: PROTOCOL_VERSION }).collect()
}

/// Issue #99: one node may not talk a hundred invented destinations into this
/// table, and none of them may be handed on to anybody else before they have
/// answered.
#[tokio::test]
async fn a_node_cannot_fill_the_table_with_addresses_that_do_not_exist() {
    let net = Net::new();
    let ghosts = fabricated(100);
    let victim = Arc::new(DhtNode::new(
        Arc::new(FillerTransport { liar: "liar".into(), fabrications: ghosts.clone() }),
        Arc::new(MemStorage::new(StoreLimits::default())),
        config(),
        { let clock = net.clock.clone(); Arc::new(move || clock.load(Ordering::Relaxed)) },
    ));
    victim.add_candidates(&["liar".to_string()], true);

    let _ = victim.lookup(&[7; 32]).await;

    // The liar itself is worth having — it answered, even if it lied.
    assert_eq!(victim.peer_count(), 1, "the liar answered, so it is one known node");
    // Its hundred inventions are not the whole table: it spent an allowance
    // and the rest were dropped unread.
    assert!(
        victim.table_len() <= config().max_introduced as usize + 1,
        "a node introduced {} addresses at once, allowance is {}",
        victim.table_len().saturating_sub(1),
        config().max_introduced,
    );
    assert!(victim.table_len() < 50, "table grew to {} on one liar's word", victim.table_len());

    // And a fresh node with nothing confirmed says so rather than passing the
    // ghosts along: it is in this table, it just has nobody to vouch for.
    let honest = new_node(&net, Some("honest"), true);
    honest.add_candidates(&["liar".to_string()], true);
    let challenge = [3; 32];
    let r = honest.handle(&challenge, DhtEnvelope { from: None, req: DhtRequest::FindNode { target: [7; 32] } });
    let DhtResponse::Nodes { me, nodes } = r else { panic!("FindNode answers with nodes") };
    assert!(me.is_some(), "it is a node and says so");
    assert!(nodes.is_empty(), "nothing has answered it, so it vouches for nobody, got {}", nodes.len());
}
