//! Deterministic mapping lifecycle and network-change scheduling.

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use super::mapping::{cleanup_except, desired, maintain, release, Igd, Outcome, Rule};
use super::ownership::{Registry, Secret};
use super::protocol::public;

pub(super) const RENEW: Duration = Duration::from_secs(180);
pub(super) const RETRY: Duration = Duration::from_secs(60);
/// Discovery backoff ceiling, and how long an offline worker sleeps unasked.
pub(super) const BACKOFF_CAP: Duration = Duration::from_secs(30 * 60);
/// However often the host reports a change, the gateway is probed no more often.
pub(super) const RECHECK: Duration = Duration::from_secs(30);
/// Delay before the next discovery after `misses` consecutive empty ones.
pub(super) fn backoff(misses: u32) -> Duration {
    let doubled = RETRY
        .as_secs()
        .saturating_mul(1u64 << misses.saturating_sub(1).min(16));
    Duration::from_secs(doubled.min(BACKOFF_CAP.as_secs()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Ports {
    /// The router is not running (it is stopping): no conclusions.
    Stopped,
    /// Off in settings, hidden, or proxied: nothing should be mapped.
    Disabled,
    Active {
        tcp: u16,
        udp: u16,
    },
}
pub(super) trait Env {
    type Gateway: Igd;
    fn ports(&mut self) -> Ports;
    fn discover(&mut self, secret: &Secret, abort: &dyn Fn() -> bool) -> Option<Self::Gateway>;
    fn publish(&mut self, external: Ipv4Addr);
}

/// The worker's state between cycles. Time is passed in, so tests can run
/// hours of schedule in microseconds.
pub(super) struct Session<G: Igd> {
    pub(super) registry: Registry,
    pub(super) gateway: Option<G>,
    pub(super) states: Vec<(Rule, Outcome)>,
    pub(super) offline: bool,
    /// Next scheduled pass: maintenance with a gateway, discovery without.
    next: Instant,
    /// A network notification asked for a look before then.
    recheck: Option<Instant>,
    /// Last time the gateway was verified or searched for.
    probed: Option<Instant>,
    failures: u32,
    misses: u32,
    published: Option<Ipv4Addr>,
}
impl<G: Igd> Session<G> {
    pub(super) fn new(registry: Registry, now: Instant) -> Self {
        Self {
            registry,
            gateway: None,
            states: Vec::new(),
            offline: false,
            next: now,
            recheck: None,
            probed: None,
            failures: 0,
            misses: 0,
            published: None,
        }
    }
    /// The host reported a (debounced) network change. Nothing is deleted
    /// here: the next look decides whether the gateway is still the same one.
    pub(super) fn network(&mut self, offline: bool, now: Instant) {
        self.offline = offline;
        self.misses = 0;
        if offline {
            self.recheck = None;
            return;
        }
        let earliest = self.probed.map_or(now, |at| at + RECHECK).max(now);
        self.recheck = Some(self.recheck.map_or(earliest, |at| at.min(earliest)));
    }
    fn wait(&self, now: Instant) -> Duration {
        let until = self.recheck.map_or(self.next, |at| at.min(self.next));
        until.saturating_duration_since(now)
    }
    fn forget(&mut self) {
        self.gateway = None;
        self.states.clear();
        self.failures = 0;
        self.published = None;
    }
    pub(super) fn cycle<E: Env<Gateway = G>>(
        &mut self,
        env: &mut E,
        now: Instant,
        abort: &dyn Fn() -> bool,
    ) -> Duration {
        if abort() {
            return RETRY;
        }
        if self.offline {
            return BACKOFF_CAP;
        }
        let ports = match env.ports() {
            Ports::Stopped => return RETRY,
            Ports::Active { tcp, udp } if tcp != 0 || udp != 0 => (tcp, udp),
            Ports::Active { .. } | Ports::Disabled => {
                // Hidden, proxied or no listener: nothing of ours should stay.
                if let Some(gateway) = self.gateway.take() {
                    cleanup_except(&gateway, &mut self.registry, &[], abort);
                }
                self.forget();
                self.recheck = None;
                self.next = now + RETRY;
                return RETRY;
            }
        };
        let full = now >= self.next;
        if !full && !self.recheck.is_some_and(|at| now >= at) {
            return self.wait(now);
        }
        self.recheck = None;
        if self.gateway.is_some() {
            self.probed = Some(now);
            if !self.gateway.as_ref().is_some_and(|g| g.same_device()) {
                crate::debug_trace!("[upnp] gateway changed or unreachable; searching again");
                // Nothing is deleted on the way out: the old control URL may now
                // reach a different device, and finite leases lapse by themselves.
                self.forget();
            } else if !full {
                return self.wait(now);
            } else {
                return self.maintain_all(env, ports, now, abort);
            }
        }
        if abort() {
            return RETRY;
        }
        self.probed = Some(now);
        match env.discover(&self.registry.secret, abort) {
            Some(gateway) => {
                self.forget();
                self.gateway = Some(gateway);
                self.misses = 0;
                self.maintain_all(env, ports, now, abort)
            }
            None => {
                self.misses = self.misses.saturating_add(1);
                self.next = now + backoff(self.misses);
                crate::debug_trace!(
                    "[upnp] no usable gateway; next search in {}s",
                    backoff(self.misses).as_secs()
                );
                self.wait(now)
            }
        }
    }
    fn maintain_all<E: Env<Gateway = G>>(
        &mut self,
        env: &mut E,
        (tcp, udp): (u16, u16),
        now: Instant,
        abort: &dyn Fn() -> bool,
    ) -> Duration {
        let Some(gateway) = self.gateway.as_ref() else {
            return RETRY;
        };
        let rules = desired(tcp, udp, gateway.client());
        // Old listener ports, an old LAN address, rules from before a crash:
        // anything of ours on this gateway that is not wanted now.
        cleanup_except(gateway, &mut self.registry, &rules, abort);
        self.states.retain(|(rule, _)| rules.contains(rule));
        let (mut confirmed, mut failed) = (false, false);
        for rule in &rules {
            if abort() {
                return RETRY;
            }
            let outcome = maintain(gateway, rule, &mut self.registry, abort);
            crate::debug_trace!("[upnp] {} mapping: {outcome:?}", rule.protocol);
            match outcome {
                Outcome::Confirmed { .. } => confirmed = true,
                Outcome::Failed => failed = true,
                Outcome::Foreign => {}
            }
            match self.states.iter_mut().find(|(known, _)| known == rule) {
                Some(state) => state.1 = outcome,
                None => self.states.push((rule.clone(), outcome)),
            }
        }
        if confirmed {
            self.failures = 0;
            // Only now has this gateway shown it forwards to us; before that its
            // address is not ours to announce.
            if !abort() {
                if let Some(external) = gateway.external().filter(|ip| public(*ip)) {
                    if self.published != Some(external) {
                        env.publish(external);
                        self.published = Some(external);
                    }
                }
            }
        } else if failed {
            self.failures += 1;
            if self.failures >= 2 {
                // Routers reboot or change without a host callback. Forget the
                // handle and search again; mappings are not deleted, finite
                // ones lapse and the journal keeps permanent ones.
                self.forget();
                self.next = now + RETRY;
                return self.wait(now);
            }
        } else {
            // Every wanted port is held by someone else: the gateway is fine.
            self.failures = 0;
        }
        self.next = now + if failed { RETRY } else { RENEW };
        self.wait(now)
    }
    /// At exit only permanent leases would outlive us; finite ones lapse.
    pub(super) fn shutdown(&mut self, abort: &dyn Fn() -> bool) {
        let Some(gateway) = self.gateway.take() else {
            return;
        };
        if self.offline {
            return;
        }
        let permanent: Vec<Rule> = self
            .states
            .iter()
            .filter(|(_, outcome)| *outcome == Outcome::Confirmed { permanent: true })
            .map(|(rule, _)| rule.clone())
            .collect();
        release(&gateway, &mut self.registry, &permanent, abort);
    }
}
