//! Relay mode, built-in relay lifecycle, and reachability.

use super::*;

/// Where this client collects its mail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RelayMode {
    /// A relay inside this process, on a destination made at launch and never
    /// written down. The default: a fresh install works with no setup.
    Builtin,
    /// A relay somebody operates, named in Settings. A mailbox that is there
    /// while this app is not.
    External,
}

impl RelayMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::External => "external",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "builtin" => Some(Self::Builtin),
            "external" => Some(Self::External),
            _ => None,
        }
    }
}

/// The saved choice if there is one. Without one, a profile that already names
/// a relay keeps using it, and a profile that names none gets the built-in one.
fn resolve_relay_mode(saved: Option<&[u8]>, external: &str) -> RelayMode {
    match saved
        .and_then(|b| std::str::from_utf8(b).ok())
        .and_then(RelayMode::parse)
    {
        // External with no address can never connect, and Settings no longer
        // offers a way out of it: fall back to the built-in relay.
        Some(RelayMode::External) if external.trim().is_empty() => RelayMode::Builtin,
        Some(mode) => mode,
        None if external.trim().is_empty() => RelayMode::Builtin,
        None => RelayMode::External,
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
    pub mode: RelayMode,
    /// The address saved for external mode, whether or not it is in use.
    pub external: String,
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
    /// The relay named in Settings, used in external mode.
    pub(super) fn external_relay(&self) -> String {
        self.db
            .get_setting(SETTING_RELAY_ADDRESS)
            .ok()
            .flatten()
            .and_then(|v| String::from_utf8(v).ok())
            .unwrap_or_else(|| DEFAULT_RELAY.to_string())
    }

    pub fn relay_mode(&self) -> RelayMode {
        resolve_relay_mode(
            self.db
                .get_setting(SETTING_RELAY_MODE)
                .ok()
                .flatten()
                .as_deref(),
            &self.external_relay(),
        )
    }

    /// Where we collect right now; empty while there is nowhere yet. In
    /// built-in mode this is the address of this launch and lives in memory
    /// only — it is never written into the settings, which is how 0.4.0 left
    /// clients listening on a relay that no longer existed.
    pub(super) fn relay_address(&self) -> String {
        match self.relay_mode() {
            RelayMode::External => self.external_relay(),
            RelayMode::Builtin => {
                match &*self.hosted_state.read().unwrap_or_else(|p| p.into_inner()) {
                    HostedRelayState::Ready { address } => address.clone(),
                    _ => String::new(),
                }
            }
        }
    }

    pub fn relay_info(&self) -> RelayInfo {
        RelayInfo {
            mode: self.relay_mode(),
            external: self.external_relay(),
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
    /// as a contact would. Stops with the relay; started again with it.
    pub(super) fn spawn_reach_check(self: &Arc<Self>, address: String) {
        const EVERY: Duration = Duration::from_secs(180);
        let this = self.clone();
        let handle = tokio::spawn(async move {
            // Give the new LeaseSet a moment to reach the floodfills.
            tokio::time::sleep(Duration::from_secs(60)).await;
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
                match result {
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
                tokio::time::sleep(EVERY).await;
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

    pub async fn set_relay_mode(self: &Arc<Self>, mode: RelayMode) -> Result<()> {
        self.db
            .set_setting(SETTING_RELAY_MODE, mode.as_str().as_bytes())?;
        match mode {
            RelayMode::Builtin => self.start_hosted_relay(),
            RelayMode::External => {
                if let Some(task) = self
                    .hosted_task
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take()
                {
                    task.abort();
                }
                *self.hosted_relay.lock().unwrap_or_else(|p| p.into_inner()) = None;
                self.announce_pending.lock().await.clear();
                self.set_hosted_state(HostedRelayState::Off);
            }
        }
        // The open connection to the old relay notices at its next ping that
        // it is no longer the one to collect from, and the loop redials.
        Ok(())
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

    /// The relay built while the password was typed, if there is one and it
    /// came up, claimed for us; otherwise a new one, built now.
    pub(super) async fn bring_up_hosted_relay(
        self: &Arc<Self>,
    ) -> std::result::Result<gipny_libcore::EphemeralRelay, gipny_libcore::NetError> {
        let owner = self.identity.card().sign_pk;
        let prebuilt = self
            .prebuilt_relay
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(task) = prebuilt {
            match task.await {
                Ok(Ok(relay)) => {
                    gipny_libcore::debug_trace!(
                        "[relay-hosted] using the relay built before unlock"
                    );
                    relay.claim(owner, Some(dht_client::handler(&self.dht)));
                    return Ok(relay);
                }
                Ok(Err(e)) => gipny_libcore::debug_error!(
                    "[relay-hosted] the relay built before unlock failed: {e:?}; building one now"
                ),
                Err(_) => {}
            }
        }
        gipny_libcore::EphemeralRelay::start(
            // This relay is our inbox and nobody else's.
            gipny_libcore::MemStoreLimits::personal(owner),
            Some(dht_client::handler(&self.dht)),
        )
        .await
    }

    /// Brings the built-in relay up and hands it over. Off the path anything
    /// user-facing waits on: `EphemeralRelay::start` returns once the
    /// destination's tunnels exist, commonly a minute or two.
    pub(super) async fn run_hosted_relay(self: Arc<Self>) {
        let mut wait = Duration::from_secs(15);
        loop {
            if self.relay_mode() != RelayMode::Builtin {
                self.set_hosted_state(HostedRelayState::Off);
                return;
            }
            self.set_hosted_state(HostedRelayState::Starting);
            gipny_libcore::debug_trace!(
                "[relay-hosted] starting the built-in relay (in-process router)..."
            );
            match self.bring_up_hosted_relay().await {
                Ok(relay) => {
                    // The mode may have changed while the tunnels were building.
                    if self.relay_mode() != RelayMode::Builtin {
                        drop(relay);
                        self.set_hosted_state(HostedRelayState::Off);
                        return;
                    }
                    let address = relay.address().to_string();
                    gipny_libcore::debug_trace!(
                        "[relay-hosted] built-in relay ready at {}",
                        &address[..address.len().min(16)]
                    );
                    *self.hosted_relay.lock().unwrap_or_else(|p| p.into_inner()) = Some(relay);
                    // Every contact still holds the previous launch's address.
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
                        // Anything left for us while we were away.
                        this.collect_from_dht(DHT_COLLECT_DAYS_FIRST).await;
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
    /// or through the network when that relay is silent, opening a session if
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

    pub fn set_relay_address(&self, addr: &str) -> Result<()> {
        self.db
            .set_setting(SETTING_RELAY_ADDRESS, addr.trim().as_bytes())?;
        Ok(())
    }
}
#[cfg(test)]
mod relay_mode_tests {
    use super::{resolve_relay_mode, RelayMode};

    #[test]
    fn a_fresh_profile_gets_the_built_in_relay() {
        assert_eq!(resolve_relay_mode(None, ""), RelayMode::Builtin);
        assert_eq!(resolve_relay_mode(None, "  "), RelayMode::Builtin);
    }

    #[test]
    fn a_profile_that_already_names_a_relay_keeps_it() {
        // Upgrading must not move anyone off the relay their contacts know.
        assert_eq!(resolve_relay_mode(None, "abc.b32.i2p"), RelayMode::External);
    }

    #[test]
    fn an_explicit_choice_wins_either_way() {
        assert_eq!(
            resolve_relay_mode(Some(b"builtin"), "abc.b32.i2p"),
            RelayMode::Builtin
        );
        assert_eq!(
            resolve_relay_mode(Some(b"external"), "abc.b32.i2p"),
            RelayMode::External
        );
        // …except external with no address, which could never connect and
        // which Settings no longer offers a way out of.
        assert_eq!(
            resolve_relay_mode(Some(b"external"), " "),
            RelayMode::Builtin
        );
        // Garbage in the setting is no choice at all.
        assert_eq!(resolve_relay_mode(Some(b"???"), ""), RelayMode::Builtin);
    }
}
