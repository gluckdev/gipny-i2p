use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::crypto::{self, AttachmentCipher, Identity, PreKeyBundle, PreKeyPair, RatchetState, X3dhInitial, CryptoError};
use crate::db::{Contact, Db, DbError, Direction, NewAttachment, PreKeyKind, TrustLevel};
use crate::net::{NetError, TorNode};
use crate::relay::{self, ClientToRelay, EnvelopeBlob, RelayClient, RelayError, RelayToClient, DEFAULT_RELAY};

pub type Result<T> = std::result::Result<T, SessionError>;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("db")] Db(#[from] DbError),
    #[error("crypto")] Crypto(#[from] CryptoError),
    #[error("net")] Net(#[from] NetError),
    #[error("relay")] Relay(#[from] RelayError),
    #[error("io")] Io(#[from] std::io::Error),
    #[error("codec")] Codec,
    #[error("not found")] NotFound,
    #[error("state")] State,
    #[error("stale opk")] StaleOpk,
    #[error("sealed drop")] SealedDrop,
}

impl From<bincode::Error> for SessionError { fn from(_: bincode::Error) -> Self { Self::Codec } }

/// State of a connection to somebody else's relay. Mirrors Core::PeerRelay —
/// `Connecting` and `Failed` are what let the send loop check instead of dial.
enum PeerRelay {
    Ready(mpsc::Sender<ClientToRelay>),
    Connecting,
    Failed { until: std::time::Instant },
}

/// Give up on a peer relay's dial after this long: nothing below has a timeout,
/// and opening an i2p destination means building tunnels.
const PEER_RELAY_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);
/// Leave a failed peer relay alone this long instead of redialing every tick.
const PEER_RELAY_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_secs(120);

const SETTING_IDENTITY_SIGN: &str = "identity_sign";
const SETTING_IDENTITY_DH: &str = "identity_dh";
const SETTING_SIGNED_PREKEY_ID: &str = "signed_prekey_id";
const SETTING_RELAY_ONION: &str = "relay_onion";
const SETTING_DISPLAY_NAME: &str = "display_name";
const ATTACHMENTS_DIR: &str = "attachments";
const TARGET_OPK: usize = 20;
const RECONNECT_INITIAL_MS: u64 = 500;
const RECONNECT_MAX_MS: u64 = 15_000;
const PING_INTERVAL_SECS: u64 = 20;
const DEAD_THRESHOLD_SECS: u64 = 75;
const BUNDLE_REFRESH_SECS: u64 = 12 * 3600;
const PENDING_REQ_TIMEOUT_MS: u64 = 30_000;
const TIEBREAKER_TIMEOUT_MS: i64 = 10_000;
const FRESH_SESSION_GRACE_MS: i64 = 60_000;
const KEEPALIVE_INCOMING_THRESHOLD: u32 = 100;
const MAX_PAYLOAD_BYTES: usize = 14 * 1024 * 1024;
const RETRY_BASE_BACKOFF_MS: i64 = 5_000;
const RETRY_MAX_BACKOFF_MS: i64 = 300_000;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WirePayload {
    pub origin_msg_id: u64,
    pub body: String,
    pub attachments: Vec<WireAttachment>,
    pub sent_at: i64,
    pub ttl_ms: Option<i64>,
    pub group: Option<WireGroupRef>,
    #[serde(default)]
    pub buttons: Option<Vec<Vec<WireButton>>>,
    #[serde(default)]
    pub callback_data: Option<String>,
    #[serde(default)]
    pub edit_of: Option<u64>,
    #[serde(default)]
    pub pin: Option<WirePin>,
    #[serde(default)]
    pub ack_for: Option<u64>,
    #[serde(default)]
    pub sender_name: Option<String>,
    #[serde(default)]
    pub reply_to: Option<WireReply>,
    #[serde(default)]
    pub typing: Option<bool>,
    #[serde(default)]
    pub notify_sound: Option<String>,
    #[serde(default)]
    pub console: Option<WireConsole>,
    #[serde(default)]
    pub relay_address: Option<String>,
}

impl WirePayload {
    pub fn simple(origin: u64, body: String, attachments: Vec<WireAttachment>, sent_at: i64, ttl_ms: Option<i64>) -> Self {
        Self {
            origin_msg_id: origin, body, attachments, sent_at, ttl_ms,
            group: None, buttons: None, callback_data: None, edit_of: None, pin: None,
            ack_for: None, sender_name: None, reply_to: None,
            typing: None, notify_sound: None, console: None, relay_address: None,
        }
    }
}

/// Agent-mode "console" marker on a message. Carried as an optional trailing
/// field so a client without this field simply sees the message's text body
/// (a short ASCII marker for control kinds, the command or its output otherwise)
/// and ignores the console framing. `kind` is a `u8`, not an enum, so an
/// unknown future kind decodes cleanly instead of failing the whole payload.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct WireConsole {
    /// See the `CONSOLE_*` constants.
    pub kind: u8,
    /// Process exit code for `CONSOLE_OUTPUT`; `None` if killed by signal/timeout.
    pub exit_code: Option<i32>,
    /// Wall-clock duration of the command for `CONSOLE_OUTPUT`.
    pub duration_ms: Option<u64>,
    /// Output was capped and trimmed.
    pub truncated: bool,
}

/// A command the master wants run on the agent.
pub const CONSOLE_COMMAND: u8 = 0;
/// The result of running a command, sent back to the master.
pub const CONSOLE_OUTPUT: u8 = 1;
/// The agent telling the master "you may drive my console" (agent mode on).
pub const CONSOLE_GRANT: u8 = 2;
/// The agent telling the master the console is closed (agent mode off).
pub const CONSOLE_REVOKE: u8 = 3;
/// The master telling the agent to switch agent mode off.
pub const CONSOLE_OFF: u8 = 4;

impl WireConsole {
    pub fn new(kind: u8) -> Self {
        Self { kind, exit_code: None, duration_ms: None, truncated: false }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WireAttachment { pub name: String, pub data: Vec<u8> }

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WireButton { pub text: String, pub callback_data: String }

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WireGroupRef { pub id: Vec<u8>, pub name: String, pub members: Vec<WireMember> }

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WireMember { pub sign_pk: Vec<u8>, pub dh_pk: Vec<u8>, pub onion: String, pub name: String }

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WirePin {
    pub sender_sign_pk: Vec<u8>,
    pub origin_msg_id: u64,
    pub unpin: bool,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WireReply {
    pub sender_sign_pk: Vec<u8>,
    pub origin_msg_id: u64,
}

#[derive(Serialize, Deserialize)]
struct WireV0 {
    origin_msg_id: u64,
    body: String,
    attachments: Vec<WireAttachment>,
    sent_at: i64,
    ttl_ms: Option<i64>,
    group: Option<WireGroupRef>,
}

#[derive(Serialize, Deserialize)]
struct WireV1 {
    origin_msg_id: u64,
    body: String,
    attachments: Vec<WireAttachment>,
    sent_at: i64,
    ttl_ms: Option<i64>,
    group: Option<WireGroupRef>,
    buttons: Option<Vec<Vec<WireButton>>>,
    callback_data: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct WireV2 {
    origin_msg_id: u64,
    body: String,
    attachments: Vec<WireAttachment>,
    sent_at: i64,
    ttl_ms: Option<i64>,
    group: Option<WireGroupRef>,
    buttons: Option<Vec<Vec<WireButton>>>,
    callback_data: Option<String>,
    edit_of: Option<u64>,
    pin: Option<WirePin>,
}

#[derive(Serialize, Deserialize)]
struct WireV3 {
    origin_msg_id: u64,
    body: String,
    attachments: Vec<WireAttachment>,
    sent_at: i64,
    ttl_ms: Option<i64>,
    group: Option<WireGroupRef>,
    buttons: Option<Vec<Vec<WireButton>>>,
    callback_data: Option<String>,
    edit_of: Option<u64>,
    pin: Option<WirePin>,
    ack_for: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct WireV4 {
    origin_msg_id: u64,
    body: String,
    attachments: Vec<WireAttachment>,
    sent_at: i64,
    ttl_ms: Option<i64>,
    group: Option<WireGroupRef>,
    buttons: Option<Vec<Vec<WireButton>>>,
    callback_data: Option<String>,
    edit_of: Option<u64>,
    pin: Option<WirePin>,
    ack_for: Option<u64>,
    sender_name: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct WireV5 {
    origin_msg_id: u64,
    body: String,
    attachments: Vec<WireAttachment>,
    sent_at: i64,
    ttl_ms: Option<i64>,
    group: Option<WireGroupRef>,
    buttons: Option<Vec<Vec<WireButton>>>,
    callback_data: Option<String>,
    edit_of: Option<u64>,
    pin: Option<WirePin>,
    ack_for: Option<u64>,
    sender_name: Option<String>,
    reply_to: Option<WireReply>,
}

#[derive(Serialize, Deserialize)]
struct WireV7 {
    origin_msg_id: u64,
    body: String,
    attachments: Vec<WireAttachment>,
    sent_at: i64,
    ttl_ms: Option<i64>,
    group: Option<WireGroupRef>,
    buttons: Option<Vec<Vec<WireButton>>>,
    callback_data: Option<String>,
    edit_of: Option<u64>,
    pin: Option<WirePin>,
    ack_for: Option<u64>,
    sender_name: Option<String>,
    reply_to: Option<WireReply>,
    typing: Option<bool>,
    notify_sound: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct WireV6 {
    origin_msg_id: u64,
    body: String,
    attachments: Vec<WireAttachment>,
    sent_at: i64,
    ttl_ms: Option<i64>,
    group: Option<WireGroupRef>,
    buttons: Option<Vec<Vec<WireButton>>>,
    callback_data: Option<String>,
    edit_of: Option<u64>,
    pin: Option<WirePin>,
    ack_for: Option<u64>,
    sender_name: Option<String>,
    reply_to: Option<WireReply>,
    typing: Option<bool>,
}

impl From<&WirePayload> for WireV6 {
    fn from(p: &WirePayload) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id, body: p.body.clone(), attachments: p.attachments.clone(),
            sent_at: p.sent_at, ttl_ms: p.ttl_ms, group: p.group.clone(),
            buttons: p.buttons.clone(), callback_data: p.callback_data.clone(),
            edit_of: p.edit_of, pin: p.pin.clone(), ack_for: p.ack_for,
            sender_name: p.sender_name.clone(), reply_to: p.reply_to.clone(),
            typing: p.typing,
        }
    }
}

impl From<&WirePayload> for WireV7 {
    fn from(p: &WirePayload) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id, body: p.body.clone(), attachments: p.attachments.clone(),
            sent_at: p.sent_at, ttl_ms: p.ttl_ms, group: p.group.clone(),
            buttons: p.buttons.clone(), callback_data: p.callback_data.clone(),
            edit_of: p.edit_of, pin: p.pin.clone(), ack_for: p.ack_for,
            sender_name: p.sender_name.clone(), reply_to: p.reply_to.clone(),
            typing: p.typing, notify_sound: p.notify_sound.clone(),
        }
    }
}

impl From<WireV7> for WirePayload {
    fn from(v: WireV7) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: v.buttons, callback_data: v.callback_data,
            edit_of: v.edit_of, pin: v.pin, ack_for: v.ack_for, sender_name: v.sender_name,
            reply_to: v.reply_to, typing: v.typing, notify_sound: v.notify_sound, console: None,
            relay_address: None,
        }
    }
}

impl From<WireV6> for WirePayload {
    fn from(v: WireV6) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: v.buttons, callback_data: v.callback_data,
            edit_of: v.edit_of, pin: v.pin, ack_for: v.ack_for, sender_name: v.sender_name,
            reply_to: v.reply_to, typing: v.typing, notify_sound: None, console: None,
            relay_address: None,
        }
    }
}

