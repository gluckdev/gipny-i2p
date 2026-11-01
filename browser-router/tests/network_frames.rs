#![cfg(target_arch = "wasm32")]
use futures::channel::oneshot;
use futures::future::Shared;
use futures::io::{AsyncReadExt, AsyncWriteExt};
use futures::FutureExt;
use gipny_browser_router::{
    decode_client_frame, decode_relay_frame, encode_client_frame, encode_relay_frame,
    BrowserTcpStream,
};
use wasm_bindgen_test::*;
wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn test_client_frame_ping_roundtrip_and_native_golden_bytes() {
    let json = "\"Ping\"";
    let encoded = encode_client_frame(json).expect("encode Ping");
    // Wire compatibility: ClientToRelay::Ping variant 5 in little-endian fixed-width 4 bytes
    assert_eq!(encoded, vec![0x05, 0x00, 0x00, 0x00]);
    let decoded = decode_client_frame(&encoded).expect("decode Ping");
    assert_eq!(decoded, "\"Ping\"");
}

#[wasm_bindgen_test]
fn test_relay_frame_unit_variants_native_golden_bytes() {
    let auth_ok = "\"AuthOk\"";
    let enc_ok = encode_relay_frame(auth_ok).expect("encode AuthOk");
    assert_eq!(enc_ok, vec![0x01, 0x00, 0x00, 0x00]);
    assert_eq!(decode_relay_frame(&enc_ok).unwrap(), "\"AuthOk\"");

    let auth_fail = "\"AuthFail\"";
    let enc_fail = encode_relay_frame(auth_fail).expect("encode AuthFail");
    assert_eq!(enc_fail, vec![0x02, 0x00, 0x00, 0x00]);
    assert_eq!(decode_relay_frame(&enc_fail).unwrap(), "\"AuthFail\"");

    let pong = "\"Pong\"";
    let enc_pong = encode_relay_frame(pong).expect("encode Pong");
    assert_eq!(enc_pong, vec![0x06, 0x00, 0x00, 0x00]);
    assert_eq!(decode_relay_frame(&enc_pong).unwrap(), "\"Pong\"");
}

#[wasm_bindgen_test]
fn test_client_frame_auth_v2_roundtrip() {
    let sign_pk = vec![42u8; 32];
    let signature = vec![7u8; 64];
    let val = serde_json::json!({
        "AuthV2": {
            "sign_pk": sign_pk,
            "signature": signature
        }
    });
    let json = serde_json::to_string(&val).unwrap();
    let encoded = encode_client_frame(&json).expect("encode AuthV2");
    let decoded_json = decode_client_frame(&encoded).expect("decode AuthV2");
    let decoded_val: serde_json::Value = serde_json::from_str(&decoded_json).unwrap();
    assert_eq!(decoded_val, val);
}

#[wasm_bindgen_test]
fn test_relay_frame_challenge_roundtrip() {
    let challenge = vec![99u8; 32];
    let challenge_val = serde_json::json!({
        "Challenge": challenge
    });
    let encoded = encode_relay_frame(&serde_json::to_string(&challenge_val).unwrap())
        .expect("encode Challenge");
    let decoded = decode_relay_frame(&encoded).expect("decode Challenge");
    let val: serde_json::Value = serde_json::from_str(&decoded).unwrap();
    assert_eq!(val, challenge_val);
}

#[wasm_bindgen_test]
fn test_relay_frame_bundle_roundtrip() {
    let pk = vec![11u8; 32];
    let bundle_bytes = vec![1, 2, 3, 4, 5];
    let bundle_val = serde_json::json!({
        "Bundle": {
            "pk": pk,
            "bundle": bundle_bytes
        }
    });
    let encoded =
        encode_relay_frame(&serde_json::to_string(&bundle_val).unwrap()).expect("encode Bundle");
    let decoded = decode_relay_frame(&encoded).expect("decode Bundle");
    let val: serde_json::Value = serde_json::from_str(&decoded).unwrap();
    assert_eq!(val, bundle_val);
}

