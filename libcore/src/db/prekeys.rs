//! Identity, signed, and one-time prekey records.

use super::*;

impl Db {
    pub fn add_prekey(&self, kind: PreKeyKind, private: &[u8], public: &[u8]) -> Result<i64> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO prekeys (kind, private, public, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![kind.to_i64(), private, public, now_ms()],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn get_prekey(&self, id: i64) -> Result<Option<PreKey>> {
        self.with_conn(|c| {
            c.prepare_cached(
                "SELECT id, kind, private, public, created_at FROM prekeys WHERE id = ?1",
            )?
            .query_row(params![id], Self::map_prekey)
            .optional()
            .map_err(Into::into)
        })
    }

    pub fn list_prekeys(&self, kind: PreKeyKind) -> Result<Vec<PreKey>> {
        self.with_conn(|c| c.prepare_cached("SELECT id, kind, private, public, created_at FROM prekeys WHERE kind = ?1 ORDER BY id")?
            .query_map(params![kind.to_i64()], Self::map_prekey)?.collect_rows())
    }

    pub fn consume_prekey(&self, id: i64) -> Result<Option<PreKey>> {
        self.with_tx(|tx| {
            let pk: Option<PreKey> = tx
                .prepare_cached(
                    "SELECT id, kind, private, public, created_at FROM prekeys WHERE id = ?1",
                )?
                .query_row(params![id], Self::map_prekey)
                .optional()?;
            if pk.is_some() {
                tx.execute("DELETE FROM prekeys WHERE id = ?1", params![id])?;
            }
            Ok(pk)
        })
    }

    pub(super) fn map_prekey(r: &rusqlite::Row) -> rusqlite::Result<PreKey> {
        Ok(PreKey {
            id: r.get(0)?,
            kind: PreKeyKind::from_i64(r.get(1)?),
            private: r.get(2)?,
            public: r.get(3)?,
            created_at: r.get(4)?,
        })
    }

    pub fn delete_prekey(&self, id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM prekeys WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    pub fn count_prekeys(&self, kind: PreKeyKind) -> Result<i64> {
        self.with_conn(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM prekeys WHERE kind = ?1",
                params![kind.to_i64()],
                |r| r.get(0),
            )?)
        })
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
}
