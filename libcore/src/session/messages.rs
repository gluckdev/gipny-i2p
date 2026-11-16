//! Outgoing message and group operations.

use super::*;

impl SessionManager {
    pub async fn send_message(
        &self,
        contact_id: i64,
        body: String,
        attachments: Vec<(String, Vec<u8>)>,
        ttl: Option<Duration>,
        buttons: Option<Vec<Vec<WireButton>>>,
        notify_sound: Option<String>,
    ) -> Result<i64> {
        let sent_at = now_ms();
        let expires_at = ttl.map(|d| sent_at + d.as_millis() as i64);
        let buttons = buttons.as_ref().map(bincode::serialize).transpose()?;
        let staged = self.store_outgoing(&attachments)?;
        let msg_id = staged.publish_dm(
            &self.db,
            &NewOutgoingDm {
                contact_id,
                body: &body,
                sent_at,
                expires_at,
                reply_to: None,
                buttons: buttons.as_deref(),
                notify_sound: notify_sound.as_ref().map(|s| s.as_bytes()),
                console: None,
            },
        )?;
        self.send_kick.notify_one();
        Ok(msg_id)
    }

    /// Sends a console-framed message: a command, its output, or a control
    /// marker. The frame rides in `console_<id>` next to the row, as buttons
    /// and sounds do, and goes onto the wire when the send loop builds the
    /// payload — so it is queued, retried and acked like any other message.
    pub async fn send_console(
        &self,
        contact_id: i64,
        body: String,
        console: WireConsole,
        attachments: Vec<(String, Vec<u8>)>,
    ) -> Result<i64> {
        let sent_at = now_ms();
        let console = bincode::serialize(&console)?;
        let staged = self.store_outgoing(&attachments)?;
        let msg_id = staged.publish_dm(
            &self.db,
            &NewOutgoingDm {
                contact_id,
                body: &body,
                sent_at,
                expires_at: None,
                reply_to: None,
                buttons: None,
                notify_sound: None,
                console: Some(&console),
            },
        )?;
        self.send_kick.notify_one();
        Ok(msg_id)
    }

