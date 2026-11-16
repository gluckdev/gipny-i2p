//! Pending delivery, session establishment and encryption.

use super::*;

impl SessionManager {
    pub(super) fn spawn_send_loop(self: Arc<Self>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            tick.tick().await;
            loop {
                tokio::select! {
                    _ = this.send_kick.notified() => {}
                    _ = tick.tick() => {}
                }
                if let Err(e) = this.flush_all_pending().await {
                    crate::debug_error!("[session] flush err: {:?}", e);
                }
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    pub(super) async fn flush_all_pending(self: &Arc<Self>) -> Result<()> {
        for deletion in self.db.delete_expired_dm(now_ms())? {
            self.emit_dm_deleted(&deletion)?;
        }
        self.db
            .cleanup_deleted_files(&self.data_dir.join(ATTACHMENTS_DIR))?;
        self.db.purge_expired_group_outbound(now_ms())?;
        let contacts = self.db.list_contacts()?;
        let groups_by_id: HashMap<Vec<u8>, String> = self
            .db
            .list_groups()?
            .into_iter()
            .map(|group| (group.id, group.name))
            .collect();
        let members_by_group: HashMap<Vec<u8>, Vec<GroupMember>> =
            self.db.list_all_group_members()?;
        for contact in contacts {
            if contact.trust == TrustLevel::Blocked {
                continue;
            }
            let pending = self.db.list_unsent_outgoing(contact.id, 50)?;
            let unacked = self.db.list_unacked_outgoing(
                contact.id,
                now_ms(),
                RETRY_BASE_BACKOFF_MS,
                RETRY_MAX_BACKOFF_MS,
                50,
            )?;
            let needs_session = self.db.resync_recent(contact.id, 120_000).unwrap_or(false)
                && !self.sessions.lock().await.contains_key(&contact.id);
            let needs_keepalive = self
                .incoming_since_send
                .lock()
                .await
                .get(&contact.id)
                .copied()
                .unwrap_or(0)
                >= KEEPALIVE_INCOMING_THRESHOLD
                && self.sessions.lock().await.contains_key(&contact.id);
            let group_pending = self.db.pending_outbound_for_recipient(
                contact.id,
                now_ms(),
                RETRY_BASE_BACKOFF_MS,
                RETRY_MAX_BACKOFF_MS,
                50,
            )?;
            let deletions = self.db.pending_dm_deletes(contact.id, now_ms())?;
            let notice_acks = self.db.pending_dm_notice_acks(contact.id, now_ms())?;
            let file_work = !self.db.file_peers_for(contact.id)?.is_empty();
            if pending.is_empty()
                && unacked.is_empty()
                && group_pending.is_empty()
                && deletions.is_empty()
                && notice_acks.is_empty()
                && !needs_session
                && !needs_keepalive
                && !file_work
            {
                continue;
            }
            let route = match self.relay_for(&contact).await {
                Some(out) => Route::Relay(out),
                None if self.peer_relay_settling(&contact).await => continue,
                None if self.dht.peer_count() > 0 => {
                    self.maybe_look_up_address(&contact).await;
                    Route::Dht
                }
                None => continue,
            };
            if self.ensure_session_for(&contact, &route).await.is_err() {
                continue;
            }
            for reference in deletions {
                let at = self.db.dm_local_delete_time(contact.id, &reference)?;
                let mut payload = WirePayload::simple(0, String::new(), vec![], at, None);
                payload.dm = Some(crate::session::WireDm {
                    kind: 1,
                    reference: reference.clone(),
                });
                self.db
                    .record_dm_delete_attempt(contact.id, &reference, now_ms())?;
                let _ = self
                    .send_payload_via_relay(&contact, &mut payload, &route)
                    .await;
            }
            for reference in notice_acks {
                let mut payload = WirePayload::simple(0, String::new(), vec![], now_ms(), None);
                payload.dm = Some(crate::session::WireDm {
                    kind: 3,
                    reference: reference.clone(),
                });
                self.db
                    .record_dm_notice_attempt(contact.id, &reference, now_ms())?;
                let _ = self
                    .send_payload_via_relay(&contact, &mut payload, &route)
                    .await;
            }
            if needs_keepalive && pending.is_empty() && unacked.is_empty() {
                let mut payload = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
                payload.ack_for = Some(0);
                if let Err(e) = self
                    .send_payload_via_relay(&contact, &mut payload, &route)
                    .await
                {
                    crate::debug_trace!(
                        "[session] keepalive err to contact {}: {:?}",
                        contact.id,
                        e
                    );
                } else {
                    crate::debug_trace!(
                        "[session] keepalive sent to contact {} (DH-roll forced)",
                        contact.id
                    );
                }
            }
            for msg in pending {
                let mut payload = self.build_payload_from_db(&msg)?;
                if let Err(e) = self
                    .send_payload_via_relay(&contact, &mut payload, &route)
                    .await
                {
                    crate::debug_trace!("[session] send err contact {}: {:?}", contact.id, e);
                    break;
                }
            }
            for msg in unacked {
                let mut payload = self.build_payload_from_db(&msg)?;
                crate::debug_trace!(
                    "[session] retry unacked msg {} to contact {} (attempt {})",
                    msg.id,
                    contact.id,
                    msg.send_attempts + 1
                );
                self.db.record_send_attempt(msg.id)?;
                if let Err(e) = self
                    .send_payload_via_relay(&contact, &mut payload, &route)
                    .await
                {
                    crate::debug_trace!("[session] retry err contact {}: {:?}", contact.id, e);
                    break;
                }
            }
            // Parts only to their relay: the relay network holds too little
            // per key for a file (dht/src/store.rs); they wait for it.
            if file_work && matches!(route, Route::Relay(_)) {
                if let Err(e) = self.send_file_parts(&contact, &route).await {
                    crate::debug_trace!("[files] parts to contact {}: {:?}", contact.id, e);
                }
            }
            for msg_id in group_pending {
                let msg = match self.db.get_message(msg_id)? {
                    Some(msg) => msg,
                    None => {
                        let _ = self.db.pending_outbound_remove(msg_id, contact.id);
                        continue;
                    }
                };
                let mut payload = self.build_payload_from_db(&msg)?;
                if let Some(group_id) = &msg.group_id {
                    let name = groups_by_id.get(group_id).cloned().unwrap_or_default();
                    let members = members_by_group
                        .get(group_id)
                        .map(|members| {
                            members
                                .iter()
                                .map(|member| WireMember {
                                    sign_pk: member.sign_pk.clone(),
                                    dh_pk: member.dh_pk.clone(),
                                    i2p_address: member.i2p_address.clone(),
                                    name: member.display_name.clone(),
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    payload.group = Some(WireGroupRef {
                        id: group_id.clone(),
                        name,
                        members,
                    });
                }
                if let Err(e) = attempt_group_delivery(
                    &self.db,
                    msg_id,
                    contact.id,
                    self.send_payload_via_relay(&contact, &mut payload, &route),
                )
                .await
                {
                    crate::debug_trace!(
                        "[session] retry group msg {} to contact {} failed: {:?}",
                        msg_id,
                        contact.id,
                        e
                    );
                    break;
                }
            }
        }
        Ok(())
    }

    pub(super) async fn ensure_session_for(&self, contact: &Contact, route: &Route) -> Result<()> {
        if self.sessions.lock().await.contains_key(&contact.id) {
            return Ok(());
        }
        if let Some(state) = self
            .db
            .get_session(contact.id)?
            .map(|blob| {
                let blob = zeroize::Zeroizing::new(blob);
                RatchetState::from_bytes(&blob)
            })
            .transpose()?
        {
            self.sessions.lock().await.insert(contact.id, state);
            return Ok(());
        }
        // No waiting for the other side to go first (that cost every new
        // conversation 10 s): if both open at once, `ours_stands` picks one.

        let mut pk = [0u8; 32];
        pk.copy_from_slice(&contact.identity_sign);
        let prefetched = {
            let mut cache = self.bundle_cache.lock().await;
            cache
                .remove(&pk)
                .filter(|(_, at)| at.elapsed() < BUNDLE_PREFETCH_TTL)
                .map(|(b, _)| b)
        };
        let bundle_bytes = match route {
            _ if prefetched.is_some() => prefetched,
            Route::Relay(out) => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                self.bundle_waiters
                    .lock()
                    .await
                    .entry(pk)
                    .or_default()
                    .push(tx);
                out.send(ClientToRelay::GetBundle { pk })
                    .await
                    .map_err(|_| SessionError::State)?;
                tokio::time::timeout(Duration::from_millis(PENDING_REQ_TIMEOUT_MS), rx)
                    .await
                    .map_err(|_| SessionError::State)?
                    .map_err(|_| SessionError::State)?
            }
            Route::Dht => {
                let their_dh = to_arr32(contact.identity_dh.clone())?;
                dht_client::find_bundle(&self.dht, &pk, &their_dh).await
            }
        };
        let bundle_bytes = bundle_bytes.ok_or(SessionError::NotFound)?;
        let bundle: PreKeyBundle = bincode::deserialize(&bundle_bytes)?;

        if bundle.identity.sign_pk != contact.identity_sign.as_slice()
            || bundle.identity.dh_pk != contact.identity_dh.as_slice()
        {
            return Err(SessionError::State);
        }
        let ad = build_ad(&self.identity.card().dh_pk, &contact.identity_dh);
        let mut empty = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
        empty.sender_name = self.outgoing_sender_name();
        // A contact created from this init would otherwise have no relay to
        // answer to until a later message brings one.
        let relay = self.relay_address();
        if !relay.is_empty() {
            empty.relay_address = Some(relay);
        }
        let pt = pad_payload(&encode_payload(&empty)?);
        let (state, init) = crypto::x3dh_initiate(&self.identity, &bundle, &pt, &ad)?;
        self.own_inits
            .lock()
            .await
            .insert(contact.id, init.header.dh);
        self.lost_inits.lock().await.remove(&contact.id);
        self.db.put_session(contact.id, &state.to_bytes()?)?;
        self.sessions.lock().await.insert(contact.id, state);
        self.session_created_at
            .lock()
            .await
            .insert(contact.id, now_ms());
        let blob = bincode::serialize(&EnvelopeBlob::X3dhInit(init))?;
        self.deliver(contact, blob, route, true).await?;
        crate::debug_trace!("[session] x3dh sent to contact {}", contact.id);
        Ok(())
    }

    pub(super) async fn send_payload_via_relay(
        &self,
        contact: &Contact,
        payload: &mut WirePayload,
        route: &Route,
    ) -> Result<()> {
        if payload.sender_name.is_none() {
            payload.sender_name = self.outgoing_sender_name();
        }
        // Every payload tells the contact where we collect, except typing
        // notices: the most frequent payload, and ~520 bytes of address each.
        if payload.relay_address.is_none() && payload.typing.is_none() {
            let r = self.relay_address();
            if !r.is_empty() {
                payload.relay_address = Some(r);
            }
        }
        let ad = build_ad(&self.identity.card().dh_pk, &contact.identity_dh);
        let raw = encode_payload(payload)?;
        if raw.len() > MAX_PAYLOAD_BYTES {
            crate::debug_error!(
                "[session] payload too large ({}B), dropping msg id={}",
                raw.len(),
                payload.origin_msg_id
            );
            if payload.origin_msg_id > 0 {
                let _ = self.db.mark_sent(payload.origin_msg_id as i64);
                let _ = self
                    .events
                    .send(SessionEvent::MessageFailed {
                        message_id: payload.origin_msg_id as i64,
                        reason: "too large for one letter".into(),
                    })
                    .await;
            }
            return Err(SessionError::State);
        }
        let pt = pad_payload(&raw);
        let (header, ct) = {
            let mut sess = self.sessions.lock().await;
            let state = sess.get_mut(&contact.id).ok_or(SessionError::State)?;
            let r = state.encrypt(&pt, &ad)?;
            self.db.put_session(contact.id, &state.to_bytes()?)?;
            r
        };
        let blob = bincode::serialize(&EnvelopeBlob::Ratchet {
            header,
            ciphertext: ct,
        })?;
        self.deliver(contact, blob, route, false).await?;
        self.incoming_since_send.lock().await.insert(contact.id, 0);
        if payload.origin_msg_id > 0
            && payload.edit_of.is_none()
            && payload.pin.is_none()
            && payload.callback_data.is_none()
            && payload.ack_for.is_none()
        {
            self.db.mark_sent(payload.origin_msg_id as i64)?;
        }
        Ok(())
    }

    pub(super) fn build_payload_from_db(&self, msg: &crate::db::Message) -> Result<WirePayload> {
        let atts = self.db.list_attachments(msg.id)?;
        let mut wire_atts = Vec::with_capacity(atts.len());
        for a in atts {
            // Sealed in parts: offered below and sent from file_out.
            if a.chunk_size.is_some() {
                continue;
            }
            let key = to_arr32(a.key.clone())?;
            let full = self.data_dir.join(ATTACHMENTS_DIR).join(&a.path);
            let enc = std::fs::read(&full)?;
            let data = AttachmentCipher::from_key(key).decrypt_chunk(0, &[], &enc)?;
            wire_atts.push(WireAttachment { name: a.name, data });
        }
        let buttons: Option<Vec<Vec<WireButton>>> = self
            .db
            .get_setting(&format!("buttons_{}", msg.id))
            .ok()
            .flatten()
            .and_then(|b| bincode::deserialize::<Vec<Vec<WireButton>>>(&b).ok());
        let sound: Option<String> = self
            .db
            .get_setting(&format!("sound_{}", msg.id))
            .ok()
            .flatten()
            .and_then(|b| String::from_utf8(b).ok());
        let console: Option<WireConsole> = self
            .db
            .get_setting(&format!("console_{}", msg.id))
            .ok()
            .flatten()
            .and_then(|b| bincode::deserialize::<WireConsole>(&b).ok());
        let ttl_ms = msg.expires_at.map(|e| e - msg.sent_at);
        let mut p = WirePayload::simple(
            msg.id as u64,
            msg.body.clone(),
            wire_atts,
            msg.sent_at,
            ttl_ms,
        );
        p.buttons = buttons;
        p.notify_sound = sound;
        p.console = console;
        p.files = self.offers_for(msg.id)?;
        if msg.group_id.is_none() && msg.contact_id.is_some() {
            p.dm = Some(WireDm {
                kind: 0,
                reference: self.db.dm_reference(msg.id)?,
            });
        }
        Ok(p)
    }
}
