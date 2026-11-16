//! Controlled file goodput and concurrent, bounded message-latency measurement.
//! This is a separate opt-in scenario; ordinary delivery and restart tests keep
//! their original behavior. All clocks are monotonic and startup is excluded.

mod stats;
mod worker;

use anyhow::{bail, ensure, Context, Result};
use gipny_libcore::{SessionEvent, SessionManager};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc::Receiver;

use super::{file_transfer, no_local_ports, start_bot, start_in_process_relays, BotHandle};
use stats::{millis, Probes};
use worker::{Job, Worker};

const FORBIDDEN_ENV: &[&str] = &[
    "GIPNY_FAST",
    "GIPNY_FAST_HOPS",
    "GIPNY_SERVER_HOPS",
    "I2P_EMBED_STUB",
    "I2P_EMBED_SKIP_NATIVE",
];

#[derive(Serialize)]
struct Config {
    file_bytes: usize,
    timeout_secs: u64,
    probe_interval_ms: u64,
    probe_timeout_secs: u64,
    max_in_flight: usize,
    requested_file_lanes: Option<usize>,
    in_process_relays: bool,
    #[serde(skip)]
    work_dir: PathBuf,
    #[serde(skip)]
    report_path: PathBuf,
}

fn bounded(raw: Option<&str>, default: u64, min: u64, max: u64, name: &str) -> Result<u64> {
    let value = match raw {
        Some(s) => s
            .parse::<u64>()
            .with_context(|| format!("{name} must be an integer"))?,
        None => default,
    };
    ensure!(
        (min..=max).contains(&value),
        "{name} must be in {min}..={max}"
    );
    Ok(value)
}

fn env_number(name: &str, default: u64, min: u64, max: u64) -> Result<u64> {
    bounded(std::env::var(name).ok().as_deref(), default, min, max, name)
}

fn validate_environment(present: impl Fn(&str) -> bool) -> Result<()> {
    for name in FORBIDDEN_ENV {
        ensure!(
            !present(name),
            "unset {name}: benchmark requires native routers and normal three-hop tunnels"
        );
    }
    for name in [
        "E2E_AGENT_BIN",
        "E2E_DHT_OFFLINE",
        "E2E_UPDATE_CHECK",
        "E2E_BIG_FILE_RESTART",
        "E2E_BOTH_FIRST",
    ] {
        ensure!(
            !present(name),
            "unset {name}: this scenario cannot be combined with the performance benchmark"
        );
    }
    Ok(())
}

