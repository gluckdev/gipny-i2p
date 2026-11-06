use std::{
    cell::RefCell,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};

use emissary_core::{
    runtime::{AsyncRead, AsyncWrite, TcpStream},
    Error,
};
use futures::{
    channel::{mpsc, oneshot},
    task::AtomicWaker,
    Sink, SinkExt, Stream, StreamExt,
};
use wasm_bindgen::{closure::Closure, prelude::wasm_bindgen, JsCast, JsValue};
use web_sys::{CloseEvent, Event, MessageEvent, WebSocket};

pub const MAX_FRAME: usize = 64 * 1024;
const QUEUE_DEPTH: usize = 8;
/// Bytes handed to JS but not yet written: a write waits above
/// MAX_BUFFERED - MAX_FRAME, so at most 128 KiB is outstanding. The WebRTC
/// adapter (`ui/src/browser-core/rtc-transport.ts`, SEND_BUDGET) enforces the
/// same byte budget in both the Worker and the Window.
const MAX_BUFFERED: u32 = 2 * MAX_FRAME as u32;
const IO_TIMEOUT_MS: f64 = 30_000.0;

thread_local! {
    static FORWARDER: RefCell<Option<(url::Url, Option<String>, bool)>> = const { RefCell::new(None) };
}

/// Authenticate through a WebSocket subprotocol, keeping the capability out of
/// URLs and access logs. The forwarder must echo the selected subprotocol.
pub fn configure_forwarder_authenticated(
    endpoint: &str,
    capability: &str,
) -> Result<(), &'static str> {
    if capability.len() != 43
        || !capability
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err("capability must be a 32-byte unpadded base64url token");
    }
    configure_forwarder(endpoint)?;
    FORWARDER.with(|value| {
        if let Some((_, protocol, _)) = value.borrow_mut().as_mut() {
            *protocol = Some(format!("gipny.ntcp2.v1.{capability}"));
        }
    });
    Ok(())
}

/// Configure the byte forwarder's endpoint for this browser thread. Target
/// selection is a `target=<IP:port>` query parameter; authentication/allowlisting
/// must be implemented by the forwarder before using it outside CI/loopback.
pub fn configure_forwarder(endpoint: &str) -> Result<(), &'static str> {
    let url = url::Url::parse(endpoint).map_err(|_| "invalid forwarder URL")?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if url.scheme() != "wss" && !(url.scheme() == "ws" && loopback) {
        return Err("forwarder requires wss, or ws on loopback");
    }
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err("credentials and fragments are not supported");
    }
    FORWARDER.with(|value| *value.borrow_mut() = Some((url, None, false)));
    Ok(())
}

// Called only from the wasm-only `network` module.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn configure_transport(
    endpoint: &str,
    capability: &str,
    transport: &str,
) -> Result<(), &'static str> {
    if !matches!(transport, "wss" | "webrtc") {
        return Err("unknown browser transport");
    }
    configure_forwarder_authenticated(endpoint, capability)?;
    FORWARDER.with(|value| {
        if let Some((_, _, rtc)) = value.borrow_mut().as_mut() {
            *rtc = transport == "webrtc";
        }
    });
    Ok(())
}

