//! Bounded WSS / WebRTC to NTCP2 TCP forwarder. No I2P identity or E2E keys.
use anyhow::{Context, Result, anyhow, bail, ensure};
use axum::{
    Router,
    extract::{
        RawQuery, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::Engine;
use bytes::{Bytes, BytesMut};
use futures_util::SinkExt;
use gipny_browser_router::{
    bootstrap::is_public_ipv4,
    ntcp2::{MAX_ROUTER_INFO_BYTES, MAX_TARGET_ROWS, is_allowed_ntcp2_port, verified_peer},
};
use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
    path::Path,
    sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::{Semaphore, mpsc},
    time::{MissedTickBehavior, interval_at, timeout},
};
use webrtc::{
    data_channel::{DataChannel, DataChannelEvent},
    peer_connection::{
        PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCIceGatheringState,
        RTCPeerConnectionState, RTCSessionDescription, SettingEngineBuilder,
    },
};

pub const FRAME: usize = 65536;
const RTC_FRAME: usize = 16384;
const OPEN: Duration = Duration::from_secs(25);
const CONNECT: Duration = Duration::from_secs(5);
const SEND: Duration = Duration::from_secs(15);
pub const PROTOCOL: &str = "gipny.ntcp2.v1.";
const LABEL: &str = "gipny-ntcp2-v1";
/// Server WebSocket Ping interval. Below nginx's default 60 s
/// `proxy_read_timeout` and Cloudflare's ~100 s idle limit.
pub const KEEPALIVE: Duration = Duration::from_secs(20);
/// Per-destination dial budget: `TARGET_BURST` dials, refilled one per
/// `TARGET_REFILL` (about four per minute per IP:port).
pub const TARGET_BURST: f64 = 4.0;
pub const TARGET_REFILL: Duration = Duration::from_secs(15);
/// Base64 of the largest accepted RouterInfo (padded I2P base64).
const MAX_ROUTER_INFO_BASE64: usize = MAX_ROUTER_INFO_BYTES.div_ceil(3) * 4;
/// Address, identity hash, kind, date and separators fit in 128 bytes.
const MAX_TARGET_ROW: usize = MAX_ROUTER_INFO_BASE64 + 128;

pub struct Policy {
    pub protocol: String,
    pub hosts: HashSet<String>,
    pub origins: HashSet<String>,
    pub targets: RwLock<HashMap<SocketAddrV4, u64>>,
    pub session: Duration,
    pub idle: Duration,
    pub keepalive: Duration,
    pub max_bytes: usize,
    pub rtc_bind: SocketAddrV4,
    pub rtc_public_ip: Option<Ipv4Addr>,
    pub fixture_ice_private: bool,
    permits: Arc<Semaphore>,
    rate: Mutex<(f64, Instant)>,
    dials: Mutex<HashMap<SocketAddrV4, (f64, Instant)>>,
}

/// A panic elsewhere must not turn every later request into a panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Policy {
    pub fn new(
        capability: &str,
        hosts: HashSet<String>,
        origins: HashSet<String>,
        targets: HashMap<SocketAddrV4, u64>,
        connections: usize,
    ) -> Result<Self> {
        ensure!(
            capability.len() == 43
                && capability
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
            "invalid capability"
        );
        ensure!(
            !hosts.is_empty() && !origins.is_empty() && !targets.is_empty(),
            "explicit nonempty allowlists required"
        );
        ensure!(
            (1..=32).contains(&connections),
            "connection limit must be 1..32"
        );
        Ok(Self {
            protocol: format!("{PROTOCOL}{capability}"),
            hosts,
            origins,
            targets: RwLock::new(targets),
            session: Duration::from_secs(600),
            idle: Duration::from_secs(120),
            keepalive: KEEPALIVE,
            max_bytes: 64 * 1024 * 1024,
            rtc_bind: SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0),
            rtc_public_ip: None,
            fixture_ice_private: false,
            permits: Arc::new(Semaphore::new(connections)),
            rate: Mutex::new((32.0, Instant::now())),
            dials: Mutex::new(HashMap::new()),
        })
    }
    fn admit(&self) -> bool {
        let mut rate = lock(&self.rate);
        rate.0 = (rate.0 + rate.1.elapsed().as_secs_f64()).min(32.0);
        rate.1 = Instant::now();
        if rate.0 < 1.0 {
            return false;
        }
        rate.0 -= 1.0;
        true
    }
    /// Per-destination dial limit, applied after authorization: even a valid
    /// capability cannot make the bridge hammer one IP:port.
    pub fn admit_target(&self, addr: SocketAddrV4) -> bool {
        let now = Instant::now();
        let refill = |(tokens, at): (f64, Instant)| {
            (tokens + now.duration_since(at).as_secs_f64() / TARGET_REFILL.as_secs_f64())
                .min(TARGET_BURST)
        };
        let mut dials = lock(&self.dials);
        // Entries exist only for allowlisted targets; reloads replace the
        // allowlist, so forget buckets that have fully refilled.
        if dials.len() >= 2 * MAX_TARGET_ROWS {
            dials.retain(|_, bucket| refill(*bucket) < TARGET_BURST);
        }
        let tokens = dials
            .get(&addr)
            .map_or(TARGET_BURST, |bucket| refill(*bucket));
        let admitted = tokens >= 1.0;
        dials.insert(addr, (if admitted { tokens - 1.0 } else { tokens }, now));
        admitted
    }
    /// WebRTC is served only when the answer cannot advertise internal
    /// interface addresses: an advertised public IP, a concrete public bind
    /// address, or a CI fixture.
    pub fn rtc_enabled(&self) -> bool {
        self.rtc_public_ip.is_some()
            || is_public_ipv4(IpAddr::V4(*self.rtc_bind.ip()))
            || self.fixture_ice_private
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let first = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        None
    } else {
        Some(first)
    }
}
fn canonical_target(value: &str) -> Result<SocketAddrV4> {
    let addr: SocketAddrV4 = value.parse().context("IPv4 literal target required")?;
    ensure!(addr.to_string() == value, "noncanonical target");
    Ok(addr)
}
/// Canonical IPv4 literal on a port allowed by the shared NTCP2 port policy
/// (no port below 1024, no listed service port).
pub fn target(value: &str) -> Result<SocketAddrV4> {
    let addr = canonical_target(value)?;
    ensure!(is_allowed_ntcp2_port(addr.port()), "blocked target port");
    Ok(addr)
}
pub fn authorize(
    policy: &Policy,
    headers: &HeaderMap,
    query: Option<&str>,
) -> Result<SocketAddrV4> {
    ensure!(
        policy.hosts.contains(header(headers, "host").unwrap_or("")),
        "host"
    );
    ensure!(
        policy
            .origins
            .contains(header(headers, "origin").unwrap_or("")),
        "origin"
    );
    let protocol = header(headers, "sec-websocket-protocol").unwrap_or("");
    ensure!(
        bool::from(protocol.as_bytes().ct_eq(policy.protocol.as_bytes())),
        "capability"
    );
    ensure!(
        header(headers, "sec-websocket-version") == Some("13"),
        "version"
    );
    let key = base64::engine::general_purpose::STANDARD
        .decode(header(headers, "sec-websocket-key").unwrap_or(""))?;
    ensure!(key.len() == 16, "key");
    let pairs: Vec<_> = url::form_urlencoded::parse(query.unwrap_or("").as_bytes()).collect();
    ensure!(pairs.len() == 1 && pairs[0].0 == "target", "query");
    let addr = target(&pairs[0].1)?;
    ensure!(
        policy
            .targets
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&addr)
            .copied()
            .unwrap_or(0)
            > now(),
        "target unavailable"
    );
    Ok(addr)
}

