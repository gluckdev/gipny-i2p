use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, MappedRows, OptionalExtension, Row, Transaction};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::security::MasterKey;

pub const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

const MIGRATE_TABLES: &[&str] = &["messages", "contacts", "groups"];

trait CollectRows<T> {
    fn collect_rows(self) -> Result<Vec<T>>;
}
impl<F, T> CollectRows<T> for MappedRows<'_, F>
where F: FnMut(&Row<'_>) -> rusqlite::Result<T> {
    fn collect_rows(self) -> Result<Vec<T>> {
        self.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }
}

pub type Result<T> = std::result::Result<T, DbError>;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("io: {0}")] Io(#[from] std::io::Error),
    #[error("sql: {0}")] Sql(#[from] rusqlite::Error),
    #[error("bad key")] BadKey,
    #[error("state")] State,
    #[error("payload too large")] TooLarge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustLevel { Unverified, Verified, Blocked }
impl TrustLevel {
    fn to_i64(self) -> i64 { match self { Self::Unverified => 0, Self::Verified => 1, Self::Blocked => 2 } }
    fn from_i64(v: i64) -> Self { match v { 1 => Self::Verified, 2 => Self::Blocked, _ => Self::Unverified } }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction { In, Out }
impl Direction {
    fn to_i64(self) -> i64 { if matches!(self, Self::Out) { 1 } else { 0 } }
    fn from_i64(v: i64) -> Self { if v == 1 { Self::Out } else { Self::In } }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreKeyKind { Identity, Signed, OneTime }
impl PreKeyKind {
    fn to_i64(self) -> i64 { match self { Self::Identity => 0, Self::Signed => 1, Self::OneTime => 2 } }
    fn from_i64(v: i64) -> Self { match v { 1 => Self::Signed, 2 => Self::OneTime, _ => Self::Identity } }
}

#[derive(Clone, Debug)]
pub struct Contact {
    pub id: i64,
    pub identity_sign: Vec<u8>,
    pub identity_dh: Vec<u8>,
    pub onion_address: String,
    pub display_name: String,
    pub trust: TrustLevel,
    pub created_at: i64,
    pub last_seen: Option<i64>,
    pub is_bot: bool,
    pub pinned_at: Option<i64>,
    pub last_message_at: Option<i64>,
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
    pub read: bool,
    pub expires_at: Option<i64>,
    pub last_attempt_at: Option<i64>,
    pub send_attempts: i64,
    pub reply_to: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct Attachment {
    pub id: i64,
    pub message_id: i64,
    pub name: String,
    pub size: i64,
    pub key: Vec<u8>,
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct NewAttachment {
    pub name: String,
    pub size: i64,
    pub key: Vec<u8>,
    pub path: String,
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
    pub onion: String,
    pub display_name: String,
    pub is_self: bool,
}

const SCHEMA_V1: &str = r#"
CREATE TABLE contacts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    identity_sign BLOB NOT NULL,
    identity_dh BLOB UNIQUE NOT NULL,
    onion_address TEXT NOT NULL,
    display_name TEXT NOT NULL,
    trust INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    last_seen INTEGER
);
CREATE TABLE sessions (
    contact_id INTEGER PRIMARY KEY,
    state BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (contact_id) REFERENCES contacts(id) ON DELETE CASCADE
);
CREATE TABLE prekeys (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind INTEGER NOT NULL,
    private BLOB NOT NULL,
    public BLOB NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_prekeys_kind ON prekeys(kind);
CREATE TABLE groups (
    id BLOB PRIMARY KEY,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE group_members (
    group_id BLOB NOT NULL,
    sign_pk BLOB NOT NULL,
    dh_pk BLOB NOT NULL,
    onion TEXT NOT NULL,
    display_name TEXT NOT NULL,
    is_self INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (group_id, sign_pk),
    FOREIGN KEY (group_id) REFERENCES groups(id) ON DELETE CASCADE
);
CREATE INDEX idx_group_members_group ON group_members(group_id);
CREATE TABLE messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    contact_id INTEGER,
    group_id BLOB,
    sender_sign_pk BLOB,
    direction INTEGER NOT NULL,
    body TEXT NOT NULL,
    sent_at INTEGER NOT NULL,
    delivered INTEGER NOT NULL DEFAULT 0,
    read INTEGER NOT NULL DEFAULT 0,
    expires_at INTEGER,
    FOREIGN KEY (contact_id) REFERENCES contacts(id) ON DELETE CASCADE,
    FOREIGN KEY (group_id) REFERENCES groups(id) ON DELETE CASCADE
);
CREATE INDEX idx_messages_contact ON messages(contact_id, sent_at) WHERE contact_id IS NOT NULL;
CREATE INDEX idx_messages_group ON messages(group_id, sent_at) WHERE group_id IS NOT NULL;
CREATE INDEX idx_messages_expires ON messages(expires_at) WHERE expires_at IS NOT NULL;
CREATE TABLE attachments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    size INTEGER NOT NULL,
    key BLOB NOT NULL,
    path TEXT NOT NULL,
    FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
);
CREATE INDEX idx_attachments_message ON attachments(message_id);
CREATE TABLE settings (
    k TEXT PRIMARY KEY,
    v BLOB NOT NULL
);
"#;

macro_rules! select_contact { () => { "SELECT id, identity_sign, identity_dh, onion_address, display_name, trust, created_at, last_seen, COALESCE(is_bot, 0), pinned_at, last_message_at FROM contacts" }; }
macro_rules! select_group { () => { "SELECT id, name, created_at, pinned_at, last_message_at FROM groups" }; }
macro_rules! message_cols { () => { "id, contact_id, group_id, sender_sign_pk, direction, body, sent_at, sent, delivered, read, expires_at, last_attempt_at, send_attempts, reply_to" }; }
macro_rules! message_cols_m { () => { "m.id, m.contact_id, m.group_id, m.sender_sign_pk, m.direction, m.body, m.sent_at, m.sent, m.delivered, m.read, m.expires_at, m.last_attempt_at, m.send_attempts, m.reply_to" }; }

pub struct Db { conn: Mutex<Connection> }

impl Db {
    pub fn open(path: &Path, key: &MasterKey) -> Result<Self> {
        if let Some(p) = path.parent() { fs::create_dir_all(p)?; }
        let conn = Connection::open(path)?;
        let pragma = Zeroizing::new(format!(
            "PRAGMA key = \"x'{}'\";\nPRAGMA cipher_page_size = 4096;\nPRAGMA cipher_memory_security = ON;\nPRAGMA temp_store = MEMORY;",
            &*Zeroizing::new(hex(key.as_bytes())),
        ));
        conn.execute_batch(&pragma)?;
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))
            .map_err(|_| DbError::BadKey)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;\nPRAGMA synchronous = NORMAL;\nPRAGMA foreign_keys = ON;\nPRAGMA secure_delete = ON;\nPRAGMA auto_vacuum = FULL;",
        )?;
        Self::migrate(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn open_plain(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() { fs::create_dir_all(p)?; }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;\nPRAGMA synchronous = NORMAL;\nPRAGMA foreign_keys = ON;\nPRAGMA temp_store = MEMORY;",
        )?;
        Self::migrate(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn rekey(&self, key: &MasterKey) -> Result<()> {
        self.with_conn(|conn| {
            let pragma = Zeroizing::new(format!(
                "PRAGMA rekey = \"x'{}'\";",
                &*Zeroizing::new(hex(key.as_bytes())),
            ));
            conn.execute_batch(&pragma)?;
            Ok(())
        })
    }

    pub fn vacuum(&self) -> Result<()> {
        self.with_conn(|conn| { conn.execute_batch("VACUUM")?; Ok(()) })
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

    fn migrate(conn: &Connection) -> Result<()> {
        let has_messages: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'messages'",
            [], |r| r.get(0))?;
        if has_messages == 0 {
            conn.execute_batch(SCHEMA_V1)?;
        }

        Self::ensure_column(conn, "messages", "origin_msg_id", "INTEGER")?;
        if Self::ensure_column(conn, "messages", "sent", "INTEGER NOT NULL DEFAULT 0")? {
            conn.execute_batch("UPDATE messages SET sent = delivered WHERE direction = 1;")?;
        }
        Self::ensure_column(conn, "messages", "last_attempt_at", "INTEGER")?;
        Self::ensure_column(conn, "messages", "send_attempts", "INTEGER NOT NULL DEFAULT 0")?;
        Self::ensure_column(conn, "messages", "reply_to", "INTEGER")?;

        let has_fts: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'messages_fts'",
            [], |r| r.get(0))?;
        if has_fts == 0 {
            conn.execute_batch("
                CREATE VIRTUAL TABLE messages_fts USING fts5(body, content='messages', content_rowid='id', tokenize='unicode61 remove_diacritics 2');
                INSERT INTO messages_fts(messages_fts) VALUES('rebuild');
            ")?;
        }

        conn.execute_batch("
            DELETE FROM messages WHERE id IN (
                SELECT id FROM (
                    SELECT id, ROW_NUMBER() OVER (PARTITION BY contact_id, origin_msg_id ORDER BY id) AS rn
                    FROM messages
                    WHERE contact_id IS NOT NULL AND origin_msg_id IS NOT NULL
                ) WHERE rn > 1
            );
            DELETE FROM messages WHERE id IN (
                SELECT id FROM (
                    SELECT id, ROW_NUMBER() OVER (PARTITION BY group_id, sender_sign_pk, origin_msg_id ORDER BY id) AS rn
                    FROM messages
                    WHERE group_id IS NOT NULL AND sender_sign_pk IS NOT NULL AND origin_msg_id IS NOT NULL
                ) WHERE rn > 1
            );

            DROP INDEX IF EXISTS idx_messages_origin;
            DROP INDEX IF EXISTS idx_messages_group_origin;

            CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_origin
                ON messages(contact_id, origin_msg_id)
                WHERE contact_id IS NOT NULL AND origin_msg_id IS NOT NULL;
            CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_group_origin
                ON messages(group_id, sender_sign_pk, origin_msg_id)
                WHERE group_id IS NOT NULL AND sender_sign_pk IS NOT NULL AND origin_msg_id IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_messages_outbox
                ON messages(contact_id, sent, delivered)
                WHERE direction = 1;
            CREATE INDEX IF NOT EXISTS idx_messages_reply_to
                ON messages(reply_to)
                WHERE reply_to IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_messages_unread_contact
                ON messages(contact_id)
                WHERE contact_id IS NOT NULL AND direction = 0 AND read = 0;
            CREATE INDEX IF NOT EXISTS idx_messages_unread_group
                ON messages(group_id)
                WHERE group_id IS NOT NULL AND direction = 0 AND read = 0;
            CREATE INDEX IF NOT EXISTS idx_group_members_sign
                ON group_members(sign_pk);

            CREATE TABLE IF NOT EXISTS pinned_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                contact_id INTEGER,
                group_id BLOB,
                message_id INTEGER NOT NULL,
                pinned_at INTEGER NOT NULL,
                FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE,
                FOREIGN KEY (contact_id) REFERENCES contacts(id) ON DELETE CASCADE,
                FOREIGN KEY (group_id) REFERENCES groups(id) ON DELETE CASCADE
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_pinned_contact_msg
                ON pinned_messages(contact_id, message_id) WHERE contact_id IS NOT NULL;
            CREATE UNIQUE INDEX IF NOT EXISTS idx_pinned_group_msg
                ON pinned_messages(group_id, message_id) WHERE group_id IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_pinned_contact
                ON pinned_messages(contact_id) WHERE contact_id IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_pinned_group
                ON pinned_messages(group_id) WHERE group_id IS NOT NULL;

            CREATE TABLE IF NOT EXISTS deferred_pins (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                contact_id INTEGER,
                group_id BLOB,
                sender_sign_pk BLOB NOT NULL,
                origin_msg_id INTEGER NOT NULL,
                unpin INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                FOREIGN KEY (contact_id) REFERENCES contacts(id) ON DELETE CASCADE,
                FOREIGN KEY (group_id) REFERENCES groups(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_deferred_pins_contact
                ON deferred_pins(contact_id, sender_sign_pk, origin_msg_id)
                WHERE contact_id IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_deferred_pins_group
                ON deferred_pins(group_id, sender_sign_pk, origin_msg_id)
                WHERE group_id IS NOT NULL;

            CREATE TABLE IF NOT EXISTS resync_log (
                contact_id INTEGER PRIMARY KEY,
                last_resync_at INTEGER NOT NULL,
                FOREIGN KEY (contact_id) REFERENCES contacts(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS callback_seen (
                sender_sign_pk BLOB NOT NULL,
                sent_at INTEGER NOT NULL,
                callback_data TEXT NOT NULL,
                seen_at INTEGER NOT NULL,
                PRIMARY KEY (sender_sign_pk, sent_at, callback_data)
            );
            CREATE INDEX IF NOT EXISTS idx_callback_seen_age ON callback_seen(seen_at);

            CREATE TABLE IF NOT EXISTS dead_letters (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                contact_id INTEGER,
                group_id BLOB,
                payload BLOB NOT NULL,
                kind TEXT NOT NULL,
                error TEXT NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0,
                first_seen_at INTEGER NOT NULL,
                last_seen_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_dead_letters_age ON dead_letters(last_seen_at);

            CREATE TABLE IF NOT EXISTS notify_seen (
                idempotency_key TEXT PRIMARY KEY,
                message_id INTEGER NOT NULL,
                seen_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_notify_seen_age ON notify_seen(seen_at);

            CREATE TABLE IF NOT EXISTS pending_outbound (
                msg_id INTEGER NOT NULL,
                recipient_contact_id INTEGER NOT NULL,
                last_attempt_at INTEGER,
                send_attempts INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                PRIMARY KEY (msg_id, recipient_contact_id)
            );
            CREATE INDEX IF NOT EXISTS idx_pending_outbound_recipient ON pending_outbound(recipient_contact_id);

            CREATE TRIGGER IF NOT EXISTS tr_pending_outbound_msg_del
                AFTER DELETE ON messages
                BEGIN
                    DELETE FROM pending_outbound WHERE msg_id = OLD.id;
                END;

            CREATE TRIGGER IF NOT EXISTS tr_pending_outbound_contact_del
                AFTER DELETE ON contacts
                BEGIN
                    DELETE FROM pending_outbound WHERE recipient_contact_id = OLD.id;
                END;

            CREATE TRIGGER IF NOT EXISTS tr_buttons_msg_del
                AFTER DELETE ON messages
                BEGIN
                    DELETE FROM settings WHERE k = 'buttons_' || OLD.id;
                END;

            CREATE TRIGGER IF NOT EXISTS tr_messages_fts_ai AFTER INSERT ON messages BEGIN
                INSERT INTO messages_fts(rowid, body) VALUES (NEW.id, NEW.body);
            END;
            CREATE TRIGGER IF NOT EXISTS tr_messages_fts_ad AFTER DELETE ON messages BEGIN
                INSERT INTO messages_fts(messages_fts, rowid, body) VALUES('delete', OLD.id, OLD.body);
            END;
            CREATE TRIGGER IF NOT EXISTS tr_messages_fts_au AFTER UPDATE OF body ON messages BEGIN
                INSERT INTO messages_fts(messages_fts, rowid, body) VALUES('delete', OLD.id, OLD.body);
                INSERT INTO messages_fts(rowid, body) VALUES (NEW.id, NEW.body);
            END;

            PRAGMA user_version = 9;
        ")?;
        Self::ensure_column(conn, "contacts", "is_bot", "INTEGER NOT NULL DEFAULT 0")?;
        let added_contact_lma = Self::ensure_column(conn, "contacts", "last_message_at", "INTEGER")?;
        Self::ensure_column(conn, "contacts", "pinned_at", "INTEGER")?;
        let added_group_lma = Self::ensure_column(conn, "groups", "last_message_at", "INTEGER")?;
        Self::ensure_column(conn, "groups", "pinned_at", "INTEGER")?;
        if added_contact_lma {
            conn.execute_batch("
                UPDATE contacts SET last_message_at = (
                    SELECT MAX(sent_at) FROM messages m WHERE m.contact_id = contacts.id
                ) WHERE last_message_at IS NULL;
            ")?;
        }
        if added_group_lma {
            conn.execute_batch("
                UPDATE groups SET last_message_at = (
                    SELECT MAX(sent_at) FROM messages m WHERE m.group_id = groups.id
                ) WHERE last_message_at IS NULL;
            ")?;
        }
        conn.execute_batch("
            CREATE TRIGGER IF NOT EXISTS tr_messages_bump_contact_last
                AFTER INSERT ON messages WHEN NEW.contact_id IS NOT NULL
                BEGIN
                    UPDATE contacts SET last_message_at = NEW.sent_at
                    WHERE id = NEW.contact_id
                      AND (last_message_at IS NULL OR last_message_at < NEW.sent_at);
                END;
            CREATE TRIGGER IF NOT EXISTS tr_messages_bump_group_last
                AFTER INSERT ON messages WHEN NEW.group_id IS NOT NULL
                BEGIN
                    UPDATE groups SET last_message_at = NEW.sent_at
                    WHERE id = NEW.group_id
                      AND (last_message_at IS NULL OR last_message_at < NEW.sent_at);
                END;
            CREATE INDEX IF NOT EXISTS idx_contacts_sort
                ON contacts(pinned_at, last_message_at);
            CREATE INDEX IF NOT EXISTS idx_groups_sort
                ON groups(pinned_at, last_message_at);
        ")?;
        Ok(())
    }

    fn ensure_column(conn: &Connection, table: &str, column: &str, decl: &str) -> Result<bool> {
        assert!(MIGRATE_TABLES.contains(&table), "ensure_column: table {table} not in whitelist");
        assert!(column.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "ensure_column: column name {column:?} has unsafe chars");
        assert!(decl.chars().all(|c| c.is_ascii_alphanumeric() || c.is_whitespace() || c == '_'),
            "ensure_column: decl {decl:?} has unsafe chars");
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
            params![table, column], |r| r.get(0))?;
        if n == 0 {
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl};"))?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn add_contact(&self, sign: &[u8], dh: &[u8], onion: &str, name: &str) -> Result<i64> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO contacts (identity_sign, identity_dh, onion_address, display_name, trust, created_at) VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                params![sign, dh, onion, name, now_ms()],
            )?;
            Ok(conn.query_row("SELECT id FROM contacts WHERE identity_dh = ?1", params![dh], |r| r.get(0))?)
        })
    }

    pub fn get_contact(&self, id: i64) -> Result<Option<Contact>> {
        self.with_conn(|c| c.prepare_cached(concat!(select_contact!(), " WHERE id = ?1"))?
            .query_row(params![id], Self::map_contact).optional().map_err(Into::into))
    }

    pub fn find_contact_by_identity(&self, dh: &[u8]) -> Result<Option<Contact>> {
        self.with_conn(|c| c.prepare_cached(concat!(select_contact!(), " WHERE identity_dh = ?1"))?
            .query_row(params![dh], Self::map_contact).optional().map_err(Into::into))
    }

    pub fn find_contact_by_sign(&self, sign: &[u8]) -> Result<Option<Contact>> {
        self.with_conn(|c| c.prepare_cached(concat!(select_contact!(), " WHERE identity_sign = ?1"))?
            .query_row(params![sign], Self::map_contact).optional().map_err(Into::into))
    }

    pub fn list_contacts(&self) -> Result<Vec<Contact>> {
        self.with_conn(|c| c.prepare_cached(concat!(
            select_contact!(),
            " ORDER BY pinned_at IS NULL, pinned_at DESC, last_message_at IS NULL, last_message_at DESC, display_name"
        ))?
            .query_map([], Self::map_contact)?.collect_rows())
    }

    pub fn pin_contact(&self, id: i64, ts: i64) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "UPDATE contacts SET pinned_at = ?1 WHERE id = ?2",
            params![ts, id])?; Ok(()) })
    }

    pub fn unpin_contact(&self, id: i64) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "UPDATE contacts SET pinned_at = NULL WHERE id = ?1",
            params![id])?; Ok(()) })
    }

    pub fn pin_group(&self, id: &[u8], ts: i64) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "UPDATE groups SET pinned_at = ?1 WHERE id = ?2",
            params![ts, id])?; Ok(()) })
    }

    pub fn unpin_group(&self, id: &[u8]) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "UPDATE groups SET pinned_at = NULL WHERE id = ?1",
            params![id])?; Ok(()) })
    }

    pub fn update_contact(&self, id: i64, name: &str, trust: TrustLevel) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "UPDATE contacts SET display_name = ?1, trust = ?2 WHERE id = ?3",
            params![name, trust.to_i64(), id])?; Ok(()) })
    }

    pub fn update_contact_name(&self, id: i64, name: &str) -> Result<bool> {
        self.with_conn(|c| Ok(c.execute(
            "UPDATE contacts SET display_name = ?1 WHERE id = ?2 AND display_name <> ?1",
            params![name, id])? > 0))
    }

    pub fn touch_contact(&self, id: i64) -> Result<()> {
        self.with_conn(|c| { c.execute("UPDATE contacts SET last_seen = ?1 WHERE id = ?2", params![now_ms(), id])?; Ok(()) })
    }

    pub fn delete_contact(&self, id: i64) -> Result<()> {
        self.with_conn(|c| { c.execute("DELETE FROM contacts WHERE id = ?1", params![id])?; Ok(()) })
    }

    fn map_contact(r: &rusqlite::Row) -> rusqlite::Result<Contact> {
        Ok(Contact {
            id: r.get(0)?,
            identity_sign: r.get(1)?,
            identity_dh: r.get(2)?,
            onion_address: r.get(3)?,
            display_name: r.get(4)?,
            trust: TrustLevel::from_i64(r.get(5)?),
            created_at: r.get(6)?,
            last_seen: r.get(7)?,
            is_bot: r.get::<_, i64>(8)? != 0,
            pinned_at: r.get(9)?,
            last_message_at: r.get(10)?,
        })
    }

    pub fn set_contact_is_bot(&self, id: i64, is_bot: bool) -> Result<bool> {
        self.with_conn(|c| Ok(c.execute(
            "UPDATE contacts SET is_bot = ?1 WHERE id = ?2 AND is_bot <> ?1",
            params![is_bot as i64, id])? > 0))
    }

    pub fn put_session(&self, contact_id: i64, state: &[u8]) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "INSERT INTO sessions (contact_id, state, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(contact_id) DO UPDATE SET state = excluded.state, updated_at = excluded.updated_at",
            params![contact_id, state, now_ms()])?; Ok(()) })
    }

    pub fn get_session(&self, contact_id: i64) -> Result<Option<Vec<u8>>> {
        self.with_conn(|c| c.prepare_cached("SELECT state FROM sessions WHERE contact_id = ?1")?
            .query_row(params![contact_id], |r| r.get::<_, Vec<u8>>(0)).optional().map_err(Into::into))
    }

    pub fn delete_session(&self, contact_id: i64) -> Result<()> {
        self.with_conn(|c| { c.execute("DELETE FROM sessions WHERE contact_id = ?1", params![contact_id])?; Ok(()) })
    }

    pub fn add_prekey(&self, kind: PreKeyKind, private: &[u8], public: &[u8]) -> Result<i64> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO prekeys (kind, private, public, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![kind.to_i64(), private, public, now_ms()])?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn get_prekey(&self, id: i64) -> Result<Option<PreKey>> {
        self.with_conn(|c| c.prepare_cached("SELECT id, kind, private, public, created_at FROM prekeys WHERE id = ?1")?
            .query_row(params![id], Self::map_prekey).optional().map_err(Into::into))
    }

    pub fn list_prekeys(&self, kind: PreKeyKind) -> Result<Vec<PreKey>> {
        self.with_conn(|c| c.prepare_cached("SELECT id, kind, private, public, created_at FROM prekeys WHERE kind = ?1 ORDER BY id")?
            .query_map(params![kind.to_i64()], Self::map_prekey)?.collect_rows())
    }

    pub fn consume_prekey(&self, id: i64) -> Result<Option<PreKey>> {
        self.with_tx(|tx| {
            let pk: Option<PreKey> = tx.prepare_cached("SELECT id, kind, private, public, created_at FROM prekeys WHERE id = ?1")?
                .query_row(params![id], Self::map_prekey).optional()?;
            if pk.is_some() { tx.execute("DELETE FROM prekeys WHERE id = ?1", params![id])?; }
            Ok(pk)
        })
    }

    fn map_prekey(r: &rusqlite::Row) -> rusqlite::Result<PreKey> {
        Ok(PreKey {
            id: r.get(0)?,
            kind: PreKeyKind::from_i64(r.get(1)?),
            private: r.get(2)?,
            public: r.get(3)?,
            created_at: r.get(4)?,
        })
    }

    pub fn insert_message(
        &self,
        contact_id: i64,
        direction: Direction,
        body: &str,
        sent_at: i64,
        expires_at: Option<i64>,
        attachments: &[NewAttachment],
    ) -> Result<i64> {
        self.insert_message_with_origin(contact_id, direction, body, sent_at, expires_at, attachments, None)
    }

    pub fn insert_message_with_origin(
        &self,
        contact_id: i64,
        direction: Direction,
        body: &str,
        sent_at: i64,
        expires_at: Option<i64>,
        attachments: &[NewAttachment],
        origin_msg_id: Option<i64>,
    ) -> Result<i64> {
        check_body(body)?;
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO messages (contact_id, direction, body, sent_at, expires_at, origin_msg_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![contact_id, direction.to_i64(), body, sent_at, expires_at, origin_msg_id])?;
            let id = tx.last_insert_rowid();
            insert_attachments(tx, id, attachments)?;
            Ok(id)
        })
    }

    pub fn find_message_by_origin(&self, contact_id: i64, origin_msg_id: i64) -> Result<Option<i64>> {
        self.with_conn(|c| c.prepare_cached(
            "SELECT id FROM messages WHERE contact_id = ?1 AND origin_msg_id = ?2 ORDER BY id DESC LIMIT 1")?
            .query_row(params![contact_id, origin_msg_id], |r| r.get::<_, i64>(0)).optional().map_err(Into::into))
    }

    pub fn resolve_contact_message(&self, contact_id: i64, origin: i64) -> Result<Option<i64>> {
        self.with_conn(|c| c.prepare_cached(
            "SELECT id FROM messages WHERE contact_id = ?1 AND (id = ?2 OR origin_msg_id = ?2) ORDER BY id DESC LIMIT 1")?
            .query_row(params![contact_id, origin], |r| r.get::<_, i64>(0)).optional().map_err(Into::into))
    }

    pub fn resolve_group_message(
        &self, group_id: &[u8], sender_sign_pk: &[u8], origin: i64, self_sign_pk: &[u8],
    ) -> Result<Option<i64>> {
        self.with_conn(|c| if sender_sign_pk == self_sign_pk {
            c.prepare_cached("SELECT id FROM messages WHERE group_id = ?1 AND direction = 1 AND id = ?2")?
                .query_row(params![group_id, origin], |r| r.get::<_, i64>(0)).optional().map_err(Into::into)
        } else {
            c.prepare_cached("SELECT id FROM messages
                 WHERE group_id = ?1 AND sender_sign_pk = ?2 AND origin_msg_id = ?3
                 ORDER BY id DESC LIMIT 1")?
                .query_row(params![group_id, sender_sign_pk, origin], |r| r.get::<_, i64>(0)).optional().map_err(Into::into)
        })
    }

    pub fn message_origin(&self, id: i64) -> Result<Option<i64>> {
        self.with_conn(|c| c.prepare_cached("SELECT origin_msg_id FROM messages WHERE id = ?1")?
            .query_row(params![id], |r| r.get::<_, Option<i64>>(0)).optional()
            .map(|o| o.flatten()).map_err(Into::into))
    }

    pub fn get_message(&self, id: i64) -> Result<Option<Message>> {
        self.with_conn(|c| c.prepare_cached(concat!("SELECT ", message_cols!(), " FROM messages WHERE id = ?1"))?
            .query_row(params![id], Self::map_message).optional().map_err(Into::into))
    }

    pub fn update_message_body(&self, id: i64, body: &str) -> Result<()> {
        check_body(body)?;
        self.with_conn(|c| { c.execute("UPDATE messages SET body = ?1 WHERE id = ?2", params![body, id])?; Ok(()) })
    }

    pub fn set_reply_to(&self, id: i64, reply_to: Option<i64>) -> Result<()> {
        self.with_conn(|c| { c.execute("UPDATE messages SET reply_to = ?1 WHERE id = ?2", params![reply_to, id])?; Ok(()) })
    }

    pub fn insert_group_message(
        &self,
        group_id: &[u8],
        sender_sign_pk: Option<&[u8]>,
        direction: Direction,
        body: &str,
        sent_at: i64,
        expires_at: Option<i64>,
        attachments: &[NewAttachment],
    ) -> Result<i64> {
        self.insert_group_message_with_origin(
            group_id, sender_sign_pk, direction, body, sent_at, expires_at, attachments, None,
        )
    }

    pub fn insert_group_message_with_origin(
        &self,
        group_id: &[u8],
        sender_sign_pk: Option<&[u8]>,
        direction: Direction,
        body: &str,
        sent_at: i64,
        expires_at: Option<i64>,
        attachments: &[NewAttachment],
        origin_msg_id: Option<i64>,
    ) -> Result<i64> {
        check_body(body)?;
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO messages (group_id, sender_sign_pk, direction, body, sent_at, expires_at, origin_msg_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![group_id, sender_sign_pk, direction.to_i64(), body, sent_at, expires_at, origin_msg_id])?;
            let id = tx.last_insert_rowid();
            insert_attachments(tx, id, attachments)?;
            Ok(id)
        })
    }

    pub fn list_messages(&self, contact_id: i64, limit: i64, before_id: Option<i64>) -> Result<Vec<Message>> {
        self.with_conn(|c| match before_id {
            Some(b) => c.prepare_cached(concat!("SELECT ", message_cols!(),
                " FROM messages WHERE contact_id = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3"))?
                .query_map(params![contact_id, b, limit], Self::map_message)?.collect_rows(),
            None => c.prepare_cached(concat!("SELECT ", message_cols!(),
                " FROM messages WHERE contact_id = ?1 ORDER BY id DESC LIMIT ?2"))?
                .query_map(params![contact_id, limit], Self::map_message)?.collect_rows(),
        })
    }

    pub fn list_group_messages(&self, group_id: &[u8], limit: i64, before_id: Option<i64>) -> Result<Vec<Message>> {
        self.with_conn(|c| match before_id {
            Some(b) => c.prepare_cached(concat!("SELECT ", message_cols!(),
                " FROM messages WHERE group_id = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3"))?
                .query_map(params![group_id, b, limit], Self::map_message)?.collect_rows(),
            None => c.prepare_cached(concat!("SELECT ", message_cols!(),
                " FROM messages WHERE group_id = ?1 ORDER BY id DESC LIMIT ?2"))?
                .query_map(params![group_id, limit], Self::map_message)?.collect_rows(),
        })
    }

    pub fn message_position_dm(&self, contact_id: i64, message_id: i64) -> Result<Option<i64>> {
        self.with_conn(|c| {
            let exists: bool = c.prepare_cached(
                "SELECT 1 FROM messages WHERE id = ?1 AND contact_id = ?2 AND group_id IS NULL")?
                .query_row(params![message_id, contact_id], |_| Ok(())).optional()?.is_some();
            if !exists { return Ok(None); }
            let n: i64 = c.prepare_cached(
                "SELECT COUNT(*) FROM messages WHERE contact_id = ?1 AND group_id IS NULL AND id > ?2")?
                .query_row(params![contact_id, message_id], |r| r.get(0))?;
            Ok(Some(n))
        })
    }

    pub fn message_position_group(&self, group_id: &[u8], message_id: i64) -> Result<Option<i64>> {
        self.with_conn(|c| {
            let exists: bool = c.prepare_cached(
                "SELECT 1 FROM messages WHERE id = ?1 AND group_id = ?2")?
                .query_row(params![message_id, group_id], |_| Ok(())).optional()?.is_some();
            if !exists { return Ok(None); }
            let n: i64 = c.prepare_cached(
                "SELECT COUNT(*) FROM messages WHERE group_id = ?1 AND id > ?2")?
                .query_row(params![group_id, message_id], |r| r.get(0))?;
            Ok(Some(n))
        })
    }

    pub fn list_unsent_outgoing(&self, contact_id: i64, limit: i64) -> Result<Vec<Message>> {
        self.with_conn(|c| c.prepare_cached(concat!("SELECT ", message_cols!(),
            " FROM messages WHERE contact_id = ?1 AND direction = 1 AND sent = 0 ORDER BY id ASC LIMIT ?2"))?
            .query_map(params![contact_id, limit], Self::map_message)?.collect_rows())
    }

    pub fn list_unacked_outgoing(
        &self, contact_id: i64, now_ms: i64, base_backoff_ms: i64, max_backoff_ms: i64, limit: i64,
    ) -> Result<Vec<Message>> {
        const MAX_SEND_ATTEMPTS: i64 = 8;
        self.with_conn(|c| c.prepare_cached(concat!("SELECT ", message_cols!(),
            " FROM messages WHERE contact_id = ?1 AND direction = 1 AND sent = 1 AND delivered = 0
               AND send_attempts < ?6
               AND (last_attempt_at IS NULL OR (?2 - last_attempt_at) >= MIN(?4, ?3 * (1 << MIN(send_attempts, 16))))
             ORDER BY id ASC LIMIT ?5"))?
            .query_map(params![contact_id, now_ms, base_backoff_ms, max_backoff_ms, limit, MAX_SEND_ATTEMPTS], Self::map_message)?
            .collect_rows())
    }

    pub fn callback_seen(&self, sender_sign_pk: &[u8], sent_at: i64, callback_data: &str) -> Result<bool> {
        self.with_conn(|c| Ok(c.query_row(
            "SELECT COUNT(*) FROM callback_seen WHERE sender_sign_pk = ?1 AND sent_at = ?2 AND callback_data = ?3",
            params![sender_sign_pk, sent_at, callback_data], |r| r.get::<_, i64>(0))? > 0))
    }

    pub fn record_callback_seen(&self, sender_sign_pk: &[u8], sent_at: i64, callback_data: &str) -> Result<bool> {
        self.with_conn(|c| Ok(c.execute(
            "INSERT OR IGNORE INTO callback_seen (sender_sign_pk, sent_at, callback_data, seen_at) VALUES (?1, ?2, ?3, ?4)",
            params![sender_sign_pk, sent_at, callback_data, now_ms()])? > 0))
    }

    pub fn purge_old_callback_seen(&self, older_than: i64) -> Result<usize> {
        self.with_conn(|c| Ok(c.execute(
            "DELETE FROM callback_seen WHERE seen_at < ?1", params![older_than])?))
    }

    pub fn notify_seen_lookup(&self, idem: &str) -> Result<Option<i64>> {
        self.with_conn(|c| c.prepare_cached("SELECT message_id FROM notify_seen WHERE idempotency_key = ?1")?
            .query_row(params![idem], |r| r.get::<_, i64>(0)).optional().map_err(Into::into))
    }

    pub fn notify_seen_record(&self, idem: &str, message_id: i64) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "INSERT OR IGNORE INTO notify_seen (idempotency_key, message_id, seen_at) VALUES (?1, ?2, ?3)",
            params![idem, message_id, now_ms()])?; Ok(()) })
    }

    pub fn notify_seen_purge(&self, older_than: i64) -> Result<usize> {
        self.with_conn(|c| Ok(c.execute("DELETE FROM notify_seen WHERE seen_at < ?1", params![older_than])?))
    }

    pub fn pending_outbound_add(&self, msg_id: i64, recipient_contact_id: i64) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "INSERT OR IGNORE INTO pending_outbound (msg_id, recipient_contact_id, send_attempts, created_at) VALUES (?1, ?2, 0, ?3)",
            params![msg_id, recipient_contact_id, now_ms()])?; Ok(()) })
    }

    pub fn pending_outbound_remove(&self, msg_id: i64, recipient_contact_id: i64) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "DELETE FROM pending_outbound WHERE msg_id = ?1 AND recipient_contact_id = ?2",
            params![msg_id, recipient_contact_id])?; Ok(()) })
    }

    pub fn pending_outbound_for_recipient(
        &self, recipient_contact_id: i64, now_ms: i64, base_backoff_ms: i64, max_backoff_ms: i64, limit: i64,
    ) -> Result<Vec<i64>> {
        const MAX_ATTEMPTS: i64 = 8;
        self.with_conn(|c| c.prepare_cached(
            "SELECT msg_id FROM pending_outbound
             WHERE recipient_contact_id = ?1
               AND send_attempts < ?5
               AND (last_attempt_at IS NULL OR (?2 - last_attempt_at) >= MIN(?4, ?3 * (1 << MIN(send_attempts, 16))))
             ORDER BY msg_id ASC LIMIT ?6")?
            .query_map(params![recipient_contact_id, now_ms, base_backoff_ms, max_backoff_ms, MAX_ATTEMPTS, limit], |r| r.get(0))?
            .collect_rows())
    }

    pub fn pending_outbound_record_attempt(&self, msg_id: i64, recipient_contact_id: i64) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "UPDATE pending_outbound SET last_attempt_at = ?3, send_attempts = send_attempts + 1
             WHERE msg_id = ?1 AND recipient_contact_id = ?2",
            params![msg_id, recipient_contact_id, now_ms()])?; Ok(()) })
    }

    pub fn add_dead_letter(
        &self, contact_id: Option<i64>, group_id: Option<&[u8]>, kind: &str, payload: &[u8], error: &str,
    ) -> Result<i64> {
        self.with_conn(|c| {
            let now = now_ms();
            c.execute(
                "INSERT INTO dead_letters (contact_id, group_id, payload, kind, error, attempts, first_seen_at, last_seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)",
                params![contact_id, group_id, payload, kind, error, now])?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn purge_old_dead_letters(&self, older_than: i64) -> Result<usize> {
        self.with_conn(|c| Ok(c.execute("DELETE FROM dead_letters WHERE last_seen_at < ?1", params![older_than])?))
    }

    pub fn mark_sent(&self, id: i64) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "UPDATE messages SET sent = 1, last_attempt_at = ?2, send_attempts = send_attempts + 1 WHERE id = ?1",
            params![id, now_ms()])?; Ok(()) })
    }

    pub fn record_send_attempt(&self, id: i64) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "UPDATE messages SET last_attempt_at = ?2, send_attempts = send_attempts + 1 WHERE id = ?1",
            params![id, now_ms()])?; Ok(()) })
    }

    pub fn mark_delivered(&self, id: i64) -> Result<()> {
        self.with_conn(|c| { c.execute("UPDATE messages SET sent = 1, delivered = 1 WHERE id = ?1", params![id])?; Ok(()) })
    }

    pub fn resync_recent(&self, contact_id: i64, within_ms: i64) -> Result<bool> {
        self.with_conn(|c| Ok(c.query_row(
            "SELECT COUNT(*) FROM resync_log WHERE contact_id = ?1 AND last_resync_at >= ?2",
            params![contact_id, now_ms() - within_ms], |r| r.get::<_, i64>(0))? > 0))
    }

    pub fn record_resync(&self, contact_id: i64) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "INSERT INTO resync_log (contact_id, last_resync_at) VALUES (?1, ?2)
             ON CONFLICT(contact_id) DO UPDATE SET last_resync_at = excluded.last_resync_at",
            params![contact_id, now_ms()])?; Ok(()) })
    }

    pub fn mark_read(&self, contact_id: i64) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "UPDATE messages SET read = 1 WHERE contact_id = ?1 AND direction = 0 AND read = 0",
            params![contact_id])?; Ok(()) })
    }

    pub fn mark_group_read(&self, group_id: &[u8]) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "UPDATE messages SET read = 1 WHERE group_id = ?1 AND direction = 0 AND read = 0",
            params![group_id])?; Ok(()) })
    }

    pub fn unread_count(&self, contact_id: i64) -> Result<i64> {
        self.with_conn(|c| Ok(c.prepare_cached(
            "SELECT COUNT(*) FROM messages WHERE contact_id = ?1 AND direction = 0 AND read = 0")?
            .query_row(params![contact_id], |r| r.get(0))?))
    }

    pub fn group_unread_count(&self, group_id: &[u8]) -> Result<i64> {
        self.with_conn(|c| Ok(c.prepare_cached(
            "SELECT COUNT(*) FROM messages WHERE group_id = ?1 AND direction = 0 AND read = 0")?
            .query_row(params![group_id], |r| r.get(0))?))
    }

    pub fn delete_message(&self, id: i64) -> Result<()> {
        self.with_conn(|c| { c.execute("DELETE FROM messages WHERE id = ?1", params![id])?; Ok(()) })
    }

    pub fn purge_expired(&self, now: i64) -> Result<usize> {
        self.with_conn(|c| Ok(c.execute(
            "DELETE FROM messages WHERE id IN (
                SELECT m.id FROM messages m
                LEFT JOIN pinned_messages p ON p.message_id = m.id
                WHERE m.expires_at IS NOT NULL AND m.expires_at > m.sent_at
                  AND m.expires_at <= ?1 AND p.message_id IS NULL
            )", params![now])?))
    }

    pub fn cleanup_orphan_pins(&self) -> Result<usize> {
        self.with_conn(|c| Ok(c.execute(
            "DELETE FROM pinned_messages WHERE message_id NOT IN (SELECT id FROM messages)", [])?))
    }

    fn map_message(r: &rusqlite::Row) -> rusqlite::Result<Message> {
        Ok(Message {
            id: r.get(0)?,
            contact_id: r.get(1)?,
            group_id: r.get(2)?,
            sender_sign_pk: r.get(3)?,
            direction: Direction::from_i64(r.get(4)?),
            body: r.get(5)?,
            sent_at: r.get(6)?,
            sent: r.get::<_, i64>(7)? != 0,
            delivered: r.get::<_, i64>(8)? != 0,
            read: r.get::<_, i64>(9)? != 0,
            expires_at: r.get(10)?,
            last_attempt_at: r.get(11)?,
            send_attempts: r.get(12)?,
            reply_to: r.get(13)?,
        })
    }

    pub fn list_attachments(&self, message_id: i64) -> Result<Vec<Attachment>> {
        self.with_conn(|c| c.prepare_cached(
            "SELECT id, message_id, name, size, key, path FROM attachments WHERE message_id = ?1")?
            .query_map(params![message_id], map_attachment)?.collect_rows())
    }

    pub fn list_attachments_for_messages(&self, ids: &[i64]) -> Result<HashMap<i64, Vec<Attachment>>> {
        if ids.is_empty() { return Ok(HashMap::new()); }
        let placeholders = (0..ids.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT id, message_id, name, size, key, path FROM attachments WHERE message_id IN ({})", placeholders);
        self.with_conn(|c| {
            let mut s = c.prepare(&sql)?;
            let mut out: HashMap<i64, Vec<Attachment>> = HashMap::new();
            for row in s.query_map(rusqlite::params_from_iter(ids.iter()), map_attachment)? {
                let a = row?;
                out.entry(a.message_id).or_default().push(a);
            }
            Ok(out)
        })
    }

    pub fn list_attachments_for_contact(&self, contact_id: i64, limit: i64) -> Result<Vec<(Attachment, i64)>> {
        self.with_conn(|c| c.prepare_cached(
            "SELECT a.id, a.message_id, a.name, a.size, a.key, a.path, m.sent_at
             FROM attachments a JOIN messages m ON m.id = a.message_id
             WHERE m.contact_id = ?1 ORDER BY m.id DESC LIMIT ?2")?
            .query_map(params![contact_id, limit], |r| Ok((map_attachment(r)?, r.get(6)?)))?
            .collect_rows())
    }

    pub fn list_all_messages(&self) -> Result<Vec<Message>> {
        self.with_conn(|c| c.prepare_cached(concat!("SELECT ", message_cols!(), " FROM messages ORDER BY id ASC"))?
            .query_map([], Self::map_message)?.collect_rows())
    }

    pub fn list_all_attachments(&self) -> Result<Vec<Attachment>> {
        self.with_conn(|c| c.prepare_cached("SELECT id, message_id, name, size, key, path FROM attachments ORDER BY id ASC")?
            .query_map([], map_attachment)?.collect_rows())
    }

    pub fn list_all_pinned(&self) -> Result<Vec<(Option<i64>, Option<Vec<u8>>, i64, i64)>> {
        self.with_conn(|c| c.prepare_cached("SELECT contact_id, group_id, message_id, pinned_at FROM pinned_messages")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect_rows())
    }

    pub fn list_all_prekeys(&self) -> Result<Vec<PreKey>> {
        self.with_conn(|c| c.prepare_cached("SELECT id, kind, private, public, created_at FROM prekeys ORDER BY id ASC")?
            .query_map([], Self::map_prekey)?.collect_rows())
    }

    pub fn list_all_settings(&self) -> Result<Vec<(String, Vec<u8>)>> {
        self.with_conn(|c| c.prepare_cached("SELECT k, v FROM settings")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect_rows())
    }

    pub fn bulk_insert_messages(&self, msgs: &[Message]) -> Result<()> {
        self.with_tx(|tx| {
            let mut s = tx.prepare_cached(
                "INSERT OR IGNORE INTO messages (id, contact_id, group_id, sender_sign_pk, direction, body, sent_at, sent, delivered, read, expires_at, last_attempt_at, send_attempts, reply_to)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)")?;
            for m in msgs {
                s.execute(params![
                    m.id, m.contact_id, m.group_id, m.sender_sign_pk, m.direction.to_i64(), m.body, m.sent_at,
                    m.sent as i64, m.delivered as i64, m.read as i64,
                    m.expires_at, m.last_attempt_at, m.send_attempts, m.reply_to,
                ])?;
            }
            Ok(())
        })
    }

    pub fn bulk_insert_attachments(&self, atts: &[Attachment]) -> Result<()> {
        self.with_tx(|tx| {
            let mut s = tx.prepare_cached(
                "INSERT OR IGNORE INTO attachments (id, message_id, name, size, key, path) VALUES (?1,?2,?3,?4,?5,?6)")?;
            for a in atts {
                s.execute(params![a.id, a.message_id, a.name, a.size, a.key, a.path])?;
            }
            Ok(())
        })
    }

    pub fn bulk_insert_pinned(&self, rows: &[(Option<i64>, Option<Vec<u8>>, i64, i64)]) -> Result<()> {
        self.with_tx(|tx| {
            let mut s = tx.prepare_cached(
                "INSERT OR IGNORE INTO pinned_messages (contact_id, group_id, message_id, pinned_at) VALUES (?1,?2,?3,?4)")?;
            for (cid, gid, mid, ts) in rows {
                s.execute(params![cid, gid, mid, ts])?;
            }
            Ok(())
        })
    }

    pub fn bulk_insert_prekeys(&self, prekeys: &[PreKey]) -> Result<()> {
        self.with_tx(|tx| {
            let mut s = tx.prepare_cached(
                "INSERT OR IGNORE INTO prekeys (id, kind, private, public, created_at) VALUES (?1,?2,?3,?4,?5)")?;
            for p in prekeys {
                s.execute(params![p.id, p.kind as i64, p.private, p.public, p.created_at])?;
            }
            Ok(())
        })
    }

    pub fn bulk_set_settings(&self, rows: &[(String, Vec<u8>)]) -> Result<()> {
        self.with_tx(|tx| {
            let mut s = tx.prepare_cached("INSERT OR REPLACE INTO settings (k, v) VALUES (?1, ?2)")?;
            for (k, v) in rows {
                s.execute(params![k, v])?;
            }
            Ok(())
        })
    }

    pub fn list_attachments_for_group(&self, group_id: &[u8], limit: i64) -> Result<Vec<(Attachment, i64)>> {
        self.with_conn(|c| c.prepare_cached(
            "SELECT a.id, a.message_id, a.name, a.size, a.key, a.path, m.sent_at
             FROM attachments a JOIN messages m ON m.id = a.message_id
             WHERE m.group_id = ?1 ORDER BY m.id DESC LIMIT ?2")?
            .query_map(params![group_id, limit], |r| Ok((map_attachment(r)?, r.get(6)?)))?
            .collect_rows())
    }

    pub fn search_messages(&self, needle: &str, contact_id: Option<i64>, group_id: Option<&[u8]>, limit: i64) -> Result<Vec<Message>> {
        let query = fts_prefix_query(needle);
        if query.is_empty() { return Ok(Vec::new()); }
        self.with_conn(|c| match (contact_id, group_id) {
            (Some(cid), _) => c.prepare_cached(concat!("SELECT ", message_cols_m!(),
                " FROM messages m JOIN messages_fts f ON f.rowid = m.id
                  WHERE messages_fts MATCH ?1 AND m.contact_id = ?2 ORDER BY m.id DESC LIMIT ?3"))?
                .query_map(params![query, cid, limit], Self::map_message)?.collect_rows(),
            (_, Some(gid)) => c.prepare_cached(concat!("SELECT ", message_cols_m!(),
                " FROM messages m JOIN messages_fts f ON f.rowid = m.id
                  WHERE messages_fts MATCH ?1 AND m.group_id = ?2 ORDER BY m.id DESC LIMIT ?3"))?
                .query_map(params![query, gid, limit], Self::map_message)?.collect_rows(),
            (None, None) => c.prepare_cached(concat!("SELECT ", message_cols_m!(),
                " FROM messages m JOIN messages_fts f ON f.rowid = m.id
                  WHERE messages_fts MATCH ?1 ORDER BY m.id DESC LIMIT ?2"))?
                .query_map(params![query, limit], Self::map_message)?.collect_rows(),
        })
    }

    pub fn get_attachment(&self, id: i64) -> Result<Option<Attachment>> {
        self.with_conn(|c| c.prepare_cached(
            "SELECT id, message_id, name, size, key, path FROM attachments WHERE id = ?1")?
            .query_row(params![id], map_attachment).optional().map_err(Into::into))
    }

    pub fn delete_attachment(&self, id: i64) -> Result<()> {
        self.with_conn(|c| { c.execute("DELETE FROM attachments WHERE id = ?1", params![id])?; Ok(()) })
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.with_conn(|c| c.prepare_cached("SELECT v FROM settings WHERE k = ?1")?
            .query_row(params![key], |r| r.get::<_, Vec<u8>>(0)).optional().map_err(Into::into))
    }

    pub fn load_buttons_batch(&self, ids: &[i64]) -> Result<HashMap<i64, Vec<u8>>> {
        if ids.is_empty() { return Ok(HashMap::new()); }
        let keys: Vec<String> = ids.iter().map(|i| format!("buttons_{}", i)).collect();
        let placeholders = (0..keys.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT k, v FROM settings WHERE k IN ({})", placeholders);
        self.with_conn(|c| {
            let mut s = c.prepare(&sql)?;
            let mut out: HashMap<i64, Vec<u8>> = HashMap::with_capacity(keys.len());
            for row in s.query_map(rusqlite::params_from_iter(keys.iter()), |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)))? {
                let (k, v) = row?;
                if let Some(mid) = k.strip_prefix("buttons_").and_then(|s| s.parse::<i64>().ok()) {
                    out.insert(mid, v);
                }
            }
            Ok(out)
        })
    }

    pub fn set_setting(&self, key: &str, value: &[u8]) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "INSERT INTO settings (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            params![key, value])?; Ok(()) })
    }

    pub fn delete_setting(&self, key: &str) -> Result<()> {
        self.with_conn(|c| { c.execute("DELETE FROM settings WHERE k = ?1", params![key])?; Ok(()) })
    }

    pub fn create_group(&self, id: &[u8], name: &str) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "INSERT OR IGNORE INTO groups (id, name, created_at) VALUES (?1, ?2, ?3)",
            params![id, name, now_ms()])?; Ok(()) })
    }

    pub fn update_group_name(&self, id: &[u8], name: &str) -> Result<()> {
        self.with_conn(|c| { c.execute("UPDATE groups SET name = ?1 WHERE id = ?2", params![name, id])?; Ok(()) })
    }

    pub fn get_group(&self, id: &[u8]) -> Result<Option<Group>> {
        self.with_conn(|c| c.prepare_cached(concat!(select_group!(), " WHERE id = ?1"))?
            .query_row(params![id], map_group).optional().map_err(Into::into))
    }

    pub fn list_groups(&self) -> Result<Vec<Group>> {
        self.with_conn(|c| c.prepare_cached(concat!(
            select_group!(),
            " ORDER BY pinned_at IS NULL, pinned_at DESC, last_message_at IS NULL, last_message_at DESC, name"
        ))?
            .query_map([], map_group)?.collect_rows())
    }

    pub fn delete_group(&self, id: &[u8]) -> Result<()> {
        self.with_conn(|c| { c.execute("DELETE FROM groups WHERE id = ?1", params![id])?; Ok(()) })
    }

    pub fn delete_prekey(&self, id: i64) -> Result<()> {
        self.with_conn(|c| { c.execute("DELETE FROM prekeys WHERE id = ?1", params![id])?; Ok(()) })
    }

    pub fn count_prekeys(&self, kind: PreKeyKind) -> Result<i64> {
        self.with_conn(|c| Ok(c.query_row(
            "SELECT COUNT(*) FROM prekeys WHERE kind = ?1",
            params![kind.to_i64()], |r| r.get(0))?))
    }

    pub fn take_oldest_prekey(&self, kind: PreKeyKind) -> Result<Option<PreKey>> {
        self.with_tx(|tx| {
            let pk: Option<PreKey> = tx
                .prepare_cached("SELECT id, kind, private, public, created_at FROM prekeys WHERE kind = ?1 ORDER BY id ASC LIMIT 1")?
                .query_row(params![kind.to_i64()], Self::map_prekey).optional()?;
            if let Some(ref p) = pk { tx.execute("DELETE FROM prekeys WHERE id = ?1", params![p.id])?; }
            Ok(pk)
        })
    }

    pub fn peek_oldest_prekey(&self, kind: PreKeyKind) -> Result<Option<PreKey>> {
        self.with_conn(|c| c.prepare_cached("SELECT id, kind, private, public, created_at FROM prekeys WHERE kind = ?1 ORDER BY id ASC LIMIT 1")?
            .query_row(params![kind.to_i64()], Self::map_prekey).optional().map_err(Into::into))
    }

    pub fn get_group_name(&self, id: &[u8]) -> Result<Option<String>> {
        self.with_conn(|c| c.prepare_cached("SELECT name FROM groups WHERE id = ?1")?
            .query_row(params![id], |r| r.get::<_, String>(0)).optional().map_err(Into::into))
    }

    pub fn is_group_member(&self, group_id: &[u8], sign_pk: &[u8]) -> Result<bool> {
        self.with_conn(|c| Ok(c.query_row(
            "SELECT COUNT(*) FROM group_members WHERE group_id = ?1 AND sign_pk = ?2",
            params![group_id, sign_pk], |r| r.get::<_, i64>(0))? > 0))
    }

    pub fn find_contact_by_sign_pk(&self, sign: &[u8]) -> Result<Option<Contact>> {
        self.find_contact_by_sign(sign)
    }

    pub fn add_group_member(
        &self, group_id: &[u8], sign_pk: &[u8], dh_pk: &[u8], onion: &str, name: &str, is_self: bool,
    ) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "INSERT OR REPLACE INTO group_members (group_id, sign_pk, dh_pk, onion, display_name, is_self)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![group_id, sign_pk, dh_pk, onion, name, is_self as i64])?; Ok(()) })
    }

    pub fn update_group_member_name(&self, group_id: &[u8], sign_pk: &[u8], name: &str) -> Result<bool> {
        self.with_conn(|c| Ok(c.execute(
            "UPDATE group_members SET display_name = ?1
             WHERE group_id = ?2 AND sign_pk = ?3 AND display_name <> ?1",
            params![name, group_id, sign_pk])? > 0))
    }

    pub fn list_groups_with_member(&self, sign_pk: &[u8]) -> Result<Vec<Vec<u8>>> {
        self.with_conn(|c| c.prepare_cached("SELECT group_id FROM group_members WHERE sign_pk = ?1")?
            .query_map(params![sign_pk], |r| r.get::<_, Vec<u8>>(0))?.collect_rows())
    }

    pub fn list_group_members(&self, group_id: &[u8]) -> Result<Vec<GroupMember>> {
        self.with_conn(|c| c.prepare_cached(
            "SELECT group_id, sign_pk, dh_pk, onion, display_name, is_self FROM group_members WHERE group_id = ?1 ORDER BY display_name")?
            .query_map(params![group_id], map_group_member)?.collect_rows())
    }

    pub fn list_all_group_members(&self) -> Result<HashMap<Vec<u8>, Vec<GroupMember>>> {
        self.with_conn(|c| {
            let mut s = c.prepare_cached(
                "SELECT group_id, sign_pk, dh_pk, onion, display_name, is_self FROM group_members ORDER BY group_id, display_name")?;
            let mut out: HashMap<Vec<u8>, Vec<GroupMember>> = HashMap::new();
            for row in s.query_map([], map_group_member)? {
                let m = row?;
                out.entry(m.group_id.clone()).or_default().push(m);
            }
            Ok(out)
        })
    }

    pub fn bulk_add_group_members(&self, members: &[GroupMember]) -> Result<()> {
        self.with_tx(|tx| {
            let mut s = tx.prepare_cached(
                "INSERT OR REPLACE INTO group_members (group_id, sign_pk, dh_pk, onion, display_name, is_self)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
            for m in members {
                s.execute(params![m.group_id, m.sign_pk, m.dh_pk, m.onion, m.display_name, m.is_self as i64])?;
            }
            Ok(())
        })
    }

    pub fn bulk_update_contacts(&self, rows: &[(i64, String, TrustLevel, bool)]) -> Result<()> {
        self.with_tx(|tx| {
            let mut s = tx.prepare_cached(
                "UPDATE contacts SET display_name = ?2, trust = ?3, is_bot = ?4 WHERE id = ?1")?;
            for (id, name, trust, is_bot) in rows {
                s.execute(params![id, name, trust.to_i64(), *is_bot as i64])?;
            }
            Ok(())
        })
    }

    pub fn pin_contact_message(&self, contact_id: i64, message_id: i64) -> Result<bool> {
        self.with_conn(|c| Ok(c.execute(
            "INSERT OR IGNORE INTO pinned_messages (contact_id, message_id, pinned_at) VALUES (?1, ?2, ?3)",
            params![contact_id, message_id, now_ms()])? > 0))
    }

    pub fn pin_group_message(&self, group_id: &[u8], message_id: i64) -> Result<bool> {
        self.with_conn(|c| Ok(c.execute(
            "INSERT OR IGNORE INTO pinned_messages (group_id, message_id, pinned_at) VALUES (?1, ?2, ?3)",
            params![group_id, message_id, now_ms()])? > 0))
    }

    pub fn unpin_contact_message(&self, contact_id: i64, message_id: i64) -> Result<bool> {
        self.with_conn(|c| Ok(c.execute(
            "DELETE FROM pinned_messages WHERE contact_id = ?1 AND message_id = ?2",
            params![contact_id, message_id])? > 0))
    }

    pub fn unpin_group_message(&self, group_id: &[u8], message_id: i64) -> Result<bool> {
        self.with_conn(|c| Ok(c.execute(
            "DELETE FROM pinned_messages WHERE group_id = ?1 AND message_id = ?2",
            params![group_id, message_id])? > 0))
    }

    pub fn is_contact_message_pinned(&self, contact_id: i64, message_id: i64) -> Result<bool> {
        self.with_conn(|c| Ok(c.query_row(
            "SELECT COUNT(*) FROM pinned_messages WHERE contact_id = ?1 AND message_id = ?2",
            params![contact_id, message_id], |r| r.get::<_, i64>(0))? > 0))
    }

    pub fn is_group_message_pinned(&self, group_id: &[u8], message_id: i64) -> Result<bool> {
        self.with_conn(|c| Ok(c.query_row(
            "SELECT COUNT(*) FROM pinned_messages WHERE group_id = ?1 AND message_id = ?2",
            params![group_id, message_id], |r| r.get::<_, i64>(0))? > 0))
    }

    pub fn list_pinned_contact(&self, contact_id: i64) -> Result<Vec<Message>> {
        self.with_conn(|c| c.prepare_cached(concat!("SELECT ", message_cols_m!(),
            " FROM messages m INNER JOIN pinned_messages p ON p.message_id = m.id
              WHERE p.contact_id = ?1 ORDER BY p.pinned_at DESC"))?
            .query_map(params![contact_id], Self::map_message)?.collect_rows())
    }

    pub fn list_pinned_group(&self, group_id: &[u8]) -> Result<Vec<Message>> {
        self.with_conn(|c| c.prepare_cached(concat!("SELECT ", message_cols_m!(),
            " FROM messages m INNER JOIN pinned_messages p ON p.message_id = m.id
              WHERE p.group_id = ?1 ORDER BY p.pinned_at DESC"))?
            .query_map(params![group_id], Self::map_message)?.collect_rows())
    }

    pub fn add_deferred_pin_contact(&self, contact_id: i64, sender_sign_pk: &[u8], origin_msg_id: i64, unpin: bool) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "INSERT INTO deferred_pins (contact_id, sender_sign_pk, origin_msg_id, unpin, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![contact_id, sender_sign_pk, origin_msg_id, unpin as i64, now_ms()])?; Ok(()) })
    }

    pub fn add_deferred_pin_group(&self, group_id: &[u8], sender_sign_pk: &[u8], origin_msg_id: i64, unpin: bool) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "INSERT INTO deferred_pins (group_id, sender_sign_pk, origin_msg_id, unpin, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![group_id, sender_sign_pk, origin_msg_id, unpin as i64, now_ms()])?; Ok(()) })
    }

    pub fn take_deferred_pin_contact(&self, contact_id: i64, sender_sign_pk: &[u8], origin_msg_id: i64) -> Result<Option<bool>> {
        self.with_tx(|tx| {
            let row: Option<(i64, i64)> = tx
                .prepare_cached(
                    "SELECT id, unpin FROM deferred_pins
                     WHERE contact_id = ?1 AND sender_sign_pk = ?2 AND origin_msg_id = ?3
                     ORDER BY id ASC LIMIT 1")?
                .query_row(params![contact_id, sender_sign_pk, origin_msg_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            if let Some((id, u)) = row {
                tx.execute("DELETE FROM deferred_pins WHERE id = ?1", params![id])?;
                Ok(Some(u != 0))
            } else { Ok(None) }
        })
    }

    pub fn take_deferred_pin_group(&self, group_id: &[u8], sender_sign_pk: &[u8], origin_msg_id: i64) -> Result<Option<bool>> {
        self.with_tx(|tx| {
            let row: Option<(i64, i64)> = tx
                .prepare_cached(
                    "SELECT id, unpin FROM deferred_pins
                     WHERE group_id = ?1 AND sender_sign_pk = ?2 AND origin_msg_id = ?3
                     ORDER BY id ASC LIMIT 1")?
                .query_row(params![group_id, sender_sign_pk, origin_msg_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            if let Some((id, u)) = row {
                tx.execute("DELETE FROM deferred_pins WHERE id = ?1", params![id])?;
                Ok(Some(u != 0))
            } else { Ok(None) }
        })
    }

    pub fn purge_old_deferred_pins(&self, older_than: i64) -> Result<usize> {
        self.with_conn(|c| Ok(c.execute(
            "DELETE FROM deferred_pins WHERE created_at < ?1", params![older_than])?))
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn check_body(body: &str) -> Result<()> {
    if body.len() > MAX_BODY_BYTES { Err(DbError::TooLarge) } else { Ok(()) }
}

fn map_group(r: &Row<'_>) -> rusqlite::Result<Group> {
    Ok(Group {
        id: r.get(0)?, name: r.get(1)?, created_at: r.get(2)?,
        pinned_at: r.get(3)?, last_message_at: r.get(4)?,
    })
}

fn map_group_member(r: &Row<'_>) -> rusqlite::Result<GroupMember> {
    Ok(GroupMember {
        group_id: r.get(0)?, sign_pk: r.get(1)?, dh_pk: r.get(2)?,
        onion: r.get(3)?, display_name: r.get(4)?, is_self: r.get::<_, i64>(5)? != 0,
    })
}

fn map_attachment(r: &Row<'_>) -> rusqlite::Result<Attachment> {
    Ok(Attachment {
        id: r.get(0)?, message_id: r.get(1)?, name: r.get(2)?,
        size: r.get(3)?, key: r.get(4)?, path: r.get(5)?,
    })
}

fn insert_attachments(tx: &Transaction<'_>, message_id: i64, attachments: &[NewAttachment]) -> Result<()> {
    if attachments.is_empty() { return Ok(()); }
    let mut s = tx.prepare_cached(
        "INSERT INTO attachments (message_id, name, size, key, path) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for a in attachments {
        s.execute(params![message_id, a.name, a.size, a.key, a.path])?;
    }
    Ok(())
}

fn fts_prefix_query(needle: &str) -> String {
    needle.split_whitespace()
        .filter(|t| t.chars().any(|c| c.is_alphanumeric()))
        .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b { s.push_str(&format!("{:02x}", x)); }
    s
}