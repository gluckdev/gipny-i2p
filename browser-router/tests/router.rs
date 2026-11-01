#![cfg(all(target_arch = "wasm32", feature = "router-live-tests"))]
use emissary_core::crypto::base64_decode;
use gipny_browser_router::{configure_forwarder_authenticated, router::run_exploratory};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);
const CANDIDATES: &str = include_str!(env!("GIPNY_NTCP2_CANDIDATES"));
const CAPABILITY: &str = env!("GIPNY_FORWARDER_CAPABILITY");

#[wasm_bindgen_test]
async fn browser_router_builds_and_tests_two_hop_exploratory_paths() {
    configure_forwarder_authenticated("ws://127.0.0.1:18767/stream", CAPABILITY).unwrap();
    let routers = CANDIDATES
        .lines()
        .filter(|row| row.contains("\tnetdb\t"))
        .map(|row| base64_decode(row.split('\t').nth(1).unwrap()).unwrap())
        .collect();
    let receipt = run_exploratory(routers, 300, 30)
        .await
        .expect("real exploratory paths and roundtrips");
    assert!(receipt.inbound > 0 && receipt.outbound > 0 && receipt.roundtrips >= 2);
    assert!(receipt.held_seconds >= 30);
    assert!(receipt.wasm_memory_bytes > 0);
    console_log!("EXPLORATORY_PASS {receipt:?}");
}
