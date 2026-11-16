//! Pinned conversations/messages and deferred pin operations.

use super::*;

impl Db {
    pub fn pin_contact(&self, id: i64, ts: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE contacts SET pinned_at = ?1 WHERE id = ?2",
                params![ts, id],
            )?;
            Ok(())
        })
    }

    pub fn unpin_contact(&self, id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE contacts SET pinned_at = NULL WHERE id = ?1",
                params![id],
            )?;
            Ok(())
        })
    }

    pub fn pin_group(&self, id: &[u8], ts: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE groups SET pinned_at = ?1 WHERE id = ?2",
                params![ts, id],
            )?;
            Ok(())
        })
    }

    pub fn unpin_group(&self, id: &[u8]) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE groups SET pinned_at = NULL WHERE id = ?1",
                params![id],
            )?;
            Ok(())
        })
    }

    pub fn cleanup_orphan_pins(&self) -> Result<usize> {
        self.with_conn(|c| {
            Ok(c.execute(
                "DELETE FROM pinned_messages WHERE message_id NOT IN (SELECT id FROM messages)",
                [],
            )?)
        })
    }

    pub fn pin_contact_message(&self, contact_id: i64, message_id: i64) -> Result<bool> {
        self.with_conn(|c| Ok(c.execute(
            "INSERT OR IGNORE INTO pinned_messages (contact_id, message_id, pinned_at)
             SELECT ?1,?2,?3 WHERE EXISTS(SELECT 1 FROM messages WHERE id=?2 AND contact_id=?1 AND deleted_at IS NULL)",
            params![contact_id, message_id, now_ms()])? > 0))
    }

    pub fn pin_group_message(&self, group_id: &[u8], message_id: i64) -> Result<bool> {
        self.with_conn(|c| Ok(c.execute(
            "INSERT OR IGNORE INTO pinned_messages (group_id, message_id, pinned_at) VALUES (?1, ?2, ?3)",
            params![group_id, message_id, now_ms()])? > 0))
    }

    pub fn unpin_contact_message(&self, contact_id: i64, message_id: i64) -> Result<bool> {
        self.with_conn(|c| {
            Ok(c.execute(
                "DELETE FROM pinned_messages WHERE contact_id = ?1 AND message_id = ?2",
                params![contact_id, message_id],
            )? > 0)
        })
    }

    pub fn unpin_group_message(&self, group_id: &[u8], message_id: i64) -> Result<bool> {
        self.with_conn(|c| {
            Ok(c.execute(
                "DELETE FROM pinned_messages WHERE group_id = ?1 AND message_id = ?2",
                params![group_id, message_id],
            )? > 0)
        })
    }

    pub fn is_contact_message_pinned(&self, contact_id: i64, message_id: i64) -> Result<bool> {
        self.with_conn(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM pinned_messages WHERE contact_id = ?1 AND message_id = ?2",
                params![contact_id, message_id],
                |r| r.get::<_, i64>(0),
            )? > 0)
        })
    }

    pub fn is_group_message_pinned(&self, group_id: &[u8], message_id: i64) -> Result<bool> {
        self.with_conn(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM pinned_messages WHERE group_id = ?1 AND message_id = ?2",
                params![group_id, message_id],
                |r| r.get::<_, i64>(0),
            )? > 0)
        })
    }

    pub fn list_pinned_contact(&self, contact_id: i64) -> Result<Vec<Message>> {
        self.with_conn(|c| {
            c.prepare_cached(concat!(
                "SELECT ",
                message_cols_m!(),
                " FROM messages m INNER JOIN pinned_messages p ON p.message_id = m.id
              WHERE p.contact_id = ?1 ORDER BY p.pinned_at DESC"
            ))?
            .query_map(params![contact_id], Self::map_message)?
            .collect_rows()
        })
    }

    pub fn list_pinned_group(&self, group_id: &[u8]) -> Result<Vec<Message>> {
        self.with_conn(|c| {
            c.prepare_cached(concat!(
                "SELECT ",
                message_cols_m!(),
                " FROM messages m INNER JOIN pinned_messages p ON p.message_id = m.id
              WHERE p.group_id = ?1 ORDER BY p.pinned_at DESC"
            ))?
            .query_map(params![group_id], Self::map_message)?
            .collect_rows()
        })
    }

    pub fn add_deferred_pin_contact(
        &self,
        contact_id: i64,
        sender_sign_pk: &[u8],
        origin_msg_id: i64,
        unpin: bool,
    ) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "INSERT INTO deferred_pins (contact_id, sender_sign_pk, origin_msg_id, unpin, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![contact_id, sender_sign_pk, origin_msg_id, unpin as i64, now_ms()])?; Ok(()) })
    }

    pub fn add_deferred_pin_group(
        &self,
        group_id: &[u8],
        sender_sign_pk: &[u8],
        origin_msg_id: i64,
        unpin: bool,
    ) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "INSERT INTO deferred_pins (group_id, sender_sign_pk, origin_msg_id, unpin, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![group_id, sender_sign_pk, origin_msg_id, unpin as i64, now_ms()])?; Ok(()) })
    }

    pub fn take_deferred_pin_contact(
        &self,
        contact_id: i64,
        sender_sign_pk: &[u8],
        origin_msg_id: i64,
    ) -> Result<Option<bool>> {
        self.with_tx(|tx| {
            let row: Option<(i64, i64)> = tx
                .prepare_cached(
                    "SELECT id, unpin FROM deferred_pins
                     WHERE contact_id = ?1 AND sender_sign_pk = ?2 AND origin_msg_id = ?3
                     ORDER BY id ASC LIMIT 1",
                )?
                .query_row(params![contact_id, sender_sign_pk, origin_msg_id], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            if let Some((id, u)) = row {
                tx.execute("DELETE FROM deferred_pins WHERE id = ?1", params![id])?;
                Ok(Some(u != 0))
            } else {
                Ok(None)
            }
        })
    }

    pub fn take_deferred_pin_group(
        &self,
        group_id: &[u8],
        sender_sign_pk: &[u8],
        origin_msg_id: i64,
    ) -> Result<Option<bool>> {
        self.with_tx(|tx| {
            let row: Option<(i64, i64)> = tx
                .prepare_cached(
                    "SELECT id, unpin FROM deferred_pins
                     WHERE group_id = ?1 AND sender_sign_pk = ?2 AND origin_msg_id = ?3
                     ORDER BY id ASC LIMIT 1",
                )?
                .query_row(params![group_id, sender_sign_pk, origin_msg_id], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            if let Some((id, u)) = row {
                tx.execute("DELETE FROM deferred_pins WHERE id = ?1", params![id])?;
                Ok(Some(u != 0))
            } else {
                Ok(None)
            }
        })
    }

    pub fn purge_old_deferred_pins(&self, older_than: i64) -> Result<usize> {
        self.with_conn(|c| {
            Ok(c.execute(
                "DELETE FROM deferred_pins WHERE created_at < ?1",
                params![older_than],
            )?)
        })
    }
}
