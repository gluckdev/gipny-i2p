//! Bounded ingestion of the release netDb tar.gz format with a public local cache.
//! The archive digest identifies the chosen snapshot; every usable RouterInfo
//! is independently signature/freshness/network checked before it reaches core.
//!
//! Besides the published snapshot, the routers this browser learned while
//! running are kept between sessions ([`LiveNetDb`]), so a restart is warm even
//! when the snapshot cannot be fetched or has outlived its 36 hours. Both are
//! public RouterInfos and go through the same per-record checks.
use crate::{ntcp2::verified_peer, BrowserRuntime};
use emissary_core::{
    crypto::{base64_encode, sha256::Sha256},
    runtime::Runtime,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    sync::Mutex,
    time::Duration,
};

pub const MAX_DOWNLOAD_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TAR_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_RECORDS: usize = 2048;
const MAX_ENTRIES: usize = 4096;

#[derive(Debug, Default, serde::Serialize)]
pub struct SnapshotReceipt {
    pub from_cache: bool,
    /// Actual seed source, not a claim that any tunnel is ready.
    pub source: String,
    pub local_ms: u128,
    pub compressed_bytes: usize,
    pub tar_bytes: usize,
    pub entries: usize,
    pub accepted: usize,
    pub rejected: usize,
    pub duplicates: usize,
    pub download_ms: u128,
    pub digest_ms: u128,
    pub inflate_ms: u128,
    pub parse_verify_ms: u128,
    pub wasm_memory_bytes_after_inflate: u64,
    pub wasm_memory_bytes_after_verify: u64,
}

pub struct Snapshot {
    pub routers: Vec<Vec<u8>>,
    pub receipt: SnapshotReceipt,
}

/// Fewer usable RouterInfos than this and an archive is not worth starting from.
pub const MIN_ROUTERS: usize = 16;

fn expected_digest(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("snapshot SHA256 must contain 64 hex digits".into());
    }
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| "invalid snapshot SHA256")?;
    }
    Ok(bytes)
}

fn inflate(bytes: &[u8], expected: Option<&str>) -> Result<(Vec<u8>, SnapshotReceipt), String> {
    if bytes.is_empty() || bytes.len() > MAX_DOWNLOAD_BYTES {
        return Err("snapshot compressed size limit".into());
    }
    let started = web_time::Instant::now();
    // The published snapshot is pinned by digest; the local live archive has
    // none, and relies on the per-record checks below like every archive.
    if let Some(expected) = expected {
        if Sha256::new().update(bytes).finalize_new() != expected_digest(expected)? {
            return Err("snapshot SHA256 mismatch".into());
        }
    }
    let digest_ms = started.elapsed().as_millis();
    let started = web_time::Instant::now();
    // MultiGzDecoder also validates CRC/trailers and consumes concatenated
    // members/trailing data instead of silently accepting an unchecked suffix.
    let mut decoder = flate2::read::MultiGzDecoder::new(bytes).take((MAX_TAR_BYTES + 1) as u64);
    let mut tar = Vec::new();
    decoder
        .read_to_end(&mut tar)
        .map_err(|_| "invalid snapshot gzip")?;
    if tar.len() > MAX_TAR_BYTES {
        return Err("snapshot decompression limit".into());
    }
    // A complete tar has whole blocks and two end-of-archive blocks. Reject
    // truncated downloads even if the last file happened to be complete.
    if tar.len() < 1024 || tar.len() % 512 != 0 || !tar[tar.len() - 1024..].iter().all(|b| *b == 0)
    {
        return Err("incomplete snapshot tar".into());
    }
    let receipt = SnapshotReceipt {
        compressed_bytes: bytes.len(),
        tar_bytes: tar.len(),
        digest_ms,
        inflate_ms: started.elapsed().as_millis(),
        wasm_memory_bytes_after_inflate: crate::router::wasm_memory_bytes(),
        ..SnapshotReceipt::default()
    };
    Ok((tar, receipt))
}

pub async fn verify_snapshot(bytes: &[u8], expected: &str) -> Result<Snapshot, String> {
    verify_archive(bytes, Some(expected)).await
}

