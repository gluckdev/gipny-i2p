//! Ownership-checked mapping creation, renewal, and cleanup.

use std::net::Ipv4Addr;

use super::ownership::{owned_label, GatewayKey, Registry};

/// Finite lease; maintenance renews it well before it lapses.
const LEASE: &str = "600";
/// Only for IGDs that refuse finite leases (error 725).
const PERMANENT: &str = "0";
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Rule {
    pub(super) protocol: &'static str,
    pub(super) port: u16,
    pub(super) client: Ipv4Addr,
}
pub(super) fn desired(tcp: u16, udp: u16, client: Ipv4Addr) -> Vec<Rule> {
    [("TCP", tcp), ("UDP", udp)]
        .into_iter()
        .filter(|(_, port)| *port != 0)
        .map(|(protocol, port)| Rule {
            protocol,
            port,
            client,
        })
        .collect()
}
pub(super) struct Mapping {
    pub(super) client: Ipv4Addr,
    pub(super) port: u16,
    pub(super) description: String,
    pub(super) enabled: bool,
    pub(super) lease: Option<u32>,
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Decision {
    Create,
    Renew,
    Foreign,
    Retry,
}
pub(super) fn decision(found: &Result<Mapping, i32>, rule: &Rule, label: &str) -> Decision {
    match found {
        Err(714) => Decision::Create,
        Ok(mapping)
            if mapping.client == rule.client
                && mapping.port == rule.port
                && owned_label(&mapping.description, label) =>
        {
            Decision::Renew
        }
        Ok(_) => Decision::Foreign,
        Err(_) => Decision::Retry,
    }
}

/// One gateway's port-mapping service. The worker logic sees only this, so it
/// can be driven without a network in tests.
pub(super) trait Igd {
    fn key(&self) -> &GatewayKey;
    fn client(&self) -> Ipv4Addr;
    fn query(&self, rule: &Rule) -> Result<Mapping, i32>;
    fn add(&self, rule: &Rule, lease: &str) -> i32;
    fn delete(&self, rule: &Rule) -> i32;
    fn external(&self) -> Option<Ipv4Addr>;
    /// Fetch the root description again (one request, one parse): still the
    /// same device, offering the same service, reached from the same address?
    fn same_device(&self) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    /// Ours and enabled. `permanent`: it will not lapse by itself.
    Confirmed { permanent: bool },
    /// Held by someone else. Left alone, and no reason to doubt the gateway.
    Foreign,
    /// The gateway did not give a usable answer.
    Failed,
}
pub(super) fn maintain<G: Igd>(
    gateway: &G,
    rule: &Rule,
    registry: &mut Registry,
    abort: &dyn Fn() -> bool,
) -> Outcome {
    let label = &gateway.key().label;
    match decision(&gateway.query(rule), rule, label) {
        Decision::Foreign => return Outcome::Foreign,
        Decision::Retry => return Outcome::Failed,
        Decision::Create | Decision::Renew => {}
    }
    if abort() || !registry.reserve(rule, &gateway.key().id) {
        return Outcome::Failed;
    }
    let mut permanent = false;
    let mut result = gateway.add(rule, LEASE);
    // IGDv1 devices may only support permanent leases. Durable ownership
    // allows verified cleanup on restart as well as on normal shutdown.
    if result == 725 && !abort() {
        permanent = true;
        result = gateway.add(rule, PERMANENT);
    }
    // 718 is the gateway calling it a conflict; only a fresh look says whose
    // rule that is (ours under another lease, or someone else's).
    if (result != 0 && result != 718) || abort() {
        return Outcome::Failed;
    }
    let found = gateway.query(rule);
    match decision(&found, rule, label) {
        Decision::Renew => match found {
            Ok(mapping) if mapping.enabled => Outcome::Confirmed {
                permanent: permanent || mapping.lease.map_or(true, |lease| lease == 0),
            },
            _ => Outcome::Failed,
        },
        Decision::Foreign => Outcome::Foreign,
        Decision::Create | Decision::Retry => Outcome::Failed,
    }
}
/// Delete these rules from this gateway where they are provably ours. An entry
/// is retired only on what this gateway says: 714 here says nothing about the
/// same port on any other gateway.
pub(super) fn release<G: Igd>(
    gateway: &G,
    registry: &mut Registry,
    rules: &[Rule],
    abort: &dyn Fn() -> bool,
) {
    let key = gateway.key();
    for rule in rules {
        if abort() {
            break;
        }
        let gone = match decision(&gateway.query(rule), rule, &key.label) {
            // Absent, or replaced by someone else: nothing of ours is left.
            Decision::Create | Decision::Foreign => true,
            Decision::Renew => !abort() && matches!(gateway.delete(rule), 0 | 714),
            Decision::Retry => false,
        };
        if gone {
            registry.retire(rule, &key.id);
        }
    }
    registry.flush();
}
/// Everything this gateway's journal entries hold except `keep`.
pub(super) fn cleanup_except<G: Igd>(
    gateway: &G,
    registry: &mut Registry,
    keep: &[Rule],
    abort: &dyn Fn() -> bool,
) {
    let stale: Vec<Rule> = registry
        .rules_on(&gateway.key().id)
        .into_iter()
        .filter(|rule| !keep.contains(rule))
        .collect();
    release(gateway, registry, &stale, abort);
}
