use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

pub const BUNDLE_TTL_MS: i64 = 30 * 24 * 3600 * 1000;
pub const MESSAGE_TTL_MS: i64 = 14 * 24 * 3600 * 1000;
pub const PENDING_LIMIT: i64 = 200;
pub const MAX_PER_RECIPIENT: i64 = 50_000;
pub const MAX_TOTAL_RECORDS: i64 = 200_000;
pub const MAX_TOTAL_BYTES: i64 = 512 * 1024 * 1024;
const MESSAGE_METADATA_BYTES: i64 = 96;
const BUNDLE_METADATA_BYTES: i64 = 64;

fn message_cost(blob_len: i64) -> i64 {
    blob_len.saturating_add(MESSAGE_METADATA_BYTES)
}

pub struct Storage {
    conn: Mutex<Connection>,
    limits: Limits,
}

#[derive(Clone, Copy)]
struct Limits {
    max_total_records: i64,
    max_total_bytes: i64,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_total_records: MAX_TOTAL_RECORDS, max_total_bytes: MAX_TOTAL_BYTES }
    }
}

impl Storage {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        Self::open_with_limits(path, Limits::default())
    }

    fn open_with_limits(path: &Path, limits: Limits) -> anyhow::Result<Self> {
        if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
        let conn = Connection::open(path)?;
        conn.execute_batch("
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            CREATE TABLE IF NOT EXISTS bundles (
                sign_pk BLOB PRIMARY KEY,
                bundle BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                recipient_pk BLOB NOT NULL,
                blob BLOB NOT NULL,
                deposited_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_messages_recipient ON messages(recipient_pk, id);
            CREATE INDEX IF NOT EXISTS idx_messages_deposited ON messages(deposited_at);
        ")?;
        let has_sender: bool = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name='sender_pk'",
            [], |r| r.get::<_, i64>(0).map(|v| v > 0),
        ).unwrap_or(false);
        if has_sender {
            conn.execute_batch("
                BEGIN;
                CREATE TABLE messages_new (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    recipient_pk BLOB NOT NULL,
                    blob BLOB NOT NULL,
                    deposited_at INTEGER NOT NULL
                );
                INSERT INTO messages_new (id, recipient_pk, blob, deposited_at)
                    SELECT id, recipient_pk, blob, deposited_at FROM messages;
                DROP TABLE messages;
                ALTER TABLE messages_new RENAME TO messages;
                CREATE INDEX IF NOT EXISTS idx_messages_recipient ON messages(recipient_pk, id);
                CREATE INDEX IF NOT EXISTS idx_messages_deposited ON messages(deposited_at);
                COMMIT;
            ")?;
        }
        Ok(Self { conn: Mutex::new(conn), limits })
    }

    pub fn store_bundle(&self, pk: &[u8], bundle: &[u8]) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO bundles (sign_pk, bundle, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(sign_pk) DO UPDATE SET bundle = excluded.bundle, updated_at = excluded.updated_at",
            params![pk, bundle, now_ms()],
        )?;
        trim_global_limits(&conn, self.limits)?;
        Ok(())
    }

    pub fn get_bundle(&self, pk: &[u8]) -> anyhow::Result<Option<Vec<u8>>> {
        let conn = self.conn.lock().unwrap();
        let mut s = conn.prepare_cached("SELECT bundle FROM bundles WHERE sign_pk = ?1")?;
        let row: Option<Vec<u8>> = s.query_row(params![pk], |r| r.get(0)).optional()?;
        Ok(row)
    }

    pub fn deposit(&self, to: &[u8], blob: &[u8]) -> anyhow::Result<i64> {
        if blob.is_empty() {
            anyhow::bail!("invalid message blob");
        }
        if message_cost(blob.len() as i64) > self.limits.max_total_bytes {
            anyhow::bail!("message larger than relay total budget");
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO messages (recipient_pk, blob, deposited_at) VALUES (?1, ?2, ?3)",
            params![to, blob, now_ms()],
        )?;
        let id = conn.last_insert_rowid();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE recipient_pk = ?1",
            params![to], |r| r.get(0),
        )?;
        if count > MAX_PER_RECIPIENT {
            let drop_n = count - MAX_PER_RECIPIENT;
            conn.execute(
                "DELETE FROM messages WHERE id IN (
                   SELECT id FROM messages WHERE recipient_pk = ?1 ORDER BY id ASC LIMIT ?2
                 )",
                params![to, drop_n],
            )?;
        }
        trim_global_limits(&conn, self.limits)?;
        Ok(id)
    }

    pub fn pending_for(&self, pk: &[u8]) -> anyhow::Result<Vec<(i64, Vec<u8>)>> {
        self.pending_above(pk, 0)
    }

    pub fn pending_above(&self, pk: &[u8], cursor: i64) -> anyhow::Result<Vec<(i64, Vec<u8>)>> {
        let conn = self.conn.lock().unwrap();
        let mut s = conn.prepare_cached(
            "SELECT id, blob FROM messages
             WHERE recipient_pk = ?1 AND id > ?2
             ORDER BY id ASC LIMIT ?3",
        )?;
        let rows = s.query_map(params![pk, cursor, PENDING_LIMIT], |r| {
            let id: i64 = r.get(0)?;
            let blob: Vec<u8> = r.get(1)?;
            Ok((id, blob))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Delete message `id`, but only from `pk`'s own inbox.
    ///
    /// This used to delete by id alone. Ids are sequential, so any client that
    /// could authenticate — any keypair at all — could erase everyone's queued
    /// mail by acking 1, 2, 3, ... An ack is only the recipient's to give.
    pub fn ack(&self, pk: &[u8], id: i64) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM messages WHERE id = ?1 AND recipient_pk = ?2", params![id, pk])?;
        Ok(())
    }

    pub fn gc(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms();
        conn.execute("DELETE FROM messages WHERE deposited_at < ?1", params![now - MESSAGE_TTL_MS])?;
        conn.execute("DELETE FROM bundles WHERE updated_at < ?1", params![now - BUNDLE_TTL_MS])?;
        Ok(())
    }

    pub fn stats(&self) -> anyhow::Result<(i64, i64)> {
        let conn = self.conn.lock().unwrap();
        let bundles: i64 = conn.query_row("SELECT COUNT(*) FROM bundles", [], |r| r.get(0))?;
        let messages: i64 = conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;
        Ok((bundles, messages))
    }
}

fn totals(conn: &Connection) -> anyhow::Result<(i64, i64, i64, i64)> {
    let messages: i64 = conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;
    let bundles: i64 = conn.query_row("SELECT COUNT(*) FROM bundles", [], |r| r.get(0))?;
    let message_bytes: i64 = conn.query_row("SELECT COALESCE(SUM(LENGTH(blob)), 0) FROM messages", [], |r| r.get(0))?;
    let bundle_bytes: i64 = conn.query_row("SELECT COALESCE(SUM(LENGTH(bundle)), 0) FROM bundles", [], |r| r.get(0))?;
    Ok((messages, bundles, message_bytes, bundle_bytes))
}

fn trim_global_limits(conn: &Connection, limits: Limits) -> anyhow::Result<()> {
    loop {
        let (messages, bundles, message_bytes, bundle_bytes) = totals(conn)?;
        let records = messages.saturating_add(bundles);
        let bytes = message_bytes
            .saturating_add(bundle_bytes)
            .saturating_add(messages.saturating_mul(MESSAGE_METADATA_BYTES))
            .saturating_add(bundles.saturating_mul(BUNDLE_METADATA_BYTES));
        if records <= limits.max_total_records && bytes <= limits.max_total_bytes {
            return Ok(());
        }

        let dropped_message = conn.execute(
            "DELETE FROM messages WHERE id = (SELECT id FROM messages ORDER BY id ASC LIMIT 1)",
            [],
        )?;
        if dropped_message > 0 {
            continue;
        }
        let dropped_bundle = conn.execute(
            "DELETE FROM bundles WHERE sign_pk = (
               SELECT sign_pk FROM bundles ORDER BY updated_at ASC, sign_pk ASC LIMIT 1
             )",
            [],
        )?;
        if dropped_bundle == 0 {
            return Ok(());
        }
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny(limit_records: i64, limit_bytes: i64) -> Storage {
        Storage::open_with_limits(Path::new(":memory:"), Limits { max_total_records: limit_records, max_total_bytes: limit_bytes }).unwrap()
    }

    #[test]
    fn ack_deletes_only_from_the_acking_recipients_inbox() {
        let s = Storage::open(Path::new(":memory:")).unwrap();
        let (alice, mallory) = ([0xA_u8; 32], [0xE_u8; 32]);
        let id = s.deposit(&alice, b"for alice").unwrap();

        s.ack(&mallory, id).unwrap();
        assert_eq!(s.pending_for(&alice).unwrap().len(), 1, "a stranger's ack must not delete");

        s.ack(&alice, id).unwrap();
        assert!(s.pending_for(&alice).unwrap().is_empty());
    }

    #[test]
    fn empty_message_blob_is_rejected() {
        let s = Storage::open(Path::new(":memory:")).unwrap();
        let alice = [0xA_u8; 32];
        assert!(s.deposit(&alice, b"").is_err());
        assert!(s.pending_for(&alice).unwrap().is_empty());
    }

    #[test]
    fn global_record_cap_keeps_many_tiny_messages_bounded() {
        let s = tiny(3, 1 << 20);
        let alice = [0xA_u8; 32];
        for _ in 0..8 {
            let _ = s.deposit(&alice, b"x").unwrap();
        }
        let conn = s.conn.lock().unwrap();
        let messages: i64 = conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0)).unwrap();
        assert!(messages <= 3);
    }

    #[test]
    fn global_byte_cap_counts_metadata_for_many_small_messages() {
        let cap = message_cost(1) * 2 + 1;
        let s = tiny(100, cap);
        let alice = [0xA_u8; 32];
        for _ in 0..5 {
            let _ = s.deposit(&alice, b"x").unwrap();
        }
        let conn = s.conn.lock().unwrap();
        let messages: i64 = conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0)).unwrap();
        let payload: i64 = conn.query_row("SELECT COALESCE(SUM(LENGTH(blob)), 0) FROM messages", [], |r| r.get(0)).unwrap();
        let budget = payload.saturating_add(messages.saturating_mul(MESSAGE_METADATA_BYTES));
        assert!(budget <= cap);
        assert_eq!(messages, 2);
    }

    #[test]
    fn global_record_cap_applies_to_bundles_too() {
        let s = tiny(2, 1 << 20);
        s.store_bundle(&[1u8; 32], b"a").unwrap();
        s.store_bundle(&[2u8; 32], b"b").unwrap();
        s.store_bundle(&[3u8; 32], b"c").unwrap();
        let conn = s.conn.lock().unwrap();
        let bundles: i64 = conn.query_row("SELECT COUNT(*) FROM bundles", [], |r| r.get(0)).unwrap();
        assert_eq!(bundles, 2);
    }
}
