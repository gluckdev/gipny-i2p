use gipny_libcore::dht_client;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gipny_libcore::crypto::fill_random;
use serde::Serialize;
use thiserror::Error;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

#[cfg(target_os = "android")]
use super::android::install_apk;
use gipny_libcore::crypto::{
    self, AttachmentCipher, Identity, IdentityCard, PreKeyBundle, PreKeyPair, RatchetState,
};
use gipny_libcore::db::{
    Attachment, Db, Direction, GroupMember, NewOutgoingDm, NewOutgoingGroup, PreKeyKind,
    RequestState, TrustLevel,
};
use gipny_libcore::net::{NetError, I2pNode};
use gipny_libcore::relay::{
    self, ClientToRelay, EnvelopeBlob, RelayClient, RelayToClient, DEFAULT_RELAY,
};
#[cfg(not(target_os = "android"))]
use gipny_libcore::update::InstallOutcome;
use gipny_libcore::update::{Component as UpdateComponent, UpdateError, UpdateInfo, Updater};

pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("db")]
    Db(#[from] gipny_libcore::db::DbError),
    #[error("crypto")]
    Crypto(#[from] gipny_libcore::crypto::CryptoError),
    #[error("net")]
    Net(#[from] NetError),
    #[error("relay")]
    Relay(#[from] gipny_libcore::relay::RelayError),
    #[error("io")]
    Io(#[from] std::io::Error),
    #[error("update")]
    Update(#[from] UpdateError),
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

impl From<bincode::Error> for CoreError {
    fn from(_: bincode::Error) -> Self {
        Self::Codec
    }
}

const SETTING_IDENTITY_SIGN: &str = "identity_sign";
const SETTING_IDENTITY_DH: &str = "identity_dh";
const SETTING_SIGNED_PREKEY_ID: &str = "signed_prekey_id";
const SETTING_RELAY_ADDRESS: &str = "relay_address";
const SETTING_RELAY_MODE: &str = "relay_mode";
/// A contact's relay that has not answered for this long, while mail for them
/// is queued, is reported to the user: with built-in relays it most likely
/// means the contact restarted and collects somewhere else now.
const CONTACT_UNREACHABLE_AFTER: Duration = Duration::from_secs(600);
/// Beyond this a round trip is not a slow link but a message that sat in the
/// retry queue, or a clock that moved. Ten minutes is already far past the
/// worst honest case (a cold router rebuilding tunnels).
const MAX_PLAUSIBLE_RTT_MS: i64 = 600_000;
/// How far a peer's clock may differ from ours before we stop calling their
/// letter "just now". Generous on purpose: phones and laptops that have been
/// asleep drift by minutes, and the alternative is a contact who is online
/// and never shown as such.
const LIVE_SKEW_MS: i64 = 5 * 60_000;
const SETTING_DISMISSED_UPDATE: &str = "dismissed_update_version";
/// Default on: absent or anything but `"0"` means auto-update stays on,
/// matching `attachment_privacy`'s convention.
const SETTING_AUTO_UPDATE: &str = "auto_update";
/// What the interface keeps in the vault for itself (JSON it does not
/// interpret here): contact folders, chosen avatars, chat preferences such as
/// the disappearing-message timer. Only these keys.
const UI_DATA_KEYS: &[&str] = &["contact_folders", "avatars", "chat_prefs"];
/// A local log of copies kept by an earlier build, with text fragments in it;
/// removed on start (peer events replaced it).
const RETIRED_UI_DATA: &str = "ui_exfiltration_receipts";
const MAX_UI_DATA_BYTES: usize = 64 * 1024;
const ATTACHMENTS_DIR: &str = "attachments";
const TARGET_OPK: usize = 20;
const PURGE_INTERVAL_SECS: u64 = 60;
/// How often letters left in the durable inbox are tried again. The first
/// tick is at start, so a crash's leftovers go first.
const INBOX_RETRY_SECS: u64 = 30;
const RECONNECT_INITIAL_MS: u64 = 500;
const RECONNECT_MAX_MS: u64 = 15_000;
/// An unconfirmed address announcement is repeated no more often than this.
const ANNOUNCE_REPEAT_EVERY: Duration = Duration::from_secs(20 * 60);

/// A contact whose relay is silent gets one address lookup in the network per
/// this interval; each one costs tunnels and tens of seconds.
const DHT_ADDRESS_LOOKUP_EVERY: Duration = Duration::from_secs(10 * 60);

/// How often to look in the network for letters left while we were away.
const DHT_COLLECT_EVERY: Duration = Duration::from_secs(10 * 60);

/// Letters are remembered as handled for as long as one can live in the
/// network, plus a day.
const DHT_SEEN_TTL_MS: i64 = 8 * 24 * 3600 * 1000;

/// How far back the first pass after a launch looks; a letter lives a week.
const DHT_COLLECT_DAYS_FIRST: u32 = 7;
/// Later passes only need today and (around midnight) yesterday.
const DHT_COLLECT_DAYS: u32 = 2;

/// Where an outgoing letter goes. `Relay` is a live connection to the relay
/// the contact collects from; `Dht` leaves it in the relay network, where it
/// waits until they come back (see libcore/src/dht_client.rs).
enum Route {
    Relay(mpsc::Sender<ClientToRelay>),
    Dht,
}

/// State of a connection to somebody else's relay.
///
/// `Connecting` and `Failed` exist so the send loop never dials: it checks this
/// map, and either gets a live sender or moves on to the next contact.
enum PeerRelay {
    Ready(mpsc::Sender<ClientToRelay>),
    Connecting,
    /// `failures` in a row: the next wait is [`peer_relay_backoff`] of it.
    Failed {
        until: Instant,
        failures: u32,
    },
}

/// The relay built before unlock, still coming up or up.
pub type PrebuiltRelay =
    JoinHandle<std::result::Result<gipny_libcore::EphemeralRelay, gipny_libcore::NetError>>;

/// A delete request that has not reached a contact in this long is given up
/// on, and the contact deleted here anyway.
const WIPE_TTL_MS: i64 = 7 * 24 * 3600 * 1000;

fn wipe_key(contact_id: i64) -> String {
    format!("wipe_pending_{contact_id}")
}

/// How long to wait for another person's relay to answer before giving up on
/// this attempt. Nothing in the dial path has a timeout of its own, and opening
/// an i2p destination means building tunnels, so an unreachable relay would
/// otherwise hang its connection task forever.
/// As in libcore's session.rs: 45 s, not 90 — a hung dial is retried in
/// 5–10 s instead of holding a letter a minute and a half.
const PEER_RELAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(45);
/// Unfinished incoming transfers are dropped after the relay's letter TTL.
const FILE_IN_TTL_MS: i64 = 7 * 24 * 3600 * 1000;

const PING_INTERVAL_SECS: u64 = 20;
const DEAD_THRESHOLD_SECS: u64 = 75;
const BUNDLE_REFRESH_SECS: u64 = 12 * 3600;
const EVENTS_CAPACITY: usize = 1024;
const PENDING_REQ_TIMEOUT_MS: u64 = 30_000;
/// A bundle asked for ahead of use is used only this fresh: its one-time
/// prekey may be handed to someone else meanwhile, and an init on a spent one
/// is dropped at the far end.
const BUNDLE_PREFETCH_TTL: Duration = Duration::from_secs(120);
/// The largest letter a relay frame carries once padded: the next padding
/// bucket (16 MiB) plus the ratchet header and AEAD tag is over `MAX_FRAME`,
/// so anything past the 4 MiB bucket never arrived (it was 14 MiB, and was
/// dropped at the relay). Large files go in parts; see `files`.
const MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024 - 4;
const RETRY_BASE_BACKOFF_MS: i64 = 5_000;
const RETRY_MAX_BACKOFF_MS: i64 = 300_000;
const FRESH_SESSION_GRACE_MS: i64 = 60_000;
const KEEPALIVE_INCOMING_THRESHOLD: u32 = 100;
/// Unanswered introductions kept at once; the oldest goes first.
const MAX_INCOMING_REQUESTS: usize = 50;

use gipny_libcore::agent::{self, ExecOptions, BODY_GRANT, BODY_REVOKE, SETTING_AGENT_MASTER};
use gipny_libcore::{
    WireAttachment, WireButton, WireConsole, WireGroupRef, WireMember, WirePayload,
};
use gipny_libcore::{CONSOLE_COMMAND, CONSOLE_GRANT, CONSOLE_OFF, CONSOLE_REVOKE};

#[path = "core/agent.rs"]
mod agent_loop;
mod contacts;
mod dht;
mod events;
mod files;
mod groups;
mod history;
mod hosted_relay;
mod identity;
mod inbox;
mod lifecycle;
mod messages;
mod network;
mod outbox;
#[path = "core/relay.rs"]
mod relay_loop;
mod updates;

pub use agent_loop::AgentMaster;
pub use events::CoreEvent;
pub use files::PendingAttachment;
pub use hosted_relay::{DialState, HostedRelayState, RelayInfo, RelayMode};
pub use network::{LinkRoute, LinkStats, Rtt};

fn hex_bytes(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push_str(&format!("{:02x}", x));
    }
    s
}
use gipny_libcore::session::{
    collect_lane_holds, collect_lane_wanted, decode_payload, encode_payload,
    ours_stands, pad_payload, peer_relay_backoff, peer_relay_redial_after, unpad_payload,
    WireFileAck, WireFileChunk, WireFileOffer, WirePin, WireReply,
};

fn decode_with_padding_fallback(pt: &[u8]) -> std::result::Result<WirePayload, bincode::Error> {
    if let Some(unpadded) = unpad_payload(pt) {
        if let Ok(p) = decode_payload(&unpadded) {
            return Ok(p);
        }
    }
    decode_payload(pt)
}

type BundleWaiter = tokio::sync::oneshot::Sender<Option<Vec<u8>>>;

pub struct Core {
    db: Arc<Db>,
    node: Arc<I2pNode>,
    identity: Arc<Identity>,
    sessions: Arc<Mutex<HashMap<i64, RatchetState>>>,
    events: mpsc::Sender<CoreEvent>,
    data_dir: PathBuf,
    /// Connection to our own relay — where we receive, and where our prekey
    /// bundle is published.
    relay_out: Arc<RwLock<Option<mpsc::Sender<ClientToRelay>>>>,
    /// Connections to other people's relays, keyed by destination. A message
    /// goes to where its recipient collects it, which is usually not here.
    peer_relays: Arc<Mutex<HashMap<String, PeerRelay>>>,
    bundle_waiters: Arc<Mutex<HashMap<[u8; 32], Vec<BundleWaiter>>>>,
    /// Bundles asked for ahead of a first letter (see `relay_for`), by signing
    /// key, with when they came: a new conversation then skips that round trip.
    bundle_cache: Arc<Mutex<HashMap<[u8; 32], (Vec<u8>, Instant)>>>,
    send_kick: Arc<tokio::sync::Notify>,
    /// Sessions we opened (contact → our init's ratchet key) not yet answered
    /// on: an X3dhInit from them meanwhile crossed ours. See `ours_stands`.
    own_inits: Arc<Mutex<HashMap<i64, [u8; 32]>>>,
    /// Their sessions that lost to ours, kept so what they sent on one before
    /// taking ours is still read, not dropped and waited for again.
    lost_inits: Arc<Mutex<HashMap<i64, RatchetState>>>,
    /// Inbox rows being processed right now by the receive path, which the
    /// periodic retry must leave alone.
    inbox_busy: Arc<std::sync::Mutex<std::collections::HashSet<i64>>>,
    /// When each contact was last heard from: a part is resent on a timer
    /// only to someone alive since it went.
    heard: Arc<std::sync::Mutex<HashMap<i64, i64>>>,
    /// Extra connections to a contact's relay for file parts (by relay
    /// destination); see libcore's session.rs `file_lanes`.
    file_lanes: Arc<Mutex<HashMap<String, Vec<mpsc::Sender<ClientToRelay>>>>>,
    file_lanes_dialling: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// When the last file part came (ms), and the extra connections to our
    /// own relay taking them; see libcore's session.rs `spawn_collect_lanes`.
    part_seen: Arc<std::sync::atomic::AtomicI64>,
    /// How parts go to each contact (window, round trip); see libcore's
    /// files.rs `Flow`.
    flows: Arc<std::sync::Mutex<HashMap<i64, gipny_libcore::files::Flow>>>,
    collect_lanes: Arc<std::sync::atomic::AtomicUsize>,
    collect_kick: Arc<tokio::sync::Notify>,
    session_created_at: Arc<Mutex<HashMap<i64, i64>>>,
    incoming_since_send: Arc<Mutex<HashMap<i64, u32>>>,
    updater: Arc<Updater>,
    pending_update: Arc<Mutex<Option<UpdateInfo>>>,
    update_install: Arc<Mutex<()>>,
    prepared_update: Arc<Mutex<Option<String>>>,
    tasks: Arc<std::sync::Mutex<Vec<JoinHandle<()>>>>,
    /// Ids of console commands to run, in order, by the single agent worker.
    agent_tx: mpsc::UnboundedSender<i64>,
    /// The relay this process hosts for itself in built-in mode. Holding it is
    /// what keeps it serving; dropping it takes the destination away.
    hosted_relay: Arc<std::sync::Mutex<Option<gipny_libcore::EphemeralRelay>>>,
    hosted_state: Arc<std::sync::RwLock<HostedRelayState>>,
    /// How our own relay connection is going, for the banner that used to say
    /// only that there was none.
    relay_dial: Arc<std::sync::RwLock<DialState>>,
    hosted_task: Arc<std::sync::Mutex<Option<JoinHandle<()>>>>,
    /// A relay whose tunnels were built before unlock (see `lib.rs`
    /// `prewarm_network`), taken by the first `bring_up_hosted_relay`.
    prebuilt_relay: std::sync::Mutex<Option<PrebuiltRelay>>,
    /// Contacts that have not been told this launch's relay address yet.
    announce_pending: Arc<Mutex<HashSet<i64>>>,
    /// When each of them was last told, so the repeat is paced.
    announce_sent_at: Arc<Mutex<HashMap<i64, Instant>>>,
    /// When each peer relay first stopped answering, and who was told about it.
    relay_down_since: Arc<Mutex<HashMap<String, Instant>>>,
    unreachable_reported: Arc<Mutex<HashSet<i64>>>,
    /// Our node in the relay network, answering through the built-in relay.
    dht: Arc<dht_client::Node>,
    /// When we last asked the network where a contact collects, so a contact
    /// whose relay is down does not start a lookup on every send tick.
    dht_addr_asked: Arc<Mutex<HashMap<i64, Instant>>>,
    /// Measured round trip per contact: how long the last few messages took
    /// from leaving here to their acknowledgement coming back. Kept in memory
    /// on purpose — it describes this link right now, and a number from last
    /// week would only mislead.
    rtt: Arc<Mutex<HashMap<i64, Rtt>>>,
}

fn build_ad(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    if a <= b {
        out.extend_from_slice(a);
        out.extend_from_slice(b);
    } else {
        out.extend_from_slice(b);
        out.extend_from_slice(a);
    }
    out
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn make_typing_payload(group: Option<WireGroupRef>, typing: bool) -> WirePayload {
    WirePayload {
        origin_msg_id: 0,
        body: String::new(),
        attachments: vec![],
        sent_at: now_ms(),
        ttl_ms: None,
        group,
        buttons: None,
        callback_data: None,
        edit_of: None,
        pin: None,
        ack_for: None,
        sender_name: None,
        reply_to: None,
        typing: Some(typing),
        notify_sound: None,
        console: None,
        relay_address: None,
        wipe: None,
        files: Vec::new(),
        file_chunk: None,
        file_ack: None,
        file_cancel: None,
        dm: None,
    }
}

/// The storage or the network failed, not the letter: worth trying again
/// later. Processing a stored letter again is idempotent (duplicates are
/// recognised by origin), so a reply that could not be sent is no reason to
/// drop what was received.
fn is_transient(error: &CoreError) -> bool {
    match error {
        CoreError::Net(_) | CoreError::Relay(_) => true,
        CoreError::Db(e) => gipny_libcore::db::is_transient(e),
        CoreError::Io(_) => true,
        _ => false,
    }
}

/// A stored or received 32-byte key. Any other length is an error, never a
/// panic: release builds abort on panic, and these bytes can come off the wire.
fn to_arr32(v: Vec<u8>) -> Result<[u8; 32]> {
    <[u8; 32]>::try_from(v).map_err(|_| CoreError::State)
}

fn to_hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{:02x}", x));
    }
    s
}

fn hex_short(b: &[u8]) -> String {
    let mut s = String::new();
    for x in &b[..8.min(b.len())] {
        s.push_str(&format!("{:02x}", x));
    }
    s
}
