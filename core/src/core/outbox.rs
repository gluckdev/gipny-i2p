use super::*;

impl Core {
    pub(super) fn spawn_send_loop(self: Arc<Self>) {
        // A slow relay or DHT lookup must not delay local expiry or offline notices.
        let maintenance = self.clone();
        let handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(3));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                if let Err(error) = maintenance.maintain_message_expiry().await {
                    gipny_libcore::debug_error!("[delivery] message expiry: {error:?}");
                }
            }
        });
        self.tasks.lock().unwrap().push(handle);

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

    async fn maintain_message_expiry(&self) -> Result<()> {
        let now = now_ms();
        for notice in self.db.expire_outgoing_delivery(now)? {
            let _ = self.events.send(CoreEvent::MessageDeliveryExpired(notice)).await;
        }
        for deleted in self.db.delete_expired_dm(now)? { self.emit_dm_deleted(&deleted)?; }
        self.db.purge_expired(now)?;
        self.db.cleanup_deleted_files(&self.data_dir.join(ATTACHMENTS_DIR))?;
        Ok(())
    }

    pub(super) async fn flush_all_pending(self: &Arc<Self>) -> Result<()> {
        // Address announcements start once our own personal terminal is ready.
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
            let receipts = self.db.pending_delivery_receipts(contact.id, now_ms())?;
            let file_work = !self.db.file_peers_for(contact.id)?.is_empty();
            if pending.is_empty()
                && unacked.is_empty()
                && group_pending.is_empty()
                && receipts.is_empty()
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
                    tx
                }
                None => {
                    self.note_reachability(&contact,false,has_mail).await;
                    self.maybe_look_up_address(&contact).await;
                    continue;
                }
            };
            if self.ensure_session_for(&contact, &out).await.is_err() { continue; }
            for receipt in receipts {
                let mut payload = receipt.payload(now_ms(), &groups_by_id, &members_by_group,
                    &self.identity.card().sign_pk, &contact.identity_sign);
                if self.send_payload_via_relay(&contact, &mut payload, &out).await.is_err() {
                    break;
                }
                self.db.finish_delivery_receipt(contact.id, receipt.message_id)?;
            }
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
            if file_work {
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
                if let Some(id) = &msg.group_id {
                    payload.group = Some(WireGroupRef::from_stored_members(id, &groups_by_id, &members_by_group));
                }
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
        if db.pending_outbound_record_attempt(message_id, contact_id)? {
            send.await?;
        }
        Ok(())
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
        } else if !db.acknowledge_dm_delivery(local_id, contact_id)? {
            return Ok(None);
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
        if let Some(blob) = self.db.pending_initial(contact.id)? {
            self.deliver(contact, blob.clone(), route, None).await?;
            self.db.confirm_initial(contact.id, &blob)?;
        }
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
        let bundle_bytes = if prefetched.is_some() { prefetched } else {
                let out = route;
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
        ;
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
        let relay = self.relay_address();
        if !relay.is_empty() {
            empty_payload.relay_address = Some(relay);
        }
        let pt = pad_payload(&encode_payload(&empty_payload)?);
        let (state, init) = crypto::x3dh_initiate(&self.identity, &bundle, &pt, &ad)?;
        let own_dh = init.header.dh;
        let blob = bincode::serialize(&EnvelopeBlob::X3dhInit(init))?;
        if self.db.install_outbound_initial(contact.id, &state.to_bytes()?, &blob)? {
            self.own_inits.lock().await.insert(contact.id, own_dh);
            self.lost_inits.lock().await.remove(&contact.id);
            self.sessions.lock().await.insert(contact.id, state);
            self.session_created_at.lock().await.insert(contact.id, now_ms());
        }
        if let Some(pending) = self.db.pending_initial(contact.id)? {
            self.deliver(contact, pending.clone(), route, None).await?;
            self.db.confirm_initial(contact.id, &pending)?;
        }
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
            let r = self.relay_address();
            if !r.is_empty() {
                payload.relay_address = Some(r);
            }
        }
        let ad = build_ad(&self.identity.card().dh_pk, &contact.identity_dh);
        let raw = encode_payload(payload)?;
        if raw.len() > MAX_PAYLOAD_BYTES {
            gipny_libcore::debug_error!("[relay-client] payload too large ({}B), dropping msg id={}", raw.len(), payload.origin_msg_id);
            if payload.origin_msg_id > 0 {
                let _ = self.events.try_send(CoreEvent::MessageFailed { message_id: payload.origin_msg_id as i64, reason: "too large for one letter".into() });
            }
            return Err(CoreError::State);
        }
        let message_offer = payload.origin_msg_id > 0
            && payload.edit_of.is_none()
            && payload.pin.is_none()
            && payload.callback_data.is_none()
            && payload.ack_for.is_none();
        let pt = pad_payload(&raw);
        let msg_id = message_offer.then_some(payload.origin_msg_id as i64);
        let blob = {
            let mut sess = self.sessions.lock().await;
            if message_offer && !self.db.may_send_message(payload.origin_msg_id as i64, contact.id, now_ms())? {
                return Ok(());
            }
            if let Some(part) = &payload.file_chunk {
                if !self.db.may_send_file_part(&part.file_id, contact.id, now_ms())? { return Ok(()); }
            }
            let state = sess.get_mut(&contact.id).ok_or(CoreError::State)?;
            let Some(blob) = self.db.seal_for_deposit(contact.id, msg_id, state, &pt, &ad)? else { return Ok(()); };
            blob
        };
        let remaining = message_offer.then(|| {
            let limit = payload.ttl_ms.unwrap_or(i64::MAX).min(7 * 86_400_000);
            let ms = payload.sent_at.saturating_add(limit).saturating_sub(now_ms()).max(0);
            Duration::from_millis(ms as u64)
        });
        self.deliver(contact, blob.clone(), route, remaining).await?;

        self.incoming_since_send.lock().await.insert(contact.id, 0);

        if let Some(id) = msg_id {
            if self.db.confirm_deposit(contact.id, id, &blob)? {
                let _ = self.events.try_send(CoreEvent::MessageSent { message_id: id });
            }
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
        let ttl_ms = gipny_libcore::db::MessageLifetime::wire_ttl_ms(msg.sent_at,msg.expires_at);
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
        self.db.queue_delivery_receipt(contact.id, original, now_ms())?;
        self.send_kick.notify_one();
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
