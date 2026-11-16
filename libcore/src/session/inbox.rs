//! Envelope decryption and durable incoming delivery.

use super::*;

impl SessionManager {
    pub(super) async fn handle_incoming_envelope(
        self: &Arc<Self>,
        from_pk: &[u8; 32],
        blob: &[u8],
    ) -> Result<()> {
        let envelope: EnvelopeBlob = bincode::deserialize(blob)?;
        let sealed = from_pk == &[0u8; 32];
        match envelope {
            EnvelopeBlob::X3dhInit(init) => {
                let sender_sign = init.identity.sign_pk;
                let sender_dh = init.identity.dh_pk;
                let known = self.db.find_contact_by_sign_pk(&sender_sign)?;
                if let Some(c) = &known {
                    if c.trust == TrustLevel::Blocked {
                        return Ok(());
                    }
                    if sender_dh != c.identity_dh.as_slice() {
                        return Err(SessionError::State);
                    }
                }
                // Authenticate before touching anything: an init that does not
                // decrypt (forged, replayed, stale) must leave the contact list,
                // the one-time prekeys and the working session as they were.
                let ad = build_ad(&self.identity.card().dh_pk, &sender_dh);
                let (state, plaintext, opk_id) = self.accept_x3dh(&init, &ad).await?;
                let plaintext = zeroize::Zeroizing::new(plaintext);
                let payload: WirePayload = decode_with_padding_fallback(&plaintext)?;
                let contact = match known {
                    Some(c) => c,
                    None => {
                        let name = hex_short(&sender_sign);
                        let id = self
                            .db
                            .add_contact(&sender_sign, &sender_dh, "", &name, None)?;
                        let _ = self
                            .events
                            .send(SessionEvent::ContactAdded { contact_id: id })
                            .await;
                        self.db.get_contact(id)?.ok_or(SessionError::NotFound)?
                    }
                };
                let crossed = self.own_inits.lock().await.get(&contact.id).is_some()
                    && self.sessions.lock().await.contains_key(&contact.id);
                // A second init from them while their first lost to ours: they
                // never took ours (it did not reach them, or they started over
                // since — a reinstall, a resync), so this one is a new start.
                let crossed = crossed && !self.lost_inits.lock().await.contains_key(&contact.id);
                if crossed && ours_stands(&self.identity.card().sign_pk, &contact.identity_sign) {
                    // Both opened a session at once. Ours stands; theirs is
                    // read for what it says (name, relay) and set aside, and
                    // they will take ours when it reaches them.
                    crate::debug_trace!(
                        "[session] X3dhInit from contact {} crossed ours; ours stands",
                        contact.id
                    );
                    let letter = self
                        .db
                        .commit_received(contact.id, None, opk_id, &plaintext)?;
                    self.lost_inits.lock().await.insert(contact.id, state);
                    self.deliver_letter(letter, contact.id, payload).await;
                    if init.one_time_id.is_some() {
                        self.republish_bundle().await;
                    }
                    return Ok(());
                }
                if crossed {
                    // Theirs stands. What we sent on ours they read on it (they
                    // keep it for that); anything that did not make it goes
                    // again on the usual retry, so no resend of everything here.
                    crate::debug_trace!(
                        "[session] X3dhInit from contact {} crossed ours; theirs stands",
                        contact.id
                    );
                } else {
                    crate::debug_trace!(
                        "[session] received X3dhInit from contact {}, accepting",
                        contact.id
                    );
                }
                self.own_inits.lock().await.remove(&contact.id);
                self.lost_inits.lock().await.remove(&contact.id);
                let sb = zeroize::Zeroizing::new(state.to_bytes()?);
                let letter = {
                    // Replace the old session in memory and on disk together,
                    // under the lock, as the ratchet path does; the prekey is
                    // spent and the letter kept in that same commit.
                    let mut sessions = self.sessions.lock().await;
                    let letter =
                        self.db
                            .commit_received(contact.id, Some(&sb), opk_id, &plaintext)?;
                    sessions.insert(contact.id, state);
                    letter
                };
                drop(sb);
                self.session_created_at
                    .lock()
                    .await
                    .insert(contact.id, now_ms());
                self.deliver_letter(letter, contact.id, payload).await;
                crate::debug_trace!(
                    "[session] session established with contact {} via X3dhInit",
                    contact.id
                );
                if init.one_time_id.is_some() {
                    self.republish_bundle().await;
                }
                self.send_kick.notify_one();
            }
            EnvelopeBlob::Ratchet { header, ciphertext } => {
                let mut decrypted: Option<(i64, zeroize::Zeroizing<Vec<u8>>, i64)> = None;
                let candidates: Vec<Contact> = if sealed {
                    self.db
                        .list_contacts()?
                        .into_iter()
                        .filter(|c| c.trust != TrustLevel::Blocked)
                        .collect()
                } else {
                    match self.db.find_contact_by_sign_pk(from_pk)? {
                        Some(c) if c.trust != TrustLevel::Blocked => vec![c],
                        _ => vec![],
                    }
                };
                for c in candidates {
                    let ad = build_ad(&self.identity.card().dh_pk, &c.identity_dh);
                    let mut sess = self.sessions.lock().await;
                    let attempt = match sess.get_mut(&c.id) {
                        Some(state) => state
                            .decrypt(&header, &ciphertext, &ad)
                            .map(|pt| (pt, state.to_bytes())),
                        None => match self.db.get_session(c.id)?.map(zeroize::Zeroizing::new) {
                            Some(blob) => {
                                let mut state = RatchetState::from_bytes(&blob)?;
                                let r = state
                                    .decrypt(&header, &ciphertext, &ad)
                                    .map(|pt| (pt, state.to_bytes()));
                                if r.is_ok() {
                                    sess.insert(c.id, state);
                                }
                                r
                            }
                            None => continue,
                        },
                    };
                    // Saved before the lock goes: two letters of one contact
                    // read at once (several connections) must not write their
                    // states back in the other order. The letter is saved in
                    // the same commit: its key is now spent.
                    let attempt = match attempt {
                        Ok((pt, sb_res)) => {
                            let pt = zeroize::Zeroizing::new(pt);
                            let committed = sb_res
                                .map_err(SessionError::from)
                                .map(zeroize::Zeroizing::new)
                                .and_then(|sb| {
                                    Ok(self.db.commit_received(c.id, Some(&sb), None, &pt)?)
                                });
                            match committed {
                                Ok(letter) => Ok((pt, letter)),
                                Err(e) => {
                                    // Not saved: forget the advanced state, so
                                    // the relay's copy still opens next time.
                                    sess.remove(&c.id);
                                    return Err(e);
                                }
                            }
                        }
                        Err(e) => Err(e),
                    };
                    drop(sess);
                    if let Ok((pt, letter)) = attempt {
                        decrypted = Some((c.id, pt, letter));
                        break;
                    }
                }
                let (cid, pt, letter) = match decrypted {
                    Some(x) => x,
                    None => {
                        // On a session of theirs that lost to ours: sent before
                        // ours reached them. Read it on that session.
                        let lost_hit = {
                            let mut lost = self.lost_inits.lock().await;
                            let mut hit = None;
                            for (cid, state) in lost.iter_mut() {
                                let Ok(Some(c)) = self.db.get_contact(*cid) else {
                                    continue;
                                };
                                if !sealed && c.identity_sign.as_slice() != from_pk.as_slice() {
                                    continue;
                                }
                                let ad = build_ad(&self.identity.card().dh_pk, &c.identity_dh);
                                if let Ok(pt) = state.decrypt(&header, &ciphertext, &ad) {
                                    hit = Some((*cid, pt));
                                    break;
                                }
                            }
                            hit
                        };
                        if let Some((cid, pt)) = lost_hit {
                            crate::debug_trace!("[session] letter from contact {cid} on its session that lost to ours; read on it");
                            let pt = zeroize::Zeroizing::new(pt);
                            let letter = self.db.commit_received(cid, None, None, &pt)?;
                            self.deliver_stored(letter, cid, &pt).await;
                            return Ok(());
                        }
                        if sealed {
                            crate::debug_trace!(
                                "[session] sealed ratchet: no session matched, ACK and drop"
                            );
                            return Err(SessionError::SealedDrop);
                        }
                        if let Ok(Some(c)) = self.db.find_contact_by_sign_pk(from_pk) {
                            crate::debug_trace!(
                                "[session] no session for {}, requesting resync",
                                c.id
                            );
                            let _ = self.request_resync(&c).await;
                        }
                        return Err(SessionError::Crypto(CryptoError::Mac));
                    }
                };
                // They answered on this session: it is settled, and an init
                // from them from now on is a new start, not a crossing.
                self.own_inits.lock().await.remove(&cid);
                let Ok(payload) = decode_with_padding_fallback(&pt) else {
                    // Read, but not a payload this version understands. The
                    // key is spent, so the relay copy is of no further use.
                    crate::debug_trace!(
                        "[session] letter from contact {cid} did not decode; dropped"
                    );
                    let _ = self.db.inbox_done(letter);
                    return Ok(());
                };
                let is_keepalive = payload.ack_for == Some(0)
                    && payload.origin_msg_id == 0
                    && payload.body.is_empty()
                    && payload.attachments.is_empty();
                if !is_keepalive {
                    *self
                        .incoming_since_send
                        .lock()
                        .await
                        .entry(cid)
                        .or_insert(0) += 1;
                }
                self.deliver_letter(letter, cid, payload).await;
                if !is_keepalive {
                    let counter = self
                        .incoming_since_send
                        .lock()
                        .await
                        .get(&cid)
                        .copied()
                        .unwrap_or(0);
                    if counter >= KEEPALIVE_INCOMING_THRESHOLD {
                        self.send_kick.notify_one();
                    }
                }
            }
        }
        Ok(())
    }

