#![cfg(target_arch = "wasm32")]
use emissary_core::runtime::Storage;
use emissary_core::{
    crypto::{sha256::Sha256, SigningPrivateKey, StaticPrivateKey},
    primitives::{RouterAddress, RouterInfo},
    runtime::Runtime,
    Config,
};
use gipny_browser_router::{
    bootstrap::{
        fetch_snapshot, merge_routers, verify_archive, verify_snapshot, LiveNetDb,
        MAX_DOWNLOAD_BYTES, MAX_TAR_BYTES,
    },
    BrowserRuntime,
};
use rand::RngExt;
use std::io::Write;
use std::sync::{Arc, Mutex};
use wasm_bindgen_test::*;
wasm_bindgen_test_configure!(run_in_browser);

fn digest(bytes: &[u8]) -> String {
    Sha256::new()
        .update(bytes)
        .finalize_new()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gzip.write_all(bytes).unwrap();
    gzip.finish().unwrap()
}

fn archive(records: &[Vec<u8>]) -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    for (index, bytes) in records.iter().enumerate() {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(
            &mut header,
            format!("rX/routerInfo-{index}.dat"),
            bytes.as_slice(),
        )
        .unwrap();
    }
    gzip(&tar.into_inner().unwrap())
}

fn signed_info(ip: &str, network: u8) -> Vec<u8> {
    let key = BrowserRuntime::rng().random();
    let secret = StaticPrivateKey::from_bytes(key);
    let signing = SigningPrivateKey::random(BrowserRuntime::rng());
    let address = RouterAddress::new_published_ntcp2(
        key,
        [0; 16],
        None,
        true,
        ip.parse().unwrap(),
        format!("{ip}:12345").parse().unwrap(),
    );
    RouterInfo::new::<BrowserRuntime>(
        &Config {
            net_id: Some(network),
            ..Config::default()
        },
        Some(address),
        None,
        None,
        None,
        &secret,
        &signing,
        false,
    )
    .serialize(&signing)
}

#[wasm_bindgen_test]
async fn snapshot_verifies_signatures_network_public_targets_and_deduplicates() {
    let mut records: Vec<_> = (0..16).map(|_| signed_info("8.8.8.8", 2)).collect();
    records.push(records[0].clone());
    let mut invalid = records[0].clone();
    *invalid.last_mut().unwrap() ^= 1;
    records.push(invalid);
    records.push(signed_info("127.0.0.1", 2));
    records.push(signed_info("8.8.8.8", 3));
    let bytes = archive(&records);
    let result = verify_snapshot(&bytes, &digest(&bytes)).await.unwrap();
    assert_eq!(result.routers.len(), 16);
    assert_eq!(result.receipt.accepted, 16);
    assert_eq!(result.receipt.duplicates, 1);
    assert_eq!(result.receipt.rejected, 3);
}

#[wasm_bindgen_test]
async fn snapshot_rejects_wrong_digest_crc_and_truncated_tar() {
    let valid = archive(&[vec![1; 10]]);
    assert!(verify_snapshot(&valid, &"00".repeat(32))
        .await
        .err()
        .unwrap()
        .contains("SHA256"));
    let mut corrupt = valid.clone();
    let index = corrupt.len() - 8;
    corrupt[index] ^= 1;
    assert!(verify_snapshot(&corrupt, &digest(&corrupt))
        .await
        .err()
        .unwrap()
        .contains("gzip"));
    let truncated = gzip(&[0; 512]);
    assert!(verify_snapshot(&truncated, &digest(&truncated))
        .await
        .err()
        .unwrap()
        .contains("incomplete"));
}

#[wasm_bindgen_test]
async fn snapshot_rejects_compression_bombs_and_oversized_records() {
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    for _ in 0..=MAX_TAR_BYTES / 65536 {
        gzip.write_all(&[0; 65536]).unwrap();
    }
    let bomb = gzip.finish().unwrap();
    assert!(bomb.len() < MAX_DOWNLOAD_BYTES);
    assert!(verify_snapshot(&bomb, &digest(&bomb))
        .await
        .err()
        .unwrap()
        .contains("decompression limit"));
    let oversized = archive(&[vec![0; 16385]]);
    assert!(verify_snapshot(&oversized, &digest(&oversized))
        .await
        .err()
        .unwrap()
        .contains("RouterInfo size limit"));
}

#[wasm_bindgen_test]
async fn snapshot_rejects_links_and_parent_paths() {
    for link in [false, true] {
        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(0);
        if link {
            header.set_path("routerInfo-link.dat").unwrap();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_link_name("/tmp/escape").unwrap();
        } else {
            let path = b"../routerInfo-escape.dat";
            header.as_mut_bytes()[..path.len()].copy_from_slice(path);
        }
        header.set_cksum();
        tar.append(&header, std::io::empty()).unwrap();
        let bytes = gzip(&tar.into_inner().unwrap());
        let error = verify_snapshot(&bytes, &digest(&bytes))
            .await
            .err()
            .unwrap();
        assert!(error.contains(if link { "non-regular" } else { "path" }));
    }
}

