use std::sync::Arc;
use std::time::Instant;

use tokio::time::MissedTickBehavior;

use gipny_libcore::db::{RequestState, TrustLevel};
use gipny_libcore::dht_client;
use gipny_libcore::relay::{self, ClientToRelay};

use super::{
    now_ms, to_arr32, Core, CoreError, CoreEvent, HostedRelayState, DHT_ADDRESS_LOOKUP_EVERY,
    DHT_COLLECT_DAYS, DHT_COLLECT_DAYS_FIRST, DHT_COLLECT_EVERY, DHT_SEEN_TTL_MS,
};

impl Core {
    /// Relay-network upkeep; joining happens when the built-in relay is up.
    ///
    /// Not started under the measuring profile (`GIPNY_FAST`). This loop is a
    /// request to a handful of stranger relays every ten minutes and a search
    /// of what they hold, twice over: traffic the two nodes being measured never
    /// asked for, on the same link whose speed is the number being taken. It is
    /// also the one part of this process a router cannot switch off from the
    /// command line, so it has to be gated here.
    pub(super) fn spawn_dht_loop(self: Arc<Self>) {
        if gipny_libcore::fast_on() {
            gipny_libcore::debug_trace!("[dht] relay-network upkeep off (fast profile)");
            return;
        }
        let this = self.clone();
        let handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(dht_client::MAINTAIN_EVERY);
            tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
            tick.tick().await;
            let mut collect = tokio::time::interval(DHT_COLLECT_EVERY);
            collect.set_missed_tick_behavior(MissedTickBehavior::Skip);
            collect.tick().await;
            let mut days = DHT_COLLECT_DAYS_FIRST;
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        let address = match &*this.hosted_state.read().unwrap_or_else(|p| p.into_inner()) {
                            HostedRelayState::Ready { address } => Some(address.clone()),
                            _ => None,
                        };
                        dht_client::maintain(&this.dht, &this.db, &this.identity, address.as_deref()).await;
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

    /// Collect what the network holds for us and hand it to the ordinary
    /// incoming path. `days` is how far back to look: a week on the first pass
    /// after a launch, then today and yesterday.
    pub(super) async fn collect_from_dht(self: &Arc<Self>, days: u32) {
        if self.dht.peer_count() == 0 {
            return;
        }
        let Ok(contacts) = self.db.list_contacts() else {
            return;
        };
        // First letters from people who may not know us yet; an unknown sender
        // becomes a contact request, exactly as over a relay. Introductions go
        // first: the letters written after one open only with the session it
        // starts (e2e-dht run 35950928731 lost all three otherwise).
        let mut letters = dht_client::collect_intros(&self.dht, &self.identity, days).await;
        for c in &contacts {
            if c.trust == TrustLevel::Blocked || c.request_state == RequestState::Incoming {
                continue;
            }
            let (Ok(their_sign), Ok(their_dh)) = (
                to_arr32(c.identity_sign.clone()),
                to_arr32(c.identity_dh.clone()),
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
            // A ratchet envelope decrypted twice looks like a broken session.
            match self.db.dht_seen_mark(&hash, now_ms()) {
                Ok(true) => {}
                _ => continue,
            }
            match self
                .handle_incoming_envelope(&[0u8; 32], &letter.envelope)
                .await
            {
                Ok(()) | Err(CoreError::StaleOpk) => {
                    // Ours and handled: take it out of the network so nobody
                    // holds it for the rest of its week.
                    dht_client::drop_letter(&self.dht, &letter).await;
                }
                // No session opens it yet. Over a relay the sender resends what
                // is not acked; here the sender is away, so the letter stays in
                // the network for a later pass, when the session may exist.
                Err(CoreError::SealedDrop) => {
                    let _ = self.db.dht_seen_forget(&hash);
                }
                Err(e) => {
                    gipny_libcore::debug_trace!(
                        "[dht] letter from the network did not open: {e:?}"
                    );
                    // Not handled: let a later pass try again.
                    let _ = self.db.dht_seen_forget(&hash);
                }
            }
        }
        // The answers (acks among them) go out on the next send tick.
        self.send_kick.notify_one();
    }

    /// Ask at most once every `DHT_ADDRESS_LOOKUP_EVERY`.
    pub(super) async fn maybe_look_up_address(
        self: &Arc<Self>,
        contact: &gipny_libcore::db::Contact,
    ) {
        if self.dht.peer_count() == 0 {
            return;
        }
        {
            let mut asked = self.dht_addr_asked.lock().await;
            let now = Instant::now();
            match asked.get(&contact.id) {
                Some(t) if now.duration_since(*t) < DHT_ADDRESS_LOOKUP_EVERY => return,
                _ => asked.insert(contact.id, now),
            };
        }
        self.look_up_address(contact);
    }

    /// Ask the network where a contact collects now. Runs in the background:
    /// a lookup takes tens of seconds over i2p, and the send loop must not
    /// wait for it.
    fn look_up_address(self: &Arc<Self>, contact: &gipny_libcore::db::Contact) {
        let (Ok(their_sign), Ok(their_dh)) = (
            to_arr32(contact.identity_sign.clone()),
            to_arr32(contact.identity_dh.clone()),
        ) else {
            return;
        };
        let (this, id, known) = (self.clone(), contact.id, contact.relay_address.clone());
        tokio::spawn(async move {
            let Some((relay, _issued)) =
                dht_client::find_address(&this.dht, &this.identity, &their_sign, &their_dh).await
            else {
                return;
            };
            if known
                .as_deref()
                .is_some_and(|known| relay::same_destination(known, &relay))
                || relay.trim().is_empty()
            {
                return;
            }
            gipny_libcore::debug_trace!(
                "[dht] contact {id} moved to a new relay; taking the address from the network"
            );
            if this.db.set_contact_relay(id, Some(&relay)).is_ok() {
                this.warm_relay_of(id);
                this.send_kick.notify_one();
                let _ = this
                    .events
                    .try_send(CoreEvent::ContactUpdated { contact_id: id });
            }
        });
    }

    /// Put our prekey bundle in the network, so a contact can open a session
    /// with us while our relay is down. It is the same bundle the relay hands
    /// out, and anyone holding our card may read it.
    pub(super) async fn publish_bundle_to_dht(self: &Arc<Self>) {
        if self.dht.peer_count() == 0 {
            return;
        }
        let Ok(bundle) = self.my_bundle() else { return };
        let Ok(bytes) = bincode::serialize(&bundle) else {
            return;
        };
        dht_client::publish_bundle(&self.dht, &self.identity, &bytes).await;
    }

    pub(super) async fn republish_bundle(&self) {
        let _ = self.ensure_prekeys().await;
        if let Some(tx) = self.relay_out.read().await.clone() {
            if let Ok(b) = self.my_bundle() {
                if let Ok(bytes) = bincode::serialize(&b) {
                    let _ = tx.send(ClientToRelay::Publish { bundle: bytes }).await;
                }
            }
        }
    }
}

impl Core {
    pub fn dht_status(&self) -> dht_client::DhtStatus {
        dht_client::status(&self.dht)
    }
}
