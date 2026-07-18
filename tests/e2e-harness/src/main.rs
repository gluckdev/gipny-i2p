//! End-to-end messaging harness.
//!
//! Boots two headless bot instances (A and B) against a shared local relay,
//! cross-adds them as contacts, sends N messages A→B with an attachment,
//! verifies that B echoes every message back to A, and reports latency and
//! resource metrics to stdout and to `$GITHUB_STEP_SUMMARY` when running in CI.
//!
//! # Required environment variables
//! * `E2E_RELAY_DEST` — i2p destination of the running relay (contents of
//!   `dest.pub` as printed by `gipny-relay` and `relay-testnet.yml`).
//!
//! # Optional environment variables
//! * `E2E_N_MESSAGES`   — number of messages A sends to B (default: 5).
//! * `E2E_TIMEOUT_SECS` — hard deadline for the whole test (default: 300).
//! * `E2E_WORK_DIR`     — working directory for bot data dirs (default:
//!   `/tmp/e2e-harness`).
//! * `GIPNY_I2P_BIN`    — path to the `gipny-i2p-router` binary; libcore
//!   falls back to the executable's directory and `$PATH` when not set.
//! * `GITHUB_STEP_SUMMARY` — when set (always true in GitHub Actions), the
//!   timing table is appended to this file.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use gipny_libcore::{Db, IdentityCard, SessionEvent, SessionManager, TorNode};
use tokio::sync::{Mutex, Notify};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn hex8(b: &[u8]) -> String {
    b.iter().take(8).map(|x| format!("{x:02x}")).collect()
}

// ---------------------------------------------------------------------------
// Bot startup
// ---------------------------------------------------------------------------

struct BotHandle {
    session: Arc<SessionManager>,
    card: IdentityCard,
    onion: String,
    /// Wall-clock milliseconds from `TorNode::start` call to SAM ready.
    router_ready_ms: u64,
}

async fn start_bot(
    name: &'static str,
    work_dir: &PathBuf,
    relay_dest: &str,
) -> Result<(BotHandle, tokio::sync::mpsc::Receiver<SessionEvent>)> {
    let data_dir = work_dir.join(name);
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("{name}: create data dir"))?;

    eprintln!("[e2e] {name}: starting i2p router...");
    let t0 = Instant::now();
    let node = Arc::new(
        TorNode::start(&data_dir)
            .await
            .with_context(|| format!("{name}: TorNode::start failed"))?,
    );
    let router_ready_ms = t0.elapsed().as_millis() as u64;
    eprintln!("[e2e] {name}: router ready in {router_ready_ms} ms");

    let onion = node.onion_address().to_string();
    let db_path = data_dir.join("bot.db");
    let db = Arc::new(
        Db::open_plain(&db_path)
            .with_context(|| format!("{name}: Db::open_plain failed"))?,
    );

    let (session, events) = SessionManager::start(data_dir, db, node)
        .await
        .with_context(|| format!("{name}: SessionManager::start failed"))?;

    // Persist relay destination before the relay loop first wakes up.
    session
        .set_relay_onion(relay_dest)
        .with_context(|| format!("{name}: set_relay_onion"))?;
    session
        .set_display_name(name)
        .with_context(|| format!("{name}: set_display_name"))?;

    let card = session.my_card();
    eprintln!("[e2e] {name}: identity sign_pk={}", hex8(&card.sign_pk));

    Ok((BotHandle { session, card, onion, router_ready_ms }, events))
}

// ---------------------------------------------------------------------------
// Timing summary
// ---------------------------------------------------------------------------

struct Timings {
    a_router_ms: u64,
    b_router_ms: u64,
    a_connect_ms: u64,
    b_connect_ms: u64,
    n_sent: usize,
    n_received: usize,
    rtt_min_ms: u64,
    rtt_median_ms: u64,
    rtt_max_ms: u64,
    total_ms: u64,
}

