use super::*;

pub(super) fn terminal_addresses(contact: &gipny_libcore::db::Contact) -> Vec<&str> {
    let mut result = Vec::with_capacity(2);
    for address in [
        contact.relay_address.as_deref(),
        Some(contact.i2p_address.as_str()),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    {
        if gipny_libcore::card::is_valid_i2p_address(address)
            && !result
                .iter()
                .any(|previous: &&str| relay::same_destination(previous, address))
        {
            result.push(address);
        }
    }
    result
}

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

    pub(super) fn spawn_relay_loop(self: Arc<Self>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            let mut backoff = RECONNECT_INITIAL_MS;
            loop {
                let i2p_address = this.relay_address();
                // Our own built-in relay is in this process: talk to it over a
                // pipe, not out through i2p and back in.
                let local = this
                    .hosted_relay
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                    .filter(|r| relay::same_destination(r.address(), &i2p_address))
                    .map(|r| r.connect_local());
                // Bounded: nothing below has a timeout of its own, and a dial
                // i2p leaves hanging would otherwise keep us off our own mail
                // until restart (e2e run 35963073832: the master never got the
                // agent's GRANT).
                let Some(local) = local else {
                    tokio::time::sleep(Duration::from_millis(RECONNECT_INITIAL_MS)).await;
                    continue;
                };
                let connected = match tokio::time::timeout(
                    PEER_RELAY_CONNECT_TIMEOUT,
                    relay::connect_local(local, &i2p_address, &this.identity),
                )
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
                            .run_recv_loop(client, Some(i2p_address.clone()))
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

    pub(super) async fn route_for(
        self: &Arc<Self>,
        contact: &gipny_libcore::db::Contact,
    ) -> Option<Route> {
        self.relay_for(contact).await
    }

    /// Hand one envelope to the contact by `route`.
    pub(super) async fn deliver(
        &self,
        contact: &gipny_libcore::db::Contact,
        blob: Vec<u8>,
        route: &Route,
        remaining: Option<Duration>,
    ) -> Result<()> {
        let their_sign = to_arr32(contact.identity_sign.clone())?;
        route
            .deposit(their_sign, blob, remaining)
            .await
            .map_err(|_| CoreError::State)
    }

    /// The connection to deposit this contact's mail on.
    ///
    /// Returns a boxed future on purpose: this calls into the spawned receive
    /// loop, which handles a frame, which may send an ack, which comes back
    /// here. `async fn` would make that an infinitely recursive future type and
    /// the compiler cannot prove it `Send`.
    pub(super) fn relay_for<'a>(
        self: &'a Arc<Self>,
        contact: &'a gipny_libcore::db::Contact,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<relay::RelaySender>> + Send + 'a>>
    {
        Box::pin(async move {
            let owner: [u8; 32] = contact.identity_sign.as_slice().try_into().ok()?;
            for address in terminal_addresses(contact) {
                if let Some(tx) = self.relay_at(contact.id, owner, address).await {
                    return Some(tx);
                }
            }
            None
        })
    }

    async fn relay_at(
        self: &Arc<Self>,
        contact_id: i64,
        owner: [u8; 32],
        address: &str,
    ) -> Option<relay::RelaySender> {
        let key = (address.to_string(), owner);
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
            match pool.get(&key) {
                Some(PeerRelay::Ready(tx)) if !tx.is_closed() && tx.belongs_to(&owner) => {
                    return Some(tx.clone())
                }
                // A dead sender means the recv loop is on its way out; let it
                // finish cleaning up rather than racing a second connection.
                Some(PeerRelay::Ready(_)) | Some(PeerRelay::Connecting) => return None,
                Some(PeerRelay::Failed { until, .. }) if Instant::now() < *until => return None,
                _ => {}
            }
            let failures = match pool.get(&key) {
                Some(PeerRelay::Failed { failures, .. }) => *failures,
                _ => 0,
            };
            pool.insert(key.clone(), PeerRelay::Connecting);
            failures
        };

        let this = self.clone();
        let handle = tokio::spawn(async move {
            let short = &key.0[..16.min(key.0.len())];
            let dial = tokio::time::timeout(
                PEER_RELAY_CONNECT_TIMEOUT,
                relay::connect_peer(&this.node, &key.0, &this.identity, &owner),
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
            if no_session {
                let pk = owner;
                let _ = client.out_tx.send(ClientToRelay::GetBundle { pk }).await;
            }
            this.send_kick.notify_one();

            // Drain it like our own: a relay we deposit on may also be holding
            // mail for us, and the frame handling is identical. No
            // RelayConnected event — that state is about our own relay, and
            // flipping it here would tell the user the wrong thing.
            this.clone().run_recv_loop(client, None).await;
            this.peer_relays.lock().await.remove(&key);
            gipny_libcore::debug_trace!("[relay-client] peer relay {short} disconnected");
        });
        self.tasks.lock().unwrap().push(handle);
        None
    }

    /// Only the local terminal connection is allowed to offer incoming mail.
    pub(super) async fn run_recv_loop(self: Arc<Self>, client: RelayClient, own: Option<String>) {
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
                    if own.as_deref().is_some_and(|o| !relay::same_destination(o, &self.relay_address())) {
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
                    if own.is_none() && matches!(frame, RelayToClient::Incoming { .. }) { continue; }
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
        out_tx: &relay::RelaySender,
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
    /// used to wait for, until their terminal was ready. They were dialled at start only
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
