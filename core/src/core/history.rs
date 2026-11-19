//! History retention, durable deletion, and peer-event controls.

use super::*;

impl Core {
    /// The interface's own data under `key` (see `UI_DATA_KEYS`), kept in the
    /// vault so it comes back with the profile. `None` when never saved.
    pub fn ui_data(&self, key: &str) -> Result<Option<String>> {
        if !UI_DATA_KEYS.contains(&key) {
            return Err(CoreError::NotFound);
        }
        Ok(self
            .db
            .get_setting(&format!("ui_{key}"))?
            .and_then(|v| String::from_utf8(v).ok()))
    }

    pub fn set_ui_data(&self, key: &str, json: &str) -> Result<()> {
        if !UI_DATA_KEYS.contains(&key) {
            return Err(CoreError::NotFound);
        }
        if json.len() > MAX_UI_DATA_BYTES {
            return Err(CoreError::Db(gipny_libcore::db::DbError::TooLarge));
        }
        self.db.set_setting(&format!("ui_{key}"), json.as_bytes())?;
        Ok(())
    }

    /// Empty the transcript, as the "clear history when I lock" setting asks.
    ///
    /// Runs before the vault is closed so it still has the unlocked database
    /// under it. DM pins do not override deletion. Group pins retain their
    /// previous local-only semantics; see `purge_all_messages`.
    pub fn clear_message_history(&self) -> Result<usize> {
        let deleted = self.db.clear_dm_history()?;
        for message in &deleted {
            self.emit_dm_deleted(message)?;
        }
        let dropped = self.db.purge_all_messages()?
            + deleted.iter().filter(|message| message.changed).count();
        // Also clean pins left by earlier versions or interrupted imports.
        self.db.cleanup_orphan_pins()?;
        Ok(dropped)
    }

    /// Automatic cleanup honours explicit per-chat choices; an explicit
    /// clear-all continues to use `clear_message_history`.
    pub fn clear_unsaved_history(&self) -> Result<usize> {
        let dropped = self.db.clear_unsaved_transcripts()?;
        self.db.cleanup_deleted_files(&self.data_dir.join(ATTACHMENTS_DIR))?;
        Ok(dropped)
    }

    pub fn clear_dm_contact(&self, contact_id: i64) -> Result<usize> {
        let deletions = self.db.clear_dm_contact(contact_id)?;
        for deletion in &deletions {
            self.emit_dm_deleted(deletion)?;
        }
        Ok(deletions.iter().filter(|deletion| deletion.changed).count())
    }

    /// Direct messages become durable tombstones; group deletion retains its
    /// previous local semantics and does not broadcast a new group operation.
    pub fn delete_message(&self, message_id: i64) -> Result<()> {
        let message = self
            .db
            .get_message(message_id)?
            .ok_or(CoreError::NotFound)?;
        if message.contact_id.is_none() || message.group_id.is_some() {
            return Ok(self.db.delete_message(message_id)?);
        }
        let deleted = self.db.delete_dm_message(message_id)?;
        self.emit_dm_deleted(&deleted)
    }

    /// Tell the contact we copied the text of, saved an attachment of, or
    /// pressed a screenshot shortcut over, a message of this direct chat.
    /// Queued and repeated until their client confirms it.
    pub fn report_peer_event(&self, message_id: i64, kind: u8) -> Result<()> {
        self.db.queue_peer_event(message_id, kind)?;
        self.send_kick.notify_one();
        Ok(())
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

    pub(super) fn emit_dm_deleted(&self, deletion: &gipny_libcore::db::DmDelete) -> Result<()> {
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
            .ok_or(CoreError::NotFound)?;
        let _ = self.events.try_send(CoreEvent::MessageDeleted {
            contact_id: deletion.contact_id,
            message_id: deletion.message_id,
            deleted_at: message.deleted_at.ok_or(CoreError::State)?,
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
            return Err(CoreError::State);
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
                    .ok_or(CoreError::NotFound)?;
                let mut ack = WirePayload::simple(0, String::new(), vec![], now_ms(), None);
                ack.dm = Some(gipny_libcore::session::WireDm {
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
                        self.emit_dm_deleted(&gipny_libcore::db::DmDelete {
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
                    self.emit_dm_deleted(&gipny_libcore::db::DmDelete {
                        contact_id,
                        message_id,
                        changed: false,
                        files: vec![],
                    })?;
                    let contact = self
                        .db
                        .get_contact(contact_id)?
                        .ok_or(CoreError::NotFound)?;
                    let mut ack = WirePayload::simple(0, String::new(), vec![], now_ms(), None);
                    ack.dm = Some(gipny_libcore::session::WireDm {
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
            kind if gipny_libcore::db::is_peer_event_kind(kind) => {
                if let Some(message_id) =
                    self.db
                        .receive_peer_event(contact_id, &dm.reference, kind, payload.sent_at)?
                {
                    let events = self.db.peer_events(message_id)?;
                    let _ = self.events.try_send(CoreEvent::PeerEvent {
                        contact_id,
                        message_id,
                        events,
                    });
                }
                // Confirmed even when the message is gone here, so the
                // contact stops repeating it.
                let contact = self
                    .db
                    .get_contact(contact_id)?
                    .ok_or(CoreError::NotFound)?;
                let mut receipt = WirePayload::simple(0, String::new(), vec![], now_ms(), None);
                receipt.dm = Some(gipny_libcore::session::WireDm {
                    kind: kind + gipny_libcore::db::PEER_EVENT_RECEIPT_OFFSET,
                    reference: dm.reference.clone(),
                });
                if let Some(route) = self.route_for(&contact).await {
                    let _ = self
                        .send_payload_via_relay(&contact, &mut receipt, &route)
                        .await;
                }
            }
            kind if kind
                .checked_sub(gipny_libcore::db::PEER_EVENT_RECEIPT_OFFSET)
                .is_some_and(gipny_libcore::db::is_peer_event_kind) =>
            {
                self.db.confirm_peer_event(
                    contact_id,
                    &dm.reference,
                    kind - gipny_libcore::db::PEER_EVENT_RECEIPT_OFFSET,
                )?;
            }
            _ => return Err(CoreError::State),
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
                return Err(CoreError::State);
            }
            return Ok(payload.content_expired(now_ms()));
        }
        let contact = self
            .db
            .get_contact(contact_id)?
            .ok_or(CoreError::NotFound)?;
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
                return Err(CoreError::State);
            }
            Some(dm.reference.clone())
        } else if ordinary {
            Some(gipny_libcore::db::DmReference {
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
        if payload.content_expired(now_ms()) {
            let deletion = self.db.expire_incoming_dm(contact_id, &reference)?;
            self.db
                .reject_deleted_dm_files(contact_id, &reference, &files)?;
            self.emit_dm_deleted(&deletion)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// The same, but only if the person asked for it, and saying whether it
    /// happened.
    ///
    /// Quitting from the tray never reaches the interface, so the preference has
    /// to be read on this side of the wall: by the time a quit arrives the
    /// window is already gone and there is nobody left to ask. New chats clear their local transcript on logout by default.
    pub fn clear_history_if_asked(&self) -> Result<bool> {
        Ok(self.clear_unsaved_history()? > 0)
    }
}