    /// Process a letter already committed to the inbox. Whatever happens here
    /// the relay copy is acknowledged: the letter is safe on disk. A storage
    /// failure leaves the row for `drain_inbox`; any other failure is the
    /// letter's own and drops it. Returns whether the letter was kept.
    pub(super) async fn deliver_letter(
        self: &Arc<Self>,
        letter: i64,
        contact_id: i64,
        payload: WirePayload,
    ) -> bool {
        self.inbox_busy
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(letter);
        let outcome = self.persist_incoming(contact_id, payload).await;
        self.inbox_busy
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&letter);
        match outcome {
            Err(e) if is_transient(&e) => {
                crate::debug_error!(
                    "[inbox] letter {letter} from contact {contact_id} kept for retry: {e:?}"
                );
                true
            }
            outcome => {
                if let Err(e) = outcome {
                    crate::debug_trace!(
                        "[inbox] letter {letter} from contact {contact_id} rejected: {e:?}"
                    );
                }
                let _ = self.db.inbox_done(letter);
                false
            }
        }
    }

    pub(super) async fn deliver_stored(
        self: &Arc<Self>,
        letter: i64,
        contact_id: i64,
        plaintext: &[u8],
    ) -> bool {
        match decode_with_padding_fallback(plaintext) {
            Ok(payload) => self.deliver_letter(letter, contact_id, payload).await,
            Err(_) => {
                let _ = self.db.inbox_done(letter);
                false
            }
        }
    }

    /// Finish letters a crash, a storage or a network failure left in the
    /// inbox, oldest first. One that fails again waits for the next round
    /// without holding up the others.
    pub(super) async fn drain_inbox(self: &Arc<Self>) {
        let Ok(pending) = self.db.inbox_pending(64) else {
            return;
        };
        for row in pending {
            if self
                .inbox_busy
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .contains(&row.id)
            {
                continue;
            }
            self.deliver_stored(row.id, row.contact_id, &row.payload)
                .await;
        }
    }

    /// Reads an init without changing anything: the one-time prekey it used
    /// is returned for the caller to spend once the payload is accepted.
    pub(super) async fn accept_x3dh(
        &self,
        init: &X3dhInitial,
        ad: &[u8],
    ) -> Result<(RatchetState, Vec<u8>, Option<i64>)> {
        let signed_id = i64::from_be_bytes(
            self.db
                .get_setting(SETTING_SIGNED_PREKEY_ID)?
                .ok_or(SessionError::State)?
                .try_into()
                .map_err(|_| SessionError::State)?,
        );
        let signed = self.db.get_prekey(signed_id)?.ok_or(SessionError::State)?;
        let signed_pair = PreKeyPair::from_secret(to_arr32(signed.private.clone())?);
        let opk = match init.one_time_id {
            Some(opk_id) => {
                let p = self.db.get_prekey(opk_id)?.ok_or(SessionError::StaleOpk)?;
                if p.kind != PreKeyKind::OneTime {
                    return Err(SessionError::StaleOpk);
                }
                Some((p.id, PreKeyPair::from_secret(to_arr32(p.private.clone())?)))
            }
            None => None,
        };
        let (state, pt) = crypto::x3dh_respond(
            &self.identity,
            &signed_pair,
            opk.as_ref().map(|(_, pair)| pair),
            init,
            ad,
        )?;
        Ok((state, pt, opk.map(|(id, _)| id)))
    }

    pub(super) async fn persist_incoming(
        self: &Arc<Self>,
        contact_id: i64,
        payload: WirePayload,
    ) -> Result<()> {
        if self.handle_dm_control(contact_id, &payload).await? {
            return Ok(());
        }
        if self.reject_deleted_dm_content(contact_id, &payload).await? {
            return Ok(());
        }
        // As in the app: a contact who deleted us asks for the chat to go.
        if payload.wipe == Some(true) {
            crate::debug_trace!(
                "[wipe] contact {contact_id} deleted us and asked for the chat to go; deleting it"
            );
            self.db.delete_contact(contact_id)?;
            self.sessions.lock().await.remove(&contact_id);
            self.session_created_at.lock().await.remove(&contact_id);
            self.own_inits.lock().await.remove(&contact_id);
            self.lost_inits.lock().await.remove(&contact_id);
            self.incoming_since_send.lock().await.remove(&contact_id);
            let _ = self
                .events
                .send(SessionEvent::ContactWiped { contact_id })
                .await;
            return Ok(());
        }
        if self
            .db
            .get_contact(contact_id)?
            .is_some_and(|c| c.request_state == crate::db::RequestState::Incoming)
        {
            if crate::files::is_request_content(&payload) {
                crate::files::validate_message_files(&payload)?;
                let mut staged =
                    crate::files::StagedAttachments::new(&self.data_dir.join(ATTACHMENTS_DIR))?;
                for attachment in &payload.attachments {
                    staged.inline(&attachment.name, &attachment.data)?;
                }
                let committed = staged.publish_incoming(&self.db, contact_id, &payload)?;
                if !committed.pending && !committed.deleted {
                    self.activate_message_files(contact_id, committed.id)
                        .await?;
                    if committed.inserted {
                        self.db.if_message_live(committed.id, || {
                            let _ = self.events.try_send(SessionEvent::IncomingPayload {
                                contact_id,
                                payload: payload.clone(),
                                message_id: committed.id,
                            });
                        })?;
                    }
                    if let Some(contact) = self.db.get_contact(contact_id)? {
                        let _ = self.send_ack(&contact, &payload).await;
                    }
                }
            }
            return Ok(());
        }
        self.heard
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(contact_id, now_ms());
        // Files in parts: these letters are nothing else.
        if let Some(ack) = &payload.file_ack {
            self.on_file_ack(contact_id, ack).await?;
        }
        if let Some(chunk) = &payload.file_chunk {
            self.on_file_chunk(
                contact_id,
                chunk,
                payload.sent_at,
                payload.dm.as_ref().map(|dm| &dm.reference),
            )
            .await?;
        }
        if let Some(file_id) = &payload.file_cancel {
            self.on_file_cancel(contact_id, file_id).await?;
        }
        if let Some(name) = payload.sender_name.as_deref() {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                self.apply_peer_name(contact_id, trimmed).await;
            }
        }

        if let Some(relay) = payload.relay_address.as_deref() {
            let trimmed = relay.trim();
            if !trimmed.is_empty() && crate::card::is_valid_i2p_address(trimmed) {
                if let Ok(current) = self.db.contact_relay(contact_id) {
                    if !current
                        .as_deref()
                        .is_some_and(|known| relay::same_destination(known, trimmed))
                    {
                        crate::debug_trace!(
                            "[relay-discovery] updated relay for contact {} to {}",
                            contact_id,
                            &trimmed[..trimmed.len().min(16)]
                        );
                        let _ = self.db.set_contact_relay(contact_id, Some(trimmed));
                        self.warm_relay_of(contact_id);
                    }
                }
            }
        }
        let is_empty = payload.body.is_empty()
            && payload.attachments.is_empty()
            && payload.files.is_empty()
            && payload.group.is_none()
            && payload.callback_data.is_none()
            && payload.edit_of.is_none()
            && payload.pin.is_none()
            && payload.ack_for.is_none()
            && payload.console.is_none();
        if is_empty {
            return Ok(());
        }

        if payload.ack_for.is_some() {
            if let Some(local_id) = acknowledge_delivery(&self.db, contact_id, &payload)? {
                let _ = self
                    .events
                    .send(SessionEvent::MessageDelivered {
                        message_id: local_id,
                    })
                    .await;
            }
            return Ok(());
        }

        if let Some(gref) = &payload.group {
            let sender = self
                .db
                .get_contact(contact_id)?
                .ok_or(SessionError::State)?;
            let card = self.identity.card();
            self.db
                .apply_group_ref(gref, &sender.identity_sign, &card.sign_pk, &card.dh_pk)?;
        }

        if payload.callback_data.is_some()
            && payload.edit_of.is_none()
            && payload.pin.is_none()
            && payload.body.is_empty()
            && payload.attachments.is_empty()
        {
            let _ = self
                .events
                .send(SessionEvent::IncomingPayload {
                    contact_id,
                    payload,
                    message_id: 0,
                })
                .await;
            return Ok(());
        }

        if let Some(edit_target_origin) = payload.edit_of {
            let lookup = if let Some(gref) = &payload.group {
                let contact = self
                    .db
                    .get_contact(contact_id)?
                    .ok_or(SessionError::NotFound)?;
                let self_sign = self.identity.card().sign_pk.to_vec();
                self.db.resolve_group_message(
                    &gref.id,
                    &contact.identity_sign,
                    edit_target_origin as i64,
                    &self_sign,
                )?
            } else {
                self.db
                    .find_message_by_origin(contact_id, edit_target_origin as i64)?
            };
            if let Some(local_id) = lookup {
                if self
                    .db
                    .get_message(local_id)?
                    .is_some_and(|message| message.deleted_at.is_some())
                {
                    return Ok(());
                }
                let buttons_bytes = payload
                    .buttons
                    .as_ref()
                    .and_then(|b| bincode::serialize(b).ok());
                self.db.update_message_body(local_id, &payload.body)?;
                if let Some(b) = buttons_bytes {
                    self.db.set_setting(&format!("buttons_{}", local_id), &b)?;
                } else {
                    let _ = self.db.delete_setting(&format!("buttons_{}", local_id));
                }
                let permit = self
                    .events
                    .reserve()
                    .await
                    .map_err(|_| SessionError::State)?;
                self.db.if_message_live(local_id, || {
                    permit.send(SessionEvent::MessageEdited {
                        message_id: local_id,
                        new_body: payload.body.clone(),
                        buttons: payload.buttons.clone(),
                    });
                })?;
                return Ok(());
            }
        }

        // An edit whose original has not arrived is never a new DM.
        if payload.edit_of.is_some() && payload.group.is_none() {
            return Ok(());
        }

        if let Some(pin) = &payload.pin {
            let self_sign = self.identity.card().sign_pk.to_vec();
            let (local, target_cid, target_gid) = if let Some(gref) = &payload.group {
                (
                    self.db.resolve_group_message(
                        &gref.id,
                        &pin.sender_sign_pk,
                        pin.origin_msg_id as i64,
                        &self_sign,
                    )?,
                    None,
                    Some(gref.id.clone()),
                )
            } else {
                (
                    self.db
                        .resolve_contact_message(contact_id, pin.origin_msg_id as i64)?,
                    Some(contact_id),
                    None,
                )
            };
            if let Some(local) = local {
                let _ = match (&target_gid, pin.unpin) {
                    (Some(gid), true) => {
                        self.db.unpin_group_message(gid, local)?;
                    }
                    (Some(gid), false) => {
                        self.db.pin_group_message(gid, local)?;
                    }
                    (None, true) => {
                        self.db.unpin_contact_message(contact_id, local)?;
                    }
                    (None, false) => {
                        self.db.pin_contact_message(contact_id, local)?;
                    }
                };
                let ev = if pin.unpin {
                    SessionEvent::MessageUnpinned {
                        contact_id: target_cid,
                        group_id: target_gid,
                        message_id: local,
                    }
                } else {
                    SessionEvent::MessagePinned {
                        contact_id: target_cid,
                        group_id: target_gid,
                        message_id: local,
                    }
                };
                let _ = self.events.send(ev).await;
            }
            return Ok(());
        }

        let contact = self
            .db
            .get_contact(contact_id)?
            .ok_or(SessionError::NotFound)?;

        if payload.origin_msg_id > 0 {
            let existing = if let Some(gref) = &payload.group {
                let self_sign = self.identity.card().sign_pk.to_vec();
                self.db.resolve_group_message(
                    &gref.id,
                    &contact.identity_sign,
                    payload.origin_msg_id as i64,
                    &self_sign,
                )?
            } else {
                self.db
                    .find_message_by_origin(contact_id, payload.origin_msg_id as i64)?
            };
            if let Some(mid) = existing {
                crate::debug_trace!(
                    "[session] duplicate origin={} from contact {}, re-acking",
                    payload.origin_msg_id,
                    contact_id
                );
                self.activate_message_files(contact_id, mid).await?;
                if self.db.can_receive_files(contact_id)? {
                    let _ = self.send_ack(&contact, &payload).await;
                }
                return Ok(());
            }
        }

        crate::files::validate_message_files(&payload)?;
        let mut staged =
            crate::files::StagedAttachments::new(&self.data_dir.join(ATTACHMENTS_DIR))?;
        for attachment in &payload.attachments {
            staged.inline(&attachment.name, &attachment.data)?;
        }
        let committed = staged.publish_incoming(&self.db, contact_id, &payload)?;
        if committed.deleted || committed.pending {
            return Ok(());
        }
        let mid = committed.id;
        self.activate_message_files(contact_id, mid).await?;
        if !committed.inserted {
            let _ = self.send_ack(&contact, &payload).await;
            return Ok(());
        }
        if let Some(c) = &payload.console {
            // Console framing is a DM affair; in a group it is stored for
            // display and nothing more.
            if payload.group.is_none() {
                match c.kind {
                    CONSOLE_GRANT | CONSOLE_REVOKE => {
                        let granted = c.kind == CONSOLE_GRANT;
                        if self
                            .db
                            .set_contact_agent_granted(contact_id, granted)
                            .unwrap_or(false)
                        {
                            let _ = self
                                .events
                                .send(SessionEvent::ContactUpdated { contact_id })
                                .await;
                        }
                    }
                    _ => {}
                }
            }
        }
        self.db.touch_contact(contact_id)?;
        let payload_for_ack = payload.clone();
        let permit = self
            .events
            .reserve()
            .await
            .map_err(|_| SessionError::State)?;
        self.db.if_message_live(mid, || {
            permit.send(SessionEvent::IncomingPayload {
                contact_id,
                payload,
                message_id: mid,
            });
        })?;
        let _ = self.send_ack(&contact, &payload_for_ack).await;
        Ok(())
    }

    pub(super) async fn request_resync(&self, contact: &Contact) -> Result<()> {
        let throttled = self.db.resync_recent(contact.id, 60_000)?;
        if !throttled {
            crate::debug_trace!("[session] forcing resync for contact {}", contact.id);
            self.db.record_resync(contact.id)?;
            self.sessions.lock().await.remove(&contact.id);
            self.session_created_at.lock().await.remove(&contact.id);
            let _ = self.db.delete_session(contact.id);
        }
        self.send_kick.notify_one();
        Ok(())
    }

    pub(super) async fn send_ack(
        self: &Arc<Self>,
        contact: &Contact,
        original: &WirePayload,
    ) -> Result<()> {
        if original.origin_msg_id == 0 {
            return Ok(());
        }
        let mut payload = WirePayload {
            origin_msg_id: 0,
            body: String::new(),
            attachments: vec![],
            sent_at: now_ms(),
            ttl_ms: None,
            group: original.group.clone(),
            buttons: None,
            callback_data: None,
            edit_of: None,
            pin: None,
            ack_for: Some(original.origin_msg_id),
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
        let Some(route) = self.route_for(contact).await else {
            return Ok(());
        };
        if self.ensure_session_for(contact, &route).await.is_err() {
            return Ok(());
        }
        let _ = self
            .send_payload_via_relay(contact, &mut payload, &route)
            .await;
        Ok(())
    }
}

/// The storage or the network failed, not the letter: worth trying again
/// later. Processing a stored letter again is idempotent (duplicates are
/// recognised by origin), so a reply that could not be sent is no reason to
/// drop what was received.
pub(super) fn is_transient(error: &SessionError) -> bool {
    match error {
        SessionError::Net(_) | SessionError::Relay(_) => true,
        SessionError::Db(e) => crate::db::is_transient(e),
        SessionError::Io(_) => true,
        _ => false,
    }
}