impl From<&WirePayload> for WireV5 {
    fn from(p: &WirePayload) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id, body: p.body.clone(), attachments: p.attachments.clone(),
            sent_at: p.sent_at, ttl_ms: p.ttl_ms, group: p.group.clone(),
            buttons: p.buttons.clone(), callback_data: p.callback_data.clone(),
            edit_of: p.edit_of, pin: p.pin.clone(), ack_for: p.ack_for,
            sender_name: p.sender_name.clone(), reply_to: p.reply_to.clone(),
        }
    }
}

impl From<&WirePayload> for WireV4 {
    fn from(p: &WirePayload) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id, body: p.body.clone(), attachments: p.attachments.clone(),
            sent_at: p.sent_at, ttl_ms: p.ttl_ms, group: p.group.clone(),
            buttons: p.buttons.clone(), callback_data: p.callback_data.clone(),
            edit_of: p.edit_of, pin: p.pin.clone(), ack_for: p.ack_for,
            sender_name: p.sender_name.clone(),
        }
    }
}

impl From<&WirePayload> for WireV3 {
    fn from(p: &WirePayload) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id, body: p.body.clone(), attachments: p.attachments.clone(),
            sent_at: p.sent_at, ttl_ms: p.ttl_ms, group: p.group.clone(),
            buttons: p.buttons.clone(), callback_data: p.callback_data.clone(),
            edit_of: p.edit_of, pin: p.pin.clone(), ack_for: p.ack_for,
        }
    }
}

impl From<&WirePayload> for WireV2 {
    fn from(p: &WirePayload) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id, body: p.body.clone(), attachments: p.attachments.clone(),
            sent_at: p.sent_at, ttl_ms: p.ttl_ms, group: p.group.clone(),
            buttons: p.buttons.clone(), callback_data: p.callback_data.clone(),
            edit_of: p.edit_of, pin: p.pin.clone(),
        }
    }
}

impl From<&WirePayload> for WireV1 {
    fn from(p: &WirePayload) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id, body: p.body.clone(), attachments: p.attachments.clone(),
            sent_at: p.sent_at, ttl_ms: p.ttl_ms, group: p.group.clone(),
            buttons: p.buttons.clone(), callback_data: p.callback_data.clone(),
        }
    }
}

impl From<WireV5> for WirePayload {
    fn from(v: WireV5) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: v.buttons, callback_data: v.callback_data,
            edit_of: v.edit_of, pin: v.pin, ack_for: v.ack_for, sender_name: v.sender_name,
            reply_to: v.reply_to, typing: None, notify_sound: None, console: None, relay_address: None,
        }
    }
}

impl From<WireV4> for WirePayload {
    fn from(v: WireV4) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: v.buttons, callback_data: v.callback_data,
            edit_of: v.edit_of, pin: v.pin, ack_for: v.ack_for, sender_name: v.sender_name,
            reply_to: None, typing: None, notify_sound: None, console: None, relay_address: None,
        }
    }
}

impl From<WireV3> for WirePayload {
    fn from(v: WireV3) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: v.buttons, callback_data: v.callback_data,
            edit_of: v.edit_of, pin: v.pin, ack_for: v.ack_for,
            sender_name: None, reply_to: None, typing: None, notify_sound: None, console: None, relay_address: None,
        }
    }
}

impl From<WireV2> for WirePayload {
    fn from(v: WireV2) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: v.buttons, callback_data: v.callback_data,
            edit_of: v.edit_of, pin: v.pin,
            ack_for: None, sender_name: None, reply_to: None, typing: None, notify_sound: None, console: None, relay_address: None,
        }
    }
}

impl From<WireV1> for WirePayload {
    fn from(v: WireV1) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: v.buttons, callback_data: v.callback_data,
            edit_of: None, pin: None, ack_for: None, sender_name: None, reply_to: None, typing: None, notify_sound: None, console: None, relay_address: None,
        }
    }
}

impl From<WireV0> for WirePayload {
    fn from(v: WireV0) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: None, callback_data: None,
            edit_of: None, pin: None, ack_for: None, sender_name: None, reply_to: None, typing: None, notify_sound: None, console: None,
            relay_address: None,
        }
    }
}

pub fn encode_payload(p: &WirePayload) -> std::result::Result<Vec<u8>, bincode::Error> {
    if p.console.is_some() || p.relay_address.is_some() { bincode::serialize(p) }
    else if p.notify_sound.is_some()                    { bincode::serialize(&WireV7::from(p)) }
    else if p.typing.is_some()                     { bincode::serialize(&WireV6::from(p)) }
    else if p.reply_to.is_some()                   { bincode::serialize(&WireV5::from(p)) }
    else if p.sender_name.is_some()                { bincode::serialize(&WireV4::from(p)) }
    else if p.ack_for.is_some()                    { bincode::serialize(&WireV3::from(p)) }
    else if p.edit_of.is_some() || p.pin.is_some() { bincode::serialize(&WireV2::from(p)) }
    else                                           { bincode::serialize(&WireV1::from(p)) }
}

pub fn decode_payload(pt: &[u8]) -> std::result::Result<WirePayload, bincode::Error> {
    if let Ok(v) = bincode::deserialize::<WirePayload>(pt) { return Ok(v); }
    if let Ok(v) = bincode::deserialize::<WireV7>(pt)      { return Ok(v.into()); }
    if let Ok(v) = bincode::deserialize::<WireV6>(pt)      { return Ok(v.into()); }
    if let Ok(v) = bincode::deserialize::<WireV5>(pt)      { return Ok(v.into()); }
    if let Ok(v) = bincode::deserialize::<WireV4>(pt)      { return Ok(v.into()); }
    if let Ok(v) = bincode::deserialize::<WireV3>(pt)      { return Ok(v.into()); }
    if let Ok(v) = bincode::deserialize::<WireV2>(pt)      { return Ok(v.into()); }
    if let Ok(v) = bincode::deserialize::<WireV1>(pt)      { return Ok(v.into()); }
    Ok(bincode::deserialize::<WireV0>(pt)?.into())
}

const PADDING_BUCKETS: &[usize] = &[
    256, 1024, 4096, 16_384, 65_536, 262_144, 1_048_576, 4_194_304, 16_777_216,
];

pub fn pad_payload(pt: &[u8]) -> Vec<u8> {
    let with_header = 4 + pt.len();
    let bucket = PADDING_BUCKETS.iter().copied().find(|&b| b >= with_header).unwrap_or(with_header);
    let mut out = Vec::with_capacity(bucket);
    out.extend_from_slice(&(pt.len() as u32).to_be_bytes());
    out.extend_from_slice(pt);
    if bucket > with_header {
        let mut pad = vec![0u8; bucket - with_header];
        crate::crypto::fill_random(&mut pad);
        out.extend_from_slice(&pad);
    }
    out
}

pub fn unpad_payload(padded: &[u8]) -> Option<Vec<u8>> {
    if padded.len() < 4 { return None; }
    let len = u32::from_be_bytes(padded[0..4].try_into().ok()?) as usize;
    if 4 + len > padded.len() { return None; }
    Some(padded[4..4 + len].to_vec())
}

