//! Browser network wrapper around the in-process emissary I2P router and typed ClientSession.
//!
//! JS handles remain on the local browser worker executor; no raw JS objects
//! cross Send boundaries. All socket drivers and spawned tasks are tracked
//! under TaskScope and stopped cleanly on close, drop or failed startup.

#![cfg(target_arch = "wasm32")]

use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

use emissary_core::{
    crypto::base64_encode, router::RouterBuilder, runtime::Runtime, ClientIdentity, ClientSession,
};
use futures::{
    channel::oneshot,
    future::{select, Either, Shared},
    io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf},
    lock::Mutex as AsyncMutex,
    FutureExt,
};
use gloo_timers::future::TimeoutFuture;
use wasm_bindgen::prelude::*;

use crate::{
    bootstrap, configure_forwarder_authenticated,
    engine::BrowserEngine,
    router::{client_config, verify_no_listener},
    tasks::TaskScope,
    BrowserRuntime, BrowserTcpStream,
};

pub const MAX_CONCURRENT_STREAMS: usize = 16;
pub const MAX_ACCEPT_STREAMS: usize = 8;
pub const STARTUP_DEADLINE_MS: u32 = 300_000;
pub const CONNECT_DEADLINE_MS: u32 = 90_000;
pub const READ_TIMEOUT_MS: u32 = 120_000;
pub const WRITE_TIMEOUT_MS: u32 = 60_000;

fn bincode_options() -> impl bincode::Options {
    use bincode::Options;
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
        .with_limit(gipny_libcore::relay::MAX_FRAME as u64)
}

#[derive(serde::Deserialize)]
struct ConfigDto {
    snapshot_url: String,
    snapshot_sha256: String,
    forwarder_url: String,
    capability: String,
}

fn validate_config(config_json: &str) -> Result<ConfigDto, String> {
    let cfg: ConfigDto =
        serde_json::from_str(config_json).map_err(|e| format!("invalid config JSON: {e}"))?;

    let snap_url =
        url::Url::parse(&cfg.snapshot_url).map_err(|_| "invalid snapshot URL".to_string())?;
    let loopback = matches!(snap_url.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
        || matches!(snap_url.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback())
        || snap_url.host_str() == Some("localhost");
    if !(snap_url.scheme() == "https" || (snap_url.scheme() == "http" && loopback))
        || !snap_url.username().is_empty()
        || snap_url.password().is_some()
        || snap_url.fragment().is_some()
    {
        return Err(
            "snapshot URL requires HTTPS or loopback HTTP, without credentials/fragment".into(),
        );
    }

    if cfg.snapshot_sha256.len() != 64
        || !cfg.snapshot_sha256.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("snapshot SHA256 must contain 64 hex digits".into());
    }

    let fwd_url =
        url::Url::parse(&cfg.forwarder_url).map_err(|_| "invalid forwarder URL".to_string())?;
    let fwd_loopback = matches!(
        fwd_url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]")
    );
    if fwd_url.scheme() != "wss" && !(fwd_url.scheme() == "ws" && fwd_loopback) {
        return Err("forwarder requires wss, or ws on loopback".into());
    }
    if !fwd_url.username().is_empty()
        || fwd_url.password().is_some()
        || fwd_url.fragment().is_some()
    {
        return Err("forwarder credentials and fragments are not supported".into());
    }

    if cfg.capability.len() != 43
        || !cfg
            .capability
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err("capability must be a 32-byte unpadded base64url token".into());
    }

    Ok(cfg)
}

#[derive(serde::Serialize)]
struct BootStatus<'a> {
    stage: &'a str,
    state: &'a str,
    detail: &'a str,
}

fn emit_boot(on_event: &js_sys::Function, stage: &str, state: &str, detail: &str) {
    let status = BootStatus {
        stage,
        state,
        detail,
    };
    if let Ok(json) = serde_json::to_string(&status) {
        let global = js_sys::global();
        let arg = JsValue::from_str(&json);
        if let Err(e) = on_event.call1(&global, &arg) {
            web_sys::console::warn_2(&"network boot callback failed:".into(), &e);
        }
    }
}

