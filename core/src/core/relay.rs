use super::*;

impl Core {
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
                    let total = gipny_libcore::files::chunk_count(fin.size as u64, fin.chunk_size);
                    let ack = gipny_libcore::files::Received::from_bits(fin.bits.clone(), total)
                        .resume_ack(fin.file_id);
                    if this
                        .send_file_message(fin.contact_id, |p| p.file_ack = Some(ack))
                        .await
                    {
                        gipny_libcore::debug_trace!(
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
    /// runs elsewhere. Mirrors libcore's session.rs `spawn_collect_lanes`.
    pub(super) fn spawn_collect_lanes(self: Arc<Self>) {
        use std::sync::atomic::Ordering::Relaxed;
        let this = self.clone();
        let handle = tokio::spawn(async move {
            loop {
                let _ = tokio::time::timeout(Duration::from_secs(5), this.collect_kick.notified())
                    .await;
                let onion = this.relay_onion();
                let external = !onion.is_empty()
                    && this.relay_out.read().await.is_some()
                    && !this
                        .hosted_relay
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .as_ref()
                        .is_some_and(|r| relay::same_destination(r.address(), &onion));
                if !collect_lane_wanted(
                    external,
                    this.part_seen.load(Relaxed),
                    now_ms(),
                    this.collect_lanes.load(Relaxed),
                ) {
                    continue;
                }
                let dial = tokio::time::timeout(
                    PEER_RELAY_CONNECT_TIMEOUT,
                    relay::connect(&this.node, &onion, &this.identity),
                )
                .await;
                let Ok(Ok(client)) = dial else { continue };
                let n = this.collect_lanes.fetch_add(1, Relaxed) + 1;
                gipny_libcore::debug_trace!(
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
                        let onion = onion.clone();
                        move || {
                            !collect_lane_holds(lane.part_seen.load(Relaxed), now_ms())
                                || !relay::same_destination(&lane.relay_onion(), &onion)
                        }
                    };
                    lane.clone()
                        .run_recv_loop(client, Some(onion.clone()), Some(&quiet))
                        .await;
                    lane.collect_lanes.fetch_sub(1, Relaxed);
                });
                this.track(handle);
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    pub(super) fn spawn_relay_loop(self: Arc<Self>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            let mut backoff = RECONNECT_INITIAL_MS;
            loop {
                let onion = this.relay_onion();
                if onion.is_empty() {
                    // No relay address yet: the built-in relay has not published one,
                    // or external mode has none saved in Settings (SETTING_RELAY_ONION).
                    // Look again soon; the backoff is for
                    // failed dials, and growing it here made the first real one
                    // start late and its retry wait the full 15 s (e2e run
                    // 36042483601: 20 s from the relay being set to connected).
                    tokio::time::sleep(Duration::from_millis(RECONNECT_INITIAL_MS)).await;
                    continue;
                }
                gipny_libcore::debug_trace!(
                    "[relay-client] connecting to {}",
                    &onion[..16.min(onion.len())]
                );
                // Our own built-in relay is in this process: talk to it over a
                // pipe, not out through i2p and back in.
                let local = this
                    .hosted_relay
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                    .filter(|r| relay::same_destination(r.address(), &onion))
                    .map(|r| r.connect_local());
                // Bounded: nothing below has a timeout of its own, and a dial
                // i2p leaves hanging would otherwise keep us off our own mail
                // until restart (e2e run 35963073832: the master never got the
                // agent's GRANT).
                let connected = match tokio::time::timeout(PEER_RELAY_CONNECT_TIMEOUT, async {
                    match local {
                        Some(stream) => relay::connect_local(stream, &onion, &this.identity).await,
                        None => relay::connect(&this.node, &onion, &this.identity).await,
                    }
                })
                .await
                {
                    Ok(r) => r,
                    Err(_) => Err(relay::RelayError::Proto(format!(
                        "no answer in {PEER_RELAY_CONNECT_TIMEOUT:?}"
                    ))),
                };
                match connected {
                    Ok(client) => {
                        gipny_libcore::debug_trace!("[relay-client] connected & authed");
                        this.note_relay_dial(None);
                        backoff = RECONNECT_INITIAL_MS;
                        *this.relay_out.write().await = Some(client.out_tx.clone());
                        let _ = this.events.try_send(CoreEvent::RelayConnected);

                        if let Ok(bundle) = this.my_bundle() {
                            if let Ok(bytes) = bincode::serialize(&bundle) {
                                let _ = client
                                    .out_tx
                                    .send(ClientToRelay::Publish { bundle: bytes })
                                    .await;
                            }
                        }
                        this.send_kick.notify_one();
                        this.clone()
                            .run_recv_loop(client, Some(onion.clone()), None)
                            .await;
                        *this.relay_out.write().await = None;
                        let _ = this.events.try_send(CoreEvent::RelayDisconnected);
                    }
                    Err(e) => {
                        gipny_libcore::debug_trace!("[relay-client] connect fail: {:?}", e);
                        this.note_relay_dial(Some(format!("{e:?}")));
                    }
                }
                tokio::time::sleep(Duration::from_millis(backoff)).await;
                backoff = (backoff * 2).min(RECONNECT_MAX_MS);
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    /// Where a letter goes out: straight to the contact's relay, or into the
    /// relay network when that relay is not answering. The network path is
    /// slower and needs proof of work, so it is the fallback, never the
    /// default.
    pub(super) async fn route_for(
        self: &Arc<Self>,
        contact: &gipny_libcore::db::Contact,
    ) -> Option<Route> {
        if let Some(tx) = self.relay_for(contact).await {
            return Some(Route::Relay(tx));
        }
        if self.peer_relay_settling(contact).await {
            return None;
        }
        // Their relay is down or unknown. The network holds the letter until
        // they come back, but only if we are in it.
        (self.dht.peer_count() > 0).then_some(Route::Dht)
    }

    /// Their relay is being dialled (or its old connection is still closing):
    /// the letter waits in the queue for it rather than going to the network.
    /// Otherwise the first letters of a burst would take the slow network
    /// path while the later ones overtake them on the relay, and an agent
    /// runs commands in the order they arrive.
    pub(super) async fn peer_relay_settling(&self, contact: &gipny_libcore::db::Contact) -> bool {
        let Some(theirs) = contact
            .relay_address
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
        else {
            return false;
        };
        matches!(
            self.peer_relays.lock().await.get(theirs),
            Some(PeerRelay::Connecting) | Some(PeerRelay::Ready(_))
        )
    }

    /// Hand one envelope to the contact by `route`.
    pub(super) async fn deliver(
        &self,
        contact: &gipny_libcore::db::Contact,
        blob: Vec<u8>,
        route: &Route,
        first_letter: bool,
    ) -> Result<()> {
        let (their_sign, their_dh) = (
            to_arr32(contact.identity_sign.clone())?,
            to_arr32(contact.identity_dh.clone())?,
        );
        match route {
            Route::Relay(out) => out
                .send(ClientToRelay::Send {
                    to: their_sign,
                    blob,
                })
                .await
                .map_err(|_| CoreError::State),
            Route::Dht => {
                // A first letter goes to the box anyone holding their card can
                // find (they may not know us yet); everything else to the one
                // only the two of us can compute.
                let ok = if first_letter {
                    dht_client::put_intro(&self.dht, &their_sign, &their_dh, &blob).await
                } else {
                    dht_client::put_mail(&self.dht, &self.identity, &their_sign, &their_dh, &blob)
                        .await
                };
                if ok {
                    gipny_libcore::debug_trace!(
                        "[dht] letter for contact {} left in the network",
                        contact.id
                    );
                    Ok(())
                } else {
                    Err(CoreError::State)
                }
            }
        }
    }

    /// The connection to deposit this contact's mail on.
    ///
    /// Their card names where they collect; ours is only the fallback for
    /// contacts added before cards carried a relay. Connections to other
    /// people's relays are opened on demand and kept for reuse — a contact is
    /// written to repeatedly, and i2p charges tunnel setup for every new
    /// destination.
    ///
    /// Returns a boxed future on purpose: this calls into the spawned receive
    /// loop, which handles a frame, which may send an ack, which comes back
    /// here. `async fn` would make that an infinitely recursive future type and
    /// the compiler cannot prove it `Send`.
    pub(super) fn relay_for<'a>(
        self: &'a Arc<Self>,
        contact: &'a gipny_libcore::db::Contact,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Option<mpsc::Sender<ClientToRelay>>> + Send + 'a>,
    > {
        Box::pin(async move {
            let theirs = contact
                .relay_address
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty());
            let Some(theirs) = theirs else {
                return self.relay_out.read().await.clone();
            };
            if relay::same_destination(theirs, &self.relay_onion()) {
                return self.relay_out.read().await.clone();
            }

            // Never connect on this path. It runs inside the send loop, which walks
            // contacts one at a time, and opening an i2p destination means building
            // tunnels — seconds when it works and unbounded when the relay is gone,
            // since nothing in the dial path has a timeout. Blocking here would stall
            // delivery to *every other contact* behind one unreachable relay.
            //
            // So: hand back a connection if we have one, otherwise start one in the
            // background and skip this contact for now. The send loop comes round
            // every few seconds and the message is still queued.
            let failures = {
                let mut pool = self.peer_relays.lock().await;
                match pool.get(theirs) {
                    Some(PeerRelay::Ready(tx)) if !tx.is_closed() => return Some(tx.clone()),
                    // A dead sender means the recv loop is on its way out; let it
                    // finish cleaning up rather than racing a second connection.
                    Some(PeerRelay::Ready(_)) | Some(PeerRelay::Connecting) => return None,
                    Some(PeerRelay::Failed { until, .. }) if Instant::now() < *until => {
                        return None
                    }
                    _ => {}
                }
                let failures = match pool.get(theirs) {
                    Some(PeerRelay::Failed { failures, .. }) => *failures,
                    _ => 0,
                };
                pool.insert(theirs.to_string(), PeerRelay::Connecting);
                failures
            };

            let (contact_id, contact_pk) = (
                contact.id,
                <[u8; 32]>::try_from(contact.identity_sign.as_slice()).ok(),
            );
            let this = self.clone();
            let key = theirs.to_string();
            let handle = tokio::spawn(async move {
                let short = &key[..16.min(key.len())];
                let dial = tokio::time::timeout(
                    PEER_RELAY_CONNECT_TIMEOUT,
                    relay::connect_peer(&this.node, &key, &this.identity),
                )
                .await;
                let client = match dial {
                    Ok(Ok(c)) => c,
                    other => {
                        match other {
                            Err(_) => gipny_libcore::debug_trace!(
                                "[relay-client] peer relay {short} did not answer in {:?}",
                                PEER_RELAY_CONNECT_TIMEOUT
                            ),
                            Ok(Err(e)) => {
                                gipny_libcore::debug_trace!(
                                    "[relay-client] peer relay {short} unreachable: {e:?}"
                                )
                            }
                            Ok(Ok(_)) => unreachable!(),
                        }
                        // Back off rather than redialing on every send tick: a relay
                        // that is down stays down for a while, and each attempt costs
                        // tunnel building.
                        this.peer_relays.lock().await.insert(
                            key.clone(),
                            PeerRelay::Failed {
                                until: Instant::now() + peer_relay_backoff(failures),
                                failures: failures + 1,
                            },
                        );
                        // Dialled again when the wait is over, letter or not; see
                        // libcore's `peer_relay_redial_after`.
                        if let Some(wait) = peer_relay_redial_after(failures) {
                            let again = this.clone();
                            let handle = tokio::spawn(async move {
                                tokio::time::sleep(wait).await;
                                if let Ok(Some(c)) = again.db.get_contact(contact_id) {
                                    let _ = again.relay_for(&c).await;
                                }
                            });
                            this.track(handle);
                        }
                        return;
                    }
                };
                gipny_libcore::debug_trace!("[relay-client] connected to peer relay {short}");
                this.peer_relays
                    .lock()
                    .await
                    .insert(key.clone(), PeerRelay::Ready(client.out_tx.clone()));
                // No session with them yet: ask for their bundle now, so a first
                // letter does not wait a round trip for it.
                let no_session = !this.sessions.lock().await.contains_key(&contact_id)
                    && !this.db.has_session(contact_id).unwrap_or(false);
                if let (true, Some(pk)) = (no_session, contact_pk) {
                    let _ = client.out_tx.send(ClientToRelay::GetBundle { pk }).await;
                }
                this.send_kick.notify_one();

                // Drain it like our own: a relay we deposit on may also be holding
                // mail for us, and the frame handling is identical. No
                // RelayConnected event — that state is about our own relay, and
                // flipping it here would tell the user the wrong thing.
                this.clone().run_recv_loop(client, None, None).await;
                this.peer_relays.lock().await.remove(&key);
                gipny_libcore::debug_trace!("[relay-client] peer relay {short} disconnected");
            });
            self.tasks.lock().unwrap().push(handle);
            None
        })
    }

    /// `own` is the address this connection was opened as *our* relay under.
    /// When the relay we collect from changes — another mode, another address
    /// in Settings — the connection ends at its next ping and the loop redials;
    /// before, a new address only took effect when the old relay went away.
    pub(super) async fn run_recv_loop(
        self: Arc<Self>,
        client: RelayClient,
        own: Option<String>,
        done: Option<&(dyn Fn() -> bool + Send + Sync)>,
    ) {
        let in_rx = client.in_rx.clone();
        let out_tx = client.out_tx.clone();
        let mut ping = tokio::time::interval(Duration::from_secs(PING_INTERVAL_SECS));
        ping.set_missed_tick_behavior(MissedTickBehavior::Skip);
        ping.tick().await;
        let dead_threshold = Duration::from_secs(DEAD_THRESHOLD_SECS);
        let mut last_activity = std::time::Instant::now();
        loop {
            tokio::select! {
                _ = ping.tick() => {
                    // Checked between frames, never in the middle of one: a letter
                    // half-handled when the connection went could have moved the
                    // ratchet on and not be saved.
                    if done.is_some_and(|d| d()) { break; }
                    if own.as_deref().is_some_and(|o| !relay::same_destination(o, &self.relay_onion())) {
                        gipny_libcore::debug_trace!("[relay-client] our relay changed, reconnecting");
                        break;
                    }
                    if last_activity.elapsed() > dead_threshold {
                        gipny_libcore::debug_trace!("[relay-client] no activity for {:?}, forcing reconnect", last_activity.elapsed());
                        break;
                    }
                    if out_tx.send(ClientToRelay::Ping).await.is_err() { break; }
                }
                frame = async { in_rx.lock().await.recv().await } => {
                    let Some(frame) = frame else { break; };
                    last_activity = std::time::Instant::now();
                    // Logged in with plain Auth after an AuthV2 attempt fell
                    // through: this relay would take our deposits and never
                    // hand us our mail. Reconnect, which tries AuthV2 again.
                    if own.is_some() && matches!(&frame, RelayToClient::Error(e) if e == relay::ERR_NEEDS_AUTH_V2) {
                        gipny_libcore::debug_trace!("[relay-client] our relay wants AuthV2, reconnecting");
                        break;
                    }
                    if let Err(e) = self.handle_relay_frame(frame, &out_tx).await {
                        gipny_libcore::debug_trace!("[relay-client] handle err: {:?}", e);
                    }
                }
            }
        }
    }

    async fn handle_relay_frame(
        self: &Arc<Self>,
        frame: RelayToClient,
        out_tx: &mpsc::Sender<ClientToRelay>,
    ) -> Result<()> {
        match frame {
            RelayToClient::Incoming { id, from, blob } => {
                match self.handle_incoming_envelope(&from, &blob).await {
                    Ok(()) => {
                        let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                    }
                    Err(CoreError::Codec) => {
                        gipny_libcore::debug_trace!(
                            "[relay-client] codec err on msg {}, NOT acking (will retry)",
                            id
                        );
                    }
                    Err(CoreError::StaleOpk) => {
                        gipny_libcore::debug_trace!(
                            "[relay-client] stale-OPK X3dhInit on msg {}, ACK and skip (zombie)",
                            id
                        );
                        let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                    }
                    Err(CoreError::SealedDrop) => {
                        let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                    }
                    Err(CoreError::Crypto(_)) => {
                        if let Ok(Some(c)) = self.db.find_contact_by_sign_pk(&from) {
                            let fresh = {
                                let m = self.session_created_at.lock().await;
                                m.get(&c.id)
                                    .map(|t| now_ms() - *t < FRESH_SESSION_GRACE_MS)
                                    .unwrap_or(false)
                            };
                            if fresh {
                                gipny_libcore::debug_trace!("[relay-client] crypto err on msg {} within fresh-session grace, ACK and skip", id);
                                let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                            } else {
                                gipny_libcore::debug_trace!(
                                    "[relay-client] crypto err on msg {}, requesting resync, NOT acking",
                                    id
                                );
                                let _ = self.request_resync(&c).await;
                            }
                        } else {
                            gipny_libcore::debug_trace!(
                                "[relay-client] crypto err on msg {}, no contact, NOT acking",
                                id
                            );
                        }
                    }
                    // Nothing was committed (a letter once read is always
                    // acknowledged): the relay keeps it and offers it again.
                    Err(e) if is_transient(&e) => {
                        gipny_libcore::debug_trace!(
                            "[relay-client] storage err on msg {}: {:?}, NOT acking (will retry)",
                            id,
                            e
                        );
                    }
                    Err(e) => {
                        gipny_libcore::debug_trace!("[relay-client] incoming err: {:?}", e);
                        let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                    }
                }
            }
            RelayToClient::Bundle { pk, bundle } => {
                let waiters = self.bundle_waiters.lock().await.remove(&pk);
                match waiters {
                    Some(vec) => {
                        for tx in vec {
                            let _ = tx.send(bundle.clone());
                        }
                    }
                    // Nobody waiting: the one asked for ahead of use.
                    None => {
                        if let Some(b) = bundle {
                            self.bundle_cache
                                .lock()
                                .await
                                .insert(pk, (b, Instant::now()));
                        }
                    }
                }
            }
            RelayToClient::Error(reason) => {
                // Dropped silently before. The usual cause now: a deposit for a
                // contact whose card names a relay built into someone else's
                // app, which holds mail for its owner only.
                gipny_libcore::debug_trace!("[relay-client] relay error: {reason}");
            }
            _ => {}
        }
        Ok(())
    }
}

impl Core {
    /// Open the connections to the relays of the people written to most
    /// recently, at start, while our own relay is still building.
    ///
    /// A contact's relay is dialled on first use, and finding its LeaseSet and
    /// opening the stream costs 5–30 s — which the first letter after a start
    /// used to wait for, or, when the dial had not finished, the letter went
    /// the slow way through the relay network. They were dialled at start only
    /// once our relay was up, to announce its address: 20–40 s later. This
    /// learns nothing new about us — the announcement goes to every contact
    /// anyway. `relay_for` dials in the background and never blocks.
    pub(super) fn warm_peer_relays(self: Arc<Self>) {
        const WARM: usize = 5;
        let Ok(mut contacts) = self.db.list_contacts() else {
            return;
        };
        contacts.retain(|c| {
            c.trust != TrustLevel::Blocked
                && c.request_state != RequestState::Incoming
                && c.relay_address
                    .as_deref()
                    .is_some_and(|r| !r.trim().is_empty())
                && c.last_message_at.is_some()
        });
        contacts.sort_by_key(|c| std::cmp::Reverse(c.last_message_at));
        contacts.truncate(WARM);
        if contacts.is_empty() {
            return;
        }
        tokio::spawn(async move {
            for contact in &contacts {
                let _ = self.relay_for(contact).await;
            }
            gipny_libcore::debug_trace!(
                "[relay-client] dialling the relays of {} recent contact(s) ahead of use",
                contacts.len()
            );
        });
    }

    /// Dial the relay of one contact whose relay just became known (added,
    /// or a letter or the network named a new one), in the background, so
    /// their first letter does not wait 5–30 s for it. See warm_peer_relays.
    pub(super) fn warm_relay_of(self: &Arc<Self>, contact_id: i64) {
        let this = self.clone();
        tokio::spawn(async move {
            let Ok(Some(contact)) = this.db.get_contact(contact_id) else {
                return;
            };
            if contact.trust == TrustLevel::Blocked
                || contact.request_state == RequestState::Incoming
            {
                return;
            }
            let _ = this.relay_for(&contact).await;
        });
    }
}
