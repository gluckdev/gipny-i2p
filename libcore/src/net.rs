//! Network transport over i2p (SAMv3).
//!
//! This is the i2p replacement for the former Tor/Arti transport. The public
//! surface is deliberately unchanged from the old `TorNode`: the rest of the
//! app (session, relay client, update client, bot-sdk) treats a node's address
//! as an opaque `String` (historically an `.onion`, now an i2p destination) and
//! operates over an abstract [`DuplexStream`]. Only this module and the relay
//! server know we speak SAMv3.
//!
//! The actual i2p router (go-i2p) runs as a separate process/host; see
//! [`crate::router`]. Here we open one SAMv3 STREAM session bound to our
//! persistent destination and use it for outbound connections (and, optionally,
//! inbound via `STREAM FORWARD`).

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use yosemite::{style, DestinationKind, RouterApi, Session, SessionOptions, StreamOptions};

use sha2::{Digest, Sha256};

use crate::crypto::{IdentityCard, PreKeyBundle, RatchetHeader, X3dhInitial};
use crate::router::RouterHandle;

pub type Result<T> = std::result::Result<T, NetError>;

#[derive(Debug, Error)]
pub enum NetError {
    #[error("io")] Io(#[from] std::io::Error),
    #[error("i2p: {0}")] I2p(String),
    #[error("codec")] Codec,
    #[error("frame too large")] TooLarge,
    #[error("closed")] Closed,
}

impl From<bincode::Error> for NetError { fn from(_: bincode::Error) -> Self { Self::Codec } }

const MAX_FRAME: u32 = 16 * 1024 * 1024;
/// SAM session nickname prefix (a unique suffix is appended per session).
const NICKNAME: &str = "gipny";
const INBOX_CAPACITY: usize = 64;
const RECREATE_AFTER_FAILURES: u32 = 5;
const RECREATE_COOLDOWN: Duration = Duration::from_secs(300);
const RECREATE_MIN_AGE: Duration = Duration::from_secs(60);

/// Monotonic counter making each SAM session nickname unique, so a rebuilt
/// session never collides (`DUPLICATED_ID`) with one the router hasn't dropped.
static SESSION_SEQ: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Frame {
    Hello { identity: IdentityCard, onion: String },
    BundleRequest,
    Bundle(PreKeyBundle),
    X3dhInit(X3dhInitial),
    Ratchet { header: RatchetHeader, ciphertext: Vec<u8> },
    Ack { msg_id: u64 },
    Ping,
    Pong,
}

pub trait DuplexStream: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> DuplexStream for T {}

pub struct Connection {
    stream: Pin<Box<dyn DuplexStream>>,
    pub peer_onion: Option<String>,
}

pub struct RelayStream {
    inner: Pin<Box<dyn DuplexStream>>,
}

impl RelayStream {
    pub fn into_inner(self) -> Pin<Box<dyn DuplexStream>> { self.inner }
}

impl Connection {
    pub async fn send(&mut self, frame: &Frame) -> Result<()> {
        let data = bincode::serialize(frame)?;
        if data.len() > MAX_FRAME as usize { return Err(NetError::TooLarge); }
        self.stream.write_all(&(data.len() as u32).to_be_bytes()).await?;
        self.stream.write_all(&data).await?;
        self.stream.flush().await?;
        Ok(())
    }

    pub async fn recv(&mut self) -> Result<Frame> {
        let mut len_buf = [0u8; 4];
        self.stream.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf);
        if len > MAX_FRAME { return Err(NetError::TooLarge); }
        let mut buf = vec![0u8; len as usize];
        self.stream.read_exact(&mut buf).await?;
        Ok(bincode::deserialize(&buf)?)
    }

    pub async fn close(mut self) -> Result<()> {
        let _ = self.stream.shutdown().await;
        Ok(())
    }
}

