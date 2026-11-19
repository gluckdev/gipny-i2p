//! Peer discovery and signed personal-terminal address lookup.

use super::*;

impl SessionManager {
    /// Serve discovery requests at the personal terminal.
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
    }

    pub(super) fn spawn_dht_loop(self: Arc<Self>) {
        let this = self.clone();
        self.spawn_task(async move {
            this.join_dht(&this.terminal_address()).await;
            loop {
                tokio::select! {
                    _=tokio::time::sleep(dht_client::MAINTAIN_EVERY)=>{},
                    _=this.dht_kick.notified()=>{},
                }
                dht_client::maintain(
                    &this.dht,
                    &this.db,
                    &this.identity,
                    Some(&this.terminal_address()),
                )
                .await;
                this.publish_bundle_to_dht().await;
            }
        });
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
        self.spawn_task(async move {
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
    }

    /// Publish our signed prekey bundle for contact discovery.
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