fn decode_with_padding_fallback(pt: &[u8]) -> Result<WirePayload> {
    if let Some(unpadded) = unpad_payload(pt) {
        if let Ok(p) = decode_payload(&unpadded) { return Ok(p); }
    }
    Ok(decode_payload(pt)?)
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
    Connected,
    Disconnected,
    IncomingPayload { contact_id: i64, payload: WirePayload, message_id: i64 },
    MessageDelivered { message_id: i64 },
    MessageEdited { message_id: i64, new_body: String, buttons: Option<Vec<Vec<WireButton>>> },
    MessagePinned { contact_id: Option<i64>, group_id: Option<Vec<u8>>, message_id: i64 },
    MessageUnpinned { contact_id: Option<i64>, group_id: Option<Vec<u8>>, message_id: i64 },
    ContactAdded { contact_id: i64 },
    ContactUpdated { contact_id: i64 },
    /// A relay answered with an error frame: a deposit or a publish it would
    /// not take. `reason` is the relay's text, e.g. [`crate::relay::ERR_NOT_SERVED`].
    RelayError { reason: String },
}

pub struct SessionManager {
    pub db: Arc<Db>,
    pub node: Arc<TorNode>,
    pub identity: Arc<Identity>,
    data_dir: PathBuf,
    sessions: Arc<Mutex<HashMap<i64, RatchetState>>>,
    /// Connection to our own relay — where we receive and publish our bundle.
    relay_out: Arc<RwLock<Option<mpsc::Sender<ClientToRelay>>>>,
    /// Connections to other people's relays, keyed by destination.
    peer_relays: Arc<Mutex<HashMap<String, PeerRelay>>>,
    bundle_waiters: Arc<Mutex<HashMap<[u8; 32], Vec<tokio::sync::oneshot::Sender<Option<Vec<u8>>>>>>>,
    tiebreaker_waits: Arc<Mutex<HashMap<i64, i64>>>,
    session_created_at: Arc<Mutex<HashMap<i64, i64>>>,
    incoming_since_send: Arc<Mutex<HashMap<i64, u32>>>,
    send_kick: Arc<tokio::sync::Notify>,
    events: mpsc::Sender<SessionEvent>,
    tasks: Arc<std::sync::Mutex<Vec<JoinHandle<()>>>>,
}

