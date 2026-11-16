use super::*;

impl Core {
    pub(super) fn spawn_send_loop(self: Arc<Self>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(3));
            tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
            tick.tick().await;
            loop {
                tokio::select! {
                    _ = this.send_kick.notified() => {}
                    _ = tick.tick() => {}
                }
                if let Err(e) = this.flush_all_pending().await {
                    gipny_libcore::debug_error!("[relay-client] flush err: {:?}", e);
                }
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    pub(super) async fn flush_all_pending(self: &Arc<Self>) -> Result<()> {
        for deleted in self.db.delete_expired_dm(now_ms())? { self.emit_dm_deleted(&deleted)?; }
        self.db.cleanup_deleted_files(&self.data_dir.join(ATTACHMENTS_DIR))?;
        self.db.purge_expired_group_outbound(now_ms())?;
        // Sending needs *their* relay, not ours: a letter is deposited where the
        // recipient collects it. Ours is where replies come back to, and it takes
        // a minute or two of tunnel building — waiting for it held every outgoing
        // message hostage for no reason (owner, 2026-09-18). A contact who has no
        // relay of their own still needs ours as the fallback, and `relay_for`
        // already returns nothing in that case, so that contact is skipped rather
        // than everyone.
        //
        // Announcing our address obviously waits for it to exist.
        if self.relay_out.read().await.is_some() {
            self.flush_relay_announcements().await;
        }
        let contacts = self.db.list_contacts()?;
        let groups_by_id: HashMap<Vec<u8>, String> = self.db.list_groups()?
            .into_iter().map(|g| (g.id, g.name)).collect();
        let members_by_group: HashMap<Vec<u8>, Vec<GroupMember>> = self.db.list_all_group_members()?;
        for contact in contacts {
            if let Some(since) = self.wipe_pending_since(contact.id) {
                // Only the delete request goes to them now.
                let _ = self.send_wipe(&contact, since).await;
                continue;
            }
            if contact.trust == TrustLevel::Blocked { continue; }
            // Nothing goes to someone we have not accepted, not even an ack.
            if contact.request_state == RequestState::Incoming { continue; }
            let pending = self.db.list_unsent_outgoing(contact.id, 50)?;
            let unacked = self.db.list_unacked_outgoing(
                contact.id, now_ms(), RETRY_BASE_BACKOFF_MS, RETRY_MAX_BACKOFF_MS, 50,
            )?;
            let introducing = contact.request_state == RequestState::Outgoing
                && !self.db.has_session(contact.id)?;
            let needs_session = (introducing || self.db.resync_recent(contact.id, 120_000).unwrap_or(false))
                && !self.sessions.lock().await.contains_key(&contact.id);
            let needs_keepalive = self.incoming_since_send.lock().await.get(&contact.id).copied().unwrap_or(0) >= KEEPALIVE_INCOMING_THRESHOLD
                && self.sessions.lock().await.contains_key(&contact.id);
            let group_pending = self.db.pending_outbound_for_recipient(
                contact.id, now_ms(), RETRY_BASE_BACKOFF_MS, RETRY_MAX_BACKOFF_MS, 50,
            )?;
            let deletions = self.db.pending_dm_deletes(contact.id, now_ms())?;
            let notice_acks=self.db.pending_dm_notice_acks(contact.id,now_ms())?;
            let peer_events = self.db.pending_peer_events(contact.id, now_ms())?;
            let file_work = !self.db.file_peers_for(contact.id)?.is_empty();
            if pending.is_empty()
                && unacked.is_empty()
                && group_pending.is_empty()
                && deletions.is_empty()
                && notice_acks.is_empty()
                && peer_events.is_empty()
                && !needs_session
                && !needs_keepalive
                && !file_work
            {
                continue;
            }
            // Deposit on the relay this contact collects from, not on ours.
            let has_mail = !pending.is_empty() || !unacked.is_empty();
            gipny_libcore::debug_trace!("[send] contact {} \"{}\": {} new, {} unacked, session={}, relay={}",
                contact.id,
                contact.display_name,
                pending.len(),
                unacked.len(),
                if self.db.has_session(contact.id).unwrap_or(false) { "yes" } else { "no" },
                contact.relay_address.as_deref().map(|r| &r[..16.min(r.len())]).unwrap_or("(none)"));
            let out = match self.relay_for(&contact).await {
                Some(tx) => {
                    self.note_reachability(&contact, true, has_mail).await;
                    Route::Relay(tx)
                }
                // Still dialling: the next round sends it there, in order.
                None if self.peer_relay_settling(&contact).await => continue,
                // Their relay is away. Leave it in the network, where it waits
                // for them — and keep the contact marked unreachable, because
                // nothing has been handed over yet.
                None => {
                    self.note_reachability(&contact, false, has_mail).await;
                    if self.dht.peer_count() == 0 {
                        gipny_libcore::debug_trace!("[send] contact {}: relay silent and the network is empty — mail stays queued", contact.id);
                        continue;
                    }
                    gipny_libcore::debug_trace!("[send] contact {}: relay silent, going through the network", contact.id);
                    // They may simply have restarted onto a new address; ask
                    // the network in the background, and meanwhile leave the
                    // letter where they will find it either way.
                    self.maybe_look_up_address(&contact).await;
                    Route::Dht
                }
            };
            if self.ensure_session_for(&contact, &out).await.is_err() { continue; }
            for reference in deletions {
                let at=self.db.dm_local_delete_time(contact.id,&reference)?;
                let mut payload=WirePayload::simple(0,String::new(),vec![],at,None);
                payload.dm=Some(gipny_libcore::session::WireDm {kind:1,reference:reference.clone()});
                self.db.record_dm_delete_attempt(contact.id,&reference,now_ms())?;
                let _=self.send_payload_via_relay(&contact,&mut payload,&out).await;
            }
            for reference in notice_acks {
                let mut payload=WirePayload::simple(0,String::new(),vec![],now_ms(),None);
                payload.dm=Some(gipny_libcore::session::WireDm {kind:3,reference:reference.clone()});
                self.db.record_dm_notice_attempt(contact.id,&reference,now_ms())?;
                let _=self.send_payload_via_relay(&contact,&mut payload,&out).await;
            }
            for (reference, kind) in peer_events {
                let mut payload = WirePayload::simple(0, String::new(), vec![], now_ms(), None);
                payload.dm = Some(gipny_libcore::session::WireDm { kind, reference: reference.clone() });
                self.db.record_peer_event_attempt(contact.id, &reference, kind, now_ms())?;
                let _ = self.send_payload_via_relay(&contact, &mut payload, &out).await;
            }
            if needs_keepalive && pending.is_empty() && unacked.is_empty() {
                let mut payload = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
                payload.ack_for = Some(0);
                if let Err(e) = self.send_payload_via_relay(&contact, &mut payload, &out).await {
                    gipny_libcore::debug_trace!("[relay-client] keepalive err to contact {}: {:?}", contact.id, e);
                } else {
                    gipny_libcore::debug_trace!("[relay-client] keepalive sent to contact {} (DH-roll forced)", contact.id);
                }
            }
            for msg in pending {
                let mut payload = self.build_payload_from_db(&msg)?;
                if let Err(e) = self.send_payload_via_relay(&contact, &mut payload, &out).await {
                    gipny_libcore::debug_trace!("[relay-client] send err to contact {}: {:?}", contact.id, e);
                    break;
                }
            }
            for msg in unacked {
                let mut payload = self.build_payload_from_db(&msg)?;
                gipny_libcore::debug_trace!("[relay-client] retry unacked msg {} to contact {} (attempt {})",
                    msg.id, contact.id, msg.send_attempts + 1);
                // Record before sending, like the group path below: without it
                // last_attempt_at stays NULL, the backoff in
                // list_unacked_outgoing is always satisfied, and every
                // undelivered message goes out on every tick of the relay loop.
                self.db.record_send_attempt(msg.id)?;
                if let Err(e) = self.send_payload_via_relay(&contact, &mut payload, &out).await {
                    gipny_libcore::debug_trace!("[relay-client] retry err to contact {}: {:?}", contact.id, e);
                    break;
                }
            }
            // Parts only to their relay: the relay network holds too little
            // per key for a file (dht/src/store.rs); they wait for it.
            if file_work && matches!(out, Route::Relay(_)) {
                if let Err(e) = self.send_file_parts(&contact, &out).await {
                    gipny_libcore::debug_trace!("[files] parts to contact {}: {:?}", contact.id, e);
                }
            }
            for msg_id in group_pending {
                let msg = match self.db.get_message(msg_id)? { Some(m) => m, None => {
                    let _ = self.db.pending_outbound_remove(msg_id, contact.id);
                    continue;
                } };
                let mut payload = self.build_payload_from_db(&msg)?;
                if let Some(gid) = &msg.group_id {
                    let gname = groups_by_id.get(gid).cloned().unwrap_or_default();
                    let gref_members: Vec<WireMember> = members_by_group.get(gid).map(|v| v.iter().map(|m| WireMember {
                        sign_pk: m.sign_pk.clone(), dh_pk: m.dh_pk.clone(),
                        onion: m.onion.clone(), name: m.display_name.clone(),
                    }).collect()).unwrap_or_default();
                    payload.group = Some(WireGroupRef { id: gid.clone(), name: gname, members: gref_members });
                }
                gipny_libcore::debug_trace!("[relay-client] retry group msg {} to contact {}", msg_id, contact.id);
                if let Err(e) = Self::attempt_group_delivery(
                    &self.db, msg_id, contact.id,
                    self.send_payload_via_relay(&contact, &mut payload, &out),
                ).await {
                    gipny_libcore::debug_trace!("[relay-client] retry group err to contact {}: {:?}", contact.id, e);
                    break;
                }
            }
        }
        Ok(())
    }

    /// Both the initial send and retries retain the durable recipient row
    /// until its end-to-end ACK. Record before polling the send future so
    /// failures and accepted-but-lost relay enqueues obey the same backoff.
    pub(super) async fn attempt_group_delivery<F>(db: &Db, message_id: i64, contact_id: i64, send: F) -> Result<()>
    where F: std::future::Future<Output = Result<()>>,
    {
        db.pending_outbound_record_attempt(message_id, contact_id)?;
        send.await
    }

    /// `contact_id` comes from the authenticated, decrypted session. A group
    /// ACK may consume only that recipient's obligation for this message.
    pub(super) fn acknowledge_delivery(db: &Db, contact_id: i64, payload: &WirePayload) -> Result<Option<gipny_libcore::db::Message>> {
        let Some(ack_id) = payload.ack_for else { return Ok(None) };
        let Ok(local_id) = i64::try_from(ack_id) else { return Ok(None) };
        let Some(msg) = db.get_message(local_id)? else { return Ok(None) };
        let belongs = match (msg.contact_id, &msg.group_id, &payload.group) {
            (Some(cid), _, None) => cid == contact_id,
            (_, Some(gid), Some(gref)) => gid == &gref.id,
            _ => false,
        };
        if msg.deleted_at.is_some() || !belongs || !matches!(msg.direction, Direction::Out) { return Ok(None); }
        if msg.group_id.is_some() {
            if !matches!(db.acknowledge_group_delivery(local_id, contact_id)?, gipny_libcore::db::GroupDeliveryAck::Complete) {
                return Ok(None);
            }
        } else {
            let _ = db.pending_outbound_remove(local_id, contact_id);
            db.mark_delivered(local_id)?;
        }
        Ok(Some(msg))
    }

    pub(super) async fn send_to_contact(self: &Arc<Self>, contact_id: i64, payload: &mut WirePayload) -> Result<()> {
        let contact = self.db.get_contact(contact_id)?.ok_or(CoreError::NotFound)?;
        let out = self.route_for(&contact).await.ok_or(CoreError::State)?;
        self.ensure_session_for(&contact, &out).await?;
        self.send_payload_via_relay(&contact, payload, &out).await
    }

    pub(super) async fn ensure_session_for(
        &self,
        contact: &gipny_libcore::db::Contact,
        route: &Route,
    ) -> Result<()> {
        if self.sessions.lock().await.contains_key(&contact.id) {
            gipny_libcore::debug_trace!("[relay-client] ensure_session: contact {} already in cache, no-op", contact.id);
            return Ok(());
        }
        if let Some(state) = self.db.get_session(contact.id)?.map(|blob| {
            let blob=zeroize::Zeroizing::new(blob);
            RatchetState::from_bytes(&blob)
        }).transpose()? {
            gipny_libcore::debug_trace!("[relay-client] ensure_session: contact {} found in DB, loading", contact.id);
            self.sessions.lock().await.insert(contact.id, state);
            return Ok(());
        }
        gipny_libcore::debug_trace!("[relay-client] ensure_session: contact {} has NO session, will initiate X3DH", contact.id);

        // No waiting for the other side to go first (10 s on every new
        // conversation): if both open at once, `ours_stands` picks one.

        let mut pk = [0u8; 32];
        pk.copy_from_slice(&contact.identity_sign);
        let prefetched = {
            let mut cache = self.bundle_cache.lock().await;
            cache.remove(&pk).filter(|(_, at)| at.elapsed() < BUNDLE_PREFETCH_TTL).map(|(b, _)| b)
        };
        let bundle_bytes = match route {
            _ if prefetched.is_some() => prefetched,
            Route::Relay(out) => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                self.bundle_waiters.lock().await.entry(pk).or_default().push(tx);
                if out.send(ClientToRelay::GetBundle { pk }).await.is_err() {
                    gipny_libcore::debug_trace!("[relay-client] get_bundle send failed for contact {} (relay channel closed)", contact.id);
                    return Err(CoreError::State);
                }
                match tokio::time::timeout(Duration::from_millis(PENDING_REQ_TIMEOUT_MS), rx).await {
                    Err(_) => {
                        gipny_libcore::debug_trace!("[relay-client] get_bundle timeout for contact {} after {}ms — relay not responding", contact.id, PENDING_REQ_TIMEOUT_MS);
                        return Err(CoreError::State);
                    }
                    Ok(Err(_)) => {
                        gipny_libcore::debug_trace!("[relay-client] get_bundle channel dropped for contact {}", contact.id);
                        return Err(CoreError::State);
                    }
                    Ok(Ok(v)) => v,
                }
            }
            // Their relay is away, so their bundle comes from the network,
            // where they publish it for exactly this case.
            Route::Dht => {
                let their_dh = to_arr32(contact.identity_dh.clone())?;
                dht_client::find_bundle(&self.dht, &pk, &their_dh).await
            }
        };
        let bundle_bytes = match bundle_bytes {
            Some(b) => b,
            None => {
                gipny_libcore::debug_trace!("[relay-client] get_bundle: relay has NO bundle for contact {} — peer never published or relay dropped it", contact.id);
                return Err(CoreError::NotFound);
            }
        };
        let bundle: PreKeyBundle = bincode::deserialize(&bundle_bytes)?;

        if bundle.identity.sign_pk != contact.identity_sign.as_slice()
            || bundle.identity.dh_pk != contact.identity_dh.as_slice()
        {
            gipny_libcore::debug_trace!("[relay-client] bundle identity mismatch for contact {} — bundle from someone else?", contact.id);
            return Err(CoreError::State);
        }
        let ad = build_ad(&self.identity.card().dh_pk, &contact.identity_dh);
        let mut empty_payload = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
        empty_payload.sender_name = self.outgoing_sender_name();
        // A contact created from this init would otherwise have no relay to
        // answer to until a later message brings one.
        let relay = self.relay_onion();
        if !relay.is_empty() {
            empty_payload.relay_address = Some(relay);
        }
        let pt = pad_payload(&encode_payload(&empty_payload)?);
        let (state, init) = crypto::x3dh_initiate(&self.identity, &bundle, &pt, &ad)?;
        self.own_inits.lock().await.insert(contact.id, init.header.dh);
        self.lost_inits.lock().await.remove(&contact.id);
        self.db.put_session(contact.id, &state.to_bytes()?)?;
        self.sessions.lock().await.insert(contact.id, state);
        self.session_created_at.lock().await.insert(contact.id, now_ms());

        let envelope = EnvelopeBlob::X3dhInit(init);
        let blob = bincode::serialize(&envelope)?;
        self.deliver(contact, blob, route, true).await?;
        gipny_libcore::debug_trace!("[relay-client] x3dh sent to contact {}", contact.id);
        Ok(())
    }

