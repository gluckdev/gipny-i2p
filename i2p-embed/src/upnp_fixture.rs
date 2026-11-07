//! Real miniupnpc HTTP/SOAP exercised against a loopback IGD fixture in CI.
//! The unchecked gateway constructor is test-only; production discovery
//! rejects loopback URLs.
use super::*;
use std::collections::HashMap;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone)]
struct Row {
    client: String,
    port: String,
    owner: String,
}
#[derive(Default)]
struct State {
    rows: HashMap<(String, String), Row>,
    add: usize,
    delete: usize,
    /// Root description downloads.
    roots: usize,
    query_error: bool,
    durable: bool,
    udn: String,
    /// List a disconnected WANPPPConnection before the working service.
    ppp_first: bool,
    agents: Vec<String>,
    labels: Vec<String>,
}
struct Fixture {
    address: std::net::SocketAddr,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    task: Option<JoinHandle<()>>,
    scratch: Scratch,
}
fn field<'a>(body: &'a str, key: &str) -> &'a str {
    body.split_once(&format!("<{key}>"))
        .and_then(|(_, rest)| rest.split_once(&format!("</{key}>")))
        .map_or("", |(value, _)| value)
}
fn fault(code: u16) -> String {
    format!("<s:Fault><detail><UPnPError xmlns=\"urn:schemas-upnp-org:control-1-0\"><errorCode>{code}</errorCode><errorDescription>fixture</errorDescription></UPnPError></detail></s:Fault>")
}
fn never() -> bool {
    false
}
const WAN: Ipv4Addr = Ipv4Addr::new(1, 2, 3, 4);
impl Fixture {
    fn start() -> Self {
        Self::start_with(|_| {})
    }
    fn start_with(setup: impl FnOnce(&mut State)) -> Self {
        let scratch = Scratch::new();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let mut initial = State {
            durable: true,
            ..State::default()
        };
        setup(&mut initial);
        let state = Arc::new(Mutex::new(initial));
        let shared = state.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let journal = scratch.0.join("upnp-mappings");
        let task = std::thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("IGD fixture accept failed: {error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let length = stream.read(&mut chunk).unwrap();
                    if length == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..length]);
                    assert!(request.len() <= 32768);
                    let text = String::from_utf8_lossy(&request);
                    if let Some((header, body)) = text.split_once("\r\n\r\n") {
                        let size = header
                            .lines()
                            .filter_map(|line| line.split_once(':'))
                            .find(|(key, _)| key.eq_ignore_ascii_case("Content-Length"))
                            .map_or(0, |(_, value)| value.trim().parse::<usize>().unwrap());
                        if body.len() >= size {
                            break;
                        }
                    }
                }
                if request.is_empty() {
                    continue;
                }
                let request = String::from_utf8(request).unwrap();
                let (head, body) = request.split_once("\r\n\r\n").unwrap();
                let mut state = shared.lock().unwrap();
                if let Some((_, agent)) = head
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(key, _)| key.eq_ignore_ascii_case("User-Agent"))
                {
                    state.agents.push(agent.trim().to_owned());
                }
                let (status, body) = if request.starts_with("GET /root.xml ") {
                    state.roots += 1;
                    let ip = "<service><serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType><controlURL>/control</controlURL><SCPDURL>/service.xml</SCPDURL></service>";
                    let ppp = "<service><serviceType>urn:schemas-upnp-org:service:WANPPPConnection:1</serviceType><controlURL>/ppp</controlURL><SCPDURL>/ppp.xml</SCPDURL></service>";
                    let services = if state.ppp_first {
                        format!("{ppp}{ip}")
                    } else {
                        ip.to_owned()
                    };
                    let udn = if state.udn.is_empty() {
                        String::new()
                    } else {
                        format!("<UDN>{}</UDN>", state.udn)
                    };
                    (200, format!("<?xml version=\"1.0\"?><root><URLBase>http://{address}/</URLBase><device><deviceType>urn:schemas-upnp-org:device:InternetGatewayDevice:1</deviceType>{udn}<serviceList>{services}</serviceList></device></root>"))
                } else {
                    let key = (
                        field(body, "NewProtocol").to_owned(),
                        field(body, "NewExternalPort").to_owned(),
                    );
                    let (action, fields, code) = if request.contains("#GetStatusInfo") {
                        let status = if request.starts_with("POST /ppp ") {
                            "Disconnected"
                        } else {
                            "Connected"
                        };
                        ("GetStatusInfo", format!("<NewConnectionStatus>{status}</NewConnectionStatus><NewLastConnectionError>ERROR_NONE</NewLastConnectionError><NewUptime>42</NewUptime>"), 200)
                    } else if request.contains("#GetExternalIPAddress") {
                        ("GetExternalIPAddress", format!("<NewExternalIPAddress>{WAN}</NewExternalIPAddress>"), 200)
                    } else if request.contains("#GetSpecificPortMappingEntry") {
                        if state.query_error {
                            ("", fault(501), 500)
                        } else if let Some(row) = state.rows.get(&key) {
                            ("GetSpecificPortMappingEntry", format!("<NewInternalClient>{}</NewInternalClient><NewInternalPort>{}</NewInternalPort><NewPortMappingDescription>{}</NewPortMappingDescription><NewEnabled>1</NewEnabled><NewLeaseDuration>0</NewLeaseDuration>", row.client, row.port, row.owner), 200)
                        } else {
                            ("", fault(714), 500)
                        }
                    } else if request.contains("#AddPortMapping") {
                        state.add += 1;
                        let journal = read_record(&journal, 4096).unwrap_or_default();
                        state.durable &= journal.contains(&format!(
                            "{} {} {}",
                            key.0,
                            key.1,
                            field(body, "NewInternalClient")
                        ));
                        state
                            .labels
                            .push(field(body, "NewPortMappingDescription").to_owned());
                        if field(body, "NewLeaseDuration") != "0" {
                            ("", fault(725), 500)
                        } else {
                            state.rows.insert(
                                key,
                                Row {
                                    client: field(body, "NewInternalClient").into(),
                                    port: field(body, "NewInternalPort").into(),
                                    owner: field(body, "NewPortMappingDescription").into(),
                                },
                            );
                            ("AddPortMapping", String::new(), 200)
                        }
                    } else if request.contains("#DeletePortMapping") {
                        state.delete += 1;
                        state.rows.remove(&key);
                        ("DeletePortMapping", String::new(), 200)
                    } else {
                        panic!("unexpected IGD operation");
                    };
                    let payload = if code == 200 {
                        format!("<u:{action}Response xmlns:u=\"urn:schemas-upnp-org:service:WANIPConnection:1\">{fields}</u:{action}Response>")
                    } else {
                        fields
                    };
                    (code, format!("<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body>{payload}</s:Body></s:Envelope>"))
                };
                drop(state);
                let header = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                stream.write_all(header.as_bytes()).unwrap();
                stream.write_all(body.as_bytes()).unwrap();
            }
        });
        Self {
            address,
            state,
            stop,
            task: Some(task),
            scratch,
        }
    }
    fn url(&self) -> String {
        format!("http://{}/root.xml", self.address)
    }
    fn gateway(&self, secret: &Secret) -> Gateway {
        let mut gateway =
            Gateway::open(&self.url(), secret, false, &never).expect("fixture gateway");
        // Journal entries must name a LAN client; the fixture is on loopback.
        gateway.client = Ipv4Addr::new(192, 168, 1, 20);
        gateway
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(100));
        if let Some(task) = self.task.take() {
            task.join().unwrap();
        }
    }
}

