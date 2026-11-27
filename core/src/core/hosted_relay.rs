//! Personal terminal lifecycle and reachability.

use super::*;

/// What the built-in relay's reach check does next. Reading our own relay
/// goes over a pipe that outlives its tunnels, so an unreachable relay shows
/// only in this check — and the check is what has to act on it.
#[derive(Default)]
struct ReachHeal {
    failures: u32,
    rebuilt_at: Option<Instant>,
}

struct ReachStep {
    rebuild: bool,
    wait: Duration,
}

impl ReachHeal {
    const EVERY: Duration = Duration::from_secs(180);
    const EVERY_FAILING: Duration = Duration::from_secs(60);
    const REBUILD_AFTER: u32 = 2;
    const REBUILD_COOLDOWN: Duration = Duration::from_secs(600);
    const AFTER_REBUILD: Duration = Duration::from_secs(120);

    fn after_check(&mut self, reachable: bool, now: Instant) -> ReachStep {
        if reachable {
            self.failures = 0;
            return ReachStep { rebuild: false, wait: Self::EVERY };
        }
        self.failures = self.failures.saturating_add(1);
        let cooled = self.rebuilt_at.is_none_or(|at| now.duration_since(at) >= Self::REBUILD_COOLDOWN);
        if self.failures >= Self::REBUILD_AFTER && cooled {
            self.failures = 0;
            self.rebuilt_at = Some(now);
            return ReachStep { rebuild: true, wait: Self::AFTER_REBUILD };
        }
        ReachStep { rebuild: false, wait: Self::EVERY_FAILING }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum HostedRelayState {
    Off,
    /// A published destination is usable once its tunnels exist: a minute or two.
    Starting,
    Ready {
        address: String,
    },
    Failed {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct RelayInfo {
    pub hosted: HostedRelayState,
    pub dial: DialState,
}

/// How the attempt to reach our own relay is going.
///
/// Without this, "нет связи с релеем" was the whole story, forever, with no
/// way to tell a relay that is still building tunnels from one that refuses
/// us — on a phone, where there is no log to open, that is the difference
/// between waiting and reinstalling.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DialState {
    /// Failed attempts since the last time we were connected.
    pub attempts: u32,
    /// Why the last one failed, in the transport's own words.
    pub last_error: Option<String>,
    /// Checks in a row in which our built-in relay could not be reached from
    /// the network, as a contact would reach it (`relay::probe`). We read the
    /// relay over a pipe, so our own connection says nothing about this: on a
    /// phone that lost its tunnels after a network change, mail read fine
    /// and nobody could write to us.
    pub unreachable_checks: u32,
    pub unreachable_error: Option<String>,
}

impl Core {
    pub(super) fn relay_address(&self) -> String {
        self.personal_address.clone()
    }

    pub fn relay_info(&self) -> RelayInfo {
        RelayInfo {
            hosted: self
                .hosted_state
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .clone(),
            dial: self
                .relay_dial
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .clone(),
        }
    }

    /// Record how the dial went and tell the interface, which is showing it.
    pub(super) fn note_relay_reach(&self, error: Option<String>) {
        {
            let mut d = self.relay_dial.write().unwrap_or_else(|p| p.into_inner());
            match &error {
                Some(_) => d.unreachable_checks = d.unreachable_checks.saturating_add(1),
                None => d.unreachable_checks = 0,
            }
            d.unreachable_error = error;
        }
        let _ = self.events.try_send(CoreEvent::RelayInfoChanged {
            info: self.relay_info(),
        });
    }

    /// While our built-in relay is up, reach it from the network now and then,
    /// as a contact would, and rebuild its tunnels when that keeps failing.
    /// Stops with the relay; started again with it.
    pub(super) fn spawn_reach_check(self: &Arc<Self>, address: String) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            // Give the new LeaseSet a moment to reach the floodfills.
            tokio::time::sleep(Duration::from_secs(60)).await;
            let mut heal = ReachHeal::default();
            loop {
                let still_ours = matches!(
                    &*this.hosted_state.read().unwrap_or_else(|p| p.into_inner()),
                    HostedRelayState::Ready { address: a } if relay::same_destination(a, &address)
                );
                if !still_ours {
                    this.note_relay_reach(None);
                    return;
                }
                let result = tokio::time::timeout(
                    PEER_RELAY_CONNECT_TIMEOUT,
                    relay::probe(&this.node, &address),
                )
                .await;
                match &result {
                    Ok(Ok(())) => this.note_relay_reach(None),
                    Ok(Err(e)) => {
                        gipny_libcore::debug_error!(
                            "[relay-hosted] not reachable from the network: {e:?}"
                        );
                        this.note_relay_reach(Some(format!("{e:?}")));
                    }
                    Err(_) => {
                        gipny_libcore::debug_error!("[relay-hosted] not reachable from the network: no answer in {PEER_RELAY_CONNECT_TIMEOUT:?}");
                        this.note_relay_reach(Some(format!(
                            "no answer in {PEER_RELAY_CONNECT_TIMEOUT:?}"
                        )));
                    }
                }
                let step = heal.after_check(matches!(result, Ok(Ok(()))), Instant::now());
                if step.rebuild {
                    if let Some(relay) = &*this.hosted_relay.lock().unwrap_or_else(|p| p.into_inner()) {
                        relay.rebuild_tunnels();
                    }
                }
                tokio::time::sleep(step.wait).await;
            }
        });
        self.tasks.lock().unwrap().push(handle);
    }

