//! End-to-end messaging harness.
//!
//! Boots two headless bot instances (A and B) on the i2p router inside this
//! process using their own personal terminals,
//! cross-adds them as contacts, sends N messages A→B with an attachment,
//! verifies that B echoes every message back to A, and reports latency and
//! resource metrics to stdout and to `$GITHUB_STEP_SUMMARY` when running in CI.
//!
//! # Optional environment variables
//! * `E2E_AGENT_BIN=<path>` — a different test: bot-a is the master and the
//!   far side is the real `gipny-agent` binary at that path, started with
//!   bot-a's v3 card, on its own router in its own process. Both peers host a personal terminal.
//!   `E2E_N_MESSAGES` is the number of commands.
//! * `E2E_OFFLINE_RETURN=1` — verify the sender retains its outbox while
//!   the peers are online at disjoint times and delivers once both return.
//! * `E2E_BOTH_FIRST=1` — bot-b writes to bot-a at the same moment bot-a
//!   writes to bot-b: two sessions are opened at once and their X3dhInits
//!   cross (session.rs `ours_stands`). Its letter must arrive too.
//! * `E2E_BIG_FILE=<bytes>` — after the echoes, bot-a sends bot-b a file of
//!   that size; it goes in parts (libcore::files) and must arrive whole and
//!   equal. The throughput is printed.
//! * `E2E_BIG_FILE_RESTART=1` — with E2E_BIG_FILE: bot-b stops once a third
//!   of the parts are in and starts again on the same data, and the file
//!   must still arrive whole — the transfer picks up where it was.
//! * `E2E_PERFORMANCE=1` — separate controlled file/echo benchmark, writing
//!   JSON to `E2E_PERF_REPORT`. Three-hop participants and bounded probes;
//!   see README.md for transport controls and metric definitions.
//! * `E2E_UPDATE_CHECK=1` — a different test: one router, and the updater
//!   asks GitHub for the latest release through the i2p outproxy and fetches
//!   its smallest file, as an installed app would. While GitHub has only
//!   prereleases, it stops at GitHub's answer "no stable release", checks the
//!   updater reports exactly that, and fails: there is no file to fetch.
//! * `E2E_N_MESSAGES`   — number of messages A sends to B (default: 5).
//! * `E2E_TIMEOUT_SECS` — hard deadline for the whole test (default: 300).
//! * `E2E_WORK_DIR`     — working directory for bot data dirs (default:
//!   `/tmp/e2e-harness`).
//! * `GITHUB_STEP_SUMMARY` — when set (always true in GitHub Actions), the
//!   timing table is appended to this file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use gipny_libcore::agent::BODY_OFF;
use gipny_libcore::{
    ContactCard, Db, IdentityCard, SessionEvent, SessionManager, I2pNode, WireConsole,
    CONSOLE_COMMAND, CONSOLE_GRANT, CONSOLE_OFF, CONSOLE_OUTPUT, CONSOLE_REVOKE,
};
use tokio::sync::{Mutex, Notify};

mod file_transfer;
mod performance;
mod ports;

/// Longest a router may take to be ready: reseed, then tunnels. Minutes at
/// worst on a runner; past this something is stuck, and the run says so.
const ROUTER_START: Duration = Duration::from_secs(600);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The run fails if `pid` (`"self"` for the harness, whose routers run in
/// it) listens on loopback: nothing local may reach the router. Linux only.
fn no_local_ports(who: &str, pid: &str) -> Result<()> {
    if !cfg!(target_os = "linux") {
        return Ok(());
    }
    let listeners = ports::observed_tcp_listeners(pid).with_context(|| format!("{who}: inspect /proc/{pid}"))?;
    for l in &listeners {
        eprintln!("[e2e] {who} listens on {}:{}{}", l.addr, l.port, if l.loopback { " (LOOPBACK)" } else { "" });
    }
    ports::reject_loopback(&listeners).with_context(|| format!("{who}: loopback policy"))?;
    eprintln!("[e2e] {who}: no local ports");
    Ok(())
}

fn hex8(b: &[u8]) -> String {
    b.iter().take(8).map(|x| format!("{x:02x}")).collect()
}

// ---------------------------------------------------------------------------
// Bot startup
// ---------------------------------------------------------------------------

struct BotHandle {
    session: Arc<SessionManager>,
    card: IdentityCard,
    i2p_address: String,
    /// Wall-clock milliseconds from the `I2pNode::start` call to its tunnels.
    router_ready_ms: u64,
}

async fn start_bot(
    name: &'static str,
    work_dir: &Path,
) -> Result<(BotHandle, tokio::sync::mpsc::Receiver<SessionEvent>)> {
    let data_dir = work_dir.join(name);
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("{name}: create data dir"))?;

    eprintln!("[e2e] {name}: starting i2p router...");
    let t0 = Instant::now();
    // Bounded: a reseed server that never answers left a run waiting on
    // the router for 36 minutes without a word (the laptop, 2026-09-25).
    let node = Arc::new(
        tokio::time::timeout(ROUTER_START, I2pNode::start(&data_dir, Default::default()))
            .await
            .map_err(|_| anyhow::anyhow!("{name}: the router was not ready in {ROUTER_START:?}"))?
            .with_context(|| format!("{name}: I2pNode::start failed"))?,
    );
    let router_ready_ms = t0.elapsed().as_millis() as u64;
    eprintln!("[e2e] {name}: router ready in {router_ready_ms} ms");

    let db_path = data_dir.join("bot.db");
    let db = Arc::new(
        Db::open_plain(&db_path)
            .with_context(|| format!("{name}: Db::open_plain failed"))?,
    );
    let (session, events) = SessionManager::start(data_dir, db, node)
        .await
        .with_context(|| format!("{name}: SessionManager::start failed"))?;

    let i2p_address = session.terminal_address();
    session
        .set_display_name(name)
        .with_context(|| format!("{name}: set_display_name"))?;

    let card = session.my_card();
    eprintln!("[e2e] {name}: identity sign_pk={}", hex8(&card.sign_pk));

    Ok((BotHandle { session, card, i2p_address, router_ready_ms }, events))
}

