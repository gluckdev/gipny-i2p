//! Metadata for accepted messages and their receipts outlives transcript cleanup.
//! A bounded replay record prevents duplicate packets from restoring cleared history.
use super::*;
use crate::session::{WireGroupRef, WireMember, WirePayload};

pub struct DeliveryReceipt {
    pub message_id: i64,
    origin: i64,
    group: Option<Vec<u8>>,
}

impl WireGroupRef {
    pub fn from_stored_members(
        id: &[u8],
        names: &HashMap<Vec<u8>, String>,
        rosters: &HashMap<Vec<u8>, Vec<GroupMember>>,
    ) -> Self {
        Self {
            id: id.to_vec(),
            name: names.get(id).cloned().unwrap_or_default(),
            members: rosters
                .get(id)
                .into_iter()
                .flatten()
                .map(|member| WireMember {
                    sign_pk: member.sign_pk.clone(),
                    dh_pk: member.dh_pk.clone(),
                    i2p_address: member.i2p_address.clone(),
                    name: member.display_name.clone(),
                })
                .collect(),
        }
    }
}

impl DeliveryReceipt {
    pub fn payload(
        &self,
        now: i64,
        names: &HashMap<Vec<u8>, String>,
        rosters: &HashMap<Vec<u8>, Vec<GroupMember>>,
        me: &[u8],
        peer: &[u8],
    ) -> WirePayload {
        let mut payload = WirePayload::simple(0, String::new(), vec![], now, None);
        payload.ack_for = Some(self.origin as u64);
        payload.group = self.group.as_ref().map(|id| {
            let mut group = WireGroupRef::from_stored_members(id, names, rosters);
            // Earlier browser clients require the two endpoints in a group
            // reference. A receipt never introduces other group members.
            group
                .members
                .retain(|member| member.sign_pk == me || member.sign_pk == peer);
            group
        });
        payload
    }
}

