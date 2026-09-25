//! Network transport over i2p, on the router inside this process.
//!
//! The public surface is deliberately unchanged from the old `TorNode`: the
//! rest of the app (session, relay client, update client, bot-sdk) treats a
//! node's address as an opaque `String` (historically an `.onion`, now an i2p
//! destination) and operates over an abstract [`DuplexStream`].
//!
//! The router is libi2pd compiled in ([`crate::embedded`], `i2p-embed`): no
//! SAM, no router process, no local port. A node is one destination of ours
//! on it, outbound only.

use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;

use sha2::{Digest, Sha256};

use crate::crypto::{IdentityCard, PreKeyBundle, RatchetHeader, X3dhInitial};

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
const RECREATE_AFTER_FAILURES: u32 = 5;
const RECREATE_COOLDOWN: Duration = Duration::from_secs(300);
const RECREATE_MIN_AGE: Duration = Duration::from_secs(60);

/// Hops in each tunnel, as i2p builds them by default and as this app has
/// always asked for them.
pub const DEFAULT_HOPS: u8 = 3;
/// The shortest tunnel we will build.
///
/// Two, not one: a single hop is both our first and our last, so it learns our
/// address and where the letter went in the same breath, and one stranger's
/// notes are enough to undo us. Two keeps somebody in the middle who knows only
/// half. The floor lives here rather than at the call site so no future caller
/// can talk the transport below it by passing a smaller number.
pub const MIN_HOPS: u8 = 2;

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

/// i2p transport node: one destination of ours on the in-process router.
///
/// Outbound only — all messaging is relay-mediated; our relay, when we host
/// one, is a destination of its own ([`crate::EphemeralRelay`]).
pub struct I2pNode {
    /// Our destination. Replaced whole by `recreate` (same keys, new tunnels).
    dest: Mutex<Arc<i2p_embed::Destination>>,
    /// Shareable public destination (opaque address; the old code's "onion").
    address: String,
    /// Private keys, reused when the destination is rebuilt. Wrapped in
    /// `Zeroizing` so our long-lived copy is scrubbed from memory on drop (the
    /// process also mlocks to keep it out of swap).
    privkey: zeroize::Zeroizing<String>,
    created_at: Instant,
    /// Hops each of our tunnels is built with.
    ///
    /// This is our leg of the path and nobody else's: a letter leaves through
    /// these hops and arrives through the inbound tunnel of the relay the
    /// recipient collects from. Shortening it trades our own anonymity for
    /// speed — the hop we pick learns both our address and where the letter
    /// went — so it is only ever changed by an explicit, informed choice.
    hops: AtomicU8,
    relay_fail_count: AtomicU32,
    last_recreate_at: Mutex<Option<Instant>>,
    recreate_lock: Mutex<()>,
}

impl I2pNode {
    /// Start the node with a fresh, **ephemeral per-session** i2p destination.
    ///
    /// The stable identity is the ed25519/x25519 keypair in the vault, and the
    /// relay routes by that key — not by i2p address — so the network address is
    /// deliberately regenerated every session for unlinkability. Nothing is
    /// persisted to disk; the key stays within a session only (for `recreate`).
    /// `settings` configures the router, when this is the first node of the
    /// process to start it — transit share and Yggdrasil.
    pub async fn start(data_dir: &Path, settings: crate::router::RouterSettings) -> Result<Self> {
        Self::start_with_progress(data_dir, settings, None).await
    }

    /// As [`Self::start`], reporting each step through `progress`. The app uses
    /// it to keep the unlock screen moving: everything here takes from a second
    /// to three minutes and used to happen in complete silence.
    pub async fn start_with_progress(
        data_dir: &Path,
        settings: crate::router::RouterSettings,
        progress: Option<crate::router::BootProgress>,
    ) -> Result<Self> {
        use crate::router::note;
        note(&progress, "router", "starting the i2p router inside the app (no local ports)");
        let router = crate::embedded::router(data_dir, settings)?;
        note(&progress, "tunnels-done", "router running");
        note(&progress, "session", "generating ephemeral destination for this session...");
        let privkey = i2p_embed::generate_keys();
        let dest = i2p_embed::Destination::new(&router, Some(&privkey), &crate::embedded::destination_options(false, DEFAULT_HOPS))
            .map_err(|e| NetError::I2p(format!("destination: {e}")))?;
        let address = dest.address().to_string();
        note(&progress, "session", format!("destination = {}", short_addr(&address)));
        dest.ready(Duration::from_secs(600)).await
            .map_err(|e| NetError::I2p(format!("tunnels: {e}")))?;
        note(&progress, "session-done", "tunnels built");
        Ok(Self {
            dest: Mutex::new(Arc::new(dest)),
            address,
            privkey: zeroize::Zeroizing::new(privkey),
            created_at: Instant::now(),
            hops: AtomicU8::new(DEFAULT_HOPS),
            relay_fail_count: AtomicU32::new(0),
            last_recreate_at: Mutex::new(None),
            recreate_lock: Mutex::new(()),
        })
    }

    /// Our destination on the router.
    pub async fn destination(&self) -> Arc<i2p_embed::Destination> {
        self.dest.lock().await.clone()
    }

    pub async fn shutdown(&self) {}