impl SessionManager {
    pub async fn start(
        data_dir: PathBuf,
        db: Arc<Db>,
        node: Arc<TorNode>,
    ) -> Result<(Arc<Self>, mpsc::Receiver<SessionEvent>)> {
        std::fs::create_dir_all(data_dir.join(ATTACHMENTS_DIR))?;
        let identity = Arc::new(load_or_create_identity(&db)?);
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
            tiebreaker_waits: Arc::new(Mutex::new(HashMap::new())),
            session_created_at: Arc::new(Mutex::new(HashMap::new())),
            incoming_since_send: Arc::new(Mutex::new(HashMap::new())),
            send_kick: Arc::new(tokio::sync::Notify::new()),
            events: events_tx,
            tasks: Arc::new(std::sync::Mutex::new(Vec::new())),
        });
        this.ensure_prekeys().await?;
        this.clone().spawn_relay_loop();
        this.clone().spawn_send_loop();
        this.clone().spawn_bundle_refresh_loop();
        Ok((this, events_rx))
    }

    pub fn shutdown(&self) {
        let mut v = self.tasks.lock().unwrap();
        for h in v.drain(..) { h.abort(); }
    }

    pub fn my_card(&self) -> crate::crypto::IdentityCard { self.identity.card() }
    pub fn my_fingerprint(&self) -> [u8; 32] { self.identity.fingerprint() }

    pub fn display_name(&self) -> Result<String> {
        Ok(self.db.get_setting(SETTING_DISPLAY_NAME)?
            .and_then(|b| String::from_utf8(b).ok()).unwrap_or_default())
    }

    pub fn set_display_name(&self, name: &str) -> Result<()> {
        self.db.set_setting(SETTING_DISPLAY_NAME, name.as_bytes())?;
        Ok(())
    }

    fn outgoing_sender_name(&self) -> Option<String> {
        let user = self.db.get_setting(SETTING_DISPLAY_NAME).ok().flatten()
            .and_then(|b| String::from_utf8(b).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Some(user.unwrap_or_else(|| {
            let pk = self.identity.card().sign_pk;
            let mut s = String::with_capacity(16);
            for x in &pk[..8] { s.push_str(&format!("{:02x}", x)); }
            s
        }))
    }

    fn relay_onion(&self) -> String {
        self.db.get_setting(SETTING_RELAY_ONION).ok().flatten()
            .and_then(|v| String::from_utf8(v).ok())
            .unwrap_or_else(|| DEFAULT_RELAY.to_string())
    }

    pub fn set_relay_onion(&self, onion: &str) -> Result<()> {
        self.db.set_setting(SETTING_RELAY_ONION, onion.as_bytes())?;
        Ok(())
    }

    pub async fn add_contact(&self, card: &crate::crypto::IdentityCard, onion: &str, name: &str) -> Result<i64> {
        self.add_contact_via(card, onion, name, None).await
    }

    /// Add a contact, recording the relay their card named.
    ///
    /// `relay` is where *they* receive: messages to this contact are deposited
    /// there, not on whatever relay this client happens to use. `None` keeps the
    /// old behaviour of falling back to this client's configured relay.
    pub async fn add_contact_via(
        &self, card: &crate::crypto::IdentityCard, onion: &str, name: &str, relay: Option<&str>,
    ) -> Result<i64> {
        let id = self.db.add_contact(&card.sign_pk, &card.dh_pk, onion, name, relay)?;
        let _ = self.events.send(SessionEvent::ContactAdded { contact_id: id }).await;
        self.send_kick.notify_one();
        Ok(id)
    }

    pub async fn send_message(
        &self,
        contact_id: i64,
        body: String,
        attachments: Vec<(String, Vec<u8>)>,
        ttl: Option<Duration>,
        buttons: Option<Vec<Vec<WireButton>>>,
        notify_sound: Option<String>,
    ) -> Result<i64> {
        let sent_at = now_ms();
        let expires_at = ttl.map(|d| sent_at + d.as_millis() as i64);
        let mut stored = Vec::with_capacity(attachments.len());
        for (name, data) in &attachments {
            let (key, path, size) = store_attachment(&self.data_dir, data)?;
            stored.push(NewAttachment {
                name: name.clone(), size: size as i64, key: key.to_vec(), path,
            });
        }
        let msg_id = self.db.insert_message(
            contact_id, Direction::Out, &body, sent_at, expires_at, &stored,
        )?;
        if let Some(b) = &buttons {
            if let Ok(bytes) = bincode::serialize(b) {
                self.db.set_setting(&format!("buttons_{}", msg_id), &bytes)?;
            }
        }
        if let Some(s) = &notify_sound {
            self.db.set_setting(&format!("sound_{}", msg_id), s.as_bytes())?;
        }
        self.send_kick.notify_one();
        Ok(msg_id)
    }

    /// Sends a console-framed message: a command, its output, or a control
    /// marker. The frame rides in `console_<id>` next to the row, as buttons
    /// and sounds do, and goes onto the wire when the send loop builds the
    /// payload — so it is queued, retried and acked like any other message.
    pub async fn send_console(
        &self,
        contact_id: i64,
        body: String,
        console: WireConsole,
        attachments: Vec<(String, Vec<u8>)>,
    ) -> Result<i64> {
        let sent_at = now_ms();
        let mut stored = Vec::with_capacity(attachments.len());
        for (name, data) in &attachments {
            let (key, path, size) = store_attachment(&self.data_dir, data)?;
            stored.push(NewAttachment {
                name: name.clone(), size: size as i64, key: key.to_vec(), path,
            });
        }
        let msg_id = self.db.insert_message(
            contact_id, Direction::Out, &body, sent_at, None, &stored,
        )?;
        self.db.set_setting(&format!("console_{}", msg_id), &bincode::serialize(&console)?)?;
        self.send_kick.notify_one();
        Ok(msg_id)
    }

    pub async fn send_callback(self: &Arc<Self>, contact_id: i64, data: String) -> Result<()> {
        let mut payload = WirePayload {
            origin_msg_id: 0,
            body: String::new(),
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: None,
            buttons: None,
            callback_data: Some(data),
            edit_of: None,
            pin: None,
            ack_for: None,
            sender_name: None,
            reply_to: None,
            typing: None,
            notify_sound: None,
            console: None,
            relay_address: None,
        };
        self.send_to_contact(contact_id, &mut payload).await
    }

    pub async fn send_edit(
        self: &Arc<Self>,
        contact_id: i64,
        edit_target_origin: u64,
        new_body: String,
        buttons: Option<Vec<Vec<WireButton>>>,
    ) -> Result<()> {
        let mut payload = WirePayload {
            origin_msg_id: 0,
            body: new_body,
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: None,
            buttons,
            callback_data: None,
            edit_of: Some(edit_target_origin),
            pin: None,
            ack_for: None,
            sender_name: None,
            reply_to: None,
            typing: None,
            notify_sound: None,
            console: None,
            relay_address: None,
        };
        self.send_to_contact(contact_id, &mut payload).await
    }

    pub async fn send_to_group(
        self: &Arc<Self>,
        group_id: &[u8],
        body: String,
        attachments: Vec<(String, Vec<u8>)>,
        buttons: Option<Vec<Vec<WireButton>>>,
        notify_sound: Option<String>,
    ) -> Result<i64> {
        let sent_at = now_ms();
        let mut stored = Vec::with_capacity(attachments.len());
        for (name, data) in &attachments {
            let (key, path, size) = store_attachment(&self.data_dir, data)?;
            stored.push(NewAttachment {
                name: name.clone(), size: size as i64, key: key.to_vec(), path,
            });
        }
        let msg_id = self.db.insert_group_message_with_origin(
            group_id, Some(&self.identity.card().sign_pk), Direction::Out,
            &body, sent_at, None, &stored, None,
        )?;
        if let Some(b) = &buttons {
            if let Ok(bytes) = bincode::serialize(b) {
                self.db.set_setting(&format!("buttons_{}", msg_id), &bytes)?;
            }
        }
        if let Some(s) = &notify_sound {
            self.db.set_setting(&format!("sound_{}", msg_id), s.as_bytes())?;
        }
        let wire_atts: Vec<WireAttachment> = attachments.into_iter()
            .map(|(name, data)| WireAttachment { name, data }).collect();
        let members = self.db.list_group_members(group_id)?;
        let gref_members: Vec<WireMember> = members.iter().map(|m| WireMember {
            sign_pk: m.sign_pk.clone(), dh_pk: m.dh_pk.clone(),
            onion: m.onion.clone(), name: m.display_name.clone(),
        }).collect();
        let gname = self.db.get_group_name(group_id)?.unwrap_or_default();
        let gref = WireGroupRef { id: group_id.to_vec(), name: gname, members: gref_members };
        for m in members {
            if m.is_self { continue; }
            let contact = match self.db.find_contact_by_identity(&m.dh_pk)? {
                Some(c) => c,
                None => continue,
            };
            if contact.trust == TrustLevel::Blocked { continue; }
            let mut payload = WirePayload::simple(
                msg_id as u64, body.clone(), wire_atts.clone(), sent_at, None,
            );
            payload.group = Some(gref.clone());
            payload.buttons = buttons.clone();
            payload.notify_sound = notify_sound.clone();
            let _ = self.send_to_contact(contact.id, &mut payload).await;
        }
        Ok(msg_id)
    }

    pub async fn send_edit_group(
        self: &Arc<Self>,
        group_id: &[u8],
        edit_target_origin: u64,
        new_body: String,
        buttons: Option<Vec<Vec<WireButton>>>,
    ) -> Result<()> {
        let local_id = edit_target_origin as i64;
        let _ = self.db.update_message_body(local_id, &new_body);
        if let Some(b) = &buttons {
            if let Ok(bytes) = bincode::serialize(b) {
                let _ = self.db.set_setting(&format!("buttons_{}", local_id), &bytes);
            }
        } else {
            let _ = self.db.delete_setting(&format!("buttons_{}", local_id));
        }
        let members = self.db.list_group_members(group_id)?;
        let gref_members: Vec<WireMember> = members.iter().map(|m| WireMember {
            sign_pk: m.sign_pk.clone(), dh_pk: m.dh_pk.clone(),
            onion: m.onion.clone(), name: m.display_name.clone(),
        }).collect();
        let gname = self.db.get_group_name(group_id)?.unwrap_or_default();
        let gref = WireGroupRef { id: group_id.to_vec(), name: gname, members: gref_members };
        for m in members {
            if m.is_self { continue; }
            let contact = match self.db.find_contact_by_identity(&m.dh_pk)? {
                Some(c) => c,
                None => continue,
            };
            if contact.trust == TrustLevel::Blocked { continue; }
            let mut payload = WirePayload {
                origin_msg_id: 0,
                body: new_body.clone(),
                attachments: vec![],
                sent_at: now_ms(),
                ttl_ms: None,
                group: Some(gref.clone()),
                buttons: buttons.clone(),
                callback_data: None,
                edit_of: Some(edit_target_origin),
                pin: None,
                ack_for: None,
                sender_name: None,
                reply_to: None,
            typing: None,
            notify_sound: None,
            console: None,
            relay_address: None,
            };
            let _ = self.send_to_contact(contact.id, &mut payload).await;
        }
        Ok(())
    }

    async fn send_to_contact(self: &Arc<Self>, contact_id: i64, payload: &mut WirePayload) -> Result<()> {
        let contact = self.db.get_contact(contact_id)?.ok_or(SessionError::NotFound)?;
        let out = self.relay_for(&contact).await.ok_or(SessionError::State)?;
        self.ensure_session_for(&contact, &out).await?;
        self.send_payload_via_relay(&contact, payload, &out).await
    }

    pub async fn send_pin_contact(
        self: &Arc<Self>,
        contact_id: i64,
        sender_sign_pk: Vec<u8>,
        origin_msg_id: u64,
        unpin: bool,
    ) -> Result<()> {
        let mut payload = WirePayload {
            origin_msg_id: 0,
            body: String::new(),
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: None,
            buttons: None,
            callback_data: None,
            edit_of: None,
            pin: Some(WirePin { sender_sign_pk, origin_msg_id, unpin }),
            ack_for: None,
            sender_name: None,
            reply_to: None,
            typing: None,
            notify_sound: None,
            console: None,
            relay_address: None,
        };
        self.send_to_contact(contact_id, &mut payload).await
    }

    async fn ensure_prekeys(&self) -> Result<()> {
        let id = self.db.get_setting(SETTING_SIGNED_PREKEY_ID)?;
        if id.is_none() {
            let pair = PreKeyPair::generate();
            let pid = self.db.add_prekey(PreKeyKind::Signed, pair.secret(), pair.public())?;
            self.db.set_setting(SETTING_SIGNED_PREKEY_ID, &pid.to_be_bytes())?;
        }
        let count = self.db.count_prekeys(PreKeyKind::OneTime)?;
        for _ in count..(TARGET_OPK as i64) {
            let pair = PreKeyPair::generate();
            self.db.add_prekey(PreKeyKind::OneTime, pair.secret(), pair.public())?;
        }
        Ok(())
    }

    pub fn my_bundle(&self) -> Result<PreKeyBundle> {
        let signed_id = i64::from_be_bytes(
            self.db.get_setting(SETTING_SIGNED_PREKEY_ID)?.ok_or(SessionError::State)?
                .try_into().map_err(|_| SessionError::State)?);
        let signed = self.db.get_prekey(signed_id)?.ok_or(SessionError::State)?;
        let signed_pair = PreKeyPair::from_secret(to_arr32(signed.private.clone())?);
        let opk = self.db.peek_oldest_prekey(PreKeyKind::OneTime)?;
        let opk_pair = opk.as_ref().map(|p| {
            let sk = to_arr32(p.private.clone()).expect("prekey size");
            (p.id, PreKeyPair::from_secret(sk))
        });
        Ok(PreKeyBundle::new(&self.identity, &signed_pair,
            opk_pair.as_ref().map(|(id, kp)| (*id, kp))))
    }

    fn spawn_relay_loop(self: Arc<Self>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            let mut backoff = RECONNECT_INITIAL_MS;
            loop {
                let onion = this.relay_onion();
                if onion.is_empty() {
                    // No relay configured yet (i2p: DEFAULT_RELAY not baked in and
                    // none set in Settings). Wait quietly instead of hammering.
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                    backoff = (backoff * 2).min(RECONNECT_MAX_MS);
                    continue;
                }
                eprintln!("[session] relay connect {}", &onion[..16.min(onion.len())]);
                match relay::connect(&this.node, &onion, &this.identity).await {
                    Ok(client) => {
                        eprintln!("[session] relay connected & authed");
                        backoff = RECONNECT_INITIAL_MS;
                        *this.relay_out.write().await = Some(client.out_tx.clone());
                        let _ = this.events.send(SessionEvent::Connected).await;
                        if let Ok(bundle) = this.my_bundle() {
                            if let Ok(bytes) = bincode::serialize(&bundle) {
                                let _ = client.out_tx.send(ClientToRelay::Publish { bundle: bytes }).await;
                            }
                        }
                        this.send_kick.notify_one();
                        this.clone().run_recv_loop(client, Some(onion.clone())).await;
                        *this.relay_out.write().await = None;
                        let _ = this.events.send(SessionEvent::Disconnected).await;
                    }
                    Err(e) => eprintln!("[session] connect fail: {:?}", e),
                }
                tokio::time::sleep(Duration::from_millis(backoff)).await;
                backoff = (backoff * 2).min(RECONNECT_MAX_MS);
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    /// The connection to deposit this contact's mail on: the relay their card
    /// named, or ours when they named none. Mirrors Core::relay_for — see there
    /// for why the future is boxed.
    fn relay_for<'a>(
        self: &'a Arc<Self>,
        contact: &'a Contact,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<mpsc::Sender<ClientToRelay>>> + Send + 'a>> {
        Box::pin(async move {
            let theirs = contact.relay_address.as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty());
            let Some(theirs) = theirs else {
                return self.relay_out.read().await.clone();
            };
            if theirs == self.relay_onion() {
                return self.relay_out.read().await.clone();
            }
            // Never dial on this path — see Core::relay_for. It runs inside the
            // send loop, and an unreachable relay has no timeout of its own, so
            // blocking here would stall delivery to every other contact.
            {
                let mut pool = self.peer_relays.lock().await;
                match pool.get(theirs) {
                    Some(PeerRelay::Ready(tx)) if !tx.is_closed() => return Some(tx.clone()),
                    Some(PeerRelay::Ready(_)) | Some(PeerRelay::Connecting) => return None,
                    Some(PeerRelay::Failed { until }) if std::time::Instant::now() < *until => return None,
                    _ => {}
                }
                pool.insert(theirs.to_string(), PeerRelay::Connecting);
            }

            let this = self.clone();
            let key = theirs.to_string();
            let handle = tokio::spawn(async move {
                let short = &key[..16.min(key.len())];
                let dial = tokio::time::timeout(
                    PEER_RELAY_CONNECT_TIMEOUT,
                    crate::relay::connect_peer(&this.node, &key, &this.identity),
                ).await;
                let client = match dial {
                    Ok(Ok(c)) => c,
                    other => {
                        match other {
                            Err(_) => eprintln!("[session] peer relay {short} did not answer in {:?}",
                                PEER_RELAY_CONNECT_TIMEOUT),
                            Ok(Err(e)) => eprintln!("[session] peer relay {short} unreachable: {e:?}"),
                            Ok(Ok(_)) => unreachable!(),
                        }
                        this.peer_relays.lock().await.insert(
                            key.clone(),
                            PeerRelay::Failed {
                                until: std::time::Instant::now() + PEER_RELAY_RETRY_BACKOFF,
                            },
                        );
                        return;
                    }
                };
                eprintln!("[session] connected to peer relay {short}");
                this.peer_relays.lock().await
                    .insert(key.clone(), PeerRelay::Ready(client.out_tx.clone()));
                this.send_kick.notify_one();
                this.clone().run_recv_loop(client, None).await;
                this.peer_relays.lock().await.remove(&key);
            });
            self.tasks.lock().unwrap().push(handle);
            None
        })
    }

    /// `collecting_from` is set on the connection to our own relay: once the
    /// relay we collect from changes, this one is no longer where mail arrives.
    async fn run_recv_loop(self: Arc<Self>, client: RelayClient, collecting_from: Option<String>) {
        let in_rx = client.in_rx.clone();
        let out_tx = client.out_tx.clone();
        let mut ping = tokio::time::interval(Duration::from_secs(PING_INTERVAL_SECS));
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ping.tick().await;
        let dead_threshold = Duration::from_secs(DEAD_THRESHOLD_SECS);
        let mut last_activity = std::time::Instant::now();
        loop {
            tokio::select! {
                _ = ping.tick() => {
                    if collecting_from.as_deref().is_some_and(|o| o != self.relay_onion()) {
                        eprintln!("[session] our relay changed, reconnecting to the new one");
                        break;
                    }
                    if last_activity.elapsed() > dead_threshold {
                        eprintln!("[session] no relay activity for {:?}, forcing reconnect", last_activity.elapsed());
                        break;
                    }
                    if out_tx.send(ClientToRelay::Ping).await.is_err() { break; }
                }
                frame = async { in_rx.lock().await.recv().await } => {
                    let Some(frame) = frame else { break; };
                    last_activity = std::time::Instant::now();
                    // Logged in with plain Auth after an AuthV2 attempt fell
                    // through: this relay would take our deposits and never
                    // hand us our mail. Reconnect, which tries AuthV2 again.
                    if collecting_from.is_some() && matches!(&frame, RelayToClient::Error(e) if e == relay::ERR_NEEDS_AUTH_V2) {
                        eprintln!("[session] our relay wants AuthV2, reconnecting");
                        break;
                    }
                    if let Err(e) = self.handle_relay_frame(frame, &out_tx).await {
                        eprintln!("[session] handle err: {:?}", e);
                    }
                }
            }
        }
    }

    async fn handle_relay_frame(
        self: &Arc<Self>,
        frame: RelayToClient,
        out_tx: &mpsc::Sender<ClientToRelay>,
    ) -> Result<()> {
        match frame {
            RelayToClient::Incoming { id, from, blob } => {
                match self.handle_incoming_envelope(&from, &blob).await {
                    Ok(()) => { let _ = out_tx.send(ClientToRelay::Ack { id }).await; }
                    Err(SessionError::Codec) => {
                        eprintln!("[session] codec err on msg {}, NOT acking", id);
                    }
                    Err(SessionError::StaleOpk) => {
                        eprintln!("[session] stale-OPK X3dhInit on msg {}, ACK and skip (zombie)", id);
                        let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                    }
                    Err(SessionError::SealedDrop) => {
                        let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                    }
                    Err(SessionError::Crypto(_)) => {
                        if let Ok(Some(c)) = self.db.find_contact_by_sign_pk(&from) {
                            let fresh = {
                                let m = self.session_created_at.lock().await;
                                m.get(&c.id).map(|t| now_ms() - *t < FRESH_SESSION_GRACE_MS).unwrap_or(false)
                            };
                            if fresh {
                                eprintln!("[session] crypto err on msg {} within fresh-session grace, ACK and skip", id);
                                let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                            } else {
                                eprintln!("[session] crypto err on msg {}, requesting resync, NOT acking", id);
                                let _ = self.request_resync(&c).await;
                            }
                        } else {
                            eprintln!("[session] crypto err on msg {}, no contact, NOT acking", id);
                        }
                    }
                    Err(e) => {
                        eprintln!("[session] incoming err: {:?}", e);
                        let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                    }
                }
            }
            RelayToClient::Bundle { pk, bundle } => {
                let mut w = self.bundle_waiters.lock().await;
                if let Some(vec) = w.remove(&pk) {
                    for tx in vec { let _ = tx.send(bundle.clone()); }
                }
            }
            RelayToClient::Error(reason) => {
                // Dropped silently before, which hid a relay refusing us.
                eprintln!("[session] relay error: {reason}");
                let _ = self.events.send(SessionEvent::RelayError { reason }).await;
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_incoming_envelope(self: &Arc<Self>, from_pk: &[u8; 32], blob: &[u8]) -> Result<()> {
        let envelope: EnvelopeBlob = bincode::deserialize(blob)?;
        let sealed = from_pk == &[0u8; 32];
        match envelope {
            EnvelopeBlob::X3dhInit(init) => {
                let sender_sign = init.identity.sign_pk;
                let sender_dh = init.identity.dh_pk;
                let contact = match self.db.find_contact_by_sign_pk(&sender_sign)? {
                    Some(c) => c,
                    None => {
                        let name = hex_short(&sender_sign);
                        let id = self.db.add_contact(&sender_sign, &sender_dh, "", &name, None)?;
                        let _ = self.events.send(SessionEvent::ContactAdded { contact_id: id }).await;
                        self.db.get_contact(id)?.ok_or(SessionError::NotFound)?
                    }
                };
                if contact.trust == TrustLevel::Blocked { return Ok(()); }
                if init.identity.sign_pk != contact.identity_sign.as_slice()
                    || init.identity.dh_pk != contact.identity_dh.as_slice()
                {
                    return Err(SessionError::State);
                }
                let ad = build_ad(&self.identity.card().dh_pk, &contact.identity_dh);
                eprintln!("[session] received X3dhInit from contact {}, accepting", contact.id);
                self.sessions.lock().await.remove(&contact.id);
                let _ = self.db.delete_session(contact.id);
                self.tiebreaker_waits.lock().await.remove(&contact.id);
                let (state, plaintext) = self.accept_x3dh(&init, &ad).await?;
                self.sessions.lock().await.insert(contact.id, state);
                self.session_created_at.lock().await.insert(contact.id, now_ms());
                let sb = {
                    let s = self.sessions.lock().await;
                    s.get(&contact.id).unwrap().to_bytes()?
                };
                self.db.put_session(contact.id, &sb)?;
                let payload: WirePayload = decode_with_padding_fallback(&plaintext)?;
                self.persist_incoming(contact.id, payload).await?;
                eprintln!("[session] session established with contact {} via X3dhInit", contact.id);
                if init.one_time_id.is_some() {
                    self.republish_bundle().await;
                }
                self.send_kick.notify_one();
            }
            EnvelopeBlob::Ratchet { header, ciphertext } => {
                let mut decrypted: Option<(i64, Vec<u8>, Vec<u8>, Contact)> = None;
                let candidates: Vec<Contact> = if sealed {
                    self.db.list_contacts()?.into_iter().filter(|c| c.trust != TrustLevel::Blocked).collect()
                } else {
                    match self.db.find_contact_by_sign_pk(from_pk)? {
                        Some(c) if c.trust != TrustLevel::Blocked => vec![c],
                        _ => vec![],
                    }
                };
                for c in candidates {
                    let ad = build_ad(&self.identity.card().dh_pk, &c.identity_dh);
                    let mut sess = self.sessions.lock().await;
                    let attempt = match sess.get_mut(&c.id) {
                        Some(state) => state.decrypt(&header, &ciphertext, &ad).map(|pt| (pt, state.to_bytes())),
                        None => match self.db.get_session(c.id)? {
                            Some(blob) => {
                                let mut state = RatchetState::from_bytes(&blob)?;
                                let r = state.decrypt(&header, &ciphertext, &ad).map(|pt| (pt, state.to_bytes()));
                                if r.is_ok() { sess.insert(c.id, state); }
                                r
                            }
                            None => continue,
                        },
                    };
                    drop(sess);
                    if let Ok((pt, sb_res)) = attempt {
                        let sb = sb_res?;
                        decrypted = Some((c.id, pt, sb, c));
                        break;
                    }
                }
                let (cid, pt, sb, contact) = match decrypted {
                    Some(x) => x,
                    None => {
                        if sealed {
                            eprintln!("[session] sealed ratchet: no session matched, ACK and drop");
                            return Err(SessionError::SealedDrop);
                        }
                        if let Ok(Some(c)) = self.db.find_contact_by_sign_pk(from_pk) {
                            eprintln!("[session] no session for {}, requesting resync", c.id);
                            let _ = self.request_resync(&c).await;
                        }
                        return Err(SessionError::Crypto(CryptoError::Mac));
                    }
                };
                self.db.put_session(cid, &sb)?;
                let payload: WirePayload = decode_with_padding_fallback(&pt)?;
                let is_keepalive = payload.ack_for == Some(0)
                    && payload.origin_msg_id == 0
                    && payload.body.is_empty()
                    && payload.attachments.is_empty();
                if !is_keepalive {
                    *self.incoming_since_send.lock().await.entry(cid).or_insert(0) += 1;
                }
                self.persist_incoming(cid, payload).await?;
                if !is_keepalive {
                    let counter = self.incoming_since_send.lock().await.get(&cid).copied().unwrap_or(0);
                    if counter >= KEEPALIVE_INCOMING_THRESHOLD {
                        self.send_kick.notify_one();
                    }
                }
                let _ = contact;
            }
        }
        Ok(())
    }

    async fn accept_x3dh(
        &self,
        init: &X3dhInitial,
        ad: &[u8],
    ) -> Result<(RatchetState, Vec<u8>)> {
        let signed_id = i64::from_be_bytes(
            self.db.get_setting(SETTING_SIGNED_PREKEY_ID)?.ok_or(SessionError::State)?
                .try_into().map_err(|_| SessionError::State)?);
        let signed = self.db.get_prekey(signed_id)?.ok_or(SessionError::State)?;
        let signed_pair = PreKeyPair::from_secret(to_arr32(signed.private.clone())?);
        let opk_pair = if let Some(opk_id) = init.one_time_id {
            let p = self.db.get_prekey(opk_id)?;
            if p.is_none() { return Err(SessionError::StaleOpk); }
            if let Some(ref pk) = p { let _ = self.db.delete_prekey(pk.id); }
            p.map(|p| Ok::<PreKeyPair, SessionError>(PreKeyPair::from_secret(to_arr32(p.private.clone())?))).transpose()?
        } else { None };
        let (state, pt) = crypto::x3dh_respond(&self.identity, &signed_pair, opk_pair.as_ref(), init, ad)?;
        Ok((state, pt))
    }

    async fn persist_incoming(self: &Arc<Self>, contact_id: i64, payload: WirePayload) -> Result<()> {
        if let Some(name) = payload.sender_name.as_deref() {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                self.apply_peer_name(contact_id, trimmed).await;
            }
        }

        if let Some(relay) = payload.relay_address.as_deref() {
            let trimmed = relay.trim();
            if !trimmed.is_empty() && crate::card::is_valid_i2p_address(trimmed) {
                if let Ok(current) = self.db.contact_relay(contact_id) {
                    if current.as_deref() != Some(trimmed) {
                        eprintln!("[relay-discovery] updated relay for contact {} to {}", contact_id, &trimmed[..trimmed.len().min(16)]);
                        let _ = self.db.set_contact_relay(contact_id, Some(trimmed));
                    }
                }
            }
        }
        let is_empty = payload.body.is_empty()
            && payload.attachments.is_empty()
            && payload.group.is_none()
            && payload.callback_data.is_none()
            && payload.edit_of.is_none()
            && payload.pin.is_none()
            && payload.ack_for.is_none()
            && payload.console.is_none();
        if is_empty { return Ok(()); }

        if let Some(gref) = &payload.group {
            ensure_group_session(&self.db, &self.identity, gref)?;
        }

        if let Some(ack_local) = payload.ack_for {
            let local_id = ack_local as i64;
            if let Some(msg) = self.db.get_message(local_id)? {
                let belongs = match (msg.contact_id, &msg.group_id, &payload.group) {
                    (Some(cid), _, None) => cid == contact_id,
                    (_, Some(gid), Some(gref)) => gid == &gref.id,
                    _ => false,
                };
                if belongs && matches!(msg.direction, Direction::Out) {
                    self.db.mark_delivered(local_id)?;
                    let _ = self.events.send(SessionEvent::MessageDelivered { message_id: local_id }).await;
                }
            }
            return Ok(());
        }

        if payload.callback_data.is_some()
            && payload.edit_of.is_none()
            && payload.pin.is_none()
            && payload.body.is_empty()
            && payload.attachments.is_empty()
        {
            let _ = self.events.send(SessionEvent::IncomingPayload {
                contact_id, payload, message_id: 0,
            }).await;
            return Ok(());
        }

        if let Some(edit_target_origin) = payload.edit_of {
            let lookup = if let Some(gref) = &payload.group {
                let contact = self.db.get_contact(contact_id)?.ok_or(SessionError::NotFound)?;
                let self_sign = self.identity.card().sign_pk.to_vec();
                self.db.resolve_group_message(&gref.id, &contact.identity_sign, edit_target_origin as i64, &self_sign)?
            } else {
                self.db.find_message_by_origin(contact_id, edit_target_origin as i64)?
            };
            if let Some(local_id) = lookup {
                let buttons_bytes = payload.buttons.as_ref().and_then(|b| bincode::serialize(b).ok());
                self.db.update_message_body(local_id, &payload.body)?;
                if let Some(b) = buttons_bytes {
                    self.db.set_setting(&format!("buttons_{}", local_id), &b)?;
                } else {
                    let _ = self.db.delete_setting(&format!("buttons_{}", local_id));
                }
                let _ = self.events.send(SessionEvent::MessageEdited {
                    message_id: local_id, new_body: payload.body.clone(), buttons: payload.buttons.clone(),
                }).await;
                return Ok(());
            }
        }

        if let Some(pin) = &payload.pin {
            let self_sign = self.identity.card().sign_pk.to_vec();
            let (local, target_cid, target_gid) = if let Some(gref) = &payload.group {
                (self.db.resolve_group_message(&gref.id, &pin.sender_sign_pk, pin.origin_msg_id as i64, &self_sign)?,
                 None, Some(gref.id.clone()))
            } else {
                (self.db.resolve_contact_message(contact_id, pin.origin_msg_id as i64)?,
                 Some(contact_id), None)
            };
            if let Some(local) = local {
                let _ = match (&target_gid, pin.unpin) {
                    (Some(gid), true)  => { self.db.unpin_group_message(gid, local)?; }
                    (Some(gid), false) => { self.db.pin_group_message(gid, local)?; }
                    (None, true)       => { self.db.unpin_contact_message(contact_id, local)?; }
                    (None, false)      => { self.db.pin_contact_message(contact_id, local)?; }
                };
                let ev = if pin.unpin {
                    SessionEvent::MessageUnpinned { contact_id: target_cid, group_id: target_gid, message_id: local }
                } else {
                    SessionEvent::MessagePinned { contact_id: target_cid, group_id: target_gid, message_id: local }
                };
                let _ = self.events.send(ev).await;
            }
            return Ok(());
        }

        let contact = self.db.get_contact(contact_id)?.ok_or(SessionError::NotFound)?;

        if payload.origin_msg_id > 0 {
            let existing = if let Some(gref) = &payload.group {
                let self_sign = self.identity.card().sign_pk.to_vec();
                self.db.resolve_group_message(&gref.id, &contact.identity_sign, payload.origin_msg_id as i64, &self_sign)?
            } else {
                self.db.find_message_by_origin(contact_id, payload.origin_msg_id as i64)?
            };
            if existing.is_some() {
                eprintln!("[session] duplicate origin={} from contact {}, re-acking", payload.origin_msg_id, contact_id);
                let _ = self.send_ack(&contact, &payload).await;
                return Ok(());
            }
        }

        let expires_at = payload.ttl_ms.map(|t| payload.sent_at + t);
        let mut atts = Vec::with_capacity(payload.attachments.len());
        for a in &payload.attachments {
            let (key, path, size) = store_attachment(&self.data_dir, &a.data)?;
            atts.push(NewAttachment { name: a.name.clone(), size: size as i64, key: key.to_vec(), path });
        }
        let mid = if let Some(gref) = &payload.group {
            self.db.insert_group_message_with_origin(
                &gref.id, Some(&contact.identity_sign), Direction::In,
                &payload.body, payload.sent_at, expires_at, &atts,
                Some(payload.origin_msg_id as i64),
            )?
        } else {
            self.db.insert_message_with_origin(
                contact_id, Direction::In, &payload.body, payload.sent_at, expires_at, &atts,
                Some(payload.origin_msg_id as i64),
            )?
        };
        if let Some(btns) = &payload.buttons {
            if let Ok(b) = bincode::serialize(btns) {
                self.db.set_setting(&format!("buttons_{}", mid), &b)?;
            }
        }
        if let Some(c) = &payload.console {
            if let Ok(b) = bincode::serialize(c) {
                self.db.set_setting(&format!("console_{}", mid), &b)?;
            }
            // Console framing is a DM affair; in a group it is stored for
            // display and nothing more.
            if payload.group.is_none() {
                match c.kind {
                    CONSOLE_COMMAND => {
                        self.db.set_setting(&format!("console_pending_{}", mid), b"1")?;
                    }
                    CONSOLE_GRANT | CONSOLE_REVOKE => {
                        let granted = c.kind == CONSOLE_GRANT;
                        if self.db.set_contact_agent_granted(contact_id, granted).unwrap_or(false) {
                            let _ = self.events.send(SessionEvent::ContactUpdated { contact_id }).await;
                        }
                    }
                    _ => {}
                }
            }
        }
        self.db.touch_contact(contact_id)?;
        let payload_for_ack = payload.clone();
        let _ = self.events.send(SessionEvent::IncomingPayload {
            contact_id, payload, message_id: mid,
        }).await;
        let _ = self.send_ack(&contact, &payload_for_ack).await;
        Ok(())
    }

    pub async fn reset_contact_session(&self, contact_id: i64) -> Result<()> {
        let contact = self.db.get_contact(contact_id)?.ok_or(SessionError::NotFound)?;
        self.request_resync(&contact).await
    }

    async fn apply_peer_name(&self, contact_id: i64, name: &str) {
        let contact_changed = self.db.update_contact_name(contact_id, name).unwrap_or(false);
        let mut group_changed = false;
        if let Ok(Some(c)) = self.db.get_contact(contact_id) {
            if let Ok(groups) = self.db.list_groups_with_member(&c.identity_sign) {
                for gid in groups {
                    if self.db.update_group_member_name(&gid, &c.identity_sign, name).unwrap_or(false) {
                        group_changed = true;
                    }
                }
            }
        }
        if contact_changed || group_changed {
            let _ = self.events.send(SessionEvent::ContactUpdated { contact_id }).await;
        }
    }

    async fn request_resync(&self, contact: &Contact) -> Result<()> {
        let throttled = self.db.resync_recent(contact.id, 60_000)?;
        if !throttled {
            eprintln!("[session] forcing resync for contact {}", contact.id);
            self.db.record_resync(contact.id)?;
            self.sessions.lock().await.remove(&contact.id);
            self.session_created_at.lock().await.remove(&contact.id);
            let _ = self.db.delete_session(contact.id);
            let mut w = self.tiebreaker_waits.lock().await;
            w.insert(contact.id, now_ms() - TIEBREAKER_TIMEOUT_MS - 1);
        }
        self.send_kick.notify_one();
        Ok(())
    }

    async fn send_ack(self: &Arc<Self>, contact: &Contact, original: &WirePayload) -> Result<()> {
        if original.origin_msg_id == 0 { return Ok(()); }
        let mut payload = WirePayload {
            origin_msg_id: 0,
            body: String::new(),
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: original.group.clone(),
            buttons: None,
            callback_data: None,
            edit_of: None,
            pin: None,
            ack_for: Some(original.origin_msg_id),
            sender_name: None,
            reply_to: None,
            typing: None,
            notify_sound: None,
            console: None,
            relay_address: None,
        };
        // The contact's relay, not ours: an ack left on our own relay is never
        // collected by anyone.
        let Some(out) = self.relay_for(contact).await else { return Ok(()) };
        if self.ensure_session_for(contact, &out).await.is_err() {
            return Ok(());
        }
        let _ = self.send_payload_via_relay(contact, &mut payload, &out).await;
        Ok(())
    }

    fn spawn_send_loop(self: Arc<Self>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            tick.tick().await;
            loop {
                tokio::select! {
                    _ = this.send_kick.notified() => {}
                    _ = tick.tick() => {}
                }
                if let Err(e) = this.flush_all_pending().await {
                    eprintln!("[session] flush err: {:?}", e);
                }
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    async fn flush_all_pending(self: &Arc<Self>) -> Result<()> {
        // Our own relay must be up: replies come back there.
        if self.relay_out.read().await.is_none() {
            return Ok(());
        }
        for contact in self.db.list_contacts()? {
            if contact.trust == TrustLevel::Blocked { continue; }
            let pending = self.db.list_unsent_outgoing(contact.id, 50)?;
            let unacked = self.db.list_unacked_outgoing(
                contact.id, now_ms(), RETRY_BASE_BACKOFF_MS, RETRY_MAX_BACKOFF_MS, 50,
            )?;
            let needs_session = self.db.resync_recent(contact.id, 120_000).unwrap_or(false)
                && !self.sessions.lock().await.contains_key(&contact.id);
            let needs_keepalive = self.incoming_since_send.lock().await.get(&contact.id).copied().unwrap_or(0) >= KEEPALIVE_INCOMING_THRESHOLD
                && self.sessions.lock().await.contains_key(&contact.id);
            if pending.is_empty() && unacked.is_empty() && !needs_session && !needs_keepalive { continue; }
            // Deposit where this contact collects, not where we do.
            let Some(out) = self.relay_for(&contact).await else { continue };
            if self.ensure_session_for(&contact, &out).await.is_err() { continue; }
            if needs_keepalive && pending.is_empty() && unacked.is_empty() {
                let mut payload = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
                payload.ack_for = Some(0);
                if let Err(e) = self.send_payload_via_relay(&contact, &mut payload, &out).await {
                    eprintln!("[session] keepalive err to contact {}: {:?}", contact.id, e);
                } else {
                    eprintln!("[session] keepalive sent to contact {} (DH-roll forced)", contact.id);
                }
            }
            for msg in pending {
                let mut payload = self.build_payload_from_db(&msg)?;
                if let Err(e) = self.send_payload_via_relay(&contact, &mut payload, &out).await {
                    eprintln!("[session] send err contact {}: {:?}", contact.id, e);
                    break;
                }
            }
            for msg in unacked {
                let mut payload = self.build_payload_from_db(&msg)?;
                eprintln!("[session] retry unacked msg {} to contact {} (attempt {})",
                    msg.id, contact.id, msg.send_attempts + 1);
                if let Err(e) = self.send_payload_via_relay(&contact, &mut payload, &out).await {
                    eprintln!("[session] retry err contact {}: {:?}", contact.id, e);
                    break;
                }
            }
        }
        Ok(())
    }

    async fn ensure_session_for(
        &self,
        contact: &Contact,
        out: &mpsc::Sender<ClientToRelay>,
    ) -> Result<()> {
        if self.sessions.lock().await.contains_key(&contact.id) { return Ok(()); }
        if let Some(blob) = self.db.get_session(contact.id)? {
            self.sessions.lock().await.insert(contact.id, RatchetState::from_bytes(&blob)?);
            return Ok(());
        }
        let me_sign = self.identity.card().sign_pk;
        let should_initiate = me_sign.as_slice() < contact.identity_sign.as_slice();
        if !should_initiate {
            let waited = {
                let mut w = self.tiebreaker_waits.lock().await;
                let now = now_ms();
                let started = *w.entry(contact.id).or_insert(now);
                now - started
            };
            if waited < TIEBREAKER_TIMEOUT_MS {
                return Err(SessionError::State);
            }
            eprintln!("[session] tiebreaker timeout, initiating for contact {}", contact.id);
        }
        self.tiebreaker_waits.lock().await.remove(&contact.id);

        let mut pk = [0u8; 32];
        pk.copy_from_slice(&contact.identity_sign);
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.bundle_waiters.lock().await.entry(pk).or_default().push(tx);
        out.send(ClientToRelay::GetBundle { pk }).await.map_err(|_| SessionError::State)?;
        let bundle_bytes = tokio::time::timeout(Duration::from_millis(PENDING_REQ_TIMEOUT_MS), rx).await
            .map_err(|_| SessionError::State)?
            .map_err(|_| SessionError::State)?;
        let bundle_bytes = bundle_bytes.ok_or(SessionError::NotFound)?;
        let bundle: PreKeyBundle = bincode::deserialize(&bundle_bytes)?;

        if bundle.identity.sign_pk != contact.identity_sign.as_slice()
            || bundle.identity.dh_pk != contact.identity_dh.as_slice()
        {
            return Err(SessionError::State);
        }
        let ad = build_ad(&self.identity.card().dh_pk, &contact.identity_dh);
        let mut empty = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
        empty.sender_name = self.outgoing_sender_name();
        // A contact created from this init would otherwise have no relay to
        // answer to until a later message brings one.
        let relay = self.relay_onion();
        if !relay.is_empty() {
            empty.relay_address = Some(relay);
        }
        let pt = pad_payload(&encode_payload(&empty)?);
        let (state, init) = crypto::x3dh_initiate(&self.identity, &bundle, &pt, &ad)?;
        self.db.put_session(contact.id, &state.to_bytes()?)?;
        self.sessions.lock().await.insert(contact.id, state);
        self.session_created_at.lock().await.insert(contact.id, now_ms());
        let mut to = [0u8; 32];
        to.copy_from_slice(&contact.identity_sign);
        let blob = bincode::serialize(&EnvelopeBlob::X3dhInit(init))?;
        out.send(ClientToRelay::Send { to, blob }).await.map_err(|_| SessionError::State)?;
        eprintln!("[session] x3dh sent to contact {}", contact.id);
        Ok(())
    }

    async fn send_payload_via_relay(
        &self,
        contact: &Contact,
        payload: &mut WirePayload,
        out: &mpsc::Sender<ClientToRelay>,
    ) -> Result<()> {
        if payload.sender_name.is_none() {
            payload.sender_name = self.outgoing_sender_name();
        }
        // Every payload tells the contact where we collect, except typing
        // notices: the most frequent payload, and ~520 bytes of address each.
        if payload.relay_address.is_none() && payload.typing.is_none() {
            let r = self.relay_onion();
            if !r.is_empty() {
                payload.relay_address = Some(r);
            }
        }
        let ad = build_ad(&self.identity.card().dh_pk, &contact.identity_dh);
        let raw = encode_payload(payload)?;
        if raw.len() > MAX_PAYLOAD_BYTES {
            eprintln!("[session] payload too large ({}B), dropping msg id={}", raw.len(), payload.origin_msg_id);
            if payload.origin_msg_id > 0 {
                let _ = self.db.mark_sent(payload.origin_msg_id as i64);
            }
            return Err(SessionError::State);
        }
        let pt = pad_payload(&raw);
        let (header, ct) = {
            let mut sess = self.sessions.lock().await;
            let state = sess.get_mut(&contact.id).ok_or(SessionError::State)?;
            let r = state.encrypt(&pt, &ad)?;
            self.db.put_session(contact.id, &state.to_bytes()?)?;
            r
        };
        let blob = bincode::serialize(&EnvelopeBlob::Ratchet { header, ciphertext: ct })?;
        let mut to = [0u8; 32];
        to.copy_from_slice(&contact.identity_sign);
        out.send(ClientToRelay::Send { to, blob }).await.map_err(|_| SessionError::State)?;
        self.incoming_since_send.lock().await.insert(contact.id, 0);
        if payload.origin_msg_id > 0
            && payload.edit_of.is_none()
            && payload.pin.is_none()
            && payload.callback_data.is_none()
            && payload.ack_for.is_none()
        {
            self.db.mark_sent(payload.origin_msg_id as i64)?;
        }
        Ok(())
    }

    fn build_payload_from_db(&self, msg: &crate::db::Message) -> Result<WirePayload> {
        let atts = self.db.list_attachments(msg.id)?;
        let mut wire_atts = Vec::with_capacity(atts.len());
        for a in atts {
            let key = to_arr32(a.key.clone())?;
            let full = self.data_dir.join(ATTACHMENTS_DIR).join(&a.path);
            let enc = std::fs::read(&full)?;
            let data = AttachmentCipher::from_key(key).decrypt_chunk(0, &[], &enc)?;
            wire_atts.push(WireAttachment { name: a.name, data });
        }
        let buttons: Option<Vec<Vec<WireButton>>> = self.db.get_setting(&format!("buttons_{}", msg.id))
            .ok().flatten()
            .and_then(|b| bincode::deserialize::<Vec<Vec<WireButton>>>(&b).ok());
        let sound: Option<String> = self.db.get_setting(&format!("sound_{}", msg.id))
            .ok().flatten()
            .and_then(|b| String::from_utf8(b).ok());
        let console: Option<WireConsole> = self.db.get_setting(&format!("console_{}", msg.id))
            .ok().flatten()
            .and_then(|b| bincode::deserialize::<WireConsole>(&b).ok());
        let ttl_ms = msg.expires_at.map(|e| e - msg.sent_at);
        let mut p = WirePayload::simple(msg.id as u64, msg.body.clone(), wire_atts, msg.sent_at, ttl_ms);
        p.buttons = buttons;
        p.notify_sound = sound;
        p.console = console;
        Ok(p)
    }

    fn spawn_bundle_refresh_loop(self: Arc<Self>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(BUNDLE_REFRESH_SECS));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            tick.tick().await;
            loop {
                tick.tick().await;
                this.republish_bundle().await;
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    async fn republish_bundle(&self) {
        let _ = self.ensure_prekeys().await;
        if let Some(tx) = self.relay_out.read().await.clone() {
            if let Ok(b) = self.my_bundle() {
                if let Ok(bytes) = bincode::serialize(&b) {
                    let _ = tx.send(ClientToRelay::Publish { bundle: bytes }).await;
                }
            }
        }
    }
}

fn ensure_group_session(db: &Arc<Db>, identity: &Arc<Identity>, gref: &WireGroupRef) -> Result<()> {
    if db.get_group_name(&gref.id)?.is_none() {
        db.create_group(&gref.id, &gref.name)?;
    }
    let my_sign = identity.card().sign_pk;
    for m in &gref.members {
        let is_self = m.sign_pk.as_slice() == my_sign.as_slice();
        if db.is_group_member(&gref.id, &m.sign_pk)? { continue; }
        let name = if m.name.is_empty() { hex_short(&m.sign_pk) } else { m.name.clone() };
        db.add_group_member(&gref.id, &m.sign_pk, &m.dh_pk, &m.onion, &name, is_self)?;
        if !is_self && db.find_contact_by_identity(&m.dh_pk)?.is_none() {
            let _ = db.add_contact(&m.sign_pk, &m.dh_pk, &m.onion, &name, None)?;
        }
    }
    Ok(())
}

fn load_or_create_identity(db: &Db) -> Result<Identity> {
    match (db.get_setting(SETTING_IDENTITY_SIGN)?, db.get_setting(SETTING_IDENTITY_DH)?) {
        (Some(s), Some(d)) if s.len() == 32 && d.len() == 32 => {
            let mut sign = [0u8; 32]; sign.copy_from_slice(&s);
            let mut dh = [0u8; 32]; dh.copy_from_slice(&d);
            Ok(Identity::from_bytes(sign, dh))
        }
        _ => {
            let id = Identity::generate();
            db.set_setting(SETTING_IDENTITY_SIGN, id.sign_seed())?;
            db.set_setting(SETTING_IDENTITY_DH, id.dh_secret())?;
            Ok(id)
        }
    }
}

fn store_attachment(data_dir: &PathBuf, data: &[u8]) -> Result<([u8; 32], String, u64)> {
    let cipher = AttachmentCipher::generate();
    let encrypted = cipher.encrypt_chunk(0, &[], data)?;
    let mut name = [0u8; 24];
    crate::crypto::fill_random(&mut name);
    let hex = to_hex(&name);
    let path = data_dir.join(ATTACHMENTS_DIR).join(&hex);
    std::fs::write(&path, &encrypted)?;
    Ok((*cipher.key(), hex, data.len() as u64))
}

fn build_ad(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    if a <= b { out.extend_from_slice(a); out.extend_from_slice(b); }
    else      { out.extend_from_slice(b); out.extend_from_slice(a); }
    out
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn to_arr32(v: Vec<u8>) -> Result<[u8; 32]> {
    v.try_into().map(|a: Vec<u8>| {
        let mut o = [0u8; 32]; o.copy_from_slice(&a); o
    }).map_err(|_| SessionError::State)
}

fn to_hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b { s.push_str(&format!("{:02x}", x)); }
    s
}

fn hex_short(b: &[u8]) -> String {
    let mut s = String::new();
    for x in &b[..8.min(b.len())] { s.push_str(&format!("{:02x}", x)); }
    s
}

#[cfg(test)]
mod wire_tests {
    use super::*;

    fn sample() -> WirePayload {
        let mut p = WirePayload::simple(7, "uptime".into(), vec![], 1_700_000_000_000, None);
        p.sender_name = Some("laptop".into());
        p
    }

    #[test]
    fn console_roundtrips() {
        let mut p = sample();
        p.console = Some(WireConsole {
            kind: CONSOLE_OUTPUT, exit_code: Some(0), duration_ms: Some(42), truncated: true,
        });
        let bytes = encode_payload(&p).unwrap();
        let back = decode_payload(&bytes).unwrap();
        assert_eq!(back.console, p.console);
        assert_eq!(back.body, "uptime");
        assert_eq!(back.sender_name.as_deref(), Some("laptop"));
    }

    #[test]
    fn older_shapes_decode_with_no_console() {
        // sender_name alone is a WireV4; a newer client must still read it.
        let p = sample();
        let bytes = encode_payload(&p).unwrap();
        assert!(bincode::deserialize::<WirePayload>(&bytes).is_err(), "must fall through, not misread");
        let back = decode_payload(&bytes).unwrap();
        assert_eq!(back.console, None);
        assert_eq!(back.sender_name.as_deref(), Some("laptop"));
    }

    #[test]
    fn notify_sound_alone_is_the_v7_shape() {
        let mut p = sample();
        p.notify_sound = Some("ping".into());
        let bytes = encode_payload(&p).unwrap();
        assert_eq!(bytes, bincode::serialize(&WireV7::from(&p)).unwrap());
        let back = decode_payload(&bytes).unwrap();
        assert_eq!(back.notify_sound.as_deref(), Some("ping"));
        assert_eq!(back.console, None);
    }

    #[test]
    fn a_build_without_the_console_field_still_reads_the_body() {
        // What a pre-console client does: deserialize its own newest shape from
        // bytes that carry one more trailing field. bincode's `deserialize`
        // tolerates trailing bytes, so the marker body comes through as text.
        let mut p = sample();
        p.body = crate::agent::BODY_GRANT.into();
        p.console = Some(WireConsole::new(CONSOLE_GRANT));
        let bytes = encode_payload(&p).unwrap();
        let old: WireV7 = bincode::deserialize(&bytes).expect("trailing field tolerated");
        assert_eq!(old.body, "[agent on]");
        assert_eq!(old.sender_name.as_deref(), Some("laptop"));
    }

    #[test]
    fn unknown_console_kind_still_decodes() {
        let mut p = sample();
        p.console = Some(WireConsole::new(200));
        let back = decode_payload(&encode_payload(&p).unwrap()).unwrap();
        assert_eq!(back.console.map(|c| c.kind), Some(200));
    }
}