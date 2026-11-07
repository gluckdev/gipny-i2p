//! Bounded IPv4 IGD port mapping for the embedded router. No daemon services,
//! proxy listeners or application ports are exposed by this module.
//!
//! What the LAN learns from it is kept to what a mapping needs. The HTTP
//! User-Agent is miniupnpc's own with a bare OS name. A mapping is described
//! as a generic word plus a tag derived per gateway from a local secret, so
//! two networks cannot match their tables against each other. The external
//! address goes to the router only once a mapping on that gateway is confirmed.
//! Mappings survive window focus and network notifications: the gateway is
//! re-verified, not torn down, and only rules that are provably ours on that
//! same gateway are ever deleted.
use std::collections::HashSet;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::io::{Read, Write};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[repr(C)]
struct Device {
    next: *mut Device,
    url: *mut c_char,
    st: *mut c_char,
    usn: *mut c_char,
    scope: c_uint,
}
#[repr(C)]
struct Urls {
    control: *mut c_char,
    description: *mut c_char,
    cif: *mut c_char,
    ipv6: *mut c_char,
    root: *mut c_char,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Service {
    control: [c_char; 128],
    event: [c_char; 128],
    description: [c_char; 128],
    kind: [c_char; 128],
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Data {
    element: [c_char; 128],
    base: [c_char; 128],
    presentation: [c_char; 128],
    level: c_int,
    cif: Service,
    first: Service,
    second: Service,
    ipv6: Service,
    temporary: Service,
}

extern "C" {
    fn upnpDiscoverDevices(
        kinds: *const *const c_char,
        delay: c_int,
        interface: *const c_char,
        daemon: *const c_char,
        port: c_int,
        ipv6: c_int,
        ttl: u8,
        error: *mut c_int,
        all: c_int,
    ) -> *mut Device;
    fn freeUPNPDevlist(list: *mut Device);
    fn miniwget_getaddr(
        url: *const c_char,
        size: *mut c_int,
        local: *mut c_char,
        local_len: c_int,
        scope: c_uint,
        status: *mut c_int,
    ) -> *mut c_void;
    fn parserootdesc(buffer: *const c_char, size: c_int, data: *mut Data);
    fn FreeUPNPUrls(urls: *mut Urls);
    fn GetUPNPUrls(urls: *mut Urls, data: *mut Data, description: *const c_char, scope: c_uint);
    fn UPNP_GetStatusInfo(
        url: *const c_char,
        kind: *const c_char,
        status: *mut c_char,
        uptime: *mut c_uint,
        error: *mut c_char,
    ) -> c_int;
    fn UPNP_GetExternalIPAddress(
        url: *const c_char,
        kind: *const c_char,
        address: *mut c_char,
    ) -> c_int;
    fn UPNP_GetSpecificPortMappingEntry(
        url: *const c_char,
        kind: *const c_char,
        port: *const c_char,
        proto: *const c_char,
        remote: *const c_char,
        client: *mut c_char,
        internal: *mut c_char,
        description: *mut c_char,
        enabled: *mut c_char,
        lease: *mut c_char,
    ) -> c_int;
    fn UPNP_AddPortMapping(
        url: *const c_char,
        kind: *const c_char,
        external: *const c_char,
        internal: *const c_char,
        client: *const c_char,
        description: *const c_char,
        proto: *const c_char,
        remote: *const c_char,
        lease: *const c_char,
    ) -> c_int;
    fn UPNP_DeletePortMapping(
        url: *const c_char,
        kind: *const c_char,
        port: *const c_char,
        proto: *const c_char,
        remote: *const c_char,
    ) -> c_int;
    fn gipny_router_upnp_ports(tcp: *mut u16, udp: *mut u16) -> c_int;
    fn gipny_router_upnp_external(ip: *const c_char);
    fn gipny_router_upnp_random(token: *mut u8) -> c_int;
    fn gipny_router_upnp_hmac(
        key: *const u8,
        key_len: usize,
        data: *const u8,
        data_len: usize,
        out: *mut u8,
    ) -> c_int;
    fn gipny_router_upnp_abort();
    fn gipny_router_stop_hook(hook: extern "C" fn());
    fn gipny_router_network_hook(hook: extern "C" fn(c_int));
}

/// Finite lease; maintenance renews it well before it lapses.
const LEASE: &str = "600";
/// Only for IGDs that refuse finite leases (error 725).
const PERMANENT: &str = "0";
const RENEW: Duration = Duration::from_secs(180);
const RETRY: Duration = Duration::from_secs(60);
/// Discovery backoff ceiling, and how long an offline worker sleeps unasked.
const BACKOFF_CAP: Duration = Duration::from_secs(30 * 60);
/// Network notifications come in bursts (focus is offline+online; Android
/// repeats itself): act once the host has been quiet this long.
const SETTLE: Duration = Duration::from_secs(2);
/// However often the host reports a change, the gateway is probed no more often.
const RECHECK: Duration = Duration::from_secs(30);
/// Exit waits at most about this long for the worker (plus one in-flight read).
const SHUTDOWN_BUDGET: Duration = Duration::from_secs(3);
/// No new exchange is started this close to the end of the budget.
const SHUTDOWN_MARGIN: Duration = Duration::from_millis(500);
const JOURNAL_LIMIT: usize = 16;
const DISCOVERY_DELAY_MS: c_int = 1500;
const CANDIDATES: usize = 8;
/// What a mapping is called on the gateway: a generic word and a per-gateway
/// tag, 15 characters, short enough that few routers truncate it.
const LABEL: &str = "UPnP ";
const TAG_HEX: usize = 10;
/// Tag digits that must survive a router truncating descriptions before a
/// rule is treated as ours.
const TAG_MATCH: usize = 8;
const SEARCH: [&str; 4] = [
    "urn:schemas-upnp-org:device:InternetGatewayDevice:1",
    "urn:schemas-upnp-org:service:WANIPConnection:1",
    "urn:schemas-upnp-org:service:WANPPPConnection:1",
    "upnp:rootdevice",
];
const SERVICES: [&str; 3] = [
    "urn:schemas-upnp-org:service:WANIPConnection:1",
    "urn:schemas-upnp-org:service:WANIPConnection:2",
    "urn:schemas-upnp-org:service:WANPPPConnection:1",
];

fn text(bytes: &[c_char]) -> Option<String> {
    let end = bytes.iter().position(|b| *b == 0)?;
    String::from_utf8(bytes[..end].iter().map(|b| *b as u8).collect()).ok()
}
fn c(value: &str) -> CString {
    CString::new(value).expect("validated UPnP string")
}
fn lan(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_link_local()
}
/// Only a globally routable address is worth handing to the router.
fn public(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        || a >= 240
        || (a == 100 && (64..128).contains(&b))
        || (a == 198 && (b == 18 || b == 19)))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The numeric IPv4 host of a plain `http://` URL: no DNS, userinfo, IPv6
/// scope tricks or alternate schemes from an untrusted XML file.
fn host(value: &str) -> Option<Ipv4Addr> {
    if value.len() > 2048
        || value
            .bytes()
            .any(|b| b <= 32 || b >= 127 || b == b'\\' || b == b'#' || b == b'@')
    {
        return None;
    }
    let rest = value.strip_prefix("http://")?;
    let authority = rest.split('/').next()?;
    let (host, port) = authority.split_once(':').unwrap_or((authority, "80"));
    if port.parse::<u16>().ok()? == 0 {
        return None;
    }
    host.parse::<Ipv4Addr>().ok()
}
/// Numeric LAN URLs only: no loopback or off-device SOAP targets.
fn endpoint(value: &str) -> Option<Ipv4Addr> {
    host(value).filter(|ip| lan(*ip))
}

fn hmac(key: &[u8], input: &[u8]) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    (unsafe {
        gipny_router_upnp_hmac(
            key.as_ptr(),
            key.len(),
            input.as_ptr(),
            input.len(),
            out.as_mut_ptr(),
        )
    } == 1)
        .then_some(out)
}

type Secret = [u8; 16];

/// Who we are on one gateway. Derived from the local secret and that gateway's
/// identity, so the tag a router (or whoever manages it) sees cannot be matched
/// with the tag on any other network, while it stays the same on this one
/// across restarts — which is what lets a later start clean up after a crash.
#[derive(Clone, Debug, PartialEq, Eq)]
struct GatewayKey {
    /// Binds journal entries to this gateway. Never leaves this machine.
    id: String,
    /// The description our mappings carry on this gateway.
    label: String,
}
/// `udn` is the root device's UDN (empty when it publishes none); `device` is
/// its LAN address. A gateway whose UDN is not unique per unit can only be told
/// apart from another of its model by that address.
fn gateway_key(secret: &Secret, udn: &str, device: Ipv4Addr) -> Option<GatewayKey> {
    let digest = hex(&hmac(
        secret,
        format!("gipny-upnp-gateway-v1\n{udn}\n{device}").as_bytes(),
    )?);
    Some(GatewayKey {
        id: digest[..32].to_owned(),
        label: format!("{LABEL}{}", &digest[32..32 + TAG_HEX]),
    })
}
/// Routers may cut descriptions short; what is left must still carry enough
/// of the tag, and be a prefix of ours.
fn owned_label(found: &str, ours: &str) -> bool {
    let found = found.trim_end();
    found.len() >= LABEL.len() + TAG_MATCH && ours.starts_with(found)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Rule {
    protocol: &'static str,
    port: u16,
    client: Ipv4Addr,
}
fn desired(tcp: u16, udp: u16, client: Ipv4Addr) -> Vec<Rule> {
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
struct Mapping {
    client: Ipv4Addr,
    port: u16,
    description: String,
    enabled: bool,
    lease: Option<u32>,
}
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Create,
    Renew,
    Foreign,
    Retry,
}
fn decision(found: &Result<Mapping, i32>, rule: &Rule, label: &str) -> Decision {
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

fn record_error(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.to_owned())
}
fn read_record(path: &Path, limit: u64) -> std::io::Result<String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(record_error("invalid UPnP record type"));
    }
    let mut value = String::new();
    file.take(limit + 1).read_to_string(&mut value)?;
    if value.len() as u64 > limit {
        return Err(record_error("UPnP record size limit"));
    }
    Ok(value)
}
/// Temporary file, flush, rename, flush the directory: a crash leaves the old
/// record or the new one, never a torn or empty one.
fn write_record(path: &Path, value: &str) -> std::io::Result<()> {
    let temporary = path.with_extension("pending");
    // A leftover (or planted) temporary of any type is replaced, never followed.
    match std::fs::remove_file(&temporary) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(value.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path)?;
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}
/// Missing, damaged, oversized, not a file, or a symlink: rewrite rather than
/// turn UPnP off for good. Anything else (permissions, I/O) is reported.
fn replaceable(error: &std::io::Error) -> bool {
    if matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidData
    ) {
        return true;
    }
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::ELOOP) {
        return true;
    }
    false
}
fn parse_secret(value: &str) -> Option<Secret> {
    let value = value.trim();
    if value.len() != 32 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut secret = [0u8; 16];
    for (index, byte) in secret.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[2 * index..2 * index + 2], 16).ok()?;
    }
    Some(secret)
}
/// The local secret, and whether it had to be made (or remade) just now.
fn load_secret(path: &Path) -> std::io::Result<(Secret, bool)> {
    match read_record(path, 64) {
        Ok(value) => {
            if let Some(secret) = parse_secret(&value) {
                return Ok((secret, false));
            }
        }
        Err(error) if replaceable(&error) => {}
        Err(error) => return Err(error),
    }
    let mut secret = [0u8; 16];
    if unsafe { gipny_router_upnp_random(secret.as_mut_ptr()) } != 1 {
        return Err(std::io::Error::other(
            "UPnP ownership randomness unavailable",
        ));
    }
    write_record(path, &hex(&secret))?;
    Ok((secret, true))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    rule: Rule,
    /// [`GatewayKey::id`] of the gateway the rule was requested from.
    gateway: String,
}
fn parse_journal(saved: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    for line in saved.lines().take(JOURNAL_LIMIT) {
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        // Lines without a gateway (unreleased development builds) cannot be
        // attributed to one, so they are not acted on anywhere.
        if fields.len() != 4 {
            continue;
        }
        let protocol = match fields[0] {
            "TCP" => "TCP",
            "UDP" => "UDP",
            _ => continue,
        };
        let gateway = fields[3];
        if gateway.len() != 32 || !gateway.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        if let (Ok(port), Ok(client)) = (fields[1].parse::<u16>(), fields[2].parse::<Ipv4Addr>())
        {
            let entry = Entry {
                rule: Rule {
                    protocol,
                    port,
                    client,
                },
                gateway: gateway.to_ascii_lowercase(),
            };
            if port != 0 && lan(client) && !entries.contains(&entry) {
                entries.push(entry);
            }
        }
    }
    entries
}