/// i2p transport node.
///
/// Holds one SAMv3 STREAM session (bound to our persistent destination) plus the
/// router process handle. Outbound connections are opened via detached SAM
/// streams (so concurrent dials don't serialize); inbound — unused by the client
/// today, all messaging is relay-mediated — is available via `STREAM FORWARD`
/// when `GIPNY_I2P_ACCEPT` is set.
pub struct I2pNode {
    session: Arc<Mutex<Session<style::Stream>>>,
    /// Shareable public destination (opaque address; the old code's "onion").
    address: String,
    /// Persistent private key blob, reused across session rebuilds. Wrapped in
    /// `Zeroizing` so our long-lived copy is scrubbed from memory on drop (the
    /// process also mlocks to keep it out of swap).
    privkey: zeroize::Zeroizing<String>,
    #[allow(dead_code)]
    data_dir: PathBuf,
    sam_port: u16,
    inbound_tx: mpsc::Sender<Connection>,
    inbound: Arc<Mutex<mpsc::Receiver<Connection>>>,
    accept_task: Mutex<Option<JoinHandle<()>>>,
    created_at: Instant,
    relay_fail_count: AtomicU32,
    last_recreate_at: Mutex<Option<Instant>>,
    recreate_lock: Mutex<()>,
    /// Owns the router child process; dropping it tears the router down.
    _router: RouterHandle,
}

impl I2pNode {
    /// Start the node with a fresh, **ephemeral per-session** i2p destination.
    ///
    /// The stable identity is the ed25519/x25519 keypair in the vault, and the
    /// relay routes by that key — not by i2p address — so the network address is
    /// deliberately regenerated every session for unlinkability. Nothing is
    /// persisted to disk; the key stays within a session only (for `recreate`).
    pub async fn start(data_dir: &Path) -> Result<Self> {
        #[cfg(target_os = "android")]
        let router = RouterHandle::attach(DEFAULT_SAM_PORT).await?;
        #[cfg(not(target_os = "android"))]
        let router = RouterHandle::start(data_dir, None).await?;

        let sam_port = router.sam_port();
        eprintln!("[i2p] generating ephemeral destination for this session...");
        let (address, privkey) = RouterApi::new(sam_port)
            .generate_destination()
            .await
            .map_err(|e| NetError::I2p(format!("generate destination: {e}")))?;
        eprintln!("[i2p] destination = {}", short_addr(&address));

        let session = build_session(sam_port, &privkey).await?;
        let session = Arc::new(Mutex::new(session));

        let (tx, rx) = mpsc::channel::<Connection>(INBOX_CAPACITY);
        let accept_task = spawn_inbound(session.clone(), tx.clone()).await;

        Ok(Self {
            session,
            address,
            privkey: zeroize::Zeroizing::new(privkey),
            data_dir: data_dir.to_path_buf(),
            sam_port,
            inbound_tx: tx,
            inbound: Arc::new(Mutex::new(rx)),
            accept_task: Mutex::new(accept_task),
            created_at: Instant::now(),
            relay_fail_count: AtomicU32::new(0),
            last_recreate_at: Mutex::new(None),
            recreate_lock: Mutex::new(()),
            _router: router,
        })
    }

    pub async fn shutdown(&self) {
        if let Some(h) = self.accept_task.lock().await.take() {
            h.abort();
            let _ = h.await;
        }
        // The router child is torn down when `self._router` (this node) drops.
    }

    /// Our current (ephemeral) i2p address (kept named `onion_address` for API parity).
    pub fn onion_address(&self) -> &str { &self.address }

    /// Short `.b32.i2p` address derived from the destination.
    ///
    /// The b32 address is `base32(sha256(binary_destination)).b32.i2p`.
    /// The destination string uses i2p's base64 variant (`-` and `~` instead of
    /// `+` and `/`), so we normalise before decoding.
    pub fn b32_address(&self) -> Option<String> {
        // Normalise i2p base64 → standard base64.
        let std_b64: String = self.address
            .chars()
            .map(|c| match c { '-' => '+', '~' => '/', c => c })
            .collect();
        // Strip the i2p base64 certificate prefix if present (destination format
        // is variable-length; we hash the full binary blob regardless).
        let bytes = base64_decode_padded(&std_b64)?;
        let hash = Sha256::digest(&bytes);
        Some(format!("{}.b32.i2p", base32_encode_nopad(&hash)))
    }