impl Config {
    fn from_env() -> Result<Self> {
        validate_environment(|name| std::env::var_os(name).is_some())?;
        let requested_file_lanes = std::env::var("GIPNY_FILE_LANES")
            .ok()
            .map(|s| bounded(Some(&s), 4, 1, 8, "GIPNY_FILE_LANES").map(|n| n as usize))
            .transpose()?;
        Ok(Self {
            file_bytes: env_number(
                "E2E_BIG_FILE",
                10 * 1024 * 1024,
                1024 * 1024,
                256 * 1024 * 1024,
            )? as usize,
            timeout_secs: env_number("E2E_TIMEOUT_SECS", 900, 1, 7200)?,
            probe_interval_ms: env_number("E2E_PERF_PROBE_INTERVAL_MS", 2000, 250, 60_000)?,
            probe_timeout_secs: env_number("E2E_PERF_PROBE_TIMEOUT_SECS", 60, 1, 300)?,
            max_in_flight: env_number("E2E_PERF_MAX_IN_FLIGHT", 4, 1, 16)? as usize,
            requested_file_lanes,
            in_process_relays: std::env::var("E2E_IN_PROCESS_RELAYS").is_ok_and(|v| v == "1"),
            work_dir: std::env::var_os("E2E_WORK_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| "/tmp/e2e-harness".into()),
            report_path: std::env::var_os("E2E_PERF_REPORT")
                .map(PathBuf::from)
                .context("E2E_PERF_REPORT is required")?,
        })
    }
}

#[derive(Default, Serialize)]
struct FileReport {
    requested_bytes: usize,
    verified_unique_bytes: usize,
    elapsed_ms: Option<u64>,
    goodput_bytes_per_sec: Option<f64>,
    duplicate_completion_events: usize,
}

impl FileReport {
    fn verify(
        &mut self,
        expected: &[u8],
        received: &[u8],
        elapsed: impl FnOnce() -> Duration,
    ) -> Result<()> {
        ensure!(
            received == expected,
            "file verification failed: received {} bytes, expected {}",
            received.len(),
            expected.len()
        );
        if self.elapsed_ms.is_some() {
            self.duplicate_completion_events += 1;
            return Ok(());
        }
        let elapsed = elapsed();
        ensure!(
            !elapsed.is_zero(),
            "file verification has no measurable elapsed time"
        );
        self.verified_unique_bytes = expected.len();
        self.elapsed_ms = Some(millis(elapsed));
        self.goodput_bytes_per_sec = Some(expected.len() as f64 / elapsed.as_secs_f64());
        Ok(())
    }
}

#[derive(Serialize)]
struct Report {
    schema_version: u8,
    status: &'static str,
    phase: &'static str,
    measurement_started: bool,
    run_id: String,
    source_sha: Option<String>,
    topology: &'static str,
    configured_file_lanes: usize,
    participant_hops: Option<[u8; 4]>,
    padded: bool,
    setup_elapsed_ms: Option<u64>,
    warmup_rtt_ms: Option<u64>,
    file: FileReport,
    probes: Value,
    echo_send_errors: usize,
    transport: Value,
    errors: Vec<String>,
}

fn write_report(path: &Path, config: &Config, report: &Report) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).context("create performance report directory")?;
    }
    let mut value = serde_json::to_value(report)?;
    value["config"] = serde_json::to_value(config)?;
    value["metric_definitions"] = json!({
        "goodput": "one completely byte-verified file / time from enqueue start through receiver read and verification; excludes startup and probe drain",
        "rtt": "nearest-rank percentiles of unique on-time application echo RTTs; includes queueing; not one-way media latency",
        "jitter": "mean absolute difference between consecutive on-time RTT samples in arrival order",
        "lost": "known successfully queued probes without an echo before their deadline; late echoes remain lost; interrupted unknown enqueue outcomes are reported separately",
        "peak_send_lanes": "open streams available at successful part enqueue; does not prove every stream carried useful bytes"
    });
    std::fs::write(path, serde_json::to_vec_pretty(&value)?).context("write performance report")
}

pub(super) async fn run() -> Result<()> {
    let config = Config::from_env()?;
    let mut nonce = [0u8; 16];
    gipny_libcore::crypto::fill_random(&mut nonce);
    let run_id: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    let mut report = Report {
        schema_version: 1,
        status: "running",
        phase: "setup",
        measurement_started: false,
        run_id,
        source_sha: std::env::var("GITHUB_SHA").ok(),
        topology: if config.in_process_relays {
            "shared_router_personal_relays"
        } else {
            "shared_client_router_external_relays"
        },
        configured_file_lanes: gipny_libcore::files::lanes::limit(),
        participant_hops: None,
        padded: true,
        setup_elapsed_ms: None,
        warmup_rtt_ms: None,
        file: FileReport {
            requested_bytes: config.file_bytes,
            ..Default::default()
        },
        probes: Value::Null,
        echo_send_errors: 0,
        transport: Value::Null,
        errors: vec![],
    };
    write_report(&config.report_path, &config, &report)?;
    let outcome = execute(&config, &mut report, nonce).await;
    match &outcome {
        Ok(()) => {
            report.status = "passed";
            report.phase = "complete";
        }
        Err(error) => {
            report.status = "failed";
            report.errors.push(format!("{error:#}"));
        }
    }
    write_report(&config.report_path, &config, &report)?;
    println!("[e2e-performance] {}", config.report_path.display());
    outcome
}

#[derive(Deserialize)]
struct RelayPolicy {
    destination: String,
    inbound_hops: u8,
    outbound_hops: u8,
}

