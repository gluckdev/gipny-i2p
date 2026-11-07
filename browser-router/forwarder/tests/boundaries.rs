use axum::http::{HeaderMap, HeaderValue};
use emissary_core::{
    Config,
    crypto::{SigningPrivateKey, StaticPrivateKey, base64_encode},
    primitives::{RouterAddress, RouterInfo},
    runtime::Runtime,
};
use futures_util::{SinkExt, StreamExt};
use gipny_browser_router::{
    BrowserRuntime,
    ntcp2::{MAX_PUBLIC_TARGETS, MAX_TARGET_ROWS, is_allowed_ntcp2_port, verified_peer},
};
use gipny_forwarder::{
    PROTOCOL, Policy, TARGET_BURST, app, authorize, filter_answer, load_targets, target,
    validate_offer,
};
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    net::{SocketAddr, SocketAddrV4},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest, http::Request},
};

fn policy(addr: SocketAddrV4, connections: usize) -> Policy {
    Policy::new(
        &"a".repeat(43),
        HashSet::from(["localhost:1234".into()]),
        HashSet::from(["https://client.example".into()]),
        HashMap::from([(
            addr,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + 60,
        )]),
        connections,
    )
    .unwrap()
}
fn headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (key, value) in [
        ("host", "localhost:1234".into()),
        ("origin", "https://client.example".into()),
        (
            "sec-websocket-protocol",
            format!("{PROTOCOL}{}", "a".repeat(43)),
        ),
        ("sec-websocket-version", "13".into()),
        ("sec-websocket-key", "MDEyMzQ1Njc4OWFiY2RlZg==".into()),
    ] {
        headers.insert(key, HeaderValue::from_str(&value).unwrap());
    }
    headers
}
fn request(host: &str, path: &str, addr: SocketAddrV4, origin: &str) -> Request<()> {
    let mut request = format!("ws://{host}/{path}?target={addr}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("origin", origin.parse().unwrap());
    request.headers_mut().insert(
        "sec-websocket-protocol",
        format!("{PROTOCOL}{}", "a".repeat(43)).parse().unwrap(),
    );
    request
}
/// Loopback echo standing in for an NTCP2 peer; counts TCP dials.
async fn echo_target() -> (SocketAddrV4, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let echo = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = target(&echo.local_addr().unwrap().to_string()).unwrap();
    let dials = Arc::new(AtomicUsize::new(0));
    let observed = dials.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut tcp, _) = echo.accept().await.unwrap();
            observed.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut bytes = [0; 16384];
                while let Ok(n) = tcp.read(&mut bytes).await {
                    if n == 0 || tcp.write_all(&bytes[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    (addr, dials, task)
}
async fn serve(policy: Policy) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = listener.local_addr().unwrap().to_string();
    let mut policy = policy;
    policy.hosts = HashSet::from([host.clone()]);
    let server = tokio::spawn(async move {
        axum::serve(listener, app(Arc::new(policy))).await.unwrap();
    });
    (host, server)
}
/// A genuinely signed, fresh mainnet RouterInfo publishing NTCP2 on `port`.
fn signed_router(seed: u8, port: u16) -> Vec<u8> {
    let key = [seed; 32];
    let signing = SigningPrivateKey::random(BrowserRuntime::rng());
    let address = RouterAddress::new_published_ntcp2(
        key,
        [seed; 16],
        None,
        true,
        "8.8.8.8".parse().unwrap(),
        SocketAddr::from(([8, 8, 8, 8], port)),
    );
    RouterInfo::new::<BrowserRuntime>(
        &Config::default(),
        Some(address),
        None,
        None,
        None,
        &StaticPrivateKey::from_bytes(key),
        &signing,
        false,
    )
    .serialize(&signing)
}
fn tsv(rows: &[String]) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all((rows.join("\n") + "\n").as_bytes()).unwrap();
    file
}

#[test]
fn authentication_and_query_fail_closed() {
    let policy = policy("8.8.8.8:1234".parse().unwrap(), 1);
    let valid = headers();
    assert!(authorize(&policy, &valid, Some("target=8.8.8.8%3A1234")).is_ok());
    for field in [
        "host",
        "origin",
        "sec-websocket-protocol",
        "sec-websocket-key",
        "sec-websocket-version",
    ] {
        let mut missing = valid.clone();
        missing.remove(field);
        assert!(authorize(&policy, &missing, Some("target=8.8.8.8:1234")).is_err());
        let mut duplicate = valid.clone();
        duplicate.append(field, valid[field].clone());
        assert!(authorize(&policy, &duplicate, Some("target=8.8.8.8:1234")).is_err());
    }
    for query in [
        "target=8.8.8.8:1234&target=8.8.8.8:1234",
        "target=8.8.8.8:1234&extra=x",
        "target=localhost:1234",
        "target=127.0.0.1:1234",
        "target=8.8.8.8:1234&",
        "",
    ] {
        // An empty trailing query separator has no semantic effect; all other
        // malformed / extra / unlisted destinations must be rejected.
        if !query.ends_with('&') {
            assert!(authorize(&policy, &valid, Some(query)).is_err());
        }
    }
    for value in [
        "127.1:80",
        "8.8.8.8:0",
        "8.8.8.8:0080",
        "[::1]:80",
        "example.com:80",
    ] {
        assert!(target(value).is_err());
    }
}

#[test]
fn port_policy_refuses_privileged_and_service_ports() {
    for port in [
        0, 22, 25, 80, 443, 1023, 1080, 1900, 2049, 2375, 3128, 3306, 3389, 4444, 5060, 5432, 5900,
        6379, 6667, 7070, 7657, 8080, 8332, 8333, 9050, 9051, 9151, 9200, 11211, 27017, 31000,
    ] {
        assert!(!is_allowed_ntcp2_port(port), "{port} must be refused");
        assert!(target(&format!("8.8.8.8:{port}")).is_err(), "{port}");
    }
    for port in [1024, 9152, 12345, 19720, 30777, 40000, 65535] {
        assert!(is_allowed_ntcp2_port(port), "{port} must be allowed");
        assert_eq!(target(&format!("8.8.8.8:{port}")).unwrap().port(), port);
    }
    // An allowlisted service port is unreachable even if a stale TSV listed it.
    let blocked: SocketAddrV4 = "8.8.8.8:6379".parse().unwrap();
    let policy = policy(blocked, 1);
    assert!(authorize(&policy, &headers(), Some("target=8.8.8.8:6379")).is_err());
}

#[test]
fn signed_router_info_cannot_name_a_service_port() {
    // Anyone can sign a RouterInfo: the signature check alone must not make
    // victim:6379 or victim:25 a dial target.
    for (seed, port) in [(1, 6379), (2, 25), (3, 3389)] {
        assert!(verified_peer(&signed_router(seed, port)).is_err(), "{port}");
    }
    let good = signed_router(4, 12345);
    let peer = verified_peer(&good).unwrap();
    assert_eq!(peer.address, SocketAddr::from(([8, 8, 8, 8], 12345)));
    let row = |addr: &str| {
        format!(
            "{addr}\t{}\t{}\tnetdb\t{}",
            base64_encode(&good),
            base64_encode(peer.router.identity.hash()),
            peer.router.published.date() / 1000
        )
    };
    let file = tsv(&[row("8.8.8.8:12345")]);
    let loaded = load_targets(file.path(), false).unwrap();
    assert!(loaded.contains_key(&"8.8.8.8:12345".parse().unwrap()));
    // A service-port row (old or foreign generator) is skipped before any
    // decoding and never aborts the reload of the remaining rows.
    let file = tsv(&[
        "8.8.8.8:6379\tnot-a-router-info\tx\tnetdb\t0".into(),
        row("8.8.8.8:12345"),
    ]);
    assert_eq!(load_targets(file.path(), false).unwrap().len(), 1);
}

#[test]
fn allowlist_accepts_exactly_what_prepare_netdb_emits() {
    assert_eq!(MAX_TARGET_ROWS, MAX_PUBLIC_TARGETS + 1);
    // Rows on a blocked port are counted, then skipped without decoding.
    let rows: Vec<String> = (0..MAX_TARGET_ROWS)
        .map(|i| format!("8.8.{}.{}:6379\tx\tx\tnetdb\t0", i / 256, i % 256))
        .collect();
    let full = tsv(&rows);
    assert!(load_targets(full.path(), false).unwrap().is_empty());
    let mut rows = rows;
    rows.push("8.8.9.1:6379\tx\tx\tnetdb\t0".into());
    let over = tsv(&rows);
    assert!(load_targets(over.path(), false).is_err());
}

#[test]
fn dials_per_target_are_rate_limited() {
    let victim: SocketAddrV4 = "8.8.8.8:12345".parse().unwrap();
    let policy = policy(victim, 1);
    for _ in 0..TARGET_BURST as usize {
        assert!(policy.admit_target(victim));
    }
    assert!(!policy.admit_target(victim));
    assert!(policy.admit_target("8.8.4.4:12345".parse().unwrap()));
}

#[test]
fn sdp_cannot_probe_private_networks_or_negotiate_media() {
    let valid = "v=0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=candidate:1 1 UDP 123 8.8.8.8 5678 typ host\r\n";
    assert!(validate_offer(valid, false).is_ok());
    for candidate in [
        "127.0.0.1",
        "10.0.0.1",
        "169.254.169.254",
        "100.64.0.1",
        "x.local",
        "::1",
    ] {
        assert!(validate_offer(&valid.replace("8.8.8.8", candidate), false).is_err());
    }
    assert!(validate_offer(&valid.replace("8.8.8.8", "127.0.0.1"), true).is_ok());
    assert!(validate_offer(&valid.replace("m=application", "m=audio"), false).is_err());
    assert!(validate_offer(&format!("{valid}m=video 9 UDP/TLS/RTP/SAVPF 96\r\n"), false).is_err());
    assert!(validate_offer(&valid.replace("UDP 123", "TCP 123"), false).is_err());
    assert!(validate_offer(&"x".repeat(65537), false).is_err());
    // No candidate: a NAT'd browser must use STUN/TURN; prflx is not relied on.
    let empty = "v=0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n";
    assert!(validate_offer(empty, false).is_err());
}

#[test]
fn answer_never_advertises_internal_interfaces() {
    let candidate = |ip: &str, port: u16, kind: &str| {
        format!("a=candidate:1 1 udp 2130706431 {ip} {port} typ {kind}\r\n")
    };
    let sdp = format!(
        "v=0\r\nc=IN IP4 0.0.0.0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n{}{}{}{}a=end-of-candidates\r\n",
        candidate("10.1.2.3", 40001, "host"),
        candidate("172.17.0.1", 40002, "host"),
        candidate("8.8.8.8", 40003, "host"),
        candidate("8.8.4.4", 40004, "srflx raddr 10.1.2.3 rport 40001"),
    );
    // Concrete public bind: only the public host candidate survives.
    let public = filter_answer(&sdp, None, false).unwrap();
    assert!(public.contains("8.8.8.8 40003 typ host"));
    for leaked in ["10.1.2.3", "172.17.0.1", "8.8.4.4", "raddr"] {
        assert!(!public.contains(leaked), "{leaked}");
    }
    assert!(public.contains("a=end-of-candidates") && public.contains("m=application"));
    // 1:1 NAT: every host candidate advertises the public IP, ports unchanged.
    let mapped = filter_answer(&sdp, Some("203.0.114.7".parse().unwrap()), false).unwrap();
    for port in [40001, 40002, 40003] {
        assert!(mapped.contains(&format!("203.0.114.7 {port} typ host")));
    }
    for leaked in ["10.1.2.3", "172.17.0.1", "8.8.8.8 ", "srflx"] {
        assert!(!mapped.contains(leaked), "{leaked}");
    }
    // Fixtures may keep private addresses; nothing advertisable fails closed.
    assert!(
        filter_answer(&sdp, None, true)
            .unwrap()
            .contains("10.1.2.3")
    );
    let private = sdp.replace("8.8.8.8", "192.168.1.1");
    assert!(filter_answer(&private.replace("8.8.4.4", "192.168.1.2"), None, false).is_err());
}

#[tokio::test]
async fn wss_byte_order_rejection_admission_and_cleanup() {
    let (addr, dials, echo_task) = echo_target().await;
    let mut policy = policy(addr, 1);
    policy.session = Duration::from_secs(3);
    let (host, server) = serve(policy).await;
    assert!(
        connect_async(request(&host, "stream", addr, "https://attacker.example"))
            .await
            .is_err()
    );
    assert_eq!(dials.load(Ordering::SeqCst), 0);
    let (mut client, _) = connect_async(request(&host, "stream", addr, "https://client.example"))
        .await
        .unwrap();
    assert!(
        connect_async(request(&host, "stream", addr, "https://client.example"))
            .await
            .is_err()
    );
    let bytes: Vec<u8> = (0..100_000).map(|i| (i % 251) as u8).collect();
    for part in bytes.chunks(65536) {
        client
            .send(Message::Binary(part.to_vec().into()))
            .await
            .unwrap();
    }
    let mut returned = Vec::new();
    while returned.len() < bytes.len() {
        let Message::Binary(part) = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        else {
            panic!("binary response required")
        };
        returned.extend_from_slice(&part);
    }
    assert_eq!(returned, bytes);
    let _ = tokio::time::timeout(Duration::from_secs(5), client.next())
        .await
        .unwrap();
    // Expiry releases the permit even if the peer never performs a close handshake.
    tokio::time::sleep(Duration::from_millis(40)).await;
    let (mut next, _) = connect_async(request(&host, "stream", addr, "https://client.example"))
        .await
        .unwrap();
    next.send(Message::Text("forbidden".into())).await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), next.next())
        .await
        .unwrap();
    assert!(!matches!(response, Some(Ok(Message::Binary(_)))));
    server.abort();
    echo_task.abort();
}

