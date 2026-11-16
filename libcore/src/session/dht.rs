//! Relay-network discovery, mailbox collection and address lookup.

use super::*;

impl SessionManager {
    /// Handler installed on an embedded relay so this session can also serve
    /// relay-network requests. The node itself is shared with all DHT fallback
    /// delivery and collection performed by this manager.
    pub fn dht_handler(&self) -> DhtHandler {
        dht_client::handler(&self.dht)
    }

    /// Relay-network nodes that have answered us so far.
    pub fn dht_peer_count(&self) -> usize {
        self.dht.peer_count()
    }

    /// Announce an embedded relay as this node's reachable DHT endpoint.
    pub async fn join_dht(self: &Arc<Self>, address: &str) {
        // The outcome is deliberately not used here: the caller is the
        // background path that re-announces, and the boot screen's copy of this
        // same call is where the reached/peers count is reported. Both are the
        // same join — this one just has nothing to tell a person about.
        let _ = dht_client::join(&self.dht, &self.db, &self.identity, address).await;
        self.publish_bundle_to_dht().await;
        self.collect_from_dht(DHT_COLLECT_DAYS_FIRST).await;
    }

    pub(super) fn spawn_dht_loop(self: Arc<Self>) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            // Bootstrap immediately. Otherwise a headless client configured
            // with an external relay would wait 45 minutes before learning
            // that the relay network exists.
            dht_client::maintain(
                &this.dht,
                &this.db,
                &this.identity,
                nonempty(this.relay_address()).as_deref(),
            )
            .await;
            this.publish_bundle_to_dht().await;
            this.collect_from_dht(DHT_COLLECT_DAYS_FIRST).await;

            let mut maintain = tokio::time::interval(dht_client::MAINTAIN_EVERY);
            maintain.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            maintain.tick().await;
            let mut collect = tokio::time::interval(DHT_COLLECT_EVERY);
            collect.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            collect.tick().await;
            let mut days = DHT_COLLECT_DAYS;
            loop {
                tokio::select! {
                    _ = this.dht_kick.notified() => {
                        let address = nonempty(this.relay_address());
                        dht_client::maintain(
                            &this.dht,
                            &this.db,
                            &this.identity,
                            address.as_deref(),
                        ).await;
                        this.publish_bundle_to_dht().await;
                        this.collect_from_dht(DHT_COLLECT_DAYS_FIRST).await;
                        this.send_kick.notify_one();
                    }
                    _ = maintain.tick() => {
                        let address = nonempty(this.relay_address());
                        dht_client::maintain(
                            &this.dht,
                            &this.db,
                            &this.identity,
                            address.as_deref(),
                        ).await;
                        this.publish_bundle_to_dht().await;
                        let _ = this.db.dht_seen_purge(now_ms() - DHT_SEEN_TTL_MS);
                    }
                    _ = collect.tick() => {
                        this.collect_from_dht(days).await;
                        days = DHT_COLLECT_DAYS;
                    }
                }
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    pub(super) async fn collect_from_dht(self: &Arc<Self>, days: u32) {
        if self.dht.peer_count() == 0 {
            return;
        }
        let Ok(contacts) = self.db.list_contacts() else {
            return;
        };
        // Introductions first: the letters written after one open only with the
        // session it starts (e2e-dht run 35950928731 lost all three otherwise).
        let mut letters = dht_client::collect_intros(&self.dht, &self.identity, days).await;
        for contact in contacts {
            if contact.trust == TrustLevel::Blocked {
                continue;
            }
            let (Ok(their_sign), Ok(their_dh)) = (
                to_arr32(contact.identity_sign),
                to_arr32(contact.identity_dh),
            ) else {
                continue;
            };
            letters.extend(
                dht_client::collect_mail(&self.dht, &self.identity, &their_sign, &their_dh, days)
                    .await,
            );
        }

        for letter in letters {
            let hash = dht_client::letter_hash(&letter.envelope);
            match self.db.dht_seen_mark(&hash, now_ms()) {
                Ok(true) => {}
                _ => continue,
            }
            match self
                .handle_incoming_envelope(&[0u8; 32], &letter.envelope)
                .await
            {
                Ok(()) | Err(SessionError::StaleOpk) => {
                    dht_client::drop_letter(&self.dht, &letter).await;
                }
                // No session opens it yet. Over a relay the sender resends what
                // is not acked; here the sender is away, so the letter stays in
                // the network for a later pass, when the session may exist.
                Err(SessionError::SealedDrop) => {
                    let _ = self.db.dht_seen_forget(&hash);
                }
                Err(e) => {
                    crate::debug_trace!("[dht/session] letter did not open: {e:?}");
                    let _ = self.db.dht_seen_forget(&hash);
                }
            }
        }
        self.send_kick.notify_one();
    }

    pub(super) async fn maybe_look_up_address(self: &Arc<Self>, contact: &Contact) {
        if self.dht.peer_count() == 0 {
            return;
        }
        {
            let mut asked = self.dht_addr_asked.lock().await;
            let now = Instant::now();
            if asked
                .get(&contact.id)
                .is_some_and(|then| now.duration_since(*then) < DHT_ADDRESS_LOOKUP_EVERY)
            {
                return;
            }
            asked.insert(contact.id, now);
        }

        let (Ok(their_sign), Ok(their_dh)) = (
            to_arr32(contact.identity_sign.clone()),
            to_arr32(contact.identity_dh.clone()),
        ) else {
            return;
        };
        let this = self.clone();
        let id = contact.id;
        let known = contact.relay_address.clone();
        let handle = tokio::spawn(async move {
            let Some((relay, _)) =
                dht_client::find_address(&this.dht, &this.identity, &their_sign, &their_dh).await
            else {
                return;
            };
            if relay.trim().is_empty()
                || known
                    .as_deref()
                    .is_some_and(|known| relay::same_destination(known, &relay))
            {
                return;
            }
            crate::debug_trace!("[dht/session] contact {id} published a new relay address");
            if this.db.set_contact_relay(id, Some(&relay)).is_ok() {
                if let Some(old) = known.as_deref() {
                    this.peer_relays.lock().await.remove(old);
                }
                this.warm_relay_of(id);
                this.send_kick.notify_one();
                let _ = this
                    .events
                    .send(SessionEvent::ContactUpdated { contact_id: id })
                    .await;
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    /// Put our prekey bundle into the relay network, so a contact can open a
    /// session while our relay is away. True once some node holds it.
    pub async fn publish_bundle_to_dht(&self) -> bool {
        if self.dht.peer_count() == 0 {
            return false;
        }
        let Ok(bundle) = self.my_bundle() else {
            return false;
        };
        let Ok(bytes) = bincode::serialize(&bundle) else {
            return false;
        };
        dht_client::publish_bundle(&self.dht, &self.identity, &bytes).await
    }
}

pub(super) fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}