fn validate_relay_policy(bytes: &[u8], destination: &str) -> Result<()> {
    let policy: RelayPolicy =
        serde_json::from_slice(bytes).context("parse relay destination policy")?;
    ensure!(
        policy.destination == destination,
        "relay policy belongs to a different destination"
    );
    ensure!(
        policy.inbound_hops == 3 && policy.outbound_hops == 3,
        "relay must use three hops in both directions"
    );
    Ok(())
}

fn external_relay(name: &str, fallback: Option<&str>) -> Result<String> {
    let address_key = format!("E2E_RELAY_DEST_{name}");
    let address = std::env::var(&address_key)
        .ok()
        .or_else(|| fallback.map(str::to_owned))
        .with_context(|| format!("{address_key} or E2E_RELAY_DEST is required"))?;
    ensure!(!address.trim().is_empty(), "{address_key} is empty");
    let manifest_key = format!("E2E_PERF_RELAY_{name}_MANIFEST");
    let path = std::env::var_os(&manifest_key)
        .with_context(|| format!("{manifest_key} is required for an external relay"))?;
    let bytes = std::fs::read(path).with_context(|| format!("read {manifest_key}"))?;
    ensure!(bytes.len() <= 16 * 1024, "relay policy is too large");
    validate_relay_policy(&bytes, &address)?;
    Ok(address)
}

async fn execute(config: &Config, report: &mut Report, nonce: [u8; 16]) -> Result<()> {
    let setup = Instant::now();
    // A new directory for every run prevents old queued messages or attachments
    // from satisfying this run, without deleting previous measurement data.
    let work_dir = config
        .work_dir
        .join(format!("performance-{}", report.run_id));
    std::fs::create_dir_all(&config.work_dir)?;
    std::fs::create_dir(&work_dir).context("create fresh performance profiles")?;
    let fallback = std::env::var("E2E_RELAY_DEST").ok();
    let (relay_a, relay_b) = if config.in_process_relays {
        (String::new(), String::new())
    } else {
        (
            external_relay("A", fallback.as_deref())?,
            external_relay("B", fallback.as_deref())?,
        )
    };
    let (a, mut a_events) = start_bot("bot-a", &work_dir, &relay_a, &[]).await?;
    let (b, mut b_events) = start_bot("bot-b", &work_dir, &relay_b, &[]).await?;
    let owned_relays = if config.in_process_relays {
        let (ra, rb) =
            start_in_process_relays(&a.session.node, a.card.sign_pk, b.card.sign_pk).await?;
        ensure!(
            ra.hops() == 3 && rb.hops() == 3,
            "personal relays must have three-hop tunnels"
        );
        a.session.set_relay_onion(ra.address())?;
        b.session.set_relay_onion(rb.address())?;
        Some((ra, rb))
    } else {
        None
    };
    ensure!(
        a.session.node.hops() == 3 && b.session.node.hops() == 3,
        "both clients must have three-hop tunnels"
    );
    report.participant_hops = Some([3, 3, 3, 3]);
    let relay_a = owned_relays
        .as_ref()
        .map(|r| r.0.address())
        .unwrap_or(&relay_a);
    let relay_b = owned_relays
        .as_ref()
        .map(|r| r.1.address())
        .unwrap_or(&relay_b);
    let b_in_a = a
        .session
        .add_contact_via(&b.card, &b.onion, "bot-b", Some(relay_b))
        .await?;
    let a_in_b = b
        .session
        .add_contact_via(&a.card, &a.onion, "bot-a", Some(relay_a))
        .await?;
    let budget = Duration::from_secs(config.timeout_secs);
    let result = async {
        wait_connected(&mut a_events, &mut b_events, budget).await?;
        report.warmup_rtt_ms = Some(
            warmup(
                &a,
                &b,
                b_in_a,
                a_in_b,
                &mut a_events,
                &mut b_events,
                &report.run_id,
                budget,
            )
            .await?,
        );
        report.setup_elapsed_ms = Some(millis(setup.elapsed()));
        no_local_ports("performance harness", "self")?;
        report.phase = "measurement";
        measure(
            config,
            report,
            nonce,
            &a,
            &b,
            b_in_a,
            a_in_b,
            &mut a_events,
            &mut b_events,
        )
        .await
    }
    .await;
    report.transport =
        json!({ "sender": a.session.file_metrics(), "receiver": b.session.file_metrics() });
    a.session.shutdown();
    b.session.shutdown();
    drop(owned_relays);
    result
}

