//! Relay connections, routing and frame handling.

use super::inbox::is_transient;
use super::*;

impl SessionManager {
    pub fn terminal_address(&self) -> String {
        self.local_relay
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|r| r.address().to_owned())
            .unwrap_or_default()
    }

    pub fn terminal_hops(&self) -> Option<u8> {
        self.local_relay
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|r| r.hops())
    }

    pub(super) fn relay_address(&self) -> String {
        self.terminal_address()
    }

    /// Dial the relays of the most recent contacts now, in the background.
    ///
    /// Finding a relay's LeaseSet and opening the stream costs 5–30 s, and it
    /// was paid by the first letter to each contact, on the way out and again
    /// on the way back (e2e: 8 s and 32 s of a 64 s round trip). Dialled
    /// ahead, the letter finds the connection open. `relay_for` never blocks.
    pub(super) fn warm_peer_relays(self: &Arc<Self>) {
        const WARM: usize = 8;
        let Ok(mut contacts) = self.db.list_contacts() else {
            return;
        };
        contacts.retain(|c| {
            c.trust != TrustLevel::Blocked
                && c.request_state != crate::db::RequestState::Incoming
                && (crate::is_valid_i2p_address(&c.i2p_address) || c.relay_address.as_deref().is_some_and(crate::is_valid_i2p_address))
        });
        contacts.sort_by_key(|c| std::cmp::Reverse(c.last_message_at));
        contacts.truncate(WARM);
        if contacts.is_empty() {
            return;
        }
        let this = self.clone();
        self.spawn_task(async move {
            for contact in &contacts {
                let _ = this.relay_for(contact).await;
            }
        });
    }

    /// As [`Self::warm_peer_relays`], for one contact whose relay just became
    /// known: added, or a letter or the network named a new one.
    pub(super) fn warm_relay_of(self: &Arc<Self>, contact_id: i64) {
        let this = self.clone();
        self.spawn_task(async move {
            let Ok(Some(contact)) = this.db.get_contact(contact_id) else {
                return;
            };
            if contact.trust == TrustLevel::Blocked
                || contact.request_state == crate::db::RequestState::Incoming
            {
                return;
            }
            let _ = this.relay_for(&contact).await;
        });
    }

    pub(super) fn spawn_relay_loop(self: Arc<Self>) {
        let this = self.clone();
        self.spawn_task(async move {
            let mut backoff = RECONNECT_INITIAL_MS;
            loop {
                let i2p_address = this.relay_address();
                if i2p_address.is_empty() {
                    tokio::time::sleep(Duration::from_millis(RECONNECT_INITIAL_MS)).await;
                    continue;
                }
                crate::debug_trace!("[session] relay connect {}", &i2p_address[..16.min(i2p_address.len())]);
                let local = this
                    .local_relay
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                    .filter(|r| relay::same_destination(r.address(), &i2p_address))
                    .map(|r| r.connect_local());
                // Bounded: nothing below has a timeout of its own, and a dial
                // i2p leaves hanging would otherwise keep us off our own mail
                // until restart (e2e run 35963073832: the master never got the
                // agent's GRANT).
                let connected = match tokio::time::timeout(PEER_RELAY_CONNECT_TIMEOUT, async {
                    match local {
                        Some(stream) => relay::connect_local(stream, &i2p_address, &this.identity).await,
                        None => Err(RelayError::Closed),
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
                        crate::debug_trace!("[session] relay connected & authed");
                        backoff = RECONNECT_INITIAL_MS;
                        *this.relay_out.write().await = Some(client.out_tx.clone());
                        let _ = this.events.send(SessionEvent::Connected).await;
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
                            .run_recv_loop(client, Some(i2p_address.clone()), None)
                            .await;
                        *this.relay_out.write().await = None;
                        let _ = this.events.send(SessionEvent::Disconnected).await;
                    }
                    Err(e) => crate::debug_trace!("[session] connect fail: {:?}", e),
                }
                tokio::time::sleep(Duration::from_millis(backoff)).await;
                backoff = (backoff * 2).min(RECONNECT_MAX_MS);
            }
        });
    }

    /// A legacy card can contain two candidates. Neither receives a payload
    /// until its live terminal proves ownership of this contact's pinned key.
    pub(super) fn relay_for<'a>(
        self: &'a Arc<Self>,
        contact: &'a Contact,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<relay::RelaySender>> + Send + 'a>>
    {
        Box::pin(async move {
            let owner = to_arr32(contact.identity_sign.clone()).ok()?;
            let mut candidates = Vec::new();
            for address in [
                Some(contact.i2p_address.as_str()),
                contact.relay_address.as_deref(),
            ]
            .into_iter()
            .flatten()
            {
                if crate::is_valid_i2p_address(address)
                    && !candidates
                        .iter()
                        .any(|other: &String| relay::same_destination(other, address))
                {
                    candidates.push(address.to_owned());
                }
            }
            let first = candidates.first()?;
            let key = format!("{}:{}:{}", contact.id, first, candidates.last()?);
            // Never dial on this path — see Core::relay_for. It runs inside the
            // send loop, and an unreachable relay has no timeout of its own, so
            // blocking here would stall delivery to every other contact.
            let failures = {
                let mut pool = self.peer_relays.lock().await;
                match pool.get(&key) {
                    Some(PeerRelay::Ready(tx)) if !tx.is_closed() && tx.belongs_to(&owner) => {
                        return Some(tx.clone())
                    }
                    Some(PeerRelay::Ready(_)) | Some(PeerRelay::Connecting) => return None,
                    Some(PeerRelay::Failed { until, .. }) if std::time::Instant::now() < *until => {
                        return None
                    }
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
            let (contact_id, contact_pk) =
                (contact.id, to_arr32(contact.identity_sign.clone()).ok());
            self.spawn_task(async move {
                let short = &key[..16.min(key.len())];
                let mut dial = Ok(Err(RelayError::AuthFailed));
                for address in &candidates {
                    dial = tokio::time::timeout(
                        PEER_RELAY_CONNECT_TIMEOUT,
                        crate::relay::connect_peer(&this.node, address, &this.identity, &owner),
                    )
                    .await;
                    if matches!(dial, Ok(Ok(_))) {
                        break;
                    }
                }
                let client = match dial {
                    Ok(Ok(c)) => c,
                    other => {
                        match other {
                            Err(_) => crate::debug_trace!(
                                "[session] peer relay {short} did not answer in {:?}",
                                PEER_RELAY_CONNECT_TIMEOUT
                            ),
                            Ok(Err(e)) => crate::debug_trace!(
                                "[session] peer relay {short} unreachable: {e:?}"
                            ),
                            Ok(Ok(_)) => unreachable!(),
                        }
                        this.peer_relays.lock().await.insert(
                            key.clone(),
                            PeerRelay::Failed {
                                until: std::time::Instant::now() + peer_relay_backoff(failures),
                                failures: failures + 1,
                            },
                        );
                        // Dialled again when the wait is over, letter or not: a
                        // first dial fails mostly because the relay's LeaseSet has
                        // not spread yet, and the first letter then waited for a
                        // dial of its own (e2e run 36042483601: 7 s of a 16 s echo).
                        if let Some(wait) = peer_relay_redial_after(failures) {
                            let again = this.clone();
                            this.spawn_task(async move {
                                tokio::time::sleep(wait).await;
                                if let Ok(Some(c)) = again.db.get_contact(contact_id) {
                                    let _ = again.relay_for(&c).await;
                                }
                            });
                        }
                        return;
                    }
                };
                crate::debug_trace!("[session] connected to peer relay {short}");
                this.peer_relays
                    .lock()
                    .await
                    .insert(key.clone(), PeerRelay::Ready(client.out_tx.clone()));
                // No session with them yet: ask for their bundle now, so a
                // first letter does not wait a round trip for it.
                let no_session = !this.sessions.lock().await.contains_key(&contact_id)
                    && !this.db.has_session(contact_id).unwrap_or(false);
                if let (true, Some(pk)) = (no_session, contact_pk) {
                    let _ = client.out_tx.send(ClientToRelay::GetBundle { pk }).await;
                }
                this.send_kick.notify_one();
                this.clone().run_recv_loop(client, None, None).await;
                this.peer_relays.lock().await.remove(&key);
            });
            None
        })
    }

    pub(super) async fn route_for(self: &Arc<Self>, contact: &Contact) -> Option<Route> {
        self.relay_for(contact).await
    }

    pub(super) async fn deliver(
        &self,
        contact: &Contact,
        blob: Vec<u8>,
        route: &Route,
        _first_letter: bool,
        remaining: Option<Duration>,
    ) -> Result<()> {
        let owner = to_arr32(contact.identity_sign.clone())?;
        route.deposit(owner, blob, remaining).await?;
        Ok(())
    }

    /// `collecting_from` is set on the connection to our own relay: once the
    /// relay we collect from changes, this one is no longer where mail arrives.
    pub(super) async fn run_recv_loop(
        self: Arc<Self>,
        client: RelayClient,
        collecting_from: Option<String>,
        done: Option<&(dyn Fn() -> bool + Send + Sync)>,
    ) {
        let in_rx = client.in_rx.clone();
        let out_tx = client.out_tx.clone();
        let mut ping = tokio::time::interval(Duration::from_secs(PING_INTERVAL_SECS));
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
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
                    if collecting_from.as_deref().is_some_and(|o| !relay::same_destination(o, &self.relay_address())) {
                        crate::debug_trace!("[session] our relay changed, reconnecting to the new one");
                        break;
                    }
                    if last_activity.elapsed() > dead_threshold {
                        crate::debug_trace!("[session] no relay activity for {:?}, forcing reconnect", last_activity.elapsed());
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
                    if collecting_from.is_some() && matches!(&frame, RelayToClient::Error(e) if e == relay::ERR_NEEDS_AUTH_V2) {
                        crate::debug_trace!("[session] our relay wants AuthV2, reconnecting");
                        break;
                    }
                    if let Err(e) = self.handle_relay_frame(frame, &out_tx).await {
                        crate::debug_trace!("[session] handle err: {:?}", e);
                    }
                }
            }
        }
    }

    pub(super) async fn handle_relay_frame(
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
                    Err(SessionError::Codec) => {
                        crate::debug_trace!("[session] codec err on msg {}, NOT acking", id);
                    }
                    Err(SessionError::StaleOpk) => {
                        crate::debug_trace!(
                            "[session] stale-OPK X3dhInit on msg {}, ACK and skip (zombie)",
                            id
                        );
                        let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                    }
                    Err(SessionError::SealedDrop) => {
                        let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                    }
                    Err(SessionError::Crypto(_)) => {
                        if let Ok(Some(c)) = self.db.find_contact_by_sign_pk(&from) {
                            let fresh = {
                                let m = self.session_created_at.lock().await;
                                m.get(&c.id)
                                    .map(|t| now_ms() - *t < FRESH_SESSION_GRACE_MS)
                                    .unwrap_or(false)
                            };
                            if fresh {
                                crate::debug_trace!("[session] crypto err on msg {} within fresh-session grace, ACK and skip", id);
                                let _ = out_tx.send(ClientToRelay::Ack { id }).await;
                            } else {
                                crate::debug_trace!(
                                    "[session] crypto err on msg {}, requesting resync, NOT acking",
                                    id
                                );
                                let _ = self.request_resync(&c).await;
                            }
                        } else {
                            crate::debug_trace!(
                                "[session] crypto err on msg {}, no contact, NOT acking",
                                id
                            );
                        }
                    }
                    // Nothing was committed (a letter once read is always
                    // acknowledged): the relay keeps it and offers it again.
                    Err(e) if is_transient(&e) => {
                        crate::debug_trace!(
                            "[session] storage err on msg {}: {:?}, NOT acking (will retry)",
                            id,
                            e
                        );
                    }
                    Err(e) => {
                        crate::debug_trace!("[session] incoming err: {:?}", e);
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
                                .insert(pk, (b, std::time::Instant::now()));
                        }
                    }
                }
            }
            RelayToClient::Error(reason) => {
                // Dropped silently before, which hid a relay refusing us.
                crate::debug_trace!("[session] relay error: {reason}");
                let _ = self.events.send(SessionEvent::RelayError { reason }).await;
            }
            _ => {}
        }
        Ok(())
    }
}