#[tokio::test]
async fn wss_keepalive_pings_do_not_extend_idle() {
    let (addr, _dials, echo_task) = echo_target().await;
    let mut policy = policy(addr, 1);
    policy.keepalive = Duration::from_millis(100);
    policy.idle = Duration::from_millis(900);
    let (host, server) = serve(policy).await;
    let (mut client, _) = connect_async(request(&host, "stream", addr, "https://client.example"))
        .await
        .unwrap();
    let started = tokio::time::Instant::now();
    let mut pings = 0;
    // tungstenite answers each Ping with a Pong while we read; the bridge must
    // still end the idle stream instead of treating Pong as activity.
    loop {
        match tokio::time::timeout(Duration::from_secs(5), client.next())
            .await
            .expect("idle stream must close")
        {
            Some(Ok(Message::Ping(_))) => pings += 1,
            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
            Some(Ok(other)) => panic!("unexpected {other:?}"),
        }
    }
    assert!(pings >= 3, "pings={pings}");
    assert!(started.elapsed() < Duration::from_secs(3));
    server.abort();
    echo_task.abort();
}

#[tokio::test]
async fn rtc_requires_a_non_leaking_configuration() {
    let (addr, dials, echo_task) = echo_target().await;
    // Default: wildcard bind, no advertised public IP, no fixture -> no /rtc.
    let (host, server) = serve(policy(addr, 1)).await;
    let refused = connect_async(request(&host, "rtc", addr, "https://client.example")).await;
    assert!(matches!(
        refused,
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status() == 404
    ));
    assert_eq!(dials.load(Ordering::SeqCst), 0);
    server.abort();
    echo_task.abort();
}

