//! The i2pd router inside our process, reached through its C++ API rather
//! than SAM: no TCP, no local port a stranger on the machine could use.
//!
//! One [`Router`] per process (libi2pd keeps global state). Destinations are
//! made from base64 private keys or fresh ones; streams are
//! `tokio::io::{AsyncRead, AsyncWrite}`. Everything below `ffi` runs the
//! router's callbacks on its own threads and only wakes a task from them.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, oneshot};

mod ffi {
    use super::*;

    #[repr(C)]
    pub struct GipnyDest {
        _p: [u8; 0],
    }
    #[repr(C)]
    pub struct GipnyStream {
        _p: [u8; 0],
    }

    pub type StreamCb = extern "C" fn(ctx: *mut c_void, stream: *mut GipnyStream);
    pub type IoCb = extern "C" fn(ctx: *mut c_void, error: c_int, bytes: usize);

    extern "C" {
        pub fn gipny_router_init(argc: c_int, argv: *const *const c_char) -> c_int;
        pub fn gipny_router_start(log_path: *const c_char);
        pub fn gipny_router_stop();
        pub fn gipny_router_set_online(online: c_int);
        pub fn gipny_keys_generate() -> *mut c_char;
        pub fn gipny_keys_public(private_b64: *const c_char) -> *mut c_char;
        pub fn gipny_dest_create(
            private_b64: *const c_char,
            publish: c_int,
            option_keys: *const *const c_char,
            option_values: *const *const c_char,
            options: usize,
        ) -> *mut GipnyDest;
        pub fn gipny_dest_destroy(dest: *mut GipnyDest);
        pub fn gipny_dest_is_ready(dest: *const GipnyDest) -> c_int;
        pub fn gipny_dest_address(dest: *const GipnyDest) -> *mut c_char;
        pub fn gipny_dest_connect(dest: *mut GipnyDest, remote: *const c_char, port: u16, cb: StreamCb, ctx: *mut c_void) -> c_int;
        pub fn gipny_dest_accept(dest: *mut GipnyDest, cb: StreamCb, ctx: *mut c_void);
        pub fn gipny_dest_stop_accepting(dest: *mut GipnyDest);
        pub fn gipny_stream_recv(s: *mut GipnyStream, buf: *mut u8, len: usize, timeout_secs: c_int, cb: IoCb, ctx: *mut c_void);
        pub fn gipny_stream_send(s: *mut GipnyStream, buf: *const u8, len: usize, cb: IoCb, ctx: *mut c_void);
        pub fn gipny_stream_close(s: *mut GipnyStream);
        pub fn gipny_stream_free(s: *mut GipnyStream);
        pub fn gipny_string_free(s: *mut c_char);
    }
}

#[derive(Debug)]
pub enum Error {
    AlreadyStarted,
    InitFailed,
    BadKeys,
    BadAddress,
    Unreachable,
    Timeout,
    /// The certificates could not be laid out in the data dir.
    Certificates(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Error::Certificates(e) = self {
            return write!(f, "reseed certificates: {e}");
        }
        f.write_str(match self {
            Error::Certificates(_) => unreachable!(),
            Error::AlreadyStarted => "the router is already running in this process",
            Error::InitFailed => "the router did not initialise",
            Error::BadKeys => "not i2p private keys",
            Error::BadAddress => "not an i2p destination",
            Error::Unreachable => "the destination could not be reached",
            Error::Timeout => "timed out",
        })
    }
}

impl std::error::Error for Error {}

fn take_string(p: *mut c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the shim returns malloc'ed, NUL-terminated strings.
    let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    unsafe { ffi::gipny_string_free(p) };
    Some(s)
}

static STARTED: OnceLock<()> = OnceLock::new();

/// The router. Only one per process; stopping it ends every destination.
pub struct Router {
    _private: (),
}

impl Router {
    /// Start the router with i2pd's own options, e.g. `--datadir=…`,
    /// `--bandwidth=L`. `log_path` `None` logs into the data dir.
    pub fn start(options: &[String], log_path: Option<&str>) -> Result<Self, Error> {
        if STARTED.get().is_some() {
            return Err(Error::AlreadyStarted);
        }
        // The router verifies reseed bundles against these; with none it can
        // never join from an empty netDb. Unless told to look elsewhere, it
        // looks in <datadir>/certificates (see the shim).
        let datadir = options.iter().find_map(|o| o.strip_prefix("--datadir="));
        if let Some(dir) = datadir {
            if !options.iter().any(|o| o.starts_with("--certsdir=")) {
                write_certificates(&std::path::Path::new(dir).join("certificates")).map_err(Error::Certificates)?;
            }
        }
        if STARTED.set(()).is_err() {
            return Err(Error::AlreadyStarted);
        }
        let mut args = vec![CString::new("gipny").unwrap()];
        for o in options {
            args.push(CString::new(o.as_str()).map_err(|_| Error::InitFailed)?);
        }
        let ptrs: Vec<*const c_char> = args.iter().map(|a| a.as_ptr()).collect();
        // SAFETY: argv outlives the call; the shim copies what it keeps.
        if unsafe { ffi::gipny_router_init(ptrs.len() as c_int, ptrs.as_ptr()) } == 0 {
            return Err(Error::InitFailed);
        }
        let log = log_path.map(|p| CString::new(p).unwrap_or_default());
        unsafe { ffi::gipny_router_start(log.as_ref().map_or(std::ptr::null(), |l| l.as_ptr())) };
        Ok(Self { _private: () })
    }

