//! Durable callback and notification idempotency records.

use super::*;

impl Db {
    pub fn callback_seen(
        &self,
        sender_sign_pk: &[u8],
        sent_at: i64,
        callback_data: &str,
    ) -> Result<bool> {
        self.with_conn(|c| Ok(c.query_row(
            "SELECT COUNT(*) FROM callback_seen WHERE sender_sign_pk = ?1 AND sent_at = ?2 AND callback_data = ?3",
            params![sender_sign_pk, sent_at, callback_data], |r| r.get::<_, i64>(0))? > 0))
    }

    pub fn record_callback_seen(
        &self,
        sender_sign_pk: &[u8],
        sent_at: i64,
        callback_data: &str,
    ) -> Result<bool> {
        self.with_conn(|c| Ok(c.execute(
            "INSERT OR IGNORE INTO callback_seen (sender_sign_pk, sent_at, callback_data, seen_at) VALUES (?1, ?2, ?3, ?4)",
            params![sender_sign_pk, sent_at, callback_data, now_ms()])? > 0))
    }

    pub fn purge_old_callback_seen(&self, older_than: i64) -> Result<usize> {
        self.with_conn(|c| {
            Ok(c.execute(
                "DELETE FROM callback_seen WHERE seen_at < ?1",
                params![older_than],
            )?)
        })
    }

    pub fn notify_seen_lookup(&self, idem: &str) -> Result<Option<i64>> {
        self.with_conn(|c| {
            c.prepare_cached("SELECT message_id FROM notify_seen WHERE idempotency_key = ?1")?
                .query_row(params![idem], |r| r.get::<_, i64>(0))
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn notify_seen_record(&self, idem: &str, message_id: i64) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "INSERT OR IGNORE INTO notify_seen (idempotency_key, message_id, seen_at) VALUES (?1, ?2, ?3)",
            params![idem, message_id, now_ms()])?; Ok(()) })
    }

    pub fn notify_seen_purge(&self, older_than: i64) -> Result<usize> {
        self.with_conn(|c| {
            Ok(c.execute(
                "DELETE FROM notify_seen WHERE seen_at < ?1",
                params![older_than],
            )?)
        })
    }
}
