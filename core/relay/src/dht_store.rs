//! SQLite-backed DHT storage for the standalone seed node.
//!
//! A relay started with `--dht` stores DHT items for others between restarts. This module implements the `gipny_dht::store::Storage`
//! trait against the SQLite database that the relay already opens.
//!
//! Why here and not in `dht/`: the standalone relay lives in its own workspace
//! and cannot link `rusqlite` from the main workspace (the `links = "sqlite3"`
//! manifest key is workspace-wide). So both the relay and the DHT storage go
//! here, in the relay's workspace, which already pulls in `rusqlite`.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection};

use gipny_dht::items::delete_hash;
use gipny_dht::proto::{StoredItem, MAX_VALUE_BYTES};
use gipny_dht::store::{admit, StoreError, StoreLimits, StoreStats, Storage};

pub struct SqliteStorage {
    conn: Mutex<Connection>,
    limits: StoreLimits,
}

impl SqliteStorage {
    /// Open (or create) the DHT item store at `path`.
    pub fn open(path: &Path, limits: StoreLimits) -> anyhow::Result<Self> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            CREATE TABLE IF NOT EXISTS dht_items (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                key         BLOB NOT NULL,
                value       BLOB NOT NULL,
                value_hash  BLOB NOT NULL,
                delete_hash BLOB,
                expires_at  INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_dht_key ON dht_items(key, id);
            CREATE INDEX IF NOT EXISTS idx_dht_expires ON dht_items(expires_at);
        ")?;
        Ok(Self { conn: Mutex::new(conn), limits })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl Storage for SqliteStorage {
    fn put(&self, item: StoredItem, now_ms: u64) -> Result<(), StoreError> {
        let item = admit(item, &self.limits, now_ms)?;
        if item.value.len() > MAX_VALUE_BYTES {
            return Err(StoreError::TooLarge);
        }

        let value_hash: [u8; 32] = gipny_dht::crypto::sha256(&[&item.value]);
        let conn = self.lock();

        // Idempotent: same key + same value hash is a no-op.
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM dht_items WHERE key = ?1 AND value_hash = ?2",
                params![&item.key[..], &value_hash[..]],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false);
        if exists {
            return Ok(());
        }

        // Evict oldest per-key entries when the key is full.
        let key_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dht_items WHERE key = ?1",
                params![&item.key[..]],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let max = self.limits.max_items_per_key as i64;
        if key_count >= max {
            let drop_n = key_count - max + 1;
            conn.execute(
                "DELETE FROM dht_items WHERE id IN (
                   SELECT id FROM dht_items WHERE key = ?1 ORDER BY id ASC LIMIT ?2
                 )",
                params![&item.key[..], drop_n],
            )
            .ok();
        }

        // Evict globally oldest items when the total budget is exceeded.
        let total_bytes: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(LENGTH(value)), 0) FROM dht_items",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let budget = self.limits.max_total_bytes as i64;
        if total_bytes + item.value.len() as i64 > budget {
            // Drop the oldest items until there is room; each round drops one.
            let mut remaining = total_bytes + item.value.len() as i64 - budget;
            while remaining > 0 {
                let (id, len): (i64, i64) = match conn.query_row(
                    "SELECT id, LENGTH(value) FROM dht_items ORDER BY id ASC LIMIT 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                ) {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                conn.execute("DELETE FROM dht_items WHERE id = ?1", params![id]).ok();
                remaining -= len;
            }
        }

        let delete_hash_bytes: Option<Vec<u8>> = item.delete_hash.map(|h| h.to_vec());
        conn.execute(
            "INSERT INTO dht_items (key, value, value_hash, delete_hash, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &item.key[..],
                &item.value[..],
                &value_hash[..],
                delete_hash_bytes.as_deref(),
                item.expires_at_ms as i64,
            ],
        )
        .map_err(|e| StoreError::Backend(e.to_string()))?;
        Ok(())
    }

    fn get(&self, key: &[u8; 32], now_ms: u64) -> Vec<StoredItem> {
        let conn = self.lock();
        let mut stmt = match conn.prepare_cached(
            "SELECT value, delete_hash, expires_at FROM dht_items
             WHERE key = ?1 AND expires_at > ?2
             ORDER BY id ASC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![&key[..], now_ms as i64], |r| {
            let value: Vec<u8> = r.get(0)?;
            let dh: Option<Vec<u8>> = r.get(1)?;
            let expires_at: i64 = r.get(2)?;
            Ok((value, dh, expires_at))
        })
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|r| r.ok())
        .map(|(value, dh, exp)| StoredItem {
            key: *key,
            value,
            delete_hash: dh.and_then(|b| b.try_into().ok()),
            expires_at_ms: exp as u64,
        })
        .collect()
    }

    fn delete(&self, key: &[u8; 32], token: &[u8; 32]) -> u32 {
        let wanted = delete_hash(token);
        let conn = self.lock();
        conn.execute(
            "DELETE FROM dht_items WHERE key = ?1 AND delete_hash = ?2",
            params![&key[..], &wanted[..]],
        )
        .map(|n| n as u32)
        .unwrap_or(0)
    }

    fn gc(&self, now_ms: u64) {
        let conn = self.lock();
        conn.execute(
            "DELETE FROM dht_items WHERE expires_at <= ?1",
            params![now_ms as i64],
        )
        .ok();
    }

    fn all(&self, now_ms: u64) -> Vec<StoredItem> {
        let conn = self.lock();
        let mut stmt = match conn.prepare_cached(
            "SELECT key, value, delete_hash, expires_at FROM dht_items WHERE expires_at > ?1 ORDER BY id ASC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![now_ms as i64], |r| {
            let key_bytes: Vec<u8> = r.get(0)?;
            let value: Vec<u8> = r.get(1)?;
            let dh: Option<Vec<u8>> = r.get(2)?;
            let exp: i64 = r.get(3)?;
            Ok((key_bytes, value, dh, exp))
        })
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|r| r.ok())
        .filter_map(|(k, v, dh, exp)| {
            let key: [u8; 32] = k.try_into().ok()?;
            Some(StoredItem {
                key,
                value: v,
                delete_hash: dh.and_then(|b| b.try_into().ok()),
                expires_at_ms: exp as u64,
            })
        })
        .collect()
    }

    fn stats(&self) -> StoreStats {
        let conn = self.lock();
        let items = conn
            .query_row("SELECT COUNT(*) FROM dht_items", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as usize;
        let bytes = conn
            .query_row(
                "SELECT COALESCE(SUM(LENGTH(value)), 0) FROM dht_items",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0) as usize;
        StoreStats { items, bytes }
    }
}