    pub async fn accept(&self) -> Option<Connection> {
        self.inbound.lock().await.recv().await
    }

    pub async fn connect(&self, onion: &str) -> Result<Connection> {
        let dest = onion.trim().to_string();
        // `connect_detached` clones the SAM controller and returns an owned
        // future, so we only hold the session lock for the clone — concurrent
        // dials proceed in parallel.
        let fut = {
            let mut s = self.session.lock().await;
            s.connect_detached(&dest)
        };
        let stream = fut.await.map_err(|e| NetError::I2p(e.to_string()))?;
        Ok(Connection { stream: Box::pin(stream), peer_onion: Some(dest) })
    }

    pub async fn connect_retry(&self, onion: &str, attempts: u32) -> Result<Connection> {
        let mut delay_ms = 300u64;
        let mut last = NetError::Closed;
        for i in 0..attempts {
            match self.connect(onion).await {
                Ok(c) => return Ok(c),
                Err(e) => {
                    last = e;
                    if i + 1 < attempts {
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        delay_ms = (delay_ms * 2).min(3_000);
                    }
                }
            }
        }
        Err(last)
    }

    pub async fn connect_relay(&self, onion: &str, port: u16) -> Result<RelayStream> {
        let dest = onion.trim().to_string();
        // `port` maps to the SAM stream destination port. For a single-service
        // i2p destination the far end ignores it, so this is a harmless carry-over
        // of the old per-onion-port dialing.
        let opts = StreamOptions { dst_port: port, src_port: 0 };
        let fut = {
            let mut s = self.session.lock().await;
            s.connect_detached_with_options(&dest, opts)
        };
        match fut.await {
            Ok(stream) => {
                self.relay_fail_count.store(0, Ordering::Relaxed);
                Ok(RelayStream { inner: Box::pin(stream) })
            }
            Err(e) => {
                let n = self.relay_fail_count.fetch_add(1, Ordering::Relaxed) + 1;
                self.maybe_recreate(n).await;
                Err(NetError::I2p(e.to_string()))
            }
        }
    }

    async fn maybe_recreate(&self, fail_count: u32) {
        if fail_count < RECREATE_AFTER_FAILURES { return; }
        if self.created_at.elapsed() < RECREATE_MIN_AGE { return; }
        let Ok(_g) = self.recreate_lock.try_lock() else { return; };
        {
            let last = self.last_recreate_at.lock().await;
            if let Some(t) = *last {
                if t.elapsed() < RECREATE_COOLDOWN { return; }
            }
        }
        eprintln!("[i2p] {} consecutive relay failures past {}s mark, rebuilding SAM session",
            fail_count, RECREATE_MIN_AGE.as_secs());
        *self.last_recreate_at.lock().await = Some(Instant::now());
        match self.recreate().await {
            Ok(()) => {
                self.relay_fail_count.store(0, Ordering::Relaxed);
                eprintln!("[i2p] session rebuild succeeded; relay loop will resume on next attempt");
            }
            Err(e) => eprintln!("[i2p] session rebuild failed: {:?}", e),
        }
    }

    /// Rebuild the SAM session against the same persistent destination (the
    /// router keeps running). This is the i2p analogue of the old Tor client
    /// recreate — a fresh set of tunnels without changing our address.
    pub async fn recreate(&self) -> Result<()> {
        let new_session = build_session(self.sam_port, self.privkey.as_str()).await?;

        if let Some(old) = self.accept_task.lock().await.take() {
            old.abort();
            let _ = old.await;
        }
        *self.session.lock().await = new_session;
        let task = spawn_inbound(self.session.clone(), self.inbound_tx.clone()).await;
        *self.accept_task.lock().await = task;
        Ok(())
    }
}