struct FixtureEnv<'a> {
    fixture: &'a Fixture,
    published: Vec<Ipv4Addr>,
}
impl Env for FixtureEnv<'_> {
    type Gateway = Gateway;
    fn ports(&mut self) -> Ports {
        Ports::Active {
            tcp: 34567,
            udp: 34567,
        }
    }
    fn discover(&mut self, secret: &Secret, _: &dyn Fn() -> bool) -> Option<Gateway> {
        Some(self.fixture.gateway(secret))
    }
    fn publish(&mut self, external: Ipv4Addr) {
        self.published.push(external);
    }
}

#[test]
fn real_igd_create_renew_recreate_permanent_fallback_and_owned_cleanup() {
    let fixture = Fixture::start();
    std::fs::write(fixture.scratch.0.join("upnp-owner"), "1".repeat(32)).unwrap();
    let mut registry = Registry::open(&fixture.scratch.0).unwrap();
    let gateway = fixture.gateway(&registry.secret);
    let permanent = Outcome::Confirmed { permanent: true };
    let tcp = Rule {
        protocol: "TCP",
        port: 34567,
        client: gateway.client,
    };
    assert_eq!(maintain(&gateway, &tcp, &mut registry, &never), permanent);
    assert_eq!(maintain(&gateway, &tcp, &mut registry, &never), permanent);
    assert_eq!(
        fixture.state.lock().unwrap().add,
        4,
        "create and renew each negotiate a permanent-only IGD"
    );
    fixture.state.lock().unwrap().query_error = true;
    assert_eq!(maintain(&gateway, &tcp, &mut registry, &never), Outcome::Failed);
    assert_eq!(
        fixture.state.lock().unwrap().add,
        4,
        "a SOAP error is not proof a port is free"
    );
    {
        let mut state = fixture.state.lock().unwrap();
        state.query_error = false;
        state
            .rows
            .get_mut(&("TCP".into(), "34567".into()))
            .unwrap()
            .owner = "another application".into();
    }
    assert_eq!(maintain(&gateway, &tcp, &mut registry, &never), Outcome::Foreign);
    let udp = Rule {
        protocol: "UDP",
        ..tcp.clone()
    };
    assert_eq!(maintain(&gateway, &udp, &mut registry, &never), permanent);
    fixture
        .state
        .lock()
        .unwrap()
        .rows
        .remove(&("UDP".into(), "34567".into()));
    assert_eq!(
        maintain(&gateway, &udp, &mut registry, &never),
        permanent,
        "router reboot/lost rule is repaired"
    );
    let mut saved = Registry::open(&fixture.scratch.0).unwrap();
    assert_eq!(saved.secret, registry.secret);
    assert_eq!(saved.entries, registry.entries);
    assert!(saved.entries.iter().all(|e| e.gateway == gateway.key.id));
    assert_eq!(read_record(&saved.path, 4096).unwrap().lines().count(), 2);
    cleanup_except(&gateway, &mut saved, &[], &never);
    assert!(saved.entries.is_empty());
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.rows.len(), 1, "foreign rule survives cleanup");
    assert_eq!(
        state.delete, 1,
        "only the verified owned UDP rule is removed"
    );
    assert!(
        state.durable,
        "mapping intent was flushed before every AddPortMapping"
    );
}

