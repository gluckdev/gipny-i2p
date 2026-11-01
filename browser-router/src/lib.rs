//! Feasibility runtime, not a shipping router. JS handles remain on the local
//! browser executor; only channels cross emissary's Send/Sync boundaries.

mod metrics;
#[cfg(all(feature = "native-peer", not(target_arch = "wasm32")))]
pub mod native_fixture;
pub mod ntcp2;
pub mod router;
mod socket;
mod tasks;
mod unsupported;

use std::{
    future::Future,
    io::{Read, Write},
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use emissary_core::runtime::{Instant as RuntimeInstant, JoinSet, MetricType, Runtime};
use futures::{channel::oneshot, future::BoxFuture, stream::FuturesUnordered, Stream};
use rand::{CryptoRng, RngExt};
use wasm_bindgen::prelude::*;

pub use metrics::{metrics_snapshot, BrowserMetrics, MetricsSnapshot};
pub use socket::{configure_forwarder, configure_forwarder_authenticated, BrowserTcpStream};
pub use unsupported::{BrowserTcpListener, BrowserUdpSocket};

/// Maximum accepted decompressed I2P payload (also bounds untrusted gzip input).
pub const MAX_GZIP_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Default)]
pub struct BrowserRuntime;

pub struct BrowserJoinSet<T> {
    inner: FuturesUnordered<BoxFuture<'static, T>>,
    waker: Option<std::task::Waker>,
}

impl<T: Send + 'static> Stream for BrowserJoinSet<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(value)) => Poll::Ready(Some(value)),
            _ => {
                this.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

impl<T: Send + 'static> JoinSet<T> for BrowserJoinSet<T> {
    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn push<F>(&mut self, future: F)
    where
        F: Future<Output = T> + Send + 'static,
        F::Output: Send,
    {
        self.inner.push(Box::pin(future));
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub struct BrowserInstant(web_time::Instant);

impl RuntimeInstant for BrowserInstant {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
}

/// A timer exposes a Send channel, never the non-Send JavaScript timeout handle.
pub struct BrowserTimer(oneshot::Receiver<()>);

impl Future for BrowserTimer {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        Pin::new(&mut self.get_mut().0).poll(cx).map(|_| ())
    }
}

impl Runtime for BrowserRuntime {
    const TCP_LISTENER_SUPPORTED: bool = false;
    type TcpStream = BrowserTcpStream;
    type UdpSocket = BrowserUdpSocket;
    type TcpListener = BrowserTcpListener;
    type JoinSet<T: Send + 'static> = BrowserJoinSet<T>;
    type MetricsHandle = BrowserMetrics;
    type Instant = BrowserInstant;
    type Timer = BrowserTimer;

    fn spawn<F>(future: F)
    where
        F: Future + Send + 'static,
        F::Output: Send,
    {
        tasks::spawn(future);
    }

    fn time_since_epoch() -> Duration {
        web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .unwrap_or_default()
    }

    fn now() -> Self::Instant {
        BrowserInstant(web_time::Instant::now())
    }

    fn rng() -> impl CryptoRng + RngExt {
        rand::rng()
    }

    fn join_set<T: Send + 'static>() -> Self::JoinSet<T> {
        BrowserJoinSet {
            inner: FuturesUnordered::new(),
            waker: None,
        }
    }

    fn register_metrics(_: Vec<MetricType>, _: Option<u16>) -> BrowserMetrics {
        metrics::register()
    }

    fn timer(duration: Duration) -> BrowserTimer {
        let (mut tx, rx) = oneshot::channel();
        wasm_bindgen_futures::spawn_local(async move {
            // setTimeout's delay is a signed 32-bit integer. Round up sub-ms
            // durations and split long waits instead of wrapping/truncating.
            let mut millis =
                duration.as_millis() + u128::from(duration.subsec_nanos() % 1_000_000 != 0);
            while millis > 0 {
                let chunk = millis.min(i32::MAX as u128) as u32;
                let timeout = gloo_timers::future::TimeoutFuture::new(chunk);
                futures::pin_mut!(timeout);
                match futures::future::select(timeout, tx.cancellation()).await {
                    futures::future::Either::Left(_) => millis -= u128::from(chunk),
                    futures::future::Either::Right(_) => return,
                }
            }
            let _ = tx.send(());
        });
        BrowserTimer(rx)
    }

    fn delay(duration: Duration) -> impl Future<Output = ()> + Send {
        Self::timer(duration)
    }

    fn gzip_compress(bytes: impl AsRef<[u8]>) -> Option<Vec<u8>> {
        if bytes.as_ref().len() > MAX_GZIP_BYTES {
            return None;
        }
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(bytes.as_ref()).ok()?;
        encoder.finish().ok()
    }

    fn gzip_decompress(bytes: impl AsRef<[u8]>) -> Option<Vec<u8>> {
        if bytes.as_ref().len() > MAX_GZIP_BYTES {
            return None;
        }
        let mut decoder =
            flate2::read::GzDecoder::new(bytes.as_ref()).take((MAX_GZIP_BYTES + 1) as u64);
        let mut out = Vec::new();
        decoder.read_to_end(&mut out).ok()?;
        (out.len() <= MAX_GZIP_BYTES).then_some(out)
    }
}

/// Small linked export: exercise browser clock, entropy and gzip in the artifact.
#[wasm_bindgen]
pub fn runtime_probe() -> bool {
    let random: [u8; 32] = BrowserRuntime::rng().random();
    BrowserRuntime::time_since_epoch().as_secs() > 0
        && BrowserRuntime::gzip_compress(random)
            .and_then(BrowserRuntime::gzip_decompress)
            .as_deref()
            == Some(random.as_slice())
}
