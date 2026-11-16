//! Encrypted per-profile settings and batched message extras.

use super::*;

impl Db {
    pub fn get_setting(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.with_conn(|c| {
            c.prepare_cached("SELECT v FROM settings WHERE k = ?1")?
                .query_row(params![key], |r| r.get::<_, Vec<u8>>(0))
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn load_buttons_batch(&self, ids: &[i64]) -> Result<HashMap<i64, Vec<u8>>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let keys: Vec<String> = ids.iter().map(|i| format!("buttons_{}", i)).collect();
        let placeholders = (0..keys.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT k, v FROM settings WHERE k IN ({})", placeholders);
        self.with_conn(|c| {
            let mut s = c.prepare(&sql)?;
            let mut out: HashMap<i64, Vec<u8>> = HashMap::with_capacity(keys.len());
            for row in s.query_map(rusqlite::params_from_iter(keys.iter()), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
            })? {
                let (k, v) = row?;
                if let Some(mid) = k
                    .strip_prefix("buttons_")
                    .and_then(|s| s.parse::<i64>().ok())
                {
                    out.insert(mid, v);
                }
            }
            Ok(out)
        })
    }

    /// Per-message extras under `<prefix><message id>`, for a batch of ids.
    pub fn load_settings_batch(&self, prefix: &str, ids: &[i64]) -> Result<HashMap<i64, Vec<u8>>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let keys: Vec<String> = ids.iter().map(|i| format!("{prefix}{i}")).collect();
        let placeholders = (0..keys.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT k, v FROM settings WHERE k IN ({})", placeholders);
        self.with_conn(|c| {
            let mut s = c.prepare(&sql)?;
            let mut out: HashMap<i64, Vec<u8>> = HashMap::with_capacity(keys.len());
            for row in s.query_map(rusqlite::params_from_iter(keys.iter()), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
            })? {
                let (k, v) = row?;
                if let Some(mid) = k.strip_prefix(prefix).and_then(|s| s.parse::<i64>().ok()) {
                    out.insert(mid, v);
                }
            }
            Ok(out)
        })
    }

    /// Message ids that have a `<prefix><id>` setting, ascending. `substr`
    /// rather than `LIKE`: the prefixes carry `_`, which LIKE treats as a wildcard.
    pub fn list_setting_ids_with_prefix(&self, prefix: &str) -> Result<Vec<i64>> {
        self.with_conn(|c| {
            let mut s = c.prepare("SELECT k FROM settings WHERE substr(k, 1, length(?1)) = ?1")?;
            let mut ids: Vec<i64> = s
                .query_map(params![prefix], |r| r.get::<_, String>(0))?
                .filter_map(|k| k.ok())
                .filter_map(|k| k.strip_prefix(prefix).and_then(|s| s.parse::<i64>().ok()))
                .collect();
            ids.sort_unstable();
            Ok(ids)
        })
    }

    pub fn set_setting(&self, key: &str, value: &[u8]) -> Result<()> {
        self.with_conn(|c| {
            let message_id = ["buttons_", "sound_", "console_pending_", "console_"].iter()
                .find_map(|prefix|key.strip_prefix(prefix).and_then(|id|id.parse::<i64>().ok()));
            if let Some(id)=message_id {
                let deleted:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM messages WHERE id=?1 AND deleted_at IS NOT NULL)",params![id],|r|r.get(0))?;
                if deleted {return Ok(());}
            }
            c.execute(
            "INSERT INTO settings (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            params![key, value])?; Ok(()) })
    }

    pub fn delete_setting(&self, key: &str) -> Result<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM settings WHERE k = ?1", params![key])?;
            Ok(())
        })
    }
}