impl Db {
    pub(super) fn migrate_delivery_receipts(conn: &Connection) -> Result<()> {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS delivery_receipts (
            message_id INTEGER PRIMARY KEY,
            contact_id INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
            origin INTEGER NOT NULL CHECK(origin>0),
            group_id BLOB,
            deadline INTEGER NOT NULL,
            pending INTEGER NOT NULL CHECK(pending IN (0,1))
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_delivery_receipt_origin ON delivery_receipts(contact_id,origin,coalesce(group_id,x''));
        CREATE INDEX IF NOT EXISTS idx_delivery_receipt_pending ON delivery_receipts(contact_id,message_id) WHERE pending=1;
        CREATE INDEX IF NOT EXISTS idx_delivery_receipt_deadline ON delivery_receipts(deadline);")?;
        // Profiles opened by the original in-row implementation may already
        // have outstanding receipts. Move them once before retention runs.
        let legacy: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM pragma_table_info('messages') WHERE name='receipt_to')",
            [],
            |r| r.get(0),
        )?;
        if legacy {
            let tx = conn.unchecked_transaction()?;
            tx.execute("INSERT OR IGNORE INTO delivery_receipts(message_id,contact_id,origin,group_id,deadline,pending)
                SELECT id,receipt_to,origin_msg_id,group_id,CAST(MIN(COALESCE(expires_at,9223372036854775807),sent_at+?1,?2) AS INTEGER),1
                FROM messages WHERE receipt_to IS NOT NULL AND origin_msg_id>0",params![RETRY_TTL_MS,now_ms().saturating_add(RETRY_TTL_MS)])?;
            tx.execute(
                "UPDATE messages SET receipt_to=NULL WHERE receipt_to IS NOT NULL",
                [],
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    pub(super) fn queue_stored_receipt(
        conn: &Connection,
        contact: i64,
        id: i64,
        now: i64,
    ) -> Result<()> {
        conn.execute("INSERT INTO delivery_receipts(message_id,contact_id,origin,group_id,deadline,pending)
            SELECT id,?1,origin_msg_id,group_id,CAST(MIN(COALESCE(expires_at,9223372036854775807),sent_at+?4,?5) AS INTEGER),1
            FROM messages WHERE id=?2 AND origin_msg_id>0 AND direction=0 AND deleted_at IS NULL
            AND sent_at>?3 AND (expires_at IS NULL OR expires_at>?6)
            ON CONFLICT(message_id) DO UPDATE SET pending=1 WHERE delivery_receipts.deadline>?6",
            params![contact,id,now.saturating_sub(RETRY_TTL_MS),RETRY_TTL_MS,now.saturating_add(RETRY_TTL_MS),now])?;
        Ok(())
    }

    /// Only a previously accepted envelope can refresh this replay record.
    pub(super) fn requeue_received_origin(
        conn: &Connection,
        contact: i64,
        origin: i64,
        group: Option<&[u8]>,
        now: i64,
    ) -> Result<Option<i64>> {
        Ok(conn.query_row("UPDATE delivery_receipts SET pending=1
            WHERE contact_id=?1 AND origin=?2 AND group_id IS ?3 AND deadline>?4 RETURNING message_id",
            params![contact,origin,group,now], |r| r.get(0)).optional()?)
    }

    pub fn queue_delivery_receipt(
        &self,
        contact: i64,
        original: &WirePayload,
        now: i64,
    ) -> Result<()> {
        let Ok(origin) = i64::try_from(original.origin_msg_id) else {
            return Ok(());
        };
        if origin <= 0 {
            return Ok(());
        }
        let group = original.group.as_ref().map(|g| g.id.as_slice());
        self.with_tx(|tx| {
            if Self::requeue_received_origin(tx,contact,origin,group,now)?.is_some() {return Ok(())}
            // A duplicate from before the receipt ledger was introduced can
            // still be matched to a retained message by its authenticated author.
            let id=tx.query_row("SELECT id FROM messages WHERE origin_msg_id=?2 AND direction=0
                AND ((group_id IS NULL AND ?3 IS NULL AND contact_id=?1)
                OR (group_id=?3 AND sender_sign_pk=(SELECT identity_sign FROM contacts WHERE id=?1)))",
                params![contact,origin,group],|r|r.get(0)).optional()?;
            if let Some(id)=id { Self::queue_stored_receipt(tx,contact,id,now)?; }
            Ok(())
        })
    }

    pub fn pending_delivery_receipts(
        &self,
        contact: i64,
        now: i64,
    ) -> Result<Vec<DeliveryReceipt>> {
        self.with_conn(|conn| {
            conn.prepare_cached(
                "SELECT r.message_id,r.origin,r.group_id
            FROM delivery_receipts r JOIN contacts c ON r.contact_id=c.id
            WHERE c.id=?1 AND c.trust!=2 AND c.request_state!=1 AND r.pending=1 AND r.deadline>?2
            ORDER BY r.message_id LIMIT 50",
            )?
            .query_map(params![contact, now], |row| {
                Ok(DeliveryReceipt {
                    message_id: row.get(0)?,
                    origin: row.get(1)?,
                    group: row.get(2)?,
                })
            })?
            .collect_rows()
        })
    }

    pub fn finish_delivery_receipt(&self, contact: i64, message: i64) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE delivery_receipts SET pending=0 WHERE message_id=?1 AND contact_id=?2",
                params![message, contact],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logout_clears_both_transcripts_and_attachments_but_keeps_receipts_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("retention.db");
        let files = dir.path().join("files");
        std::fs::create_dir(&files).unwrap();
        let db = Db::open_plain(&path).unwrap();
        db.configure_dm_identity(&[7; 32]).unwrap();
        let peer = db
            .add_contact(&[1; 32], &[2; 32], "", "peer", None)
            .unwrap();
        let gid = vec![3; 32];
        db.create_group(&gid, "group").unwrap();
        let prefs = serde_json::json!({"clearOnLock":false,"saveHistory":{
            format!("contact:{peer}"):false,format!("group:{}",hex(&gid)):false
        }});
        db.set_setting("ui_chat_prefs", prefs.to_string().as_bytes())
            .unwrap();
        let now = now_ms();
        let mut dm = WirePayload::simple(73, "private DM".into(), vec![], now, Some(300_000));
        dm.attachments.push(crate::session::WireAttachment {
            name: "private.txt".into(),
            data: b"private attachment".to_vec(),
        });
        let mut group = dm.clone();
        group.body = "private group".into();
        group.group = Some(WireGroupRef {
            id: gid.clone(),
            name: "group".into(),
            members: vec![],
        });
        let accept = |db: &Db, payload: &WirePayload| {
            let mut staged = crate::files::StagedAttachments::new(&files).unwrap();
            let file = &payload.attachments[0];
            staged.inline(&file.name, &file.data).unwrap();
            staged.publish_incoming(db, peer, payload).unwrap()
        };
        let direct = accept(&db, &dm).id;
        let grouped = accept(&db, &group).id;
        assert_eq!(std::fs::read_dir(&files).unwrap().count(), 2);
        assert_eq!(db.clear_unsaved_transcripts().unwrap(), 2);
        db.cleanup_deleted_files(&files).unwrap();
        drop(db);
        let db = Db::open_plain(&path).unwrap();
        let redacted = db.get_message(direct).unwrap().unwrap();
        assert!(redacted.body.is_empty() && redacted.deleted_at.is_some());
        assert!(db.get_message(grouped).unwrap().is_none());
        assert!(db.list_attachments(direct).unwrap().is_empty());
        assert!(db.list_attachments(grouped).unwrap().is_empty());
        assert_eq!(std::fs::read_dir(&files).unwrap().count(), 0);
        let due = db.pending_delivery_receipts(peer, now).unwrap();
        assert_eq!(due.len(), 2);
        assert!(due[0].group.is_none());
        assert_eq!(due[1].group.as_deref(), Some(gid.as_slice()));
        for receipt in due {
            db.finish_delivery_receipt(peer, receipt.message_id)
                .unwrap();
        }
        assert!(db.pending_delivery_receipts(peer, now).unwrap().is_empty());
        // A lost ACK can bring back the same encrypted group packet. Do not
        // recreate a cleared transcript or leave the newly staged file behind.
        let replay = accept(&db, &group);
        assert!(replay.deleted && !replay.inserted);
        assert_eq!(replay.id, grouped);
        assert!(db.get_message(grouped).unwrap().is_none());
        assert_eq!(std::fs::read_dir(&files).unwrap().count(), 0);
        assert_eq!(db.pending_delivery_receipts(peer, now).unwrap().len(), 1);
        db.queue_delivery_receipt(peer, &dm, now).unwrap();
        assert_eq!(db.pending_delivery_receipts(peer, now).unwrap().len(), 2);
        db.purge_expired(now + 300_000).unwrap();
        assert!(db.pending_delivery_receipts(peer, now).unwrap().is_empty());
    }

    #[test]
    fn committed_receipts_survive_restart_and_are_scoped_to_sender_and_lifetime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("receipt.db");
        let db = Db::open_plain(&path).unwrap();
        let peer = db
            .add_contact(&[1; 32], &[2; 32], "", "peer", None)
            .unwrap();
        let stranger = db
            .add_contact(&[3; 32], &[4; 32], "", "other", None)
            .unwrap();
        let now = now_ms();
        let dm = WirePayload::simple(42, "one".into(), vec![], now, Some(300_000));
        db.set_contact_request_state(peer, RequestState::Incoming)
            .unwrap();
        let dm_id = db.commit_incoming_message(peer, &dm, &[]).unwrap().id;
        assert!(db.pending_delivery_receipts(peer, now).unwrap().is_empty());
        db.set_contact_request_state(peer, RequestState::None)
            .unwrap();
        let group_id = vec![7; 32];
        db.create_group(&group_id, "group").unwrap();
        let mut grouped = dm.clone();
        grouped.group = Some(WireGroupRef {
            id: group_id.clone(),
            name: "group".into(),
            members: vec![],
        });
        let group_message = db.commit_incoming_message(peer, &grouped, &[]).unwrap().id;
        let stranger_message = db
            .commit_incoming_message(stranger, &grouped, &[])
            .unwrap()
            .id;
        drop(db);
        let db = Db::open_plain(&path).unwrap();
        let pending = db.pending_delivery_receipts(peer, now).unwrap();
        assert_eq!(
            pending.iter().map(|r| r.message_id).collect::<Vec<_>>(),
            vec![dm_id, group_message]
        );
        assert_eq!(pending[1].origin, 42);
        assert_eq!(pending[1].group.as_deref(), Some(group_id.as_slice()));
        for key in [1, 3, 5] {
            db.add_group_member(
                &group_id,
                &[key; 32],
                &[key + 1; 32],
                "",
                "member",
                key == 5,
            )
            .unwrap();
        }
        let payload = pending[1].payload(
            now,
            &HashMap::new(),
            &db.list_all_group_members().unwrap(),
            &[5; 32],
            &[1; 32],
        );
        assert_eq!(payload.ack_for, Some(42));
        assert!(payload.body.is_empty());
        let group = payload.group.unwrap();
        assert_eq!(group.id, group_id);
        assert_eq!(group.members.len(), 2);
        assert!(group
            .members
            .iter()
            .all(|member| member.sign_pk != vec![3; 32]));

        for receipt in pending {
            db.finish_delivery_receipt(peer, receipt.message_id)
                .unwrap();
        }
        db.finish_delivery_receipt(peer, stranger_message).unwrap();
        assert_eq!(
            db.pending_delivery_receipts(stranger, now).unwrap().len(),
            1
        );
        assert!(db.pending_delivery_receipts(peer, now).unwrap().is_empty());
        db.queue_delivery_receipt(peer, &grouped, now).unwrap();
        db.queue_delivery_receipt(peer, &grouped, now).unwrap();
        let pending = db.pending_delivery_receipts(peer, now).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].message_id, group_message);
        assert!(db
            .pending_delivery_receipts(peer, now + 300_000)
            .unwrap()
            .is_empty());
        db.finish_delivery_receipt(peer, group_message).unwrap();
        db.queue_delivery_receipt(peer, &grouped, now + 300_000)
            .unwrap();
        assert!(db.pending_delivery_receipts(peer, now).unwrap().is_empty());
    }
}
