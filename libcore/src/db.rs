use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, MappedRows, OptionalExtension, Row, Transaction};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::security::MasterKey;

mod dm;
pub use dm::{
    is_peer_event_kind, DmDelete, DmDeletionNotice, DmFileUpdate, DmReference, PeerEvent,
    PEER_EVENT_COPIED, PEER_EVENT_RECEIPT_OFFSET, PEER_EVENT_SAVED, PEER_EVENT_SCREENSHOT,
};
mod file_transfer;
pub use file_transfer::FileBatch;
mod delivery_receipt;
pub use delivery_receipt::DeliveryReceipt;
mod message_commit;
pub use message_commit::{IncomingCommit, MessageLifetime, NewOutgoingDm};
mod group_wire;
pub use group_wire::MAX_GROUP_MEMBERS;
mod inbox;
pub use inbox::{is_transient, InboxLetter, INBOX_TTL_MS};

pub const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// How long an undelivered outgoing message keeps being retried.
pub const RETRY_TTL_MS: i64 = 7 * 24 * 3600 * 1000;

trait CollectRows<T> {
    fn collect_rows(self) -> Result<Vec<T>>;
}
impl<F, T> CollectRows<T> for MappedRows<'_, F>
where
    F: FnMut(&Row<'_>) -> rusqlite::Result<T>,
{
    fn collect_rows(self) -> Result<Vec<T>> {
        self.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}

pub type Result<T> = std::result::Result<T, DbError>;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("sql: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("bad key")]
    BadKey,
    #[error(transparent)]
    Memory(#[from] crate::memory::MemoryError),
    #[error("state")]
    State,
    #[error("payload too large")]
    TooLarge,
    #[error(
        "This legacy message has no original sender ID; deletion for both sides cannot be verified"
    )]
    MissingMessageOrigin,
}

/// Result of acknowledging one persisted group recipient. Only `Complete`
/// means the last remaining queued recipient has acknowledged the message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupDeliveryAck {
    NotPending,
    PendingRecipients,
    Complete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustLevel {
    Unverified,
    Verified,
    Blocked,
}
impl TrustLevel {
    fn to_i64(self) -> i64 {
        match self {
            Self::Unverified => 0,
            Self::Verified => 1,
            Self::Blocked => 2,
        }
    }
    fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::Verified,
            2 => Self::Blocked,
            _ => Self::Unverified,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    In,
    Out,
}
impl Direction {
    fn to_i64(self) -> i64 {
        if matches!(self, Self::Out) {
            1
        } else {
            0
        }
    }
    fn from_i64(v: i64) -> Self {
        if v == 1 {
            Self::Out
        } else {
            Self::In
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreKeyKind {
    Identity,
    Signed,
    OneTime,
}
impl PreKeyKind {
    fn to_i64(self) -> i64 {
        match self {
            Self::Identity => 0,
            Self::Signed => 1,
            Self::OneTime => 2,
        }
    }
    fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::Signed,
            2 => Self::OneTime,
            _ => Self::Identity,
        }
    }
}

/// Where a contact stands in being introduced. Separate from [`TrustLevel`]:
/// a request is about whether we talk at all, trust about who they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestState {
    /// An ordinary contact.
    None,
    /// Someone we do not know introduced themselves. Their messages are kept
    /// but neither shown nor acknowledged until we accept.
    Incoming,
    /// We added their card and are waiting to hear back.
    Outgoing,
}
impl RequestState {
    pub fn to_i64(self) -> i64 {
        match self {
            Self::None => 0,
            Self::Incoming => 1,
            Self::Outgoing => 2,
        }
    }
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::Incoming,
            2 => Self::Outgoing,
            _ => Self::None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Contact {
    pub id: i64,
    pub identity_sign: Vec<u8>,
    pub identity_dh: Vec<u8>,
    pub i2p_address: String,
    pub display_name: String,
    pub trust: TrustLevel,
    pub created_at: i64,
    pub last_seen: Option<i64>,
    pub is_bot: bool,
    pub pinned_at: Option<i64>,
    pub last_message_at: Option<i64>,
    /// Relay this contact receives through, from their card. `None` means "use
    /// whatever this client has configured" — the old single-relay behaviour.
    pub relay_address: Option<String>,
    /// This contact has put itself in agent mode with us as master: its console
    /// is open to us until it sends a revoke.
    pub agent_granted: bool,
    pub request_state: RequestState,
}

#[derive(Clone, Debug)]
pub struct Message {
    pub id: i64,
    pub contact_id: Option<i64>,
    pub group_id: Option<Vec<u8>>,
    pub sender_sign_pk: Option<Vec<u8>>,
    pub direction: Direction,
    pub body: String,
    pub sent_at: i64,
    pub sent: bool,
    pub delivered: bool,
    pub delivery_expired_at: Option<i64>,
    pub read: bool,
    pub expires_at: Option<i64>,
    pub last_attempt_at: Option<i64>,
    pub send_attempts: i64,
    pub reply_to: Option<i64>,
    /// Stable local arrival number within this direct conversation.
    pub dm_seq: Option<i64>,
    pub deleted_at: Option<i64>,
}

/// A file we send in parts, with its attachment (sealed in parts on disk).
#[derive(Clone, Debug)]
pub struct FileOut {
    pub file_id: [u8; 16],
    pub message_id: i64,
    pub sha256: [u8; 32],
    pub cancelled: bool,
    pub attachment: Attachment,
}

/// Where one recipient is with one file (`crate::files::Sending`).
#[derive(Clone, Debug)]
pub struct FilePeer {
    pub file_id: [u8; 16],
    pub contact_id: i64,
    pub next: u32,
    pub acked: u32,
    pub resend: Vec<u32>,
    pub last_sent_at: Option<i64>,
    pub last_ack_at: Option<i64>,
    pub failed: bool,
}

/// A file coming in parts.
#[derive(Clone, Debug)]
pub struct FileIn {
    pub file_id: [u8; 16],
    pub contact_id: i64,
    pub message_id: i64,
    pub name: String,
    pub size: u64,
    pub sha256: [u8; 32],
    pub chunk_size: u32,
    pub path: String,
    pub key: Vec<u8>,
    pub bits: Vec<u8>,
    pub received: u32,
    pub created_at: i64,
}

fn blob16(v: Vec<u8>) -> rusqlite::Result<[u8; 16]> {
    v.try_into().map_err(|_| {
        rusqlite::Error::InvalidColumnType(0, "file_id".into(), rusqlite::types::Type::Blob)
    })
}

fn blob32(v: Vec<u8>) -> rusqlite::Result<[u8; 32]> {
    v.try_into().map_err(|_| {
        rusqlite::Error::InvalidColumnType(0, "sha256".into(), rusqlite::types::Type::Blob)
    })
}

fn u32s(v: Option<Vec<u8>>) -> Vec<u32> {
    v.unwrap_or_default()
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn u32s_blob(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

#[derive(Clone, Debug)]
pub struct Attachment {
    pub id: i64,
    pub message_id: i64,
    pub name: String,
    pub size: i64,
    pub key: Vec<u8>,
    pub path: String,
    /// Sealed in parts of this many bytes (`crate::files`); `None`: sealed
    /// whole, as one chunk.
    pub chunk_size: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct NewAttachment {
    pub name: String,
    pub size: i64,
    pub key: Vec<u8>,
    pub path: String,
    /// Sealed in parts of this many bytes (`crate::files`); `None`: sealed
    /// whole, as one chunk.
    pub chunk_size: Option<i64>,
}

/// A staged outgoing attachment and, for a chunked file, its stable offer.
/// The encrypted bytes must be complete before publishing the group message.
#[derive(Clone, Debug)]
pub struct NewGroupAttachment {
    pub attachment: NewAttachment,
    pub file_offer: Option<([u8; 16], [u8; 32])>,
}

/// All message-level data needed to publish one outgoing group fan-out.
/// Settings are already serialized by the caller; no callbacks or awaits run
/// under the database lock.
pub struct NewOutgoingGroup<'a> {
    pub group_id: &'a [u8],
    pub sender_sign_pk: &'a [u8],
    pub body: &'a str,
    pub sent_at: i64,
    pub expires_at: Option<i64>,
    pub reply_to: Option<i64>,
    pub buttons: Option<&'a [u8]>,
    pub notify_sound: Option<&'a [u8]>,
    pub recipients: &'a [i64],
}

#[derive(Clone, Debug)]
pub struct PreKey {
    pub id: i64,
    pub kind: PreKeyKind,
    pub private: Vec<u8>,
    pub public: Vec<u8>,
    pub created_at: i64,
}

#[derive(Clone, Debug)]
pub struct Group {
    pub id: Vec<u8>,
    pub name: String,
    pub created_at: i64,
    pub pinned_at: Option<i64>,
    pub last_message_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct GroupMember {
    pub group_id: Vec<u8>,
    pub sign_pk: Vec<u8>,
    pub dh_pk: Vec<u8>,
    pub i2p_address: String,
    pub display_name: String,
    pub is_self: bool,
}

macro_rules! select_contact { () => { "SELECT id, identity_sign, identity_dh, i2p_address, display_name, trust, created_at, last_seen, COALESCE(is_bot, 0), pinned_at, last_message_at, relay_address, COALESCE(agent_granted, 0), COALESCE(request_state, 0) FROM contacts" }; }
macro_rules! select_group {
    () => {
        "SELECT id, name, created_at, pinned_at, last_message_at FROM groups"
    };
}
macro_rules! message_cols { () => { "id, contact_id, group_id, sender_sign_pk, direction, body, sent_at, sent, delivered, read, expires_at, last_attempt_at, send_attempts, reply_to, dm_seq, deleted_at, delivery_expired_at" }; }
macro_rules! message_cols_m { () => { "m.id, m.contact_id, m.group_id, m.sender_sign_pk, m.direction, m.body, m.sent_at, m.sent, m.delivered, m.read, m.expires_at, m.last_attempt_at, m.send_attempts, m.reply_to, m.dm_seq, m.deleted_at, m.delivery_expired_at" }; }

mod attachments;
mod backup;
mod contacts;
mod dedup;
mod dht;
mod groups;
mod messages;
mod outbox;
mod delivery_expiry;
mod view_once;
pub use view_once::ViewOnceDelivery;
pub use delivery_expiry::DeliveryExpiryNotice;
mod pins;
mod prekeys;
mod schema;
mod sessions;
mod settings;

pub struct Db {
    conn: Mutex<Connection>,
    file_mutations: Mutex<()>,
}

impl Db {
    pub fn open(path: &Path, key: &MasterKey) -> Result<Self> {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p)?;
        }
        let conn = Connection::open(path)?;
        let pragma = {
            let exposed = key.expose()?;
            Zeroizing::new(format!(
            "PRAGMA key = \"x'{}'\";\nPRAGMA cipher_page_size = 4096;\nPRAGMA cipher_memory_security = ON;\nPRAGMA temp_store = MEMORY;",
            &*Zeroizing::new(hex(&*exposed)),
        ))
        };
        conn.execute_batch(&pragma)?;
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
            r.get::<_, i64>(0)
        })
        .map_err(|_| DbError::BadKey)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;\nPRAGMA synchronous = NORMAL;\nPRAGMA foreign_keys = ON;\nPRAGMA secure_delete = ON;\nPRAGMA auto_vacuum = FULL;",
        )?;
        Self::migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            file_mutations: Mutex::new(()),
        })
    }

    pub fn open_plain(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;\nPRAGMA synchronous = NORMAL;\nPRAGMA foreign_keys = ON;\nPRAGMA temp_store = MEMORY;",
        )?;
        Self::migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            file_mutations: Mutex::new(()),
        })
    }

    pub fn rekey(&self, key: &MasterKey) -> Result<()> {
        self.with_conn(|conn| {
            let pragma = {
                let exposed = key.expose()?;
                Zeroizing::new(format!(
                    "PRAGMA rekey = \"x'{}'\";",
                    &*Zeroizing::new(hex(&*exposed)),
                ))
            };
            conn.execute_batch(&pragma)?;
            Ok(())
        })
    }

    pub fn vacuum(&self) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute_batch("VACUUM")?;
            Ok(())
        })
    }

    fn with_conn<R>(&self, f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
        let conn = self.conn.lock().map_err(|_| DbError::State)?;
        f(&conn)
    }

    fn with_tx<R>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<R>) -> Result<R> {
        let mut conn = self.conn.lock().map_err(|_| DbError::State)?;
        let tx = conn.transaction()?;
        let r = f(&tx)?;
        tx.commit()?;
        Ok(r)
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn check_body(body: &str) -> Result<()> {
    if body.len() > MAX_BODY_BYTES {
        Err(DbError::TooLarge)
    } else {
        Ok(())
    }
}

fn map_group(r: &Row<'_>) -> rusqlite::Result<Group> {
    Ok(Group {
        id: r.get(0)?,
        name: r.get(1)?,
        created_at: r.get(2)?,
        pinned_at: r.get(3)?,
        last_message_at: r.get(4)?,
    })
}

fn map_group_member(r: &Row<'_>) -> rusqlite::Result<GroupMember> {
    Ok(GroupMember {
        group_id: r.get(0)?,
        sign_pk: r.get(1)?,
        dh_pk: r.get(2)?,
        i2p_address: r.get(3)?,
        display_name: r.get(4)?,
        is_self: r.get::<_, i64>(5)? != 0,
    })
}

fn map_attachment(r: &Row<'_>) -> rusqlite::Result<Attachment> {
    Ok(Attachment {
        id: r.get(0)?,
        message_id: r.get(1)?,
        name: r.get(2)?,
        size: r.get(3)?,
        key: r.get(4)?,
        path: r.get(5)?,
        chunk_size: r.get(6)?,
    })
}

fn insert_attachments(
    tx: &Transaction<'_>,
    message_id: i64,
    attachments: &[NewAttachment],
) -> Result<()> {
    if attachments.is_empty() {
        return Ok(());
    }
    let mut s = tx.prepare_cached(
        "INSERT INTO attachments (message_id, name, size, key, path, chunk_size) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for a in attachments {
        s.execute(params![
            message_id,
            a.name,
            a.size,
            a.key,
            a.path,
            a.chunk_size
        ])?;
    }
    Ok(())
}

fn fts_prefix_query(needle: &str) -> String {
    needle
        .split_whitespace()
        .filter(|t| t.chars().any(|c| c.is_alphanumeric()))
        .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push_str(&format!("{:02x}", x));
    }
    s
}