async fn wait_connected(
    a: &mut Receiver<SessionEvent>,
    b: &mut Receiver<SessionEvent>,
    budget: Duration,
) -> Result<()> {
    tokio::time::timeout(budget, async {
        let (mut a_ready, mut b_ready) = (false, false);
        while !a_ready || !b_ready {
            tokio::select! {
                event = a.recv() => match event.context("bot-a event stream closed")? {
                    SessionEvent::Connected => a_ready = true,
                    SessionEvent::Disconnected => a_ready = false,
                    _ => {}
                },
                event = b.recv() => match event.context("bot-b event stream closed")? {
                    SessionEvent::Connected => b_ready = true,
                    SessionEvent::Disconnected => b_ready = false,
                    _ => {}
                }
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("timeout waiting for both relays")?
}

async fn send(
    session: &Arc<SessionManager>,
    contact: i64,
    body: String,
    deadline: Instant,
) -> Result<()> {
    tokio::time::timeout_at(
        deadline.into(),
        session.send_message(contact, body, vec![], None, None, None),
    )
    .await
    .context("message enqueue timed out")?
    .context("message enqueue failed")?;
    Ok(())
}

async fn warmup(
    a: &BotHandle,
    b: &BotHandle,
    b_in_a: i64,
    a_in_b: i64,
    a_events: &mut Receiver<SessionEvent>,
    b_events: &mut Receiver<SessionEvent>,
    run: &str,
    budget: Duration,
) -> Result<u64> {
    let body = format!("performance:{run}:warmup");
    let echo = format!("echo:{body}");
    let start = Instant::now();
    let deadline = start + budget;
    send(&a.session, b_in_a, body.clone(), deadline).await?;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline.into()) => bail!("warmup echo timed out"),
            event = a_events.recv() => if let SessionEvent::IncomingPayload { contact_id, payload, .. } = event.context("bot-a event stream closed")? {
                if contact_id == b_in_a && payload.body == echo { return Ok(millis(start.elapsed())); }
            },
            event = b_events.recv() => if let SessionEvent::IncomingPayload { contact_id, payload, .. } = event.context("bot-b event stream closed")? {
                if contact_id == a_in_b && payload.body == body { send(&b.session, a_in_b, echo.clone(), deadline).await?; }
            },
        }
    }
}

fn queue_probe(
    sender: &mut Worker,
    prefix: &str,
    probes: &mut Probes,
    deadline: Instant,
    probe_timeout: Duration,
) {
    if let Some(id) = probes.begin(Instant::now()) {
        if !sender.offer(Job {
            id: Some(id),
            body: format!("{prefix}{id}"),
            deadline: deadline.min(Instant::now() + probe_timeout),
        }) {
            probes.sent(id, false);
            eprintln!("[performance] probe {id}: bounded enqueue worker is full");
        }
    }
}

async fn measure(
    config: &Config,
    report: &mut Report,
    nonce: [u8; 16],
    a: &BotHandle,
    b: &BotHandle,
    b_in_a: i64,
    a_in_b: i64,
    a_events: &mut Receiver<SessionEvent>,
    b_events: &mut Receiver<SessionEvent>,
) -> Result<()> {
    let mut data = file_transfer::payload(config.file_bytes);
    data[..nonce.len()].copy_from_slice(&nonce);
    let data = Arc::new(data);
    let file_body = format!("performance:{}:file", report.run_id);
    let prefix = format!("performance:{}:probe:", report.run_id);
    let echo_prefix = format!("echo:{prefix}");
    report.measurement_started = true;
    let start = Instant::now();
    let deadline = start + Duration::from_secs(config.timeout_secs);
    let probe_timeout = Duration::from_secs(config.probe_timeout_secs);
    let mut probes = Probes::new(start, probe_timeout, config.max_in_flight);
    let mut sender = Worker::new(a.session.clone(), b_in_a, config.max_in_flight);
    let mut echoer = Worker::new(b.session.clone(), a_in_b, config.max_in_flight);
    let mut echoed_ids = std::collections::HashSet::new();
    let mut verifying: Option<tokio::task::JoinHandle<Result<FileReport>>> = None;
    let mut file_identity = None;
    let result = async {
        tokio::time::timeout_at(deadline.into(), a.session.send_message(b_in_a, file_body.clone(),
            vec![("performance.bin".into(), data.as_ref().clone())], None, None, None))
            .await.context("file enqueue timed out")?.context("file enqueue failed")?;
        // Queue one probe immediately, then at a fixed, bounded cadence. Missed
        // ticks never cause a burst, and no sender task is spawned per message.
        queue_probe(&mut sender, &prefix, &mut probes, deadline, probe_timeout);
        let period = Duration::from_millis(config.probe_interval_ms);
        let mut ticker = tokio::time::interval_at((Instant::now() + period).into(), period);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut expiry = tokio::time::interval(Duration::from_millis(100));
        expiry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            probes.expire(Instant::now());
            ensure!(Instant::now() < deadline, "performance phase deadline reached");
            if report.file.elapsed_ms.is_some() && probes.pending() == 0
                && sender.pending() == 0 && echoer.pending() == 0 { break; }
            tokio::select! {
                _ = tokio::time::sleep_until(deadline.into()) => bail!("performance phase deadline reached"),
                _ = expiry.tick() => {},
                verified = async { verifying.as_mut().expect("verification branch is enabled").await }, if verifying.is_some() => {
                    let mut file = verified.context("file verification worker failed")??;
                    file.duplicate_completion_events = report.file.duplicate_completion_events;
                    report.file = file;
                    verifying = None;
                },
                completion = sender.completed(), if sender.pending() > 0 => {
                    let completion = completion.context("probe enqueue worker closed")?;
                    probes.sent(completion.id.context("probe worker omitted identity")?, completion.error.is_none());
                    if let Some(error) = completion.error { eprintln!("[performance] probe enqueue: {error}"); }
                },
                completion = echoer.completed(), if echoer.pending() > 0 => {
                    let completion = completion.context("echo enqueue worker closed")?;
                    if let Some(error) = completion.error {
                        report.echo_send_errors += 1;
                        eprintln!("[performance] echo enqueue: {error}");
                    }
                },
                _ = ticker.tick(), if report.file.elapsed_ms.is_none() => {
                    queue_probe(&mut sender, &prefix, &mut probes, deadline, probe_timeout);
                },
                event = a_events.recv() => if let SessionEvent::IncomingPayload { contact_id, payload, .. } = event.context("bot-a event stream closed")? {
                    if contact_id == b_in_a {
                        if let Some(id) = payload.body.strip_prefix(&echo_prefix).and_then(|n| n.parse().ok()) {
                            probes.receive(id, Instant::now());
                        }
                    }
                },
                event = b_events.recv() => match event.context("bot-b event stream closed")? {
                    SessionEvent::IncomingPayload { contact_id, payload, .. } if contact_id == a_in_b && payload.body.starts_with(&prefix) => {
                        let id = payload.body.strip_prefix(&prefix).and_then(|s| s.parse::<usize>().ok())
                            .filter(|id| (1..=stats::MAX_PROBES).contains(id)).context("invalid performance probe")?;
                        if echoed_ids.insert(id) && !echoer.offer(Job { id: None, body: format!("echo:{}", payload.body),
                            deadline: deadline.min(Instant::now() + probe_timeout) }) {
                            report.echo_send_errors += 1;
                            eprintln!("[performance] bounded echo enqueue worker is full");
                        }
                    }
                    SessionEvent::FileReceived { message_id, attachment_id } => {
                        if let Some(identity) = file_identity {
                            ensure!(identity == (message_id, attachment_id), "a different file completed during this measurement");
                            report.file.duplicate_completion_events += 1;
                        } else {
                            file_identity = Some((message_id, attachment_id));
                            let session = b.session.clone();
                            let expected = data.clone();
                            let file_body = file_body.clone();
                            // Decryption, disk IO and the complete byte comparison
                            // must not hold up the event loop used for RTT samples.
                            verifying = Some(tokio::task::spawn_blocking(move || {
                                let message = session.db.get_message(message_id)?.context("completed file has no message")?;
                                ensure!(message.body == file_body && message.contact_id == Some(a_in_b), "unexpected file completion in fresh performance profile");
                                let attachment = session.db.get_attachment(attachment_id)?.context("completed file has no attachment")?;
                                let received = session.read_attachment(&attachment)?;
                                let mut file = FileReport { requested_bytes: expected.len(), ..Default::default() };
                                file.verify(&expected, &received, || start.elapsed())?;
                                Ok(file)
                            }));
                        }
                    }
                    SessionEvent::FileFailed { reason, .. } => bail!("file delivery failed: {reason}"),
                    _ => {}
                }
            }
        }
        ensure!(report.file.verified_unique_bytes == config.file_bytes, "file was not fully verified");
        ensure!(probes.successful() && report.echo_send_errors == 0, "probe delivery incomplete; see loss and timeout counts in JSON");
        Ok(())
    }.await;
    if let Some(task) = verifying {
        task.abort();
    }
    probes.finish(Instant::now());
    report.probes = serde_json::to_value(probes.summary())?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_or_conflicting_environment_is_rejected_even_when_set_to_zero() {
        for forbidden in FORBIDDEN_ENV
            .iter()
            .copied()
            .chain(["E2E_AGENT_BIN", "E2E_BIG_FILE_RESTART"])
        {
            assert!(
                validate_environment(|name| name == forbidden).is_err(),
                "{forbidden}"
            );
        }
        assert!(validate_environment(|_| false).is_ok());
    }

    #[test]
    fn limits_reject_invalid_values_instead_of_mislabeling_an_experiment() {
        for raw in ["0", "17", "-1", "word"] {
            assert!(bounded(Some(raw), 4, 1, 16, "inflight").is_err());
        }
        assert_eq!(bounded(None, 4, 1, 16, "inflight").unwrap(), 4);
        assert!(bounded(Some("249"), 2000, 250, 60_000, "interval").is_err());
    }

    #[test]
    fn relay_manifest_must_bind_the_actual_destination_and_both_hop_counts() {
        let valid = br#"{"destination":"test-destination","inbound_hops":3,"outbound_hops":3}"#;
        assert!(validate_relay_policy(valid, "test-destination").is_ok());
        assert!(validate_relay_policy(valid, "another-destination").is_err());
        assert!(validate_relay_policy(
            br#"{"destination":"test-destination","inbound_hops":2,"outbound_hops":3}"#,
            "test-destination"
        )
        .is_err());
        assert!(validate_relay_policy(
            br#"{"destination":"test-destination","inbound_hops":3}"#,
            "test-destination"
        )
        .is_err());
    }

    #[test]
    fn only_a_complete_matching_file_contributes_unique_goodput_once() {
        let mut report = FileReport {
            requested_bytes: 4,
            ..Default::default()
        };
        assert!(report
            .verify(b"abcd", b"abc", || Duration::from_secs(2))
            .is_err());
        assert!(report
            .verify(b"abcd", b"abce", || Duration::from_secs(2))
            .is_err());
        assert_eq!(report.verified_unique_bytes, 0);
        assert_eq!(report.goodput_bytes_per_sec, None);
        assert!(report.verify(b"abcd", b"abcd", || Duration::ZERO).is_err());
        report
            .verify(b"abcd", b"abcd", || Duration::from_secs(2))
            .unwrap();
        report
            .verify(b"abcd", b"abcd", || Duration::from_secs(3))
            .unwrap();
        assert_eq!(report.verified_unique_bytes, 4);
        assert_eq!(report.elapsed_ms, Some(2000));
        assert_eq!(report.goodput_bytes_per_sec, Some(2.0));
        assert_eq!(report.duplicate_completion_events, 1);
    }
}
