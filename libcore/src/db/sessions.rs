//! Persisted ratchet sessions and resynchronization throttling.

use super::*;

impl Db {
    pub fn put_session(&self, contact_id: i64, state: &[u8]) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "INSERT INTO sessions (contact_id, state, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(contact_id) DO UPDATE SET state = excluded.state, updated_at = excluded.updated_at",
            params![contact_id, state, now_ms()])?; Ok(()) })
    }

    pub fn get_session(&self, contact_id: i64) -> Result<Option<Vec<u8>>> {
        self.with_conn(|c| {
            c.prepare_cached("SELECT state FROM sessions WHERE contact_id = ?1")?
                .query_row(params![contact_id], |r| r.get::<_, Vec<u8>>(0))
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn has_session(&self, contact_id: i64) -> Result<bool> {
        self.with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE contact_id=?1)",
                params![contact_id],
                |row| row.get(0),
            )?)
        })
    }

    pub fn delete_session(&self, contact_id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "DELETE FROM sessions WHERE contact_id = ?1",
                params![contact_id],
            )?;
            Ok(())
        })
    }

    pub fn resync_recent(&self, contact_id: i64, within_ms: i64) -> Result<bool> {
        self.with_conn(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM resync_log WHERE contact_id = ?1 AND last_resync_at >= ?2",
                params![contact_id, now_ms() - within_ms],
                |r| r.get::<_, i64>(0),
            )? > 0)
        })
    }

    pub fn record_resync(&self, contact_id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO resync_log (contact_id, last_resync_at) VALUES (?1, ?2)
             ON CONFLICT(contact_id) DO UPDATE SET last_resync_at = excluded.last_resync_at",
                params![contact_id, now_ms()],
            )?;
            Ok(())
        })
    }
}
