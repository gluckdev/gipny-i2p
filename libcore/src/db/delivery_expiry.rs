//! The delivery deadline starts at creation, including time spent offline.
//! Record the notification before a short content TTL can erase its message.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeliveryExpiryNotice {
    pub message_id: i64,
    pub contact_id: Option<i64>,
    pub group_id: Option<String>,
    pub expired_at: i64,
}

impl DeliveryExpiryNotice {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            message_id: row.get(0)?,
            contact_id: row.get(1)?,
            group_id: row.get::<_, Option<Vec<u8>>>(2)?.as_deref().map(hex),
            expired_at: row.get(3)?,
        })
    }
}

impl Db {
    /// Claims every due outgoing row once, even when no relay ever accepted it.
    /// A group with outstanding recipients fails as a whole. Files can keep
    /// going to members who already acknowledged their offer until content TTL.
    pub fn expire_outgoing_delivery(&self, now: i64) -> Result<Vec<DeliveryExpiryNotice>> {
        self.with_tx(|tx| {
            let due = tx.prepare_cached(
                "SELECT id, contact_id, group_id, deadline FROM awaiting_delivery
                 WHERE deadline <= ?1 ORDER BY deadline, id"
            )?.query_map([now], DeliveryExpiryNotice::from_row)?.collect_rows()?;
            for notice in &due {
                let id = notice.message_id;
                tx.execute("INSERT INTO delivery_expiry_notices(message_id, contact_id, group_id, expired_at)
                    SELECT id, contact_id, group_id, ?2 FROM messages WHERE id=?1",
                    params![id, notice.expired_at])?;
                tx.execute("UPDATE messages SET delivery_expired_at=?2 WHERE id=?1",
                    params![id, notice.expired_at])?;
                tx.execute("UPDATE file_out_peer SET failed=1 WHERE file_id IN
                    (SELECT file_id FROM file_out WHERE message_id=?1)
                    AND (?2 IS NOT NULL OR contact_id IN
                        (SELECT recipient_contact_id FROM pending_outbound WHERE msg_id=?1))",
                    params![id, notice.contact_id])?;
                tx.execute("DELETE FROM pending_outbound WHERE msg_id=?1", [id])?;
            }
            Ok(due)
        })
    }

    /// A screen can be hidden or closed while it reads the queue. Only the
    /// explicit acknowledgement after presentation removes a notice.
    pub fn list_delivery_expiry_notices(&self) -> Result<Vec<DeliveryExpiryNotice>> {
        self.with_conn(Self::delivery_notices)
    }

    pub fn acknowledge_delivery_expiry_notices(&self, ids: &[i64]) -> Result<()> {
        self.with_tx(|tx| {
            let mut remove =
                tx.prepare_cached("DELETE FROM delivery_expiry_notices WHERE message_id=?1")?;
            for id in ids {
                remove.execute([id])?;
            }
            Ok(())
        })
    }

    fn delivery_notices(conn: &Connection) -> Result<Vec<DeliveryExpiryNotice>> {
        conn.prepare_cached(
            "SELECT message_id, contact_id, group_id, expired_at
            FROM delivery_expiry_notices ORDER BY expired_at, message_id",
        )?
        .query_map([], DeliveryExpiryNotice::from_row)?
        .collect_rows()
    }

    /// Hand each notice to the headless session channel. A full or closed
    /// channel leaves that notice and the rest in the database for next time.
    pub(crate) fn dispatch_delivery_expiry_notices(
        &self,
        mut dispatch: impl FnMut(DeliveryExpiryNotice) -> bool,
    ) -> Result<()> {
        self.with_tx(|tx| {
            for notice in Self::delivery_notices(tx)? {
                let id = notice.message_id;
                if !dispatch(notice) {
                    break;
                }
                tx.execute(
                    "DELETE FROM delivery_expiry_notices WHERE message_id=?1",
                    [id],
                )?;
            }
            Ok(())
        })
    }

