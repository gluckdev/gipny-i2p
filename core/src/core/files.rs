use super::*;

impl Core {
    pub(super) fn store_outgoing(
        &self,
        attachments: &[PendingAttachment],
    ) -> Result<gipny_libcore::files::StagedAttachments> {
        let mut staged =
            gipny_libcore::files::StagedAttachments::new(&self.data_dir.join(ATTACHMENTS_DIR))?;
        for attachment in attachments {
            if let Some(source) = &attachment.from {
                staged.parts(
                    &attachment.name,
                    std::io::BufReader::new(std::fs::File::open(source)?),
                )?;
            } else if !staged.can_inline(attachment.data.len()) {
                staged.parts(&attachment.name, attachment.data.as_slice())?;
            } else {
                staged.inline(&attachment.name, &attachment.data)?;
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
                    .unwrap_or(gipny_libcore::files::CHUNK_SIZE as i64)
                    as u32,
            })
            .collect())
    }

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
            let total = gipny_libcore::files::chunk_count(fin.size, fin.chunk_size);
            self.db.if_message_live(message_id, || {
                let _ = self.events.try_send(CoreEvent::FileProgress {
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
                let ack = gipny_libcore::files::Received::from_bits(current.bits, total)
                    .resume_ack(fin.file_id);
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
        reference: Option<&gipny_libcore::db::DmReference>,
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
                let _ = self.events.try_send(CoreEvent::FileFailed {
                    message_id: update.message_id,
                    contact_id,
                    reason: "came broken".into(),
                });
            })?;
            return Ok(());
        }
        if let Some(attachment_id) = update.attachment_id {
            self.db.if_message_live(update.message_id, || {
                let _ = self.events.try_send(CoreEvent::FileReceived {
                    message_id: update.message_id,
                    attachment_id,
                });
            })?;
        } else if update.fresh {
            self.db.if_message_live(update.message_id, || {
                let _ = self.events.try_send(CoreEvent::FileProgress {
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
            || update.received.count() % gipny_libcore::files::ACK_EVERY == 0
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
        let total = gipny_libcore::files::chunk_count(
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
            let _ = self.events.try_send(CoreEvent::FileProgress {
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
        if let Some(message_id) = self.db.cancel_incoming_file(file_id, contact_id)? {
            self.db
                .cleanup_deleted_files(&self.data_dir.join(ATTACHMENTS_DIR))?;
            let _ = self.events.try_send(CoreEvent::FileFailed {
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
                let _ = self.events.try_send(CoreEvent::FileFailed {
                    message_id: file.message_id,
                    contact_id,
                    reason: "cancelled".into(),
                });
            }
        }
        Ok(())
    }

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

    /// The usual connection to this contact's relay and up to
    /// `files::lanes::limit() - 1` more, dialled in the background as needed: parts
    /// spread over several i2p streams go that many times faster.
    pub(super) async fn relay_lanes(
        self: &Arc<Self>,
        contact: &gipny_libcore::db::Contact,
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
            let dial = v.len() + 1 < want.min(gipny_libcore::files::lanes::limit())
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
                    relay::connect_peer(&this.node, &relay, &this.identity),
                )
                .await;
                if let Ok(Ok(client)) = dial {
                    gipny_libcore::debug_trace!(
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

    pub(super) async fn send_file_parts(
        self: &Arc<Self>,
        contact: &gipny_libcore::db::Contact,
        route: &Route,
    ) -> Result<()> {
        use gipny_libcore::files;
        let now = now_ms();
        let Route::Relay(main) = route else {
            return Ok(());
        };
        let wanted = self
            .flows
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(contact.id)
            .or_default()
            .desired_lanes();
        let lanes = self.relay_lanes(contact, main, wanted).await;
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
            let chunk_size = a.chunk_size.unwrap_or(files::CHUNK_SIZE as i64) as u32;
            let total = files::chunk_count(a.size as u64, chunk_size);
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
                let _ = self.events.try_send(CoreEvent::FileFailed {
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
            gipny_libcore::debug_trace!("[files] {} to contact {}: parts {:?} (again {:?}), window {window}, lanes {} (incl main), in flight {in_flight}, timeout {rto} ms, round trip {srtt:?} ms",
                a.name, contact.id, due, again, lanes.len());
            let path = self.data_dir.join(ATTACHMENTS_DIR).join(&a.path);
            let cipher = AttachmentCipher::from_key(to_arr32(a.key.clone())?);
            for index in &due {
                let data = files::read_part(&path, &cipher, a.size as u64, chunk_size, *index)?;
                let mut payload = WirePayload::simple(0, String::new(), Vec::new(), now, None);
                payload.file_chunk = Some(WireFileChunk {
                    file_id: file.file_id,
                    index: *index,
                    data,
                });
                if let Ok(reference) = self.db.dm_reference(file.message_id) {
                    payload.dm = Some(gipny_libcore::session::WireDm { kind: 0, reference });
                }
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
                    gipny_libcore::debug_trace!(
                        "[files] part {index} of {} to contact {}: {e:?}",
                        a.name,
                        contact.id
                    );
                    let mut flows = self.flows.lock().unwrap_or_else(|p| p.into_inner());
                    let flow = flows.entry(contact.id).or_default();
                    let retry: Vec<u32> = due.iter().copied().filter(|i| i >= index).collect();
                    self.db
                        .retry_file_parts(&file.file_id, contact.id, &retry, flow)?;
                    break;
                }
            }
        }
        Ok(())
    }

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

pub struct PendingAttachment {
    pub name: String,
    pub data: Vec<u8>,
    /// Sent from this file instead of `data` (empty then): sealed in parts
    /// straight from disk, never whole in memory.
    pub from: Option<std::path::PathBuf>,
}

impl Core {
    /// Write an attachment out without holding it whole: part by part when
    /// it was sealed in parts.
    pub fn write_attachment_to(
        &self,
        att: &Attachment,
        out: &mut impl std::io::Write,
    ) -> Result<()> {
        match att.chunk_size {
            Some(cs) => {
                let full = self.data_dir.join(ATTACHMENTS_DIR).join(&att.path);
                let cipher = AttachmentCipher::from_key(to_arr32(att.key.clone())?);
                gipny_libcore::files::open_to(&full, &cipher, att.size as u64, cs as u32, out)?;
            }
            None => out.write_all(&self.read_attachment(att)?)?,
        }
        Ok(())
    }

    pub fn read_attachment(&self, att: &Attachment) -> Result<Vec<u8>> {
        let key = to_arr32(att.key.clone())?;
        let full = self.data_dir.join(ATTACHMENTS_DIR).join(&att.path);
        Ok(gipny_libcore::files::read_attachment(
            &full,
            key,
            att.size as u64,
            att.chunk_size,
        )?)
    }

    /// Stop sending the files of a message, to everyone, and tell them.
    pub async fn cancel_files(self: &Arc<Self>, message_id: i64) -> Result<()> {
        for f in self.db.files_out_for_message(message_id)? {
            self.db.file_out_cancel(&f.file_id)?;
            for peer in self.db.file_peers_of(&f.file_id)? {
                if let Some(flow) = self
                    .flows
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .get_mut(&peer.contact_id)
                {
                    flow.forget(f.file_id);
                }
                let id = f.file_id;
                self.send_file_message(peer.contact_id, move |p| p.file_cancel = Some(id))
                    .await;
            }
        }
        Ok(())
    }

    /// Where the files of a message stand, for the chat: (name, size, done,
    /// total, incoming) of each still moving.
    pub fn files_progress(
        &self,
        message_id: i64,
    ) -> Result<Vec<(String, u64, u32, u32, bool, String)>> {
        use gipny_libcore::files;
        let mut out = Vec::new();
        for f in self.db.files_in_for_message(message_id)? {
            out.push((
                f.name,
                f.size,
                f.received,
                files::chunk_count(f.size, f.chunk_size),
                true,
                "receiving".into(),
            ));
        }
        for f in self.db.files_out_for_message(message_id)? {
            let a = &f.attachment;
            let total = files::chunk_count(a.size as u64, a.chunk_size.unwrap_or(1) as u32);
            let peers = self.db.file_peers_of(&f.file_id)?;
            let done = peers.iter().map(|p| p.acked).min().unwrap_or(total);
            let status = if f.cancelled {
                "cancelled"
            } else if peers.iter().any(|peer| peer.failed) {
                "failed"
            } else if done >= total {
                "complete"
            } else {
                "sending"
            };
            out.push((
                a.name.clone(),
                a.size as u64,
                done,
                total,
                false,
                status.into(),
            ));
        }
        for (name, size, status) in self.db.failed_files_for_message(message_id)? {
            out.push((name, size, 0, 1, true, status));
        }
        Ok(out)
    }
}