/// Reverify every signed RouterInfo; the TSV is not itself an authority.
/// Accepts at most `MAX_TARGET_ROWS` (= prepare-netdb's public maximum plus
/// one fixture row). Rows naming a blocked port are skipped, not fatal.
pub fn load_targets(path: &Path, fixture_loopback: bool) -> Result<HashMap<SocketAddrV4, u64>> {
    ensure!(
        std::fs::metadata(path)?.len() <= (MAX_TARGET_ROWS * MAX_TARGET_ROW) as u64,
        "oversize target file"
    );
    let text = std::fs::read_to_string(path)?;
    let mut targets = HashMap::new();
    let mut count = 0;
    for line in text
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        count += 1;
        ensure!(count <= MAX_TARGET_ROWS, "too many RouterInfos");
        let fields: Vec<_> = line.split('\t').collect();
        ensure!(
            fields.len() == 5 && fields[1].len() <= MAX_ROUTER_INFO_BASE64,
            "invalid target record"
        );
        let addr = canonical_target(fields[0])?;
        if !is_allowed_ntcp2_port(addr.port()) {
            continue;
        }
        let fixture = fixture_loopback
            && fields[3] == "fixture"
            && *addr.ip() == std::net::Ipv4Addr::LOCALHOST;
        if !is_public_ipv4(IpAddr::V4(*addr.ip())) && !fixture {
            continue;
        }
        let bytes = emissary_core::crypto::base64_decode(fields[1])
            .context("invalid RouterInfo encoding")?;
        let Ok(peer) = verified_peer(&bytes) else {
            continue;
        };
        ensure!(
            peer.address == SocketAddr::V4(addr),
            "RouterInfo target mismatch"
        );
        ensure!(
            emissary_core::crypto::base64_encode(peer.router.identity.hash()) == fields[2],
            "RouterInfo identity mismatch"
        );
        let published = *peer.router.published.date() / 1000;
        ensure!(
            fields[4].parse::<u64>()? == published,
            "RouterInfo date mismatch"
        );
        let expires = (published + 36 * 3600).min(now() + 600);
        if expires > now() {
            targets.insert(addr, expires);
        }
    }
    Ok(targets)
}

