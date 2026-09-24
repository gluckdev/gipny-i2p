use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::crypto::{self, AttachmentCipher, Identity, PreKeyBundle, PreKeyPair, RatchetState, X3dhInitial, CryptoError};
use crate::db::{Contact, Db, DbError, Direction, GroupMember, NewAttachment, PreKeyKind, TrustLevel};
use crate::dht_client;
use crate::net::{NetError, TorNode};
use crate::relay::{self, ClientToRelay, EnvelopeBlob, RelayClient, RelayError, RelayToClient, DEFAULT_RELAY};
use crate::relay_server::DhtHandler;

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
    /// `failures` in a row: the next wait is [`peer_relay_backoff`] of it.
    Failed { until: std::time::Instant, failures: u32 },
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

/// Whether to open one more connection to our own relay for file parts:
/// it is elsewhere, parts came lately, and fewer than `FILE_LANES - 1` extra
/// are open.
pub fn collect_lane_wanted(external: bool, part_seen_ms: i64, now_ms: i64, open: usize) -> bool {
    external && open + 1 < FILE_LANES && collect_lane_holds(part_seen_ms, now_ms)
}

/// Whether extra connections for parts are still worth holding.
pub fn collect_lane_holds(part_seen_ms: i64, now_ms: i64) -> bool {
    part_seen_ms > 0 && now_ms - part_seen_ms < COLLECT_LANE_IDLE_MS
}

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
/// A part unacknowledged this long, by a recipient heard from since, goes again.
const FILE_RESEND_IDLE_MS: i64 = 30 * 60 * 1000;
/// Streams to a recipient's relay carrying parts at once (the usual one
/// included).
const FILE_LANES: usize = 4;
/// Extra connections to our own relay, when it is not in this process, are
/// held while parts keep coming and closed this long after the last one.
const COLLECT_LANE_IDLE_MS: i64 = 60_000;
/// A recipient alive this long without acknowledging a single part cannot
/// take files in parts.
const FILE_GIVE_UP_MS: i64 = 24 * 3600 * 1000;
/// Unfinished incoming transfers are dropped after the relay's letter TTL.
const FILE_IN_TTL_MS: i64 = 7 * 24 * 3600 * 1000;
const RETRY_BASE_BACKOFF_MS: i64 = 5_000;
const RETRY_MAX_BACKOFF_MS: i64 = 300_000;
const DHT_ADDRESS_LOOKUP_EVERY: Duration = Duration::from_secs(10 * 60);
const DHT_COLLECT_EVERY: Duration = Duration::from_secs(10 * 60);
const DHT_SEEN_TTL_MS: i64 = 8 * 24 * 3600 * 1000;
const DHT_COLLECT_DAYS_FIRST: u32 = 7;
const DHT_COLLECT_DAYS: u32 = 2;

/// Where an outgoing envelope goes. A live relay is preferred; the relay
/// network is the store-and-forward fallback when that relay is unavailable.
enum Route {
    Relay(mpsc::Sender<ClientToRelay>),
    Dht,
}

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
    /// "Forget me": the sender deleted this contact and asks the recipient to
    /// delete the conversation and the contact too. Only ever honoured from
    /// the contact itself, inside its ratchet session.
    #[serde(default)]
    pub wipe: Option<bool>,
    /// Files too large to ride inside one letter, named here and sent in parts
    /// on the same session ([`WireFileChunk`]); see `crate::files`.
    #[serde(default)]
    pub files: Vec<WireFileOffer>,
    /// One part of an offered file. A letter carrying it has no text and no
    /// `origin_msg_id`: it is not a message in the chat.
    #[serde(default)]
    pub file_chunk: Option<WireFileChunk>,
    /// What the recipient holds of a file so far.
    #[serde(default)]
    pub file_ack: Option<WireFileAck>,
    /// The sender stopped sending this file (or the recipient refused it).
    #[serde(default)]
    pub file_cancel: Option<[u8; 16]>,
}

/// A file sent in parts: what it is, so the recipient can take the parts in
/// any order and check the whole at the end.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct WireFileOffer {
    pub file_id: [u8; 16],
    pub name: String,
    pub size: u64,
    pub sha256: [u8; 32],
    /// Plaintext bytes per part; the last may be shorter.
    pub chunk_size: u32,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct WireFileChunk {
    pub file_id: [u8; 16],
    pub index: u32,
    pub data: Vec<u8>,
}

/// Cumulative and selective: every part below `received_up_to` is here, and
/// of those after it, `missing` are known lost (a later one arrived). Only
/// `missing` makes the sender resend quickly; see `crate::files`.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct WireFileAck {
    pub file_id: [u8; 16],
    pub received_up_to: u32,
    pub missing: Vec<u32>,
}

impl WirePayload {
    pub fn simple(origin: u64, body: String, attachments: Vec<WireAttachment>, sent_at: i64, ttl_ms: Option<i64>) -> Self {
        Self {
            origin_msg_id: origin, body, attachments, sent_at, ttl_ms,
            group: None, buttons: None, callback_data: None, edit_of: None, pin: None,
            ack_for: None, sender_name: None, reply_to: None,
            typing: None, notify_sound: None, console: None, relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None,
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

/// Everything up to `wipe`: the full payload up to the files sent in parts.
#[derive(Serialize, Deserialize)]
struct WireV9 {
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
    console: Option<WireConsole>,
    relay_address: Option<String>,
    wipe: Option<bool>,
}

impl From<&WirePayload> for WireV9 {
    fn from(p: &WirePayload) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id, body: p.body.clone(), attachments: p.attachments.clone(),
            sent_at: p.sent_at, ttl_ms: p.ttl_ms, group: p.group.clone(),
            buttons: p.buttons.clone(), callback_data: p.callback_data.clone(),
            edit_of: p.edit_of, pin: p.pin.clone(), ack_for: p.ack_for,
            sender_name: p.sender_name.clone(), reply_to: p.reply_to.clone(),
            typing: p.typing, notify_sound: p.notify_sound.clone(),
            console: p.console.clone(), relay_address: p.relay_address.clone(), wipe: p.wipe,
        }
    }
}

impl From<WireV9> for WirePayload {
    fn from(v: WireV9) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: v.buttons, callback_data: v.callback_data,
            edit_of: v.edit_of, pin: v.pin, ack_for: v.ack_for, sender_name: v.sender_name,
            reply_to: v.reply_to, typing: v.typing, notify_sound: v.notify_sound,
            console: v.console, relay_address: v.relay_address, wipe: v.wipe,
            files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None,
        }
    }
}

/// Everything before `wipe`: the full payload up to 0.4.13.
#[derive(Serialize, Deserialize)]
struct WireV8 {
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
    console: Option<WireConsole>,
    relay_address: Option<String>,
}

impl From<&WirePayload> for WireV8 {
    fn from(p: &WirePayload) -> Self {
        Self {
            origin_msg_id: p.origin_msg_id, body: p.body.clone(), attachments: p.attachments.clone(),
            sent_at: p.sent_at, ttl_ms: p.ttl_ms, group: p.group.clone(),
            buttons: p.buttons.clone(), callback_data: p.callback_data.clone(),
            edit_of: p.edit_of, pin: p.pin.clone(), ack_for: p.ack_for,
            sender_name: p.sender_name.clone(), reply_to: p.reply_to.clone(),
            typing: p.typing, notify_sound: p.notify_sound.clone(),
            console: p.console.clone(), relay_address: p.relay_address.clone(),
        }
    }
}

impl From<WireV8> for WirePayload {
    fn from(v: WireV8) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: v.buttons, callback_data: v.callback_data,
            edit_of: v.edit_of, pin: v.pin, ack_for: v.ack_for, sender_name: v.sender_name,
            reply_to: v.reply_to, typing: v.typing, notify_sound: v.notify_sound,
            console: v.console, relay_address: v.relay_address, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None,
        }
    }
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
            relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None,
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
            relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None,
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
            reply_to: v.reply_to, typing: None, notify_sound: None, console: None, relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None,
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
            reply_to: None, typing: None, notify_sound: None, console: None, relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None,
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
            sender_name: None, reply_to: None, typing: None, notify_sound: None, console: None, relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None,
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
            ack_for: None, sender_name: None, reply_to: None, typing: None, notify_sound: None, console: None, relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None,
        }
    }
}

impl From<WireV1> for WirePayload {
    fn from(v: WireV1) -> Self {
        Self {
            origin_msg_id: v.origin_msg_id, body: v.body, attachments: v.attachments,
            sent_at: v.sent_at, ttl_ms: v.ttl_ms, group: v.group,
            buttons: v.buttons, callback_data: v.callback_data,
            edit_of: None, pin: None, ack_for: None, sender_name: None, reply_to: None, typing: None, notify_sound: None, console: None, relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None,
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
            relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None,
        }
    }
}