    pub(super) async fn send_payload_via_relay(
        &self,
        contact: &gipny_libcore::db::Contact,
        payload: &mut WirePayload,
        route: &Route,
    ) -> Result<()> {
        if payload.sender_name.is_none() {
            payload.sender_name = self.outgoing_sender_name();
        }
        // Our relay rides along so the contact learns where we collect, but
        // not on typing notices: they are the most frequent payload by far and
        // a full destination is ~520 bytes of padding-bucket every keystroke.
        if payload.relay_address.is_none() && payload.typing.is_none() {
            let r = self.relay_onion();
            if !r.is_empty() {
                payload.relay_address = Some(r);
            }
        }
        let ad = build_ad(&self.identity.card().dh_pk, &contact.identity_dh);
        let raw = encode_payload(payload)?;
        if raw.len() > MAX_PAYLOAD_BYTES {
            gipny_libcore::debug_error!("[relay-client] payload too large ({}B), dropping msg id={}", raw.len(), payload.origin_msg_id);
            if payload.origin_msg_id > 0 {
                let _ = self.db.mark_sent(payload.origin_msg_id as i64);
                let _ = self.events.try_send(CoreEvent::MessageFailed { message_id: payload.origin_msg_id as i64, reason: "too large for one letter".into() });
            }
            return Err(CoreError::State);
        }
        let pt = pack_payload(&raw, self.lane.lock().await.padded());
        let (header, ct) = {
            let mut sess = self.sessions.lock().await;
            let state = sess.get_mut(&contact.id).ok_or(CoreError::State)?;
            let r = state.encrypt(&pt, &ad)?;
            self.db.put_session(contact.id, &state.to_bytes()?)?;
            r
        };
        let envelope = EnvelopeBlob::Ratchet { header, ciphertext: ct };
        let blob = bincode::serialize(&envelope)?;
        self.deliver(contact, blob, route, false).await?;

        self.incoming_since_send.lock().await.insert(contact.id, 0);

        if payload.origin_msg_id > 0
            && payload.edit_of.is_none()
            && payload.pin.is_none()
            && payload.callback_data.is_none()
            && payload.ack_for.is_none()
        {
            self.db.mark_sent(payload.origin_msg_id as i64)?;
            let _ = self.events.try_send(CoreEvent::MessageSent { message_id: payload.origin_msg_id as i64 });
        }
        Ok(())
    }

