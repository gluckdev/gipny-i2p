//! Direct message sending, replies, edits, pins, and typing.

use super::*;

impl Core {
    pub async fn send_message(
        &self,
        contact_id: i64,
        body: String,
        attachments: Vec<PendingAttachment>,
        ttl: Option<Duration>,
        reply_to: Option<i64>,
    ) -> Result<i64> {
        let sent_at = now_ms();
        let expires_at = ttl.map(|d| sent_at + d.as_millis() as i64);
        let staged = self.store_outgoing(&attachments)?;
        let msg_id = staged.publish_dm(
            &self.db,
            &NewOutgoingDm {
                contact_id,
                body: &body,
                sent_at,
                expires_at,
                reply_to,
                buttons: None,
                notify_sound: None,
                console: None,
            },
        )?;
        self.send_kick.notify_one();
        Ok(msg_id)
    }

    pub(super) fn build_wire_reply(&self, local_id: i64) -> Result<Option<WireReply>> {
        let m = match self.db.get_message(local_id)? {
            Some(m) => m,
            None => return Ok(None),
        };
        match m.direction {
            Direction::Out => Ok(Some(WireReply {
                sender_sign_pk: self.identity.card().sign_pk.to_vec(),
                origin_msg_id: m.id as u64,
            })),
            Direction::In => {
                let sign_pk = match m.sender_sign_pk.clone() {
                    Some(s) => s,
                    None => {
                        let cid = m.contact_id.ok_or(CoreError::State)?;
                        self.db
                            .get_contact(cid)?
                            .ok_or(CoreError::NotFound)?
                            .identity_sign
                    }
                };
                let origin = self.db.message_origin(m.id)?.unwrap_or(m.id);
                Ok(Some(WireReply {
                    sender_sign_pk: sign_pk,
                    origin_msg_id: origin as u64,
                }))
            }
        }
    }

    pub async fn press_button(
        self: &Arc<Self>,
        contact_id: i64,
        message_id: i64,
        callback_data: String,
    ) -> Result<()> {
        let contact = self
            .db
            .get_contact(contact_id)?
            .ok_or(CoreError::NotFound)?;
        let origin_msg_id = self.db.message_origin(message_id)?.unwrap_or(message_id);
        let mut payload = WirePayload {
            origin_msg_id: origin_msg_id as u64,
            body: String::new(),
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: None,
            buttons: None,
            callback_data: Some(callback_data),
            edit_of: None,
            pin: None,
            ack_for: None,
            sender_name: None,
            reply_to: None,
            typing: None,
            notify_sound: None,
            console: None,
            relay_address: None,
            wipe: None,
            files: Vec::new(),
            file_chunk: None,
            file_ack: None,
            file_cancel: None,
            dm: None,
        };
        let out = self.route_for(&contact).await.ok_or(CoreError::State)?;
        self.ensure_session_for(&contact, &out).await?;
        self.send_payload_via_relay(&contact, &mut payload, &out)
            .await
    }

    pub async fn send_edit(
        self: &Arc<Self>,
        contact_id: i64,
        message_id: i64,
        new_body: String,
    ) -> Result<()> {
        let contact = self
            .db
            .get_contact(contact_id)?
            .ok_or(CoreError::NotFound)?;
        let msg = self
            .db
            .get_message(message_id)?
            .ok_or(CoreError::NotFound)?;
        if msg.deleted_at.is_some() {
            return Err(CoreError::State);
        }
        if !matches!(msg.direction, Direction::Out) || msg.contact_id != Some(contact_id) {
            return Err(CoreError::State);
        }
        self.db.update_message_body(message_id, &new_body)?;
        let buttons: Option<Vec<Vec<WireButton>>> = self
            .db
            .get_setting(&format!("buttons_{}", message_id))
            .ok()
            .flatten()
            .and_then(|b| bincode::deserialize(&b).ok());
        if !self.db.if_message_live(message_id, || {
            let _ = self.events.try_send(CoreEvent::MessageEdited {
                message_id,
                body: new_body.clone(),
                buttons: buttons.clone(),
            });
        })? {
            return Err(CoreError::State);
        }
        let mut payload = WirePayload {
            origin_msg_id: 0,
            body: new_body,
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: None,
            buttons,
            callback_data: None,
            edit_of: Some(message_id as u64),
            pin: None,
            ack_for: None,
            sender_name: None,
            reply_to: None,
            typing: None,
            notify_sound: None,
            console: None,
            relay_address: None,
            wipe: None,
            files: Vec::new(),
            file_chunk: None,
            file_ack: None,
            file_cancel: None,
            dm: None,
        };
        let out = self.route_for(&contact).await.ok_or(CoreError::State)?;
        self.ensure_session_for(&contact, &out).await?;
        self.send_payload_via_relay(&contact, &mut payload, &out)
            .await
    }

