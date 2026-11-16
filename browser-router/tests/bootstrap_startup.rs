#![cfg(target_arch = "wasm32")]
use gipny_browser_router::bootstrap::{bootstrap_routers, PublicBootstrap};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;

#[path = "common/bootstrap.rs"]
mod fixture;
use fixture::{archive, digest, signed_info, signed_info_at};
wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(inline_js = r#"
let original, requests = 0, aborted = 0;
export async function startupFixture(hash, published, learned, downloaded, mode) {
  original = globalThis.fetch;
  requests = 0; aborted = 0;
  await caches.delete('gipny-public-netdb-v1');
  await caches.delete('gipny-public-netdb-live-v1');
  if (published.length) await (await caches.open('gipny-public-netdb-v1')).put(
    new URL('/__gipny-public-netdb/v1/' + hash, location.href), new Response(published.slice()));
  if (learned.length) await (await caches.open('gipny-public-netdb-live-v1')).put(
    new URL('/__gipny-public-netdb/live/v1', location.href), new Response(learned.slice()));
  const bytes = downloaded.slice();
  globalThis.fetch = request => {
    requests++;
    if (mode === 'offline') return Promise.reject(new Error('offline'));
    if (mode === 'hang') return new Promise((resolve, reject) => {
      request.signal.addEventListener('abort', () => { aborted++; reject(new Error('aborted')); }, {once:true});
    });
    return Promise.resolve(new Response(bytes));
  };
}
export function startupRequests() { return requests; }
export function startupAborts() { return aborted; }
export function startupRestore() { if (original) globalThis.fetch = original; original = null; }
"#)]
extern "C" {
    #[wasm_bindgen(js_name = startupFixture)]
    fn setup(
        hash: &str,
        published: &[u8],
        learned: &[u8],
        downloaded: &[u8],
        mode: &str,
    ) -> js_sys::Promise;
    #[wasm_bindgen(js_name = startupRequests)]
    fn requests() -> u32;
    #[wasm_bindgen(js_name = startupAborts)]
    fn aborts() -> u32;
    #[wasm_bindgen(js_name = startupRestore)]
    fn restore();
}
struct Fixture;
impl Drop for Fixture {
    fn drop(&mut self) {
        restore();
    }
}
async fn fixture(
    hash: &str,
    published: &[u8],
    learned: &[u8],
    downloaded: &[u8],
    mode: &str,
) -> Fixture {
    JsFuture::from(setup(hash, published, learned, downloaded, mode))
        .await
        .unwrap();
    Fixture
}

#[wasm_bindgen_test]
async fn cold_warm_offline_and_stale_published_with_fresh_learned_seeds() {
    let fresh = archive(
        &(0..16)
            .map(|_| signed_info("8.8.8.8", 2))
            .collect::<Vec<_>>(),
    );
    let hash = digest(&fresh);
    {
        let _fixture = fixture(&hash, &[], &[], &fresh, "download").await;
        let cold = bootstrap_routers("https://seeds.example.test/current.tar.gz", &hash)
            .await
            .unwrap();
        assert_eq!(requests(), 1);
        assert_eq!(cold.receipt.source, "download");
        assert_eq!(cold.routers.len(), 16);
        // The first download persists the fully verified public snapshot.
        let warm = bootstrap_routers("https://seeds.example.test/current.tar.gz", &hash)
            .await
            .unwrap();
        assert_eq!(requests(), 1, "warm startup must not touch the network");
        assert_eq!(warm.receipt.source, "published-cache");
        assert!(warm.receipt.from_cache);
    }
    {
        let _fixture = fixture(&hash, &fresh, &[], &[], "offline").await;
        assert!(
            bootstrap_routers("https://seeds.example.test/current.tar.gz", &hash)
                .await
                .is_ok()
        );
        assert_eq!(requests(), 0);
    }
    // Correctly signed and digest-matching, but outside the 36-hour RI bound.
    let stale = archive(
        &(0..16)
            .map(|_| signed_info_at("8.8.8.8", 2, 37 * 60 * 60 * 1000))
            .collect::<Vec<_>>(),
    );
    let stale_hash = digest(&stale);
    {
        let _fixture = fixture(&stale_hash, &stale, &fresh, &[], "offline").await;
        let restored = bootstrap_routers("https://seeds.example.test/current.tar.gz", &stale_hash)
            .await
            .unwrap();
        assert_eq!(
            requests(),
            0,
            "fresh learned netDb must not wait for snapshot fetch"
        );
        assert_eq!(restored.receipt.source, "learned-cache");
        assert_eq!(restored.routers.len(), 16);
    }
    {
        let too_small = archive(
            &(0..15)
                .map(|_| signed_info("8.8.8.8", 2))
                .collect::<Vec<_>>(),
        );
        let _fixture = fixture(&stale_hash, &stale, &too_small, &[], "offline").await;
        assert!(
            bootstrap_routers("https://seeds.example.test/current.tar.gz", &stale_hash)
                .await
                .is_err()
        );
        assert_eq!(
            requests(),
            1,
            "insufficient local seeds still require a valid download"
        );
    }
}

#[wasm_bindgen_test]
async fn cancelling_public_preparation_aborts_fetch_without_router_or_identity() {
    let hash = "a".repeat(64);
    let _fixture = fixture(&hash, &[], &[], &[], "hang").await;
    let job = PublicBootstrap::new("https://seeds.example.test/current.tar.gz", &hash).unwrap();
    let result = job.ready();
    assert!(
        js_sys::Object::is(&result, &job.ready()),
        "callers join the same job"
    );
    for _ in 0..100 {
        if requests() > 0 {
            break;
        }
        gloo_timers::future::TimeoutFuture::new(5).await;
    }
    assert_eq!(requests(), 1);
    job.close();
    assert!(JsFuture::from(result).await.is_err());
    assert_eq!(
        aborts(),
        1,
        "dropping Abortable must abort the underlying HTTP request"
    );
}