/// SDP is untrusted egress configuration. No DNS/mDNS, LAN probing, media or
/// trickle ICE. Related-address metadata is never used as a dial target.
/// At least one literal public candidate is required: a NAT'd browser needs
/// STUN (policy `all`) or TURN (policy `relay`); peer-reflexive discovery from
/// a candidate-less offer is deliberately not relied upon.
pub fn validate_offer(sdp: &str, fixture_private: bool) -> Result<()> {
    ensure!(sdp.len() <= FRAME && !sdp.contains('\0'), "oversize SDP");
    let mut media = 0;
    let mut candidates = 0;
    for line in sdp.lines().map(str::trim_end) {
        if line.starts_with("m=") {
            media += 1;
            let words: Vec<_> = line.split_whitespace().collect();
            ensure!(
                words.len() == 4
                    && words[0] == "m=application"
                    && words[2] == "UDP/DTLS/SCTP"
                    && words[3] == "webrtc-datachannel",
                "data channel SDP required"
            );
        }
        if line.starts_with("a=candidate:") {
            candidates += 1;
            ensure!(candidates <= 16, "too many ICE candidates");
            let fields: Vec<_> = line.split_whitespace().collect();
            ensure!(
                fields.len() >= 8 && fields[1] == "1" && fields[2].eq_ignore_ascii_case("udp"),
                "unsupported candidate"
            );
            let ip: IpAddr = fields[4].parse().context("literal candidate required")?;
            ensure!(
                is_public_ipv4(ip)
                    || (fixture_private
                        && matches!(ip, IpAddr::V4(v) if v.is_private() || v.is_loopback())),
                "nonpublic ICE candidate"
            );
            ensure!(fields[5].parse::<u16>()? > 0, "invalid ICE port");
        }
    }
    ensure!(
        media == 1 && candidates > 0,
        "one data channel section and gathered candidates required"
    );
    Ok(())
}