    /// Our current (ephemeral) i2p address (kept named `onion_address` for API parity).
    pub fn onion_address(&self) -> &str { &self.address }

    /// Short `.b32.i2p` address derived from the destination.
    ///
    /// The b32 address is `base32(sha256(binary_destination)).b32.i2p`.
    /// The destination string uses i2p's base64 variant (`-` and `~` instead of
    /// `+` and `/`), so we normalise before decoding.
    pub fn b32_address(&self) -> Option<String> {
        // Normalise i2p base64 → standard base64.
        let std_b64 = self.address.replace('-', "+").replace('~', "/");
        let bytes = base64_decode_padded(&std_b64)?;
        let hash = Sha256::digest(&bytes);
        Some(format!("{}.b32.i2p", base32_encode_nopad(&hash)))
    }

    pub async fn connect(&self, onion: &str) -> Result<Connection> {
        let dest = onion.trim().to_string();
        let stream = self.dial(&dest, 0).await?;
        Ok(Connection { stream: stream.inner, peer_onion: Some(dest) })
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

    /// Dial the relay. Failures here count toward our tunnels' health.
    ///
    /// Only the relay loop should use this. Repeated failures to reach the relay
    /// are evidence our tunnels have gone bad, which is what
    /// [`Self::maybe_recreate`] acts on.
    pub async fn connect_relay(&self, onion: &str, port: u16) -> Result<RelayStream> {
        match self.dial(onion, port).await {
            Ok(stream) => {
                self.relay_fail_count.store(0, Ordering::Relaxed);
                Ok(stream)
            }
            Err(e) => {
                let n = self.relay_fail_count.fetch_add(1, Ordering::Relaxed) + 1;
                self.maybe_recreate(n).await;
                Err(e)
            }
        }
    }

    /// Dial some other i2p service (the update server, for instance).
    ///
    /// Deliberately does not touch the relay failure counter. It used to: every
    /// subsystem shared `connect_relay`, so an unreachable update server counted
    /// as relay trouble, and five such failures tore down and rebuilt tunnels
    /// that were carrying live messages perfectly well. A destination
    /// being down says nothing about our own session.
    pub async fn connect_service(&self, dest: &str, port: u16) -> Result<RelayStream> {
        self.dial(dest, port).await
    }

    async fn dial(&self, onion: &str, port: u16) -> Result<RelayStream> {
        let dest = self.destination().await;
        match dest.connect(onion.trim(), port).await {
            Ok(stream) => Ok(RelayStream { inner: Box::pin(stream) }),
            Err(e) => Err(NetError::I2p(e.to_string())),
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
        eprintln!("[i2p] {} consecutive relay failures past {}s mark, rebuilding our tunnels",
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

    /// How many hops our tunnels are built with right now.
    pub fn hops(&self) -> u8 {
        self.hops.load(Ordering::Relaxed)
    }

    /// Rebuild our tunnels at a different length.
    ///
    /// This is not a switch. i2p fixes tunnel length when a destination's
    /// tunnel pool is created, so the destination is built again on the same
    /// keys — tens of seconds of waiting, during which nothing sends. The destination is kept, so nobody has to learn a
    /// new address for us.
    ///
    /// On failure the old length is restored and the error returned: a caller
    /// that asked for fewer hops and did not get them must not go on believing
    /// it is faster, and one that asked to go back to three must not be left
    /// thinking it is safe when it is not.
    pub async fn set_hops(&self, hops: u8) -> Result<()> {
        let hops = hops.clamp(MIN_HOPS, DEFAULT_HOPS);
        let previous = self.hops.swap(hops, Ordering::Relaxed);
        if previous == hops {
            return Ok(());
        }
        eprintln!("[i2p] rebuilding tunnels: {previous} hops -> {hops}");
        match self.recreate().await {
            Ok(()) => {
                eprintln!("[i2p] tunnels now {hops} hops");
                Ok(())
            }
            Err(e) => {
                self.hops.store(previous, Ordering::Relaxed);
                eprintln!("[i2p] could not rebuild at {hops} hops, staying at {previous}: {e:?}");
                Err(e)
            }
        }
    }

    /// The destination built again on the same keys: a fresh set of tunnels
    /// without changing our address. The router keeps running.
    pub async fn recreate(&self) -> Result<()> {
        let router = crate::embedded::running().ok_or(NetError::Closed)?;
        let dest = i2p_embed::Destination::new(&router, Some(self.privkey.as_str()), &crate::embedded::destination_options(false, self.hops()))
            .map_err(|e| NetError::I2p(format!("destination: {e}")))?;
        dest.ready(Duration::from_secs(600)).await.map_err(|e| NetError::I2p(format!("tunnels: {e}")))?;
        *self.dest.lock().await = Arc::new(dest);
        Ok(())
    }
}

/// Truncate a long i2p destination for logging.
fn short_addr(addr: &str) -> String {
    if addr.len() <= 20 {
        addr.to_string()
    } else {
        format!("{}…{} ({} chars)", &addr[..12], &addr[addr.len() - 6..], addr.len())
    }
}

/// Decode a standard base64 string, padding it to the next multiple of 4 if necessary.
fn base64_decode_padded(s: &str) -> Option<Vec<u8>> {
    // (4 - len%4) % 4 gives the number of '=' needed to reach the next 4-byte boundary.
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
