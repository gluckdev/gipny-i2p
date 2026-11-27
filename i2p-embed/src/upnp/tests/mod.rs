//! Shared deterministic IGD fixtures and domain-specific unit tests.

mod control;
mod mapping;
mod ownership;
mod protocol;
mod session;

use std::collections::HashSet;
use std::ffi::{c_char, c_int};
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use super::mapping::{cleanup_except, decision, maintain, Decision, Igd, Mapping, Outcome, Rule};
use super::ownership::{
    gateway_key, hex, hmac, owned_label, parse_secret, read_record, GatewayKey, Registry, Secret,
    JOURNAL_LIMIT, LABEL, TAG_MATCH,
};
use super::protocol::{c, endpoint, public, root_udn};
use super::session::{backoff, Env, Ports, Session, BACKOFF_CAP, RECHECK, RENEW, RETRY};
use super::{Control, Woke};

pub(super) use crate::test_support::Scratch;

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

const HOME: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 1);
const ME: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 20);
const WAN: Ipv4Addr = Ipv4Addr::new(1, 2, 3, 4);

#[derive(Default)]
struct FakeState {
    /// (protocol, port) → (client, description, lease)
    entries: HashMap<(&'static str, u16), (Ipv4Addr, String, u32)>,
    adds: usize,
    deletes: usize,
    queries: usize,
    verifies: usize,
    moved: bool,
    failing: HashSet<&'static str>,
    add_result: Option<i32>,
    permanent_only: bool,
}
#[derive(Clone)]
struct Fake {
    key: GatewayKey,
    state: Rc<RefCell<FakeState>>,
}
impl Fake {
    fn new(secret: &Secret, udn: &str) -> Self {
        Self {
            key: gateway_key(secret, udn, HOME).unwrap(),
            state: Rc::default(),
        }
    }
    fn foreign(&self, protocol: &'static str, port: u16) {
        self.state.borrow_mut().entries.insert(
            (protocol, port),
            (
                Ipv4Addr::new(192, 168, 1, 99),
                "other application".into(),
                0,
            ),
        );
    }
}
impl Igd for Fake {
    fn key(&self) -> &GatewayKey {
        &self.key
    }
    fn client(&self) -> Ipv4Addr {
        ME
    }
    fn query(&self, rule: &Rule) -> Result<Mapping, i32> {
        let mut state = self.state.borrow_mut();
        state.queries += 1;
        if state.failing.contains(rule.protocol) {
            return Err(501);
        }
        match state.entries.get(&(rule.protocol, rule.port)) {
            Some((client, description, lease)) => Ok(Mapping {
                client: *client,
                port: rule.port,
                description: description.clone(),
                enabled: true,
                lease: Some(*lease),
            }),
            None => Err(714),
        }
    }
    fn add(&self, rule: &Rule, lease: &str) -> i32 {
        let mut state = self.state.borrow_mut();
        state.adds += 1;
        if let Some(code) = state.add_result {
            return code;
        }
        if state.permanent_only && lease != "0" {
            return 725;
        }
        let key = (rule.protocol, rule.port);
        if state
            .entries
            .get(&key)
            .is_some_and(|(client, _, _)| *client != rule.client)
        {
            return 718;
        }
        state.entries.insert(
            key,
            (rule.client, self.key.label.clone(), lease.parse().unwrap()),
        );
        0
    }
    fn delete(&self, rule: &Rule) -> i32 {
        let mut state = self.state.borrow_mut();
        state.deletes += 1;
        if state.entries.remove(&(rule.protocol, rule.port)).is_some() {
            0
        } else {
            714
        }
    }
    fn external(&self) -> Option<Ipv4Addr> {
        Some(WAN)
    }
    fn same_device(&self) -> bool {
        let mut state = self.state.borrow_mut();
        state.verifies += 1;
        !state.moved
    }
}
struct FakeEnv {
    ports: Ports,
    gateways: VecDeque<Fake>,
    searches: usize,
    published: Vec<Ipv4Addr>,
}
impl FakeEnv {
    fn new(gateways: &[&Fake]) -> Self {
        Self {
            ports: Ports::Active {
                tcp: 34567,
                udp: 34568,
            },
            gateways: gateways.iter().map(|g| (*g).clone()).collect(),
            searches: 0,
            published: Vec::new(),
        }
    }
}
impl Env for FakeEnv {
    type Gateway = Fake;
    fn ports(&mut self) -> Ports {
        self.ports
    }
    fn discover(&mut self, _: &Secret, _: &dyn Fn() -> bool) -> Option<Fake> {
        self.searches += 1;
        self.gateways.pop_front()
    }
    fn publish(&mut self, external: Ipv4Addr) {
        self.published.push(external);
    }
}
fn never() -> bool {
    false
}
fn tcp() -> Rule {
    Rule {
        protocol: "TCP",
        port: 34567,
        client: ME,
    }
}