/// webrtc 0.21.1 derives host candidates from the bound sockets and stores,
/// but never applies, `SettingEngine::with_nat_1to1_ips`. The answer is
/// therefore rewritten here: with `public_ip` every host candidate advertises
/// that address (port unchanged, as with 1:1 NAT); without it only literal
/// public IPv4 host candidates survive (private/loopback for CI fixtures).
/// Non-host candidates (their `raddr` names local addresses) are dropped, so
/// VPC, docker0 or other interface addresses never reach the browser.
pub fn filter_answer(
    sdp: &str,
    public_ip: Option<Ipv4Addr>,
    fixture_private: bool,
) -> Result<String> {
    ensure!(sdp.len() <= FRAME && !sdp.contains('\0'), "oversize SDP");
    let mut advertised = HashSet::new();
    let mut lines = Vec::new();
    for line in sdp.lines() {
        if !line.starts_with("a=candidate:") {
            lines.push(line.to_owned());
            continue;
        }
        let mut fields: Vec<_> = line.split_whitespace().map(str::to_owned).collect();
        if fields.len() < 8
            || fields[1] != "1"
            || !fields[2].eq_ignore_ascii_case("udp")
            || fields[6] != "typ"
            || fields[7] != "host"
        {
            continue;
        }
        let (Ok(ip), Ok(port)) = (fields[4].parse::<Ipv4Addr>(), fields[5].parse::<u16>()) else {
            continue;
        };
        let address = match public_ip {
            Some(public) => public,
            None if is_public_ipv4(IpAddr::V4(ip))
                || (fixture_private && (ip.is_private() || ip.is_loopback())) =>
            {
                ip
            }
            None => continue,
        };
        if port == 0 || !advertised.insert((address, port)) {
            continue;
        }
        fields[4] = address.to_string();
        lines.push(fields.join(" "));
    }
    ensure!(!advertised.is_empty(), "no advertisable ICE candidate");
    Ok(lines.join("\r\n") + "\r\n")
}

pub fn app(policy: Arc<Policy>) -> Router {
    Router::new()
        .route("/stream", get(stream_upgrade))
        .route("/rtc", get(rtc_upgrade))
        .with_state(policy)
}
async fn stream_upgrade(
    State(policy): State<Arc<Policy>>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
    ws: WebSocketUpgrade,
) -> Response {
    upgrade(policy, headers, query, ws, false).await
}
async fn rtc_upgrade(
    State(policy): State<Arc<Policy>>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
    ws: WebSocketUpgrade,
) -> Response {
    upgrade(policy, headers, query, ws, true).await
}
async fn upgrade(
    policy: Arc<Policy>,
    headers: HeaderMap,
    query: Option<String>,
    ws: WebSocketUpgrade,
    rtc: bool,
) -> Response {
    let Ok(addr) = authorize(&policy, &headers, query.as_deref()) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    if rtc && !policy.rtc_enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !policy.admit() {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let Ok(permit) = policy.permits.clone().try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    if !policy.admit_target(addr) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    // WSS dials before the upgrade (open == connected). RTC keeps the
    // authorization and permit here but dials only after its DataChannel
    // opens, so slow ICE/TURN/DTLS cannot consume the peer's NTCP2 timeout.
    let tcp = if rtc {
        None
    } else {
        let Ok(Ok(tcp)) = timeout(CONNECT, TcpStream::connect(addr)).await else {
            return StatusCode::BAD_GATEWAY.into_response();
        };
        let _ = tcp.set_nodelay(true);
        Some(tcp)
    };
    ws.protocols([policy.protocol.clone()])
        .max_message_size(FRAME)
        .max_frame_size(FRAME)
        .write_buffer_size(RTC_FRAME)
        .max_write_buffer_size(2 * FRAME)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            match tcp {
                Some(tcp) => {
                    let _ = timeout(policy.session, ws_session(socket, tcp, &policy)).await;
                }
                None => {
                    let _ = rtc_session(socket, addr, &policy).await;
                }
            }
        })
        .into_response()
}

fn keepalive_timer(period: Duration) -> tokio::time::Interval {
    let mut timer = interval_at(tokio::time::Instant::now() + period, period);
    timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
    timer
}

