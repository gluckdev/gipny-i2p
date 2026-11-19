//! Session state and lifecycle; domain operations live in the session submodules.

mod contacts;
mod delivery;
#[cfg(test)]
mod delivery_tests;
mod dht;
mod files;
mod history;
mod identity;
mod inbox;
mod messages;
mod outbox;
#[path = "session/relay.rs"]
mod relay_loop;
#[cfg(test)]
mod wire_tests;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use thiserror::Error;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::crypto::{
    self, AttachmentCipher, CryptoError, Identity, PreKeyBundle, PreKeyPair, RatchetState,
    X3dhInitial,
};
use crate::db::{
    Contact, Db, DbError, Direction, GroupDeliveryAck, GroupMember, NewOutgoingDm,
    NewOutgoingGroup, PreKeyKind, TrustLevel,
};
use crate::dht_client;
use crate::net::{NetError, I2pNode};
use crate::relay::{
    self, ClientToRelay, EnvelopeBlob, RelayClient, RelayError, RelayToClient,
};
use crate::relay_server::DhtHandler;

use delivery::{acknowledge_delivery, attempt_group_delivery};
use identity::load_or_create_identity;

pub type Result<T> = std::result::Result<T, SessionError>;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("db")]
    Db(#[from] DbError),
    #[error("crypto")]
    Crypto(#[from] CryptoError),
    #[error("net")]
    Net(#[from] NetError),
    #[error("relay")]
    Relay(#[from] RelayError),
    #[error("io")]
    Io(#[from] std::io::Error),
    #[error("codec")]
    Codec,
    #[error("not found")]
    NotFound,
    #[error("state")]
    State,
    #[error("stale opk")]
    StaleOpk,
    #[error("sealed drop")]
    SealedDrop,
}

impl From<bincode::Error> for SessionError {
    fn from(_: bincode::Error) -> Self {
        Self::Codec
    }
}

/// State of a connection to somebody else's relay. Mirrors Core::PeerRelay —
/// `Connecting` and `Failed` are what let the send loop check instead of dial.
enum PeerRelay {
    Ready(relay::RelaySender),
    Connecting,
    /// `failures` in a row: the next wait is [`peer_relay_backoff`] of it.
    Failed {
        until: std::time::Instant,
        failures: u32,
    },
}

/// Give up on a relay's dial after this long: nothing below has a timeout.
/// Our tunnels exist by the time anything dials; finding the relay's LeaseSet
/// and the handshake take seconds to a few tens. It was 90 s, and a dial that
/// hung held a letter all of it before the retry (e2e run 36039568268: an
/// echo 172 s, 90 of them one stuck dial); a retry now follows in 5–10 s.
const PEER_RELAY_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);
/// How long to leave a peer relay alone after `failures` failed dials in a
/// row, instead of redialing every tick: 5 s doubling to 2 min. It was a flat
/// 2 min, and the first dial often fails only because the relay's LeaseSet
/// has not reached the floodfills yet — a contact added a moment after their
/// relay came up then waited two minutes for nothing (e2e run 36033917903:
/// an echo held 85 s behind it).
pub fn peer_relay_backoff(failures: u32) -> std::time::Duration {
    std::time::Duration::from_secs((5u64 << failures.min(5)).min(120))
}

/// Failed dials to a peer relay after which it is dialled again on its own,
/// without a letter asking: 5+10+20+40+80 s, a few minutes in all.
const PEER_RELAY_REDIALS: u32 = 5;

/// When to dial a peer relay again after `failures` failed dials before this
/// one, if at all without a letter asking; see [`PEER_RELAY_REDIALS`].
pub fn peer_relay_redial_after(failures: u32) -> Option<std::time::Duration> {
    (failures < PEER_RELAY_REDIALS).then(|| peer_relay_backoff(failures))
}

/// Desired application streams for a contact's flight window.
/// The common policy also supports bounded, fixed-concurrency measurements.
pub use crate::files::lanes::desired as lanes_for;



const SETTING_IDENTITY_SIGN: &str = "identity_sign";
const SETTING_IDENTITY_DH: &str = "identity_dh";
const SETTING_SIGNED_PREKEY_ID: &str = "signed_prekey_id";
const SETTING_DISPLAY_NAME: &str = "display_name";
const ATTACHMENTS_DIR: &str = "attachments";
const TARGET_OPK: usize = 20;
const RECONNECT_INITIAL_MS: u64 = 500;
const RECONNECT_MAX_MS: u64 = 15_000;
const PING_INTERVAL_SECS: u64 = 20;
const DEAD_THRESHOLD_SECS: u64 = 75;
const BUNDLE_REFRESH_SECS: u64 = 12 * 3600;
/// How often letters left in the durable inbox are tried again (first at start).
const INBOX_RETRY_SECS: u64 = 30;
const PENDING_REQ_TIMEOUT_MS: u64 = 30_000;
/// A bundle asked for ahead of use is used only this fresh: its one-time
/// prekey may be handed to someone else meanwhile, and an init on a spent one
/// is dropped at the far end.
const BUNDLE_PREFETCH_TTL: std::time::Duration = std::time::Duration::from_secs(120);
const FRESH_SESSION_GRACE_MS: i64 = 60_000;
const KEEPALIVE_INCOMING_THRESHOLD: u32 = 100;
/// The largest letter a relay frame carries once padded: the next padding
/// bucket (16 MiB) plus the ratchet header and AEAD tag is over `MAX_FRAME`,
/// so anything past the 4 MiB bucket never arrived (it was 14 MiB, and was
/// dropped at the relay). Large files go in parts; see `files`.
const MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024 - 4;
/// Extra connections to our own relay, when it is not in this process, are
/// held while parts keep coming and closed this long after the last one. A
/// minute closed and reopened them every two or three while a slow path
/// brought a part a minute; each close pushed what they held elsewhere again.
/// Unfinished incoming transfers are dropped after the relay's letter TTL.
const FILE_IN_TTL_MS: i64 = 7 * 24 * 3600 * 1000;
const RETRY_BASE_BACKOFF_MS: i64 = 5_000;
const RETRY_MAX_BACKOFF_MS: i64 = 300_000;
const DHT_ADDRESS_LOOKUP_EVERY: Duration = Duration::from_secs(10 * 60);
type Route = relay::RelaySender;

include!("message_wire.rs");

fn decode_with_padding_fallback(pt: &[u8]) -> Result<WirePayload> {
    if let Some(unpadded) = unpad_payload(pt) {
        if let Ok(p) = decode_payload(&unpadded) {
            return Ok(p);
        }
    }
    Ok(decode_payload(pt)?)
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
    Connected,
    Disconnected,
    IncomingPayload {
        contact_id: i64,
        payload: WirePayload,
        message_id: i64,
    },
    MessageDeliveryExpired(crate::db::DeliveryExpiryNotice),
    MessageDelivered {
        message_id: i64,
    },
    MessageDeleted {
        contact_id: i64,
        message_id: i64,
        deleted_at: i64,
        pending: bool,
        deletion_notice: Option<crate::db::DmDeletionNotice>,
    },
    /// It will not go: too large for one letter (files go in parts; this is
    /// a letter that could not). Marked sent so it is not retried forever.
    MessageFailed {
        message_id: i64,
        reason: String,
    },
    /// A file in parts moved on: `done` of `total` parts, sent (acknowledged
    /// by `contact_id`) or received.
    FileProgress {
        message_id: i64,
        contact_id: i64,
        incoming: bool,
        done: u32,
        total: u32,
    },
    /// A file in parts is whole and is now attachment `attachment_id`.
    FileReceived {
        message_id: i64,
        attachment_id: i64,
    },
    /// A file in parts will not get there: the recipient never acknowledged
    /// a part while alive (an older build), or cancelled; or it came broken.
    FileFailed {
        message_id: i64,
        contact_id: i64,
        reason: String,
    },
    MessageEdited {
        message_id: i64,
        new_body: String,
        buttons: Option<Vec<Vec<WireButton>>>,
    },
    MessagePinned {
        contact_id: Option<i64>,
        group_id: Option<Vec<u8>>,
        message_id: i64,
    },
    MessageUnpinned {
        contact_id: Option<i64>,
        group_id: Option<Vec<u8>>,
        message_id: i64,
    },
    ContactAdded {
        contact_id: i64,
    },
    ContactUpdated {
        contact_id: i64,
    },
    /// The contact deleted us and asked for the chat to go; the contact and
    /// every message with them are gone here now.
    ContactWiped {
        contact_id: i64,
    },
    /// A relay answered with an error frame: a deposit or a publish it would
    /// not take. `reason` is the relay's text, e.g. [`crate::relay::ERR_NOT_SERVED`].
    RelayError {
        reason: String,
    },
}

pub struct SessionManager {
    pub db: Arc<Db>,
    pub node: Arc<I2pNode>,
    pub identity: Arc<Identity>,
    data_dir: PathBuf,
    sessions: Arc<Mutex<HashMap<i64, RatchetState>>>,
    /// Connection to our own relay — where we receive and publish our bundle.
    relay_out: Arc<RwLock<Option<relay::RelaySender>>>,
    /// Connections to other people's relays, keyed by destination.
    peer_relays: Arc<Mutex<HashMap<String, PeerRelay>>>,
    bundle_waiters:
        Arc<Mutex<HashMap<[u8; 32], Vec<tokio::sync::oneshot::Sender<Option<Vec<u8>>>>>>>,
    /// Bundles asked for ahead of a first letter (see `relay_for`), by signing
    /// key, with when they came: a new conversation then skips that round trip.
    bundle_cache: Arc<Mutex<HashMap<[u8; 32], (Vec<u8>, std::time::Instant)>>>,
    /// Sessions we opened ourselves (contact → our init's ratchet key) that
    /// the contact has not answered on yet: an X3dhInit from them meanwhile
    /// crossed ours. See `crossing_init_stands`.
    own_inits: Arc<Mutex<HashMap<i64, [u8; 32]>>>,
    /// Their sessions that lost to ours, kept so what they sent on one before
    /// taking ours is still read, not dropped and waited for again.
    lost_inits: Arc<Mutex<HashMap<i64, RatchetState>>>,
    /// Inbox rows being processed right now by the receive path, which the
    /// periodic retry must leave alone.
    inbox_busy: Arc<std::sync::Mutex<std::collections::HashSet<i64>>>,
    /// When each contact was last heard from (anything decrypted): a part is
    /// resent on a timer only to someone alive since it went.
    heard: Arc<std::sync::Mutex<HashMap<i64, i64>>>,
    /// Extra connections to a contact's relay, for file parts only (by relay
    /// destination). One i2p stream carries at most its window per round
    /// trip; parts spread over several go that many times faster.
    file_lanes: Arc<Mutex<HashMap<String, Vec<relay::RelaySender>>>>,
    file_metrics: crate::files::metrics::FileMetrics,
    /// Relays a lane is being dialled to, so one is dialled at a time.
    file_lanes_dialling: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// How parts go to each contact (window, round trip), by contact.
    flows: Arc<std::sync::Mutex<HashMap<i64, crate::files::Flow>>>,
    session_created_at: Arc<Mutex<HashMap<i64, i64>>>,
    incoming_since_send: Arc<Mutex<HashMap<i64, u32>>>,
    dht: Arc<dht_client::Node>,
    /// Our own relay when it runs in this process: reached over a pipe.
    local_relay: std::sync::Mutex<Option<crate::EphemeralRelay>>,
    dht_addr_asked: Arc<Mutex<HashMap<i64, Instant>>>,
    dht_kick: Arc<tokio::sync::Notify>,
    send_kick: Arc<tokio::sync::Notify>,
    events: mpsc::Sender<SessionEvent>,
    tasks: std::sync::Mutex<Option<Vec<JoinHandle<()>>>>,
    runtime: tokio::runtime::Handle,
    shutdown_done: tokio::sync::watch::Sender<bool>,
}

impl SessionManager {
    pub async fn start(
        data_dir: PathBuf,
        db: Arc<Db>,
        node: Arc<I2pNode>,
    ) -> Result<(Arc<Self>, mpsc::Receiver<SessionEvent>)> {
        std::fs::create_dir_all(data_dir.join(ATTACHMENTS_DIR))?;
        let identity = Arc::new(load_or_create_identity(&db)?);
        db.configure_dm_identity(&identity.card().sign_pk)?;
        db.cleanup_deleted_files(&data_dir.join(ATTACHMENTS_DIR))?;
        let dht = dht_client::new_node(node.clone(), db.clone());
        let (events_tx, events_rx) = mpsc::channel(256);
        let this = Arc::new(Self {
            db: db.clone(),
            node: node.clone(),
            identity,
            data_dir,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            relay_out: Arc::new(RwLock::new(None)),
            peer_relays: Arc::new(Mutex::new(HashMap::new())),
            bundle_waiters: Arc::new(Mutex::new(HashMap::new())),
            bundle_cache: Arc::new(Mutex::new(HashMap::new())),
            own_inits: Arc::new(Mutex::new(HashMap::new())),
            lost_inits: Arc::new(Mutex::new(HashMap::new())),
            inbox_busy: Arc::default(),
            heard: Arc::new(std::sync::Mutex::new(HashMap::new())),
            file_lanes: Arc::new(Mutex::new(HashMap::new())),
            file_metrics: Default::default(),
            file_lanes_dialling: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            flows: Arc::new(std::sync::Mutex::new(HashMap::new())),
            session_created_at: Arc::new(Mutex::new(HashMap::new())),
            incoming_since_send: Arc::new(Mutex::new(HashMap::new())),
            dht,
            local_relay: std::sync::Mutex::new(None),
            dht_addr_asked: Arc::new(Mutex::new(HashMap::new())),
            dht_kick: Arc::new(tokio::sync::Notify::new()),
            send_kick: Arc::new(tokio::sync::Notify::new()),
            events: events_tx,
            tasks: std::sync::Mutex::new(Some(Vec::new())),
            runtime: tokio::runtime::Handle::current(),
            shutdown_done: tokio::sync::watch::channel(false).0,
        });
        this.ensure_prekeys().await?;
        let terminal = crate::EphemeralRelay::start_personal(&db, this.identity.clone(), Some(this.dht_handler())).await?;
        *this.local_relay.lock().unwrap_or_else(|p| p.into_inner()) = Some(terminal);
        this.clone().spawn_relay_loop();
        this.clone().spawn_resume_acks();
        this.clone().spawn_send_loop();
        this.clone().spawn_bundle_refresh_loop();
        this.clone().spawn_dht_loop();
        this.warm_peer_relays();
        Ok((this, events_rx))
    }

    /// Closing the registry and spawning a task are mutually exclusive. In
    /// particular an interrupted dial cannot start a redial after shutdown.
    fn spawn_task(&self, task: impl std::future::Future<Output = ()> + Send + 'static) {
        let mut registry = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(handles) = registry.as_mut() {
            handles.retain(|handle| !handle.is_finished());
            handles.push(self.runtime.spawn(task));
        }
    }

    /// Wait for all session tasks and the personal listener to release their
    /// connections and published destination before a same-profile restart.
    pub async fn shutdown_and_wait(&self) {
        self.shutdown();
        let mut done = self.shutdown_done.subscribe();
        let _ = done.wait_for(|finished| *finished).await;
    }

    pub fn shutdown(&self) {
        let Some(handles) = self.tasks.lock().unwrap_or_else(|p| p.into_inner()).take() else {
            return;
        };
        for handle in &handles {
            handle.abort();
        }
        let terminal = self.local_relay.lock().unwrap_or_else(|p| p.into_inner()).take();
        let relay = self.relay_out.clone();
        let peers = self.peer_relays.clone();
        let lanes = self.file_lanes.clone();
        let done = self.shutdown_done.clone();
        self.runtime.spawn(async move {
            for handle in handles {
                let _ = handle.await;
            }
            *relay.write().await = None;
            peers.lock().await.clear();
            lanes.lock().await.clear();
            if let Some(terminal) = terminal {
                terminal.shutdown().await;
            }
            done.send_replace(true);
        });
    }

    fn spawn_bundle_refresh_loop(self: Arc<Self>) {
        let this = self.clone();
        self.spawn_task(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(BUNDLE_REFRESH_SECS));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            tick.tick().await;
            // Letters left in the inbox by a crash or a storage failure.
            let mut inbox_retry = tokio::time::interval(Duration::from_secs(INBOX_RETRY_SECS));
            inbox_retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = inbox_retry.tick() => this.drain_inbox().await,
                    _ = tick.tick() => {
                        this.republish_bundle().await;
                        this.gc_files();
                    }
                }
            }
        });
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// See `Core`'s twin: a wrong length is an error, never a panic.
fn to_arr32(v: Vec<u8>) -> Result<[u8; 32]> {
    <[u8; 32]>::try_from(v).map_err(|_| SessionError::State)
}

/// When two X3dhInits cross, the session opened by the side with the lower
/// signing key stands — the same answer on both ends, with no waiting. The
/// app (Core) decides by this too.
pub fn ours_stands(my_sign: &[u8], their_sign: &[u8]) -> bool {
    my_sign < their_sign
}

fn hex_short(b: &[u8]) -> String {
    let mut s = String::new();
    for x in &b[..8.min(b.len())] {
        s.push_str(&format!("{:02x}", x));
    }
    s
}
