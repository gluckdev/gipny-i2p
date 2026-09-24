//! The relay as a node of the gipny relay network (`--dht`).
//!
//! Same protocol and node as the apps and the agent (`gipny-dht`), with two
//! differences that make it a seed: what it holds survives a restart
//! (`dht-items.db`), and so does the table of nodes that answered it
//! (`dht-peers.db`). Its node id is the hash of the relay's persistent
//! destination, so the next run is the same node in the same place.
//!
//! Requests come in on the relay's own connections: a `Dht` frame in place of
//! `Auth` (see `handle_client` in main.rs), exactly as libcore's relay_server
//! serves them. Outgoing requests go through one transient, unpublished SAM
//! session, as libcore's `I2pTransport` does through the app's session.

use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::future::BoxFuture;
use gipny_dht::node::{self as dht_node, Connection, DhtNode, NetError, NodeConfig, Transport};
use gipny_dht::proto::{DhtEnvelope, DhtResponse, NodeInfo, PROTOCOL_VERSION};
use gipny_dht::store::StoreLimits;
use tokio::sync::Mutex;
use yosemite::{style, DestinationKind, Session, SessionOptions, Stream as I2pStream};

use crate::dht_store::{PeerStore, SqliteStorage};
use crate::proto::*;
use crate::{recv_frame, send_frame};

/// Answers one relay-network request, given the connection's challenge.
pub type DhtHandler = Arc<dyn Fn(&[u8; 32], Vec<u8>) -> Vec<u8> + Send + Sync>;

type Node = DhtNode<RelayTransport, SqliteStorage>;

/// Same cadence as the apps (libcore `dht_client::MAINTAIN_EVERY`).
const MAINTAIN_EVERY: Duration = Duration::from_secs(45 * 60);
/// Our own leaseSet has to be out before anyone can answer us back.
const BOOTSTRAP_DELAY: Duration = Duration::from_secs(90);
/// Nodes silent this long are dropped from the saved table.
const PEER_FORGET_MS: u64 = 14 * 24 * 3600 * 1000;
/// Dials failed in a row before the outgoing SAM session is rebuilt.
const REBUILD_AFTER_FAILURES: u32 = 8;

pub struct Options {
    pub stores: bool,
    pub seeds: Vec<String>,
}