#[test]
fn real_igd_one_description_fetch_reverification_and_neutral_wire() {
    let fixture = Fixture::start_with(|state| {
        state.udn = "uuid:fixture-1".into();
        state.ppp_first = true;
    });
    let mut registry = Registry::open(&fixture.scratch.0).unwrap();
    let gateway = fixture.gateway(&registry.secret);
    assert_eq!(
        fixture.state.lock().unwrap().roots,
        1,
        "one fetch and one parse serve the disconnected and the working service"
    );
    assert_eq!(
        text(&gateway.data.first.kind).as_deref(),
        Some("urn:schemas-upnp-org:service:WANIPConnection:1")
    );
    assert_eq!(
        gateway.key,
        gateway_key(&registry.secret, "uuid:fixture-1", Ipv4Addr::LOCALHOST).unwrap()
    );
    assert!(gateway.same_device());
    assert_eq!(fixture.state.lock().unwrap().roots, 2);
    assert_eq!(gateway.external(), Some(WAN));
    let rule = Rule {
        protocol: "TCP",
        port: 34567,
        client: gateway.client,
    };
    assert_eq!(
        maintain(&gateway, &rule, &mut registry, &never),
        Outcome::Confirmed { permanent: true }
    );
    // The same address now answers as another device: not ours to touch.
    fixture.state.lock().unwrap().udn = "uuid:fixture-2".into();
    assert!(!gateway.same_device());
    let state = fixture.state.lock().unwrap();
    assert!(!state.agents.is_empty());
    for agent in &state.agents {
        assert!(agent.ends_with(" UPnP/1.1 MiniUPnPc/2.3.3"), "{agent}");
        assert!(!agent.to_ascii_lowercase().contains("gipny"), "{agent}");
        assert!(!agent.split(' ').next().unwrap_or("/").contains('/'), "no OS version: {agent}");
    }
    assert!(!state.labels.is_empty());
    for label in &state.labels {
        assert!(label.len() <= 16, "{label}");
        assert!(!label.to_ascii_lowercase().contains("gipny"), "{label}");
        assert_eq!(label, &gateway.key.label);
    }
}

#[test]
fn real_igd_focus_churn_keeps_mappings_and_exit_removes_permanent_ones() {
    let fixture = Fixture::start_with(|state| state.udn = "uuid:fixture".into());
    let registry = Registry::open(&fixture.scratch.0).unwrap();
    let mut env = FixtureEnv {
        fixture: &fixture,
        published: Vec::new(),
    };
    let t0 = Instant::now();
    let mut session = Session::new(registry, t0);
    assert_eq!(session.cycle(&mut env, t0, &never), RENEW);
    let adds = fixture.state.lock().unwrap().add;
    assert_eq!(fixture.state.lock().unwrap().rows.len(), 2);
    // Each round is what a window focus or an Android network callback sends.
    for round in 1..=5u32 {
        let now = t0 + RECHECK * round;
        session.network(true, now);
        session.cycle(&mut env, now, &never);
        session.network(false, now);
        session.cycle(&mut env, now, &never);
    }
    {
        let state = fixture.state.lock().unwrap();
        assert_eq!((state.add, state.delete), (adds, 0));
        assert_eq!(state.rows.len(), 2);
        assert_eq!(state.roots, 6, "discovery plus one verification per round");
    }
    assert_eq!(env.published, vec![WAN]);
    session.shutdown(&never);
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.delete, 2, "this IGD only grants permanent leases");
    assert!(state.rows.is_empty());
}