/// The ownership journal: which rules we asked which gateway for. Written
/// before every AddPortMapping, so a crash still leaves enough to find and
/// remove a permanent lease on a later start on that gateway.
struct Registry {
    path: PathBuf,
    secret: Secret,
    entries: Vec<Entry>,
    /// The file is behind memory (a save failed or an entry changed).
    dirty: bool,
}
impl Registry {
    fn open(directory: &Path) -> std::io::Result<Self> {
        let (secret, fresh) = load_secret(&directory.join("upnp-owner"))?;
        let path = directory.join("upnp-mappings");
        let saved = match read_record(&path, 4096) {
            Ok(value) => value,
            Err(error) if replaceable(&error) => String::new(),
            Err(error) => return Err(error),
        };
        // Entries are bound to gateway ids derived from the secret: under a new
        // secret none of them can be matched to anything again.
        let entries = if fresh {
            Vec::new()
        } else {
            parse_journal(&saved)
        };
        let mut registry = Self {
            path,
            secret,
            entries,
            dirty: fresh && !saved.is_empty(),
        };
        registry.flush();
        Ok(registry)
    }
    fn save(&mut self) -> std::io::Result<()> {
        let bytes: String = self
            .entries
            .iter()
            .map(|e| {
                format!(
                    "{} {} {} {}\n",
                    e.rule.protocol, e.rule.port, e.rule.client, e.gateway
                )
            })
            .collect();
        write_record(&self.path, &bytes)?;
        self.dirty = false;
        Ok(())
    }
    fn flush(&mut self) {
        if self.dirty {
            let _ = self.save();
        }
    }
    fn rules_on(&self, gateway: &str) -> Vec<Rule> {
        self.entries
            .iter()
            .filter(|e| e.gateway == gateway)
            .map(|e| e.rule.clone())
            .collect()
    }
    fn reserve(&mut self, rule: &Rule, gateway: &str) -> bool {
        let entry = Entry {
            rule: rule.clone(),
            gateway: gateway.to_owned(),
        };
        if !self.entries.contains(&entry) {
            if self.entries.len() >= JOURNAL_LIMIT {
                // Forget the oldest entry of another gateway, never this one's.
                let Some(index) = self.entries.iter().position(|e| e.gateway != gateway) else {
                    return false;
                };
                self.entries.remove(index);
            }
            self.entries.push(entry);
            self.dirty = true;
        }
        // Intent is durable before AddPortMapping, including permanent leases.
        !self.dirty || self.save().is_ok()
    }
    fn retire(&mut self, rule: &Rule, gateway: &str) {
        let before = self.entries.len();
        self.entries
            .retain(|e| !(e.rule == *rule && e.gateway == gateway));
        self.dirty |= self.entries.len() != before;
    }
}