/// Multi-waiter cancellation token powered by `futures::future::Shared`.
/// Safe, zero unsafe code, avoids AtomicWaker single-listener overwrites.
#[derive(Clone)]
pub(crate) struct CancellationToken {
    cancelled: Arc<AtomicBool>,
    shared_rx: Shared<oneshot::Receiver<()>>,
}

impl CancellationToken {
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        let _ = self.shared_rx.clone().await;
    }
}

pub(crate) struct CancellationSource {
    cancelled: Arc<AtomicBool>,
    tx: Option<oneshot::Sender<()>>,
    token: CancellationToken,
}

impl CancellationSource {
    pub fn new() -> Self {
        let (tx, rx) = oneshot::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let token = CancellationToken {
            cancelled: cancelled.clone(),
            shared_rx: rx.shared(),
        };
        Self {
            cancelled,
            tx: Some(tx),
            token,
        }
    }

    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    pub fn cancel(&mut self) {
        if !self.cancelled.swap(true, Ordering::SeqCst) {
            if let Some(tx) = self.tx.take() {
                let _ = tx.send(());
            }
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// RAII reservation of bounded concurrent stream permits.
/// Connects and accepts are counted before `.await`, preventing race condition overcommit.
struct StreamPermit {
    active_streams: Arc<AtomicUsize>,
    active_accepts: Option<Arc<AtomicUsize>>,
    transferred: bool,
}

impl StreamPermit {
    fn reserve_connect(active_streams: &Arc<AtomicUsize>) -> Result<Self, &'static str> {
        let current = active_streams.fetch_add(1, Ordering::SeqCst);
        if current >= MAX_CONCURRENT_STREAMS {
            active_streams.fetch_sub(1, Ordering::SeqCst);
            return Err("concurrent stream limit reached (max 16)");
        }
        Ok(Self {
            active_streams: active_streams.clone(),
            active_accepts: None,
            transferred: false,
        })
    }

    fn reserve_accept(
        active_streams: &Arc<AtomicUsize>,
        active_accepts: &Arc<AtomicUsize>,
    ) -> Result<Self, &'static str> {
        let current_accepts = active_accepts.fetch_add(1, Ordering::SeqCst);
        if current_accepts >= MAX_ACCEPT_STREAMS {
            active_accepts.fetch_sub(1, Ordering::SeqCst);
            return Err("concurrent accept limit reached (max 8)");
        }
        let current_total = active_streams.fetch_add(1, Ordering::SeqCst);
        if current_total >= MAX_CONCURRENT_STREAMS {
            active_streams.fetch_sub(1, Ordering::SeqCst);
            active_accepts.fetch_sub(1, Ordering::SeqCst);
            return Err("concurrent stream limit reached (max 16)");
        }
        Ok(Self {
            active_streams: active_streams.clone(),
            active_accepts: Some(active_accepts.clone()),
            transferred: false,
        })
    }

    fn transfer(mut self) -> StreamCountGuard {
        self.transferred = true;
        if let Some(accepts) = self.active_accepts.take() {
            accepts.fetch_sub(1, Ordering::SeqCst);
        }
        StreamCountGuard {
            active_streams: self.active_streams.clone(),
            active_accepts: None,
            decremented: AtomicBool::new(false),
        }
    }
}

impl Drop for StreamPermit {
    fn drop(&mut self) {
        if !self.transferred {
            self.active_streams.fetch_sub(1, Ordering::SeqCst);
            if let Some(accepts) = &self.active_accepts {
                accepts.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }
}

pub(crate) struct StreamCountGuard {
    active_streams: Arc<AtomicUsize>,
    active_accepts: Option<Arc<AtomicUsize>>,
    decremented: AtomicBool,
}

impl StreamCountGuard {
    pub fn release(&self) {
        if !self.decremented.swap(true, Ordering::SeqCst) {
            self.active_streams.fetch_sub(1, Ordering::SeqCst);
            if let Some(accepts) = &self.active_accepts {
                accepts.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }
}

impl Drop for StreamCountGuard {
    fn drop(&mut self) {
        self.release();
    }
}

struct NetworkInner {
    destination: String,
    session: Mutex<Option<Arc<ClientSession<BrowserRuntime>>>>,
    scope: Mutex<Option<TaskScope>>,
    cancellation_source: Arc<Mutex<CancellationSource>>,
    cancellation_token: CancellationToken,
    fatal_error: Arc<Mutex<Option<String>>>,
    active_streams: Arc<AtomicUsize>,
    active_accepts: Arc<AtomicUsize>,
}

impl NetworkInner {
    async fn connect(&self, address: &str, port: u16) -> Result<BrowserStream, String> {
        if let Some(err) = self.fatal_error.lock().unwrap().as_ref() {
            return Err(err.clone());
        }
        if self.cancellation_token.is_cancelled() {
            return Err("network closed".into());
        }

        let permit = StreamPermit::reserve_connect(&self.active_streams)?;

        let session = self
            .session
            .lock()
            .unwrap()
            .as_ref()
            .cloned()
            .ok_or_else(|| "network closed".to_string())?;

        let connect_op = session.connect(address, port);
        let timeout = TimeoutFuture::new(CONNECT_DEADLINE_MS);
        let cancellation = self.cancellation_token.cancelled();
        futures::pin_mut!(connect_op);
        futures::pin_mut!(timeout);
        futures::pin_mut!(cancellation);

        let tcp_stream = match select(connect_op, select(cancellation, timeout)).await {
            Either::Left((res, _)) => res.map_err(|e| format!("connect failed: {e:?}"))?,
            Either::Right((Either::Left(_), _)) => {
                if let Some(err) = self.fatal_error.lock().unwrap().as_ref() {
                    return Err(err.clone());
                }
                return Err("network closed".into());
            }
            Either::Right((Either::Right(_), _)) => {
                return Err("connect deadline exceeded (90s)".into());
            }
        };

        let stream_guard = permit.transfer();
        let (read_half, write_half) = tcp_stream.split();

        let stream_inner = Arc::new(StreamInner {
            remote_destination: address.to_string(),
            reader: AsyncMutex::new(Some(read_half)),
            writer: AsyncMutex::new(Some(write_half)),
            cancellation_source: Mutex::new(CancellationSource::new()),
            network_cancellation: self.cancellation_token.clone(),
            fatal_error: self.fatal_error.clone(),
            _guard: stream_guard,
        });

        Ok(BrowserStream {
            inner: stream_inner,
        })
    }

    async fn accept(&self) -> Result<BrowserStream, String> {
        if let Some(err) = self.fatal_error.lock().unwrap().as_ref() {
            return Err(err.clone());
        }
        if self.cancellation_token.is_cancelled() {
            return Err("network closed".into());
        }

        let permit = StreamPermit::reserve_accept(&self.active_streams, &self.active_accepts)?;

        let session = self
            .session
            .lock()
            .unwrap()
            .as_ref()
            .cloned()
            .ok_or_else(|| "network closed".to_string())?;

        let accept_op = session.accept();
        let cancellation = self.cancellation_token.cancelled();
        futures::pin_mut!(accept_op);
        futures::pin_mut!(cancellation);

        let (tcp_stream, destination) = match select(accept_op, cancellation).await {
            Either::Left((res, _)) => res.map_err(|e| format!("accept failed: {e:?}"))?,
            Either::Right(_) => {
                if let Some(err) = self.fatal_error.lock().unwrap().as_ref() {
                    return Err(err.clone());
                }
                return Err("network closed".into());
            }
        };

        let stream_guard = permit.transfer();
        let remote_dest = base64_encode(destination.serialize());
        let (read_half, write_half) = tcp_stream.split();

        let stream_inner = Arc::new(StreamInner {
            remote_destination: remote_dest,
            reader: AsyncMutex::new(Some(read_half)),
            writer: AsyncMutex::new(Some(write_half)),
            cancellation_source: Mutex::new(CancellationSource::new()),
            network_cancellation: self.cancellation_token.clone(),
            fatal_error: self.fatal_error.clone(),
            _guard: stream_guard,
        });

        Ok(BrowserStream {
            inner: stream_inner,
        })
    }

    fn close(&self) {
        self.cancellation_source.lock().unwrap().cancel();
        self.session.lock().unwrap().take();
        if let Some(scope) = self.scope.lock().unwrap().take() {
            scope.abort_all();
            drop(scope);
        }
    }
}

#[wasm_bindgen]
pub struct BrowserNetwork {
    inner: Arc<NetworkInner>,
}

impl BrowserNetwork {
    pub async fn start(
        identity: ClientIdentity,
        config_json: &str,
        on_event: js_sys::Function,
    ) -> Result<BrowserNetwork, JsValue> {
        let config = validate_config(config_json).map_err(|e| JsValue::from_str(&e))?;

        configure_forwarder_authenticated(&config.forwarder_url, &config.capability)
            .map_err(JsValue::from_str)?;

        let scope = TaskScope::enter().map_err(JsValue::from_str)?;

        let fatal_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let fatal_writer = fatal_error.clone();
        let cancellation_source = Arc::new(Mutex::new(CancellationSource::new()));
        let cancellation_token = cancellation_source.lock().unwrap().token();
        let cancel_source_for_router = cancellation_source.clone();

        let on_event_ref = &on_event;

        let startup = async {
            emit_boot(
                on_event_ref,
                "router",
                "active",
                "Fetching NetDb bootstrap snapshot",
            );
            let snapshot = bootstrap::fetch_snapshot(&config.snapshot_url, &config.snapshot_sha256)
                .await
                .map_err(|e| format!("fetch snapshot failed: {e}"))?;

            emit_boot(
                on_event_ref,
                "router",
                "active",
                "Verifying peer signatures and initializing router",
            );
            let (router, _events, local_info) =
                RouterBuilder::<BrowserRuntime>::new(client_config(snapshot.routers))
                    .build()
                    .await
                    .map_err(|e| format!("RouterBuilder failed: {e:?}"))?;

            verify_no_listener(router.protocol_address_info(), &local_info)
                .map_err(|e| format!("listener check: {e}"))?;

            emit_boot(
                on_event_ref,
                "router",
                "done",
                "Router initialized with client NTCP2 transport",
            );

            let client_handle = router.client_handle();

            BrowserRuntime::spawn(async move {
                router.await;
                // TaskScope cancellation drops this future before reaching
                // this tail. Any observed router completion is unexpected.
                *fatal_writer.lock().unwrap() = Some("emissary router stopped unexpectedly".into());
                cancel_source_for_router.lock().unwrap().cancel();
            });

            emit_boot(
                on_event_ref,
                "tunnels",
                "active",
                "Building client exploratory and destination pools",
            );
            emit_boot(
                on_event_ref,
                "session",
                "active",
                "Creating in-process I2P client session",
            );

            let session = client_handle
                .create(&identity)
                .await
                .map_err(|e| format!("ClientSession::create failed: {e:?}"))?;

            emit_boot(
                on_event_ref,
                "session",
                "done",
                "Client session established and tunnel pool ready",
            );

            let dest_b64 = base64_encode(session.destination().serialize());
            Ok::<_, String>((dest_b64, session))
        };

        let timeout = TimeoutFuture::new(STARTUP_DEADLINE_MS);
        futures::pin_mut!(startup);
        futures::pin_mut!(timeout);

        let (dest_b64, session) = match select(startup, timeout).await {
            Either::Left((Ok(res), _)) => res,
            Either::Left((Err(e), _)) => {
                drop(scope);
                let sanitized = e.replace(&config.capability, "[REDACTED]");
                emit_boot(on_event_ref, "router", "failed", &sanitized);
                return Err(JsValue::from_str(&sanitized));
            }
            Either::Right(_) => {
                drop(scope);
                emit_boot(
                    on_event_ref,
                    "router",
                    "failed",
                    "startup deadline exceeded (300s)",
                );
                return Err(JsValue::from_str("startup deadline exceeded (300s)"));
            }
        };

        Ok(BrowserNetwork {
            inner: Arc::new(NetworkInner {
                destination: dest_b64,
                session: Mutex::new(Some(Arc::new(session))),
                scope: Mutex::new(Some(scope)),
                cancellation_source,
                cancellation_token,
                fatal_error,
                active_streams: Arc::new(AtomicUsize::new(0)),
                active_accepts: Arc::new(AtomicUsize::new(0)),
            }),
        })
    }
}

#[wasm_bindgen]
impl BrowserNetwork {
    pub fn destination(&self) -> String {
        self.inner.destination.clone()
    }

    /// Synchronous method returning Promise to avoid holding wasm-bindgen `&self`
    /// across await points. Prevents aliasing traps if JS calls close/free during connect.
    pub fn connect(&self, address: &str, port: u16) -> js_sys::Promise {
        let inner = self.inner.clone();
        let address = address.to_string();
        wasm_bindgen_futures::future_to_promise(async move {
            let stream = inner
                .connect(&address, port)
                .await
                .map_err(|e| JsValue::from_str(&e))?;
            Ok(JsValue::from(stream))
        })
    }

    /// Synchronous method returning Promise to avoid holding wasm-bindgen `&self`
    /// across await points.
    pub fn accept(&self) -> js_sys::Promise {
        let inner = self.inner.clone();
        wasm_bindgen_futures::future_to_promise(async move {
            let stream = inner.accept().await.map_err(|e| JsValue::from_str(&e))?;
            Ok(JsValue::from(stream))
        })
    }

    pub fn close(&self) {
        self.inner.close();
    }
}

impl Drop for BrowserNetwork {
    fn drop(&mut self) {
        self.close();
    }
}

struct StreamInner {
    remote_destination: String,
    reader: AsyncMutex<Option<ReadHalf<BrowserTcpStream>>>,
    writer: AsyncMutex<Option<WriteHalf<BrowserTcpStream>>>,
    cancellation_source: Mutex<CancellationSource>,
    network_cancellation: CancellationToken,
    fatal_error: Arc<Mutex<Option<String>>>,
    _guard: StreamCountGuard,
}

impl StreamInner {
    fn is_cancelled(&self) -> bool {
        self.cancellation_source.lock().unwrap().is_cancelled()
            || self.network_cancellation.is_cancelled()
    }

    fn close(&self) {
        self.cancellation_source.lock().unwrap().cancel();
        self._guard.release();
    }

    async fn read_frame(&self) -> Result<Vec<u8>, String> {
        if let Some(err) = self.fatal_error.lock().unwrap().as_ref() {
            return Err(err.clone());
        }
        if self.is_cancelled() {
            return Err("stream closed".into());
        }

        let mut reader_guard = self.reader.lock().await;
        let reader = reader_guard
            .as_mut()
            .ok_or_else(|| "stream closed".to_string())?;

        let read_future = async {
            let mut len_bytes = [0u8; 4];
            reader
                .read_exact(&mut len_bytes)
                .await
                .map_err(|e| format!("peer closed connection: {e}"))?;

            let len = u32::from_be_bytes(len_bytes);
            if len == 0 {
                return Err("empty frame not allowed".to_string());
            }
            if len > gipny_libcore::relay::MAX_FRAME {
                return Err(format!("frame size {len} exceeds MAX_FRAME"));
            }

            let mut payload = vec![0u8; len as usize];
            reader
                .read_exact(&mut payload)
                .await
                .map_err(|e| format!("peer closed during frame payload: {e}"))?;

            Ok(payload)
        };

        let stream_cancel = self.cancellation_source.lock().unwrap().token();
        let cancel_future = async {
            futures::select! {
                _ = stream_cancel.cancelled().fuse() => (),
                _ = self.network_cancellation.cancelled().fuse() => (),
            }
        };

        let timeout = TimeoutFuture::new(READ_TIMEOUT_MS);
        futures::pin_mut!(read_future);
        futures::pin_mut!(cancel_future);
        futures::pin_mut!(timeout);

        let result = select(read_future, select(cancel_future, timeout)).await;

        match result {
            Either::Left((Ok(payload), _)) => Ok(payload),
            Either::Left((Err(e), _)) => {
                self.close();
                Err(e)
            }
            Either::Right((Either::Left(_), _)) => {
                if let Some(err) = self.fatal_error.lock().unwrap().as_ref() {
                    return Err(err.clone());
                }
                Err("stream closed".into())
            }
            Either::Right((Either::Right(_), _)) => {
                self.close();
                Err("read frame timed out (120s)".into())
            }
        }
    }

    async fn write_frame(&self, bytes: &[u8]) -> Result<(), String> {
        if let Some(err) = self.fatal_error.lock().unwrap().as_ref() {
            return Err(err.clone());
        }
        if self.is_cancelled() {
            return Err("stream closed".into());
        }
        if bytes.is_empty() {
            return Err("empty frame not allowed".into());
        }
        if bytes.len() > gipny_libcore::relay::MAX_FRAME as usize {
            return Err("frame exceeds MAX_FRAME".into());
        }

        let mut writer_guard = self.writer.lock().await;
        let writer = writer_guard
            .as_mut()
            .ok_or_else(|| "stream closed".to_string())?;

        let len_bytes = (bytes.len() as u32).to_be_bytes();

        let write_future = async {
            writer
                .write_all(&len_bytes)
                .await
                .map_err(|e| format!("write length failed: {e}"))?;
            writer
                .write_all(bytes)
                .await
                .map_err(|e| format!("write payload failed: {e}"))?;
            writer
                .flush()
                .await
                .map_err(|e| format!("flush failed: {e}"))?;
            Ok(())
        };

        let stream_cancel = self.cancellation_source.lock().unwrap().token();
        let cancel_future = async {
            futures::select! {
                _ = stream_cancel.cancelled().fuse() => (),
                _ = self.network_cancellation.cancelled().fuse() => (),
            }
        };

        let timeout = TimeoutFuture::new(WRITE_TIMEOUT_MS);
        futures::pin_mut!(write_future);
        futures::pin_mut!(cancel_future);
        futures::pin_mut!(timeout);

        let result = select(write_future, select(cancel_future, timeout)).await;

        match result {
            Either::Left((Ok(()), _)) => Ok(()),
            Either::Left((Err(e), _)) => {
                self.close();
                Err(e)
            }
            Either::Right((Either::Left(_), _)) => {
                if let Some(err) = self.fatal_error.lock().unwrap().as_ref() {
                    return Err(err.clone());
                }
                Err("stream closed".into())
            }
            Either::Right((Either::Right(_), _)) => {
                self.close();
                Err("write frame timed out (60s)".into())
            }
        }
    }
}

#[wasm_bindgen]
pub struct BrowserStream {
    inner: Arc<StreamInner>,
}

#[wasm_bindgen]
impl BrowserStream {
    pub fn remote_destination(&self) -> String {
        self.inner.remote_destination.clone()
    }

    /// Synchronous method returning Promise to avoid holding wasm-bindgen `&self`
    /// across await points.
    pub fn read_frame(&self) -> js_sys::Promise {
        let inner = self.inner.clone();
        wasm_bindgen_futures::future_to_promise(async move {
            let payload = inner
                .read_frame()
                .await
                .map_err(|e| JsValue::from_str(&e))?;
            let array = js_sys::Uint8Array::new_with_length(payload.len() as u32);
            array.copy_from(&payload);
            Ok(JsValue::from(array))
        })
    }

    /// Synchronous method returning Promise to avoid holding wasm-bindgen `&self`
    /// across await points.
    pub fn write_frame(&self, bytes: &[u8]) -> js_sys::Promise {
        let inner = self.inner.clone();
        let bytes = bytes.to_vec();
        wasm_bindgen_futures::future_to_promise(async move {
            inner
                .write_frame(&bytes)
                .await
                .map_err(|e| JsValue::from_str(&e))?;
            Ok(JsValue::UNDEFINED)
        })
    }

    pub fn close(&self) {
        self.inner.close();
    }
}

impl Drop for BrowserStream {
    fn drop(&mut self) {
        self.close();
    }
}

#[wasm_bindgen]
impl BrowserEngine {
    /// Synchronously extracts identity and returns Promise, ensuring `&self`
    /// is NOT borrowed across await points. engine.close()/free() will never trap.
    #[wasm_bindgen(js_name = start_network)]
    pub fn start_network(
        &self,
        config: &str,
        event: js_sys::Function,
    ) -> Result<js_sys::Promise, JsValue> {
        let identity = self
            .destination_identity()
            .map_err(|e| JsValue::from_str(&e))?;
        let config_str = config.to_string();
        let lifetime = self.lifetime();
        let (abort, registration) = futures::future::AbortHandle::new_pair();
        lifetime.on_close(move || abort.abort());
        Ok(wasm_bindgen_futures::future_to_promise(async move {
            let network = futures::future::Abortable::new(
                BrowserNetwork::start(identity, &config_str, event),
                registration,
            )
            .await
            .map_err(|_| JsValue::from_str("profile locked during network startup"))??;
            let inner = network.inner.clone();
            lifetime.on_close(move || inner.close());
            Ok(JsValue::from(network))
        }))
    }
}

#[wasm_bindgen]
pub fn encode_client_frame(json: &str) -> Result<Vec<u8>, JsValue> {
    use bincode::Options;
    let frame: gipny_libcore::relay::ClientToRelay = serde_json::from_str(json)
        .map_err(|e| JsValue::from_str(&format!("invalid client frame JSON: {e}")))?;
    bincode_options()
        .serialize(&frame)
        .map_err(|e| JsValue::from_str(&format!("encode client frame failed: {e}")))
}

#[wasm_bindgen]
pub fn decode_client_frame(bytes: &[u8]) -> Result<String, JsValue> {
    use bincode::Options;
    let frame: gipny_libcore::relay::ClientToRelay = bincode_options()
        .deserialize(bytes)
        .map_err(|e| JsValue::from_str(&format!("decode client frame failed: {e}")))?;
    serde_json::to_string(&frame)
        .map_err(|e| JsValue::from_str(&format!("serialize client frame to JSON failed: {e}")))
}

#[wasm_bindgen]
pub fn encode_relay_frame(json: &str) -> Result<Vec<u8>, JsValue> {
    use bincode::Options;
    let frame: gipny_libcore::relay::RelayToClient = serde_json::from_str(json)
        .map_err(|e| JsValue::from_str(&format!("invalid relay frame JSON: {e}")))?;
    bincode_options()
        .serialize(&frame)
        .map_err(|e| JsValue::from_str(&format!("encode relay frame failed: {e}")))
}

#[wasm_bindgen]
pub fn decode_relay_frame(bytes: &[u8]) -> Result<String, JsValue> {
    use bincode::Options;
    let frame: gipny_libcore::relay::RelayToClient = bincode_options()
        .deserialize(bytes)
        .map_err(|e| JsValue::from_str(&format!("decode relay frame failed: {e}")))?;
    serde_json::to_string(&frame)
        .map_err(|e| JsValue::from_str(&format!("serialize relay frame to JSON failed: {e}")))
}
