use super::*;
use futures::future::{select, Either};
use wasm_bindgen::{prelude::*, JsCast};
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen(module = "/src/bootstrap-cache.js")]
extern "C" {
    #[wasm_bindgen(js_name = loadBootstrap)]
    fn load_bootstrap(digest: &str) -> js_sys::Promise;
    #[wasm_bindgen(js_name = saveBootstrap)]
    fn save_bootstrap(digest: &str, bytes: &[u8]) -> js_sys::Promise;
    #[wasm_bindgen(js_name = removeBootstrap)]
    fn remove_bootstrap(digest: &str) -> js_sys::Promise;
    #[wasm_bindgen(js_name = loadLiveNetDb)]
    fn load_live_netdb() -> js_sys::Promise;
    #[wasm_bindgen(js_name = saveLiveNetDb)]
    fn save_live_netdb(bytes: &[u8]) -> js_sys::Promise;
}

/// Validate local candidates before attempting the network. A stale release
/// snapshot must not make a fresh learned netDb wait for the download deadline.
pub async fn bootstrap_routers(url: &str, digest: &str) -> Result<Snapshot, String> {
    validate_source(url, digest)?;
    let started = web_time::Instant::now();
    let (published, live) = futures::join!(cached_snapshot(digest), saved_routers());
    let source = match (published.is_some(), live.is_some()) {
        (true, true) => "published+learned-cache",
        (true, false) => "published-cache",
        _ => "learned-cache",
    };
    let local_ms = started.elapsed().as_millis();
    if let Ok(mut snapshot) = merge_routers(published, live) {
        snapshot.receipt.from_cache = true;
        snapshot.receipt.source = source.into();
        snapshot.receipt.local_ms = local_ms;
        return Ok(snapshot);
    }
    let mut snapshot = download_snapshot(url, digest).await?;
    snapshot.receipt.source = "download".into();
    snapshot.receipt.local_ms = local_ms;
    Ok(snapshot)
}

async fn saved_routers() -> Option<Snapshot> {
    match JsFuture::from(load_live_netdb()).await {
        Ok(value) => match value.dyn_into::<js_sys::Uint8Array>() {
            Ok(bytes) if bytes.length() as usize <= MAX_DOWNLOAD_BYTES => {
                verify_archive(&bytes.to_vec(), None).await.ok()
            }
            _ => None,
        },
        Err(_) => None,
    }
}

/// The store handed to emissary: each backup writes the archive to the
/// public live cache in the background.
pub fn live_netdb(initial: &[Vec<u8>]) -> std::sync::Arc<LiveNetDb> {
    std::sync::Arc::new(LiveNetDb::new(initial, |archive| {
        wasm_bindgen_futures::spawn_local(async move {
            let _ = JsFuture::from(save_live_netdb(&archive)).await;
        });
    }))
}

struct AbortFetch(web_sys::AbortController);
impl Drop for AbortFetch {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub async fn fetch_snapshot(url: &str, digest: &str) -> Result<Snapshot, String> {
    validate_source(url, digest)?;
    if let Some(snapshot) = cached_snapshot(digest).await {
        return Ok(snapshot);
    }
    download_snapshot(url, digest).await
}

pub(super) fn validate_source(url: &str, digest: &str) -> Result<(), String> {
    expected_digest(digest)?;
    let parsed = url::Url::parse(url).map_err(|_| "invalid snapshot URL")?;
    let loopback = matches!(parsed.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
        || matches!(parsed.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback())
        || parsed.host_str() == Some("localhost");
    if !(parsed.scheme() == "https" || (parsed.scheme() == "http" && loopback))
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(
            "snapshot URL requires HTTPS or loopback HTTP, without credentials/fragment".into(),
        );
    }
    Ok(())
}

async fn cached_snapshot(digest: &str) -> Option<Snapshot> {
    if let Ok(cached) = JsFuture::from(load_bootstrap(digest)).await {
        if let Ok(bytes) = cached.dyn_into::<js_sys::Uint8Array>() {
            if bytes.length() as usize <= MAX_DOWNLOAD_BYTES {
                if let Ok(mut snapshot) = verify_snapshot(&bytes.to_vec(), digest).await {
                    snapshot.receipt.from_cache = true;
                    snapshot.receipt.source = "published-cache".into();
                    return Some(snapshot);
                }
            }
            // Stale, damaged or wrongly signed caches never bypass validation.
            let _ = JsFuture::from(remove_bootstrap(digest)).await;
        }
    }
    None
}

async fn download_snapshot(url: &str, digest: &str) -> Result<Snapshot, String> {
    let abort = AbortFetch(web_sys::AbortController::new().map_err(|_| "fetch abort controller")?);
    let options = web_sys::RequestInit::new();
    options.set_signal(Some(&abort.0.signal()));
    options.set_cache(web_sys::RequestCache::NoStore);
    options.set_credentials(web_sys::RequestCredentials::Omit);
    options.set_redirect(web_sys::RequestRedirect::Error);
    options.set_mode(web_sys::RequestMode::Cors);
    options.set_referrer_policy(web_sys::ReferrerPolicy::NoReferrer);
    let request =
        web_sys::Request::new_with_str_and_init(url, &options).map_err(|_| "snapshot request")?;
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
    snapshot.receipt.source = "download".into();
    let _ = JsFuture::from(save_bootstrap(digest, &bytes)).await;
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
    crate::configure_forwarder_authenticated(&endpoint, &capability).map_err(JsValue::from_str)?;
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