    pub(super) fn build_payload_from_db(&self, msg: &gipny_libcore::db::Message) -> Result<WirePayload> {
        let atts = self.db.list_attachments(msg.id)?;
        let mut wire_atts = Vec::with_capacity(atts.len());
        for a in atts {
            // Sealed in parts: offered below and sent from file_out.
            if a.chunk_size.is_some() { continue; }
            let key = to_arr32(a.key.clone())?;
            let full = self.data_dir.join(ATTACHMENTS_DIR).join(&a.path);
            let enc = std::fs::read(&full)?;
            let data = AttachmentCipher::from_key(key).decrypt_chunk(0, &[], &enc)?;
            wire_atts.push(WireAttachment { name: a.name, data });
        }
        let ttl_ms = msg.expires_at.map(|e| e - msg.sent_at);
        let mut p = WirePayload::simple(msg.id as u64, msg.body.clone(), wire_atts, msg.sent_at, ttl_ms);
        if let Some(rt) = msg.reply_to {
            p.reply_to = self.build_wire_reply(rt)?;
        }
        p.console = self.db.get_setting(&format!("console_{}", msg.id))
            .ok().flatten()
            .and_then(|b| bincode::deserialize::<WireConsole>(&b).ok());
        p.files = self.offers_for(msg.id)?;
        if msg.group_id.is_none() && msg.contact_id.is_some() {
            p.dm = Some(gipny_libcore::session::WireDm { kind: 0, reference: self.db.dm_reference(msg.id)? });
        }
        Ok(p)
    }

    pub(super) async fn send_ack(self: &Arc<Self>, contact: &gipny_libcore::db::Contact, original: &WirePayload) -> Result<()> {
        if original.origin_msg_id == 0 { return Ok(()); }
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
            relay_address: None, wipe: None, files: Vec::new(), file_chunk: None, file_ack: None, file_cancel: None, dm: None,
        };
        let out = match self.route_for(contact).await {
            Some(x) => x,
            None => return Ok(()),
        };
        if self.ensure_session_for(contact, &out).await.is_err() {
            return Ok(());
        }
        let _ = self.send_payload_via_relay(contact, &mut payload, &out).await;
        Ok(())
    }

    pub(super) async fn request_resync(&self, contact: &gipny_libcore::db::Contact) -> Result<()> {
        let throttled = self.db.resync_recent(contact.id, 60_000)?;
        if !throttled {
            gipny_libcore::debug_trace!("[relay-client] forcing resync for contact {}", contact.id);
            self.db.record_resync(contact.id)?;
            self.sessions.lock().await.remove(&contact.id);
            self.session_created_at.lock().await.remove(&contact.id);
            let _ = self.db.delete_session(contact.id);
        }
        self.send_kick.notify_one();
        Ok(())
    }
}