/// What bot-b writes first with E2E_BOTH_FIRST.
const GREETING: &str = "hello from bot-b";
/// The text of the letter carrying E2E_BIG_FILE; not echoed.
const BIG_FILE_BODY: &str = "big file";

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
// Agent mode: the real gipny-agent binary as the far side
// ---------------------------------------------------------------------------

/// Everything bot-a, as the master, has seen from the agent so far.
#[derive(Default)]
struct Seen {
    connected_at: Option<Instant>,
    /// The agent's contact id on bot-a — created by bot-a itself from the
    /// agent's first message — and when its GRANT arrived.
    agent: Option<(i64, Instant)>,
    /// (body, exit code, arrival) of every CONSOLE_OUTPUT.
    outputs: Vec<(String, Option<i32>, Instant)>,
    revoked_at: Option<Instant>,
}

/// Waits until `f` yields on the shared state, or `budget` runs out.
async fn wait_for<T, F>(seen: &Arc<Mutex<Seen>>, notify: &Arc<Notify>, budget: Duration, f: F) -> Result<T>
where
    F: Fn(&Seen) -> Option<T>,
{
    tokio::time::timeout(budget, async {
        let notified = notify.notified();
        tokio::pin!(notified);
        loop {
            if let Some(v) = f(&*seen.lock().await) {
                return v;
            }
            notified.as_mut().enable();
            if let Some(v) = f(&*seen.lock().await) {
                return v;
            }
            notified.as_mut().await;
            notified.set(notify.notified());
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timeout after {}s", budget.as_secs()))
}

/// bot-a is the master; the far side is `agent_bin`, run as the separate
/// process it is on a server, with its own router. Proves, over
/// live i2p: the agent's GRANT creates the contact on the master by itself;
/// commands run one at a time in arrival order; a file sent with a command is
/// there when the command runs; OFF is answered with REVOKE and exit 0.
async fn run_agent_mode(agent_bin: PathBuf) -> Result<()> {
    let n_commands: usize = std::env::var("E2E_N_MESSAGES").ok().and_then(|s| s.parse().ok()).unwrap_or(5);
    let timeout_secs: u64 = std::env::var("E2E_TIMEOUT_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(600);
    let work_dir = PathBuf::from(std::env::var("E2E_WORK_DIR").unwrap_or_else(|_| "/tmp/e2e-harness".into()));
    std::fs::create_dir_all(&work_dir).context("create work dir")?;
    let timeout = Duration::from_secs(timeout_secs);
    let t_start = Instant::now();
    let budget = |t_start: Instant| timeout.saturating_sub(t_start.elapsed());
    eprintln!("[e2e] agent mode: {} · {n_commands} commands · timeout {timeout_secs}s", agent_bin.display());

    let terminal_start = Instant::now();
    let (a, mut a_events) = start_bot("bot-a", &work_dir).await.context("master terminal startup")?;
    let terminal_ms = terminal_start.elapsed().as_millis();
    let master_card = ContactCard {
        i2p_address: a.i2p_address.clone(),
        sign_pk: a.card.sign_pk,
        dh_pk: a.card.dh_pk,
        relay: None,
        name: Some("bot-a".into()),
    }
    .encode()?;

    // The agent's data dir is fresh, so this is a first run: --master given,
    // remembered in master.card. Its stdout/stderr are inherited so its own
    // "[agent]" lines land in the job log next to ours.
    let agent_data = work_dir.join("agent");
    let agent_cwd = work_dir.join("agent-cwd");
    std::fs::create_dir_all(&agent_cwd).context("create agent cwd")?;
    eprintln!("[e2e] starting {}...", agent_bin.display());
    let mut child = tokio::process::Command::new(&agent_bin)
        .arg("--data").arg(&agent_data)
        .arg("--master").arg(&master_card)
        .arg("--name").arg("e2e-agent")
        .arg("--timeout").arg("30")
        .arg("--cwd").arg(&agent_cwd)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("spawn {}", agent_bin.display()))?;

    let seen: Arc<Mutex<Seen>> = Default::default();
    let notify = Arc::new(Notify::new());
    {
        let seen = seen.clone();
        let notify = notify.clone();
        tokio::spawn(async move {
            while let Some(ev) = a_events.recv().await {
                match ev {
                    SessionEvent::Connected => {
                        eprintln!("[e2e] bot-a: relay connected");
                        let mut s = seen.lock().await;
                        if s.connected_at.is_none() {
                            s.connected_at = Some(Instant::now());
                        }
                    }
                    SessionEvent::Disconnected => eprintln!("[e2e] bot-a: relay disconnected"),
                    SessionEvent::IncomingPayload { contact_id, payload, .. } => {
                        let Some(c) = payload.console.as_ref() else {
                            eprintln!("[e2e] bot-a: plain message {:?}", payload.body);
                            continue;
                        };
                        let mut s = seen.lock().await;
                        match c.kind {
                            CONSOLE_GRANT => {
                                eprintln!("[e2e] bot-a: console granted by contact {contact_id}");
                                s.agent = Some((contact_id, Instant::now()));
                            }
                            CONSOLE_OUTPUT => {
                                eprintln!("[e2e] bot-a: output exit={:?} {:?}", c.exit_code, payload.body);
                                s.outputs.push((payload.body.clone(), c.exit_code, Instant::now()));
                            }
                            CONSOLE_REVOKE => {
                                eprintln!("[e2e] bot-a: console revoked");
                                s.revoked_at = Some(Instant::now());
                            }
                            other => eprintln!("[e2e] bot-a: console kind {other} {:?}", payload.body),
                        }
                    }
                    _ => {}
                }
                notify.notify_one();
            }
        });
    }

    // 1. The agent writes first. bot-a never adds it: the contact must appear
    //    from the agent's X3dhInit, and GRANT must ride on that first message.
    eprintln!("[e2e] waiting for the agent's GRANT (budget: {}s)...", budget(t_start).as_secs());
    let (agent_cid, granted_at) = wait_for(&seen, &notify, budget(t_start), |s| s.agent)
        .await
        .context("waiting for the agent's GRANT")?;
    let grant_ms = granted_at.duration_since(t_start).as_millis() as u64;
    eprintln!("[e2e] GRANT after {grant_ms} ms; the agent is contact {agent_cid} on bot-a");

    // 2. Commands. Each one writes `start-k`, sleeps a second, writes `end-k`
    //    and echoes its tag. A final `cat` of that file then shows whether the
    //    agent ran them one at a time: sequential execution leaves every
    //    `start-k` immediately followed by its own `end-k`; concurrent
    //    execution interleaves starts. Messages sent milliseconds apart may
    //    reach the agent in any order over the relay, so send order is not
    //    asserted — only that each command ran, once, and none overlapped.
    //    One command carries a file and runs it: the upload must be saved
    //    before the shell starts.
    let mut send_times: HashMap<String, Instant> = HashMap::new();
    for i in 1..=n_commands {
        let tag = format!("hello-{i}");
        let body = format!("echo start-{i} >> e2e-order.txt; sleep 1; echo end-{i} >> e2e-order.txt; echo {tag}");
        a.session
            .send_console(agent_cid, body, WireConsole::new(CONSOLE_COMMAND), vec![])
            .await
            .with_context(|| format!("send command {tag}"))?;
        send_times.insert(tag, Instant::now());
    }
    a.session
        .send_console(
            agent_cid,
            "sh e2e-script.sh".into(),
            WireConsole::new(CONSOLE_COMMAND),
            vec![("e2e-script.sh".into(), b"echo from-script".to_vec())],
        )
        .await
        .context("send script command")?;
    a.session
        .send_console(agent_cid, "cat e2e-order.txt".into(), WireConsole::new(CONSOLE_COMMAND), vec![])
        .await
        .context("send cat command")?;
    let expected = n_commands + 2;
    eprintln!("[e2e] sent {expected} commands, waiting for their outputs (budget: {}s)...", budget(t_start).as_secs());

    // 3. Outputs.
    let outputs = wait_for(&seen, &notify, budget(t_start), |s| {
        (s.outputs.len() >= expected).then(|| s.outputs.clone())
    })
    .await
    .context("waiting for command outputs")?;

    // 4. Check them.
    let mut failures: Vec<String> = Vec::new();
    let mut latencies: Vec<u64> = Vec::new();
    for i in 1..=n_commands {
        let tag = format!("hello-{i}");
        match outputs.iter().find(|(body, _, _)| body.trim() == tag) {
            Some((_, code, at)) => {
                if *code != Some(0) {
                    failures.push(format!("{tag}: exit code {code:?}, expected 0"));
                }
                latencies.push(at.duration_since(send_times[&tag]).as_millis() as u64);
            }
            None => failures.push(format!("{tag}: no output")),
        }
    }
    match outputs.iter().find(|(body, _, _)| body.contains("from-script")) {
        Some((body, code, _)) => {
            if *code != Some(0) || !body.contains("saved") {
                failures.push(format!("script: exit {code:?}, body {body:?}"));
            }
        }
        None => failures.push("script: its output never came back (upload not saved before the command ran?)".into()),
    }
    // The `cat`: 2N lines, start/end pairs, each tag exactly once.
    match outputs.iter().find(|(body, _, _)| body.starts_with("start-") && body.lines().count() == 2 * n_commands) {
        Some((body, _, _)) => {
            let lines: Vec<&str> = body.lines().collect();
            let mut seen: Vec<usize> = Vec::new();
            for pair in lines.chunks(2) {
                let k = pair[0].strip_prefix("start-").and_then(|k| k.parse::<usize>().ok());
                match (k, pair.get(1)) {
                    (Some(k), Some(end)) if *end == format!("end-{k}") => seen.push(k),
                    _ => { failures.push(format!("sequential: commands overlapped or were cut: {body:?}")); break; }
                }
            }
            seen.sort_unstable();
            if seen != (1..=n_commands).collect::<Vec<_>>() {
                failures.push(format!("sequential: not every command ran exactly once: {seen:?} in {body:?}"));
            }
            eprintln!("[e2e] the agent ran the commands one at a time, in this order: {}", lines.iter().step_by(2).map(|l| l.trim_start_matches("start-")).collect::<Vec<_>>().join(","));
        }
        None => failures.push(format!(
            "sequential: no `cat e2e-order.txt` output with {} lines; multi-line outputs: {:?}",
            2 * n_commands,
            outputs.iter().filter(|(b, _, _)| b.lines().count() > 1).map(|(b, _, _)| b).collect::<Vec<_>>(),
        )),
    }
    latencies.sort_unstable();
    let (rtt_min, rtt_median, rtt_max) = if latencies.is_empty() {
        (0, 0, 0)
    } else {
        (latencies[0], latencies[latencies.len() / 2], latencies[latencies.len() - 1])
    };
    eprintln!("[e2e] command RTT — min: {rtt_min} ms  median: {rtt_median} ms  max: {rtt_max} ms");

    // While both still run: neither listens on loopback.
    if let Err(e) = no_local_ports("harness", "self") { failures.push(e.to_string()); }
    match child.try_wait().context("poll agent before listener observation")? {
        Some(status) => failures.push(format!("the agent exited before OFF: {status}")),
        None => {
            let pid = child.id().context("live agent has no PID")?;
            if let Err(error) = no_local_ports("agent", &pid.to_string()) { failures.push(format!("{error:#}")); }
            if let Some(status) = child.try_wait().context("poll agent after listener observation")? {
                failures.push(format!("the agent exited during listener observation: {status}"));
            }
        }
    }

    // 5. OFF: the agent answers REVOKE and exits 0 once that is delivered.
    a.session
        .send_console(agent_cid, BODY_OFF.into(), WireConsole::new(CONSOLE_OFF), vec![])
        .await
        .context("send OFF")?;
    let t_off = Instant::now();
    eprintln!("[e2e] sent OFF, waiting for REVOKE and the agent's exit (budget: {}s)...", budget(t_start).as_secs());
    let status = tokio::time::timeout(budget(t_start), child.wait())
        .await
        .context("timeout: the agent did not exit after OFF")?
        .context("wait for the agent")?;
    let exit_ms = t_off.elapsed().as_millis() as u64;
    eprintln!("[e2e] agent exited with {status} after {exit_ms} ms");
    if !status.success() {
        failures.push(format!("agent exit status {status}, expected 0"));
    }
    // The agent waits for the REVOKE's delivery ack before exiting, and the
    // ack follows bot-a's event; a short wait covers the gap.
    if wait_for(&seen, &notify, Duration::from_secs(15), |s| s.revoked_at).await.is_err() {
        failures.push("REVOKE never arrived at the master".into());
    }

    // 6. Report.
    let total_ms = t_start.elapsed().as_millis() as u64;
    let table = format!(
        "| metric | value |\n\
         |---|---|\n\
         | master session and terminal ready | {terminal_ms} ms |\n\
         | bot-a router ready | {} ms |\n\
         | agent GRANT received | {grant_ms} ms |\n\
         | commands sent | {expected} |\n\
         | outputs received | {} |\n\
         | command RTT min | {rtt_min} ms |\n\
         | command RTT median | {rtt_median} ms |\n\
         | command RTT max | {rtt_max} ms |\n\
         | OFF → agent exit | {exit_ms} ms |\n\
         | total elapsed | {total_ms} ms |\n",
        a.router_ready_ms,
        outputs.len(),
    );
    println!("[e2e-timing]\n{table}");
    if let Ok(summary_path) = std::env::var("GITHUB_STEP_SUMMARY") {
        let outcome = if failures.is_empty() { "✅ PASS" } else { "❌ FAIL" };
        let content = format!(
            "### e2e agent binary — {outcome}\n\n{table}\n{}\n",
            if failures.is_empty() { String::new() } else { format!("```\n{}\n```", failures.join("\n")) }
        );
        use std::io::Write;
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&summary_path)
            .and_then(|mut f| f.write_all(content.as_bytes()))
            .map_err(|e| eprintln!("[e2e] warning: could not write GITHUB_STEP_SUMMARY: {e}"));
    }
    a.session.shutdown();
    if !failures.is_empty() {
        bail!("agent e2e failed:\n  {}", failures.join("\n  "));
    }
    eprintln!("[e2e] SUCCESS — the agent ran {n_commands} commands one at a time, ran the uploaded script, and left on OFF");
    Ok(())
}

// ---------------------------------------------------------------------------
// Offline delivery across two session restarts.
// ---------------------------------------------------------------------------

// This scenario reads receipts from durable history, while the regular session
// workers still need a live consumer for their bounded event channel.
async fn start_offline_bot(name: &'static str, dir: &Path) -> Result<BotHandle> {
    let (bot, mut events) = start_bot(name, dir).await?;
    while !matches!(
        events
            .recv()
            .await
            .context("session closed before local terminal connected")?,
        SessionEvent::Connected
    ) {}
    tokio::spawn(async move { while events.recv().await.is_some() {} });
    Ok(bot)
}

async fn stop_offline_bot(bot: BotHandle) -> Result<()> {
    let session = Arc::downgrade(&bot.session);
    bot.session.shutdown_and_wait().await;
    drop(bot);
    tokio::time::timeout(Duration::from_secs(10), async {
        while session.strong_count() != 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .context("stopped session still owns live background work")?;
    Ok(())
}

async fn run_offline_return() -> Result<()> {
    let count: usize = std::env::var("E2E_N_MESSAGES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    anyhow::ensure!(
        (1..=100).contains(&count),
        "offline scenario requires 1..100 messages"
    );
    let timeout_secs = std::env::var("E2E_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1200);
    let root =
        PathBuf::from(std::env::var("E2E_WORK_DIR").unwrap_or_else(|_| "/tmp/e2e-harness".into()));
    std::fs::create_dir_all(&root)?;
    // Preserve previous evidence and prevent an earlier success from satisfying this run.
    let dir = tempfile::Builder::new()
        .prefix("offline-")
        .tempdir_in(&root)?
        .keep();
    tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        check_offline_return(&dir, count),
    )
    .await
    .context("offline terminal return exceeded its deadline")?
}

async fn check_offline_return(dir: &Path, count: usize) -> Result<()> {
    let a = start_offline_bot("bot-a", dir).await?;
    let b = start_offline_bot("bot-b", dir).await?;
    let to_b = a
        .session
        .add_contact(&b.card, &b.i2p_address, "bot-b")
        .await?;
    let from_a = b
        .session
        .add_contact(&a.card, &a.i2p_address, "bot-a")
        .await?;
    let (address_a, address_b) = (a.i2p_address.clone(), b.i2p_address.clone());
    anyhow::ensure!(
        address_a != address_b,
        "two profiles share a personal terminal"
    );
    stop_offline_bot(b).await?;
    eprintln!("[e2e] recipient offline; queuing {count} letters");

    let mut ids = Vec::new();
    for index in 0..count {
        ids.push(
            a.session
                .send_message(to_b, format!("offline-{index}"), vec![], None, None, None)
                .await?,
        );
    }
    stop_offline_bot(a).await?;
    {
        let saved = Db::open_plain(&dir.join("bot-a/bot.db"))?;
        for id in &ids {
            let letter = saved
                .get_message(*id)?
                .context("sender restart lost a queued letter")?;
            anyhow::ensure!(
                !letter.sent && !letter.delivered && letter.delivery_expired_at.is_none(),
                "offline letter lost its pending status"
            );
        }
    }
    eprintln!("[e2e] sender queue persisted; recipient returns alone");
    let b = start_offline_bot("bot-b", dir).await?;
    anyhow::ensure!(
        b.i2p_address == address_b,
        "recipient terminal address changed on restart"
    );
    anyhow::ensure!(
        b.session
            .db
            .list_messages(from_a, 200, None)?
            .iter()
            .all(|m| !m.body.starts_with("offline-")),
        "recipient already has mail while the sender is offline"
    );
    eprintln!("[e2e] recipient authenticated with empty history; sender returns");
    let a = start_offline_bot("bot-a", dir).await?;
    anyhow::ensure!(
        a.i2p_address == address_a,
        "sender terminal address changed on restart"
    );
    let mut last_progress = None;
    let incoming = loop {
        let mut deposited = 0;
        let mut acknowledged = 0;
        for &id in &ids {
            let letter = a.session.db.get_message(id)?.context("queued letter disappeared")?;
            deposited += usize::from(letter.sent);
            acknowledged += usize::from(letter.delivered);
        }
        let received = b.session.db.list_messages(from_a, 200, None)?;
        let delivered = received.iter().filter(|row| row.body.starts_with("offline-")).count();
        let progress = (deposited, delivered, acknowledged);
        if last_progress != Some(progress) {
            eprintln!("[e2e] peer return: {deposited}/{count} deposited, {delivered} in recipient history, {acknowledged} peer ACKs");
            last_progress = Some(progress);
        }
        if acknowledged == count {
            break received;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    };
    anyhow::ensure!(
        incoming
            .iter()
            .filter(|m| m.body.starts_with("offline-"))
            .count()
            == count,
        "recipient history has missing or duplicate letters"
    );
    for index in 0..count {
        let body = format!("offline-{index}");
        anyhow::ensure!(
            incoming.iter().filter(|m| m.body == body).count() == 1,
            "invalid recipient history for {body}"
        );
    }
    no_local_ports("harness", "self")?;
    a.session.shutdown_and_wait().await;
    b.session.shutdown_and_wait().await;
    eprintln!("[e2e] SUCCESS — {count} offline letters survived restart, arrived once and received a peer ACK");
    Ok(())
}

/// The updater over the outproxy, end to end: the release list from the
/// GitHub API, then a file from it (a redirect to GitHub's storage host).
async fn run_update_check_mode() -> Result<()> {
    use gipny_libcore::update::{CheckOutcome, Component, UpdateError, Updater};
    let work_dir = PathBuf::from(std::env::var("E2E_WORK_DIR").unwrap_or_else(|_| "/tmp/gipny-e2e".into()));
    let timeout = Duration::from_secs(std::env::var("E2E_TIMEOUT_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(600));
    let data_dir = work_dir.join("updater");
    std::fs::create_dir_all(&data_dir)?;
    let t0 = Instant::now();
    let node = Arc::new(
        tokio::time::timeout(ROUTER_START, I2pNode::start(&data_dir, Default::default()))
            .await
            .map_err(|_| anyhow::anyhow!("the router was not ready in {ROUTER_START:?}"))?
            .context("I2pNode::start")?,
    );
    eprintln!("[e2e] router ready in {} ms", t0.elapsed().as_millis());
    no_local_ports("harness", "self")?;
    let updater = Updater::new(node.clone(), Component::App);

    // The outproxy's LeaseSet may not be found at the first try. GitHub
    // saying it has no stable release is an answer, not something to ask
    // again every 15 s from a shared exit with 60 API requests an hour.
    let t1 = Instant::now();
    let release = loop {
        match updater.latest_release().await {
            Ok(r) => break r,
            Err(UpdateError::NoStableRelease) => {
                eprintln!("[e2e] latest release: none stable yet, GitHub answered in {} ms", t1.elapsed().as_millis());
                match updater.check_detailed("0.0.0").await {
                    Ok(CheckOutcome::NoStableRelease) => eprintln!("[e2e] check from 0.0.0: NoStableRelease"),
                    other => bail!("check_detailed with no stable release: {other:?}"),
                }
                bail!("no stable release yet: GitHub answered through the outproxy, but there is no file to download until one is published");
            }
            Err(e) if t0.elapsed() < timeout => {
                eprintln!("[e2e] latest release: {e}; again in 15 s");
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
            Err(e) => bail!("no release list through the outproxy in {timeout:?}: {e}"),
        }
    };
    eprintln!("[e2e] latest release {} with {} files in {} ms", release.version, release.assets.len(), t1.elapsed().as_millis());
    match updater.check_detailed("0.0.0").await {
        Ok(CheckOutcome::Update(u)) => eprintln!("[e2e] an update from 0.0.0 would be {}", u.version),
        Ok(other) => eprintln!("[e2e] check from 0.0.0: {other:?}"),
        Err(e) => bail!("check_detailed: {e}"),
    }
    let Some(asset) = release.assets.iter().filter(|a| a.size > 0 && a.name != "SHA256SUMS.txt").min_by_key(|a| a.size) else {
        bail!("the latest release has no artifacts with a release checksum");
    };
    let t2 = Instant::now();
    let dest = data_dir.join("download").join(&asset.name);
    let sha256 = updater.asset_sha256(&release, &asset.name).await.context("release checksum")?;
    updater.download_asset_to(asset, &sha256, &dest, |_, _| {}).await.with_context(|| format!("download {}", asset.name))?;
    let got = std::fs::metadata(&dest)?.len();
    if got != asset.size {
        bail!("{}: {got} bytes, the release says {}", asset.name, asset.size);
    }
    eprintln!("[e2e] downloaded {} ({got} bytes) in {} ms", asset.name, t2.elapsed().as_millis());
    node.shutdown().await;
    eprintln!("[e2e] SUCCESS — the updater reached GitHub through the outproxy");
    Ok(())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    // This metadata-only branch must run before Tokio, profiles or libi2pd.
    // It lets CI inspect its non-dumpable child without relaxing hardening.
    if ports::run_observer_mode()? { return Ok(()); }
    gipny_libcore::security::harden_process()?;
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(run())
}

async fn run() -> Result<()> {
    if std::env::var("E2E_PERFORMANCE").is_ok_and(|v| v == "1") {
        return performance::run().await;
    }
    if let Some(bin) = std::env::var_os("E2E_AGENT_BIN") {
        return run_agent_mode(PathBuf::from(bin)).await;
    }
    if std::env::var("E2E_OFFLINE_RETURN").is_ok_and(|v| v == "1") {
        return run_offline_return().await;
    }
    if std::env::var("E2E_UPDATE_CHECK").is_ok_and(|v| v == "1") {
        return run_update_check_mode().await;
    }

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
    // -----------------------------------------------------------------------
    // 1. Start both bots sequentially (sharing the same i2p router).
    // -----------------------------------------------------------------------
    let (a, mut a_events) = start_bot("bot-a", &work_dir).await.context("bot-a start")?;
    let (b, mut b_events) = start_bot("bot-b", &work_dir).await.context("bot-b start")?;
    anyhow::ensure!(a.i2p_address != b.i2p_address, "bots must have distinct personal terminals");
    let t_start = Instant::now();

    // -----------------------------------------------------------------------
    // 2. Cross-add contacts (writes to DB; relay loop will handle the rest).
    // -----------------------------------------------------------------------
    // Each side records where the *other* collects — exactly what a v2 contact
    // card carries in the app.
    let contact_b_in_a = a
        .session
        .add_contact(&b.card, &b.i2p_address, "bot-b")
        .await
        .context("bot-a: add_contact(bot-b)")?;
    let contact_a_in_b = b
        .session
        .add_contact(&a.card, &a.i2p_address, "bot-a")
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

    // E2E_BOTH_FIRST: B's own first letter, as A received it.
    let both_first = std::env::var("E2E_BOTH_FIRST").is_ok_and(|v| v == "1");
    let a_greeted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let a_greeted_notify = Arc::new(Notify::new());

    // E2E_BIG_FILE: what bot-b ended up with.
    let big_file: usize = std::env::var("E2E_BIG_FILE").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let b_file: Arc<Mutex<Option<Vec<u8>>>> = Default::default();
    let b_file_notify = Arc::new(Notify::new());
    let restart = std::env::var("E2E_BIG_FILE_RESTART").is_ok_and(|v| v == "1");
    // Parts bot-b holds of the big file, and of how many.
    let b_parts = Arc::new((std::sync::atomic::AtomicU32::new(0), std::sync::atomic::AtomicU32::new(0)));

    // Bot A event loop
    {
        let connect_notify = a_connect_notify.clone();
        let connect_at = a_connect_at.clone();
        let echoes = a_echoes.clone();
        let echo_notify = a_echo_notify.clone();
        let (greeted, greeted_notify) = (a_greeted.clone(), a_greeted_notify.clone());
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
                        if payload.body.starts_with("echo:") {
                            echoes.lock().await.push((payload.body, Instant::now()));
                            echo_notify.notify_one();
                        } else if payload.body == GREETING {
                            greeted.store(true, std::sync::atomic::Ordering::SeqCst);
                            greeted_notify.notify_one();
                        }
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
        let (b_file, b_file_notify, b_parts) = (b_file.clone(), b_file_notify.clone(), b_parts.clone());
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
                    SessionEvent::FileProgress { done, total, incoming: true, .. } => {
                        b_parts.0.store(done, std::sync::atomic::Ordering::SeqCst);
                        b_parts.1.store(total, std::sync::atomic::Ordering::SeqCst);
                        if done % 8 == 0 {
                            eprintln!("[e2e] bot-b: file part {done}/{total}");
                        }
                    }
                    SessionEvent::FileReceived { attachment_id, .. } => {
                        let got = b_session.db.get_attachment(attachment_id).ok().flatten()
                            .and_then(|a| b_session.read_attachment(&a).ok());
                        eprintln!("[e2e] bot-b: file whole ({} bytes)", got.as_ref().map_or(0, |g| g.len()));
                        *b_file.lock().await = got;
                        b_file_notify.notify_one();
                    }
                    SessionEvent::FileFailed { reason, .. } => {
                        eprintln!("[e2e] bot-b: file failed: {reason}");
                    }
                    SessionEvent::IncomingPayload { contact_id, payload, .. } => {
                        if payload.body.is_empty() || payload.body == BIG_FILE_BODY {
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
            let notified = notify.notified();
            tokio::pin!(notified);
            loop {
                if let Some(ts) = *at.lock().await {
                    return ts;
                }
                notified.as_mut().enable();
                if let Some(ts) = *at.lock().await {
                    return ts;
                }
                notified.as_mut().await;
                notified.set(notify.notified());
            }
        }
    };
    let b_connected_fut = {
        let notify = b_connect_notify.clone();
        let at = b_connect_at.clone();
        async move {
            let notified = notify.notified();
            tokio::pin!(notified);
            loop {
                if let Some(ts) = *at.lock().await {
                    return ts;
                }
                notified.as_mut().enable();
                if let Some(ts) = *at.lock().await {
                    return ts;
                }
                notified.as_mut().await;
                notified.set(notify.notified());
            }
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
    if both_first {
        eprintln!("[e2e] bot-b writes first too, at the same moment: the sessions cross");
        let b_session = b.session.clone();
        tokio::spawn(async move {
            if let Err(e) = b_session.send_message(contact_a_in_b, GREETING.into(), vec![], None, None, None).await {
                eprintln!("[e2e] bot-b: greeting send error: {e}");
            }
        });
    }
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
            let notified = echo_notify_ref.notified();
            tokio::pin!(notified);
            loop {
                if echoes_ref.lock().await.len() >= n_messages {
                    return;
                }
                notified.as_mut().enable();
                if echoes_ref.lock().await.len() >= n_messages {
                    return;
                }
                notified.as_mut().await;
                notified.set(echo_notify_ref.notified());
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
    if both_first {
        let left = timeout.saturating_sub(t_start.elapsed());
        let arrived = tokio::time::timeout(left, async {
            while !a_greeted.load(std::sync::atomic::Ordering::SeqCst) {
                let n = a_greeted_notify.notified();
                if a_greeted.load(std::sync::atomic::Ordering::SeqCst) { break; }
                n.await;
            }
        }).await.is_ok();
        if !arrived {
            bail!("crossing sessions: bot-b's own first letter never reached bot-a");
        }
        eprintln!("[e2e] crossing sessions: bot-b's own first letter arrived too");
    }

    if big_file > 0 {
        file_transfer::Transfer {
            a: &a, b: &b, contact_b_in_a, work_dir: &work_dir, big_file, restart,
            b_file, b_file_notify, b_parts, timeout, t_start,
        }.run().await?;
    }

    a.session.shutdown();
    b.session.shutdown();

    if echoes.len() < n_messages {
        bail!(
            "delivery assertion failed: received {}/{n_messages} echoes",
            echoes.len()
        );
    }

    no_local_ports("harness", "self")?;
    eprintln!("[e2e] SUCCESS — all {n_messages} messages delivered and echoed");
    Ok(())
}