/// One gateway's port-mapping service. The worker logic sees only this, so it
/// can be driven without a network in tests.
trait Igd {
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
enum Outcome {
    /// Ours and enabled. `permanent`: it will not lapse by itself.
    Confirmed { permanent: bool },
    /// Held by someone else. Left alone, and no reason to doubt the gateway.
    Foreign,
    /// The gateway did not give a usable answer.
    Failed,
}
fn maintain<G: Igd>(
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
fn release<G: Igd>(
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
fn cleanup_except<G: Igd>(
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

/// Delay before the next discovery after `misses` consecutive empty ones.
fn backoff(misses: u32) -> Duration {
    let doubled = RETRY
        .as_secs()
        .saturating_mul(1u64 << misses.saturating_sub(1).min(16));
    Duration::from_secs(doubled.min(BACKOFF_CAP.as_secs()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ports {
    /// The router is not running (it is stopping): no conclusions.
    Stopped,
    /// Off in settings, hidden, or proxied: nothing should be mapped.
    Disabled,
    Active { tcp: u16, udp: u16 },
}
trait Env {
    type Gateway: Igd;
    fn ports(&mut self) -> Ports;
    fn discover(&mut self, secret: &Secret, abort: &dyn Fn() -> bool) -> Option<Self::Gateway>;
    fn publish(&mut self, external: Ipv4Addr);
}

/// The worker's state between cycles. Time is passed in, so tests can run
/// hours of schedule in microseconds.
struct Session<G: Igd> {
    registry: Registry,
    gateway: Option<G>,
    states: Vec<(Rule, Outcome)>,
    offline: bool,
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
    fn new(registry: Registry, now: Instant) -> Self {
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
    fn network(&mut self, offline: bool, now: Instant) {
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
    fn cycle<E: Env<Gateway = G>>(
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
    fn shutdown(&mut self, abort: &dyn Fn() -> bool) {
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

struct Description {
    data: Data,
    local: Ipv4Addr,
    udn: String,
}
struct Body(*mut c_void);
impl Drop for Body {
    fn drop(&mut self) {
        unsafe { libc::free(self.0) };
    }
}
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
/// The first `<UDN>` in a root description is the root device's.
fn root_udn(xml: &[u8]) -> Option<String> {
    let start = find(xml, b"<UDN>")? + 5;
    let length = find(&xml[start..], b"</UDN>")?;
    let value = std::str::from_utf8(&xml[start..start + length]).ok()?.trim();
    (!value.is_empty() && value.len() <= 128 && value.bytes().all(|b| b.is_ascii_graphic()))
        .then(|| value.to_owned())
}
/// One HTTP fetch and one parse of a root description. `checked` holds it to
/// numeric LAN URLs; only the loopback CI fixture passes `false`.
fn describe(url: &str, checked: bool) -> Option<Description> {
    if checked && endpoint(url).is_none() {
        return None;
    }
    host(url)?;
    let source = CString::new(url).ok()?;
    let (mut size, mut status) = (0 as c_int, 0 as c_int);
    let mut local = [0 as c_char; 64];
    let body = Body(unsafe {
        miniwget_getaddr(
            source.as_ptr(),
            &mut size,
            local.as_mut_ptr(),
            local.len() as c_int,
            0,
            &mut status,
        )
    });
    if body.0.is_null() || status != 200 || size <= 0 {
        return None;
    }
    let xml = unsafe { std::slice::from_raw_parts(body.0 as *const u8, size as usize) };
    // All fields are char arrays and an integer; zero is upstream's own start.
    let mut data: Data = unsafe { std::mem::zeroed() };
    unsafe { parserootdesc(body.0 as *const c_char, size, &mut data) };
    Some(Description {
        data,
        local: text(&local)?.parse().ok()?,
        udn: root_udn(xml).unwrap_or_default(),
    })
}

struct Gateway {
    urls: Urls,
    data: Data,
    url: String,
    /// Our address as the gateway sees us, when its description was fetched.
    local: Ipv4Addr,
    /// The internal client mappings point at.
    client: Ipv4Addr,
    udn: String,
    key: GatewayKey,
    checked: bool,
}
impl Drop for Gateway {
    fn drop(&mut self) {
        unsafe { FreeUPNPUrls(&mut self.urls) };
    }
}
impl Gateway {
    fn open(url: &str, secret: &Secret, checked: bool, abort: &dyn Fn() -> bool) -> Option<Self> {
        let description = describe(url, checked)?;
        let device = host(url)?;
        let key = gateway_key(secret, &description.udn, device)?;
        let source = CString::new(url).ok()?;
        // The second WAN service from the same parse, not a second download.
        for alternate in [false, true] {
            if abort() {
                return None;
            }
            let mut data = description.data;
            if alternate {
                std::mem::swap(&mut data.first, &mut data.second);
                if text(&data.first.kind).map_or(true, |kind| kind.is_empty()) {
                    break;
                }
            }
            let mut gateway = Self {
                urls: unsafe { std::mem::zeroed() },
                data,
                url: url.to_owned(),
                local: description.local,
                client: description.local,
                udn: description.udn.clone(),
                key: key.clone(),
                checked,
            };
            unsafe { GetUPNPUrls(&mut gateway.urls, &mut gateway.data, source.as_ptr(), 0) };
            if gateway.usable(device) {
                return Some(gateway);
            }
        }
        None
    }
    fn usable(&self, device: Ipv4Addr) -> bool {
        if (self.checked && !lan(self.local)) || self.urls.control.is_null() {
            return false;
        }
        let Ok(control) = unsafe { CStr::from_ptr(self.urls.control) }.to_str() else {
            return false;
        };
        if host(control) != Some(device) || (self.checked && endpoint(control).is_none()) {
            return false;
        }
        if !text(&self.data.first.kind).is_some_and(|kind| SERVICES.contains(&kind.as_str())) {
            return false;
        }
        let (mut status, mut error, mut uptime) = ([0; 64], [0; 64], 0);
        (unsafe {
            UPNP_GetStatusInfo(
                self.urls.control,
                self.data.first.kind.as_ptr(),
                status.as_mut_ptr(),
                &mut uptime,
                error.as_mut_ptr(),
            )
        }) == 0
            && text(&status).as_deref() == Some("Connected")
    }
}
impl Igd for Gateway {
    fn key(&self) -> &GatewayKey {
        &self.key
    }
    fn client(&self) -> Ipv4Addr {
        self.client
    }
    fn query(&self, rule: &Rule) -> Result<Mapping, i32> {
        let (mut client, mut port, mut description, mut enabled, mut lease) =
            ([0; 16], [0; 6], [0; 80], [0; 4], [0; 16]);
        let result = unsafe {
            UPNP_GetSpecificPortMappingEntry(
                self.urls.control,
                self.data.first.kind.as_ptr(),
                c(&rule.port.to_string()).as_ptr(),
                c(rule.protocol).as_ptr(),
                ptr::null(),
                client.as_mut_ptr(),
                port.as_mut_ptr(),
                description.as_mut_ptr(),
                enabled.as_mut_ptr(),
                lease.as_mut_ptr(),
            )
        };
        if result != 0 {
            return Err(result);
        }
        Ok(Mapping {
            client: text(&client).ok_or(-1)?.parse().map_err(|_| -1)?,
            port: text(&port).ok_or(-1)?.parse().map_err(|_| -1)?,
            description: text(&description).ok_or(-1)?,
            enabled: text(&enabled).as_deref() == Some("1"),
            lease: text(&lease).and_then(|value| value.trim().parse().ok()),
        })
    }
    fn add(&self, rule: &Rule, lease: &str) -> i32 {
        let port = c(&rule.port.to_string());
        unsafe {
            UPNP_AddPortMapping(
                self.urls.control,
                self.data.first.kind.as_ptr(),
                port.as_ptr(),
                port.as_ptr(),
                c(&rule.client.to_string()).as_ptr(),
                c(&self.key.label).as_ptr(),
                c(rule.protocol).as_ptr(),
                ptr::null(),
                c(lease).as_ptr(),
            )
        }
    }
    fn delete(&self, rule: &Rule) -> i32 {
        unsafe {
            UPNP_DeletePortMapping(
                self.urls.control,
                self.data.first.kind.as_ptr(),
                c(&rule.port.to_string()).as_ptr(),
                c(rule.protocol).as_ptr(),
                ptr::null(),
            )
        }
    }
    fn external(&self) -> Option<Ipv4Addr> {
        let mut external = [0; 16];
        if unsafe {
            UPNP_GetExternalIPAddress(
                self.urls.control,
                self.data.first.kind.as_ptr(),
                external.as_mut_ptr(),
            )
        } != 0
        {
            return None;
        }
        text(&external)?.parse().ok()
    }
    fn same_device(&self) -> bool {
        describe(&self.url, self.checked).is_some_and(|now| {
            now.local == self.local
                && now.udn == self.udn
                && now.data.base == self.data.base
                && (now.data.first.control == self.data.first.control
                    || now.data.second.control == self.data.first.control)
        })
    }
}

struct Devices(*mut Device);
impl Drop for Devices {
    fn drop(&mut self) {
        unsafe { freeUPNPDevlist(self.0) };
    }
}
fn discover(secret: &Secret, abort: &dyn Fn() -> bool) -> Option<Gateway> {
    // Empty daemon path selects bounded local SSDP, not an optional minissdpd
    // service that could omit the packet origin or block without our deadline.
    let daemon = c("");
    let mut candidates = HashSet::new();
    // One search type per call, so a shutdown waits out at most one window.
    for kind in SEARCH {
        if abort() || candidates.len() >= CANDIDATES {
            break;
        }
        let kind = c(kind);
        let kinds = [kind.as_ptr(), ptr::null()];
        let mut error = 0;
        let devices = Devices(unsafe {
            upnpDiscoverDevices(
                kinds.as_ptr(),
                DISCOVERY_DELAY_MS,
                ptr::null(),
                daemon.as_ptr(),
                0,
                0,
                2,
                &mut error,
                0,
            )
        });
        let mut current = devices.0;
        for _ in 0..64 {
            if current.is_null() || abort() {
                break;
            }
            let item = unsafe { &*current };
            if !item.url.is_null() && candidates.len() < CANDIDATES {
                if let Ok(url) = unsafe { CStr::from_ptr(item.url) }.to_str() {
                    if endpoint(url).is_some() && candidates.insert(url.to_string()) {
                        if let Some(gateway) = Gateway::open(url, secret, true, abort) {
                            return Some(gateway);
                        }
                    }
                }
            }
            current = item.next;
        }
    }
    None
}

struct Native;
impl Env for Native {
    type Gateway = Gateway;
    fn ports(&mut self) -> Ports {
        let (mut tcp, mut udp) = (0, 0);
        match unsafe { gipny_router_upnp_ports(&mut tcp, &mut udp) } {
            1 => Ports::Active { tcp, udp },
            0 => Ports::Disabled,
            _ => Ports::Stopped,
        }
    }
    fn discover(&mut self, secret: &Secret, abort: &dyn Fn() -> bool) -> Option<Gateway> {
        discover(secret, abort)
    }
    fn publish(&mut self, external: Ipv4Addr) {
        unsafe { gipny_router_upnp_external(c(&external.to_string()).as_ptr()) };
    }
}

#[derive(Default)]
struct Flags {
    /// When exit asked the worker to stop.
    stop: Option<Instant>,
    offline: bool,
    revision: u64,
}
#[derive(Debug, PartialEq, Eq)]
enum Woke {
    Stop,
    Timeout,
    Network,
}
#[derive(Default)]
struct Control {
    flags: Mutex<Flags>,
    changed: Condvar,
}
impl Control {
    fn stopping(&self) -> bool {
        self.flags.lock().map_or(true, |f| f.stop.is_some())
    }
    fn stopped_at(&self) -> Option<Instant> {
        self.flags.lock().ok().and_then(|f| f.stop)
    }
    fn request_stop(&self) {
        if let Ok(mut flags) = self.flags.lock() {
            flags.stop.get_or_insert_with(Instant::now);
        }
        self.changed.notify_all();
    }
    /// What the host says about the network. Online while already online is
    /// no news for the gateway and does not wake the worker.
    fn report(&self, online: bool) {
        if let Ok(mut flags) = self.flags.lock() {
            if online && !flags.offline {
                return;
            }
            flags.offline = !online;
            flags.revision = flags.revision.wrapping_add(1);
        }
        self.changed.notify_all();
    }
    fn wait(&self, seen: u64, timeout: Duration) -> Woke {
        let Ok(flags) = self.flags.lock() else {
            return Woke::Stop;
        };
        let Ok((flags, _)) = self
            .changed
            .wait_timeout_while(flags, timeout, |f| f.stop.is_none() && f.revision == seen)
        else {
            return Woke::Stop;
        };
        if flags.stop.is_some() {
            Woke::Stop
        } else if flags.revision != seen {
            Woke::Network
        } else {
            Woke::Timeout
        }
    }
    /// Wait until the host has been quiet for `quiet` (or has kept talking for
    /// a while), then report where it ended up. `None`: stop instead.
    fn settle(&self, quiet: Duration) -> Option<(bool, u64)> {
        let started = Instant::now();
        let mut flags = self.flags.lock().ok()?;
        loop {
            if flags.stop.is_some() {
                return None;
            }
            let revision = flags.revision;
            let (next, timeout) = self
                .changed
                .wait_timeout_while(flags, quiet, |f| f.stop.is_none() && f.revision == revision)
                .ok()?;
            flags = next;
            if flags.stop.is_some() {
                return None;
            }
            if timeout.timed_out() || started.elapsed() >= quiet * 5 {
                return Some((flags.offline, flags.revision));
            }
        }
    }
}
#[derive(Default)]
struct Manager {
    control: Arc<Control>,
    thread: Mutex<Option<JoinHandle<()>>>,
}
fn manager() -> &'static Manager {
    static INSTANCE: OnceLock<Manager> = OnceLock::new();
    INSTANCE.get_or_init(Manager::default)
}

extern "C" fn network_hook(online: c_int) {
    // JNI invokes the C shim directly; route that signal to the same manager.
    let _ = std::panic::catch_unwind(|| manager().control.report(online != 0));
}
extern "C" fn stop_hook() {
    // Never let a poisoned Rust lock/unwind cross the C exit callback.
    let _ = std::panic::catch_unwind(|| {
        let m = manager();
        m.control.request_stop();
        // An exchange already in flight gives up at its next received chunk.
        unsafe { gipny_router_upnp_abort() };
        if let Ok(mut thread) = m.thread.lock() {
            if let Some(thread) = thread.take() {
                // Joined before StopI2P either way: the worker reads transports.
                let started = Instant::now();
                while !thread.is_finished() && started.elapsed() < SHUTDOWN_BUDGET {
                    std::thread::sleep(Duration::from_millis(10));
                }
                if !thread.is_finished() {
                    unsafe { gipny_router_upnp_abort() };
                }
                let _ = thread.join();
            }
        }
    });
}
pub(crate) fn start(directory: Option<&Path>) {
    let (mut tcp, mut udp) = (0, 0);
    // Off in settings (upnp.enabled=false) or hidden: no worker, no files and
    // not a single packet to the LAN.
    if unsafe { gipny_router_upnp_ports(&mut tcp, &mut udp) } != 1 {
        return;
    }
    let Some(directory) = directory else {
        return;
    };
    let registry = match Registry::open(directory) {
        Ok(value) => value,
        Err(error) => {
            crate::debug_error!("[upnp] ownership state unavailable: {error}");
            return;
        }
    };
    let m = manager();
    let Ok(mut handle) = m.thread.lock() else {
        return;
    };
    if handle.is_some() {
        return;
    }
    if let Ok(mut flags) = m.control.flags.lock() {
        flags.stop = None;
    }
    unsafe { gipny_router_network_hook(network_hook) };
    let control = m.control.clone();
    match std::thread::Builder::new()
        .name("gipny-upnp".into())
        .spawn(move || run(control, registry))
    {
        Ok(thread) => {
            *handle = Some(thread);
            unsafe { gipny_router_stop_hook(stop_hook) };
        }
        Err(error) => crate::debug_error!("[upnp] worker start failed: {error}"),
    }
}

fn run(control: Arc<Control>, registry: Registry) {
    let mut session: Session<Gateway> = Session::new(registry, Instant::now());
    let mut seen = match control.flags.lock() {
        Ok(flags) => {
            session.offline = flags.offline;
            flags.revision
        }
        Err(_) => return,
    };
    let mut env = Native;
    let mut wait = Duration::ZERO;
    loop {
        match control.wait(seen, wait) {
            Woke::Stop => break,
            Woke::Timeout => {}
            Woke::Network => match control.settle(SETTLE) {
                None => break,
                Some((offline, revision)) => {
                    seen = revision;
                    session.network(offline, Instant::now());
                }
            },
        }
        let abort = || control.stopping();
        wait = session.cycle(&mut env, Instant::now(), &abort);
    }
    let deadline = control.stopped_at().unwrap_or_else(Instant::now) + SHUTDOWN_BUDGET;
    session.shutdown(&|| Instant::now() + SHUTDOWN_MARGIN >= deadline);
}

/// A directory under the system temporary directory, removed on drop.
#[cfg(test)]
struct Scratch(PathBuf);
#[cfg(test)]
impl Scratch {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "gipny-upnp-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
#[cfg(test)]
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
#[path = "upnp_fixture.rs"]
mod fixture;

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};
    use std::rc::Rc;

    extern "C" {
        fn gipny_ssdp_location_matches(
            location: *const c_char,
            size: c_int,
            source: *const c_char,
        ) -> c_int;
    }

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
                (Ipv4Addr::new(192, 168, 1, 99), "other application".into(), 0),
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
            if state.entries.get(&key).is_some_and(|(client, _, _)| *client != rule.client) {
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

    #[test]
    fn ssdp_description_must_belong_to_the_reply_source() {
        for (url, allowed) in [
            ("http://192.168.1.1/root.xml", true),
            ("http://192.168.1.1:5000/root.xml", true),
            ("http://192.168.1.10/root.xml", false),
            ("http://192.168.1.1.evil.test/root.xml", false),
            ("http://192.168.1.1@192.168.1.2/root.xml", false),
            ("http://router.local/root.xml", false),
            ("http://192.168.1.2/root.xml", false),
            ("https://192.168.1.1/root.xml", false),
        ] {
            let result = unsafe {
                gipny_ssdp_location_matches(
                    c(url).as_ptr(),
                    url.len() as c_int,
                    c("192.168.1.1").as_ptr(),
                )
            };
            assert_eq!(result != 0, allowed, "{url}");
        }
    }
    #[test]
    fn only_numeric_lan_description_and_same_device_control_urls_are_allowed() {
        assert_eq!(endpoint("http://192.168.1.1:5000/root.xml"), Some(HOME));
        for url in [
            "http://localhost/x",
            "http://127.0.0.1/x",
            "http://8.8.8.8/x",
            "http://[::1]/x",
            "https://192.168.1.1/x",
            "http://192.168.1.1@8.8.8.8/x",
            "http://192.168.1.1:0/x",
            "http://192.168.1.1/x\r\nHeader: evil",
        ] {
            assert_eq!(endpoint(url), None, "{url}");
        }
        assert_ne!(
            endpoint("http://192.168.1.1/a"),
            endpoint("http://192.168.1.2/b")
        );
        assert!(public(WAN));
        for ip in ["10.0.0.1", "100.64.0.1", "192.0.2.1", "127.0.0.1", "0.0.0.0", "224.0.0.1"] {
            assert!(!public(ip.parse().unwrap()), "{ip}");
        }
    }
    #[test]
    fn create_only_after_explicit_absence_and_never_overwrite_foreign_rules() {
        let label = "UPnP 0123456789";
        let rule = tcp();
        assert_eq!(decision(&Err(714), &rule, label), Decision::Create);
        for error in [-1, 402, 501, 606, 718] {
            assert_eq!(decision(&Err(error), &rule, label), Decision::Retry);
        }
        let own = || Mapping {
            client: rule.client,
            port: rule.port,
            description: label.into(),
            enabled: true,
            lease: Some(600),
        };
        assert_eq!(decision(&Ok(own()), &rule, label), Decision::Renew);
        let mut foreign = own();
        foreign.description = "other application".into();
        assert_eq!(decision(&Ok(foreign), &rule, label), Decision::Foreign);
        let mut foreign = own();
        foreign.client = Ipv4Addr::new(192, 168, 1, 21);
        assert_eq!(decision(&Ok(foreign), &rule, label), Decision::Foreign);
        let mut foreign = own();
        foreign.port += 1;
        assert_eq!(decision(&Ok(foreign), &rule, label), Decision::Foreign);
    }
    #[test]
    fn per_gateway_tags_are_stable_short_neutral_and_unlinkable() {
        // RFC 4231 test case 2: the shim's HMAC is plain HMAC-SHA256.
        assert_eq!(
            hex(&hmac(b"Jefe", b"what do ya want for nothing?").unwrap()),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        let (one, two) = ([1u8; 16], [2u8; 16]);
        let home = gateway_key(&one, "uuid:home", HOME).unwrap();
        assert_eq!(home, gateway_key(&one, "uuid:home", HOME).unwrap(), "stable");
        for other in [
            gateway_key(&one, "uuid:office", HOME).unwrap(),
            gateway_key(&one, "uuid:home", Ipv4Addr::new(10, 0, 0, 1)).unwrap(),
            gateway_key(&two, "uuid:home", HOME).unwrap(),
            gateway_key(&one, "", HOME).unwrap(),
        ] {
            assert_ne!(home.id, other.id);
            assert_ne!(home.label, other.label);
            assert!(!owned_label(&other.label, &home.label));
        }
        assert!(home.label.len() <= 16, "{}", home.label);
        assert!(home.label.starts_with(LABEL));
        assert!(!home.label.to_ascii_lowercase().contains("gipny"));
        assert!(!home.label.contains(&home.id[..TAG_MATCH]), "label is not the journal id");
        // Routers that truncate descriptions still return ours as ours.
        assert!(owned_label(&home.label, &home.label));
        assert!(owned_label(&format!("{} ", home.label), &home.label));
        assert!(owned_label(&home.label[..LABEL.len() + TAG_MATCH], &home.label));
        assert!(!owned_label(&home.label[..LABEL.len() + TAG_MATCH - 1], &home.label));
        assert!(!owned_label("", &home.label));
        assert!(!owned_label(LABEL, &home.label));
    }
    #[test]
    fn root_udn_is_read_from_the_same_bytes_and_bounded() {
        let xml = b"<root><device><UDN> uuid:abc-1 </UDN><deviceList><device><UDN>uuid:inner</UDN></device></deviceList></device></root>";
        assert_eq!(root_udn(xml).as_deref(), Some("uuid:abc-1"));
        assert_eq!(root_udn(b"<root></root>"), None);
        assert_eq!(root_udn(b"<UDN>uuid:x"), None);
        assert_eq!(root_udn(b"<UDN>uuid:\x01x</UDN>"), None);
        assert_eq!(root_udn(format!("<UDN>{}</UDN>", "a".repeat(129)).as_bytes()), None);
    }
    #[test]
    fn corrupt_or_missing_owner_file_is_regenerated_atomically() {
        let scratch = Scratch::new();
        let owner = scratch.0.join("upnp-owner");
        let journal = scratch.0.join("upnp-mappings");
        let first = Registry::open(&scratch.0).unwrap().secret;
        assert_eq!(parse_secret(&read_record(&owner, 64).unwrap()), Some(first));
        assert_eq!(Registry::open(&scratch.0).unwrap().secret, first, "kept when valid");
        let gateway = "0".repeat(32);
        let (short, long, wrong) = ("a".repeat(31), "a".repeat(33), "g".repeat(32));
        for broken in ["", "zz", short.as_str(), long.as_str(), wrong.as_str()] {
            std::fs::write(&owner, broken).unwrap();
            std::fs::write(&journal, format!("TCP 34567 192.168.1.20 {gateway}\n")).unwrap();
            let registry = Registry::open(&scratch.0).unwrap();
            assert_ne!(registry.secret, first, "{broken:?}");
            assert_eq!(parse_secret(&read_record(&owner, 64).unwrap()), Some(registry.secret));
            assert!(registry.entries.is_empty(), "old entries are unattributable");
            assert_eq!(read_record(&journal, 4096).unwrap(), "");
        }
        std::fs::write(&owner, vec![0xff; 32]).unwrap();
        assert!(Registry::open(&scratch.0).is_ok(), "not UTF-8");
        std::fs::remove_file(&owner).unwrap();
        std::fs::write(&owner, "b".repeat(4096)).unwrap();
        assert!(Registry::open(&scratch.0).is_ok(), "oversized");
        #[cfg(unix)]
        {
            let target = scratch.0.join("elsewhere");
            std::fs::write(&target, "c".repeat(32)).unwrap();
            std::fs::remove_file(&owner).unwrap();
            std::os::unix::fs::symlink(&target, &owner).unwrap();
            let registry = Registry::open(&scratch.0).unwrap();
            assert_ne!(registry.secret, [0xcc; 16], "a symlink is not followed");
            assert!(std::fs::symlink_metadata(&owner).unwrap().is_file());
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "c".repeat(32));
        }
        assert!(!scratch.0.join("upnp-owner.pending").exists());
    }
    #[test]
    fn journal_entries_are_bound_to_their_gateway() {
        let scratch = Scratch::new();
        let mut registry = Registry::open(&scratch.0).unwrap();
        let home = Fake::new(&registry.secret, "uuid:home");
        let office = Fake::new(&registry.secret, "uuid:office");
        let rule = tcp();
        assert_eq!(maintain(&home, &rule, &mut registry, &never), Outcome::Confirmed { permanent: false });
        // The office gateway has never heard of this port (714), and must not
        // retire the home entry for it — or even be asked about it.
        cleanup_except(&office, &mut registry, &[], &never);
        assert_eq!(office.state.borrow().queries, 0);
        assert_eq!(registry.rules_on(&home.key.id), vec![rule.clone()]);
        let reopened = Registry::open(&scratch.0).unwrap();
        assert_eq!(reopened.entries, registry.entries);
        assert_eq!(reopened.entries[0].gateway, home.key.id);
        // The home gateway's own 714 does retire it.
        home.state.borrow_mut().entries.clear();
        cleanup_except(&home, &mut registry, &[], &never);
        assert!(registry.entries.is_empty());
        assert_eq!(home.state.borrow().deletes, 0, "nothing to delete");
        // A full journal makes room from other gateways, never from this one.
        for port in 1..=JOURNAL_LIMIT as u16 {
            assert!(registry.reserve(&Rule { port, ..rule.clone() }, &home.key.id));
        }
        assert!(!registry.reserve(&Rule { port: 999, ..rule.clone() }, &home.key.id));
        assert!(registry.reserve(&Rule { port: 999, ..rule.clone() }, &office.key.id));
        assert_eq!(registry.entries.len(), JOURNAL_LIMIT);
        assert_eq!(registry.rules_on(&home.key.id).len(), JOURNAL_LIMIT - 1);
        assert!(!registry.rules_on(&home.key.id).iter().any(|r| r.port == 1), "oldest evicted");
    }
    #[test]
    fn focus_and_network_churn_never_delete_or_recreate_mappings() {
        let scratch = Scratch::new();
        let registry = Registry::open(&scratch.0).unwrap();
        let home = Fake::new(&registry.secret, "uuid:home");
        let mut env = FakeEnv::new(&[&home]);
        let t0 = Instant::now();
        let mut session = Session::new(registry, t0);
        assert_eq!(session.cycle(&mut env, t0, &never), RENEW);
        assert_eq!(home.state.borrow().entries.len(), 2);
        let (adds, queries) = (home.state.borrow().adds, home.state.borrow().queries);
        // A burst of focus events: at most one look per RECHECK, and a look is
        // one root-description fetch, no SOAP at all.
        for second in 1..=20 {
            let now = t0 + Duration::from_secs(second);
            session.network(false, now);
            assert!(session.cycle(&mut env, now, &never) > Duration::ZERO);
        }
        assert_eq!(home.state.borrow().verifies, 0, "rate-limited");
        let now = t0 + RECHECK;
        assert_eq!(session.cycle(&mut env, now, &never), RENEW - RECHECK);
        // Offline then online again (Android switching networks back).
        session.network(true, now + Duration::from_secs(5));
        assert_eq!(session.cycle(&mut env, now + Duration::from_secs(5), &never), BACKOFF_CAP);
        session.network(false, now + Duration::from_secs(6));
        let later = now + RECHECK;
        session.cycle(&mut env, later, &never);
        let state = home.state.borrow();
        assert_eq!(state.verifies, 2);
        assert_eq!((state.adds, state.queries, state.deletes), (adds, queries, 0));
        assert_eq!(state.entries.len(), 2);
        drop(state);
        assert_eq!(env.searches, 1, "the gateway was kept");
        // The scheduled pass verifies and renews in place.
        session.cycle(&mut env, t0 + RENEW, &never);
        assert_eq!(home.state.borrow().deletes, 0);
        assert_eq!(home.state.borrow().verifies, 3);
        assert_eq!(env.published, vec![WAN], "published once");
    }
    #[test]
    fn a_different_gateway_is_left_alone_and_the_journal_keeps_both() {
        let scratch = Scratch::new();
        let registry = Registry::open(&scratch.0).unwrap();
        let home = Fake::new(&registry.secret, "uuid:home");
        let office = Fake::new(&registry.secret, "uuid:office");
        let mut env = FakeEnv::new(&[&home, &office]);
        let t0 = Instant::now();
        let mut session = Session::new(registry, t0);
        session.cycle(&mut env, t0, &never);
        home.state.borrow_mut().moved = true;
        let now = t0 + RECHECK;
        session.network(false, now);
        session.cycle(&mut env, now, &never);
        assert_eq!(env.searches, 2);
        assert_eq!(home.state.borrow().deletes, 0, "old gateway untouched");
        assert_eq!(office.state.borrow().entries.len(), 2);
        let labels: HashSet<_> = office.state.borrow().entries.values().map(|e| e.1.clone()).collect();
        assert_eq!(labels, HashSet::from([office.key.label.clone()]));
        assert_eq!(session.registry.rules_on(&home.key.id).len(), 2);
        assert_eq!(session.registry.rules_on(&office.key.id).len(), 2);
    }
    #[test]
    fn foreign_ports_and_one_failing_rule_keep_the_gateway_and_healthy_mappings() {
        let scratch = Scratch::new();
        let registry = Registry::open(&scratch.0).unwrap();
        let home = Fake::new(&registry.secret, "uuid:home");
        home.foreign("UDP", 34568);
        let mut env = FakeEnv::new(&[&home]);
        let t0 = Instant::now();
        let mut session = Session::new(registry, t0);
        for pass in 0..4u32 {
            session.cycle(&mut env, t0 + RENEW * pass, &never);
        }
        assert_eq!(env.searches, 1);
        assert_eq!(home.state.borrow().deletes, 0);
        assert_eq!(home.state.borrow().entries[&("UDP", 34568)].1, "other application");
        assert_eq!(session.states[1].1, Outcome::Foreign);
        // A SOAP error on one rule: the other one stays, so does the gateway.
        home.state.borrow_mut().failing.insert("UDP");
        for pass in 4..8u32 {
            assert_eq!(session.cycle(&mut env, t0 + RENEW * pass, &never), RETRY);
        }
        assert_eq!(env.searches, 1);
        assert_eq!(home.state.borrow().deletes, 0);
        // Everything failing twice in a row: forget the handle, delete nothing.
        home.state.borrow_mut().failing.insert("TCP");
        session.cycle(&mut env, t0 + RENEW * 8, &never);
        assert!(session.gateway.is_some());
        session.cycle(&mut env, t0 + RENEW * 9, &never);
        assert!(session.gateway.is_none());
        assert_eq!(home.state.borrow().deletes, 0);
        assert!(home.state.borrow().entries.contains_key(&("TCP", 34567)));
    }
    #[test]
    fn conflict_718_is_settled_by_looking_again() {
        let scratch = Scratch::new();
        let mut registry = Registry::open(&scratch.0).unwrap();
        let home = Fake::new(&registry.secret, "uuid:home");
        let rule = tcp();
        home.state.borrow_mut().entries.insert(("TCP", 34567), (ME, home.key.label.clone(), 0));
        home.state.borrow_mut().add_result = Some(718);
        assert_eq!(maintain(&home, &rule, &mut registry, &never), Outcome::Confirmed { permanent: true });
        home.state.borrow_mut().entries.insert(("TCP", 34567), (ME, "UPnP ffffffffff".into(), 0));
        assert_eq!(maintain(&home, &rule, &mut registry, &never), Outcome::Foreign);
        home.state.borrow_mut().entries.clear();
        assert_eq!(maintain(&home, &rule, &mut registry, &never), Outcome::Failed);
    }
    #[test]
    fn external_address_is_published_only_after_a_confirmed_mapping() {
        let scratch = Scratch::new();
        let registry = Registry::open(&scratch.0).unwrap();
        let home = Fake::new(&registry.secret, "uuid:home");
        home.foreign("TCP", 34567);
        home.foreign("UDP", 34568);
        let mut env = FakeEnv::new(&[&home]);
        let t0 = Instant::now();
        let mut session = Session::new(registry, t0);
        session.cycle(&mut env, t0, &never);
        assert!(env.published.is_empty());
        home.state.borrow_mut().failing.extend(["TCP", "UDP"]);
        session.cycle(&mut env, t0 + RENEW, &never);
        assert!(env.published.is_empty());
        home.state.borrow_mut().failing.clear();
        home.state.borrow_mut().entries.clear();
        session.cycle(&mut env, t0 + RENEW + RETRY, &never);
        session.cycle(&mut env, t0 + RENEW * 2 + RETRY, &never);
        assert_eq!(env.published, vec![WAN]);
    }
    #[test]
    fn shutdown_removes_only_permanent_leases_and_honours_the_deadline() {
        let scratch = Scratch::new();
        let registry = Registry::open(&scratch.0).unwrap();
        let finite = Fake::new(&registry.secret, "uuid:finite");
        let mut env = FakeEnv::new(&[&finite]);
        let t0 = Instant::now();
        let mut session = Session::new(registry, t0);
        session.cycle(&mut env, t0, &never);
        let queries = finite.state.borrow().queries;
        session.shutdown(&never);
        assert_eq!(finite.state.borrow().queries, queries, "finite leases are not touched");
        assert_eq!(finite.state.borrow().entries.len(), 2);
        assert_eq!(session.registry.rules_on(&finite.key.id).len(), 2, "kept for next start");

        let permanent = Fake::new(&session.registry.secret, "uuid:permanent");
        permanent.state.borrow_mut().permanent_only = true;
        let mut env = FakeEnv::new(&[&permanent]);
        let mut session = Session::new(Registry::open(&scratch.0).unwrap(), t0);
        session.cycle(&mut env, t0, &never);
        assert_eq!(session.states.iter().filter(|s| s.1 == Outcome::Confirmed { permanent: true }).count(), 2);
        let exhausted = || true;
        session.shutdown(&exhausted);
        assert_eq!(permanent.state.borrow().deletes, 0, "deadline passed: no new exchange");

        let mut env = FakeEnv::new(&[&permanent]);
        let mut session = Session::new(Registry::open(&scratch.0).unwrap(), t0);
        session.cycle(&mut env, t0, &never);
        session.shutdown(&never);
        assert_eq!(permanent.state.borrow().deletes, 2);
        assert!(permanent.state.borrow().entries.is_empty());
        assert!(session.registry.rules_on(&permanent.key.id).is_empty());
        assert_eq!(session.registry.rules_on(&finite.key.id).len(), 2);
    }
    #[test]
    fn discovery_backs_off_and_resets_on_network_change() {
        let schedule: Vec<u64> = (1..=8).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(schedule, [60, 120, 240, 480, 960, 1800, 1800, 1800]);
        assert_eq!(backoff(u32::MAX), BACKOFF_CAP);
        let scratch = Scratch::new();
        let mut env = FakeEnv::new(&[]);
        let t0 = Instant::now();
        let mut session: Session<Fake> = Session::new(Registry::open(&scratch.0).unwrap(), t0);
        let mut now = t0;
        for expected in [60, 120, 240, 480] {
            let wait = session.cycle(&mut env, now, &never);
            assert_eq!(wait.as_secs(), expected);
            // Nothing happens before it is due.
            assert_eq!(session.cycle(&mut env, now + Duration::from_secs(1), &never).as_secs(), expected - 1);
            now += wait;
        }
        assert_eq!(session.cycle(&mut env, now, &never).as_secs(), 960);
        assert_eq!(env.searches, 5);
        session.network(false, now + Duration::from_secs(1));
        let wait = session.cycle(&mut env, now + Duration::from_secs(1), &never);
        assert_eq!(wait.as_secs(), 29, "not before RECHECK after the last search");
        assert_eq!(session.cycle(&mut env, now + RECHECK, &never), RETRY, "backoff restarted");
        assert_eq!(env.searches, 6);
    }
    #[test]
    fn disabled_listeners_remove_our_mappings_but_a_stopping_router_does_not() {
        let scratch = Scratch::new();
        let registry = Registry::open(&scratch.0).unwrap();
        let home = Fake::new(&registry.secret, "uuid:home");
        let mut env = FakeEnv::new(&[&home]);
        let t0 = Instant::now();
        let mut session = Session::new(registry, t0);
        session.cycle(&mut env, t0, &never);
        env.ports = Ports::Stopped;
        session.cycle(&mut env, t0 + RENEW, &never);
        assert_eq!(home.state.borrow().deletes, 0);
        env.ports = Ports::Disabled;
        session.cycle(&mut env, t0 + RENEW, &never);
        assert_eq!(home.state.borrow().deletes, 2);
        assert!(session.registry.entries.is_empty());
    }
    #[test]
    fn host_notifications_are_debounced() {
        let control = Control::default();
        control.report(true);
        control.report(true);
        assert_eq!(control.flags.lock().unwrap().revision, 0, "online→online is no news");
        assert_eq!(control.wait(0, Duration::from_millis(1)), Woke::Timeout);
        control.report(false);
        control.report(true);
        assert_eq!(control.wait(0, Duration::from_millis(1)), Woke::Network);
        assert_eq!(control.settle(Duration::from_millis(20)), Some((false, 2)));
        control.request_stop();
        assert_eq!(control.settle(Duration::from_millis(20)), None);
        assert_eq!(control.wait(2, Duration::from_secs(5)), Woke::Stop);
        assert!(control.stopping());
    }
}
