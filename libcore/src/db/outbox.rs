//! Durable outbound obligations, delivery acknowledgments, and retry records.

use super::*;

impl Db {
    /// Publish a fully prepared group message to the retry loop. Register the
    /// complete recipient snapshot atomically, after settings/file offers are
    /// ready and before the first network await. Preparation itself is separate.
    pub fn queue_group_recipients(&self, msg_id: i64, recipients: &[i64]) -> Result<()> {
        self.with_tx(|tx| {
            let created_at: i64 = tx.query_row(
                "SELECT sent_at FROM messages WHERE id = ?1 AND direction = 1 AND group_id IS NOT NULL",
                params![msg_id], |row| row.get(0),
            )?;
            for recipient in recipients {
                tx.execute(
                    "INSERT OR IGNORE INTO pending_outbound
                     (msg_id, recipient_contact_id, send_attempts, created_at) VALUES (?1, ?2, 0, ?3)",
                    params![msg_id, recipient, created_at],
                )?;
            }
            Ok(())
        })
    }

    /// Accept a fully prepared group message and its entire outbox atomically.
    /// A rollback exposes neither history nor a partial recipient/file queue.
    pub fn commit_outgoing_group(
        &self,
        group: &NewOutgoingGroup<'_>,
        attachments: &[NewGroupAttachment],
    ) -> Result<i64> {
        check_body(group.body)?;
        for staged in attachments {
            if staged.attachment.chunk_size.is_some() != staged.file_offer.is_some() {
                return Err(DbError::State);
            }
        }
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO messages (group_id, sender_sign_pk, direction, body, sent_at, expires_at, reply_to)
                 VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6)",
                params![group.group_id, group.sender_sign_pk, group.body, group.sent_at, group.expires_at, group.reply_to],
            )?;
            let id = tx.last_insert_rowid();
            for (prefix, value) in [("buttons", group.buttons), ("sound", group.notify_sound)] {
                if let Some(value) = value {
                    tx.execute("INSERT INTO settings (k, v) VALUES (?1, ?2)", params![format!("{prefix}_{id}"), value])?;
                }
            }
            for staged in attachments {
                let a = &staged.attachment;
                insert_attachments(tx, id, std::slice::from_ref(a))?;
                let attachment_id = tx.last_insert_rowid();
                if let Some((file_id, sha256)) = &staged.file_offer {
                    tx.execute(
                        "INSERT INTO file_out (file_id, attachment_id, message_id, sha256, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![file_id.as_slice(), attachment_id, id, sha256.as_slice(), group.sent_at],
                    )?;
                    for recipient in group.recipients {
                        tx.execute("INSERT OR IGNORE INTO file_out_peer (file_id, contact_id) VALUES (?1, ?2)",
                            params![file_id.as_slice(), recipient])?;
                    }
                }
            }
            for recipient in group.recipients {
                tx.execute(
                    "INSERT OR IGNORE INTO pending_outbound (msg_id, recipient_contact_id, send_attempts, created_at)
                     VALUES (?1, ?2, 0, ?3)", params![id, recipient, group.sent_at],
                )?;
            }
            Ok(id)
        })
    }

    pub fn list_unsent_outgoing(&self, contact_id: i64, limit: i64) -> Result<Vec<Message>> {
        self.with_conn(|c| c.prepare_cached(concat!("SELECT ", message_cols!(),
            " FROM messages WHERE contact_id = ?1 AND direction = 1 AND sent = 0 AND id IN (SELECT id FROM awaiting_delivery WHERE contact_id=?1 AND deadline>?3) ORDER BY id ASC LIMIT ?2"))?
            .query_map(params![contact_id, limit, now_ms()], Self::map_message)?.collect_rows())
    }

    /// Bounded by age, not by attempts: a recipient who closed the app right
    /// after its relay took the message has to get it once they are back, and
    /// eight attempts over a quarter of an hour gave up long before that.
    pub fn list_unacked_outgoing(
        &self,
        contact_id: i64,
        now_ms: i64,
        base_backoff_ms: i64,
        max_backoff_ms: i64,
        limit: i64,
    ) -> Result<Vec<Message>> {
        self.with_conn(|c| c.prepare_cached(concat!("SELECT ", message_cols!(),
            " FROM messages WHERE contact_id = ?1 AND direction = 1 AND sent = 1 AND delivered = 0 AND deleted_at IS NULL
               AND id IN (SELECT id FROM awaiting_delivery WHERE contact_id=?1 AND deadline>?2)
               AND (last_attempt_at IS NULL OR (?2 - last_attempt_at) >= MIN(?4, ?3 * (1 << MIN(send_attempts, 16))))
             ORDER BY id ASC LIMIT ?5"))?
            .query_map(params![contact_id, now_ms, base_backoff_ms, max_backoff_ms, limit], Self::map_message)?
            .collect_rows())
    }

    pub fn pending_outbound_add(&self, msg_id: i64, recipient_contact_id: i64) -> Result<()> {
        self.with_conn(|c| { c.execute(
            "INSERT OR IGNORE INTO pending_outbound (msg_id, recipient_contact_id, send_attempts, created_at) VALUES (?1, ?2, 0, ?3)",
            params![msg_id, recipient_contact_id, now_ms()])?; Ok(()) })
    }

    pub fn pending_outbound_remove(&self, msg_id: i64, recipient_contact_id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "DELETE FROM pending_outbound WHERE msg_id = ?1 AND recipient_contact_id = ?2",
                params![msg_id, recipient_contact_id],
            )?;
            Ok(())
        })
    }

    /// Call only after authenticating the peer and validating the message's
    /// group id. The recipient obligation and final delivered flag commit
    /// together; unrelated, duplicate and already-purged ACKs are no-ops.
    pub fn acknowledge_group_delivery(
        &self,
        msg_id: i64,
        recipient_contact_id: i64,
    ) -> Result<GroupDeliveryAck> {
        self.with_tx(|tx| {
            let removed = tx.execute(
                "DELETE FROM pending_outbound WHERE msg_id = ?1 AND recipient_contact_id = ?2
                 AND EXISTS (SELECT 1 FROM awaiting_delivery WHERE id = ?1 AND group_id IS NOT NULL AND deadline>?3)",
                params![msg_id, recipient_contact_id, now_ms()],
            )?;
            if removed == 0 { return Ok(GroupDeliveryAck::NotPending); }
            let remaining: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM pending_outbound WHERE msg_id = ?1)",
                params![msg_id], |row| row.get(0),
            )?;
            if remaining { return Ok(GroupDeliveryAck::PendingRecipients); }
            let completed = tx.execute(
                "UPDATE messages SET sent = 1, delivered = 1 WHERE id = ?1 AND delivered = 0",
                params![msg_id],
            )?;
            Ok(if completed > 0 { GroupDeliveryAck::Complete } else { GroupDeliveryAck::NotPending })
        })
    }

    pub fn pending_outbound_for_recipient(
        &self,
        recipient_contact_id: i64,
        now_ms: i64,
        base_backoff_ms: i64,
        max_backoff_ms: i64,
        limit: i64,
    ) -> Result<Vec<i64>> {
        self.with_conn(|c| c.prepare_cached(
            "SELECT msg_id FROM pending_outbound
             WHERE recipient_contact_id = ?1
               AND msg_id IN (SELECT id FROM awaiting_delivery WHERE deadline>?2)
               AND (last_attempt_at IS NULL OR (?2 - last_attempt_at) >= MIN(?4, ?3 * (1 << MIN(send_attempts, 16))))
             ORDER BY msg_id ASC LIMIT ?5")?
            .query_map(params![recipient_contact_id, now_ms, base_backoff_ms, max_backoff_ms, limit], |r| r.get(0))?
            .collect_rows())
    }

    pub fn pending_outbound_record_attempt(
        &self,
        msg_id: i64,
        recipient_contact_id: i64,
    ) -> Result<bool> {
        self.with_conn(|c| {
            let changed = c.execute(
            "UPDATE pending_outbound SET last_attempt_at = ?3, send_attempts = send_attempts + 1
             WHERE msg_id = ?1 AND recipient_contact_id = ?2 AND msg_id IN
                (SELECT id FROM awaiting_delivery WHERE id=?1 AND deadline>?3)",
            params![msg_id, recipient_contact_id, now_ms()])?;
            Ok(changed != 0)
        })
    }

    pub fn add_dead_letter(
        &self,
        contact_id: Option<i64>,
        group_id: Option<&[u8]>,
        kind: &str,
        payload: &[u8],
        error: &str,
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
        self.with_conn(|c| {
            Ok(c.execute(
                "DELETE FROM dead_letters WHERE last_seen_at < ?1",
                params![older_than],
            )?)
        })
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

    pub fn acknowledge_dm_delivery(&self, id: i64, contact_id: i64) -> Result<bool> {
        self.with_conn(|c| Ok(c.execute(
            "UPDATE messages SET sent=1, delivered=1 WHERE id IN (SELECT id FROM awaiting_delivery
                WHERE id=?1 AND contact_id=?2 AND group_id IS NULL AND deadline>?3)",
            params![id, contact_id, now_ms()],
        )? != 0))
    }
}

#[cfg(test)]
mod tests;