    pub async fn pin_contact_message(
        self: &Arc<Self>,
        contact_id: i64,
        message_id: i64,
        unpin: bool,
    ) -> Result<()> {
        let msg = self
            .db
            .get_message(message_id)?
            .ok_or(CoreError::NotFound)?;
        if msg.contact_id != Some(contact_id) {
            return Err(CoreError::State);
        }

        let (sender_sign_pk, origin) = match msg.direction {
            Direction::Out => (self.identity.card().sign_pk.to_vec(), message_id as u64),
            Direction::In => {
                let contact = self
                    .db
                    .get_contact(contact_id)?
                    .ok_or(CoreError::NotFound)?;
                let origin = self.db.message_origin(message_id)?.unwrap_or(message_id);
                (contact.identity_sign, origin as u64)
            }
        };

        if unpin {
            self.db.unpin_contact_message(contact_id, message_id)?;
        } else {
            self.db.pin_contact_message(contact_id, message_id)?;
        }

        let ev = if unpin {
            CoreEvent::MessageUnpinned {
                contact_id: Some(contact_id),
                group_id: None,
                message_id,
            }
        } else {
            CoreEvent::MessagePinned {
                contact_id: Some(contact_id),
                group_id: None,
                message_id,
            }
        };
        let _ = self.events.try_send(ev);

        let mut payload = WirePayload {
            origin_msg_id: 0,
            body: String::new(),
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: None,
            buttons: None,
            callback_data: None,
            edit_of: None,
            pin: Some(WirePin {
                sender_sign_pk,
                origin_msg_id: origin,
                unpin,
            }),
            ack_for: None,
            sender_name: None,
            reply_to: None,
            typing: None,
            notify_sound: None,
            console: None,
            relay_address: None,
            wipe: None,
            files: Vec::new(),
            file_chunk: None,
            file_ack: None,
            file_cancel: None,
            dm: None,
        };
        let contact = self
            .db
            .get_contact(contact_id)?
            .ok_or(CoreError::NotFound)?;
        let out = self.route_for(&contact).await.ok_or(CoreError::State)?;
        self.ensure_session_for(&contact, &out).await?;
        self.send_payload_via_relay(&contact, &mut payload, &out)
            .await
    }

    pub async fn send_call_frame(
        self: &Arc<Self>, contact_id: i64, frame: String, lane: usize,
    ) -> Result<()> {
        if frame.is_empty() || frame.len() > 8192 || lane > 3 { return Err(CoreError::State); }
        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
        if contact.request_state != RequestState::None || contact.trust == TrustLevel::Blocked {
            return Err(CoreError::State);
        }
        let main = self.relay_for(&contact).await.ok_or(CoreError::State)?;
        self.ensure_session_for(&contact, &Route::Relay(main.clone())).await?;
        let lanes = self.relay_lanes(&contact, &main, lane + 1).await;
        let mut payload = make_typing_payload(None, false);
        payload.body = format!("gipny-call/1 {frame}");
        self.send_payload_via_relay(&contact, &mut payload, &Route::Relay(lanes[lane % lanes.len()].clone())).await
    }

    pub async fn send_typing_dm(self: &Arc<Self>, contact_id: i64, typing: bool) -> Result<()> {
        if !self.sessions.lock().await.contains_key(&contact_id) {
            return Ok(());
        }
        let mut payload = make_typing_payload(None, typing);
        let _ = self.send_to_contact(contact_id, &mut payload).await;
        Ok(())
    }
}