    pub(super) fn note_relay_dial(&self, error: Option<String>) {
        {
            let mut d = self.relay_dial.write().unwrap_or_else(|p| p.into_inner());
            match &error {
                Some(_) => d.attempts = d.attempts.saturating_add(1),
                None => d.attempts = 0,
            }
            d.last_error = error;
        }
        let _ = self.events.try_send(CoreEvent::RelayInfoChanged {
            info: self.relay_info(),
        });
    }

    pub(super) fn set_hosted_state(&self, state: HostedRelayState) {
        if !matches!(state, HostedRelayState::Ready { .. }) {
            // Without our relay nobody can reach this node; keep asking only.
            self.dht.set_me(None);
        }
        *self.hosted_state.write().unwrap_or_else(|p| p.into_inner()) = state;
        let _ = self.events.try_send(CoreEvent::RelayInfoChanged {
            info: self.relay_info(),
        });
    }

    /// Starts the relay this client hosts for itself, unless it is already
    /// starting or up.
    pub(super) fn start_hosted_relay(self: &Arc<Self>) {
        let mut slot = self.hosted_task.lock().unwrap_or_else(|p| p.into_inner());
        let busy = slot.as_ref().is_some_and(|t| !t.is_finished());
        let up = matches!(
            &*self.hosted_state.read().unwrap_or_else(|p| p.into_inner()),
            HostedRelayState::Ready { .. }
        );
        if busy || up {
            return;
        }
        let this = self.clone();
        *slot = Some(tokio::spawn(async move { this.run_hosted_relay().await }));
    }

    async fn bring_up_hosted_relay(
        self: &Arc<Self>,
    ) -> std::result::Result<gipny_libcore::EphemeralRelay, gipny_libcore::NetError> {
        gipny_libcore::EphemeralRelay::start_personal(
            &self.db,
            self.identity.clone(),
            Some(dht_client::handler(&self.dht)),
        )
        .await
    }

    /// Brings the built-in relay up and hands it over. Off the path anything
    /// user-facing waits on: `EphemeralRelay::start_personal` returns once the
    /// destination's tunnels exist, commonly a minute or two.
    pub(super) async fn run_hosted_relay(self: Arc<Self>) {
        let mut wait = Duration::from_secs(15);
        loop {
            self.set_hosted_state(HostedRelayState::Starting);
            gipny_libcore::debug_trace!(
                "[relay-hosted] starting the built-in relay (in-process router)..."
            );
            match self.bring_up_hosted_relay().await {
                Ok(relay) => {
                    relay.set_view_once_handler(self.view_once_handler());
                    let address = relay.address().to_string();
                    gipny_libcore::debug_trace!(
                        "[relay-hosted] built-in relay ready at {}",
                        &address[..address.len().min(16)]
                    );
                    *self.hosted_relay.lock().unwrap_or_else(|p| p.into_inner()) = Some(relay);
                    // Announce the personal address to trusted contacts.
                    // They are told as soon as their relay can be reached; the
                    // send loop keeps trying until each has been.
                    if let Ok(contacts) = self.db.list_contacts() {
                        let mut pending = self.announce_pending.lock().await;
                        pending.extend(
                            contacts
                                .iter()
                                .filter(|c| c.trust != TrustLevel::Blocked)
                                .map(|c| c.id),
                        );
                    }
                    self.set_hosted_state(HostedRelayState::Ready {
                        address: address.clone(),
                    });
                    self.spawn_reach_check(address.clone());
                    self.send_kick.notify_one();
                    let (dht, db, identity) =
                        (self.dht.clone(), self.db.clone(), self.identity.clone());
                    let this = self.clone();
                    let join = tokio::spawn(async move {
                        let joined = dht_client::join(&dht, &db, &identity, &address).await;
                        let _ = this.events.try_send(CoreEvent::DhtJoined {
                            peers: joined.peers,
                            reached: joined.reached,
                        });
                        this.publish_bundle_to_dht().await;
                    });
                    self.tasks.lock().unwrap().push(join);
                    return;
                }
                Err(e) => {
                    gipny_libcore::debug_error!(
                        "[relay-hosted] could not start: {e:?}; retrying in {wait:?}"
                    );
                    self.set_hosted_state(HostedRelayState::Failed {
                        reason: format!("{e:?}"),
                    });
                    tokio::time::sleep(wait).await;
                    wait = (wait * 2).min(Duration::from_secs(300));
                }
            }
        }
    }

