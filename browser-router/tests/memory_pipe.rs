#![cfg(target_arch = "wasm32")]
use emissary_core::runtime::{AsyncRead, AsyncWrite};
use futures::future::poll_fn;
use gipny_browser_router::BrowserTcpStream;
use std::pin::Pin;
use wasm_bindgen_test::*;
wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
async fn in_process_stream_is_bidirectional_and_preserves_eof() {
    let (mut left, mut right) = BrowserTcpStream::memory_pair();
    let written = poll_fn(|cx| Pin::new(&mut left).poll_write(cx, b"hello"))
        .await
        .unwrap();
    assert_eq!(written, 5);
    poll_fn(|cx| Pin::new(&mut left).poll_flush(cx))
        .await
        .unwrap();
    let mut bytes = [0; 3];
    assert_eq!(
        poll_fn(|cx| Pin::new(&mut right).poll_read(cx, &mut bytes))
            .await
            .unwrap(),
        3
    );
    assert_eq!(&bytes, b"hel");
    assert_eq!(
        poll_fn(|cx| Pin::new(&mut right).poll_read(cx, &mut bytes))
            .await
            .unwrap(),
        2
    );
    assert_eq!(&bytes[..2], b"lo");
    poll_fn(|cx| Pin::new(&mut right).poll_write(cx, b"reply"))
        .await
        .unwrap();
    poll_fn(|cx| Pin::new(&mut right).poll_flush(cx))
        .await
        .unwrap();
    let mut reply = [0; 5];
    assert_eq!(
        poll_fn(|cx| Pin::new(&mut left).poll_read(cx, &mut reply))
            .await
            .unwrap(),
        5
    );
    assert_eq!(&reply, b"reply");
    drop(left);
    assert_eq!(
        poll_fn(|cx| Pin::new(&mut right).poll_read(cx, &mut bytes))
            .await
            .unwrap(),
        0
    );
}

#[wasm_bindgen_test]
fn embedded_destination_persists_exactly_and_rejects_an_unrelated_signing_key() {
    use emissary_core::ClientIdentity;
    use gipny_browser_router::BrowserRuntime;
    let first = ClientIdentity::generate::<BrowserRuntime>();
    let mut encoded = first.to_bytes();
    let restored = ClientIdentity::from_bytes(&encoded).unwrap();
    assert_eq!(first.destination(), restored.destination());
    assert_eq!(restored.to_bytes(), encoded);
    let last = encoded.len() - 1;
    encoded[last] ^= 1;
    assert!(ClientIdentity::from_bytes(&encoded).is_err());
    assert!(ClientIdentity::from_bytes(&encoded[..100]).is_err());
}