#[wasm_bindgen_test]
async fn snapshot_fetch_enforces_http_stream_and_url_boundaries() {
    let hash = digest(b"bad");
    for path in [
        "missing",
        "oversize",
        "stream-oversize",
        "redirect",
        "bad-gzip",
        "truncated",
    ] {
        assert!(
            fetch_snapshot(&format!("http://127.0.0.1:18766/{path}"), &hash)
                .await
                .is_err(),
            "accepted {path}"
        );
    }
    for url in [
        "http://8.8.8.8/snapshot",
        "https://user:password@example.com/snapshot",
        "https://example.com/snapshot#fragment",
        "file:///tmp/snapshot",
    ] {
        assert!(fetch_snapshot(url, &hash)
            .await
            .err()
            .unwrap()
            .contains("URL"));
    }
}

fn profile() -> emissary_core::Profile {
    emissary_core::Profile {
        is_connected: false,
        last_activity: std::time::Duration::ZERO,
        last_declined: None,
        last_dial_failure: None,
        num_accepted: 0,
        num_connection: 0,
        num_dial_failures: 0,
        num_lookup_failures: 0,
        num_lookup_no_responses: 0,
        num_lookup_successes: 0,
        num_rejected: 0,
        num_selected: 0,
        num_test_failures: 0,
        num_test_successes: 0,
        num_unaswered: 0,
    }
}

fn router_id(info: &[u8]) -> String {
    let router = emissary_core::primitives::RouterInfo::parse::<BrowserRuntime>(info).unwrap();
    emissary_core::crypto::base64_encode(router.identity.id().to_vec())
}

#[wasm_bindgen_test]
async fn live_netdb_keeps_learned_routers_for_the_next_start() {
    let initial: Vec<_> = (0..16).map(|_| signed_info("8.8.8.8", 2)).collect();
    let written = Arc::new(Mutex::new(Vec::new()));
    let sink = written.clone();
    let live = LiveNetDb::new(&initial, move |archive| *sink.lock().unwrap() = archive);
    assert_eq!(live.len(), 16);

    // emissary hands over only newly discovered RouterInfos; unusable ones
    // (private address, other network, mismatching id) are not kept.
    let learned = signed_info("1.1.1.1", 2);
    let private = signed_info("10.0.0.1", 2);
    let foreign = signed_info("9.9.9.9", 3);
    live.save_to_disk(vec![
        (router_id(&learned), Some(learned.clone()), profile()),
        (router_id(&private), Some(private), profile()),
        (router_id(&foreign), Some(foreign), profile()),
        (router_id(&initial[0]), None, profile()),
        (
            "not-this-router".into(),
            Some(signed_info("1.0.0.1", 2)),
            profile(),
        ),
    ]);
    assert_eq!(live.len(), 17);
    let archive = written.lock().unwrap().clone();
    let restored = verify_archive(&archive, None).await.unwrap();
    assert_eq!(restored.routers.len(), 17);
    assert!(restored.routers.contains(&learned));

    // Routers emissary drops go at the next backup.
    live.remove_from_disk(vec![router_id(&learned)]);
    live.save_to_disk(Vec::new());
    let archive = written.lock().unwrap().clone();
    assert_eq!(
        verify_archive(&archive, None).await.unwrap().routers.len(),
        16
    );
}

#[wasm_bindgen_test]
async fn startup_prefers_the_snapshot_and_falls_back_to_saved_routers() {
    let published: Vec<_> = (0..16).map(|_| signed_info("8.8.8.8", 2)).collect();
    let learned: Vec<_> = (0..4).map(|_| signed_info("1.1.1.1", 2)).collect();
    let snapshot = || {
        let bytes = archive(&published);
        let digest = digest(&bytes);
        async move { verify_snapshot(&bytes, &digest).await.unwrap() }
    };
    let saved = || {
        let mut all = published.clone();
        all.extend(learned.clone());
        let bytes = archive(&all);
        async move { verify_archive(&bytes, None).await.unwrap() }
    };
    // Both: the snapshot's routers first, saved ones added once each.
    let merged = merge_routers(Some(snapshot().await), Some(saved().await)).unwrap();
    assert_eq!(merged.routers.len(), 20);
    assert_eq!(&merged.routers[..16], published.as_slice());
    // Hosting gone (no snapshot): the saved routers alone still start.
    let offline = merge_routers(None, Some(saved().await)).unwrap();
    assert_eq!(offline.routers.len(), 20);
    // Nothing at all is an error, not an empty router.
    assert!(merge_routers(None, None).is_err());
}