    /// Recheck after asynchronous route/session setup immediately before the letter goes out.
    pub fn may_send_message(&self, id: i64, recipient: i64, now: i64) -> Result<bool> {
        self.with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT EXISTS(
            SELECT 1 FROM awaiting_delivery WHERE id=?1 AND deadline>?3 AND
                (contact_id=?2 OR (group_id IS NOT NULL AND EXISTS(
                    SELECT 1 FROM pending_outbound WHERE msg_id=?1 AND recipient_contact_id=?2))))",
                params![id, recipient, now],
                |r| r.get(0),
            )?)
        })
    }

    pub fn may_send_file_part(&self, file: &[u8; 16], recipient: i64, now: i64) -> Result<bool> {
        self.with_conn(|conn| Ok(conn.query_row("SELECT EXISTS(
            SELECT 1 FROM file_out f JOIN file_out_peer p USING(file_id) JOIN messages m ON m.id=f.message_id
            WHERE f.file_id=?1 AND p.contact_id=?2 AND f.cancelled=0 AND p.failed=0 AND m.deleted_at IS NULL
            AND (m.expires_at IS NULL OR m.expires_at>?3)
            AND (m.delivered=1 OR m.sent_at>?4 OR (m.group_id IS NOT NULL AND NOT EXISTS(
                SELECT 1 FROM pending_outbound WHERE msg_id=m.id AND recipient_contact_id=?2))))",
            params![file.as_slice(), recipient, now, now.saturating_sub(RETRY_TTL_MS)], |r| r.get(0))?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_unconfirmed_dm_expires_from_creation_and_notice_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("expiry.db");
        let db = Db::open_plain(&path).unwrap();
        let peer = db.add_contact(&[1; 32], &[2; 32], "", "Bob", None).unwrap();
        let sent = now_ms() - RETRY_TTL_MS - 100;
        let deadline = sent + RETRY_TTL_MS;
        let id = db
            .insert_message(peer, Direction::Out, "retained", sent, None, &[])
            .unwrap();
        let in_id = db
            .insert_message(peer, Direction::In, "incoming", sent, None, &[])
            .unwrap();

        // An authenticated ACK arriving after the deadline must lose even if
        // the maintenance timer has not run once since the profile was opened.
        assert!(!db.acknowledge_dm_delivery(id, peer).unwrap());
        assert!(db.list_unsent_outgoing(peer, 10).unwrap().is_empty());
        assert!(db.may_send_message(id, peer, deadline - 1).unwrap());
        assert!(!db.may_send_message(id, peer, deadline).unwrap());
        assert!(db
            .expire_outgoing_delivery(deadline - 1)
            .unwrap()
            .is_empty());
        let notice = DeliveryExpiryNotice {
            message_id: id,
            contact_id: Some(peer),
            group_id: None,
            expired_at: deadline,
        };
        assert_eq!(
            db.expire_outgoing_delivery(deadline).unwrap(),
            vec![notice.clone()]
        );
        assert!(db
            .expire_outgoing_delivery(deadline + 1000)
            .unwrap()
            .is_empty());
        let msg = db.get_message(id).unwrap().unwrap();
        assert!(!msg.sent && !msg.delivered);
        assert_eq!(msg.body, "retained");
        assert_eq!(msg.delivery_expired_at, Some(deadline));
        assert_eq!(
            db.get_message(in_id).unwrap().unwrap().delivery_expired_at,
            None
        );

        db.dispatch_delivery_expiry_notices(|_| false).unwrap();
        drop(db);
        let db = Db::open_plain(&path).unwrap();
        assert_eq!(
            db.list_delivery_expiry_notices().unwrap(),
            vec![notice.clone()]
        );
        assert_eq!(db.list_delivery_expiry_notices().unwrap(), vec![notice]);
        db.acknowledge_delivery_expiry_notices(&[id]).unwrap();
        assert!(db.list_delivery_expiry_notices().unwrap().is_empty());
        assert!(db.expire_outgoing_delivery(deadline).unwrap().is_empty());
    }

    #[test]
    fn partial_group_stops_only_unacknowledged_file_peer_at_delivery_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_plain(&dir.path().join("expiry.db")).unwrap();
        db.create_group(b"group", "Group").unwrap();
        let a = db.add_contact(&[1; 32], &[2; 32], "", "A", None).unwrap();
        let b = db.add_contact(&[3; 32], &[4; 32], "", "B", None).unwrap();
        let sent_at = now_ms();
        let deadline = sent_at + RETRY_TTL_MS;
        let content_expiry = deadline + 50;
        let attachment = NewGroupAttachment {
            attachment: NewAttachment {
                name: "file".into(),
                size: 8,
                key: vec![5; 32],
                path: "cipher".into(),
                chunk_size: Some(4),
            },
            file_offer: Some(([6; 16], [7; 32])),
        };
        let id = db
            .commit_outgoing_group(
                &NewOutgoingGroup {
                    group_id: b"group",
                    sender_sign_pk: &[8; 32],
                    body: "media",
                    sent_at,
                    expires_at: Some(content_expiry),
                    reply_to: None,
                    buttons: None,
                    notify_sound: None,
                    recipients: &[a, b],
                },
                &[attachment],
            )
            .unwrap();
        assert_eq!(
            db.acknowledge_group_delivery(id, a).unwrap(),
            GroupDeliveryAck::PendingRecipients
        );
        for after_sweep in [false, true] {
            if after_sweep {
                assert_eq!(db.expire_outgoing_delivery(deadline).unwrap().len(), 1);
            }
            assert!(!db.may_send_message(id, b, deadline).unwrap());
            assert!(db.may_send_file_part(&[6; 16], a, deadline).unwrap());
            assert!(!db.may_send_file_part(&[6; 16], b, deadline).unwrap());
            assert!(!db.may_send_file_part(&[6; 16], a, content_expiry).unwrap());
        }
        assert_eq!(
            db.acknowledge_group_delivery(id, b).unwrap(),
            GroupDeliveryAck::NotPending
        );
        assert!(!db.get_message(id).unwrap().unwrap().delivered);
        assert_eq!(db.purge_expired(content_expiry).unwrap(), 1);
        assert!(db.get_message(id).unwrap().is_none());
        let mut events = Vec::new();
        db.dispatch_delivery_expiry_notices(|notice| {
            events.push(notice);
            true
        })
        .unwrap();
        assert_eq!(
            events,
            vec![DeliveryExpiryNotice {
                message_id: id,
                group_id: Some(hex(b"group")),
                contact_id: None,
                expired_at: deadline
            }]
        );
        assert!(db.list_delivery_expiry_notices().unwrap().is_empty());
    }

    #[test]
    fn shorter_content_timer_rejects_late_group_ack_before_sweep_and_preserves_prior_dm_ack() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_plain(&dir.path().join("expiry.db")).unwrap();
        let peer = db.add_contact(&[1; 32], &[2; 32], "", "Bob", None).unwrap();
        db.create_group(b"group", "Group").unwrap();
        let sent = now_ms() - 100;
        let expired = sent + 1;
        let group = db
            .insert_group_message(
                b"group",
                Some(&[3; 32]),
                Direction::Out,
                "short",
                sent,
                Some(expired),
                &[],
            )
            .unwrap();
        db.queue_group_recipients(group, &[peer]).unwrap();
        let dm = db
            .insert_message(peer, Direction::Out, "done", sent, None, &[])
            .unwrap();
        assert!(db.acknowledge_dm_delivery(dm, peer).unwrap());
        assert!(!db.acknowledge_dm_delivery(dm, peer).unwrap());
        assert!(!db.pending_outbound_record_attempt(group, peer).unwrap());
        assert_eq!(
            db.acknowledge_group_delivery(group, peer).unwrap(),
            GroupDeliveryAck::NotPending
        );
        let notices = db
            .expire_outgoing_delivery(sent + RETRY_TTL_MS + 1)
            .unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(
            (notices[0].message_id, notices[0].expired_at),
            (group, expired)
        );
        assert_eq!(
            db.get_message(dm).unwrap().unwrap().delivery_expired_at,
            None
        );
    }
}
