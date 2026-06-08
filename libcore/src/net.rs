use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use arti_client::config::onion_service::OnionServiceConfigBuilder;
use arti_client::config::CfgPath;
use arti_client::{StreamPrefs, TorClient, TorClientConfig};
use async_trait::async_trait;
use futures::io::{AsyncReadExt as FuturesAsyncReadExt, AsyncWriteExt as FuturesAsyncWriteExt};
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;
use tor_cell::relaycell::msg::Connected;
use tor_hsservice::{HsNickname, RendRequest, RunningOnionService};
use tor_rtcompat::{CompoundRuntime, NetStreamProvider, PreferredRuntime, RuntimeSubstExt};

use crate::crypto::{IdentityCard, PreKeyBundle, RatchetHeader, X3dhInitial};

pub type Result<T> = std::result::Result<T, NetError>;

#[derive(Debug, Error)]
pub enum NetError {
    #[error("io")] Io(#[from] std::io::Error),
    #[error("tor: {0}")] Tor(String),
    #[error("codec")] Codec,
    #[error("frame too large")] TooLarge,
    #[error("closed")] Closed,
}

impl From<bincode::Error> for NetError { fn from(_: bincode::Error) -> Self { Self::Codec } }

const HS_PORT: u16 = 443;
const MAX_FRAME: u32 = 16 * 1024 * 1024;
const HS_NICKNAME: &str = "gipny";
const INBOX_CAPACITY: usize = 64;
const RECREATE_AFTER_FAILURES: u32 = 5;
const RECREATE_COOLDOWN: Duration = Duration::from_secs(300);
const RECREATE_MIN_AGE: Duration = Duration::from_secs(60);

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

#[derive(Clone)]
struct Socks5Tcp<R> {
    inner: R,
    cfg: Arc<ProxyConfig>,
}

#[async_trait]
impl<R> NetStreamProvider<std::net::SocketAddr> for Socks5Tcp<R>
where
    R: NetStreamProvider<std::net::SocketAddr> + Clone + Send + Sync + 'static,
    R::Stream: futures::io::AsyncRead + futures::io::AsyncWrite + Unpin,
{
    type Stream = R::Stream;
    type Listener = R::Listener;

    async fn connect(&self, addr: &std::net::SocketAddr) -> std::io::Result<Self::Stream> {
        if !self.cfg.enabled() {
            return self.inner.connect(addr).await;
        }
        let proxy_ip: std::net::IpAddr = self.cfg.host.parse().map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("proxy host parse: {}", e))
        })?;
        let proxy_addr = std::net::SocketAddr::new(proxy_ip, self.cfg.port);
        let mut s = self.inner.connect(&proxy_addr).await?;
        let auth = match (self.cfg.user.as_deref(), self.cfg.pass.as_deref()) {
            (Some(u), Some(p)) if !u.is_empty() => Some((u, p)),
            _ => None,
        };
        socks5_handshake(&mut s, addr, auth).await?;
        Ok(s)
    }

    async fn listen(&self, addr: &std::net::SocketAddr) -> std::io::Result<Self::Listener> {
        self.inner.listen(addr).await
    }
}

