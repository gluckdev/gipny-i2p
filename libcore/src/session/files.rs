//! Attachment staging, transfer lanes and file acknowledgements.

use super::*;

impl SessionManager {
    /// Transfer counters for this session only; no identifiers or payload data.
    pub fn file_metrics(&self) -> crate::files::metrics::FileMetricsSnapshot {
        self.file_metrics.snapshot()
    }

    /// Just started: tell everyone sending us a file what we hold of it, so
    /// what was in flight when we stopped goes again at once instead of at
    /// its timeout. Tried every 10 s for five minutes, until each went.
    pub(super) fn spawn_resume_acks(self: Arc<Self>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            let Ok(files) = this.db.files_in_stale(i64::MAX) else {
                return;
            };
            let mut left: Vec<_> = files.into_iter().collect();
            for _ in 0..30 {
                if left.is_empty() {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
                let mut still = Vec::new();
                for fin in left {
                    if !this.db.can_receive_files(fin.contact_id).unwrap_or(false) {
                        continue;
                    }
                    let Some(fin) = this.db.file_in(&fin.file_id, fin.contact_id).ok().flatten()
                    else {
                        continue;
                    };
                    let total = crate::files::chunk_count(fin.size as u64, fin.chunk_size);
                    let ack = crate::files::Received::from_bits(fin.bits.clone(), total)
                        .resume_ack(fin.file_id);
                    if this
                        .send_file_message(fin.contact_id, |p| p.file_ack = Some(ack))
                        .await
                    {
                        crate::debug_trace!(
                            "[files] told contact {} we are back, holding {} of {} parts",
                            fin.contact_id,
                            fin.received,
                            total
                        );
                    } else {
                        still.push(fin);
                    }
                }
                left = still;
            }
        });
        self.track(handle);
    }

    /// Keep extra connections to our own relay while file parts come, when it
    /// runs elsewhere (one in this process is a pipe, no faster for more).
    pub(super) fn spawn_collect_lanes(self: Arc<Self>) {
        use std::sync::atomic::Ordering::Relaxed;
        let this = self.clone();
        let handle = tokio::spawn(async move {
            loop {
                let _ = tokio::time::timeout(Duration::from_secs(5), this.collect_kick.notified())
                    .await;
                let i2p_address = this.relay_address();
                let external = !i2p_address.is_empty()
                    && this.relay_out.read().await.is_some()
                    && !this
                        .local_relay
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .as_ref()
                        .is_some_and(|r| relay::same_destination(r.address(), &i2p_address));
                if !collect_lane_wanted(
                    external,
                    this.part_seen.load(Relaxed),
                    now_ms(),
                    this.collect_lanes.load(Relaxed),
                ) {
                    continue;
                }
                // One dial at a time; the next tick dials the next.
                let dial = tokio::time::timeout(
                    PEER_RELAY_CONNECT_TIMEOUT,
                    relay::connect(&this.node, &i2p_address, &this.identity),
                )
                .await;
                let Ok(Ok(client)) = dial else { continue };
                let n = this.collect_lanes.fetch_add(1, Relaxed) + 1;
                this.file_metrics.receiving_lanes(n + 1);
                crate::debug_trace!(
                    "[files] another connection to our relay for parts ({n} extra)"
                );
                let lane = this.clone();
                // Tracked like every task of ours: one left running after
                // shutdown kept reading mail on the old instance, with its own
                // copy of the ratchet, and wrote it over the new one's (e2e
                // run 36056788336, the recipient restarting midway).
                let handle = tokio::spawn(async move {
                    // Ends when parts stop coming or our relay changes; what it was
                    // given and did not ack the relay deals to the others.
                    let quiet = {
                        let lane = lane.clone();
                        let i2p_address = i2p_address.clone();
                        move || {
                            !collect_lane_holds(lane.part_seen.load(Relaxed), now_ms())
                                || !relay::same_destination(&lane.relay_address(), &i2p_address)
                        }
                    };
                    lane.clone()
                        .run_recv_loop(client, Some(i2p_address.clone()), Some(&quiet))
                        .await;
                    lane.collect_lanes.fetch_sub(1, Relaxed);
                });
                this.track(handle);
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    /// Seal outgoing attachments: small ones whole (they ride inside the
    /// letter), larger ones in parts. Publish only after every file is ready.
    pub(super) fn store_outgoing(
        &self,
        attachments: &[(String, Vec<u8>)],
    ) -> Result<crate::files::StagedAttachments> {
        let mut staged =
            crate::files::StagedAttachments::new(&self.data_dir.join(ATTACHMENTS_DIR))?;
        for (name, data) in attachments {
            if !staged.can_inline(data.len()) {
                staged.parts(name, data.as_slice())?;
            } else {
                staged.inline(name, data)?;
            }
        }
        Ok(staged)
    }

    pub(super) fn offers_for(&self, msg_id: i64) -> Result<Vec<WireFileOffer>> {
        Ok(self
            .db
            .files_out_for_message(msg_id)?
            .into_iter()
            .filter(|f| !f.cancelled)
            .map(|f| WireFileOffer {
                file_id: f.file_id,
                name: f.attachment.name,
                size: f.attachment.size as u64,
                sha256: f.sha256,
                chunk_size: f
                    .attachment
                    .chunk_size
                    .unwrap_or(crate::files::CHUNK_SIZE as i64) as u32,
            })
            .collect())
    }

    /// A letter offering files: start taking their parts.
    pub(super) async fn activate_message_files(
        self: &Arc<Self>,
        contact_id: i64,
        message_id: i64,
    ) -> Result<()> {
        if !self.db.can_receive_files(contact_id)? {
            return Ok(());
        }
        for fin in self.db.files_in_for_message(message_id)? {
            if fin.contact_id != contact_id {
                continue;
            }
            let total = crate::files::chunk_count(fin.size, fin.chunk_size);
            self.db.if_message_live(message_id, || {
                let _ = self.events.try_send(SessionEvent::FileProgress {
                    message_id,
                    contact_id,
                    incoming: true,
                    done: fin.received,
                    total,
                });
            })?;
            for (index, data) in self.db.file_early_take(&fin.file_id, contact_id)? {
                let chunk = WireFileChunk {
                    file_id: fin.file_id,
                    index,
                    data,
                };
                self.on_file_chunk(contact_id, &chunk, 0, None).await?;
            }
            if let Some(current) = self.db.file_in(&fin.file_id, contact_id)? {
                let ack =
                    crate::files::Received::from_bits(current.bits, total).resume_ack(fin.file_id);
                self.send_file_message(contact_id, |p| p.file_ack = Some(ack))
                    .await;
            }
        }
        Ok(())
    }

    /// `sent_at` is the part's letter's, echoed in the ack (0 for a part set
    /// aside before its offer: no round trip to measure on it).
    pub(super) async fn on_file_chunk(
        self: &Arc<Self>,
        contact_id: i64,
        chunk: &WireFileChunk,
        sent_at: i64,
        reference: Option<&crate::db::DmReference>,
    ) -> Result<()> {
        let before = self
            .part_seen
            .swap(now_ms(), std::sync::atomic::Ordering::Relaxed);
        if !collect_lane_holds(before, now_ms()) {
            self.collect_kick.notify_one();
        }
        let Some(update) = self.db.apply_file_chunk(
            contact_id,
            chunk,
            reference,
            &self.data_dir.join(ATTACHMENTS_DIR),
        )?
        else {
            return Ok(());
        };
        if update.broken {
            self.db
                .cleanup_deleted_files(&self.data_dir.join(ATTACHMENTS_DIR))?;
            self.send_file_message(contact_id, |payload| {
                payload.file_cancel = Some(chunk.file_id)
            })
            .await;
            self.db.if_message_live(update.message_id, || {
                let _ = self.events.try_send(SessionEvent::FileFailed {
                    message_id: update.message_id,
                    contact_id,
                    reason: "came broken".into(),
                });
            })?;
            return Ok(());
        }
        self.file_metrics.received(update.fresh, chunk.data.len());
        if let Some(attachment_id) = update.attachment_id {
            self.db.if_message_live(update.message_id, || {
                let _ = self.events.try_send(SessionEvent::FileReceived {
                    message_id: update.message_id,
                    attachment_id,
                });
            })?;
        } else if update.fresh {
            self.db.if_message_live(update.message_id, || {
                let _ = self.events.try_send(SessionEvent::FileProgress {
                    message_id: update.message_id,
                    contact_id,
                    incoming: true,
                    done: update.received.count(),
                    total: update.total,
                });
            })?;
        }
        if !update.fresh
            || update.received.complete()
            || !update.received.missing().is_empty()
            || update.received.count() % crate::files::ACK_EVERY == 0
        {
            self.send_file_message(contact_id, |p| {
                p.file_ack = Some(WireFileAck {
                    echo_index: chunk.index,
                    echo_sent_at: sent_at,
                    ..update.received.ack(chunk.file_id)
                })
            })
            .await;
        }
        Ok(())
    }

    pub(super) async fn on_file_ack(
        self: &Arc<Self>,
        contact_id: i64,
        ack: &WireFileAck,
    ) -> Result<()> {
        let Some(file) = self.db.file_out(&ack.file_id)? else {
            return Ok(());
        };
        let total = crate::files::chunk_count(
            file.attachment.size as u64,
            file.attachment.chunk_size.unwrap_or(1) as u32,
        );
        let done = {
            let mut flows = self.flows.lock().unwrap_or_else(|p| p.into_inner());
            self.db.acknowledge_file_parts(
                contact_id,
                ack,
                total,
                flows.entry(contact_id).or_default(),
                now_ms(),
            )?
        };
        if let Some(done) = done {
            let _ = self.events.try_send(SessionEvent::FileProgress {
                message_id: file.message_id,
                contact_id,
                incoming: false,
                done,
                total,
            });
            self.send_kick.notify_one();
        }
        Ok(())
    }

    pub(super) async fn on_file_cancel(&self, contact_id: i64, file_id: &[u8; 16]) -> Result<()> {
        // They stopped sending it to us: forget what we had.
        if let Some(message_id) = self.db.cancel_incoming_file(file_id, contact_id)? {
            self.db
                .cleanup_deleted_files(&self.data_dir.join(ATTACHMENTS_DIR))?;
            let _ = self.events.try_send(SessionEvent::FileFailed {
                message_id,
                contact_id,
                reason: "cancelled".into(),
            });
        }
        if self.db.fail_file_peer(file_id, contact_id)? {
            if let Some(flow) = self
                .flows
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get_mut(&contact_id)
            {
                flow.forget(*file_id);
            }
            if let Some(file) = self.db.file_out(file_id)? {
                let _ = self.events.try_send(SessionEvent::FileFailed {
                    message_id: file.message_id,
                    contact_id,
                    reason: "cancelled".into(),
                });
            }
        }
        Ok(())
    }

    /// An attachment's bytes, whichever way it was sealed.
    pub fn read_attachment(&self, att: &crate::db::Attachment) -> Result<Vec<u8>> {
        let path = self.data_dir.join(ATTACHMENTS_DIR).join(&att.path);
        Ok(crate::files::read_attachment(
            &path,
            to_arr32(att.key.clone())?,
            att.size as u64,
            att.chunk_size,
        )?)
    }

    /// Stop sending a file of ours, to everyone, and tell them.
    pub async fn cancel_file(self: &Arc<Self>, file_id: [u8; 16]) -> Result<()> {
        self.db.file_out_cancel(&file_id)?;
        for peer in self.db.file_peers_of(&file_id)? {
            if let Some(flow) = self
                .flows
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get_mut(&peer.contact_id)
            {
                flow.forget(file_id);
            }
            self.send_file_message(peer.contact_id, |p| p.file_cancel = Some(file_id))
                .await;
        }
        Ok(())
    }

    /// A letter about files (an ack, a cancel): no text, not a message, not
    /// retried — the next part or ack supersedes it.
    pub(super) async fn send_file_message(
        self: &Arc<Self>,
        contact_id: i64,
        fill: impl FnOnce(&mut WirePayload),
    ) -> bool {
        if !self.db.can_receive_files(contact_id).unwrap_or(false) {
            return false;
        }
        let Ok(Some(contact)) = self.db.get_contact(contact_id) else {
            return false;
        };
        let mut payload = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
        fill(&mut payload);
        let Some(route) = self.route_for(&contact).await else {
            return false;
        };
        if self.ensure_session_for(&contact, &route).await.is_err() {
            return false;
        }
        self.send_payload_via_relay(&contact, &mut payload, &route)
            .await
            .is_ok()
    }

    /// Connections to carry parts to this contact's relay: the usual one and
    /// up to `files::lanes::limit() - 1` more, dialled in the background as needed.
    pub(super) async fn file_lanes(
        self: &Arc<Self>,
        contact: &Contact,
        main: &mpsc::Sender<ClientToRelay>,
        want: usize,
    ) -> Vec<mpsc::Sender<ClientToRelay>> {
        let mut out = vec![main.clone()];
        let Some(relay) = contact
            .relay_address
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
        else {
            return out;
        };
        let relay = relay.to_string();
        let (live, dial) = {
            let mut lanes = self.file_lanes.lock().await;
            let v = lanes.entry(relay.clone()).or_default();
            v.retain(|tx| !tx.is_closed());
            // Count and reserve together: a completed dial cannot make this
            // snapshot stale before we claim the next one.
            let dial = v.len() + 1 < want.min(crate::files::lanes::limit())
                && self
                    .file_lanes_dialling
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(relay.clone());
            (v.clone(), dial)
        };
        out.extend(live.into_iter().take(want.saturating_sub(1)));
        if dial {
            let this = self.clone();
            let handle = tokio::spawn(async move {
                let dial = tokio::time::timeout(
                    PEER_RELAY_CONNECT_TIMEOUT,
                    crate::relay::connect_peer(&this.node, &relay, &this.identity),
                )
                .await;
                if let Ok(Ok(client)) = dial {
                    crate::debug_trace!(
                        "[files] another lane to relay {}",
                        &relay[..16.min(relay.len())]
                    );
                    this.file_lanes
                        .lock()
                        .await
                        .entry(relay.clone())
                        .or_default()
                        .push(client.out_tx.clone());
                    this.file_lanes_dialling
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .remove(&relay);
                    this.send_kick.notify_one();
                    // Drained like any peer relay connection; it ends with the lane.
                    this.clone().run_recv_loop(client, None, None).await;
                } else {
                    this.file_lanes_dialling
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .remove(&relay);
                }
            });
            self.tasks.lock().unwrap().push(handle);
        }
        out
    }

    /// Send what is due of every file still going to this contact.
    pub(super) async fn send_file_parts(
        self: &Arc<Self>,
        contact: &Contact,
        route: &Route,
    ) -> Result<()> {
        let now = now_ms();
        let Route::Relay(main) = route else {
            return Ok(());
        };
        let window = self
            .flows
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(contact.id)
            .or_default()
            .window();
        let lanes = self.file_lanes(contact, main, lanes_for(window)).await;
        let heard = self
            .heard
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&contact.id)
            .copied();
        for peer in self.db.file_peers_for(contact.id)? {
            let Some(file) = self.db.file_out(&peer.file_id)? else {
                continue;
            };
            let a = &file.attachment;
            let chunk_size = a.chunk_size.unwrap_or(crate::files::CHUNK_SIZE as i64) as u32;
            let total = crate::files::chunk_count(a.size as u64, chunk_size);
            let (plan, window, in_flight, rto, srtt) = {
                let mut flows = self.flows.lock().unwrap_or_else(|p| p.into_inner());
                let flow = flows.entry(contact.id).or_default();
                let plan = self.db.reserve_file_batch(
                    &file.file_id,
                    contact.id,
                    total,
                    flow,
                    now,
                    heard,
                )?;
                (
                    plan,
                    flow.window(),
                    flow.in_flight(),
                    flow.rto_ms(),
                    flow.srtt_ms(),
                )
            };
            let Some(plan) = plan else { continue };
            if plan.gave_up {
                let _ = self.events.try_send(SessionEvent::FileFailed {
                    message_id: file.message_id,
                    contact_id: contact.id,
                    reason: "not taken".into(),
                });
                continue;
            }
            let due = plan.due;
            let next_before = plan.next_before;
            if due.is_empty() {
                continue;
            }
            // One line a part (192 KiB): what went, and why it was its turn.
            let again: Vec<u32> = due.iter().copied().filter(|i| *i < next_before).collect();
            crate::debug_trace!("[files] {} to contact {}: parts {:?} (again {:?}), window {window}, lanes {} (incl main), in flight {in_flight}, timeout {rto} ms, round trip {srtt:?} ms",
                a.name, contact.id, due, again, lanes.len());
            let path = self.data_dir.join(ATTACHMENTS_DIR).join(&a.path);
            let cipher = AttachmentCipher::from_key(to_arr32(a.key.clone())?);
            for index in &due {
                let data =
                    crate::files::read_part(&path, &cipher, a.size as u64, chunk_size, *index)?;
                let mut payload = WirePayload::simple(0, String::new(), Vec::new(), now, None);
                payload.file_chunk = Some(WireFileChunk {
                    file_id: file.file_id,
                    index: *index,
                    data,
                });
                if let Ok(reference) = self.db.dm_reference(file.message_id) {
                    payload.dm = Some(WireDm { kind: 0, reference });
                }
                // Round the lanes: each is its own i2p stream.
                let lane = self
                    .flows
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .entry(contact.id)
                    .or_default()
                    .next_lane(lanes.len());
                let via = Route::Relay(lanes[lane].clone());
                if let Err(e) = self
                    .send_payload_via_relay(contact, &mut payload, &via)
                    .await
                {
                    crate::debug_trace!(
                        "[files] part {index} of {} to contact {}: {e:?}",
                        a.name,
                        contact.id
                    );
                    // What did not go goes again next time.
                    let mut flows = self.flows.lock().unwrap_or_else(|p| p.into_inner());
                    let flow = flows.entry(contact.id).or_default();
                    let retry: Vec<u32> = due.iter().copied().filter(|i| i >= index).collect();
                    self.db
                        .retry_file_parts(&file.file_id, contact.id, &retry, flow)?;
                    break;
                }
                self.file_metrics.queued(*index < next_before, lanes.len());
            }
        }
        Ok(())
    }

    /// Unfinished transfers after the relay would have dropped their parts.
    pub(super) fn gc_files(&self) {
        let cutoff = now_ms() - FILE_IN_TTL_MS;
        if let Ok(stale) = self.db.files_in_stale(cutoff) {
            for f in stale {
                let _ = std::fs::remove_file(self.data_dir.join(ATTACHMENTS_DIR).join(&f.path));
                let _ = self.db.file_in_delete(&f.file_id, f.contact_id);
            }
        }
        let _ = self.db.file_early_gc(cutoff);
    }
}