// Peers table: remembers DHT nodes that answered us, for cold bootstrap.
pub struct PeerStore {
    conn: Mutex<Connection>,
}

impl PeerStore {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            CREATE TABLE IF NOT EXISTS dht_peers (
                destination TEXT PRIMARY KEY,
                first_seen  INTEGER NOT NULL,
                last_ok     INTEGER NOT NULL
            );
        ")?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Load up to `limit` destinations ordered by last seen descending.
    pub fn load(&self, limit: usize) -> Vec<String> {
        let conn = self.lock();
        let mut stmt = match conn.prepare_cached(
            "SELECT destination FROM dht_peers ORDER BY last_ok DESC LIMIT ?1",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![limit as i64], |r| r.get::<_, String>(0))
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|r| r.ok())
            .collect()
    }

    pub fn save(&self, peers: &[gipny_dht::node::KnownPeer]) {
        let conn = self.lock();
        for p in peers {
            conn.execute(
                "INSERT INTO dht_peers (destination, first_seen, last_ok) VALUES (?1, ?2, ?3)
                 ON CONFLICT(destination) DO UPDATE SET last_ok = excluded.last_ok",
                params![&p.info.destination, p.first_seen_ms as i64, p.last_ok_ms as i64],
            )
            .ok();
        }
        // Keep at most 2048 peers.
        conn.execute(
            "DELETE FROM dht_peers WHERE destination NOT IN (
               SELECT destination FROM dht_peers ORDER BY last_ok DESC LIMIT 2048
             )",
            [],
        )
        .ok();
    }

    pub fn gc_old(&self, older_than_ms: u64) {
        let conn = self.lock();
        conn.execute(
            "DELETE FROM dht_peers WHERE last_ok < ?1",
            params![older_than_ms as i64],
        )
        .ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(key: u8, value: &[u8], expires: u64) -> StoredItem {
        StoredItem { key: [key; 32], value: value.to_vec(), delete_hash: None, expires_at_ms: expires }
    }

    // Same cases as libcore's DbStorage test: the two stores must agree.
    #[test]
    fn sqlite_storage_behaves_like_the_memory_one() {
        let limits = StoreLimits { max_items_per_key: 2, max_total_bytes: 10, ..Default::default() };
        let s = SqliteStorage::open(Path::new(":memory:"), limits).unwrap();
        let now = 1_000;

        s.put(item(1, b"aaa", now + 100), now).unwrap();
        s.put(item(1, b"aaa", now + 100), now).unwrap(); // identical: no-op
        s.put(item(1, b"bbb", now + 100), now).unwrap();
        s.put(item(1, b"ccc", now + 100), now).unwrap(); // evicts "aaa" under the key
        let got: Vec<Vec<u8>> = s.get(&[1; 32], now).into_iter().map(|i| i.value).collect();
        assert_eq!(got, vec![b"bbb".to_vec(), b"ccc".to_vec()]);

        s.put(item(2, b"dddddd", now + 100), now).unwrap(); // 12 bytes > 10: evicts "bbb"
        assert_eq!(s.stats(), StoreStats { items: 2, bytes: 9 });

        assert_eq!(s.put(item(3, b"x", now), now), Err(StoreError::Expired));

        let token = [7u8; 32];
        let mut deletable = item(4, b"z", now + 50);
        deletable.delete_hash = Some(delete_hash(&token));
        s.put(deletable, now).unwrap();
        assert_eq!(s.delete(&[4; 32], &[8; 32]), 0);
        assert_eq!(s.delete(&[4; 32], &token), 1);

        s.gc(now + 100);
        assert!(s.all(now + 100).is_empty());
    }
}