async fn socks5_handshake<S>(
    stream: &mut S,
    target: &std::net::SocketAddr,
    auth: Option<(&str, &str)>,
) -> std::io::Result<()>
where
    S: futures::io::AsyncRead + futures::io::AsyncWrite + Unpin,
{
    if auth.is_some() {
        stream.write_all(&[0x05u8, 0x02, 0x00, 0x02]).await?;
    } else {
        stream.write_all(&[0x05u8, 0x01, 0x00]).await?;
    }
    let mut greeting = [0u8; 2];
    stream.read_exact(&mut greeting).await?;
    if greeting[0] != 0x05 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "socks5: bad version"));
    }
    match greeting[1] {
        0x00 => {}
        0x02 => {
            let (user, pass) = auth.ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::PermissionDenied, "socks5: server demanded auth, none provided")
            })?;
            if user.len() > 255 || pass.len() > 255 {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "socks5: auth field too long"));
            }
            let mut buf = Vec::with_capacity(3 + user.len() + pass.len());
            buf.push(0x01);
            buf.push(user.len() as u8);
            buf.extend_from_slice(user.as_bytes());
            buf.push(pass.len() as u8);
            buf.extend_from_slice(pass.as_bytes());
            stream.write_all(&buf).await?;
            let mut resp = [0u8; 2];
            stream.read_exact(&mut resp).await?;
            if resp[1] != 0x00 {
                return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "socks5: auth rejected"));
            }
        }
        0xff => return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "socks5: no acceptable methods")),
        m => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("socks5: unexpected method {}", m))),
    }
    let mut req = Vec::with_capacity(22);
    req.extend_from_slice(&[0x05, 0x01, 0x00]);
    match target.ip() {
        std::net::IpAddr::V4(v4) => {
            req.push(0x01);
            req.extend_from_slice(&v4.octets());
        }
        std::net::IpAddr::V6(v6) => {
            req.push(0x04);
            req.extend_from_slice(&v6.octets());
        }
    }
    req.extend_from_slice(&target.port().to_be_bytes());
    stream.write_all(&req).await?;
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await?;
    if head[0] != 0x05 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "socks5: bad reply version"));
    }
    if head[1] != 0x00 {
        return Err(std::io::Error::new(std::io::ErrorKind::Other, format!("socks5: connect rejected (code {})", head[1])));
    }
    let bnd_addr_len = match head[3] {
        0x01 => 4,
        0x04 => 16,
        0x03 => {
            let mut len_buf = [0u8; 1];
            stream.read_exact(&mut len_buf).await?;
            len_buf[0] as usize
        }
        atyp => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("socks5: unknown ATYP {}", atyp))),
    };
    let mut tail = vec![0u8; bnd_addr_len + 2];
    stream.read_exact(&mut tail).await?;
    Ok(())
}

type ProxiedRuntime = CompoundRuntime<
    PreferredRuntime,
    PreferredRuntime,
    PreferredRuntime,
    Socks5Tcp<PreferredRuntime>,
    PreferredRuntime,
    PreferredRuntime,
    PreferredRuntime,
>;

type Runtime = ProxiedRuntime;

struct TorInner {
    client: TorClient<Runtime>,
    _service: Arc<RunningOnionService>,
}