    /// The machine's network changed (Wi-Fi to LTE, out of sleep): make the
    /// router test how the new one sees it.
    pub fn network_changed(&self) {
        unsafe {
            ffi::gipny_router_set_online(0);
            ffi::gipny_router_set_online(1);
        }
    }

    pub fn set_online(&self, online: bool) {
        unsafe { ffi::gipny_router_set_online(online as c_int) };
    }
}

mod certs {
    include!(concat!(env!("OUT_DIR"), "/certificates.rs"));
}

/// Lay out the certificates compiled in (i2pd's reseed and family ones)
/// under `dir`, rewriting any that differ: they follow the binary.
pub fn write_certificates(dir: &std::path::Path) -> std::io::Result<usize> {
    let mut written = 0;
    for (name, bytes) in certs::CERTIFICATES {
        let path = dir.join(name);
        if std::fs::read(&path).is_ok_and(|have| have == *bytes) {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes)?;
        written += 1;
    }
    Ok(written)
}

impl Drop for Router {
    fn drop(&mut self) {
        unsafe { ffi::gipny_router_stop() };
    }
}

/// Fresh base64 private keys (Ed25519 signing, X25519 encryption).
pub fn generate_keys() -> String {
    take_string(unsafe { ffi::gipny_keys_generate() }).expect("key generation")
}

/// The public destination of base64 private keys.
pub fn public_of(private_b64: &str) -> Result<String, Error> {
    let c = CString::new(private_b64).map_err(|_| Error::BadKeys)?;
    take_string(unsafe { ffi::gipny_keys_public(c.as_ptr()) }).ok_or(Error::BadKeys)
}

/// Tunnel parameters for a destination.
#[derive(Clone, Debug)]
pub struct DestinationOptions {
    /// Publish the LeaseSet, so others can reach this destination.
    pub publish: bool,
    pub inbound_length: u8,
    pub outbound_length: u8,
    /// Other i2cp options, as i2pd spells them.
    pub extra: Vec<(String, String)>,
}

impl Default for DestinationOptions {
    fn default() -> Self {
        Self { publish: false, inbound_length: 3, outbound_length: 3, extra: Vec::new() }
    }
}

type Acceptor = mpsc::UnboundedSender<I2pStream>;

/// A local destination: an address of ours in the network.
pub struct Destination {
    raw: *mut ffi::GipnyDest,
    address: String,
    acceptor: Mutex<Option<*mut Acceptor>>,
}

// SAFETY: the shim's destination is a shared_ptr used from any thread by
// i2pd itself; our own state is behind a Mutex.
unsafe impl Send for Destination {}
unsafe impl Sync for Destination {}

impl Destination {
    /// `keys` `None` makes a transient destination.
    pub fn new(_router: &Router, keys: Option<&str>, opts: &DestinationOptions) -> Result<Self, Error> {
        let keys_c = keys.map(CString::new).transpose().map_err(|_| Error::BadKeys)?;
        let mut pairs: Vec<(CString, CString)> = vec![
            (CString::new("inbound.length").unwrap(), CString::new(opts.inbound_length.to_string()).unwrap()),
            (CString::new("outbound.length").unwrap(), CString::new(opts.outbound_length.to_string()).unwrap()),
        ];
        for (k, v) in &opts.extra {
            pairs.push((CString::new(k.as_str()).map_err(|_| Error::BadKeys)?, CString::new(v.as_str()).map_err(|_| Error::BadKeys)?));
        }
        let ks: Vec<*const c_char> = pairs.iter().map(|(k, _)| k.as_ptr()).collect();
        let vs: Vec<*const c_char> = pairs.iter().map(|(_, v)| v.as_ptr()).collect();
        let raw = unsafe {
            ffi::gipny_dest_create(
                keys_c.as_ref().map_or(std::ptr::null(), |k| k.as_ptr()),
                opts.publish as c_int,
                ks.as_ptr(),
                vs.as_ptr(),
                pairs.len(),
            )
        };
        if raw.is_null() {
            return Err(Error::BadKeys);
        }
        let address = take_string(unsafe { ffi::gipny_dest_address(raw) }).unwrap_or_default();
        Ok(Self { raw, address, acceptor: Mutex::new(None) })
    }