/// Server pings keep proxies from closing an idle NTCP2 stream; they are not
/// activity and never extend `policy.idle`.
async fn ws_session(mut socket: WebSocket, mut tcp: TcpStream, policy: &Policy) -> Result<()> {
    let _ = ws_pump(&mut socket, &mut tcp, policy).await;
    let _ = timeout(Duration::from_secs(2), socket.close()).await;
    Ok(())
}
async fn ws_pump(socket: &mut WebSocket, tcp: &mut TcpStream, policy: &Policy) -> Result<()> {
    let mut buffer = [0; RTC_FRAME];
    let mut total = 0usize;
    let mut ping = keepalive_timer(policy.keepalive);
    let idle = tokio::time::sleep(policy.idle);
    tokio::pin!(idle);
    loop {
        tokio::select! {
            () = &mut idle => bail!("idle"),
            _ = ping.tick() => {
                timeout(SEND, socket.send(Message::Ping(Bytes::new()))).await??;
            }
            message = socket.recv() => match message {
                Some(Ok(Message::Binary(bytes))) => {
                    ensure!(!bytes.is_empty(), "empty frame");
                    total += bytes.len(); ensure!(total <= policy.max_bytes, "byte budget");
                    timeout(SEND, tcp.write_all(&bytes)).await??;
                    idle.as_mut().reset(tokio::time::Instant::now() + policy.idle);
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => (),
                _ => bail!("closed or nonbinary stream"),
            },
            size = tcp.read(&mut buffer) => {
                let size = size?; ensure!(size > 0, "TCP EOF");
                total += size; ensure!(total <= policy.max_bytes, "byte budget");
                timeout(SEND, socket.send(Message::Binary(buffer[..size].to_vec().into()))).await??;
                idle.as_mut().reset(tokio::time::Instant::now() + policy.idle);
            }
        }
    }
}

struct Events {
    gathered: mpsc::Sender<()>,
    channels: mpsc::Sender<Arc<dyn DataChannel>>,
    done: mpsc::Sender<()>,
}
#[async_trait::async_trait]
impl PeerConnectionEventHandler for Events {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.gathered.try_send(());
        }
    }
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        if matches!(
            state,
            RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed
        ) {
            let _ = self.done.try_send(());
        }
    }
    async fn on_data_channel(&self, channel: Arc<dyn DataChannel>) {
        if self.channels.try_send(channel).is_err() {
            let _ = self.done.try_send(());
        }
    }
}

/// After the answer the signaling WebSocket carries no data but remains the
/// session's lifetime control. Pings keep reverse proxies (nginx 60 s,
/// Cloudflare ~100 s) from closing it under a healthy DataChannel.
async fn signaling(socket: &mut WebSocket, keepalive: Duration) -> anyhow::Error {
    let mut ping = keepalive_timer(keepalive);
    loop {
        tokio::select! {
            message = socket.recv() => match message {
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => (),
                _ => return anyhow!("signaling closed or unexpected renegotiation"),
            },
            _ = ping.tick() => {
                if !matches!(timeout(SEND, socket.send(Message::Ping(Bytes::new()))).await, Ok(Ok(()))) {
                    return anyhow!("signaling keepalive failed");
                }
            }
        }
    }
}