/// Build a SAMv3 STREAM session bound to our persistent destination.
async fn build_session(sam_port: u16, privkey: &str) -> Result<Session<style::Stream>> {
    let seq = SESSION_SEQ.fetch_add(1, Ordering::Relaxed);
    let opts = SessionOptions {
        nickname: format!("{NICKNAME}-{}-{}", std::process::id(), seq),
        destination: DestinationKind::Persistent { private_key: privkey.to_string() },
        samv3_tcp_port: sam_port,
        // The client is outbound-only (all messaging is relay-mediated): no need
        // to advertise a leaseSet or maintain inbound tunnels, which speeds up cold
        // start. Inbound forwarding via STREAM FORWARD (GIPNY_I2P_ACCEPT) still
        // works regardless of this flag.
        publish: false,
        // Forwarded inbound streams carry pure data, no in-band peer destination.
        silent_forward: true,
        // Payloads are already E2E-encrypted and padded to fixed buckets; SAM-level
        // gzip only burns CPU and would blur the uniform padding size classes.
        gzip: false,
        ..Default::default()
    };
    Session::<style::Stream>::new(opts)
        .await
        .map_err(|e| NetError::I2p(format!("SAM session: {e}")))
}

/// Optionally register inbound forwarding to a local TCP listener.
///
/// The client never calls [`I2pNode::accept`] (all messaging is relay-mediated),
/// so inbound is off by default and only enabled with `GIPNY_I2P_ACCEPT=1` — this
/// keeps the default path pure-outbound and maximally compatible with the
/// early-stage router. Returns `None` (no accept task) when disabled or on error.
async fn spawn_inbound(
    session: Arc<Mutex<Session<style::Stream>>>,
    tx: mpsc::Sender<Connection>,
) -> Option<JoinHandle<()>> {
    let enabled = std::env::var("GIPNY_I2P_ACCEPT")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if !enabled {
        return None;
    }
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", 0u16)).await {
        Ok(l) => l,
        Err(e) => { eprintln!("[i2p] inbound listener bind failed: {e}"); return None; }
    };
    let port = match listener.local_addr() {
        Ok(a) => a.port(),
        Err(e) => { eprintln!("[i2p] inbound listener addr failed: {e}"); return None; }
    };
    {
        let mut s = session.lock().await;
        if let Err(e) = s.forward(port).await {
            eprintln!("[i2p] STREAM FORWARD failed (inbound disabled): {e}");
            return None;
        }
    }
    eprintln!("[i2p] inbound forwarding active on 127.0.0.1:{port}");
    let handle = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((tcp, _)) => {
                    let conn = Connection { stream: Box::pin(tcp), peer_onion: None };
                    if tx.send(conn).await.is_err() { return; }
                }
                Err(e) => {
                    eprintln!("[i2p] inbound accept err: {e}");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
    });
    Some(handle)
}

/// Truncate a long i2p destination for logging.
fn short_addr(addr: &str) -> String {
    if addr.len() <= 20 {
        addr.to_string()
    } else {
        format!("{}…{} ({} chars)", &addr[..12], &addr[addr.len() - 6..], addr.len())
    }
}

/// Decode a standard base64 string, padding it to a multiple of 4 if necessary.
fn base64_decode_padded(s: &str) -> Option<Vec<u8>> {
    // Pad to nearest multiple of 4.
    let pad = (4 - s.len() % 4) % 4;
    let padded: String = format!("{}{}", s, "=".repeat(pad));
    // Manual base64 decode using only std (no extra dep needed for this small helper).
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut table = [0xffu8; 256];
    for (i, &b) in alphabet.iter().enumerate() { table[b as usize] = i as u8; }
    let mut out = Vec::with_capacity(padded.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits = 0u32;
    for ch in padded.bytes() {
        if ch == b'=' { break; }
        let v = table[ch as usize];
        if v == 0xff { return None; }
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

/// Base32 encode (RFC 4648 alphabet, lowercase, no padding).
fn base32_encode_nopad(input: &[u8]) -> String {
    const ALPHA: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut out = String::new();
    let mut buf: u64 = 0;
    let mut bits: u32 = 0;
    for &byte in input {
        buf = (buf << 8) | byte as u64;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHA[((buf >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHA[((buf << (5 - bits)) & 0x1f) as usize] as char);
    }
    out
}

/// Backwards-compatible alias: the transport is now i2p, but the rest of the
/// codebase still refers to the node type by its historical name.
pub type TorNode = I2pNode;
