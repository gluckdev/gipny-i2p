use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

pub const BUNDLE_TTL_MS: i64 = 30 * 24 * 3600 * 1000;
pub const MESSAGE_TTL_MS: i64 = 14 * 24 * 3600 * 1000;
pub const PENDING_LIMIT: i64 = 200;
pub const MAX_PER_RECIPIENT: i64 = 50_000;

pub struct Storage {
    conn: Mutex<Connection>,
}

impl Storage {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
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
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn store_bundle(&self, pk: &[u8], bundle: &[u8]) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO bundles (sign_pk, bundle, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(sign_pk) DO UPDATE SET bundle = excluded.bundle, updated_at = excluded.updated_at",
            params![pk, bundle, now_ms()],
        )?;
        Ok(())
    }

    pub fn get_bundle(&self, pk: &[u8]) -> anyhow::Result<Option<Vec<u8>>> {
        let conn = self.conn.lock().unwrap();
        let mut s = conn.prepare_cached("SELECT bundle FROM bundles WHERE sign_pk = ?1")?;
        let row: Option<Vec<u8>> = s.query_row(params![pk], |r| r.get(0)).optional()?;
        Ok(row)
    }

    pub fn deposit(&self, to: &[u8], blob: &[u8]) -> anyhow::Result<i64> {
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

    pub fn ack(&self, id: i64) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM messages WHERE id = ?1", params![id])?;
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

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}