pub struct TorNode {
    inner: RwLock<TorInner>,
    onion: String,
    data_dir: PathBuf,
    proxy: RwLock<Option<ProxyConfig>>,
    inbound_tx: mpsc::Sender<Connection>,
    inbound: Arc<Mutex<mpsc::Receiver<Connection>>>,
    accept_task: Mutex<Option<JoinHandle<()>>>,
    created_at: Instant,
    relay_fail_count: AtomicU32,
    last_recreate_at: Mutex<Option<Instant>>,
    recreate_lock: Mutex<()>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ProxyConfig {
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    pub pass: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub enum ProxyKind {
    #[default] None,
    Socks5,
    Https,
}

impl ProxyConfig {
    pub fn enabled(&self) -> bool {
        self.kind != ProxyKind::None && !self.host.is_empty() && self.port != 0
    }
}

impl TorNode {
    pub async fn start(data_dir: &Path, proxy: Option<ProxyConfig>) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<Connection>(INBOX_CAPACITY);
        let (inner, onion, accept_task) = match bootstrap_tor(data_dir, proxy.clone(), &tx).await {
            Ok(x) => x,
            Err(e) if looks_like_state_corruption(&e.to_string()) => {
                eprintln!("[tor] startup failure looks like state corruption ({}), broad-wiping runtime state and retrying once", e);
                wipe_tor_runtime_state(data_dir);
                bootstrap_tor(data_dir, proxy.clone(), &tx).await?
            }
            Err(e) => return Err(e),
        };
        Ok(Self {
            inner: RwLock::new(inner),
            onion,
            data_dir: data_dir.to_path_buf(),
            proxy: RwLock::new(proxy),
            inbound_tx: tx,
            inbound: Arc::new(Mutex::new(rx)),
            accept_task: Mutex::new(Some(accept_task)),
            created_at: Instant::now(),
            relay_fail_count: AtomicU32::new(0),
            last_recreate_at: Mutex::new(None),
            recreate_lock: Mutex::new(()),
        })
    }

    pub async fn shutdown(&self) {
        if let Some(h) = self.accept_task.lock().await.take() {
            h.abort();
            let _ = h.await;
        }
    }

    pub fn onion_address(&self) -> &str { &self.onion }

    pub async fn accept(&self) -> Option<Connection> {
        self.inbound.lock().await.recv().await
    }

    pub async fn connect(&self, onion: &str) -> Result<Connection> {
        let host = if onion.ends_with(".onion") { onion.to_string() } else { format!("{}.onion", onion) };
        let target = format!("{}:{}", host, HS_PORT);
        let mut prefs = StreamPrefs::new();
        prefs.connect_to_onion_services(arti_client::config::BoolOrAuto::Explicit(true));
        let client = self.inner.read().await.client.clone();
        let stream = client.connect_with_prefs(target.as_str(), &prefs).await
            .map_err(|e| NetError::Tor(e.to_string()))?;
        Ok(Connection {
            stream: Box::pin(stream),
            peer_onion: Some(host),
        })
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
        let host = if onion.ends_with(".onion") { onion.to_string() } else { format!("{}.onion", onion) };
        let target = format!("{}:{}", host, port);
        let mut prefs = StreamPrefs::new();
        prefs.connect_to_onion_services(arti_client::config::BoolOrAuto::Explicit(true));
        let client = self.inner.read().await.client.clone();
        match client.connect_with_prefs(target.as_str(), &prefs).await {
            Ok(stream) => {
                self.relay_fail_count.store(0, Ordering::Relaxed);
                Ok(RelayStream { inner: Box::pin(stream) })
            }
            Err(e) => {
                let n = self.relay_fail_count.fetch_add(1, Ordering::Relaxed) + 1;
                self.maybe_recreate(n).await;
                Err(NetError::Tor(e.to_string()))
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
        eprintln!("[tor] {} consecutive relay failures past {}s mark, recreating TorClient", fail_count, RECREATE_MIN_AGE.as_secs());
        *self.last_recreate_at.lock().await = Some(Instant::now());
        match self.recreate().await {
            Ok(()) => {
                self.relay_fail_count.store(0, Ordering::Relaxed);
                eprintln!("[tor] recreate succeeded; relay loop will resume on next attempt");
            }
            Err(e) => eprintln!("[tor] recreate failed: {:?}", e),
        }
    }

    pub async fn recreate(&self) -> Result<()> {
        eprintln!("[tor] wiping runtime state (cache + state/*, keystore preserved)");
        wipe_tor_runtime_state(&self.data_dir);
        let proxy = self.proxy.read().await.clone();
        let (new_inner, new_onion, new_task) = bootstrap_tor(&self.data_dir, proxy, &self.inbound_tx).await?;
        if new_onion != self.onion {
            return Err(NetError::Tor(format!(
                "onion mismatch after recreate: was {}, now {} — keystore likely wiped",
                self.onion, new_onion
            )));
        }
        if let Some(old) = self.accept_task.lock().await.take() {
            old.abort();
            let _ = old.await;
        }
        *self.accept_task.lock().await = Some(new_task);
        *self.inner.write().await = new_inner;
        Ok(())
    }
}

async fn bootstrap_tor(
    data_dir: &Path,
    proxy: Option<ProxyConfig>,
    inbound_tx: &mpsc::Sender<Connection>,
) -> Result<(TorInner, String, JoinHandle<()>)> {
    let active_proxy = proxy.as_ref().filter(|p| p.enabled() && p.kind == ProxyKind::Socks5).cloned();
    if let Some(p) = active_proxy.as_ref() {
        eprintln!("[tor] outer SOCKS5 proxy: {}:{} (auth={})", p.host, p.port, p.user.is_some());
    } else if let Some(p) = proxy.as_ref().filter(|p| p.enabled()) {
        eprintln!("[tor] proxy {:?} configured but only SOCKS5 is wired right now; ignoring", p.kind);
    }
    let mut cfg = TorClientConfig::builder();
    cfg.storage().cache_dir(CfgPath::new_literal(data_dir.join("tor/cache")));
    cfg.storage().state_dir(CfgPath::new_literal(data_dir.join("tor/state")));
    cfg.storage().permissions().dangerously_trust_everyone();
    let cfg = cfg.build().map_err(|e| NetError::Tor(e.to_string()))?;
    eprintln!("[tor] bootstrapping (first run may take 60-120s)...");
    let base = PreferredRuntime::current()
        .or_else(|_| PreferredRuntime::create())
        .map_err(|e| NetError::Tor(format!("runtime: {}", e)))?;
    let proxy_tcp = Socks5Tcp {
        inner: base.clone(),
        cfg: Arc::new(active_proxy.unwrap_or_default()),
    };
    let runtime: Runtime = base.with_tcp_provider(proxy_tcp);
    let client: TorClient<Runtime> = TorClient::with_runtime(runtime)
        .config(cfg)
        .create_bootstrapped().await
        .map_err(|e| NetError::Tor(error_chain_string(&e)))?;
    eprintln!("[tor] bootstrapped");

    let nickname: HsNickname = HS_NICKNAME.parse::<HsNickname>()
        .map_err(|e| NetError::Tor(e.to_string()))?;
    let build_hs_cfg = || OnionServiceConfigBuilder::default()
        .nickname(nickname.clone())
        .build()
        .map_err(|e| NetError::Tor(e.to_string()));
    eprintln!("[tor] launching onion service...");
    let (service, rend_stream) = match client.launch_onion_service(build_hs_cfg()?) {
        Ok(x) => x,
        Err(e) if is_corrupted_hs_state(&error_chain_string(&e)) => {
            eprintln!("[tor] onion service persistent state corrupted, narrow-wiping hss/<nick> and retrying");
            wipe_hs_state(data_dir, HS_NICKNAME);
            client.launch_onion_service(build_hs_cfg()?)
                .map_err(|e| NetError::Tor(error_chain_string(&e)))?
        }
        Err(e) => return Err(NetError::Tor(error_chain_string(&e))),
    };

    eprintln!("[tor] waiting for onion address...");
    let deadline = Instant::now() + Duration::from_secs(180);
    let onion = loop {
        if let Some(n) = service.onion_name() {
            break n.to_string();
        }
        if Instant::now() > deadline {
            return Err(NetError::Tor("onion name timeout (3min)".into()));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    };
    eprintln!("[tor] onion = {}", onion);

    let rend_stream: Pin<Box<dyn Stream<Item = RendRequest> + Send>> = Box::pin(rend_stream);
    let accept_task = tokio::spawn(accept_loop(rend_stream, inbound_tx.clone(), HS_PORT));

    Ok((TorInner { client, _service: service }, onion, accept_task))
}

fn wipe_tor_runtime_state(data_dir: &Path) {
    let tor = data_dir.join("tor");
    let cache = tor.join("cache");
    match std::fs::remove_dir_all(&cache) {
        Ok(()) => eprintln!("[tor] wiped {}", cache.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => eprintln!("[tor] wipe {} failed: {}", cache.display(), e),
    }
    let state = tor.join("state");
    let Ok(entries) = std::fs::read_dir(&state) else { return };
    for entry in entries.flatten() {
        if entry.file_name() == std::ffi::OsStr::new("keystore") { continue; }
        let p = entry.path();
        let r = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        match r {
            Ok(()) => eprintln!("[tor] wiped {}", p.display()),
            Err(e) => eprintln!("[tor] wipe {} failed: {}", p.display(), e),
        }
    }
}

fn is_corrupted_hs_state(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("corrupted data in persistent state") || m.contains("unable to launch onion service")
}

fn looks_like_state_corruption(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("corrupted data in persistent state")
        || m.contains("corrupted persistent state")
        || m.contains("unable to launch onion service")
        || m.contains("unable to read")
        || (m.contains("persistent state") && m.contains("corrupt"))
}

fn error_chain_string<E: std::error::Error + ?Sized>(e: &E) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(inner) = src {
        s.push_str(": ");
        s.push_str(&inner.to_string());
        src = inner.source();
    }
    s
}

fn wipe_hs_state(data_dir: &Path, nick: &str) {
    let hss = data_dir.join("tor").join("state").join("hss").join(nick);
    for name in ["ipts.json", "iptpub.json", "iptreplay"] {
        let p = hss.join(name);
        let r = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        match r {
            Ok(()) => eprintln!("[tor] wiped {}", p.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("[tor] wipe {} failed: {}", p.display(), e),
        }
    }
}

async fn accept_loop(
    mut requests: Pin<Box<dyn Stream<Item = RendRequest> + Send>>,
    tx: mpsc::Sender<Connection>,
    _port: u16,
) {
    while let Some(rend) = requests.next().await {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut streams = match rend.accept().await { Ok(s) => s, Err(_) => return };
            while let Some(sr) = streams.next().await {
                if let Ok(ds) = sr.accept(Connected::new_empty()).await {
                    let conn = Connection {
                        stream: Box::pin(ds),
                        peer_onion: None,
                    };
                    if tx.send(conn).await.is_err() { return; }
                }
            }
        });
    }
}