/// Open the node's stores, announce it at `destination` and start its upkeep.
/// Returns the handler the relay's accept loop passes `Dht` frames to.
pub fn start(data_dir: &Path, sam_port: u16, destination: &str, opts: Options) -> anyhow::Result<DhtHandler> {
    let items = Arc::new(SqliteStorage::open(&data_dir.join("dht-items.db"), StoreLimits::default())?);
    let peers = Arc::new(PeerStore::open(&data_dir.join("dht-peers.db"))?);
    let node: Arc<Node> = Arc::new(DhtNode::new(
        Arc::new(RelayTransport { sam_port, session: Mutex::new(None), failures: AtomicU32::new(0) }),
        items,
        NodeConfig::default(),
        dht_node::system_clock(),
    ));
    node.set_me(Some(NodeInfo { destination: destination.to_string(), stores: opts.stores, version: PROTOCOL_VERSION }));
    node.add_candidates(&opts.seeds, true);
    node.add_candidates(&peers.load(512), false);
    let stats = node.storage_stats();
    eprintln!(
        "[dht] node up: stores={} seeds={} held items={} bytes={}",
        opts.stores, opts.seeds.len(), stats.items, stats.bytes
    );

    tokio::spawn({
        let node = node.clone();
        async move {
            tokio::time::sleep(BOOTSTRAP_DELAY).await;
            node.bootstrap().await;
            eprintln!("[dht] joined: {} nodes known", node.peer_count());
            // A restart may come back to a changed neighbourhood: what we hold
            // belongs with the nodes closest to it now.
            node.republish().await;
            peers.save(&node.known_peers());

            let mut tick = tokio::time::interval(MAINTAIN_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            tick.tick().await;
            loop {
                tick.tick().await;
                if node.peer_count() == 0 {
                    node.add_candidates(&opts.seeds, true);
                    node.add_candidates(&peers.load(512), false);
                    node.bootstrap().await;
                }
                node.maintain().await;
                node.republish().await;
                peers.save(&node.known_peers());
                peers.gc_old(now_ms().saturating_sub(PEER_FORGET_MS));
                let stats = node.storage_stats();
                eprintln!("[dht] {} nodes known, holding {} items, {} bytes", node.peer_count(), stats.items, stats.bytes);
            }
        }
    });

    Ok(Arc::new(move |challenge: &[u8; 32], request: Vec<u8>| {
        let answer = match dht_node::decode_envelope(&request) {
            Some(env) => node.handle(challenge, env),
            None => DhtResponse::Error("undecodable request".into()),
        };
        dht_node::encode_response(&answer)
    }))
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn net_err(e: impl std::fmt::Display) -> NetError {
    NetError(e.to_string())
}

// ── transport ────────────────────────────────────────────────────────────

pub struct RelayTransport {
    sam_port: u16,
    /// Opened on first dial, dropped on a failed one so the next dial rebuilds
    /// it. A tunnel pool per call would cost tens of seconds each.
    session: Mutex<Option<Session<style::Stream>>>,
    /// Dials failed in a row. One node being away is normal; every dial
    /// failing means the session died with a router restart.
    failures: AtomicU32,
}

pub struct RelayConn {
    stream: Pin<Box<I2pStream>>,
    challenge: [u8; 32],
}

impl RelayTransport {
    async fn dial(&self, destination: &str) -> Result<I2pStream, NetError> {
        let fut = {
            let mut session = self.session.lock().await;
            if session.is_none() {
                let opts = SessionOptions {
                    // Unique and secret: see `sam_session_id`.
                    nickname: sam_session_id("gipny-relay-dht"),
                    destination: DestinationKind::Transient,
                    samv3_tcp_port: self.sam_port,
                    // Outgoing only: nobody needs to find this destination.
                    publish: false,
                    gzip: false,
                    ..Default::default()
                };
                *session = Some(Session::<style::Stream>::new(opts).await.map_err(|e| NetError(format!("SAM session: {e}")))?);
            }
            // Only the clone of the controller happens under the lock;
            // concurrent dials of a lookup proceed in parallel.
            session.as_mut().expect("just set").connect_detached(destination)
        };
        match fut.await {
            Ok(stream) => {
                self.failures.store(0, Ordering::Relaxed);
                Ok(stream)
            }
            Err(e) => {
                // A reply yosemite cannot parse poisons the session's controller
                // (see main.rs); a router restart kills it without a word. Either
                // way the next dial opens a new one.
                let failed = self.failures.fetch_add(1, Ordering::Relaxed) + 1;
                if matches!(e, yosemite::Error::Malformed) || failed >= REBUILD_AFTER_FAILURES {
                    eprintln!("[dht] {failed} dial(s) failed in a row ({e}); opening a new SAM session");
                    self.failures.store(0, Ordering::Relaxed);
                    *self.session.lock().await = None;
                }
                Err(net_err(e))
            }
        }
    }
}

impl Transport for RelayTransport {
    type Conn = RelayConn;

    fn open(&self, destination: &str) -> BoxFuture<'_, Result<RelayConn, NetError>> {
        let destination = destination.to_string();
        Box::pin(async move {
            let mut stream = Box::pin(self.dial(&destination).await?);
            match recv_frame::<_, RelayToClient>(&mut stream).await.map_err(net_err)? {
                RelayToClient::Challenge(challenge) => Ok(RelayConn { stream, challenge }),
                _ => Err(NetError("expected a challenge".into())),
            }
        })
    }
}

impl Connection for RelayConn {
    fn challenge(&self) -> [u8; 32] {
        self.challenge
    }

    fn call(&mut self, env: DhtEnvelope) -> BoxFuture<'_, Result<DhtResponse, NetError>> {
        Box::pin(async move {
            send_frame(&mut self.stream, &ClientToRelay::Dht(dht_node::encode_envelope(&env))).await.map_err(net_err)?;
            match recv_frame::<_, RelayToClient>(&mut self.stream).await.map_err(net_err)? {
                RelayToClient::Dht(bytes) => dht_node::decode_response(&bytes).ok_or_else(|| NetError("undecodable answer".into())),
                // A relay that is not a node says so; the node drops it.
                RelayToClient::Error(e) => Ok(DhtResponse::Error(e)),
                _ => Err(NetError("unexpected frame".into())),
            }
        })
    }
}