#[wasm_bindgen]
extern "C" {
    #[derive(Clone)]
    type RtcSocket;
    #[wasm_bindgen(catch, js_namespace = globalThis, js_name = gipnyRtcSocket)]
    fn create_rtc_socket(endpoint: &str, protocol: &str) -> Result<RtcSocket, JsValue>;
    #[wasm_bindgen(method, setter, js_name = onopen)]
    fn rtc_onopen(this: &RtcSocket, value: Option<&js_sys::Function>);
    #[wasm_bindgen(method, setter, js_name = onmessage)]
    fn rtc_onmessage(this: &RtcSocket, value: Option<&js_sys::Function>);
    #[wasm_bindgen(method, setter, js_name = onerror)]
    fn rtc_onerror(this: &RtcSocket, value: Option<&js_sys::Function>);
    #[wasm_bindgen(method, setter, js_name = onclose)]
    fn rtc_onclose(this: &RtcSocket, value: Option<&js_sys::Function>);
    #[wasm_bindgen(method, getter, js_name = bufferedAmount)]
    fn rtc_buffered(this: &RtcSocket) -> u32;
    #[wasm_bindgen(method, getter, js_name = protocol)]
    fn rtc_protocol(this: &RtcSocket) -> String;
    #[wasm_bindgen(method, catch, js_name = send)]
    fn rtc_send(this: &RtcSocket, bytes: &[u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(method, catch, js_name = close)]
    fn rtc_close(this: &RtcSocket) -> Result<(), JsValue>;
}

#[derive(Clone)]
enum TransportSocket {
    Ws(WebSocket),
    Rtc(RtcSocket),
}
impl TransportSocket {
    fn set_onopen(&self, f: Option<&js_sys::Function>) {
        match self {
            Self::Ws(s) => s.set_onopen(f),
            Self::Rtc(s) => s.rtc_onopen(f),
        }
    }
    fn set_onmessage(&self, f: Option<&js_sys::Function>) {
        match self {
            Self::Ws(s) => s.set_onmessage(f),
            Self::Rtc(s) => s.rtc_onmessage(f),
        }
    }
    fn set_onerror(&self, f: Option<&js_sys::Function>) {
        match self {
            Self::Ws(s) => s.set_onerror(f),
            Self::Rtc(s) => s.rtc_onerror(f),
        }
    }
    fn set_onclose(&self, f: Option<&js_sys::Function>) {
        match self {
            Self::Ws(s) => s.set_onclose(f),
            Self::Rtc(s) => s.rtc_onclose(f),
        }
    }
    fn close(&self) -> Result<(), JsValue> {
        match self {
            Self::Ws(s) => s.close(),
            Self::Rtc(s) => s.rtc_close(),
        }
    }
    fn protocol(&self) -> String {
        match self {
            Self::Ws(s) => s.protocol(),
            Self::Rtc(s) => s.rtc_protocol(),
        }
    }
    fn buffered_amount(&self) -> u32 {
        match self {
            Self::Ws(s) => s.buffered_amount(),
            Self::Rtc(s) => s.rtc_buffered(),
        }
    }
    fn send_with_u8_array(&self, bytes: &[u8]) -> Result<(), JsValue> {
        match self {
            Self::Ws(s) => s.send_with_u8_array(bytes),
            Self::Rtc(s) => s.rtc_send(bytes),
        }
    }
}

#[derive(Default)]
struct Terminal {
    closed: AtomicBool,
    failed: AtomicBool,
    pending_writes: AtomicUsize,
    clean_drained: AtomicBool,
    waker: AtomicWaker,
}

impl Terminal {
    fn flushed_on_close(&self) -> bool {
        self.clean_drained.load(Ordering::SeqCst) && !self.failed.load(Ordering::SeqCst)
    }

    fn finish(&self, failed: bool) {
        if failed {
            self.failed.store(true, Ordering::SeqCst);
        }
        self.closed.store(true, Ordering::SeqCst);
        self.waker.wake();
    }
}

enum Command {
    Write(Vec<u8>),
    Flush(oneshot::Sender<bool>),
}

/// This type contains Rust channels only, satisfying upstream Send + Sync
/// without unsafe implementations or exposing JS objects to router futures.
pub struct BrowserTcpStream {
    commands: mpsc::Sender<Command>,
    incoming: mpsc::Receiver<Vec<u8>>,
    terminal: Arc<Terminal>,
    buffered: Vec<u8>,
    cursor: usize,
    flushing: Option<oneshot::Receiver<bool>>,
    write_closed: bool,
}

impl BrowserTcpStream {
    /// Bounded byte pipe for an application inside the WASM process. This never
    /// opens a WebSocket, TCP listener, or UDP socket.
    pub fn memory_pair() -> (Self, Self) {
        let (commands_a, receiver_a) = mpsc::channel(QUEUE_DEPTH);
        let (commands_b, receiver_b) = mpsc::channel(QUEUE_DEPTH);
        let (incoming_a, frames_a) = mpsc::channel(QUEUE_DEPTH);
        let (incoming_b, frames_b) = mpsc::channel(QUEUE_DEPTH);
        let terminal_a = Arc::new(Terminal::default());
        let terminal_b = Arc::new(Terminal::default());
        crate::tasks::spawn(memory_driver(
            receiver_a,
            incoming_b,
            terminal_a.clone(),
            terminal_b.clone(),
        ));
        crate::tasks::spawn(memory_driver(
            receiver_b,
            incoming_a,
            terminal_b.clone(),
            terminal_a.clone(),
        ));
        let endpoint = |commands, incoming, terminal| Self {
            commands,
            incoming,
            terminal,
            buffered: Vec::new(),
            cursor: 0,
            flushing: None,
            write_closed: false,
        };
        (
            endpoint(commands_a, frames_a, terminal_a),
            endpoint(commands_b, frames_b, terminal_b),
        )
    }
}

struct MemoryClose(Arc<Terminal>);
impl Drop for MemoryClose {
    fn drop(&mut self) {
        self.0.clean_drained.store(
            self.0.pending_writes.load(Ordering::SeqCst) == 0,
            Ordering::SeqCst,
        );
        self.0.finish(false);
    }
}

async fn memory_driver(
    mut receiver: mpsc::Receiver<Command>,
    mut peer: mpsc::Sender<Vec<u8>>,
    own: Arc<Terminal>,
    remote: Arc<Terminal>,
) {
    let _closed = MemoryClose(remote);
    while let Some(command) = receiver.next().await {
        match command {
            Command::Write(bytes) => {
                if peer.send(bytes).await.is_err() {
                    own.finish(true);
                    return;
                }
                own.pending_writes.fetch_sub(1, Ordering::SeqCst);
            }
            Command::Flush(ack) => {
                let _ = ack.send(true);
            }
        }
    }
}

impl TcpStream for BrowserTcpStream {
    fn connect(address: SocketAddr) -> impl Future<Output = Option<Self>> + Send {
        // Start JS work synchronously; the returned future captures only a
        // oneshot receiver, not a WebSocket, Closure, Rc or JS timer.
        let endpoint = FORWARDER.with(|v| v.borrow().clone());
        let (reply, result) = oneshot::channel();
        // The forwarder refuses these ports too; failing locally avoids a
        // pointless bridge request for a RouterInfo that names a service port.
        let endpoint = endpoint.filter(|_| crate::ntcp2::is_allowed_ntcp2_port(address.port()));
        if let Some((mut endpoint, protocol, rtc)) = endpoint {
            // Do not allow a configured query to shadow the selected target.
            let query: Vec<_> = endpoint
                .query_pairs()
                .filter(|(k, _)| k != "target")
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            endpoint.set_query(None);
            endpoint
                .query_pairs_mut()
                .extend_pairs(query)
                .append_pair("target", &address.to_string());
            crate::tasks::spawn_local(run_socket(endpoint.into(), protocol, rtc, reply));
        }
        async move { result.await.ok().flatten() }
    }
}

impl AsyncRead for BrowserTcpStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<emissary_core::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let this = self.get_mut();
        this.terminal.waker.register(cx.waker());
        loop {
            if this.cursor < this.buffered.len() {
                let n = buf.len().min(this.buffered.len() - this.cursor);
                buf[..n].copy_from_slice(&this.buffered[this.cursor..this.cursor + n]);
                this.cursor += n;
                return Poll::Ready(Ok(n));
            }
            this.buffered.clear();
            this.cursor = 0;
            match Pin::new(&mut this.incoming).poll_next(cx) {
                Poll::Ready(Some(frame)) => this.buffered = frame,
                state => {
                    if this.terminal.closed.load(Ordering::SeqCst)
                        || matches!(state, Poll::Ready(None))
                    {
                        return Poll::Ready(if this.terminal.failed.load(Ordering::SeqCst) {
                            Err(Error::DialFailure)
                        } else {
                            Ok(0)
                        });
                    }
                    return Poll::Pending;
                }
            }
        }
    }
}

