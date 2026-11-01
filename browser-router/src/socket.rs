use std::{
    cell::RefCell,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
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
    Sink, Stream, StreamExt,
};
use wasm_bindgen::{closure::Closure, JsCast};
use web_sys::{CloseEvent, Event, MessageEvent, WebSocket};

pub const MAX_FRAME: usize = 64 * 1024;
const QUEUE_DEPTH: usize = 8;
const MAX_BUFFERED: u32 = 2 * MAX_FRAME as u32;
const IO_TIMEOUT_MS: f64 = 30_000.0;

thread_local! {
    static FORWARDER: RefCell<Option<url::Url>> = const { RefCell::new(None) };
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
    FORWARDER.with(|value| *value.borrow_mut() = Some(url));
    Ok(())
}

#[derive(Default)]
struct Terminal {
    closed: AtomicBool,
    failed: AtomicBool,
    waker: AtomicWaker,
}

impl Terminal {
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

impl TcpStream for BrowserTcpStream {
    fn connect(address: SocketAddr) -> impl Future<Output = Option<Self>> + Send {
        // Start JS work synchronously; the returned future captures only a
        // oneshot receiver, not a WebSocket, Closure, Rc or JS timer.
        let endpoint = FORWARDER.with(|v| v.borrow().clone());
        let (reply, result) = oneshot::channel();
        if let Some(mut endpoint) = endpoint {
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
            wasm_bindgen_futures::spawn_local(run_socket(endpoint.into(), reply));
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
                Poll::Ready(
                    Pin::new(&mut this.commands)
                        .start_send(Command::Write(buf[..n].to_vec()))
                        .map(|_| n)
                        .map_err(|_| Error::DialFailure),
                )
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<emissary_core::Result<()>> {
        let this = self.get_mut();
        if this.write_closed {
            return Poll::Ready(Ok(()));
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
        Poll::Ready(if matches!(result, Ok(true)) {
            Ok(())
        } else {
            Err(Error::DialFailure)
        })
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

/// Drop always detaches handlers, breaking JS/Rust reference cycles, and closes
/// the socket. This also handles cancellation during open or failed handoff.
struct SocketGuard {
    ws: WebSocket,
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

async fn run_socket(endpoint: String, reply: oneshot::Sender<Option<BrowserTcpStream>>) {
    let Ok(ws) = WebSocket::new(&endpoint) else {
        return;
    };
    ws.set_binary_type(web_sys::BinaryType::Arraybuffer);
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
            // Reject text, oversize messages and a saturated queue before
            // allocating/copying the payload into Rust memory.
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
        Closure::new(move |event: CloseEvent| {
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

    let timeout = gloo_timers::future::TimeoutFuture::new(10_000);
    if !matches!(
        futures::future::select(opened_rx, timeout).await,
        futures::future::Either::Left((Ok(true), _))
    ) {
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
            }
            Command::Flush(ack) => {
                let _ = ack.send(true);
            }
        }
    }
}
