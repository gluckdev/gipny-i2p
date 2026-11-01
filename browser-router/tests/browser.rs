#![cfg(target_arch = "wasm32")]

use emissary_core::runtime::{
    AsyncRead, AsyncWrite, Instant, JoinSet, Runtime, TcpListener, TcpStream, UdpSocket,
};
use futures::{future::poll_fn, StreamExt};
use gipny_browser_router::{
    configure_forwarder, BrowserRuntime, BrowserTcpListener, BrowserTcpStream, BrowserUdpSocket,
    MAX_GZIP_BYTES,
};
use rand::RngExt;
use std::{pin::Pin, time::Duration};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

async fn connect(port: u16) -> BrowserTcpStream {
    configure_forwarder("ws://127.0.0.1:18765/stream?target=wrong").unwrap();
    BrowserTcpStream::connect(format!("127.0.0.1:{port}").parse().unwrap())
        .await
        .unwrap()
}

async fn read(stream: &mut BrowserTcpStream, buf: &mut [u8]) -> emissary_core::Result<usize> {
    poll_fn(|cx| Pin::new(&mut *stream).poll_read(cx, buf)).await
}

async fn write(stream: &mut BrowserTcpStream, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        let n = poll_fn(|cx| Pin::new(&mut *stream).poll_write(cx, bytes))
            .await
            .unwrap();
        assert!(n > 0);
        bytes = &bytes[n..];
    }
    poll_fn(|cx| Pin::new(&mut *stream).poll_flush(cx))
        .await
        .unwrap();
}

#[wasm_bindgen_test]
async fn timers_tasks_clocks_and_entropy_work_in_browser() {
    fn send_sync<T: Send + Sync>() {}
    fn runtime<R: Runtime>() {}
    runtime::<BrowserRuntime>();
    send_sync::<BrowserTcpStream>();
    let now = BrowserRuntime::now();
    BrowserRuntime::timer(Duration::from_millis(10)).await;
    assert!(now.elapsed() >= Duration::from_millis(8));
    assert!(BrowserRuntime::time_since_epoch().as_secs() > 1_700_000_000);
    let a: [u8; 32] = BrowserRuntime::rng().random();
    let b: [u8; 32] = BrowserRuntime::rng().random();
    assert_ne!(a, b);
    assert!(gipny_browser_router::runtime_probe());
    let (tx, rx) = futures::channel::oneshot::channel();
    BrowserRuntime::spawn(async move {
        BrowserRuntime::delay(Duration::from_millis(1)).await;
        tx.send(42).unwrap();
    });
    assert_eq!(rx.await.unwrap(), 42);
    let mut tasks = BrowserRuntime::join_set();
    tasks.push(async { 1_u8 });
    tasks.push(async {
        BrowserRuntime::delay(Duration::from_millis(1)).await;
        2_u8
    });
    assert_eq!(tasks.len(), 2);
    let mut values = Vec::new();
    for _ in 0..2 {
        values.push(tasks.next().await.unwrap());
    }
    values.sort();
    assert_eq!(values, [1, 2]);
    assert!(tasks.is_empty());
}

#[wasm_bindgen_test]
async fn idle_join_set_remains_open_and_wakes_when_reused() {
    use futures::{
        task::{waker, ArcWake},
        Stream,
    };
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        task::{Context, Poll},
    };
    struct WakeCount(AtomicUsize);
    impl ArcWake for WakeCount {
        fn wake_by_ref(this: &Arc<Self>) {
            this.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let count = Arc::new(WakeCount(AtomicUsize::new(0)));
    let waker = waker(count.clone());
    let mut context = Context::from_waker(&waker);
    let mut tasks = BrowserRuntime::join_set();
    assert!(matches!(
        Pin::new(&mut tasks).poll_next(&mut context),
        Poll::Pending
    ));
    tasks.push(async { 1 });
    assert!(count.0.load(Ordering::SeqCst) > 0);
    assert_eq!(tasks.next().await, Some(1));
    assert!(matches!(
        Pin::new(&mut tasks).poll_next(&mut context),
        Poll::Pending
    ));
    tasks.push(async { 2 });
    assert_eq!(tasks.next().await, Some(2));
}

#[wasm_bindgen_test]
fn gzip_roundtrip_rejects_corruption_and_expansion_bombs() {
    use std::io::Write;
    let payload = b"NTCP2 router-info payload";
    let zipped = BrowserRuntime::gzip_compress(payload).unwrap();
    assert_eq!(BrowserRuntime::gzip_decompress(zipped).unwrap(), payload);
    assert!(BrowserRuntime::gzip_decompress(b"invalid gzip").is_none());
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&vec![0; MAX_GZIP_BYTES + 1]).unwrap();
    assert!(BrowserRuntime::gzip_decompress(encoder.finish().unwrap()).is_none());
}