mod rtc_client {
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use webrtc::peer_connection::{PeerConnectionEventHandler, RTCIceGatheringState};

    pub struct Gathered(pub mpsc::Sender<()>);
    #[async_trait::async_trait]
    impl PeerConnectionEventHandler for Gathered {
        async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
            if state == RTCIceGatheringState::Complete {
                let _ = self.0.try_send(());
            }
        }
    }
    pub fn handler() -> (Arc<Gathered>, mpsc::Receiver<()>) {
        let (sender, receiver) = mpsc::channel(1);
        (Arc::new(Gathered(sender)), receiver)
    }
}

#[tokio::test]
async fn rtc_dials_after_channel_open_and_keeps_signaling_alive() {
    use webrtc::{
        data_channel::{DataChannelEvent, RTCDataChannelInit},
        peer_connection::{PeerConnection, PeerConnectionBuilder, RTCSessionDescription},
    };
    const LABEL: &str = "gipny-ntcp2-v1";
    let (addr, dials, echo_task) = echo_target().await;
    let mut policy = policy(addr, 2);
    policy.keepalive = Duration::from_millis(200);
    policy.rtc_bind = "127.0.0.1:0".parse().unwrap();
    policy.fixture_ice_private = true;
    let (host, server) = serve(policy).await;

    let (handler, mut gathered) = rtc_client::handler();
    let client = PeerConnectionBuilder::new()
        .with_handler(handler)
        .with_udp_addrs(vec!["127.0.0.1:0"])
        .build()
        .await
        .unwrap();
    let channel = client
        .create_data_channel(
            LABEL,
            Some(RTCDataChannelInit {
                protocol: LABEL.into(),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    client
        .set_local_description(client.create_offer(None).await.unwrap())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), gathered.recv())
        .await
        .unwrap();
    let mut offer = client.local_description().await.unwrap();
    // As the browser's filteredSdp: component-1 UDP candidates only.
    offer.sdp = offer
        .sdp
        .lines()
        .filter(|line| {
            !line.starts_with("a=candidate:") || line.split_whitespace().nth(1) == Some("1")
        })
        .collect::<Vec<_>>()
        .join("\r\n")
        + "\r\n";
    validate_offer(&offer.sdp, true).unwrap();
    let (mut signal, _) = connect_async(request(&host, "rtc", addr, "https://client.example"))
        .await
        .unwrap();
    signal
        .send(Message::Text(serde_json::to_string(&offer).unwrap().into()))
        .await
        .unwrap();
    let answer = loop {
        match tokio::time::timeout(Duration::from_secs(10), signal.next())
            .await
            .unwrap()
        {
            Some(Ok(Message::Text(answer))) => break answer,
            Some(Ok(Message::Ping(_))) => (),
            other => panic!("answer required: {other:?}"),
        }
    };
    // ICE, DTLS and SCTP have not even started: the peer is not dialled yet.
    assert_eq!(dials.load(Ordering::SeqCst), 0);
    let answer: RTCSessionDescription = serde_json::from_str(&answer).unwrap();
    assert!(answer.sdp.contains("127.0.0.1"));
    client.set_remote_description(answer).await.unwrap();
    let poll = |what: &'static str| {
        let channel = channel.clone();
        async move {
            loop {
                match tokio::time::timeout(Duration::from_secs(10), channel.poll())
                    .await
                    .unwrap_or_else(|_| panic!("{what} timeout"))
                {
                    Some(DataChannelEvent::OnOpen) if what == "open" => return None,
                    Some(DataChannelEvent::OnMessage(message)) if what == "message" => {
                        return Some(message.data);
                    }
                    Some(
                        DataChannelEvent::OnBufferedAmountLow
                        | DataChannelEvent::OnBufferedAmountHigh,
                    ) => (),
                    other => panic!("{what}: {other:?}"),
                }
            }
        }
    };
    poll("open").await;
    channel
        .send(bytes::BytesMut::from(&b"ntcp2 ciphertext"[..]))
        .await
        .unwrap();
    assert_eq!(&poll("message").await.unwrap()[..], b"ntcp2 ciphertext");
    assert_eq!(dials.load(Ordering::SeqCst), 1);
    // After the answer only server pings keep the signaling path alive.
    let pinged = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match signal.next().await {
                Some(Ok(Message::Ping(_))) => break,
                Some(Ok(Message::Pong(_))) => (),
                other => panic!("signaling ended: {other:?}"),
            }
        }
    })
    .await;
    assert!(pinged.is_ok());
    let _ = client.close().await;
    server.abort();
    echo_task.abort();
}
