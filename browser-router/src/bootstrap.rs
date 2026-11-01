//! Bounded, memory-only ingestion of the release netDb tar.gz format.
//! The archive digest identifies the chosen snapshot; every usable RouterInfo
//! is independently signature/freshness/network checked before it reaches core.
use crate::{ntcp2::verified_peer, BrowserRuntime};
use emissary_core::{crypto::sha256::Sha256, runtime::Runtime};
use std::{collections::BTreeSet, io::Read, time::Duration};

pub const MAX_DOWNLOAD_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TAR_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_RECORDS: usize = 2048;
const MAX_ENTRIES: usize = 4096;

#[derive(Debug, Default, serde::Serialize)]
pub struct SnapshotReceipt {
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

fn inflate(bytes: &[u8], expected: &str) -> Result<(Vec<u8>, SnapshotReceipt), String> {
    if bytes.is_empty() || bytes.len() > MAX_DOWNLOAD_BYTES {
        return Err("snapshot compressed size limit".into());
    }
    let started = web_time::Instant::now();
    if Sha256::new().update(bytes).finalize_new() != expected_digest(expected)? {
        return Err("snapshot SHA256 mismatch".into());
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
    if routers.len() < 16 {
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
mod browser {
    use super::*;
    use futures::future::{select, Either};
    use wasm_bindgen::{prelude::*, JsCast};
    use wasm_bindgen_futures::JsFuture;

    struct AbortFetch(web_sys::AbortController);
    impl Drop for AbortFetch {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    pub async fn fetch_snapshot(url: &str, digest: &str) -> Result<Snapshot, String> {
        expected_digest(digest)?;
        let parsed = url::Url::parse(url).map_err(|_| "invalid snapshot URL")?;
        let loopback = matches!(parsed.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
            || matches!(parsed.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback());
        if !(parsed.scheme() == "https" || (parsed.scheme() == "http" && loopback))
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.fragment().is_some()
        {
            return Err(
                "snapshot URL requires HTTPS or loopback HTTP, without credentials/fragment".into(),
            );
        }
        let abort =
            AbortFetch(web_sys::AbortController::new().map_err(|_| "fetch abort controller")?);
        let options = web_sys::RequestInit::new();
        options.set_signal(Some(&abort.0.signal()));
        options.set_cache(web_sys::RequestCache::NoStore);
        options.set_credentials(web_sys::RequestCredentials::Omit);
        options.set_redirect(web_sys::RequestRedirect::Error);
        options.set_mode(web_sys::RequestMode::Cors);
        options.set_referrer_policy(web_sys::ReferrerPolicy::NoReferrer);
        let request = web_sys::Request::new_with_str_and_init(url, &options)
            .map_err(|_| "snapshot request")?;
        let started = web_time::Instant::now();
        let work = Box::pin(async {
            let global = js_sys::global();
            let fetch = js_sys::Reflect::get(&global, &"fetch".into())
                .map_err(|_| "snapshot fetch unavailable")?
                .dyn_into::<js_sys::Function>()
                .map_err(|_| "snapshot fetch unavailable")?;
            let promise = fetch
                .call1(&global, &request)
                .map_err(|_| "snapshot fetch failed")?
                .dyn_into::<js_sys::Promise>()
                .map_err(|_| "snapshot fetch response")?;
            let response = JsFuture::from(promise)
                .await
                .map_err(|_| "snapshot fetch failed")?
                .dyn_into::<web_sys::Response>()
                .map_err(|_| "invalid fetch response")?;
            if response.status() != 200 || response.redirected() {
                return Err("snapshot HTTP status/redirect".to_string());
            }
            if let Some(length) = response
                .headers()
                .get("Content-Length")
                .map_err(|_| "snapshot headers")?
            {
                let size: usize = length.parse().map_err(|_| "snapshot Content-Length")?;
                if size > MAX_DOWNLOAD_BYTES {
                    return Err("snapshot download size limit".into());
                }
            }
            let reader = response
                .body()
                .ok_or("snapshot response has no body")?
                .get_reader()
                .dyn_into::<web_sys::ReadableStreamDefaultReader>()
                .map_err(|_| "snapshot stream reader")?;
            let mut bytes = Vec::new();
            loop {
                let item = JsFuture::from(reader.read())
                    .await
                    .map_err(|_| "snapshot stream failed")?;
                if js_sys::Reflect::get(&item, &"done".into())
                    .map_err(|_| "snapshot stream result")?
                    .as_bool()
                    == Some(true)
                {
                    break;
                }
                let chunk = js_sys::Reflect::get(&item, &"value".into())
                    .map_err(|_| "snapshot stream chunk")?
                    .dyn_into::<js_sys::Uint8Array>()
                    .map_err(|_| "snapshot stream bytes")?;
                if bytes.len() + chunk.length() as usize > MAX_DOWNLOAD_BYTES {
                    return Err("snapshot download size limit".into());
                }
                bytes.extend(chunk.to_vec());
            }
            reader.release_lock();
            Ok(bytes)
        });
        let bytes = match select(work, BrowserRuntime::timer(Duration::from_secs(45))).await {
            Either::Left((result, _)) => result?,
            Either::Right(_) => return Err("snapshot download deadline".into()),
        };
        let download_ms = started.elapsed().as_millis();
        drop(abort);
        let mut snapshot = verify_snapshot(&bytes, digest).await?;
        snapshot.receipt.download_ms = download_ms;
        Ok(snapshot)
    }

    #[derive(serde::Serialize)]
    struct BootstrapReceipt {
        snapshot: SnapshotReceipt,
        router: crate::router::ExploratoryReceipt,
        ready_after_ms: u128,
        elapsed_ms: u128,
        wasm_memory_bytes_at_end: u64,
    }

    /// Download a chosen netDb snapshot and run the full exploratory gate.
    /// JSON timings include download/validation, but module loading is measured
    /// by the caller before importing/initializing this WASM module.
    #[wasm_bindgen]
    pub async fn bootstrap_probe(
        snapshot_url: String,
        sha256: String,
        endpoint: String,
        capability: String,
    ) -> Result<String, JsValue> {
        let started = web_time::Instant::now();
        crate::configure_forwarder_authenticated(&endpoint, &capability)
            .map_err(JsValue::from_str)?;
        let snapshot = fetch_snapshot(&snapshot_url, &sha256)
            .await
            .map_err(|e| JsValue::from_str(&e))?;
        let router = crate::router::run_exploratory(snapshot.routers, 300, 30)
            .await
            .map_err(|e| JsValue::from_str(&e))?;
        let elapsed_ms = started.elapsed().as_millis();
        // Include all verification before Router's own clock starts.
        let ready_after_ms = elapsed_ms.saturating_sub(router.elapsed_ms) + router.ready_after_ms;
        serde_json::to_string(&BootstrapReceipt {
            snapshot: snapshot.receipt,
            router,
            ready_after_ms,
            elapsed_ms,
            wasm_memory_bytes_at_end: crate::router::wasm_memory_bytes(),
        })
        .map_err(|_| JsValue::from_str("bootstrap receipt serialization"))
    }
}

#[cfg(target_arch = "wasm32")]
pub use browser::fetch_snapshot;
