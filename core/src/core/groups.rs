//! Group membership, sending, edits, pins, and typing.

use super::*;

impl Core {
    pub async fn press_group_button(
        self: &Arc<Self>,
        group_id: &[u8],
        message_id: i64,
        callback_data: String,
    ) -> Result<()> {
        let msg = self
            .db
            .get_message(message_id)?
            .ok_or(CoreError::NotFound)?;
        if msg.group_id.as_deref() != Some(group_id) {
            return Err(CoreError::State);
        }
        let sender_sign = msg.sender_sign_pk.clone().ok_or(CoreError::State)?;
        let contact = self
            .db
            .find_contact_by_sign_pk(&sender_sign)?
            .ok_or(CoreError::NotFound)?;
        let origin_msg_id = self.db.message_origin(message_id)?.unwrap_or(message_id);

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

        let mut payload = WirePayload {
            origin_msg_id: origin_msg_id as u64,
            body: String::new(),
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: Some(gref),
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

    pub async fn send_edit_group(
        self: &Arc<Self>,
        group_id: &[u8],
        message_id: i64,
        new_body: String,
    ) -> Result<()> {
        let msg = self
            .db
            .get_message(message_id)?
            .ok_or(CoreError::NotFound)?;
        if !matches!(msg.direction, Direction::Out) || msg.group_id.as_deref() != Some(group_id) {
            return Err(CoreError::State);
        }
        self.db.update_message_body(message_id, &new_body)?;
        let buttons: Option<Vec<Vec<WireButton>>> = self
            .db
            .get_setting(&format!("buttons_{}", message_id))
            .ok()
            .flatten()
            .and_then(|b| bincode::deserialize(&b).ok());
        let _ = self.events.try_send(CoreEvent::MessageEdited {
            message_id,
            body: new_body.clone(),
            buttons: buttons.clone(),
        });

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
            let _ = self.send_to_contact(contact.id, &mut payload).await;
        }
        Ok(())
    }

    pub async fn pin_group_message(
        self: &Arc<Self>,
        group_id: &[u8],
        message_id: i64,
        unpin: bool,
    ) -> Result<()> {
        let msg = self
            .db
            .get_message(message_id)?
            .ok_or(CoreError::NotFound)?;
        if msg.group_id.as_deref() != Some(group_id) {
            return Err(CoreError::State);
        }

        let (sender_sign_pk, origin) = match msg.sender_sign_pk.clone() {
            Some(pk) => {
                let origin = self.db.message_origin(message_id)?.unwrap_or(message_id);
                (pk, origin as u64)
            }
            None if matches!(msg.direction, Direction::Out) => {
                (self.identity.card().sign_pk.to_vec(), message_id as u64)
            }
            _ => return Err(CoreError::State),
        };

        if unpin {
            self.db.unpin_group_message(group_id, message_id)?;
        } else {
            self.db.pin_group_message(group_id, message_id)?;
        }

        let ev = if unpin {
            CoreEvent::MessageUnpinned {
                contact_id: None,
                group_id: Some(to_hex(group_id)),
                message_id,
            }
        } else {
            CoreEvent::MessagePinned {
                contact_id: None,
                group_id: Some(to_hex(group_id)),
                message_id,
            }
        };
        let _ = self.events.try_send(ev);

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
        let wire = WirePin {
            sender_sign_pk,
            origin_msg_id: origin,
            unpin,
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
                body: String::new(),
                attachments: vec![],
                sent_at: now_ms(),
                ttl_ms: None,
                group: Some(gref.clone()),
                buttons: None,
                callback_data: None,
                edit_of: None,
                pin: Some(wire.clone()),
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

    pub async fn create_group(
        self: &Arc<Self>,
        name: &str,
        member_contact_ids: &[i64],
    ) -> Result<Vec<u8>> {
        let mut gid = vec![0u8; 32];
        fill_random(&mut gid);
        self.db.create_group(&gid, name)?;
        self.db.add_group_member(
            &gid,
            &self.identity.card().sign_pk,
            &self.identity.card().dh_pk,
            self.node.onion_address(),
            &self.display_name().unwrap_or_default(),
            true,
        )?;
        let mut members_wire = Vec::new();
        members_wire.push(WireMember {
            sign_pk: self.identity.card().sign_pk.to_vec(),
            dh_pk: self.identity.card().dh_pk.to_vec(),
            onion: self.node.onion_address().to_string(),
            name: self.display_name().unwrap_or_default(),
        });
        for cid in member_contact_ids {
            let c = self.db.get_contact(*cid)?.ok_or(CoreError::NotFound)?;
            self.db.add_group_member(
                &gid,
                &c.identity_sign,
                &c.identity_dh,
                &c.onion_address,
                &c.display_name,
                false,
            )?;
            members_wire.push(WireMember {
                sign_pk: c.identity_sign.clone(),
                dh_pk: c.identity_dh.clone(),
                onion: c.onion_address.clone(),
                name: c.display_name.clone(),
            });
        }
        let gref = WireGroupRef {
            id: gid.clone(),
            name: name.to_string(),
            members: members_wire,
        };
        for cid in member_contact_ids {
            let sent_at = now_ms();
            let mut payload = WirePayload::simple(0, String::new(), Vec::new(), sent_at, None);
            payload.group = Some(gref.clone());
            self.send_to_contact(*cid, &mut payload).await?;
        }
        let _ = self.events.try_send(CoreEvent::GroupUpdated {
            group_id: to_hex(&gid),
        });
        Ok(gid)
    }

    pub async fn add_group_member(
        self: &Arc<Self>,
        group_id: &[u8],
        contact_id: i64,
    ) -> Result<()> {
        let group_name = self
            .db
            .get_group_name(group_id)?
            .ok_or(CoreError::NotFound)?;
        let contact = self
            .db
            .get_contact(contact_id)?
            .ok_or(CoreError::NotFound)?;
        if self.db.is_group_member(group_id, &contact.identity_sign)? {
            return Ok(());
        }
        self.db.add_group_member(
            group_id,
            &contact.identity_sign,
            &contact.identity_dh,
            &contact.onion_address,
            &contact.display_name,
            false,
        )?;
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
        let gref = WireGroupRef {
            id: group_id.to_vec(),
            name: group_name,
            members: gref_members,
        };
        for m in members {
            if m.is_self {
                continue;
            }
            let target = match self.db.find_contact_by_identity(&m.dh_pk)? {
                Some(c) => c,
                None => continue,
            };
            if target.trust == TrustLevel::Blocked {
                continue;
            }
            let mut payload = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
            payload.group = Some(gref.clone());
            let _ = self.send_to_contact(target.id, &mut payload).await;
        }
        let _ = self.events.try_send(CoreEvent::GroupUpdated {
            group_id: to_hex(group_id),
        });
        Ok(())
    }

    pub async fn send_to_group(
        self: &Arc<Self>,
        group_id: &[u8],
        body: String,
        attachments: Vec<PendingAttachment>,
        _ttl: Option<Duration>,
        reply_to: Option<i64>,
    ) -> Result<i64> {
        let sent_at = now_ms();
        let expires_at: Option<i64> = None;
        let ttl: Option<Duration> = None;
        let members = self.db.list_group_members(group_id)?;
        let mut recipients = Vec::new();
        for member in members.iter().filter(|member| !member.is_self) {
            if let Some(contact) = self.db.find_contact_by_identity(&member.dh_pk)? {
                if contact.trust != TrustLevel::Blocked {
                    recipients.push(contact.id);
                }
            }
        }
        let wire_reply = match reply_to {
            Some(rt) => self.build_wire_reply(rt)?,
            None => None,
        };
        let mut staged =
            gipny_libcore::files::StagedAttachments::new(&self.data_dir.join(ATTACHMENTS_DIR))?;
        let mut wire_atts = Vec::new();
        for attachment in attachments {
            match &attachment.from {
                Some(source) => staged.parts(
                    &attachment.name,
                    std::io::BufReader::new(std::fs::File::open(source)?),
                )?,
                None if !staged.can_inline(attachment.data.len()) => {
                    staged.parts(&attachment.name, &attachment.data[..])?
                }
                None => {
                    staged.inline(&attachment.name, &attachment.data)?;
                    wire_atts.push(WireAttachment {
                        name: attachment.name,
                        data: attachment.data,
                    });
                }
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
                expires_at,
                reply_to,
                buttons: None,
                notify_sound: None,
                recipients: &recipients,
            },
        )?;
        for recipient in recipients {
            // A previous attempt may have awaited while this contact was
            // blocked or deleted; do not send to that contact afterwards.
            let contact = match self.db.get_contact(recipient) {
                Ok(Some(contact)) if contact.trust != TrustLevel::Blocked => contact,
                Err(error) => {
                    gipny_libcore::debug_error!("[relay-client] group msg {msg_id} queued; contact {recipient} lookup failed: {error:?}");
                    continue;
                }
                _ => continue,
            };
            let mut payload = WirePayload::simple(
                msg_id as u64,
                body.clone(),
                wire_atts.clone(),
                sent_at,
                ttl.map(|d| d.as_millis() as i64),
            );
            payload.group = Some(gref.clone());
            payload.reply_to = wire_reply.clone();
            payload.files = offers.clone();
            if let Err(e) = Self::attempt_group_delivery(
                &self.db,
                msg_id,
                contact.id,
                self.send_to_contact(contact.id, &mut payload),
            )
            .await
            {
                gipny_libcore::debug_trace!("[relay-client] group send to contact {} failed (msg {}): {:?} — kept in pending_outbound", contact.id, msg_id, e);
            }
        }
        Ok(msg_id)
    }

    pub async fn send_typing_group(self: &Arc<Self>, group_id: &[u8], typing: bool) -> Result<()> {
        let members = self.db.list_group_members(group_id)?;
        let gref = WireGroupRef {
            id: group_id.to_vec(),
            name: self.db.get_group_name(group_id)?.unwrap_or_default(),
            members: members
                .iter()
                .map(|m| WireMember {
                    sign_pk: m.sign_pk.clone(),
                    dh_pk: m.dh_pk.clone(),
                    onion: m.onion.clone(),
                    name: m.display_name.clone(),
                })
                .collect(),
        };
        let self_sign = self.identity.card().sign_pk;
        for m in &members {
            if m.sign_pk == self_sign.as_slice() {
                continue;
            }
            if let Some(c) = self.db.find_contact_by_sign_pk(&m.sign_pk)? {
                if !self.sessions.lock().await.contains_key(&c.id) {
                    continue;
                }
                let mut payload = make_typing_payload(Some(gref.clone()), typing);
                let _ = self.send_to_contact(c.id, &mut payload).await;
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod group_delivery_tests {
    use super::*;

    fn pending(db: &Db, recipient: i64) -> Vec<i64> {
        db.pending_outbound_for_recipient(
            recipient,
            now_ms() + RETRY_MAX_BACKOFF_MS + 1,
            RETRY_BASE_BACKOFF_MS,
            RETRY_MAX_BACKOFF_MS,
            50,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn group_enqueue_and_retry_survive_restart_until_recipient_ack() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("core.db");
        let db = Db::open_plain(&path).unwrap();
        let first = db
            .add_contact(&[1; 32], &[2; 32], "", "first", None)
            .unwrap();
        let second = db
            .add_contact(&[3; 32], &[4; 32], "", "second", None)
            .unwrap();
        let outsider = db
            .add_contact(&[5; 32], &[6; 32], "", "outsider", None)
            .unwrap();
        let group = [9; 16];
        db.create_group(&group, "group").unwrap();
        let message = db
            .insert_group_message(
                &group,
                Some(&[7; 32]),
                Direction::Out,
                "durable",
                now_ms(),
                None,
                &[],
            )
            .unwrap();
        db.queue_group_recipients(message, &[first, second])
            .unwrap();

        // Exercise the same send boundary used by both Core send paths. The
        // relay accepts the first enqueue but never supplies an end-to-end ACK.
        let (relay, mut received) = mpsc::channel(4);
        let before_send = now_ms();
        Core::attempt_group_delivery(&db, message, first, async {
            relay
                .send((message, first))
                .await
                .map_err(|_| CoreError::State)?;
            db.mark_sent(message)?;
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(received.recv().await, Some((message, first)));
        assert!(Core::attempt_group_delivery(
            &db,
            message,
            second,
            std::future::ready(Err(CoreError::State)),
        )
        .await
        .is_err());
        assert_eq!(pending(&db, first), vec![message]);
        assert_eq!(pending(&db, second), vec![message]);
        assert!(
            db.pending_outbound_for_recipient(
                first,
                before_send,
                RETRY_BASE_BACKOFF_MS,
                RETRY_MAX_BACKOFF_MS,
                50,
            )
            .unwrap()
            .is_empty(),
            "an initial enqueue must observe the retry backoff"
        );
        assert!(!db.get_message(message).unwrap().unwrap().delivered);
        drop(db);

        let db = Db::open_plain(&path).unwrap();
        assert_eq!(pending(&db, first), vec![message]);
        assert_eq!(pending(&db, second), vec![message]);
        // Resynchronizing the encryption session must not discard delivery
        // obligations. A fresh retry can be accepted and lost in the same way.
        db.record_resync(first).unwrap();
        db.delete_session(first).unwrap();
        Core::attempt_group_delivery(&db, message, first, async {
            relay
                .send((message, first))
                .await
                .map_err(|_| CoreError::State)?;
            db.mark_sent(message)?;
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(received.recv().await, Some((message, first)));
        assert_eq!(pending(&db, first), vec![message]);
        assert_eq!(pending(&db, second), vec![message]);

        let mut ack = WirePayload::simple(0, String::new(), vec![], now_ms(), None);
        ack.ack_for = Some(message as u64);
        ack.group = Some(WireGroupRef {
            id: group.to_vec(),
            name: "group".into(),
            members: vec![],
        });
        assert!(Core::acknowledge_delivery(&db, outsider, &ack)
            .unwrap()
            .is_none());
        let mut wrong_group = ack.clone();
        wrong_group.group.as_mut().unwrap().id = vec![8; 16];
        assert!(Core::acknowledge_delivery(&db, first, &wrong_group)
            .unwrap()
            .is_none());
        let mut wrong_message = ack.clone();
        wrong_message.ack_for = Some(message as u64 + 1_000);
        assert!(Core::acknowledge_delivery(&db, first, &wrong_message)
            .unwrap()
            .is_none());
        assert_eq!(pending(&db, first), vec![message]);
        assert_eq!(pending(&db, second), vec![message]);

        assert!(Core::acknowledge_delivery(&db, first, &ack)
            .unwrap()
            .is_none());
        assert!(pending(&db, first).is_empty());
        assert_eq!(pending(&db, second), vec![message]);
        assert!(!db.get_message(message).unwrap().unwrap().delivered);
        assert!(
            Core::acknowledge_delivery(&db, first, &ack)
                .unwrap()
                .is_none(),
            "duplicate ACK must not acknowledge another recipient"
        );
        let completed = Core::acknowledge_delivery(&db, second, &ack)
            .unwrap()
            .unwrap();
        assert_eq!(completed.id, message);
        assert!(pending(&db, second).is_empty());
        assert!(db.get_message(message).unwrap().unwrap().delivered);
        assert!(
            Core::acknowledge_delivery(&db, second, &ack)
                .unwrap()
                .is_none(),
            "completion must not emit twice"
        );
        drop(db);

        let db = Db::open_plain(&path).unwrap();
        assert!(pending(&db, first).is_empty());
        assert!(pending(&db, second).is_empty());
        assert!(db.get_message(message).unwrap().unwrap().delivered);
    }

    #[test]
    fn delivery_ack_keeps_direct_message_contact_and_direction_checks() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_plain(&dir.path().join("core.db")).unwrap();
        let recipient = db
            .add_contact(&[1; 32], &[2; 32], "", "recipient", None)
            .unwrap();
        let outsider = db
            .add_contact(&[3; 32], &[4; 32], "", "outsider", None)
            .unwrap();
        let outgoing = db
            .insert_message(recipient, Direction::Out, "out", now_ms(), None, &[])
            .unwrap();
        let incoming = db
            .insert_message(recipient, Direction::In, "in", now_ms(), None, &[])
            .unwrap();
        let mut ack = WirePayload::simple(0, String::new(), vec![], now_ms(), None);
        ack.ack_for = Some(outgoing as u64);
        assert!(Core::acknowledge_delivery(&db, outsider, &ack)
            .unwrap()
            .is_none());
        assert!(!db.get_message(outgoing).unwrap().unwrap().delivered);
        assert_eq!(
            Core::acknowledge_delivery(&db, recipient, &ack)
                .unwrap()
                .unwrap()
                .id,
            outgoing
        );
        assert!(db.get_message(outgoing).unwrap().unwrap().delivered);
        ack.ack_for = Some(incoming as u64);
        assert!(Core::acknowledge_delivery(&db, recipient, &ack)
            .unwrap()
            .is_none());
        assert!(!db.get_message(incoming).unwrap().unwrap().delivered);
    }
}