impl AsyncWrite for BrowserTcpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<emissary_core::Result<usize>> {
        let this = self.get_mut();
        if this.write_closed || this.terminal.closed.load(Ordering::SeqCst) {
            return Poll::Ready(Err(Error::DialFailure));
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        match futures::ready!(Pin::new(&mut this.commands).poll_ready(cx)) {
            Err(_) => Poll::Ready(Err(Error::DialFailure)),
            Ok(()) => {
                let n = buf.len().min(MAX_FRAME);
                this.terminal.pending_writes.fetch_add(1, Ordering::SeqCst);
                let sent =
                    Pin::new(&mut this.commands).start_send(Command::Write(buf[..n].to_vec()));
                if sent.is_err() {
                    this.terminal.pending_writes.fetch_sub(1, Ordering::SeqCst);
                }
                Poll::Ready(sent.map(|_| n).map_err(|_| Error::DialFailure))
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<emissary_core::Result<()>> {
        let this = self.get_mut();
        if this.write_closed {
            return Poll::Ready(Ok(()));
        }
        // A peer may finish a clean close after receiving all our writes but
        // before the local actor handles the flush command. Preserve success
        // only when onclose observed no queued writes and bufferedAmount == 0.
        if this.terminal.closed.load(Ordering::SeqCst) {
            return Poll::Ready(if this.terminal.flushed_on_close() {
                Ok(())
            } else {
                Err(Error::DialFailure)
            });
        }
        if this.flushing.is_none() {
            if futures::ready!(Pin::new(&mut this.commands).poll_ready(cx)).is_err() {
                return Poll::Ready(Err(Error::DialFailure));
            }
            let (tx, rx) = oneshot::channel();
            if Pin::new(&mut this.commands)
                .start_send(Command::Flush(tx))
                .is_err()
            {
                return Poll::Ready(Err(Error::DialFailure));
            }
            this.flushing = Some(rx);
        }
        let result = futures::ready!(Pin::new(this.flushing.as_mut().unwrap()).poll(cx));
        this.flushing = None;
        Poll::Ready(
            if matches!(result, Ok(true)) || this.terminal.flushed_on_close() {
                Ok(())
            } else {
                Err(Error::DialFailure)
            },
        )
    }

    fn poll_close(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<emissary_core::Result<()>> {
        futures::ready!(self.as_mut().poll_flush(cx))?;
        self.write_closed = true;
        self.commands.close_channel();
        Poll::Ready(Ok(()))
    }
}

impl futures::io::AsyncRead for BrowserTcpStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        match emissary_core::runtime::AsyncRead::poll_read(self, cx, buf) {
            Poll::Ready(Ok(n)) => Poll::Ready(Ok(n)),
            Poll::Ready(Err(e)) => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("{e:?}"),
            ))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl futures::io::AsyncWrite for BrowserTcpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match emissary_core::runtime::AsyncWrite::poll_write(self, cx, buf) {
            Poll::Ready(Ok(n)) => Poll::Ready(Ok(n)),
            Poll::Ready(Err(e)) => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("{e:?}"),
            ))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match emissary_core::runtime::AsyncWrite::poll_flush(self, cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("{e:?}"),
            ))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match emissary_core::runtime::AsyncWrite::poll_close(self, cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("{e:?}"),
            ))),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Drop always detaches handlers, breaking JS/Rust reference cycles, and closes