    /// Base64 public destination.
    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn is_ready(&self) -> bool {
        unsafe { ffi::gipny_dest_is_ready(self.raw) != 0 }
    }

    /// Until the LeaseSet is up and there are outbound tunnels. i2pd has no
    /// callback for it; a short poll, not SAM's 3 s one.
    pub async fn ready(&self, timeout: Duration) -> Result<(), Error> {
        let deadline = tokio::time::Instant::now() + timeout;
        while !self.is_ready() {
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Ok(())
    }

    /// Open a stream to `remote` (base64 destination or "….b32.i2p").
    pub fn connect(&self, remote: &str, port: u16) -> impl Future<Output = Result<I2pStream, Error>> + Send + 'static {
        let (tx, rx) = oneshot::channel::<Option<I2pStream>>();
        let started = CString::new(remote).ok().map(|r| {
            let ctx = Box::into_raw(Box::new(tx)) as *mut c_void;
            let ok = unsafe { ffi::gipny_dest_connect(self.raw, r.as_ptr(), port, on_connected, ctx) };
            if ok == 0 {
                // Not started: the callback will never run, so take ctx back.
                drop(unsafe { Box::from_raw(ctx as *mut oneshot::Sender<Option<I2pStream>>) });
            }
            ok != 0
        });
        async move {
            if started != Some(true) {
                return Err(Error::BadAddress);
            }
            match rx.await {
                Ok(Some(s)) => Ok(s),
                _ => Err(Error::Unreachable),
            }
        }
    }

    /// Start accepting inbound streams; they arrive on the returned channel.
    /// Calling it again replaces the previous channel.
    pub fn accept(&self) -> mpsc::UnboundedReceiver<I2pStream> {
        let (tx, rx) = mpsc::unbounded_channel();
        let ctx = Box::into_raw(Box::new(tx));
        let mut slot = self.acceptor.lock().unwrap_or_else(|p| p.into_inner());
        unsafe { ffi::gipny_dest_accept(self.raw, on_accepted, ctx as *mut c_void) };
        // The previous acceptor is no longer called once the new one is set;
        // it is freed with the destination (a callback may still be running).
        // Deliberately leaked: a few bytes, and freeing it would race a
        // callback already running with it.
        let _leaked = slot.replace(ctx);
        rx
    }
}

impl Drop for Destination {
    fn drop(&mut self) {
        unsafe {
            ffi::gipny_dest_stop_accepting(self.raw);
            // Stops the destination's thread: no callback runs after this.
            ffi::gipny_dest_destroy(self.raw);
        }
        if let Some(ctx) = self.acceptor.lock().unwrap_or_else(|p| p.into_inner()).take() {
            drop(unsafe { Box::from_raw(ctx) });
        }
    }
}

extern "C" fn on_connected(ctx: *mut c_void, stream: *mut ffi::GipnyStream) {
    // SAFETY: ctx is the Box made in `connect`, handed back exactly once.
    let tx = unsafe { Box::from_raw(ctx as *mut oneshot::Sender<Option<I2pStream>>) };
    let s = (!stream.is_null()).then(|| I2pStream::from_raw(stream));
    let _ = tx.send(s);
}

extern "C" fn on_accepted(ctx: *mut c_void, stream: *mut ffi::GipnyStream) {
    // SAFETY: ctx is the acceptor Box, alive until the destination is dropped.
    let tx = unsafe { &*(ctx as *const Acceptor) };
    if tx.send(I2pStream::from_raw(stream)).is_err() {
        // Nobody listening: close it (dropping the stream does).
    }
}

const READ_BUF: usize = 64 * 1024;
/// A read waits this long for data; on timeout it is simply issued again.
const READ_WAIT_SECS: c_int = 3600;

#[derive(Default)]
struct ReadState {
    buf: Vec<u8>,
    /// Bytes of `buf` filled and not yet handed out, from `start`.
    filled: usize,
    start: usize,
    in_flight: bool,
    /// End of stream or an error, after which reads return it.
    done: Option<io::ErrorKind>,
    waker: Option<Waker>,
}

#[derive(Default)]
struct WriteState {
    in_flight: usize,
    error: Option<io::ErrorKind>,
    waker: Option<Waker>,
}

struct Shared {
    raw: *mut ffi::GipnyStream,
    read: Mutex<ReadState>,
    write: Mutex<WriteState>,
}

unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

