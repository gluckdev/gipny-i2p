//! DHT items, peer snapshots, and received-envelope deduplication.

use super::*;

type DhtStoredItem = gipny_dht::proto::StoredItem;
type DhtStoreLimits = gipny_dht::store::StoreLimits;
type DhtKnownPeer = gipny_dht::node::KnownPeer;

impl Db {
    // ── relay network ────────────────────────────────────────────────────

    /// Store an item held for others, evicting the oldest under its key and
    /// then the oldest overall to stay within the limits. An identical value
    /// under the same key is a no-op. The item is already admitted
    /// (`gipny_dht::store::admit`).
    pub fn dht_item_put(&self, item: &DhtStoredItem, limits: &DhtStoreLimits) -> Result<()> {
        let hash = gipny_dht::crypto::sha256(&[&item.value]);
        let len = item.value.len() as i64;
        self.with_tx(|tx| {
            let dup: i64 = tx.query_row(
                "SELECT COUNT(*) FROM dht_items WHERE k = ?1 AND v_hash = ?2",
                params![&item.key[..], &hash[..]], |r| r.get(0))?;
            if dup > 0 {
                return Ok(());
            }
            loop {
                let (n, bytes): (i64, i64) = tx.query_row(
                    "SELECT COUNT(*), COALESCE(SUM(LENGTH(v)), 0) FROM dht_items WHERE k = ?1",
                    params![&item.key[..]], |r| Ok((r.get(0)?, r.get(1)?)))?;
                if n == 0 || (n < limits.max_items_per_key as i64 && bytes + len <= limits.max_bytes_per_key as i64) {
                    break;
                }
                tx.execute("DELETE FROM dht_items WHERE id = (SELECT MIN(id) FROM dht_items WHERE k = ?1)", params![&item.key[..]])?;
            }
            loop {
                let total: i64 = tx.query_row("SELECT COALESCE(SUM(LENGTH(v)), 0) FROM dht_items", [], |r| r.get(0))?;
                if total == 0 || total + len <= limits.max_total_bytes as i64 {
                    break;
                }
                tx.execute("DELETE FROM dht_items WHERE id = (SELECT MIN(id) FROM dht_items)", [])?;
            }
            tx.execute(
                "INSERT INTO dht_items (k, v, v_hash, delete_hash, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![&item.key[..], &item.value, &hash[..], item.delete_hash.as_ref().map(|h| &h[..]), item.expires_at_ms as i64])?;
            Ok(())
        })
    }

    fn dht_rows(&self, sql: &str, p: impl rusqlite::Params) -> Result<Vec<DhtStoredItem>> {
        self.with_conn(|c| {
            let mut st = c.prepare(sql)?;
            let rows = st.query_map(p, |r| {
                let k: Vec<u8> = r.get(0)?;
                let d: Option<Vec<u8>> = r.get(2)?;
                let e: i64 = r.get(3)?;
                Ok((k, r.get::<_, Vec<u8>>(1)?, d, e))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (k, value, d, e) = row?;
                let Ok(key) = <[u8; 32]>::try_from(k.as_slice()) else {
                    continue;
                };
                let delete_hash = d.and_then(|d| <[u8; 32]>::try_from(d.as_slice()).ok());
                out.push(DhtStoredItem {
                    key,
                    value,
                    delete_hash,
                    expires_at_ms: e.max(0) as u64,
                });
            }
            Ok(out)
        })
    }

    /// Live items under `key`, oldest first.
    pub fn dht_items_get(&self, key: &[u8; 32], now_ms: i64) -> Result<Vec<DhtStoredItem>> {
        self.dht_rows("SELECT k, v, delete_hash, expires_at FROM dht_items WHERE k = ?1 AND expires_at > ?2 ORDER BY id",
            params![&key[..], now_ms])
    }

    pub fn dht_items_all(&self, now_ms: i64) -> Result<Vec<DhtStoredItem>> {
        self.dht_rows(
            "SELECT k, v, delete_hash, expires_at FROM dht_items WHERE expires_at > ?1 ORDER BY id",
            params![now_ms],
        )
    }

    pub fn dht_item_delete(&self, key: &[u8; 32], delete_hash: &[u8; 32]) -> Result<u32> {
        self.with_conn(|c| {
            Ok(c.execute(
                "DELETE FROM dht_items WHERE k = ?1 AND delete_hash = ?2",
                params![&key[..], &delete_hash[..]],
            )? as u32)
        })
    }

    pub fn dht_items_gc(&self, now_ms: i64) -> Result<usize> {
        self.with_conn(|c| {
            Ok(c.execute(
                "DELETE FROM dht_items WHERE expires_at <= ?1",
                params![now_ms],
            )?)
        })
    }

    /// Items held and their bytes.
    pub fn dht_items_stats(&self) -> Result<(usize, usize)> {
        self.with_conn(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*), COALESCE(SUM(LENGTH(v)), 0) FROM dht_items",
                [],
                |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as usize)),
            )?)
        })
    }

    /// Replace the saved table of nodes that answered us.
    pub fn dht_peers_save(&self, peers: &[DhtKnownPeer]) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute("DELETE FROM dht_peers", [])?;
            for p in peers {
                tx.execute(
                    "INSERT OR REPLACE INTO dht_peers (destination, stores, first_seen, last_ok) VALUES (?1, ?2, ?3, ?4)",
                    params![p.info.destination, p.info.stores as i64, p.first_seen_ms as i64, p.last_ok_ms as i64])?;
            }
            Ok(())
        })
    }

    /// Records a letter as handled; `false` when it already was.
    pub fn dht_seen_mark(&self, envelope_hash: &[u8; 32], now_ms: i64) -> Result<bool> {
        self.with_conn(|c| {
            Ok(c.execute(
                "INSERT OR IGNORE INTO dht_seen (envelope_hash, seen_at) VALUES (?1, ?2)",
                params![&envelope_hash[..], now_ms],
            )? > 0)
        })
    }

    /// Undo a mark, for a letter that turned out not to be handled after all.
    pub fn dht_seen_forget(&self, envelope_hash: &[u8; 32]) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "DELETE FROM dht_seen WHERE envelope_hash = ?1",
                params![&envelope_hash[..]],
            )?;
            Ok(())
        })
    }

    pub fn dht_seen_purge(&self, older_than: i64) -> Result<usize> {
        self.with_conn(|c| {
            Ok(c.execute(
                "DELETE FROM dht_seen WHERE seen_at < ?1",
                params![older_than],
            )?)
        })
    }

    /// Saved nodes, longest known first: a node that has been around longer
    /// is likelier to still be.
    pub fn dht_peers_load(&self) -> Result<Vec<String>> {
        self.with_conn(|c| {
            let mut st = c.prepare("SELECT destination FROM dht_peers ORDER BY first_seen")?;
            let rows = st.query_map([], |r| r.get::<_, String>(0))?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }
}

#[cfg(test)]
mod dht_tests {
    use super::*;

    #[test]
    fn a_letter_is_handled_once_until_forgotten_or_stale() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_plain(&dir.path().join("t.db")).unwrap();
        let hash = [9u8; 32];
        assert!(
            db.dht_seen_mark(&hash, 1_000).unwrap(),
            "first time is ours to handle"
        );
        assert!(
            !db.dht_seen_mark(&hash, 2_000).unwrap(),
            "a second copy is skipped"
        );
        // Not handled after all: the next pass must try again.
        db.dht_seen_forget(&hash).unwrap();
        assert!(db.dht_seen_mark(&hash, 3_000).unwrap());
        assert_eq!(db.dht_seen_purge(4_000).unwrap(), 1);
        assert!(db.dht_seen_mark(&hash, 5_000).unwrap());
    }

    #[test]
    fn items_held_for_others_survive_and_are_capped_per_key() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_plain(&dir.path().join("t.db")).unwrap();
        let limits = gipny_dht::store::StoreLimits {
            max_items_per_key: 2,
            ..Default::default()
        };
        let item = |v: &[u8]| gipny_dht::proto::StoredItem {
            key: [1u8; 32],
            value: v.to_vec(),
            delete_hash: None,
            expires_at_ms: 10_000,
        };
        db.dht_item_put(&item(b"a"), &limits).unwrap();
        db.dht_item_put(&item(b"a"), &limits).unwrap();
        db.dht_item_put(&item(b"b"), &limits).unwrap();
        db.dht_item_put(&item(b"c"), &limits).unwrap();
        let held: Vec<Vec<u8>> = db
            .dht_items_get(&[1u8; 32], 0)
            .unwrap()
            .into_iter()
            .map(|i| i.value)
            .collect();
        assert_eq!(
            held,
            vec![b"b".to_vec(), b"c".to_vec()],
            "the oldest under the key goes first"
        );
        assert_eq!(db.dht_items_stats().unwrap(), (2, 2));
        assert!(
            db.dht_items_get(&[1u8; 32], 20_000).unwrap().is_empty(),
            "expired items are not served"
        );
    }
}