/// the socket. This also handles cancellation during open or failed handoff.
struct SocketGuard {
    ws: TransportSocket,
    _open: Closure<dyn FnMut(Event)>,
    _message: Closure<dyn FnMut(MessageEvent)>,
    _error: Closure<dyn FnMut(Event)>,
    _close: Closure<dyn FnMut(CloseEvent)>,
    terminal: Arc<Terminal>,
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        self.ws.set_onopen(None);
        self.ws.set_onmessage(None);
        self.ws.set_onerror(None);
        self.ws.set_onclose(None);
        let _ = self.ws.close();
        self.terminal.finish(false);
    }
}

async fn run_socket(
    endpoint: String,
    protocol: Option<String>,
    rtc: bool,
    reply: oneshot::Sender<Option<BrowserTcpStream>>,
) {
    let socket = match (rtc, protocol.as_deref()) {
        (true, Some(protocol)) => create_rtc_socket(&endpoint, protocol).map(TransportSocket::Rtc),
        (true, None) => return,
        (false, Some(protocol)) => {
            WebSocket::new_with_str(&endpoint, protocol).map(TransportSocket::Ws)
        }
        (false, None) => WebSocket::new(&endpoint).map(TransportSocket::Ws),
    };
    let Ok(ws) = socket else {
        return;
    };
    if let TransportSocket::Ws(ref socket) = ws {
        socket.set_binary_type(web_sys::BinaryType::Arraybuffer);
    }
    let terminal = Arc::new(Terminal::default());
    let (incoming_tx, incoming) = mpsc::channel(QUEUE_DEPTH);
    let incoming_tx = Rc::new(RefCell::new(incoming_tx));
    let (opened_tx, opened_rx) = oneshot::channel();
    let opened_tx = Rc::new(RefCell::new(Some(opened_tx)));
    let (closed_tx, closed_rx) = oneshot::channel();
    let closed_tx = Rc::new(RefCell::new(Some(closed_tx)));
    let open = {
        let opened = opened_tx.clone();
        Closure::new(move |_: Event| {
            if let Some(tx) = opened.borrow_mut().take() {
                let _ = tx.send(true);
            }
        })
    };
    let message = {
        let tx = incoming_tx.clone();
        let ws = ws.clone();
        let state = terminal.clone();
        Closure::new(move |event: MessageEvent| {
            let data = event.data();
            // Reject text and oversize messages before copying into Rust.
            // A saturated bounded queue closes the stream instead of growing.
            if let Some(array) = data.dyn_ref::<js_sys::ArrayBuffer>() {
                if array.byte_length() <= MAX_FRAME as u32
                    && tx
                        .borrow_mut()
                        .try_send(js_sys::Uint8Array::new(array).to_vec())
                        .is_ok()
                {
                    return;
                }
            }
            state.finish(true);
            tx.borrow_mut().close_channel();
            let _ = ws.close();
        })
    };
    let error = {
        let state = terminal.clone();
        let opened = opened_tx.clone();
        let closed = closed_tx.clone();
        Closure::new(move |_: Event| {
            state.finish(true);
            if let Some(tx) = opened.borrow_mut().take() {
                let _ = tx.send(false);
            }
            if let Some(tx) = closed.borrow_mut().take() {
                let _ = tx.send(());
            }
        })
    };
    let close = {
        let state = terminal.clone();
        let opened = opened_tx.clone();
        let ws = ws.clone();
        Closure::new(move |event: CloseEvent| {
            state.clean_drained.store(
                event.was_clean()
                    && ws.buffered_amount() == 0
                    && state.pending_writes.load(Ordering::SeqCst) == 0,
                Ordering::SeqCst,
            );
            state.finish(!event.was_clean());
            if let Some(tx) = opened.borrow_mut().take() {
                let _ = tx.send(false);
            }
            if let Some(tx) = closed_tx.borrow_mut().take() {
                let _ = tx.send(());
            }
        })
    };
    ws.set_onopen(Some(open.as_ref().unchecked_ref()));
    ws.set_onmessage(Some(message.as_ref().unchecked_ref()));
    ws.set_onerror(Some(error.as_ref().unchecked_ref()));
    ws.set_onclose(Some(close.as_ref().unchecked_ref()));
    let guard = SocketGuard {
        ws,
        _open: open,
        _message: message,
        _error: error,
        _close: close,
        terminal: terminal.clone(),
    };

    let timeout = gloo_timers::future::TimeoutFuture::new(if rtc { 30_000 } else { 10_000 });
    if !matches!(
        futures::future::select(opened_rx, timeout).await,
        futures::future::Either::Left((Ok(true), _))
    ) {
        return;
    }
    if protocol.is_some_and(|protocol| guard.ws.protocol() != protocol) {
        return;
    }
    let (commands, mut receiver) = mpsc::channel(QUEUE_DEPTH);
    let stream = BrowserTcpStream {
        commands,
        incoming,
        terminal: terminal.clone(),
        buffered: Vec::new(),
        cursor: 0,
        flushing: None,
        write_closed: false,
    };
    if reply.send(Some(stream)).is_err() {
        return;
    }

    let mut closed = closed_rx;
    loop {
        let command = match futures::future::select(receiver.next(), &mut closed).await {
            futures::future::Either::Left((Some(command), _)) => command,
            _ => break,
        };
        let started = web_time::Instant::now();
        let threshold = if matches!(command, Command::Flush(_)) {
            0
        } else {
            MAX_BUFFERED - MAX_FRAME as u32
        };
        while guard.ws.buffered_amount() > threshold {
            if terminal.closed.load(Ordering::SeqCst)
                || started.elapsed().as_secs_f64() * 1000.0 > IO_TIMEOUT_MS
            {
                terminal.finish(true);
                return;
            }
            gloo_timers::future::TimeoutFuture::new(2).await;
        }
        if terminal.closed.load(Ordering::SeqCst) {
            break;
        }
        match command {
            Command::Write(bytes) => {
                if guard.ws.send_with_u8_array(&bytes).is_err() {
                    terminal.finish(true);
                    break;
                }
                terminal.pending_writes.fetch_sub(1, Ordering::SeqCst);
            }
            Command::Flush(ack) => {
                let _ = ack.send(true);
            }
        }
    }
}