/// As [`verify_snapshot`], for an archive not pinned by a published digest.
pub async fn verify_archive(bytes: &[u8], expected: Option<&str>) -> Result<Snapshot, String> {
    let (tar_bytes, mut receipt) = inflate(bytes, expected)?;
    let started = web_time::Instant::now();
    let mut archive = tar::Archive::new(tar_bytes.as_slice());
    let mut routers = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in archive
        .entries()
        .map_err(|_| "invalid snapshot tar")?
        .raw(true)
    {
        let mut entry = entry.map_err(|_| "invalid snapshot tar entry")?;
        receipt.entries += 1;
        if receipt.entries > MAX_ENTRIES {
            return Err("snapshot entry count limit".into());
        }
        // Never extract files. Only simple relative netDb paths and regular
        // RI files/directories are accepted; no links or archive extensions.
        let path = entry.path_bytes();
        if path.len() > 200
            || path.starts_with(b"/")
            || path.contains(&b'\\')
            || path.split(|b| *b == b'/').any(|p| p == b"..")
        {
            return Err("invalid snapshot path".into());
        }
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            continue;
        }
        if !kind.is_file() {
            return Err("snapshot contains a non-regular entry".into());
        }
        let name = path.rsplit(|b| *b == b'/').next().unwrap_or_default();
        if !name.starts_with(b"routerInfo-") || !name.ends_with(b".dat") {
            return Err("snapshot contains an unexpected file".into());
        }
        if entry.size() == 0 || entry.size() > 16 * 1024 {
            return Err("snapshot RouterInfo size limit".into());
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut bytes)
            .map_err(|_| "truncated RouterInfo")?;
        match verified_peer(&bytes) {
            Ok(peer) if is_public_ipv4(peer.address.ip()) => {
                let hash = peer.router.identity.hash().to_vec();
                if !seen.insert(hash) {
                    receipt.duplicates += 1;
                } else {
                    if routers.len() >= MAX_RECORDS {
                        return Err("snapshot accepted record limit".into());
                    }
                    routers.push(bytes);
                }
            }
            _ => receipt.rejected += 1,
        }
        if receipt.entries % 32 == 0 {
            // Let the browser process input/paint between verification batches.
            BrowserRuntime::delay(Duration::from_millis(1)).await;
        }
    }
    if routers.len() < MIN_ROUTERS {
        return Err(format!(
            "snapshot has too few usable RouterInfos: {}",
            routers.len()
        ));
    }
    receipt.accepted = routers.len();
    receipt.parse_verify_ms = started.elapsed().as_millis();
    receipt.wasm_memory_bytes_after_verify = crate::router::wasm_memory_bytes();
    Ok(Snapshot { routers, receipt })
}

/// A RouterInfo the browser can start from: signed, on the I2P network, fresh,
/// with a public IPv4 NTCP2 address. Returns its router id and publication time.
fn usable(bytes: &[u8]) -> Option<(String, u64)> {
    let peer = verified_peer(bytes).ok()?;
    is_public_ipv4(peer.address.ip()).then(|| {
        (
            base64_encode(peer.router.identity.id().to_vec()),
            *peer.router.published.date(),
        )
    })
}

/// Start from the published snapshot first (its routers are the ones the
/// bridge lets us dial), then fill up with what earlier sessions learned.
/// With no snapshot, the live set alone is enough if it is large enough.
pub fn merge_routers(
    snapshot: Option<Snapshot>,
    live: Option<Snapshot>,
) -> Result<Snapshot, String> {
    let (mut base, extra) = match (snapshot, live) {
        (Some(snapshot), live) => (snapshot, live.map(|l| l.routers).unwrap_or_default()),
        (None, Some(live)) => (live, Vec::new()),
        (None, None) => return Err("no usable netDb snapshot or saved routers".into()),
    };
    let mut seen: BTreeSet<String> = base
        .routers
        .iter()
        .filter_map(|b| usable(b).map(|(id, _)| id))
        .collect();
    for bytes in extra {
        if base.routers.len() >= MAX_RECORDS {
            break;
        }
        if let Some((id, _)) = usable(&bytes) {
            if seen.insert(id) {
                base.routers.push(bytes);
            }
        }
    }
    if base.routers.len() < MIN_ROUTERS {
        return Err(format!(
            "too few usable RouterInfos: {}",
            base.routers.len()
        ));
    }
    base.receipt.accepted = base.routers.len();
    Ok(base)
}