impl Timings {
    fn markdown_table(&self) -> String {
        format!(
            "| metric | value |\n\
             |---|---|\n\
             | bot-a router ready | {} ms |\n\
             | bot-b router ready | {} ms |\n\
             | bot-a relay-connect | {} ms |\n\
             | bot-b relay-connect | {} ms |\n\
             | messages sent | {} |\n\
             | echoes received | {} |\n\
             | RTT min | {} ms |\n\
             | RTT median | {} ms |\n\
             | RTT max | {} ms |\n\
             | total elapsed | {} ms |\n",
            self.a_router_ms,
            self.b_router_ms,
            self.a_connect_ms,
            self.b_connect_ms,
            self.n_sent,
            self.n_received,
            self.rtt_min_ms,
            self.rtt_median_ms,
            self.rtt_max_ms,
            self.total_ms,
        )
    }
}

fn compute_latencies(
    echoes: &[(String, Instant)],
    send_times: &HashMap<String, Instant>,
) -> Vec<u64> {
    let mut v: Vec<u64> = echoes
        .iter()
        .filter_map(|(echo_body, recv_at)| {
            let orig = echo_body.strip_prefix("echo:")?;
            let send_at = send_times.get(orig)?;
            Some(recv_at.duration_since(*send_at).as_millis() as u64)
        })
        .collect();
    v.sort_unstable();
    v
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let relay_dest = std::env::var("E2E_RELAY_DEST")
        .context("E2E_RELAY_DEST env var is required (contents of dest.pub)")?;
    let n_messages: usize = std::env::var("E2E_N_MESSAGES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let timeout_secs: u64 = std::env::var("E2E_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);
    let work_dir = PathBuf::from(
        std::env::var("E2E_WORK_DIR").unwrap_or_else(|_| "/tmp/e2e-harness".into()),
    );
    std::fs::create_dir_all(&work_dir)
        .context("create work dir")?;

    let timeout = Duration::from_secs(timeout_secs);
    let t_start = Instant::now();

    eprintln!(
        "[e2e] starting (relay={}... n={n_messages} timeout={timeout_secs}s)",
        &relay_dest[..20.min(relay_dest.len())]
    );

    // -----------------------------------------------------------------------
    // 1. Start both bots concurrently (each launches its own i2p router).
    // -----------------------------------------------------------------------
    let (a_result, b_result) = tokio::join!(
        start_bot("bot-a", &work_dir, &relay_dest),
        start_bot("bot-b", &work_dir, &relay_dest),
    );
    let (a, mut a_events) = a_result.context("bot-a start")?;
    let (b, mut b_events) = b_result.context("bot-b start")?;

    // -----------------------------------------------------------------------
    // 2. Cross-add contacts (writes to DB; relay loop will handle the rest).
    // -----------------------------------------------------------------------
    let contact_b_in_a = a
        .session
        .add_contact(&b.card, &b.onion, "bot-b")
        .await
        .context("bot-a: add_contact(bot-b)")?;
    let contact_a_in_b = b
        .session
        .add_contact(&a.card, &a.onion, "bot-a")
        .await
        .context("bot-b: add_contact(bot-a)")?;
    eprintln!(
        "[e2e] contacts added: A has B as id={contact_b_in_a}, B has A as id={contact_a_in_b}"
    );

    // -----------------------------------------------------------------------
    // 3. Spawn event loops.
    // -----------------------------------------------------------------------

    // Relay-connect timestamps (measured from here).
    let t_relay_start = Instant::now();

    let a_connect_notify = Arc::new(Notify::new());
    let a_connect_at: Arc<Mutex<Option<Instant>>> = Default::default();

    let b_connect_notify = Arc::new(Notify::new());
    let b_connect_at: Arc<Mutex<Option<Instant>>> = Default::default();

    // Echoes received by A: (echo_body, recv_instant).
    let a_echoes: Arc<Mutex<Vec<(String, Instant)>>> = Default::default();
    let a_echo_notify = Arc::new(Notify::new());

    // Send times recorded by A: body → send_instant.
    let a_send_times: Arc<Mutex<HashMap<String, Instant>>> = Default::default();

    // Bot A event loop
    {
        let connect_notify = a_connect_notify.clone();
        let connect_at = a_connect_at.clone();
        let echoes = a_echoes.clone();
        let echo_notify = a_echo_notify.clone();
        tokio::spawn(async move {
            while let Some(ev) = a_events.recv().await {
                match ev {
                    SessionEvent::Connected => {
                        let mut g = connect_at.lock().await;
                        if g.is_none() {
                            *g = Some(Instant::now());
                            connect_notify.notify_one();
                        }
                        eprintln!("[e2e] bot-a: relay connected");
                    }
                    SessionEvent::Disconnected => {
                        eprintln!("[e2e] bot-a: relay disconnected");
                    }
                    SessionEvent::IncomingPayload { payload, .. } => {
                        eprintln!("[e2e] bot-a: received '{}'", payload.body);
                        echoes.lock().await.push((payload.body, Instant::now()));
                        echo_notify.notify_one();
                    }
                    _ => {}
                }
            }
        });
    }

    // Bot B event loop (echo bot — sends back every non-empty message it receives)
    {
        let connect_notify = b_connect_notify.clone();
        let connect_at = b_connect_at.clone();
        let b_session = b.session.clone();
        tokio::spawn(async move {
            while let Some(ev) = b_events.recv().await {
                match ev {
                    SessionEvent::Connected => {
                        let mut g = connect_at.lock().await;
                        if g.is_none() {
                            *g = Some(Instant::now());
                            connect_notify.notify_one();
                        }
                        eprintln!("[e2e] bot-b: relay connected");
                    }
                    SessionEvent::Disconnected => {
                        eprintln!("[e2e] bot-b: relay disconnected");
                    }
                    SessionEvent::IncomingPayload { contact_id, payload, .. } => {
                        if payload.body.is_empty() {
                            continue;
                        }
                        let echo_body = format!("echo:{}", payload.body);
                        eprintln!(
                            "[e2e] bot-b: received '{}', echoing '{echo_body}'",
                            payload.body
                        );
                        if let Err(e) = b_session
                            .send_message(contact_id, echo_body, vec![], None, None, None)
                            .await
                        {
                            eprintln!("[e2e] bot-b: echo send error: {e}");
                        }
                    }
                    _ => {}
                }
            }
        });
    }

    // -----------------------------------------------------------------------
    // 4. Wait for both bots to establish relay connections.
    // -----------------------------------------------------------------------
    let remaining = timeout.saturating_sub(t_start.elapsed());
    eprintln!("[e2e] waiting for relay connections (budget: {}s)...", remaining.as_secs());

    let a_connected_fut = {
        let notify = a_connect_notify.clone();
        let at = a_connect_at.clone();
        async move {
            notify.notified().await;
            at.lock().await.expect("bot-a relay connection timestamp should be set before notify fires")
        }
    };
    let b_connected_fut = {
        let notify = b_connect_notify.clone();
        let at = b_connect_at.clone();
        async move {
            notify.notified().await;
            at.lock().await.expect("bot-b relay connection timestamp should be set before notify fires")
        }
    };

    let (a_conn_instant, b_conn_instant) = tokio::time::timeout(
        remaining,
        async { tokio::join!(a_connected_fut, b_connected_fut) },
    )
    .await
    .context("timeout: both bots must connect to relay before sending")?;

    let a_connect_ms = a_conn_instant.duration_since(t_relay_start).as_millis() as u64;
    let b_connect_ms = b_conn_instant.duration_since(t_relay_start).as_millis() as u64;
    eprintln!(
        "[e2e] bot-a relay-connect: {a_connect_ms} ms | bot-b relay-connect: {b_connect_ms} ms"
    );

    // -----------------------------------------------------------------------
    // 5. A sends N messages to B (first message includes an attachment).
    // -----------------------------------------------------------------------
    eprintln!("[e2e] sending {n_messages} messages A→B...");
    {
        let mut send_times = a_send_times.lock().await;
        for i in 1..=n_messages {
            let body = format!("msg-{i:04}");
            let attachments: Vec<(String, Vec<u8>)> = if i == 1 {
                vec![("e2e-probe.txt".into(), format!("e2e attachment #{i}").into_bytes())]
            } else {
                vec![]
            };
            let t_send = Instant::now();
            a.session
                .send_message(
                    contact_b_in_a,
                    body.clone(),
                    attachments,
                    None,
                    None,
                    None,
                )
                .await
                .with_context(|| format!("send msg-{i:04}"))?;
            send_times.insert(body.clone(), t_send);
            eprintln!("[e2e] bot-a: sent '{body}'");
        }
    }

    // -----------------------------------------------------------------------
    // 6. Wait for all N echoes to arrive at A.
    // -----------------------------------------------------------------------
    let remaining = timeout.saturating_sub(t_start.elapsed());
    eprintln!(
        "[e2e] waiting for {n_messages} echoes (budget: {}s)...",
        remaining.as_secs()
    );

    {
        let echoes_ref = a_echoes.clone();
        let echo_notify_ref = a_echo_notify.clone();
        tokio::time::timeout(remaining, async move {
            loop {
                if echoes_ref.lock().await.len() >= n_messages {
                    return;
                }
                echo_notify_ref.notified().await;
            }
        })
        .await
        .context("timeout waiting for echo replies from bot-b")?;
    }

    // -----------------------------------------------------------------------
    // 7. Compute and report timings.
    // -----------------------------------------------------------------------
    let echoes = a_echoes.lock().await.clone();
    let send_times = a_send_times.lock().await.clone();
    let latencies = compute_latencies(&echoes, &send_times);

    let (rtt_min, rtt_median, rtt_max) = if latencies.is_empty() {
        (0, 0, 0)
    } else {
        let min = latencies[0];
        let max = latencies[latencies.len() - 1];
        let median = latencies[latencies.len() / 2];
        (min, median, max)
    };

    let total_ms = t_start.elapsed().as_millis() as u64;

    eprintln!(
        "[e2e] RTT latency — min: {rtt_min} ms  median: {rtt_median} ms  max: {rtt_max} ms"
    );
    eprintln!(
        "[e2e] echoes received: {}/{n_messages}  total elapsed: {total_ms} ms",
        echoes.len()
    );

    let timings = Timings {
        a_router_ms: a.router_ready_ms,
        b_router_ms: b.router_ready_ms,
        a_connect_ms,
        b_connect_ms,
        n_sent: n_messages,
        n_received: echoes.len(),
        rtt_min_ms: rtt_min,
        rtt_median_ms: rtt_median,
        rtt_max_ms: rtt_max,
        total_ms,
    };

    // Print machine-parseable timing block for CI log scraping.
    println!("[e2e-timing]\n{}", timings.markdown_table());

    // Append to GitHub Actions step summary when running in CI (best-effort).
    if let Ok(summary_path) = std::env::var("GITHUB_STEP_SUMMARY") {
        let outcome = if echoes.len() >= n_messages { "✅ PASS" } else { "❌ FAIL" };
        let content = format!(
            "### e2e messaging test — {outcome}\n\n\
             **Messages sent:** {n_messages}  |  **Echoes received:** {}\n\n\
             {}\n",
            echoes.len(),
            timings.markdown_table(),
        );
        use std::io::Write;
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&summary_path)
            .and_then(|mut f| f.write_all(content.as_bytes()))
            .map_err(|e| eprintln!("[e2e] warning: could not write GITHUB_STEP_SUMMARY: {e}"));
    }

    // -----------------------------------------------------------------------
    // 8. Assert and exit.
    // -----------------------------------------------------------------------
    a.session.shutdown();
    b.session.shutdown();

    if echoes.len() < n_messages {
        bail!(
            "delivery assertion failed: received {}/{n_messages} echoes",
            echoes.len()
        );
    }

    eprintln!("[e2e] SUCCESS — all {n_messages} messages delivered and echoed");
    Ok(())
}
