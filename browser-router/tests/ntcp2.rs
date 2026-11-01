#![cfg(all(target_arch = "wasm32", feature = "ntcp2-live-tests"))]

use emissary_core::crypto::base64_decode;
use gipny_browser_router::{
    configure_forwarder_authenticated,
    ntcp2::{probe, verified_peer, ProbeError},
};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);
const CANDIDATES: &str = include_str!(env!("GIPNY_NTCP2_CANDIDATES"));
const CAPABILITY: &str = env!("GIPNY_FORWARDER_CAPABILITY");

fn configure() {
    configure_forwarder_authenticated("ws://127.0.0.1:18767/stream", CAPABILITY).unwrap();
}

#[wasm_bindgen_test]
async fn ntcp2_interoperates_with_pinned_native_i2pd() {
    configure();
    let row = CANDIDATES
        .lines()
        .find(|row| row.contains("\tfixture\t"))
        .unwrap();
    let bytes = base64_decode(row.split('\t').nth(1).unwrap()).unwrap();
    let receipt = probe(&bytes)
        .await
        .expect("native i2pd NTCP2 handshake and authenticated data");
    assert!(receipt.authenticated_bytes > 0);
    assert!(receipt.blocks > 0);
    console_log!("NTCP2_NATIVE_PASS {receipt:?}");
}

#[wasm_bindgen_test]
async fn ntcp2_authenticates_a_live_public_netdb_peer() {
    configure();
    let mut attempts = 0;
    for row in CANDIDATES.lines().filter(|row| row.contains("\tnetdb\t")) {
        configure();
        let bytes = base64_decode(row.split('\t').nth(1).unwrap()).unwrap();
        attempts += 1;
        match probe(&bytes).await {
            Ok(receipt) => {
                assert!(!receipt.target.ip().is_loopback());
                assert!(receipt.authenticated_bytes > 0);
                console_log!("NTCP2_PUBLIC_PASS attempts={attempts} {receipt:?}");
                return;
            }
            Err(error) => console_log!(
                "NTCP2_PUBLIC_ATTEMPT target={} error={error:?}",
                row.split('\t').next().unwrap()
            ),
        }
    }
    panic!("no authenticated public NTCP2 session after {attempts} verified candidates");
}

#[wasm_bindgen_test]
async fn corrupted_router_info_and_missing_capability_fail_closed() {
    let row = CANDIDATES
        .lines()
        .find(|row| row.contains("\tfixture\t"))
        .unwrap();
    let mut bytes = base64_decode(row.split('\t').nth(1).unwrap()).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    assert!(matches!(verified_peer(&bytes), Err(ProbeError::RouterInfo)));
    assert!(matches!(probe(&bytes).await, Err(ProbeError::RouterInfo)));
    bytes[last] ^= 1;
    configure_forwarder_authenticated("ws://127.0.0.1:18767/stream", &"b".repeat(43)).unwrap();
    assert!(matches!(
        probe(&bytes).await,
        Err(ProbeError::Io("connect"))
    ));
}