/// The routers this browser knows, kept for the next start.
///
/// emissary backs its profile storage up every 15 minutes of uptime through
/// [`emissary_core::runtime::Storage`]: it hands over only the RouterInfos
/// discovered since the last backup and names the routers it dropped. The full
/// set is therefore kept here, starting from what the router was built with,
/// and written out whole as a snapshot-format archive after each backup.
pub struct LiveNetDb {
    routers: Mutex<BTreeMap<String, (u64, Vec<u8>)>>,
    persist: Box<dyn Fn(Vec<u8>) + Send + Sync>,
}

impl LiveNetDb {
    pub fn new(initial: &[Vec<u8>], persist: impl Fn(Vec<u8>) + Send + Sync + 'static) -> Self {
        let routers = initial
            .iter()
            .filter_map(|bytes| usable(bytes).map(|(id, at)| (id, (at, bytes.clone()))))
            .collect();
        Self {
            routers: Mutex::new(routers),
            persist: Box::new(persist),
        }
    }

    pub fn len(&self) -> usize {
        self.routers.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// The current set as a gzip'd tar in the snapshot layout, newest first,
    /// within [`MAX_RECORDS`].
    pub fn archive(&self) -> Vec<u8> {
        let mut records: Vec<(String, u64, Vec<u8>)> = self
            .routers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(|(id, (at, bytes))| (id.clone(), *at, bytes.clone()))
            .collect();
        records.sort_by(|a, b| b.1.cmp(&a.1));
        records.truncate(MAX_RECORDS);
        let mut tar = tar::Builder::new(Vec::new());
        for (id, _, bytes) in &records {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            // The I2P base64 alphabet has no '/', so the id is a plain name.
            if tar
                .append_data(
                    &mut header,
                    format!("r{}/routerInfo-{id}.dat", &id[..1]),
                    bytes.as_slice(),
                )
                .is_err()
            {
                return Vec::new();
            }
        }
        let Ok(tar) = tar.into_inner() else {
            return Vec::new();
        };
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        if gzip.write_all(&tar).is_err() {
            return Vec::new();
        }
        gzip.finish().unwrap_or_default()
    }
}

impl emissary_core::runtime::Storage for LiveNetDb {
    fn save_to_disk(&self, routers: Vec<(String, Option<Vec<u8>>, emissary_core::Profile)>) {
        {
            let mut known = self.routers.lock().unwrap_or_else(|p| p.into_inner());
            for (id, bytes, _profile) in routers {
                let Some(bytes) = bytes else { continue };
                // Only what a later start could use; a newer copy replaces an older one.
                if let Some((usable_id, at)) = usable(&bytes) {
                    if usable_id == id && known.get(&id).is_none_or(|(old, _)| *old <= at) {
                        known.insert(id, (at, bytes));
                    }
                }
            }
        }
        let archive = self.archive();
        if !archive.is_empty() && archive.len() <= MAX_DOWNLOAD_BYTES {
            (self.persist)(archive);
        }
    }

    fn remove_from_disk(&self, routers: Vec<String>) {
        let mut known = self.routers.lock().unwrap_or_else(|p| p.into_inner());
        for id in routers {
            known.remove(&id);
        }
    }
}

/// Literal public IPv4 targets supported by the current browser transport.
pub fn is_public_ipv4(ip: std::net::IpAddr) -> bool {
    let std::net::IpAddr::V4(ip) = ip else {
        return false;
    };
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.is_unspecified()
        || a == 0
        || a >= 240
        || (a == 100 && (64..=127).contains(&b))
        || (a == 192 && b == 0 && (c == 0 || c == 2))
        || (a == 192 && b == 88 && c == 99)
        || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
        || (a == 203 && b == 0 && c == 113))
}

#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(target_arch = "wasm32")]
mod prewarm;

#[cfg(target_arch = "wasm32")]
pub use prewarm::PublicBootstrap;

#[cfg(target_arch = "wasm32")]
pub use browser::{bootstrap_routers, fetch_snapshot, live_netdb};