impl Drop for Shared {
    fn drop(&mut self) {
        // Last reference: no read or send callback can still come for it.
        unsafe {
            ffi::gipny_stream_close(self.raw);
            ffi::gipny_stream_free(self.raw);
        }
    }
}

/// A stream to or from another destination.
pub struct I2pStream {
    shared: Arc<Shared>,
}

impl I2pStream {
    fn from_raw(raw: *mut ffi::GipnyStream) -> Self {
        let read = ReadState { buf: vec![0; READ_BUF], ..Default::default() };
        Self { shared: Arc::new(Shared { raw, read: Mutex::new(read), write: Mutex::new(WriteState::default()) }) }
    }

    fn issue_read(&self, st: &mut ReadState) {
        st.in_flight = true;
        st.start = 0;
        st.filled = 0;
        let ctx = Arc::into_raw(self.shared.clone()) as *mut c_void;
        let buf = st.buf.as_mut_ptr();
        let len = st.buf.len();
        // SAFETY: `buf` belongs to the Shared that ctx keeps alive until the
        // callback, and is not touched while `in_flight`.
        unsafe { ffi::gipny_stream_recv(self.shared.raw, buf, len, READ_WAIT_SECS, on_read, ctx) };
    }
}

fn error_kind(code: c_int) -> io::ErrorKind {
    match code {
        1 => io::ErrorKind::UnexpectedEof,
        2 => io::ErrorKind::ConnectionReset,
        3 => io::ErrorKind::TimedOut,
        _ => io::ErrorKind::Other,
    }
}

extern "C" fn on_read(ctx: *mut c_void, error: c_int, bytes: usize) {
    // SAFETY: ctx is the Arc made in `issue_read`, handed back exactly once.
    let shared = unsafe { Arc::from_raw(ctx as *const Shared) };
    let waker = {
        let mut st = shared.read.lock().unwrap_or_else(|p| p.into_inner());
        st.in_flight = false;
        match error {
            0 => st.filled = bytes,
            3 => {} // no data within the wait; the next poll asks again
            1 => st.done = Some(io::ErrorKind::UnexpectedEof),
            e => st.done = Some(error_kind(e)),
        }
        st.waker.take()
    };
    if let Some(w) = waker {
        w.wake();
    }
}

extern "C" fn on_sent(ctx: *mut c_void, error: c_int, _bytes: usize) {
    // SAFETY: ctx is the Arc made in `poll_write`, handed back exactly once.
    let shared = unsafe { Arc::from_raw(ctx as *const Shared) };
    let waker = {
        let mut st = shared.write.lock().unwrap_or_else(|p| p.into_inner());
        st.in_flight = st.in_flight.saturating_sub(1);
        if error != 0 {
            st.error = Some(error_kind(error));
        }
        st.waker.take()
    };
    if let Some(w) = waker {
        w.wake();
    }
}

impl AsyncRead for I2pStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, out: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let mut st = self.shared.read.lock().unwrap_or_else(|p| p.into_inner());
        if st.filled > 0 {
            let n = st.filled.min(out.remaining());
            let start = st.start;
            out.put_slice(&st.buf[start..start + n]);
            st.start += n;
            st.filled -= n;
            return Poll::Ready(Ok(()));
        }
        if let Some(kind) = st.done {
            // End of stream reads as 0 bytes, like a socket.
            return if kind == io::ErrorKind::UnexpectedEof { Poll::Ready(Ok(())) } else { Poll::Ready(Err(kind.into())) };
        }
        st.waker = Some(cx.waker().clone());
        if !st.in_flight {
            self.issue_read(&mut st);
        }
        Poll::Pending
    }
}

/// Unflushed sends allowed in flight before a write waits.
const MAX_SENDS_IN_FLIGHT: usize = 16;

impl AsyncWrite for I2pStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let mut st = self.shared.write.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(kind) = st.error {
            return Poll::Ready(Err(kind.into()));
        }
        if st.in_flight >= MAX_SENDS_IN_FLIGHT {
            st.waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        st.in_flight += 1;
        let ctx = Arc::into_raw(self.shared.clone()) as *mut c_void;
        // SAFETY: the shim copies `buf` before returning.
        unsafe { ffi::gipny_stream_send(self.shared.raw, buf.as_ptr(), buf.len(), on_sent, ctx) };
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut st = self.shared.write.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(kind) = st.error {
            return Poll::Ready(Err(kind.into()));
        }
        if st.in_flight == 0 {
            return Poll::Ready(Ok(()));
        }
        st.waker = Some(cx.waker().clone());
        Poll::Pending
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.as_mut().poll_flush(cx) {
            Poll::Ready(_) => {
                unsafe { ffi::gipny_stream_close(self.shared.raw) };
                Poll::Ready(Ok(()))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