#[wasm_bindgen_test]
fn test_bincode_rejects_trailing_bytes() {
    let json = "\"Ping\"";
    let mut encoded = encode_client_frame(json).expect("encode Ping");
    encoded.push(0xff); // append trailing byte
    assert!(
        decode_client_frame(&encoded).is_err(),
        "decode must reject trailing bytes"
    );
}

#[wasm_bindgen_test]
fn test_corrupt_frame_rejected() {
    let corrupt = vec![0xde, 0xad, 0xbe, 0xef];
    assert!(decode_client_frame(&corrupt).is_err());
    assert!(decode_relay_frame(&corrupt).is_err());
}

#[wasm_bindgen_test]
async fn test_multi_waiter_cancellation_wakes_all_waiters() {
    let (tx, rx) = oneshot::channel::<()>();
    let shared: Shared<_> = rx.shared();

    let waiter1 = shared.clone();
    let waiter2 = shared.clone();
    let waiter3 = shared.clone();

    let h1 = async move {
        let _ = waiter1.await;
        1
    };
    let h2 = async move {
        let _ = waiter2.await;
        2
    };
    let h3 = async move {
        let _ = waiter3.await;
        3
    };

    let trigger = async move {
        let _ = tx.send(());
    };

    let (r1, r2, r3, _) = futures::join!(h1, h2, h3, trigger);
    assert_eq!(r1, 1);
    assert_eq!(r2, 2);
    assert_eq!(r3, 3);
}

#[wasm_bindgen_test]
async fn in_process_stream_split_and_concurrent_framing() {
    let (left, right) = BrowserTcpStream::memory_pair();
    let (mut left_read, mut left_write) = left.split();
    let (mut right_read, mut right_write) = right.split();

    let client_payload = encode_client_frame("\"Ping\"").unwrap();
    let relay_payload = encode_relay_frame("\"Pong\"").unwrap();

    // Concurrent exchange: left writes ping and reads pong; right reads ping and writes pong.
    let left_task = async {
        let len_bytes = (client_payload.len() as u32).to_be_bytes();
        left_write.write_all(&len_bytes).await.unwrap();
        left_write.write_all(&client_payload).await.unwrap();
        left_write.flush().await.unwrap();

        let mut len_buf = [0u8; 4];
        left_read.read_exact(&mut len_buf).await.unwrap();
        let len = u32::from_be_bytes(len_buf);
        let mut reply = vec![0u8; len as usize];
        left_read.read_exact(&mut reply).await.unwrap();
        assert_eq!(decode_relay_frame(&reply).unwrap(), "\"Pong\"");
    };

    let right_task = async {
        let mut len_buf = [0u8; 4];
        right_read.read_exact(&mut len_buf).await.unwrap();
        let len = u32::from_be_bytes(len_buf);
        let mut req = vec![0u8; len as usize];
        right_read.read_exact(&mut req).await.unwrap();
        assert_eq!(decode_client_frame(&req).unwrap(), "\"Ping\"");

        let len_bytes = (relay_payload.len() as u32).to_be_bytes();
        right_write.write_all(&len_bytes).await.unwrap();
        right_write.write_all(&relay_payload).await.unwrap();
        right_write.flush().await.unwrap();
    };

    futures::future::join(left_task, right_task).await;
}

#[wasm_bindgen_test]
async fn in_process_stream_peer_drop_surfaces_eof_error() {
    let (left, right) = BrowserTcpStream::memory_pair();
    let (mut right_read, _right_write) = right.split();

    // Drop left endpoint immediately
    drop(left);

    let mut len_buf = [0u8; 4];
    let res = right_read.read_exact(&mut len_buf).await;
    assert!(res.is_err(), "reading from dropped peer must error on EOF");
}