pub fn encode_payload(p: &WirePayload) -> std::result::Result<Vec<u8>, bincode::Error> {
    let has_files = !p.files.is_empty() || p.file_chunk.is_some() || p.file_ack.is_some() || p.file_cancel.is_some();
    if has_files                                         { bincode::serialize(p) }
    else if p.wipe.is_some()                             { bincode::serialize(&WireV9::from(p)) }
    else if p.console.is_some() || p.relay_address.is_some() { bincode::serialize(&WireV8::from(p)) }
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
    if let Ok(v) = bincode::deserialize::<WireV9>(pt)      { return Ok(v.into()); }
    if let Ok(v) = bincode::deserialize::<WireV8>(pt)      { return Ok(v.into()); }
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

/// As [`pad_payload`], but the padding is optional.
///
/// The length prefix is written either way, so the receiver does not care and
/// does not need to be told: an unpadded payload is simply one whose bucket
/// happened to equal its length, and `unpad_payload` reads it unchanged. That
/// is what lets the fast lanes drop padding without a protocol change and
/// without breaking anyone running an older build.
pub fn pack_payload(pt: &[u8], padded: bool) -> Vec<u8> {
    if padded {
        return pad_payload(pt);
    }
    let mut out = Vec::with_capacity(4 + pt.len());
    out.extend_from_slice(&(pt.len() as u32).to_be_bytes());
    out.extend_from_slice(pt);
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
    /// It will not go: too large for one letter (files go in parts; this is
    /// a letter that could not). Marked sent so it is not retried forever.
    MessageFailed { message_id: i64, reason: String },
    /// A file in parts moved on: `done` of `total` parts, sent (acknowledged
    /// by `contact_id`) or received.
    FileProgress { message_id: i64, contact_id: i64, incoming: bool, done: u32, total: u32 },
    /// A file in parts is whole and is now attachment `attachment_id`.
    FileReceived { message_id: i64, attachment_id: i64 },
    /// A file in parts will not get there: the recipient never acknowledged
    /// a part while alive (an older build), or cancelled; or it came broken.
    FileFailed { message_id: i64, contact_id: i64, reason: String },
    MessageEdited { message_id: i64, new_body: String, buttons: Option<Vec<Vec<WireButton>>> },
    MessagePinned { contact_id: Option<i64>, group_id: Option<Vec<u8>>, message_id: i64 },
    MessageUnpinned { contact_id: Option<i64>, group_id: Option<Vec<u8>>, message_id: i64 },
    ContactAdded { contact_id: i64 },
    ContactUpdated { contact_id: i64 },
    /// The contact deleted us and asked for the chat to go; the contact and
    /// every message with them are gone here now.
    ContactWiped { contact_id: i64 },
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
    /// When each contact was last heard from (anything decrypted): a part is
    /// resent on a timer only to someone alive since it went.
    heard: Arc<std::sync::Mutex<HashMap<i64, i64>>>,
    /// Extra connections to a contact's relay, for file parts only (by relay
    /// destination). One i2p stream carries at most its window per round
    /// trip; parts spread over several go that many times faster.
    file_lanes: Arc<Mutex<HashMap<String, Vec<mpsc::Sender<ClientToRelay>>>>>,
    /// Relays a lane is being dialled to, so one is dialled at a time.
    file_lanes_dialling: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// When the last file part came (ms), and how many extra connections to
    /// our own relay are open to take them: the relay deals new mail round
    /// every connection of a key, so parts come down several streams at once.
    part_seen: Arc<std::sync::atomic::AtomicI64>,
    collect_lanes: Arc<std::sync::atomic::AtomicUsize>,
    collect_kick: Arc<tokio::sync::Notify>,
    session_created_at: Arc<Mutex<HashMap<i64, i64>>>,
    incoming_since_send: Arc<Mutex<HashMap<i64, u32>>>,
    dht: Arc<dht_client::Node>,
    /// Our own relay when it runs in this process: reached over a pipe.
    local_relay: std::sync::Mutex<Option<Arc<crate::EphemeralRelay>>>,
    dht_addr_asked: Arc<Mutex<HashMap<i64, Instant>>>,
    dht_kick: Arc<tokio::sync::Notify>,
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
            heard: Arc::new(std::sync::Mutex::new(HashMap::new())),
            file_lanes: Arc::new(Mutex::new(HashMap::new())),
            file_lanes_dialling: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            part_seen: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            collect_lanes: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            collect_kick: Arc::new(tokio::sync::Notify::new()),
            session_created_at: Arc::new(Mutex::new(HashMap::new())),
            incoming_since_send: Arc::new(Mutex::new(HashMap::new())),
            dht,
            local_relay: std::sync::Mutex::new(None),
            dht_addr_asked: Arc::new(Mutex::new(HashMap::new())),
            dht_kick: Arc::new(tokio::sync::Notify::new()),
            send_kick: Arc::new(tokio::sync::Notify::new()),
            events: events_tx,
            tasks: Arc::new(std::sync::Mutex::new(Vec::new())),
        });
        this.ensure_prekeys().await?;
        this.clone().spawn_relay_loop();
        this.clone().spawn_collect_lanes();
        this.clone().spawn_send_loop();
        this.clone().spawn_bundle_refresh_loop();
        this.clone().spawn_dht_loop();
        this.warm_peer_relays();
        Ok((this, events_rx))
    }

    pub fn shutdown(&self) {
        let mut v = self.tasks.lock().unwrap();
        for h in v.drain(..) { h.abort(); }
    }

    pub fn my_card(&self) -> crate::crypto::IdentityCard { self.identity.card() }
    pub fn my_fingerprint(&self) -> [u8; 32] { self.identity.fingerprint() }

    /// Handler installed on an embedded relay so this session can also serve
    /// relay-network requests. The node itself is shared with all DHT fallback
    /// delivery and collection performed by this manager.
    pub fn dht_handler(&self) -> DhtHandler {
        dht_client::handler(&self.dht)
    }

    /// Relay-network nodes that have answered us so far.
    pub fn dht_peer_count(&self) -> usize {
        self.dht.peer_count()
    }

    /// The relay this process hosts for us. While `relay_onion` names it, we
    /// collect from it over a pipe instead of out through i2p and back.
    pub fn set_local_relay(&self, relay: Arc<crate::EphemeralRelay>) {
        *self.local_relay.lock().unwrap_or_else(|p| p.into_inner()) = Some(relay);
    }

    /// Announce an embedded relay as this node's reachable DHT endpoint.
    pub async fn join_dht(self: &Arc<Self>, address: &str) {
        dht_client::join(&self.dht, &self.db, &self.identity, address).await;
        self.publish_bundle_to_dht().await;
        self.collect_from_dht(DHT_COLLECT_DAYS_FIRST).await;
    }

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
        self.dht_kick.notify_one();
        Ok(())
    }

    pub async fn add_contact(self: &Arc<Self>, card: &crate::crypto::IdentityCard, onion: &str, name: &str) -> Result<i64> {
        self.add_contact_via(card, onion, name, None).await
    }

    /// Add a contact, recording the relay their card named.
    ///
    /// `relay` is where *they* receive: messages to this contact are deposited
    /// there, not on whatever relay this client happens to use. `None` keeps the
    /// old behaviour of falling back to this client's configured relay.
    pub async fn add_contact_via(
        self: &Arc<Self>, card: &crate::crypto::IdentityCard, onion: &str, name: &str, relay: Option<&str>,
    ) -> Result<i64> {
        let id = self.db.add_contact(&card.sign_pk, &card.dh_pk, onion, name, relay)?;
        let _ = self.events.send(SessionEvent::ContactAdded { contact_id: id }).await;
        self.dht_kick.notify_one();
        self.send_kick.notify_one();
        self.warm_relay_of(id);
        Ok(id)
    }

    /// Dial the relays of the most recent contacts now, in the background.
    ///
    /// Finding a relay's LeaseSet and opening the stream costs 5–30 s, and it
    /// was paid by the first letter to each contact, on the way out and again
    /// on the way back (e2e: 8 s and 32 s of a 64 s round trip). Dialled
    /// ahead, the letter finds the connection open. `relay_for` never blocks.
    fn warm_peer_relays(self: &Arc<Self>) {
        const WARM: usize = 8;
        let Ok(mut contacts) = self.db.list_contacts() else { return };
        contacts.retain(|c| {
            c.trust != TrustLevel::Blocked
                && c.request_state != crate::db::RequestState::Incoming
                && c.relay_address.as_deref().is_some_and(|r| !r.trim().is_empty())
        });
        contacts.sort_by_key(|c| std::cmp::Reverse(c.last_message_at));
        contacts.truncate(WARM);
        if contacts.is_empty() {
            return;
        }
        let this = self.clone();
        let handle = tokio::spawn(async move {
            for contact in &contacts {
                let _ = this.relay_for(contact).await;
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    /// As [`Self::warm_peer_relays`], for one contact whose relay just became
    /// known: added, or a letter or the network named a new one.
    fn warm_relay_of(self: &Arc<Self>, contact_id: i64) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            let Ok(Some(contact)) = this.db.get_contact(contact_id) else { return };
            if contact.trust == TrustLevel::Blocked || contact.request_state == crate::db::RequestState::Incoming {
                return;
            }
            let _ = this.relay_for(&contact).await;
        });
        self.tasks.lock().unwrap().push(handle);
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
        let (stored, parts) = self.store_outgoing(&attachments)?;
        let msg_id = self.db.insert_message(
            contact_id, Direction::Out, &body, sent_at, expires_at, &stored,
        )?;
        self.register_offers(msg_id, &parts, &[contact_id])?;
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
        let (stored, parts) = self.store_outgoing(&attachments)?;
        let msg_id = self.db.insert_message(
            contact_id, Direction::Out, &body, sent_at, None, &stored,
        )?;
        self.register_offers(msg_id, &parts, &[contact_id])?;
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
            relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None,
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
            relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None,
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
        let (stored, parts) = self.store_outgoing(&attachments)?;
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
            .filter(|(_, data)| data.len() <= crate::files::INLINE_MAX)
            .map(|(name, data)| WireAttachment { name, data }).collect();
        let members = self.db.list_group_members(group_id)?;
        let recipients: Vec<i64> = members.iter()
            .filter(|m| !m.is_self)
            .filter_map(|m| self.db.find_contact_by_identity(&m.dh_pk).ok().flatten())
            .filter(|c| c.trust != TrustLevel::Blocked)
            .map(|c| c.id)
            .collect();
        self.register_offers(msg_id, &parts, &recipients)?;
        let offers = self.offers_for(msg_id)?;
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
            self.db.pending_outbound_add(msg_id, contact.id)?;
            let mut payload = WirePayload::simple(
                msg_id as u64, body.clone(), wire_atts.clone(), sent_at, None,
            );
            payload.files = offers.clone();
            payload.group = Some(gref.clone());
            payload.buttons = buttons.clone();
            payload.notify_sound = notify_sound.clone();
            match self.send_to_contact(contact.id, &mut payload).await {
                Ok(()) => {
                    let _ = self.db.pending_outbound_remove(msg_id, contact.id);
                }
                Err(e) => eprintln!(
                    "[session] group send to contact {} failed (msg {}): {:?}; queued",
                    contact.id, msg_id, e
                ),
            }
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
            relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None,
            };
            let _ = self.send_to_contact(contact.id, &mut payload).await;
        }
        Ok(())
    }

    async fn send_to_contact(self: &Arc<Self>, contact_id: i64, payload: &mut WirePayload) -> Result<()> {
        let contact = self.db.get_contact(contact_id)?.ok_or(SessionError::NotFound)?;
        let route = self.route_for(&contact).await.ok_or(SessionError::State)?;
        self.ensure_session_for(&contact, &route).await?;
        self.send_payload_via_relay(&contact, payload, &route).await
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
            relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None,
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
                    // none set in Settings). Look again soon; the backoff is for
                    // failed dials, and growing it here made the first real one
                    // start late and its retry wait the full 15 s (e2e run
                    // 36042483601: 20 s from the relay being set to connected).
                    tokio::time::sleep(Duration::from_millis(RECONNECT_INITIAL_MS)).await;
                    continue;
                }
                eprintln!("[session] relay connect {}", &onion[..16.min(onion.len())]);
                let local = this.local_relay.lock().unwrap_or_else(|p| p.into_inner()).as_ref()
                    .filter(|r| r.address() == onion)
                    .map(|r| r.connect_local());
                // Bounded: nothing below has a timeout of its own, and a dial
                // i2p leaves hanging would otherwise keep us off our own mail
                // until restart (e2e run 35963073832: the master never got the
                // agent's GRANT).
                let connected = match tokio::time::timeout(PEER_RELAY_CONNECT_TIMEOUT, async {
                    match local {
                        Some(stream) => relay::connect_local(stream, &onion, &this.identity).await,
                        None => relay::connect(&this.node, &onion, &this.identity).await,
                    }
                }).await {
                    Ok(r) => r,
                    Err(_) => Err(relay::RelayError::Proto(format!("no answer in {PEER_RELAY_CONNECT_TIMEOUT:?}"))),
                };
                match connected {
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
                        this.clone().run_recv_loop(client, Some(onion.clone()), None).await;
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

    /// Keep extra connections to our own relay while file parts come, when it
    /// runs elsewhere (one in this process is a pipe, no faster for more).
    fn spawn_collect_lanes(self: Arc<Self>) {
        use std::sync::atomic::Ordering::Relaxed;
        let this = self.clone();
        let handle = tokio::spawn(async move {
            loop {
                let _ = tokio::time::timeout(Duration::from_secs(5), this.collect_kick.notified()).await;
                let onion = this.relay_onion();
                let external = !onion.is_empty()
                    && this.relay_out.read().await.is_some()
                    && !this.local_relay.lock().unwrap_or_else(|p| p.into_inner()).as_ref().is_some_and(|r| r.address() == onion);
                if !collect_lane_wanted(external, this.part_seen.load(Relaxed), now_ms(), this.collect_lanes.load(Relaxed)) {
                    continue;
                }
                // One dial at a time; the next tick dials the next.
                let dial = tokio::time::timeout(PEER_RELAY_CONNECT_TIMEOUT, relay::connect(&this.node, &onion, &this.identity)).await;
                let Ok(Ok(client)) = dial else { continue };
                let n = this.collect_lanes.fetch_add(1, Relaxed) + 1;
                eprintln!("[files] another connection to our relay for parts ({n} extra)");
                let lane = this.clone();
                tokio::spawn(async move {
                    // Ends when parts stop coming or our relay changes; what it was
                    // given and did not ack the relay deals to the others.
                    let quiet = {
                        let lane = lane.clone();
                        let onion = onion.clone();
                        move || !collect_lane_holds(lane.part_seen.load(Relaxed), now_ms()) || lane.relay_onion() != onion
                    };
                    lane.clone().run_recv_loop(client, Some(onion.clone()), Some(&quiet)).await;
                    lane.collect_lanes.fetch_sub(1, Relaxed);
                });
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
            let failures = {
                let mut pool = self.peer_relays.lock().await;
                match pool.get(theirs) {
                    Some(PeerRelay::Ready(tx)) if !tx.is_closed() => return Some(tx.clone()),
                    Some(PeerRelay::Ready(_)) | Some(PeerRelay::Connecting) => return None,
                    Some(PeerRelay::Failed { until, .. }) if std::time::Instant::now() < *until => return None,
                    _ => {}
                }
                let failures = match pool.get(theirs) {
                    Some(PeerRelay::Failed { failures, .. }) => *failures,
                    _ => 0,
                };
                pool.insert(theirs.to_string(), PeerRelay::Connecting);
                failures
            };

            let this = self.clone();
            let key = theirs.to_string();
            let (contact_id, contact_pk) = (contact.id, to_arr32(contact.identity_sign.clone()).ok());
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
                                until: std::time::Instant::now() + peer_relay_backoff(failures),
                                failures: failures + 1,
                            },
                        );
                        // Dialled again when the wait is over, letter or not: a
                        // first dial fails mostly because the relay's LeaseSet has
                        // not spread yet, and the first letter then waited for a
                        // dial of its own (e2e run 36042483601: 7 s of a 16 s echo).
                        if let Some(wait) = peer_relay_redial_after(failures) {
                            let this = this.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(wait).await;
                                if let Ok(Some(c)) = this.db.get_contact(contact_id) {
                                    let _ = this.relay_for(&c).await;
                                }
                            });
                        }
                        return;
                    }
                };
                eprintln!("[session] connected to peer relay {short}");
                this.peer_relays.lock().await
                    .insert(key.clone(), PeerRelay::Ready(client.out_tx.clone()));
                // No session with them yet: ask for their bundle now, so a
                // first letter does not wait a round trip for it.
                let no_session = !this.sessions.lock().await.contains_key(&contact_id)
                    && this.db.get_session(contact_id).ok().flatten().is_none();
                if let (true, Some(pk)) = (no_session, contact_pk) {
                    let _ = client.out_tx.send(ClientToRelay::GetBundle { pk }).await;
                }
                this.send_kick.notify_one();
                this.clone().run_recv_loop(client, None, None).await;
                this.peer_relays.lock().await.remove(&key);
            });
            self.tasks.lock().unwrap().push(handle);
            None
        })
    }

    async fn route_for(self: &Arc<Self>, contact: &Contact) -> Option<Route> {
        if let Some(tx) = self.relay_for(contact).await {
            return Some(Route::Relay(tx));
        }
        if self.peer_relay_settling(contact).await {
            return None;
        }
        (self.dht.peer_count() > 0).then_some(Route::Dht)
    }

    /// Their relay is being dialled (or its old connection is still closing):
    /// the letter waits in the queue for it rather than going to the network.
    /// Otherwise the first letters of a burst would take the slow network
    /// path while the later ones overtake them on the relay, and an agent
    /// runs commands in the order they arrive.
    async fn peer_relay_settling(&self, contact: &Contact) -> bool {
        let Some(theirs) = contact.relay_address.as_deref().map(str::trim).filter(|r| !r.is_empty()) else {
            return false;
        };
        matches!(
            self.peer_relays.lock().await.get(theirs),
            Some(PeerRelay::Connecting) | Some(PeerRelay::Ready(_))
        )
    }

    async fn deliver(
        &self,
        contact: &Contact,
        blob: Vec<u8>,
        route: &Route,
        first_letter: bool,
    ) -> Result<()> {
        let their_sign = to_arr32(contact.identity_sign.clone())?;
        let their_dh = to_arr32(contact.identity_dh.clone())?;
        match route {
            Route::Relay(out) => out
                .send(ClientToRelay::Send { to: their_sign, blob })
                .await
                .map_err(|_| SessionError::State),
            Route::Dht => {
                let stored = if first_letter {
                    dht_client::put_intro(&self.dht, &their_sign, &their_dh, &blob).await
                } else {
                    dht_client::put_mail(
                        &self.dht,
                        &self.identity,
                        &their_sign,
                        &their_dh,
                        &blob,
                    )
                    .await
                };
                if stored {
                    eprintln!("[dht/session] letter for contact {} left in the network", contact.id);
                    Ok(())
                } else {
                    Err(SessionError::State)
                }
            }
        }
    }

    /// `collecting_from` is set on the connection to our own relay: once the
    /// relay we collect from changes, this one is no longer where mail arrives.
    async fn run_recv_loop(self: Arc<Self>, client: RelayClient, collecting_from: Option<String>, done: Option<&(dyn Fn() -> bool + Send + Sync)>) {
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
                    // Checked between frames, never in the middle of one: a letter
                    // half-handled when the connection went could have moved the
                    // ratchet on and not be saved.
                    if done.is_some_and(|d| d()) { break; }
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
                let waiters = self.bundle_waiters.lock().await.remove(&pk);
                match waiters {
                    Some(vec) => {
                        for tx in vec { let _ = tx.send(bundle.clone()); }
                    }
                    // Nobody waiting: the one asked for ahead of use.
                    None => {
                        if let Some(b) = bundle {
                            self.bundle_cache.lock().await.insert(pk, (b, std::time::Instant::now()));
                        }
                    }
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
                let crossed = self.own_inits.lock().await.get(&contact.id).is_some()
                    && self.sessions.lock().await.contains_key(&contact.id);
                // A second init from them while their first lost to ours: they
                // never took ours (it did not reach them, or they started over
                // since — a reinstall, a resync), so this one is a new start.
                let crossed = crossed && !self.lost_inits.lock().await.contains_key(&contact.id);
                if crossed && ours_stands(&self.identity.card().sign_pk, &contact.identity_sign) {
                    // Both opened a session at once. Ours stands; theirs is
                    // read for what it says (name, relay) and set aside, and
                    // they will take ours when it reaches them.
                    eprintln!("[session] X3dhInit from contact {} crossed ours; ours stands", contact.id);
                    let (theirs, plaintext) = self.accept_x3dh(&init, &ad).await?;
                    self.lost_inits.lock().await.insert(contact.id, theirs);
                    let payload: WirePayload = decode_with_padding_fallback(&plaintext)?;
                    self.persist_incoming(contact.id, payload).await?;
                    if init.one_time_id.is_some() {
                        self.republish_bundle().await;
                    }
                    return Ok(());
                }
                if crossed {
                    // Theirs stands. What we sent on ours they read on it (they
                    // keep it for that); anything that did not make it goes
                    // again on the usual retry, so no resend of everything here.
                    eprintln!("[session] X3dhInit from contact {} crossed ours; theirs stands", contact.id);
                } else {
                    eprintln!("[session] received X3dhInit from contact {}, accepting", contact.id);
                }
                self.own_inits.lock().await.remove(&contact.id);
                self.lost_inits.lock().await.remove(&contact.id);
                self.sessions.lock().await.remove(&contact.id);
                let _ = self.db.delete_session(contact.id);
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
                        // On a session of theirs that lost to ours: sent before
                        // ours reached them. Read it on that session.
                        let lost_hit = {
                            let mut lost = self.lost_inits.lock().await;
                            let mut hit = None;
                            for (cid, state) in lost.iter_mut() {
                                let Ok(Some(c)) = self.db.get_contact(*cid) else { continue };
                                if !sealed && c.identity_sign.as_slice() != from_pk.as_slice() { continue; }
                                let ad = build_ad(&self.identity.card().dh_pk, &c.identity_dh);
                                if let Ok(pt) = state.decrypt(&header, &ciphertext, &ad) {
                                    hit = Some((*cid, pt));
                                    break;
                                }
                            }
                            hit
                        };
                        if let Some((cid, pt)) = lost_hit {
                            eprintln!("[session] letter from contact {cid} on its session that lost to ours; read on it");
                            let payload: WirePayload = decode_with_padding_fallback(&pt)?;
                            self.persist_incoming(cid, payload).await?;
                            return Ok(());
                        }
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
                // They answered on this session: it is settled, and an init
                // from them from now on is a new start, not a crossing.
                self.own_inits.lock().await.remove(&cid);
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
        self.heard.lock().unwrap_or_else(|p| p.into_inner()).insert(contact_id, now_ms());
        // Files in parts: these letters are nothing else.
        if let Some(ack) = &payload.file_ack {
            self.on_file_ack(contact_id, ack).await?;
        }
        if let Some(chunk) = &payload.file_chunk {
            self.on_file_chunk(contact_id, chunk).await?;
        }
        if let Some(file_id) = &payload.file_cancel {
            self.on_file_cancel(contact_id, file_id).await?;
        }
        // As in the app: a contact who deleted us asks for the chat to go.
        if payload.wipe == Some(true) {
            eprintln!("[wipe] contact {contact_id} deleted us and asked for the chat to go; deleting it");
            self.db.delete_contact(contact_id)?;
            self.sessions.lock().await.remove(&contact_id);
            self.session_created_at.lock().await.remove(&contact_id);
            self.own_inits.lock().await.remove(&contact_id);
            self.lost_inits.lock().await.remove(&contact_id);
            self.incoming_since_send.lock().await.remove(&contact_id);
            let _ = self.events.send(SessionEvent::ContactWiped { contact_id }).await;
            return Ok(());
        }
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
                        self.warm_relay_of(contact_id);
                    }
                }
            }
        }
        let is_empty = payload.body.is_empty()
            && payload.attachments.is_empty()
            && payload.files.is_empty()
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
            atts.push(NewAttachment { name: a.name.clone(), size: size as i64, key: key.to_vec(), path, chunk_size: None });
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
        if !payload.files.is_empty() {
            self.accept_offers(contact_id, mid, &payload.files).await?;
        }
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
            relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None,
        };
        let Some(route) = self.route_for(contact).await else { return Ok(()) };
        if self.ensure_session_for(contact, &route).await.is_err() {
            return Ok(());
        }
        let _ = self.send_payload_via_relay(contact, &mut payload, &route).await;
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
        let contacts = self.db.list_contacts()?;
        let groups_by_id: HashMap<Vec<u8>, String> = self
            .db
            .list_groups()?
            .into_iter()
            .map(|group| (group.id, group.name))
            .collect();
        let members_by_group: HashMap<Vec<u8>, Vec<GroupMember>> =
            self.db.list_all_group_members()?;
        for contact in contacts {
            if contact.trust == TrustLevel::Blocked { continue; }
            let pending = self.db.list_unsent_outgoing(contact.id, 50)?;
            let unacked = self.db.list_unacked_outgoing(
                contact.id, now_ms(), RETRY_BASE_BACKOFF_MS, RETRY_MAX_BACKOFF_MS, 50,
            )?;
            let needs_session = self.db.resync_recent(contact.id, 120_000).unwrap_or(false)
                && !self.sessions.lock().await.contains_key(&contact.id);
            let needs_keepalive = self.incoming_since_send.lock().await.get(&contact.id).copied().unwrap_or(0) >= KEEPALIVE_INCOMING_THRESHOLD
                && self.sessions.lock().await.contains_key(&contact.id);
            let group_pending = self.db.pending_outbound_for_recipient(
                contact.id,
                now_ms(),
                RETRY_BASE_BACKOFF_MS,
                RETRY_MAX_BACKOFF_MS,
                50,
            )?;
            let file_work = !self.db.file_peers_for(contact.id)?.is_empty();
            if pending.is_empty()
                && unacked.is_empty()
                && group_pending.is_empty()
                && !needs_session
                && !needs_keepalive
                && !file_work
            {
                continue;
            }
            let route = match self.relay_for(&contact).await {
                Some(out) => Route::Relay(out),
                None if self.peer_relay_settling(&contact).await => continue,
                None if self.dht.peer_count() > 0 => {
                    self.maybe_look_up_address(&contact).await;
                    Route::Dht
                }
                None => continue,
            };
            if self.ensure_session_for(&contact, &route).await.is_err() { continue; }
            if needs_keepalive && pending.is_empty() && unacked.is_empty() {
                let mut payload = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
                payload.ack_for = Some(0);
                if let Err(e) = self.send_payload_via_relay(&contact, &mut payload, &route).await {
                    eprintln!("[session] keepalive err to contact {}: {:?}", contact.id, e);
                } else {
                    eprintln!("[session] keepalive sent to contact {} (DH-roll forced)", contact.id);
                }
            }
            for msg in pending {
                let mut payload = self.build_payload_from_db(&msg)?;
                if let Err(e) = self.send_payload_via_relay(&contact, &mut payload, &route).await {
                    eprintln!("[session] send err contact {}: {:?}", contact.id, e);
                    break;
                }
            }
            for msg in unacked {
                let mut payload = self.build_payload_from_db(&msg)?;
                eprintln!("[session] retry unacked msg {} to contact {} (attempt {})",
                    msg.id, contact.id, msg.send_attempts + 1);
                self.db.record_send_attempt(msg.id)?;
                if let Err(e) = self.send_payload_via_relay(&contact, &mut payload, &route).await {
                    eprintln!("[session] retry err contact {}: {:?}", contact.id, e);
                    break;
                }
            }
            // Parts only to their relay: the relay network holds too little
            // per key for a file (dht/src/store.rs); they wait for it.
            if file_work && matches!(route, Route::Relay(_)) {
                if let Err(e) = self.send_file_parts(&contact, &route).await {
                    eprintln!("[files] parts to contact {}: {:?}", contact.id, e);
                }
            }
            for msg_id in group_pending {
                let msg = match self.db.get_message(msg_id)? {
                    Some(msg) => msg,
                    None => {
                        let _ = self.db.pending_outbound_remove(msg_id, contact.id);
                        continue;
                    }
                };
                let mut payload = self.build_payload_from_db(&msg)?;
                if let Some(group_id) = &msg.group_id {
                    let name = groups_by_id.get(group_id).cloned().unwrap_or_default();
                    let members = members_by_group
                        .get(group_id)
                        .map(|members| {
                            members
                                .iter()
                                .map(|member| WireMember {
                                    sign_pk: member.sign_pk.clone(),
                                    dh_pk: member.dh_pk.clone(),
                                    onion: member.onion.clone(),
                                    name: member.display_name.clone(),
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    payload.group = Some(WireGroupRef {
                        id: group_id.clone(),
                        name,
                        members,
                    });
                }
                self.db.pending_outbound_record_attempt(msg_id, contact.id)?;
                match self.send_payload_via_relay(&contact, &mut payload, &route).await {
                    Ok(()) => {
                        let _ = self.db.pending_outbound_remove(msg_id, contact.id);
                    }
                    Err(e) => {
                        eprintln!(
                            "[session] retry group msg {} to contact {} failed: {:?}",
                            msg_id, contact.id, e
                        );
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    async fn ensure_session_for(
        &self,
        contact: &Contact,
        route: &Route,
    ) -> Result<()> {
        if self.sessions.lock().await.contains_key(&contact.id) { return Ok(()); }
        if let Some(blob) = self.db.get_session(contact.id)? {
            self.sessions.lock().await.insert(contact.id, RatchetState::from_bytes(&blob)?);
            return Ok(());
        }
        // No waiting for the other side to go first (that cost every new
        // conversation 10 s): if both open at once, `ours_stands` picks one.

        let mut pk = [0u8; 32];
        pk.copy_from_slice(&contact.identity_sign);
        let prefetched = {
            let mut cache = self.bundle_cache.lock().await;
            cache.remove(&pk).filter(|(_, at)| at.elapsed() < BUNDLE_PREFETCH_TTL).map(|(b, _)| b)
        };
        let bundle_bytes = match route {
            _ if prefetched.is_some() => prefetched,
            Route::Relay(out) => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                self.bundle_waiters.lock().await.entry(pk).or_default().push(tx);
                out.send(ClientToRelay::GetBundle { pk }).await.map_err(|_| SessionError::State)?;
                tokio::time::timeout(Duration::from_millis(PENDING_REQ_TIMEOUT_MS), rx).await
                    .map_err(|_| SessionError::State)?
                    .map_err(|_| SessionError::State)?
            }
            Route::Dht => {
                let their_dh = to_arr32(contact.identity_dh.clone())?;
                dht_client::find_bundle(&self.dht, &pk, &their_dh).await
            }
        };
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
        self.own_inits.lock().await.insert(contact.id, init.header.dh);
        self.lost_inits.lock().await.remove(&contact.id);
        self.db.put_session(contact.id, &state.to_bytes()?)?;
        self.sessions.lock().await.insert(contact.id, state);
        self.session_created_at.lock().await.insert(contact.id, now_ms());
        let blob = bincode::serialize(&EnvelopeBlob::X3dhInit(init))?;
        self.deliver(contact, blob, route, true).await?;
        eprintln!("[session] x3dh sent to contact {}", contact.id);
        Ok(())
    }

    async fn send_payload_via_relay(
        &self,
        contact: &Contact,
        payload: &mut WirePayload,
        route: &Route,
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
                let _ = self.events.send(SessionEvent::MessageFailed { message_id: payload.origin_msg_id as i64, reason: "too large for one letter".into() }).await;
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
        self.deliver(contact, blob, route, false).await?;
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
            // Sealed in parts: offered below and sent from file_out.
            if a.chunk_size.is_some() { continue; }
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
        p.files = self.offers_for(msg.id)?;
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
                this.gc_files();
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    fn spawn_dht_loop(self: Arc<Self>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            // Bootstrap immediately. Otherwise a headless client configured
            // with an external relay would wait 45 minutes before learning
            // that the relay network exists.
            dht_client::maintain(
                &this.dht,
                &this.db,
                &this.identity,
                nonempty(this.relay_onion()).as_deref(),
            )
            .await;
            this.publish_bundle_to_dht().await;
            this.collect_from_dht(DHT_COLLECT_DAYS_FIRST).await;

            let mut maintain = tokio::time::interval(dht_client::MAINTAIN_EVERY);
            maintain.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            maintain.tick().await;
            let mut collect = tokio::time::interval(DHT_COLLECT_EVERY);
            collect.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            collect.tick().await;
            let mut days = DHT_COLLECT_DAYS;
            loop {
                tokio::select! {
                    _ = this.dht_kick.notified() => {
                        let address = nonempty(this.relay_onion());
                        dht_client::maintain(
                            &this.dht,
                            &this.db,
                            &this.identity,
                            address.as_deref(),
                        ).await;
                        this.publish_bundle_to_dht().await;
                        this.collect_from_dht(DHT_COLLECT_DAYS_FIRST).await;
                        this.send_kick.notify_one();
                    }
                    _ = maintain.tick() => {
                        let address = nonempty(this.relay_onion());
                        dht_client::maintain(
                            &this.dht,
                            &this.db,
                            &this.identity,
                            address.as_deref(),
                        ).await;
                        this.publish_bundle_to_dht().await;
                        let _ = this.db.dht_seen_purge(now_ms() - DHT_SEEN_TTL_MS);
                    }
                    _ = collect.tick() => {
                        this.collect_from_dht(days).await;
                        days = DHT_COLLECT_DAYS;
                    }
                }
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    async fn collect_from_dht(self: &Arc<Self>, days: u32) {
        if self.dht.peer_count() == 0 {
            return;
        }
        let Ok(contacts) = self.db.list_contacts() else { return };
        // Introductions first: the letters written after one open only with the
        // session it starts (e2e-dht run 35950928731 lost all three otherwise).
        let mut letters = dht_client::collect_intros(&self.dht, &self.identity, days).await;
        for contact in contacts {
            if contact.trust == TrustLevel::Blocked {
                continue;
            }
            let (Ok(their_sign), Ok(their_dh)) = (
                to_arr32(contact.identity_sign),
                to_arr32(contact.identity_dh),
            ) else {
                continue;
            };
            letters.extend(
                dht_client::collect_mail(
                    &self.dht,
                    &self.identity,
                    &their_sign,
                    &their_dh,
                    days,
                )
                .await,
            );
        }

        for letter in letters {
            let hash = dht_client::letter_hash(&letter.envelope);
            match self.db.dht_seen_mark(&hash, now_ms()) {
                Ok(true) => {}
                _ => continue,
            }
            match self
                .handle_incoming_envelope(&[0u8; 32], &letter.envelope)
                .await
            {
                Ok(()) | Err(SessionError::StaleOpk) => {
                    dht_client::drop_letter(&self.dht, &letter).await;
                }
                // No session opens it yet. Over a relay the sender resends what
                // is not acked; here the sender is away, so the letter stays in
                // the network for a later pass, when the session may exist.
                Err(SessionError::SealedDrop) => {
                    let _ = self.db.dht_seen_forget(&hash);
                }
                Err(e) => {
                    eprintln!("[dht/session] letter did not open: {e:?}");
                    let _ = self.db.dht_seen_forget(&hash);
                }
            }
        }
        self.send_kick.notify_one();
    }

    async fn maybe_look_up_address(self: &Arc<Self>, contact: &Contact) {
        if self.dht.peer_count() == 0 {
            return;
        }
        {
            let mut asked = self.dht_addr_asked.lock().await;
            let now = Instant::now();
            if asked
                .get(&contact.id)
                .is_some_and(|then| now.duration_since(*then) < DHT_ADDRESS_LOOKUP_EVERY)
            {
                return;
            }
            asked.insert(contact.id, now);
        }

        let (Ok(their_sign), Ok(their_dh)) = (
            to_arr32(contact.identity_sign.clone()),
            to_arr32(contact.identity_dh.clone()),
        ) else {
            return;
        };
        let this = self.clone();
        let id = contact.id;
        let known = contact.relay_address.clone();
        let handle = tokio::spawn(async move {
            let Some((relay, _)) = dht_client::find_address(
                &this.dht,
                &this.identity,
                &their_sign,
                &their_dh,
            )
            .await
            else {
                return;
            };
            if relay.trim().is_empty() || known.as_deref() == Some(relay.as_str()) {
                return;
            }
            eprintln!("[dht/session] contact {id} published a new relay address");
            if this.db.set_contact_relay(id, Some(&relay)).is_ok() {
                if let Some(old) = known.as_deref() {
                    this.peer_relays.lock().await.remove(old);
                }
                this.warm_relay_of(id);
                this.send_kick.notify_one();
                let _ = this.events.send(SessionEvent::ContactUpdated { contact_id: id }).await;
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    /// Put our prekey bundle into the relay network, so a contact can open a
    /// session while our relay is away. True once some node holds it.
    pub async fn publish_bundle_to_dht(&self) -> bool {
        if self.dht.peer_count() == 0 {
            return false;
        }
        let Ok(bundle) = self.my_bundle() else { return false };
        let Ok(bytes) = bincode::serialize(&bundle) else { return false };
        dht_client::publish_bundle(&self.dht, &self.identity, &bytes).await
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
        self.publish_bundle_to_dht().await;
    }
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
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

/// Files sent in parts (`crate::files`, docs/plans/2026-09-24-chunked-files.md).
impl SessionManager {
    /// Seal outgoing attachments: small ones whole (they ride inside the
    /// letter), larger ones in parts. Returns what to store and, for each
    /// sealed in parts, its sha256 in order.
    fn store_outgoing(&self, attachments: &[(String, Vec<u8>)]) -> Result<(Vec<NewAttachment>, Vec<[u8; 32]>)> {
        let mut stored = Vec::with_capacity(attachments.len());
        let mut parts = Vec::new();
        for (name, data) in attachments {
            if data.len() > crate::files::INLINE_MAX {
                let (key, path, size, sha) = store_attachment_parts(&self.data_dir, data)?;
                stored.push(NewAttachment {
                    name: name.clone(), size: size as i64, key: key.to_vec(), path,
                    chunk_size: Some(crate::files::CHUNK_SIZE as i64),
                });
                parts.push(sha);
            } else {
                let (key, path, size) = store_attachment(&self.data_dir, data)?;
                stored.push(NewAttachment { name: name.clone(), size: size as i64, key: key.to_vec(), path, chunk_size: None });
            }
        }
        Ok((stored, parts))
    }

    /// Offer the message's attachments sealed in parts to `recipients`.
    fn register_offers(&self, msg_id: i64, shas: &[[u8; 32]], recipients: &[i64]) -> Result<()> {
        let parted = self.db.list_attachments(msg_id)?.into_iter().filter(|a| a.chunk_size.is_some());
        for (a, sha) in parted.zip(shas) {
            let file_id: [u8; 16] = crypto::random_array();
            self.db.file_out_add(&file_id, a.id, msg_id, sha, recipients)?;
        }
        Ok(())
    }

    fn offers_for(&self, msg_id: i64) -> Result<Vec<WireFileOffer>> {
        Ok(self.db.files_out_for_message(msg_id)?.into_iter()
            .filter(|f| !f.cancelled)
            .map(|f| WireFileOffer {
                file_id: f.file_id,
                name: f.attachment.name,
                size: f.attachment.size as u64,
                sha256: f.sha256,
                chunk_size: f.attachment.chunk_size.unwrap_or(crate::files::CHUNK_SIZE as i64) as u32,
            })
            .collect())
    }

    /// A letter offering files: start taking their parts.
    async fn accept_offers(self: &Arc<Self>, contact_id: i64, message_id: i64, offers: &[WireFileOffer]) -> Result<()> {
        for o in offers {
            if o.size > crate::files::MAX_FILE_BYTES || o.chunk_size == 0 || o.chunk_size > crate::files::CHUNK_SIZE {
                eprintln!("[files] offer from contact {contact_id} refused: {} bytes in parts of {}", o.size, o.chunk_size);
                continue;
            }
            if self.db.file_in(&o.file_id, contact_id)?.is_some() {
                continue;
            }
            let cipher = AttachmentCipher::generate();
            let mut name = [0u8; 24];
            crypto::fill_random(&mut name);
            let total = crate::files::chunk_count(o.size, o.chunk_size);
            let fin = crate::db::FileIn {
                file_id: o.file_id, contact_id, message_id, name: o.name.clone(), size: o.size,
                sha256: o.sha256, chunk_size: o.chunk_size, path: to_hex(&name), key: cipher.key().to_vec(),
                bits: crate::files::Received::new(total).bits().to_vec(), received: 0, created_at: now_ms(),
            };
            if !self.db.file_in_add(&fin)? {
                continue;
            }
            eprintln!("[files] contact {contact_id} offers {} ({} bytes, {total} parts)", o.name, o.size);
            let _ = self.events.send(SessionEvent::FileProgress { message_id, contact_id, incoming: true, done: 0, total }).await;
            // Parts that came before this letter.
            let early = self.db.file_early_take(&o.file_id, contact_id)?;
            for (index, data) in early {
                let chunk = WireFileChunk { file_id: o.file_id, index, data };
                self.on_file_chunk(contact_id, &chunk).await?;
            }
        }
        Ok(())
    }

    async fn on_file_chunk(self: &Arc<Self>, contact_id: i64, chunk: &WireFileChunk) -> Result<()> {
        let before = self.part_seen.swap(now_ms(), std::sync::atomic::Ordering::Relaxed);
        if !collect_lane_holds(before, now_ms()) {
            self.collect_kick.notify_one();
        }
        let Some(fin) = self.db.file_in(&chunk.file_id, contact_id)? else {
            // Before its offer (resends reorder letters): set aside, a window's
            // worth at most.
            if chunk.data.len() <= crate::files::CHUNK_SIZE as usize {
                self.db.file_early_put(&chunk.file_id, contact_id, chunk.index, &chunk.data)?;
            }
            return Ok(());
        };
        let total = crate::files::chunk_count(fin.size, fin.chunk_size);
        let mut got = crate::files::Received::from_bits(fin.bits.clone(), total);
        let path = self.data_dir.join(ATTACHMENTS_DIR).join(&fin.path);
        let cipher = AttachmentCipher::from_key(to_arr32(fin.key.clone())?);
        let fresh = if got.has(chunk.index) {
            false
        } else {
            crate::files::write_part(&path, &cipher, fin.size, fin.chunk_size, chunk.index, &chunk.data)?;
            got.mark(chunk.index);
            self.db.file_in_save(&fin.file_id, contact_id, got.bits(), got.count())?;
            true
        };
        if got.complete() {
            let sha = crate::files::open_to(&path, &cipher, fin.size, fin.chunk_size, &mut std::io::sink())?;
            if sha != fin.sha256 {
                eprintln!("[files] {} from contact {contact_id} came broken (sha256); dropped", fin.name);
                let _ = std::fs::remove_file(&path);
                self.db.file_in_delete(&fin.file_id, contact_id)?;
                let _ = self.events.send(SessionEvent::FileFailed { message_id: fin.message_id, contact_id, reason: "came broken".into() }).await;
            } else {
                let attachment_id = self.db.file_in_finish(&fin)?;
                eprintln!("[files] {} from contact {contact_id} is whole ({} bytes)", fin.name, fin.size);
                let _ = self.events.send(SessionEvent::FileReceived { message_id: fin.message_id, attachment_id }).await;
            }
        } else if fresh {
            let _ = self.events.send(SessionEvent::FileProgress {
                message_id: fin.message_id, contact_id, incoming: true, done: got.count(), total,
            }).await;
        }
        // A resend means they did not hear our last ack; the whole file, a
        // hole, or every few parts: say where we are.
        let hole = !got.missing().is_empty();
        if !fresh || got.complete() || hole || got.count() % crate::files::ACK_EVERY == 0 {
            self.send_file_message(contact_id, |p| p.file_ack = Some(got.ack(fin.file_id))).await;
        }
        Ok(())
    }

    async fn on_file_ack(self: &Arc<Self>, contact_id: i64, ack: &WireFileAck) -> Result<()> {
        let Some(mut peer) = self.db.file_peer(&ack.file_id, contact_id)? else { return Ok(()) };
        let Some(file) = self.db.file_out(&ack.file_id)? else { return Ok(()) };
        let total = crate::files::chunk_count(file.attachment.size as u64, file.attachment.chunk_size.unwrap_or(1) as u32);
        let mut s = crate::files::Sending { next: peer.next, acked: peer.acked, resend: peer.resend.clone() };
        s.on_ack(ack);
        peer.acked = s.acked;
        peer.resend = s.resend.clone();
        peer.last_ack_at = Some(now_ms());
        self.db.file_peer_save(&peer)?;
        let _ = self.events.send(SessionEvent::FileProgress {
            message_id: file.message_id, contact_id, incoming: false, done: peer.acked, total,
        }).await;
        if s.done(total) {
            eprintln!("[files] {} is all at contact {contact_id}", file.attachment.name);
        }
        self.send_kick.notify_one();
        Ok(())
    }

    async fn on_file_cancel(&self, contact_id: i64, file_id: &[u8; 16]) -> Result<()> {
        // They stopped sending it to us: forget what we had.
        if let Some(fin) = self.db.file_in(file_id, contact_id)? {
            let _ = std::fs::remove_file(self.data_dir.join(ATTACHMENTS_DIR).join(&fin.path));
            self.db.file_in_delete(file_id, contact_id)?;
            let _ = self.events.send(SessionEvent::FileFailed { message_id: fin.message_id, contact_id, reason: "cancelled".into() }).await;
        }
        // Or they refused ours.
        if let Some(mut peer) = self.db.file_peer(file_id, contact_id)? {
            peer.failed = true;
            self.db.file_peer_save(&peer)?;
        }
        Ok(())
    }

    /// An attachment's bytes, whichever way it was sealed.
    pub fn read_attachment(&self, att: &crate::db::Attachment) -> Result<Vec<u8>> {
        let path = self.data_dir.join(ATTACHMENTS_DIR).join(&att.path);
        Ok(crate::files::read_attachment(&path, to_arr32(att.key.clone())?, att.size as u64, att.chunk_size)?)
    }

    /// Stop sending a file of ours, to everyone, and tell them.
    pub async fn cancel_file(self: &Arc<Self>, file_id: [u8; 16]) -> Result<()> {
        self.db.file_out_cancel(&file_id)?;
        for peer in self.db.file_peers_of(&file_id)? {
            self.send_file_message(peer.contact_id, |p| p.file_cancel = Some(file_id)).await;
        }
        Ok(())
    }

    /// A letter about files (an ack, a cancel): no text, not a message, not
    /// retried — the next part or ack supersedes it.
    async fn send_file_message(self: &Arc<Self>, contact_id: i64, fill: impl FnOnce(&mut WirePayload)) {
        let Ok(Some(contact)) = self.db.get_contact(contact_id) else { return };
        let mut payload = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
        fill(&mut payload);
        let Some(route) = self.route_for(&contact).await else { return };
        if self.ensure_session_for(&contact, &route).await.is_err() {
            return;
        }
        let _ = self.send_payload_via_relay(&contact, &mut payload, &route).await;
    }

    /// Connections to carry parts to this contact's relay: the usual one and
    /// up to `FILE_LANES - 1` more, dialled in the background as needed.
    async fn file_lanes(self: &Arc<Self>, contact: &Contact, main: &mpsc::Sender<ClientToRelay>) -> Vec<mpsc::Sender<ClientToRelay>> {
        let mut out = vec![main.clone()];
        let Some(relay) = contact.relay_address.as_deref().map(str::trim).filter(|r| !r.is_empty()) else { return out };
        let relay = relay.to_string();
        let live = {
            let mut lanes = self.file_lanes.lock().await;
            let v = lanes.entry(relay.clone()).or_default();
            v.retain(|tx| !tx.is_closed());
            v.clone()
        };
        let want_more = live.len() + 1 < FILE_LANES;
        out.extend(live);
        let dialling = !self.file_lanes_dialling.lock().unwrap_or_else(|p| p.into_inner()).insert(relay.clone());
        if want_more && !dialling {
            let this = self.clone();
            let handle = tokio::spawn(async move {
                let dial = tokio::time::timeout(
                    PEER_RELAY_CONNECT_TIMEOUT,
                    crate::relay::connect_peer(&this.node, &relay, &this.identity),
                ).await;
                if let Ok(Ok(client)) = dial {
                    eprintln!("[files] another lane to relay {}", &relay[..16.min(relay.len())]);
                    this.file_lanes.lock().await.entry(relay.clone()).or_default().push(client.out_tx.clone());
                    this.file_lanes_dialling.lock().unwrap_or_else(|p| p.into_inner()).remove(&relay);
                    this.send_kick.notify_one();
                    // Drained like any peer relay connection; it ends with the lane.
                    this.clone().run_recv_loop(client, None, None).await;
                } else {
                    this.file_lanes_dialling.lock().unwrap_or_else(|p| p.into_inner()).remove(&relay);
                }
            });
            self.tasks.lock().unwrap().push(handle);
        } else if !want_more && !dialling {
            self.file_lanes_dialling.lock().unwrap_or_else(|p| p.into_inner()).remove(&relay);
        }
        out
    }

    /// Send what is due of every file still going to this contact.
    async fn send_file_parts(self: &Arc<Self>, contact: &Contact, route: &Route) -> Result<()> {
        let now = now_ms();
        let Route::Relay(main) = route else { return Ok(()) };
        let lanes = self.file_lanes(contact, main).await;
        let mut lane = 0usize;
        let heard = self.heard.lock().unwrap_or_else(|p| p.into_inner()).get(&contact.id).copied();
        for mut peer in self.db.file_peers_for(contact.id)? {
            let Some(file) = self.db.file_out(&peer.file_id)? else { continue };
            let a = &file.attachment;
            let chunk_size = a.chunk_size.unwrap_or(crate::files::CHUNK_SIZE as i64) as u32;
            let total = crate::files::chunk_count(a.size as u64, chunk_size);
            let mut s = crate::files::Sending { next: peer.next, acked: peer.acked, resend: peer.resend.clone() };
            if let Some(sent) = peer.last_sent_at {
                let alive_since = heard.is_some_and(|h| h > sent);
                let no_ack_since = peer.last_ack_at.is_none_or(|a| a < sent);
                // Never a single ack from someone alive for a day: a build
                // without files in parts. Say so, rather than try for a week.
                if peer.last_ack_at.is_none() && alive_since && now - sent > FILE_GIVE_UP_MS {
                    peer.failed = true;
                    self.db.file_peer_save(&peer)?;
                    let _ = self.events.send(SessionEvent::FileFailed {
                        message_id: file.message_id, contact_id: contact.id, reason: "not taken".into(),
                    }).await;
                    continue;
                }
                // Alive, and not a word about the parts for long: they are
                // gone somewhere. Send what is outstanding again — rarely,
                // since an absent recipient cannot be told from a lost part.
                if s.resend.is_empty() && s.next > s.acked && alive_since && no_ack_since && now - sent > FILE_RESEND_IDLE_MS {
                    s.rewind();
                }
            }
            let due = s.due(total);
            if due.is_empty() {
                continue;
            }
            let path = self.data_dir.join(ATTACHMENTS_DIR).join(&a.path);
            let cipher = AttachmentCipher::from_key(to_arr32(a.key.clone())?);
            let mut sent_any = false;
            for index in &due {
                let data = crate::files::read_part(&path, &cipher, a.size as u64, chunk_size, *index)?;
                let mut payload = WirePayload::simple(0, String::new(), Vec::new(), now, None);
                payload.file_chunk = Some(WireFileChunk { file_id: file.file_id, index: *index, data });
                // Round the lanes: each is its own i2p stream.
                let via = Route::Relay(lanes[lane % lanes.len()].clone());
                lane += 1;
                if let Err(e) = self.send_payload_via_relay(contact, &mut payload, &via).await {
                    eprintln!("[files] part {index} of {} to contact {}: {e:?}", a.name, contact.id);
                    // What did not go goes again next time.
                    s.resend.extend(due.iter().copied().filter(|i| i >= index));
                    break;
                }
                sent_any = true;
            }
            peer.next = s.next;
            peer.resend = s.resend;
            if sent_any {
                peer.last_sent_at = Some(now);
            }
            self.db.file_peer_save(&peer)?;
        }
        Ok(())
    }

    /// Unfinished transfers after the relay would have dropped their parts.
    fn gc_files(&self) {
        let cutoff = now_ms() - FILE_IN_TTL_MS;
        if let Ok(stale) = self.db.files_in_stale(cutoff) {
            for f in stale {
                let _ = std::fs::remove_file(self.data_dir.join(ATTACHMENTS_DIR).join(&f.path));
                let _ = self.db.file_in_delete(&f.file_id, f.contact_id);
            }
        }
        let _ = self.db.file_early_gc(cutoff);
    }
}

/// Seal an attachment in parts (`crate::files`): key, file name, size, sha256.
fn store_attachment_parts(data_dir: &PathBuf, data: &[u8]) -> Result<([u8; 32], String, u64, [u8; 32])> {
    let cipher = AttachmentCipher::generate();
    let mut name = [0u8; 24];
    crate::crypto::fill_random(&mut name);
    let hex = to_hex(&name);
    let path = data_dir.join(ATTACHMENTS_DIR).join(&hex);
    let (size, sha) = crate::files::seal_from(data, &path, &cipher, crate::files::CHUNK_SIZE)?;
    Ok((*cipher.key(), hex, size, sha))
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

/// When two X3dhInits cross, the session opened by the side with the lower
/// signing key stands — the same answer on both ends, with no waiting. The
/// app (Core) decides by this too.
pub fn ours_stands(my_sign: &[u8], their_sign: &[u8]) -> bool {
    my_sign < their_sign
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
    fn wipe_roundtrips_and_leaves_other_payloads_as_they_were() {
        let mut p = sample();
        p.relay_address = Some("relay".into());
        // Without wipe: exactly the 0.4.13 bytes.
        assert_eq!(encode_payload(&p).unwrap(), bincode::serialize(&WireV8::from(&p)).unwrap());
        p.wipe = Some(true);
        let bytes = encode_payload(&p).unwrap();
        assert_eq!(decode_payload(&bytes).unwrap().wipe, Some(true));
        // A 0.4.13 client reads its own newest shape and ignores the rest.
        let old: WireV8 = bincode::deserialize(&bytes).expect("trailing field tolerated");
        assert_eq!(old.relay_address.as_deref(), Some("relay"));
    }

    #[test]
    fn files_roundtrip_and_leave_other_payloads_as_they_were() {
        let mut p = sample();
        p.wipe = Some(true);
        // No file fields: exactly the bytes of the build before them.
        assert_eq!(encode_payload(&p).unwrap(), bincode::serialize(&WireV9::from(&p)).unwrap());
        p.wipe = None;
        p.files = vec![WireFileOffer { file_id: [7; 16], name: "big.bin".into(), size: 5 << 20, sha256: [9; 32], chunk_size: 196_608 }];
        let back = decode_payload(&encode_payload(&p).unwrap()).unwrap();
        assert_eq!(back.files, p.files);
        assert_eq!(back.body, p.body);

        let mut c = WirePayload::simple(0, String::new(), Vec::new(), 1, None);
        c.file_chunk = Some(WireFileChunk { file_id: [7; 16], index: 3, data: vec![1, 2, 3] });
        let back = decode_payload(&encode_payload(&c).unwrap()).unwrap();
        assert_eq!(back.file_chunk, c.file_chunk);

        let mut a = WirePayload::simple(0, String::new(), Vec::new(), 1, None);
        a.file_ack = Some(WireFileAck { file_id: [7; 16], received_up_to: 4, missing: vec![6] });
        a.file_cancel = Some([8; 16]);
        let back = decode_payload(&encode_payload(&a).unwrap()).unwrap();
        assert_eq!(back.file_ack, a.file_ack);
        assert_eq!(back.file_cancel, a.file_cancel);
    }

    #[test]
    fn a_released_client_sees_a_part_as_an_empty_letter() {
        // 0.4.11–0.4.13 read their newest shape (V8) and ignore the tail: a
        // part is then a letter with no text, which persist_incoming drops.
        let mut c = WirePayload::simple(0, String::new(), Vec::new(), 1, None);
        c.file_chunk = Some(WireFileChunk { file_id: [7; 16], index: 0, data: vec![0; 1000] });
        let old: WireV8 = bincode::deserialize(&encode_payload(&c).unwrap()).expect("trailing fields tolerated");
        assert!(old.body.is_empty() && old.attachments.is_empty() && old.ack_for.is_none());
        // And an offer shows its text.
        let mut p = sample();
        p.files = vec![WireFileOffer { file_id: [1; 16], name: "a".into(), size: 1, sha256: [0; 32], chunk_size: 1 }];
        let old: WireV8 = bincode::deserialize(&encode_payload(&p).unwrap()).expect("trailing fields tolerated");
        assert_eq!(old.body, p.body);
    }

    #[test]
    fn crossing_inits_pick_the_same_session_on_both_ends() {
        let (a, b) = ([1u8; 32], [2u8; 32]);
        assert!(ours_stands(&a, &b));
        assert!(!ours_stands(&b, &a), "exactly one side keeps its own");
    }

    #[test]
    fn a_failed_relay_dial_is_retried_soon_then_less_often() {
        let secs: Vec<u64> = (0..8).map(|n| peer_relay_backoff(n).as_secs()).collect();
        assert_eq!(secs, vec![5, 10, 20, 40, 80, 120, 120, 120]);
    }

    #[test]
    fn an_unreachable_relay_is_dialled_again_for_a_few_minutes_then_left() {
        let waits: Vec<Option<u64>> = (0..7).map(|n| peer_relay_redial_after(n).map(|d| d.as_secs())).collect();
        assert_eq!(waits, vec![Some(5), Some(10), Some(20), Some(40), Some(80), None, None]);
    }

    #[test]
    fn extra_connections_to_our_relay_only_while_parts_come_to_one_elsewhere() {
        let now = 1_000_000;
        assert!(!collect_lane_wanted(true, 0, now, 0), "no part yet");
        assert!(collect_lane_wanted(true, now - 1_000, now, 0));
        assert!(!collect_lane_wanted(false, now - 1_000, now, 0), "a relay in this process is a pipe");
        assert!(collect_lane_wanted(true, now - 1_000, now, FILE_LANES - 2));
        assert!(!collect_lane_wanted(true, now - 1_000, now, FILE_LANES - 1), "no more than the sender's lanes");
        assert!(collect_lane_holds(now - COLLECT_LANE_IDLE_MS + 1, now));
        assert!(!collect_lane_holds(now - COLLECT_LANE_IDLE_MS, now), "a quiet minute closes them");
    }

    #[test]
    fn unknown_console_kind_still_decodes() {
        let mut p = sample();
        p.console = Some(WireConsole::new(200));
        let back = decode_payload(&encode_payload(&p).unwrap()).unwrap();
        assert_eq!(back.console.map(|c| c.kind), Some(200));
    }

    /// The fast lanes stop padding, and nobody is told. This is what makes
    /// that safe: both shapes come back through the same unpack, so a peer on
    /// an older build reads an unpadded message without knowing there was
    /// anything to know.
    #[test]
    fn an_unpadded_payload_unpacks_like_a_padded_one() {
        let body = b"who is going to be at the thing on friday";
        let padded = pack_payload(body, true);
        let bare = pack_payload(body, false);

        assert_eq!(unpad_payload(&padded).as_deref(), Some(&body[..]));
        assert_eq!(unpad_payload(&bare).as_deref(), Some(&body[..]));
        assert_eq!(padded.len(), 256, "a short message still fills the smallest bucket");
        assert_eq!(bare.len(), 4 + body.len(), "unpadded is the length prefix and the message");
    }

    /// The whole reason for dropping padding: the buckets step by fours, so a
    /// payload just over one becomes three times the bytes on the wire, and on
    /// a narrow tunnel that is three times the wait.
    #[test]
    fn padding_is_cheap_for_text_and_expensive_for_voice() {
        let text = vec![0u8; 40];
        assert_eq!(pack_payload(&text, true).len(), 256);

        let voice = vec![0u8; 5 * 1024];
        assert_eq!(pack_payload(&voice, true).len(), 16_384);
        assert_eq!(pack_payload(&voice, false).len(), 4 + voice.len());
    }
}
