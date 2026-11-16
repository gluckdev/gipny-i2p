//! Message deletion and authenticated DM controls.

use super::*;

impl SessionManager {
    /// Delete an incoming or outgoing DM for both endpoints, keeping its number.
    pub fn delete_message(&self, message_id: i64) -> Result<()> {
        let deletion = self.db.delete_dm_message(message_id)?;
        self.emit_dm_deleted(&deletion)
    }

    pub fn clear_message_history(&self) -> Result<usize> {
        let deletions = self.db.clear_dm_history()?;
        for deletion in &deletions {
            self.emit_dm_deleted(deletion)?;
        }
        Ok(deletions.iter().filter(|deletion| deletion.changed).count())
    }

    pub fn clear_dm_contact(&self, contact_id: i64) -> Result<usize> {
        let deletions = self.db.clear_dm_contact(contact_id)?;
        for deletion in &deletions {
            self.emit_dm_deleted(deletion)?;
        }
        Ok(deletions.iter().filter(|deletion| deletion.changed).count())
    }

    pub fn acknowledge_deletion_notices(&self, contact_id: i64, message_ids: &[i64]) -> Result<()> {
        for deletion in self
            .db
            .acknowledge_deletion_notices(contact_id, message_ids)?
        {
            self.emit_dm_deleted(&deletion)?;
        }
        Ok(())
    }

    pub(super) fn emit_dm_deleted(&self, deletion: &crate::db::DmDelete) -> Result<()> {
        if let Some(flow) = self
            .flows
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(&deletion.contact_id)
        {
            for file in &deletion.files {
                flow.forget(*file);
            }
        }
        self.db
            .cleanup_deleted_files(&self.data_dir.join(ATTACHMENTS_DIR))?;
        let message = self
            .db
            .get_message(deletion.message_id)?
            .ok_or(SessionError::NotFound)?;
        let _ = self.events.try_send(SessionEvent::MessageDeleted {
            contact_id: deletion.contact_id,
            message_id: deletion.message_id,
            deleted_at: message.deleted_at.ok_or(SessionError::State)?,
            pending: self.db.dm_delete_pending(deletion.message_id)?,
            deletion_notice: self.db.dm_deletion_notice(deletion.message_id)?,
        });
        self.send_kick.notify_one();
        Ok(())
    }

