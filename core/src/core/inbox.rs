use super::*;

impl Core {
    pub(super) async fn handle_incoming_envelope(self: &Arc<Self>, from_pk: &[u8; 32], blob: &[u8]) -> Result<()> {
        let envelope: EnvelopeBlob = bincode::deserialize(blob)?;
        gipny_libcore::debug_trace!("[recv] {} envelope, {} bytes",
            match &envelope { EnvelopeBlob::X3dhInit(_) => "x3dh", EnvelopeBlob::Ratchet { .. } => "ratchet" },
            blob.len());
        let sealed = from_pk == &[0u8; 32];
        match envelope {
            EnvelopeBlob::X3dhInit(init) => {
                let sender_sign = init.identity.sign_pk;
                let sender_dh = init.identity.dh_pk;
                let known = self.db.find_contact_by_sign_pk(&sender_sign)?;
                if let Some(c) = &known {
                    if c.trust == TrustLevel::Blocked { return Ok(()); }
                    if sender_dh != c.identity_dh.as_slice() { return Err(CoreError::State); }
                }
                // Authenticate before touching anything: an init that does not
                // decrypt (forged, replayed, stale) must leave the contact list
                // and its open requests, the one-time prekeys and the working
                // session exactly as they were.
                let ad = build_ad(&self.identity.card().dh_pk, &sender_dh);
                let (state, plaintext, opk_id) = self.accept_x3dh(&init, &ad).await?;
                let plaintext = zeroize::Zeroizing::new(plaintext);
                let payload: WirePayload = decode_with_padding_fallback(&plaintext)?;
                let contact = match known {
                    Some(c) => c,
                    None => {
                        let name = hex_short(&sender_sign);
                        let id = self.db.add_contact(&sender_sign, &sender_dh, "", &name, None)?;
                        self.db.set_contact_request_state(id, RequestState::Incoming)?;
                        let requests = self.db.list_incoming_requests()?;
                        for &old in requests.iter().take(requests.len().saturating_sub(MAX_INCOMING_REQUESTS)) {
                            gipny_libcore::debug_trace!("[relay-client] too many open requests, dropping contact {old}");
                            let _ = self.delete_contact(old).await;
                        }
                        let _ = self.events.try_send(CoreEvent::ContactRequest { contact_id: id });
                        self.db.get_contact(id)?.ok_or(CoreError::NotFound)?
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
                    // read for what it says and set aside, and they take ours
                    // when it reaches them.
                    gipny_libcore::debug_trace!("[relay-client] X3dhInit from contact {} crossed ours; ours stands", contact.id);
                    let letter = self.db.commit_received(contact.id, None, opk_id, &plaintext)?;
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
                    gipny_libcore::debug_trace!("[relay-client] X3dhInit from contact {} crossed ours; theirs stands", contact.id);
                }
                self.own_inits.lock().await.remove(&contact.id);
                self.lost_inits.lock().await.remove(&contact.id);
                let state_bytes = zeroize::Zeroizing::new(state.to_bytes()?);
                let letter = {
                    // The new session replaces the old one in memory and on
                    // disk together, under the lock, as the ratchet path does;
                    // the prekey is spent and the letter kept in that commit.
                    let mut sessions = self.sessions.lock().await;
                    let letter = self.db.commit_received(contact.id, Some(&state_bytes), opk_id, &plaintext)?;
                    sessions.insert(contact.id, state);
                    letter
                };
                drop(state_bytes);
                self.session_created_at.lock().await.insert(contact.id, now_ms());
                self.deliver_letter(letter, contact.id, payload).await;
                gipny_libcore::debug_trace!("[relay-client] session established with contact {} via X3dhInit", contact.id);
                if init.one_time_id.is_some() {
                    self.republish_bundle().await;
                }
                self.send_kick.notify_one();
            }
            EnvelopeBlob::Ratchet { header, ciphertext } => {
                let mut decrypted: Option<(i64, zeroize::Zeroizing<Vec<u8>>, i64)> = None;
                let candidates: Vec<gipny_libcore::db::Contact> = if sealed {
                    self.db.list_contacts()?.into_iter().filter(|c| c.trust != TrustLevel::Blocked).collect()
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
                        Some(state) => state.decrypt(&header, &ciphertext, &ad).map(|pt| (pt, state.to_bytes())),
                        None => match self.db.get_session(c.id)?.map(zeroize::Zeroizing::new) {
                            Some(blob) => {
                                let mut state = RatchetState::from_bytes(&blob)?;
                                let r = state.decrypt(&header, &ciphertext, &ad).map(|pt| (pt, state.to_bytes()));
                                if r.is_ok() { sess.insert(c.id, state); }
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
                            let committed = sb_res.map_err(CoreError::from).map(zeroize::Zeroizing::new)
                                .and_then(|sb| Ok(self.db.commit_received(c.id, Some(&sb), None, &pt)?));
                            match committed {
                                Ok(letter) => Ok((pt, letter)),
                                Err(e) => {
                                    // Not saved: forget the advanced state, so
                                    // the relay's copy still opens next time
                                    // against the state on disk.
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
                                let Ok(Some(c)) = self.db.get_contact(*cid) else { continue };
                                if !sealed && c.identity_sign.as_slice() != from_pk.as_slice() { continue; }
                                let ad = build_ad(&self.identity.card().dh_pk, &c.identity_dh);
                                if let Ok(pt) = state.decrypt(&header, &ciphertext, &ad) {
                                    hit = Some((*cid, pt));
                                    break;
                                }
                            }
                            hit
                        };
                        if let Some((cid, pt)) = lost_hit {
                            gipny_libcore::debug_trace!("[relay-client] letter from contact {cid} on its session that lost to ours; read on it");
                            let pt = zeroize::Zeroizing::new(pt);
                            let letter = self.db.commit_received(cid, None, None, &pt)?;
                            self.deliver_stored(letter, cid, &pt).await;
                            return Ok(());
                        }
                        if sealed {
                            gipny_libcore::debug_trace!("[relay-client] sealed ratchet: no session matched, ACK and drop");
                            return Err(CoreError::SealedDrop);
                        }
                        if let Ok(Some(c)) = self.db.find_contact_by_sign_pk(from_pk) {
                            gipny_libcore::debug_trace!("[relay-client] no session for contact {}, requesting resync", c.id);
                            let _ = self.request_resync(&c).await;
                        }
                        return Err(CoreError::Crypto(gipny_libcore::crypto::CryptoError::Mac));
                    }
                };
                // They answered on this session: settled, and an init from
                // them from now on is a new start, not a crossing.
                self.own_inits.lock().await.remove(&cid);
                let Ok(payload) = decode_with_padding_fallback(&pt) else {
                    // Read, but not a payload this version understands. The
                    // key is spent, so the relay copy is of no further use.
                    gipny_libcore::debug_trace!("[relay-client] letter from contact {cid} did not decode; dropped");
                    let _ = self.db.inbox_done(letter);
                    return Ok(());
                };
                let is_keepalive = payload.ack_for == Some(0)
                    && payload.origin_msg_id == 0
                    && payload.body.is_empty()
                    && payload.attachments.is_empty();
                if !is_keepalive {
                    *self.incoming_since_send.lock().await.entry(cid).or_insert(0) += 1;
                }
                self.deliver_letter(letter, cid, payload).await;
                if !is_keepalive {
                    let counter = self.incoming_since_send.lock().await.get(&cid).copied().unwrap_or(0);
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
    /// letter's own and drops it, as the relay path always did. Returns
    /// whether the letter was kept.
    async fn deliver_letter(self: &Arc<Self>, letter: i64, contact_id: i64, payload: WirePayload) -> bool {
        self.inbox_busy.lock().unwrap_or_else(|p| p.into_inner()).insert(letter);
        let outcome = self.persist_incoming(contact_id, payload).await;
        self.inbox_busy.lock().unwrap_or_else(|p| p.into_inner()).remove(&letter);
        match outcome {
            Err(e) if is_transient(&e) => {
                gipny_libcore::debug_error!("[inbox] letter {letter} from contact {contact_id} kept for retry: {e:?}");
                true
            }
            outcome => {
                if let Err(e) = outcome {
                    gipny_libcore::debug_trace!("[inbox] letter {letter} from contact {contact_id} rejected: {e:?}");
                }
                let _ = self.db.inbox_done(letter);
                false
            }
        }
    }

    async fn deliver_stored(self: &Arc<Self>, letter: i64, contact_id: i64, plaintext: &[u8]) -> bool {
        match decode_with_padding_fallback(plaintext) {
            Ok(payload) => self.deliver_letter(letter, contact_id, payload).await,
            Err(_) => { let _ = self.db.inbox_done(letter); false }
        }
    }

    /// Finish letters a crash, a storage or a network failure left in the
    /// inbox, oldest first. One that fails again waits for the next round
    /// without holding up the others.
    pub(super) async fn drain_inbox(self: &Arc<Self>) {
        let Ok(pending) = self.db.inbox_pending(64) else { return };
        for row in pending {
            if self.inbox_busy.lock().unwrap_or_else(|p| p.into_inner()).contains(&row.id) { continue; }
            self.deliver_stored(row.id, row.contact_id, &row.payload).await;
        }
    }

    async fn accept_x3dh(
        &self,
        init: &gipny_libcore::crypto::X3dhInitial,
        ad: &[u8],
    ) -> Result<(RatchetState, Vec<u8>, Option<i64>)> {
        // Reads the init without changing anything: the one-time prekey it
        // used is returned for the caller to spend once the payload is taken.
        let signed_id = i64::from_be_bytes(
            self.db.get_setting(SETTING_SIGNED_PREKEY_ID)?.ok_or(CoreError::State)?
                .try_into().map_err(|_| CoreError::State)?);
        let signed = self.db.get_prekey(signed_id)?.ok_or(CoreError::State)?;
        let signed_pair = PreKeyPair::from_secret(to_arr32(signed.private.clone())?);
        let opk = match init.one_time_id {
            Some(opk_id) => {
                let p = self.db.get_prekey(opk_id)?.ok_or(CoreError::StaleOpk)?;
                if p.kind != PreKeyKind::OneTime {
                    return Err(CoreError::StaleOpk);
                }
                Some((p.id, PreKeyPair::from_secret(to_arr32(p.private.clone())?)))
            }
            None => None,
        };
        let (state, pt) = crypto::x3dh_respond(&self.identity, &signed_pair, opk.as_ref().map(|(_, pair)| pair), init, ad)?;
        Ok((state, pt, opk.map(|(id, _)| id)))
    }

    /// The sender's relay and name, which every payload may carry.
    /// Anything from a contact proves they can reach us, so the address
    /// announcement for them has arrived and stops repeating.
    /// Time one message's journey, from the moment it actually left to the
    /// moment its acknowledgement arrived.
    ///
    /// `last_attempt_at` is written by `mark_sent` at the point of
    /// transmission, which is the only honest start: `sent_at` is when the
    /// message was composed, and a message that waited for a tunnel or sat in
    /// the retry queue was composed long before it went anywhere.
    ///
    /// A retry muddies this — the clock restarts on each attempt, so what we
    /// time is the attempt that worked, which is the right thing to show.
    /// Anything implausible is dropped rather than smoothed: a device whose
    /// clock jumped would otherwise poison the number for the whole session.
    async fn note_round_trip(self: &Arc<Self>, contact_id: i64, msg: &gipny_libcore::db::Message) {
        let Some(sent_at) = msg.last_attempt_at else { return };
        let elapsed = now_ms() - sent_at;
        if elapsed <= 0 || elapsed > MAX_PLAUSIBLE_RTT_MS {
            return;
        }
        let ms = elapsed as u32;
        let median = {
            let mut all = self.rtt.lock().await;
            let entry = all.entry(contact_id).or_default();
            entry.push(ms);
            entry.median_ms()
        };
        if let Some(median) = median {
            gipny_libcore::debug_trace!("[link] contact {contact_id} round trip {ms} ms (median {median} ms)");
            let _ = self.events.try_send(CoreEvent::LinkRtt { contact_id, ms: median });
        }
    }

    async fn note_heard_from(self: &Arc<Self>, contact_id: i64) {
        if self.announce_pending.lock().await.remove(&contact_id) {
            self.announce_sent_at.lock().await.remove(&contact_id);
            gipny_libcore::debug_trace!("[relay-hosted] contact {contact_id} answered; they have our address");
        }
    }

    fn apply_contact_hints(self: &Arc<Self>, contact_id: i64, payload: &WirePayload) {
        if let Some(relay) = payload.relay_address.as_deref() {
            let trimmed = relay.trim();
            if !trimmed.is_empty() && gipny_libcore::card::is_valid_i2p_address(trimmed) {
                if let Ok(current) = self.db.contact_relay(contact_id) {
                    if !current.as_deref().is_some_and(|known| relay::same_destination(known, trimmed)) {
                        gipny_libcore::debug_trace!("[relay-discovery] updated relay for contact {} to {}", contact_id, &trimmed[..trimmed.len().min(16)]);
                        let _ = self.db.set_contact_relay(contact_id, Some(trimmed));
                        self.warm_relay_of(contact_id);
                    }
                }
            }
        }
        if let Some(name) = payload.sender_name.as_deref() {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                self.apply_peer_name(contact_id, trimmed);
            }
        }
    }

    /// Until a request is accepted, keep plain messages without showing or
    /// acknowledging them, and ignore everything else.
    async fn persist_from_requester(self: &Arc<Self>, contact_id: i64, payload: WirePayload) -> Result<()> {
        self.apply_contact_hints(contact_id, &payload);
        self.note_heard_from(contact_id).await;
        if !gipny_libcore::files::is_request_content(&payload) {
            return Ok(());
        }
        gipny_libcore::files::validate_message_files(&payload)?;
        let mut staged = gipny_libcore::files::StagedAttachments::new(&self.data_dir.join(ATTACHMENTS_DIR))?;
        for attachment in &payload.attachments {
            staged.inline(&attachment.name, &attachment.data)?;
        }
        let committed = staged.publish_incoming(&self.db, contact_id, &payload)?;
        // Acceptance may race the hint/network awaits above. Only an accepted
        // contact can start bytes or receive an acknowledgement.
        if !committed.pending && !committed.deleted {
            self.activate_message_files(contact_id, committed.id).await?;
            if let Some(contact) = self.db.get_contact(contact_id)? {
                if committed.inserted {
                    self.db.if_message_live(committed.id, || { let _ = self.events.try_send(CoreEvent::IncomingMessage {
                        contact_id: Some(contact_id), group_id: None, sender_sign_pk: Some(to_hex(&contact.identity_sign)),
                        message_id: committed.id, body: payload.body.clone(), sent_at: payload.sent_at,
                        notify_sound: payload.notify_sound.clone(), console_kind: None,
                    }); })?;
                }
                let _ = self.send_ack(&contact, &payload).await;
            }
        }
        Ok(())
    }

    /// One place for "we heard from them", whatever it was they sent.
    ///
    /// Everything from the peer passes through `persist_incoming` — a letter,
    /// a typing flag, a bare acknowledgement — and before this, only letters
    /// counted. A contact whose app was acking and typing all evening looked
    /// last seen in the morning, and the online dot depended on whether their
    /// chat happened to be open.
    ///
    /// Their clock is not ours, so a timestamp from the future is pulled back
    /// to now: a peer running fast must not be permanently "online".
    fn note_peer_seen(self: &Arc<Self>, contact_id: i64, sent_at: i64) {
        let now = now_ms();
        // Their clock is not ours. Anything written within the last few
        // minutes is treated as "just now": a peer whose clock runs a minute
        // slow would otherwise never light up at all, with nothing on screen
        // to explain it. Beyond that the letter's own time stands — mail off
        // a relay is hours old, and that is the case worth telling apart.
        let at = if (now - sent_at).abs() < LIVE_SKEW_MS { now } else { sent_at.min(now) };
        let _ = self.db.note_last_seen(contact_id, at);
        let _ = self.events.try_send(CoreEvent::PeerSeen { contact_id, at_ms: at });
    }

    async fn persist_incoming(self: &Arc<Self>, contact_id: i64, payload: WirePayload) -> Result<()> {
        if payload.wipe == Some(true) {
            let name = self.db.get_contact(contact_id)?.map(|c| c.display_name).unwrap_or_default();
            gipny_libcore::debug_trace!("[wipe] contact {contact_id} deleted us and asked for the chat to go; deleting it");
            self.delete_contact(contact_id).await?;
            let _ = self.events.try_send(CoreEvent::ContactWiped { contact_id, name });
            return Ok(());
        }
        // We are deleting them: nothing more from them lands in a chat we
        // already emptied.
        if self.wipe_pending_since(contact_id).is_some() {
            return Ok(());
        }
        self.note_peer_seen(contact_id, payload.sent_at);
        if self.handle_dm_control(contact_id, &payload).await? { return Ok(()); }
        if self.reject_deleted_dm_content(contact_id, &payload).await? { return Ok(()); }
        match self.db.get_contact(contact_id)?.map(|c| c.request_state) {
            Some(RequestState::Incoming) => return self.persist_from_requester(contact_id, payload).await,
            // Anything at all from them means our introduction arrived.
            Some(RequestState::Outgoing) => {
                self.db.set_contact_request_state(contact_id, RequestState::None)?;
                let _ = self.events.try_send(CoreEvent::ContactUpdated { contact_id });
            }
            _ => {}
        }
        self.heard.lock().unwrap_or_else(|p| p.into_inner()).insert(contact_id, now_ms());
        // Files in parts: these letters are nothing else.
        if let Some(ack) = &payload.file_ack {
            self.on_file_ack(contact_id, ack).await?;
        }
        if let Some(chunk) = &payload.file_chunk {
            self.on_file_chunk(contact_id, chunk, payload.sent_at, payload.dm.as_ref().map(|dm| &dm.reference)).await?;
        }
        if let Some(file_id) = &payload.file_cancel {
            self.on_file_cancel(contact_id, file_id).await?;
        }
        if let Some(typing) = payload.typing {
            let group_id_hex = payload.group.as_ref().map(|g| hex_bytes(&g.id));
            let sender_sign_hex = self.db.get_contact(contact_id).ok().flatten()
                .map(|c| hex_bytes(&c.identity_sign));
            let _ = self.events.try_send(CoreEvent::Typing {
                contact_id: if payload.group.is_none() { Some(contact_id) } else { None },
                group_id: group_id_hex,
                sender_sign_pk: sender_sign_hex,
                typing,
            });
            return Ok(());
        }
        self.apply_contact_hints(contact_id, &payload);
        self.note_heard_from(contact_id).await;
        if payload.buttons.is_some() || payload.callback_data.is_some() {
            if self.db.set_contact_is_bot(contact_id, true).unwrap_or(false) {
                let _ = self.events.try_send(CoreEvent::ContactUpdated { contact_id });
            }
        }
        if payload.ack_for.is_some() {
            if let Some(msg) = Self::acknowledge_delivery(&self.db, contact_id, &payload)? {
                self.note_round_trip(contact_id, &msg).await;
                let _ = self.events.try_send(CoreEvent::MessageDelivered { message_id: msg.id });
            }
            return Ok(());
        }

        if let Some(gref) = &payload.group {
            let sender = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
            let card = self.identity.card();
            self.db.apply_group_ref(gref, &sender.identity_sign, &card.sign_pk, &card.dh_pk)?;
            let _ = self.events.try_send(CoreEvent::GroupUpdated { group_id: to_hex(&gref.id) });
        }

        if payload.callback_data.is_some()
            && payload.edit_of.is_none()
            && payload.pin.is_none()
            && payload.body.is_empty()
            && payload.attachments.is_empty()
        {
            return Ok(());
        }

        if let Some(edit_target_origin) = payload.edit_of {
            let lookup = if let Some(gref) = &payload.group {
                let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
                let self_sign = self.identity.card().sign_pk.to_vec();
                self.db.resolve_group_message(&gref.id, &contact.identity_sign, edit_target_origin as i64, &self_sign)?
            } else {
                self.db.find_message_by_origin(contact_id, edit_target_origin as i64)?
            };
            if let Some(local_id) = lookup {
                if self.db.get_message(local_id)?.is_some_and(|message| message.deleted_at.is_some()) { return Ok(()); }
                self.db.update_message_body(local_id, &payload.body)?;
                if let Some(btns) = &payload.buttons {
                    if let Ok(b) = bincode::serialize(btns) {
                        self.db.set_setting(&format!("buttons_{}", local_id), &b)?;
                    }
                } else {
                    let _ = self.db.delete_setting(&format!("buttons_{}", local_id));
                }
                self.db.if_message_live(local_id, || { let _ = self.events.try_send(CoreEvent::MessageEdited {
                    message_id: local_id,
                    body: payload.body.clone(),
                    buttons: payload.buttons.clone(),
                }); })?;
                return Ok(());
            }
            if payload.group.is_none() { return Ok(()); }
        }

        if let Some(pin) = &payload.pin {
            let self_sign = self.identity.card().sign_pk.to_vec();
            let origin = pin.origin_msg_id as i64;
            let (resolved, target_cid, target_gid) = if let Some(gref) = &payload.group {
                (self.db.resolve_group_message(&gref.id, &pin.sender_sign_pk, origin, &self_sign)?,
                 None, Some(gref.id.clone()))
            } else {
                (self.db.resolve_contact_message(contact_id, origin)?,
                 Some(contact_id), None)
            };
            match resolved {
                Some(local) => {
                    match (&target_gid, pin.unpin) {
                        (Some(gid), true)  => { self.db.unpin_group_message(gid, local)?; }
                        (Some(gid), false) => { self.db.pin_group_message(gid, local)?; }
                        (None, true)       => { self.db.unpin_contact_message(contact_id, local)?; }
                        (None, false)      => { self.db.pin_contact_message(contact_id, local)?; }
                    }
                    let gid_hex = target_gid.as_deref().map(to_hex);
                    let _ = self.events.try_send(if pin.unpin {
                        CoreEvent::MessageUnpinned { contact_id: target_cid, group_id: gid_hex, message_id: local }
                    } else {
                        CoreEvent::MessagePinned { contact_id: target_cid, group_id: gid_hex, message_id: local }
                    });
                }
                None => match &target_gid {
                    Some(gid) => {
                        self.db.add_deferred_pin_group(gid, &pin.sender_sign_pk, origin, pin.unpin)?;
                        gipny_libcore::debug_trace!("[relay-client] deferred pin for group {} origin={}", to_hex(gid), pin.origin_msg_id);
                    }
                    None => {
                        self.db.add_deferred_pin_contact(contact_id, &pin.sender_sign_pk, origin, pin.unpin)?;
                        gipny_libcore::debug_trace!("[relay-client] deferred pin for contact {} origin={}", contact_id, pin.origin_msg_id);
                    }
                },
            }
            return Ok(());
        }

        let is_empty = payload.body.is_empty() && payload.attachments.is_empty() && payload.files.is_empty() && payload.group.is_none() && payload.callback_data.is_none() && payload.console.is_none();
        if is_empty { return Ok(()); }

        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;

        if payload.origin_msg_id > 0 {
            let existing = if let Some(gref) = &payload.group {
                let self_sign = self.identity.card().sign_pk.to_vec();
                self.db.resolve_group_message(&gref.id, &contact.identity_sign, payload.origin_msg_id as i64, &self_sign)?
            } else {
                self.db.find_message_by_origin(contact_id, payload.origin_msg_id as i64)?
            };
            if let Some(mid) = existing {
                gipny_libcore::debug_trace!("[relay-client] duplicate origin={} from contact {}, re-acking", payload.origin_msg_id, contact_id);
                self.activate_message_files(contact_id, mid).await?;
                let _ = self.send_ack(&contact, &payload).await;
                return Ok(());
            }
        }

        let group_id_for_event = payload.group.as_ref().map(|g| g.id.clone());
        if let Some(gid) = &group_id_for_event {
            if payload.body.is_empty() && payload.attachments.is_empty() && payload.files.is_empty() {
                let _ = self.events.try_send(CoreEvent::GroupUpdated { group_id: to_hex(gid) });
                return Ok(());
            }
        }
        gipny_libcore::files::validate_message_files(&payload)?;
        let mut staged = gipny_libcore::files::StagedAttachments::new(&self.data_dir.join(ATTACHMENTS_DIR))?;
        for attachment in &payload.attachments {
            staged.inline(&attachment.name, &attachment.data)?;
        }
        let committed = staged.publish_incoming(&self.db, contact_id, &payload)?;
        if committed.deleted || committed.pending { return Ok(()); }
        let mid = committed.id;
        self.activate_message_files(contact_id, mid).await?;
        if !committed.inserted {
            let _ = self.send_ack(&contact, &payload).await;
            return Ok(());
        }
        if let Some(c) = &payload.console {
            // Console framing is a DM affair; in a group it is stored for
            // display and nothing more.
            if group_id_for_event.is_none() {
                let from_master = self.agent_master().is_some_and(|m| m.contact_id == contact_id);
                match c.kind {
                    CONSOLE_COMMAND => {
                        // Pending until run. Stays pending while the mode is
                        // off, and runs when it is switched on for this contact.
                        if from_master {
                            let _ = self.agent_tx.send(mid);
                        }
                    }
                    CONSOLE_OFF => {
                        if from_master {
                            gipny_libcore::debug_trace!("[agent] master switched agent mode off");
                            let _ = self.disable_agent_mode(true).await;
                        }
                    }
                    CONSOLE_GRANT | CONSOLE_REVOKE => {
                        let granted = c.kind == CONSOLE_GRANT;
                        if self.db.set_contact_agent_granted(contact_id, granted).unwrap_or(false) {
                            let _ = self.events.try_send(CoreEvent::ContactUpdated { contact_id });
                        }
                    }
                    _ => {}
                }
            }
        }
        self.db.touch_contact(contact_id)?;
        let deferred_unpin = match &group_id_for_event {
            Some(gid) => self.db.take_deferred_pin_group(gid, &contact.identity_sign, payload.origin_msg_id as i64)?,
            None => self.db.take_deferred_pin_contact(contact_id, &contact.identity_sign, payload.origin_msg_id as i64)?,
        };
        if let Some(unpin) = deferred_unpin {
            match (&group_id_for_event, unpin) {
                (Some(gid), true)  => { self.db.unpin_group_message(gid, mid)?; }
                (Some(gid), false) => { self.db.pin_group_message(gid, mid)?; }
                (None, true)       => { self.db.unpin_contact_message(contact_id, mid)?; }
                (None, false)      => { self.db.pin_contact_message(contact_id, mid)?; }
            }
            let (tcid, tgid) = match &group_id_for_event {
                Some(gid) => (None, Some(to_hex(gid))),
                None => (Some(contact_id), None),
            };
            let _ = self.events.try_send(if unpin {
                CoreEvent::MessageUnpinned { contact_id: tcid, group_id: tgid, message_id: mid }
            } else {
                CoreEvent::MessagePinned { contact_id: tcid, group_id: tgid, message_id: mid }
            });
        }
        gipny_libcore::debug_trace!("[relay-client] received msg id={} from contact {}", mid, contact_id);
        let body_for_event = payload.body.clone();
        self.db.if_message_live(mid, || { let _ = self.events.try_send(CoreEvent::IncomingMessage {
            contact_id: if group_id_for_event.is_none() { Some(contact_id) } else { None },
            group_id: group_id_for_event.as_deref().map(to_hex),
            sender_sign_pk: Some(to_hex(&contact.identity_sign)),
            message_id: mid,
            body: body_for_event, sent_at: payload.sent_at,
            notify_sound: payload.notify_sound.clone(),
            console_kind: payload.console.as_ref().map(|c| c.kind),
        }); })?;
        let _ = self.send_ack(&contact, &payload).await;
        Ok(())
    }
}