    /// Tells contacts where we collect during this launch. An empty payload:
    /// the receiver reads the relay address off it and drops it.
    ///
    /// A contact is never dropped from this list for being away (2026-09-17,
    /// owner's requirement): an address that changed must not cost people the
    /// contact. So an announcement goes out again and again — over their relay,
    /// opening a session if
    /// there is none — until something arrives *from* them, which is the only
    /// proof they have our address. `announce_sent_at` keeps that from
    /// repeating on every tick of the send loop.
    pub(super) async fn flush_relay_announcements(self: &Arc<Self>) {
        let ids: Vec<i64> = self.announce_pending.lock().await.iter().copied().collect();
        for id in ids {
            let contact = match self.db.get_contact(id) {
                Ok(Some(c))
                    if c.trust != TrustLevel::Blocked
                        && c.request_state != RequestState::Incoming =>
                {
                    c
                }
                _ => {
                    self.announce_pending.lock().await.remove(&id);
                    continue;
                }
            };
            {
                let sent = self.announce_sent_at.lock().await;
                if sent
                    .get(&id)
                    .is_some_and(|t| t.elapsed() < ANNOUNCE_REPEAT_EVERY)
                {
                    continue;
                }
            }
            // Dialing happens in the background; an unreachable relay is tried
            // again on a later tick, and the network carries it meanwhile.
            let Some(out) = self.route_for(&contact).await else {
                continue;
            };
            if self.ensure_session_for(&contact, &out).await.is_err() {
                continue;
            }
            let mut payload = WirePayload::simple(0, String::new(), Vec::new(), now_ms(), None);
            if self
                .send_payload_via_relay(&contact, &mut payload, &out)
                .await
                .is_ok()
            {
                gipny_libcore::debug_trace!(
                    "[relay-hosted] told contact {id} where we collect now (waiting to hear back)"
                );
                self.announce_sent_at
                    .lock()
                    .await
                    .insert(id, Instant::now());
            }
        }
    }

    pub fn get_relay_address(&self) -> String {
        self.relay_address()
    }
}

#[cfg(test)]
mod reach_heal_tests {
    use super::*;

    #[test]
    fn reachable_relay_left_alone() {
        let mut h = ReachHeal::default();
        let now = Instant::now();
        let s = h.after_check(true, now);
        assert!(!s.rebuild);
        assert_eq!(s.wait, ReachHeal::EVERY);
        assert_eq!(h.failures, 0);
    }

    #[test]
    fn one_failure_only_looks_sooner() {
        let mut h = ReachHeal::default();
        let now = Instant::now();
        let s = h.after_check(false, now);
        assert!(!s.rebuild);
        assert_eq!(s.wait, ReachHeal::EVERY_FAILING);
        assert_eq!(h.failures, 1);
    }

    #[test]
    fn second_failure_rebuilds() {
        let mut h = ReachHeal::default();
        let now = Instant::now();
        h.after_check(false, now);
        let s = h.after_check(false, now);
        assert!(s.rebuild);
        assert_eq!(s.wait, ReachHeal::AFTER_REBUILD);
        assert_eq!(h.failures, 0);
    }

    #[test]
    fn success_between_failures_resets_count() {
        let mut h = ReachHeal::default();
        let now = Instant::now();
        h.after_check(false, now);
        h.after_check(true, now);
        let s = h.after_check(false, now);
        assert!(!s.rebuild);
        assert_eq!(h.failures, 1);
    }

    #[test]
    fn cooldown_prevents_rapid_rebuilds() {
        let mut h = ReachHeal::default();
        let now = Instant::now();
        h.after_check(false, now);
        h.after_check(false, now);
        // Just rebuilt; two more failures within cooldown should not rebuild.
        h.after_check(false, now);
        let s = h.after_check(false, now);
        assert!(!s.rebuild);
        assert_eq!(s.wait, ReachHeal::EVERY_FAILING);
    }
}