    pub async fn send_callback(self: &Arc<Self>, contact_id: i64, data: String) -> Result<()> {
        let mut payload = WirePayload {
            origin_msg_id: 0,
            body: String::new(),
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: None,
            buttons: None,
            callback_data: Some(data),
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
        self.send_to_contact(contact_id, &mut payload).await
    }

    pub async fn send_edit(
        self: &Arc<Self>,
        contact_id: i64,
        edit_target_origin: u64,
        new_body: String,
        buttons: Option<Vec<Vec<WireButton>>>,
    ) -> Result<()> {
        if self
            .db
            .outgoing_dm_by_origin(contact_id, edit_target_origin)?
            .is_none()
        {
            return Err(SessionError::NotFound);
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
            edit_of: Some(edit_target_origin),
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
        self.send_to_contact(contact_id, &mut payload).await
    }

    pub async fn send_to_group(
        self: &Arc<Self>,
        group_id: &[u8],
        body: String,
        attachments: Vec<(String, Vec<u8>)>,
        buttons: Option<Vec<Vec<WireButton>>>,
        notify_sound: Option<String>,
    ) -> Result<i64> {
        let sent_at = now_ms();
        let members = self.db.list_group_members(group_id)?;
        let mut contacts = Vec::new();
        for member in members.iter().filter(|member| !member.is_self) {
            if let Some(contact) = self.db.find_contact_by_identity(&member.dh_pk)? {
                if contact.trust != TrustLevel::Blocked {
                    contacts.push(contact);
                }
            }
        }
        let recipients: Vec<i64> = contacts.iter().map(|contact| contact.id).collect();
        let buttons_bytes = buttons.as_ref().map(bincode::serialize).transpose()?;
        let mut staged =
            crate::files::StagedAttachments::new(&self.data_dir.join(ATTACHMENTS_DIR))?;
        let mut wire_atts = Vec::new();
        for (name, data) in attachments {
            if !staged.can_inline(data.len()) {
                staged.parts(&name, &data[..])?;
            } else {
                staged.inline(&name, &data)?;
                wire_atts.push(WireAttachment { name, data });
            }
        }
        let offers = staged.offers();
        let gref_members: Vec<WireMember> = members
            .iter()
            .map(|m| WireMember {
                sign_pk: m.sign_pk.clone(),
                dh_pk: m.dh_pk.clone(),
                onion: m.onion.clone(),
                name: m.display_name.clone(),
            })
            .collect();
        let gname = self.db.get_group_name(group_id)?.unwrap_or_default();
        let gref = WireGroupRef {
            id: group_id.to_vec(),
            name: gname,
            members: gref_members,
        };
        let sender_sign_pk = self.identity.card().sign_pk;
        let msg_id = staged.publish(
            &self.db,
            &NewOutgoingGroup {
                group_id,
                sender_sign_pk: &sender_sign_pk,
                body: &body,
                sent_at,
                expires_at: None,
                reply_to: None,
                buttons: buttons_bytes.as_deref(),
                notify_sound: notify_sound.as_ref().map(|sound| sound.as_bytes()),
                recipients: &recipients,
            },
        )?;
        for recipient in recipients {
            // Earlier recipients may have awaited the network; honor a contact
            // deletion or block that happened while that attempt was pending.
            let contact = match self.db.get_contact(recipient) {
                Ok(Some(contact)) if contact.trust != TrustLevel::Blocked => contact,
                Err(error) => {
                    crate::debug_error!("[session] group msg {msg_id} queued; contact {recipient} lookup failed: {error:?}");
                    continue;
                }
                _ => continue,
            };
            let mut payload = WirePayload::simple(
                msg_id as u64,
                body.clone(),
                wire_atts.clone(),
                sent_at,
                None,
            );
            payload.files = offers.clone();
            payload.group = Some(gref.clone());
            payload.buttons = buttons.clone();
            payload.notify_sound = notify_sound.clone();
            if let Err(e) = attempt_group_delivery(
                &self.db,
                msg_id,
                contact.id,
                self.send_to_contact(contact.id, &mut payload),
            )
            .await
            {
                crate::debug_trace!(
                    "[session] group send to contact {} failed (msg {}): {:?}; queued",
                    contact.id,
                    msg_id,
                    e
                );
            }
        }
        Ok(msg_id)
    }

    pub async fn send_edit_group(
        self: &Arc<Self>,
        group_id: &[u8],
        edit_target_origin: u64,
        new_body: String,
        buttons: Option<Vec<Vec<WireButton>>>,
    ) -> Result<()> {
        let local_id = edit_target_origin as i64;
        let _ = self.db.update_message_body(local_id, &new_body);
        if let Some(b) = &buttons {
            if let Ok(bytes) = bincode::serialize(b) {
                let _ = self
                    .db
                    .set_setting(&format!("buttons_{}", local_id), &bytes);
            }
        } else {
            let _ = self.db.delete_setting(&format!("buttons_{}", local_id));
        }
        let members = self.db.list_group_members(group_id)?;
        let gref_members: Vec<WireMember> = members
            .iter()
            .map(|m| WireMember {
                sign_pk: m.sign_pk.clone(),
                dh_pk: m.dh_pk.clone(),
                onion: m.onion.clone(),
                name: m.display_name.clone(),
            })
            .collect();
        let gname = self.db.get_group_name(group_id)?.unwrap_or_default();
        let gref = WireGroupRef {
            id: group_id.to_vec(),
            name: gname,
            members: gref_members,
        };
        for m in members {
            if m.is_self {
                continue;
            }
            let contact = match self.db.find_contact_by_identity(&m.dh_pk)? {
                Some(c) => c,
                None => continue,
            };
            if contact.trust == TrustLevel::Blocked {
                continue;
            }
            let mut payload = WirePayload {
                origin_msg_id: 0,
                body: new_body.clone(),
                attachments: vec![],
                sent_at: now_ms(),
                ttl_ms: None,
                group: Some(gref.clone()),
                buttons: buttons.clone(),
                callback_data: None,
                edit_of: Some(edit_target_origin),
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
            let _ = self.send_to_contact(contact.id, &mut payload).await;
        }
        Ok(())
    }

    pub(super) async fn send_to_contact(
        self: &Arc<Self>,
        contact_id: i64,
        payload: &mut WirePayload,
    ) -> Result<()> {
        let contact = self
            .db
            .get_contact(contact_id)?
            .ok_or(SessionError::NotFound)?;
        let route = self.route_for(&contact).await.ok_or(SessionError::State)?;
        self.ensure_session_for(&contact, &route).await?;
        self.send_payload_via_relay(&contact, payload, &route).await
    }

    pub async fn send_pin_contact(
        self: &Arc<Self>,
        contact_id: i64,
        sender_sign_pk: Vec<u8>,
        origin_msg_id: u64,
        unpin: bool,
    ) -> Result<()> {
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
                origin_msg_id,
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
        self.send_to_contact(contact_id, &mut payload).await
    }
}
