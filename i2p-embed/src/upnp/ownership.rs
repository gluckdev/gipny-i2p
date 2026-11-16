//! Per-gateway ownership tags and the durable mapping journal.

use std::io::{Read, Write};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use super::ffi::{gipny_router_upnp_hmac, gipny_router_upnp_random};
use super::mapping::Rule;
use super::protocol::lan;

pub(super) const JOURNAL_LIMIT: usize = 16;
/// What a mapping is called on the gateway: a generic word and a per-gateway
/// tag, 15 characters, short enough that few routers truncate it.
pub(super) const LABEL: &str = "UPnP ";
const TAG_HEX: usize = 10;
/// Tag digits that must survive a router truncating descriptions before a
/// rule is treated as ours.
pub(super) const TAG_MATCH: usize = 8;
pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) fn hmac(key: &[u8], input: &[u8]) -> Option<[u8; 32]> {
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

pub(super) type Secret = [u8; 16];

/// Who we are on one gateway. Derived from the local secret and that gateway's
/// identity, so the tag a router (or whoever manages it) sees cannot be matched
/// with the tag on any other network, while it stays the same on this one
/// across restarts — which is what lets a later start clean up after a crash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GatewayKey {
    /// Binds journal entries to this gateway. Never leaves this machine.
    pub(super) id: String,
    /// The description our mappings carry on this gateway.
    pub(super) label: String,
}
/// `udn` is the root device's UDN (empty when it publishes none); `device` is
/// its LAN address. A gateway whose UDN is not unique per unit can only be told
/// apart from another of its model by that address.
pub(super) fn gateway_key(secret: &Secret, udn: &str, device: Ipv4Addr) -> Option<GatewayKey> {
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
pub(super) fn owned_label(found: &str, ours: &str) -> bool {
    let found = found.trim_end();
    found.len() >= LABEL.len() + TAG_MATCH && ours.starts_with(found)
}

fn record_error(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.to_owned())
}
pub(super) fn read_record(path: &Path, limit: u64) -> std::io::Result<String> {
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
pub(super) fn parse_secret(value: &str) -> Option<Secret> {
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
pub(super) struct Entry {
    pub(super) rule: Rule,
    /// [`GatewayKey::id`] of the gateway the rule was requested from.
    pub(super) gateway: String,
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
        if let (Ok(port), Ok(client)) = (fields[1].parse::<u16>(), fields[2].parse::<Ipv4Addr>()) {
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
pub(super) struct Registry {
    pub(super) path: PathBuf,
    pub(super) secret: Secret,
    pub(super) entries: Vec<Entry>,
    /// The file is behind memory (a save failed or an entry changed).
    dirty: bool,
}
impl Registry {
    pub(super) fn open(directory: &Path) -> std::io::Result<Self> {
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
    pub(super) fn flush(&mut self) {
        if self.dirty {
            let _ = self.save();
        }
    }
    pub(super) fn rules_on(&self, gateway: &str) -> Vec<Rule> {
        self.entries
            .iter()
            .filter(|e| e.gateway == gateway)
            .map(|e| e.rule.clone())
            .collect()
    }
    pub(super) fn reserve(&mut self, rule: &Rule, gateway: &str) -> bool {
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
    pub(super) fn retire(&mut self, rule: &Rule, gateway: &str) {
        let before = self.entries.len();
        self.entries
            .retain(|e| !(e.rule == *rule && e.gateway == gateway));
        self.dirty |= self.entries.len() != before;
    }
}