#[wasm_bindgen_test]
async fn unsupported_network_operations_fail_without_hanging() {
    let address = "127.0.0.1:0".parse().unwrap();
    assert!(BrowserUdpSocket::bind(address).await.is_none());
    assert!(BrowserUdpSocket::bind_with_mtu(address, 1500)
        .await
        .is_none());
    assert!(BrowserTcpListener::bind(address).await.is_none());
}

#[wasm_bindgen_test]
fn forwarder_rejects_insecure_remote_urls() {
    for endpoint in [
        "ws://example.com/stream",
        "https://example.com",
        "wss://user:secret@example.com",
        "wss://example.com/#fragment",
    ] {
        assert!(configure_forwarder(endpoint).is_err());
    }
    assert!(configure_forwarder("wss://example.com/stream").is_ok());
    assert!(configure_forwarder("ws://[::1]:18765/stream").is_ok());
}

#[wasm_bindgen_test]
async fn websocket_preserves_bytes_across_frames_and_short_reads() {
    let mut stream = connect(10101).await;
    let payload = b"\x00\x01NTCP2\xff binary bytes across frames";
    write(&mut stream, payload).await;
    assert_eq!(read(&mut stream, &mut []).await.unwrap(), 0);
    let mut result = Vec::new();
    while result.len() < payload.len() {
        let mut chunk = [0; 2];
        let n = read(&mut stream, &mut chunk).await.unwrap();
        assert!(n > 0);
        result.extend_from_slice(&chunk[..n]);
    }
    assert_eq!(result, payload);
    poll_fn(|cx| Pin::new(&mut stream).poll_close(cx))
        .await
        .unwrap();
    assert!(
        poll_fn(|cx| Pin::new(&mut stream).poll_write(cx, b"closed"))
            .await
            .is_err()
    );
}

#[wasm_bindgen_test]
async fn empty_frames_are_skipped_and_clean_close_is_eof() {
    let mut stream = connect(10106).await;
    write(&mut stream, b"hello").await;
    let mut buf = [0; 32];
    let n = read(&mut stream, &mut buf).await.unwrap();
    assert_eq!(&buf[..n], b"hello");
    assert_eq!(read(&mut stream, &mut buf).await.unwrap(), 0);
}

#[wasm_bindgen_test]
async fn text_oversize_overflow_and_abnormal_close_are_errors() {
    for port in [10102, 10103, 10105, 10107] {
        let mut stream = connect(port).await;
        poll_fn(|cx| Pin::new(&mut stream).poll_write(cx, b"start"))
            .await
            .unwrap();
        BrowserRuntime::delay(Duration::from_millis(100)).await;
        let mut buf = [0; 8192];
        // Buffered valid bytes can precede an error, but never a silent EOF.
        for attempt in 0..100 {
            match read(&mut stream, &mut buf).await {
                Err(_) => break,
                Ok(0) => panic!("failure incorrectly reported as EOF for {port}"),
                Ok(_) => assert!(attempt < 99, "missing terminal error for {port}"),
            }
        }
    }
}

#[wasm_bindgen_test]
async fn connection_refusal_returns_none() {
    configure_forwarder("ws://127.0.0.1:18766/stream").unwrap();
    assert!(
        BrowserTcpStream::connect("127.0.0.1:10101".parse().unwrap())
            .await
            .is_none()
    );
}

#[wasm_bindgen_test]
async fn output_queue_applies_backpressure_and_preserves_large_writes() {
    use std::task::Poll;
    let mut stream = connect(10108).await;
    let payload = vec![7_u8; 12 * 65536 + 17];
    let mut offset = 0;
    let mut saw_pending = false;
    let mut saw_partial = false;
    poll_fn(|cx| loop {
        if offset == payload.len() {
            return Poll::Ready(());
        }
        match Pin::new(&mut stream).poll_write(cx, &payload[offset..]) {
            Poll::Ready(Ok(n)) => {
                assert!(n > 0);
                saw_partial |= n < payload.len() - offset;
                offset += n;
            }
            Poll::Ready(Err(error)) => panic!("write failed: {error}"),
            Poll::Pending => {
                saw_pending = true;
                return Poll::Pending;
            }
        }
    })
    .await;
    assert!(saw_partial);
    assert!(saw_pending);
    write(&mut stream, b"done").await;
    let mut receipt = [0_u8; 16];
    let mut filled = 0;
    while filled < receipt.len() {
        let n = read(&mut stream, &mut receipt[filled..]).await.unwrap();
        assert!(n > 0);
        filled += n;
    }
    let received = u64::from_be_bytes(receipt[..8].try_into().unwrap());
    let checksum = u64::from_be_bytes(receipt[8..].try_into().unwrap());
    assert_eq!(received, payload.len() as u64);
    assert_eq!(checksum, payload.len() as u64 * 7);
}