async fn rtc_session(mut socket: WebSocket, addr: SocketAddrV4, policy: &Policy) -> Result<()> {
    let offer = timeout(Duration::from_secs(15), socket.recv())
        .await?
        .context("signaling closed")??;
    let Message::Text(raw) = offer else {
        bail!("SDP required")
    };
    let offer: RTCSessionDescription = serde_json::from_str(&raw)?;
    ensure!(
        offer.sdp_type == webrtc::peer_connection::RTCSdpType::Offer,
        "offer required"
    );
    validate_offer(&offer.sdp, policy.fixture_ice_private)?;
    let (gathered, mut gather_rx) = mpsc::channel(1);
    let (channels, mut channel_rx) = mpsc::channel(1);
    let (done, mut done_rx) = mpsc::channel(1);
    let settings = SettingEngineBuilder::new()
        .with_sctp_max_message_size(
            rtc::peer_connection::configuration::setting_engine::SctpMaxMessageSize::Bounded(
                RTC_FRAME as u32,
            ),
        )
        .with_sctp_max_receive_buffer_size((2 * FRAME) as u32)
        .with_dcep_handshake_timeout(Some(Duration::from_secs(15)))
        .with_ice_timeouts(
            Some(Duration::from_secs(10)),
            Some(Duration::from_secs(20)),
            Some(Duration::from_secs(3)),
        )
        // The default (QueryOnly) opens a multicast socket per connection and
        // resolves remote `.local` names; neither is wanted on a bridge.
        .with_multicast_dns_mode(rtc::ice::mdns::MulticastDnsMode::Disabled);
    let peer = timeout(
        OPEN,
        PeerConnectionBuilder::new()
            .with_setting_engine(settings.build())
            .with_data_channel_send_buffer_limit(2 * FRAME)
            .with_handler(Arc::new(Events {
                gathered,
                channels,
                done,
            }))
            .with_udp_addrs(vec![policy.rtc_bind])
            .build(),
    )
    .await??;
    // Never return through ? after construction without explicitly closing the
    // peer's driver and sockets (also on signaling cancellation and timeout).
    let work = async {
        peer.set_remote_description(offer).await?;
        peer.set_local_description(peer.create_answer(None).await?)
            .await?;
        gather_rx.recv().await.context("ICE gathering stopped")?;
        let mut answer = peer.local_description().await.context("missing answer")?;
        answer.sdp = filter_answer(
            &answer.sdp,
            policy.rtc_public_ip,
            policy.fixture_ice_private,
        )?;
        let answer = serde_json::to_string(&answer)?;
        ensure!(answer.len() <= FRAME, "oversize answer");
        socket.send(Message::Text(answer.into())).await?;
        let channel = channel_rx.recv().await.context("no data channel")?;
        ensure!(
            channel.label().await? == LABEL
                && channel.protocol().await? == LABEL
                && channel.ordered().await?
                && channel.max_retransmits().await?.is_none()
                && channel.max_packet_life_time().await?.is_none(),
            "reliable ordered channel required"
        );
        // webrtc-rs announces a remote channel at its OnOpen and queues that
        // event first; dial only once the DataChannel is actually open.
        loop {
            match channel.poll().await {
                Some(DataChannelEvent::OnOpen) => break,
                Some(
                    DataChannelEvent::OnBufferedAmountLow | DataChannelEvent::OnBufferedAmountHigh,
                ) => (),
                _ => bail!("RTC channel closed before open"),
            }
        }
        let tcp = timeout(CONNECT, TcpStream::connect(addr))
            .await
            .context("TCP connect timeout")??;
        let _ = tcp.set_nodelay(true);
        Ok::<_, anyhow::Error>((channel, tcp))
    };
    let ready = timeout(OPEN, work).await;
    if let Ok(Ok((channel, tcp))) = ready {
        let session = async {
            tokio::select! {
                result = rtc_pipe(channel, tcp, policy) => result,
                _ = done_rx.recv() => Err(anyhow!("RTC failed")),
                _ = channel_rx.recv() => Err(anyhow!("extra data channel")),
                error = signaling(&mut socket, policy.keepalive) => Err(error),
            }
        };
        let _ = timeout(policy.session, session).await;
    }
    let _ = timeout(Duration::from_secs(3), peer.close()).await;
    let _ = timeout(Duration::from_secs(2), socket.close()).await;
    Ok(())
}

async fn rtc_pipe(
    channel: Arc<dyn DataChannel>,
    mut tcp: TcpStream,
    policy: &Policy,
) -> Result<()> {
    let mut total = 0usize;
    let mut buffer = [0; RTC_FRAME];
    loop {
        let active = timeout(policy.idle, async {
            tokio::select! {
                event = channel.poll() => match event {
                    Some(DataChannelEvent::OnMessage(message)) if !message.is_string && !message.data.is_empty() && message.data.len() <= RTC_FRAME => {
                        total += message.data.len(); ensure!(total <= policy.max_bytes, "byte budget");
                        timeout(SEND, tcp.write_all(&message.data)).await??;
                    },
                    Some(DataChannelEvent::OnBufferedAmountLow | DataChannelEvent::OnBufferedAmountHigh) => (),
                    _ => bail!("RTC channel closed or invalid frame"),
                },
                size = tcp.read(&mut buffer) => {
                    let size = size?;
                    if size == 0 {
                        timeout(SEND, async {
                            while channel.outstanding_bytes().await? > 0 {
                                tokio::time::sleep(Duration::from_millis(4)).await;
                            }
                            Ok::<_, anyhow::Error>(())
                        }).await??;
                        return Ok::<_, anyhow::Error>(false);
                    }
                    total += size; ensure!(total <= policy.max_bytes, "byte budget");
                    timeout(SEND, channel.send(BytesMut::from(&buffer[..size]))).await??;
                }
            }
            Ok::<_, anyhow::Error>(true)
        }).await??;
        if !active {
            return Ok(());
        }
    }
}