    pub(super) async fn handle_dm_control(
        self: &Arc<Self>,
        contact_id: i64,
        payload: &WirePayload,
    ) -> Result<bool> {
        let Some(dm) = &payload.dm else {
            return Ok(false);
        };
        if dm.kind == 0 {
            return Ok(false);
        }
        if !dm.is_control_payload(payload) {
            return Err(SessionError::State);
        }
        match dm.kind {
            1 => {
                let deleted =
                    self.db
                        .receive_dm_delete_at(contact_id, &dm.reference, payload.sent_at)?;
                self.emit_dm_deleted(&deleted)?;
                let contact = self
                    .db
                    .get_contact(contact_id)?
                    .ok_or(SessionError::NotFound)?;
                let mut ack = WirePayload::simple(0, String::new(), vec![], now_ms(), None);
                ack.dm = Some(crate::session::WireDm {
                    kind: 2,
                    reference: dm.reference.clone(),
                });
                if let Some(route) = self.route_for(&contact).await {
                    let _ = self
                        .send_payload_via_relay(&contact, &mut ack, &route)
                        .await;
                }
            }
            2 => {
                if self.db.acknowledge_dm_delete(contact_id, &dm.reference)? {
                    if let Some(message_id) =
                        self.db.dm_deleted_message_id(contact_id, &dm.reference)?
                    {
                        self.emit_dm_deleted(&crate::db::DmDelete {
                            contact_id,
                            message_id,
                            changed: false,
                            files: vec![],
                        })?;
                    }
                }
            }
            3 => {
                if let Some(message_id) =
                    self.db.receive_dm_notice_ack(contact_id, &dm.reference)?
                {
                    self.emit_dm_deleted(&crate::db::DmDelete {
                        contact_id,
                        message_id,
                        changed: false,
                        files: vec![],
                    })?;
                    let contact = self
                        .db
                        .get_contact(contact_id)?
                        .ok_or(SessionError::NotFound)?;
                    let mut ack = WirePayload::simple(0, String::new(), vec![], now_ms(), None);
                    ack.dm = Some(crate::session::WireDm {
                        kind: 4,
                        reference: dm.reference.clone(),
                    });
                    if let Some(route) = self.route_for(&contact).await {
                        let _ = self
                            .send_payload_via_relay(&contact, &mut ack, &route)
                            .await;
                    }
                }
            }
            4 => self.db.confirm_dm_notice_ack(contact_id, &dm.reference)?,
            // Headless: nobody looks at these here, but they are kept like in
            // the app and confirmed, so the contact stops repeating them.
            kind if crate::db::is_peer_event_kind(kind) => {
                self.db
                    .receive_peer_event(contact_id, &dm.reference, kind, payload.sent_at)?;
                let contact = self
                    .db
                    .get_contact(contact_id)?
                    .ok_or(SessionError::NotFound)?;
                let mut receipt = WirePayload::simple(0, String::new(), vec![], now_ms(), None);
                receipt.dm = Some(crate::session::WireDm {
                    kind: kind + crate::db::PEER_EVENT_RECEIPT_OFFSET,
                    reference: dm.reference.clone(),
                });
                if let Some(route) = self.route_for(&contact).await {
                    let _ = self
                        .send_payload_via_relay(&contact, &mut receipt, &route)
                        .await;
                }
            }
            kind if kind
                .checked_sub(crate::db::PEER_EVENT_RECEIPT_OFFSET)
                .is_some_and(crate::db::is_peer_event_kind) =>
            {
                self.db.confirm_peer_event(
                    contact_id,
                    &dm.reference,
                    kind - crate::db::PEER_EVENT_RECEIPT_OFFSET,
                )?;
            }
            _ => return Err(SessionError::State),
        }
        Ok(true)
    }

    pub(super) async fn reject_deleted_dm_content(
        self: &Arc<Self>,
        contact_id: i64,
        payload: &WirePayload,
    ) -> Result<bool> {
        if payload.group.is_some() {
            if payload.dm.is_some() {
                return Err(SessionError::State);
            }
            return Ok(false);
        }
        let contact = self
            .db
            .get_contact(contact_id)?
            .ok_or(SessionError::NotFound)?;
        let ordinary = payload.origin_msg_id > 0
            && payload.edit_of.is_none()
            && payload.ack_for.is_none()
            && payload.callback_data.is_none();
        let reference = if let Some(dm) = &payload.dm {
            if dm.kind != 0
                || dm.reference.origin == 0
                || dm.reference.origin > i64::MAX as u64
                || dm.reference.author.as_slice() != contact.identity_sign.as_slice()
                || (ordinary
                    && (dm.reference.origin != payload.origin_msg_id
                        || dm.reference.sent_at != payload.sent_at))
            {
                return Err(SessionError::State);
            }
            Some(dm.reference.clone())
        } else if ordinary {
            Some(crate::db::DmReference {
                author: to_arr32(contact.identity_sign.clone())?,
                origin: payload.origin_msg_id,
                sent_at: payload.sent_at,
            })
        } else {
            None
        };
        let Some(reference) = reference else {
            return Ok(false);
        };
        let mut files: Vec<_> = payload.files.iter().map(|file| file.file_id).collect();
        if let Some(chunk) = &payload.file_chunk {
            files.push(chunk.file_id);
        }
        if self
            .db
            .reject_deleted_dm_files(contact_id, &reference, &files)?
        {
            let _ = self.send_ack(&contact, payload).await;
            return Ok(true);
        }
        if ordinary
            && payload
                .ttl_ms
                .is_some_and(|ttl| ttl > 0 && payload.sent_at.saturating_add(ttl) <= now_ms())
        {
            let deletion = self.db.expire_incoming_dm(contact_id, &reference)?;
            self.db
                .reject_deleted_dm_files(contact_id, &reference, &files)?;
            self.emit_dm_deleted(&deletion)?;
            return Ok(true);
        }
        Ok(false)
    }
}
