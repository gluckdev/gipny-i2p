use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use arti_client::config::onion_service::OnionServiceConfigBuilder;
use arti_client::config::CfgPath;
use arti_client::{StreamPrefs, TorClient, TorClientConfig};
use async_trait::async_trait;
use futures::io::{AsyncReadExt as FuturesAsyncReadExt, AsyncWriteExt as FuturesAsyncWriteExt};
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex};
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

pub struct TorNode {
    client: TorClient<Runtime>,
    _service: Arc<RunningOnionService>,
    onion: String,
    inbound: Arc<Mutex<mpsc::Receiver<Connection>>>,
    accept_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
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
            .map_err(|e| NetError::Tor(e.to_string()))?;
        eprintln!("[tor] bootstrapped");

        let nickname: HsNickname = HS_NICKNAME.parse::<HsNickname>()
            .map_err(|e| NetError::Tor(e.to_string()))?;
        let hs_cfg = OnionServiceConfigBuilder::default()
            .nickname(nickname)
            .build()
            .map_err(|e| NetError::Tor(e.to_string()))?;
        eprintln!("[tor] launching onion service...");
        let (service, rend_stream) = client
            .launch_onion_service(hs_cfg)
            .map_err(|e| NetError::Tor(e.to_string()))?;

        eprintln!("[tor] waiting for onion address...");
        let deadline = std::time::Instant::now() + Duration::from_secs(180);
        let onion = loop {
            if let Some(n) = service.onion_name() {
                break n.to_string();
            }
            if std::time::Instant::now() > deadline {
                return Err(NetError::Tor("onion name timeout (3min)".into()));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        };
        eprintln!("[tor] onion = {}", onion);

        let (tx, rx) = mpsc::channel::<Connection>(INBOX_CAPACITY);
        let rend_stream: Pin<Box<dyn Stream<Item = RendRequest> + Send>> = Box::pin(rend_stream);
        let accept_task = tokio::spawn(accept_loop(rend_stream, tx, HS_PORT));

        Ok(Self {
            client,
            _service: service,
            onion,
            inbound: Arc::new(Mutex::new(rx)),
            accept_task: Mutex::new(Some(accept_task)),
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
        let stream = self.client.connect_with_prefs(target.as_str(), &prefs).await
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
        let stream = self.client.connect_with_prefs(target.as_str(), &prefs).await
            .map_err(|e| NetError::Tor(e.to_string()))?;
        Ok(RelayStream { inner: Box::pin(stream) })
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