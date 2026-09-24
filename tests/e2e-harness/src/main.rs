//! End-to-end messaging harness.
//!
//! Boots two headless bot instances (A and B) on the i2p router inside this
//! process (one per process, a destination per bot) against a relay,
//! cross-adds them as contacts, sends N messages A→B with an attachment,
//! verifies that B echoes every message back to A, and reports latency and
//! resource metrics to stdout and to `$GITHUB_STEP_SUMMARY` when running in CI.
//!
//! # Required environment variables
//! * `E2E_RELAY_DEST` — i2p destination of the running relay (contents of
//!   `dest.pub` as printed by `gipny-relay` and `relay-testnet.yml`). Not
//!   needed with `E2E_IN_PROCESS_RELAYS=1`.
//!
//! # Optional environment variables
//! * `E2E_RELAY_DEST_A` / `E2E_RELAY_DEST_B` — a separate relay per bot.
//! * `E2E_IN_PROCESS_RELAYS=1` — no standalone relay: each bot gets an
//!   in-process `EphemeralRelay` on the router in this process, started
//!   for that bot's key only (`MemStoreLimits::personal`) after the bot is up
//!   — the app's built-in relay, in the order the app does it.
//! * `E2E_AGENT_BIN=<path>` — a different test: bot-a is the master and the
//!   far side is the real `gipny-agent` binary at that path, started with
//!   bot-a's v2 card, on its own router in its own process, with no
//!   `--relay` — proving the agent hosts a personal relay for itself, the
//!   same as the app does. The master gets its own in-process relay too.
//!   `E2E_N_MESSAGES` is the number of commands.
//! * `E2E_DHT_OFFLINE=1` — a different test: delivery through the relay
//!   network while each side is away in turn. Needs `E2E_DHT_SEED_DEST` (the
//!   destination of a `gipny-relay --dht`). See [`run_dht_offline_mode`].
//! * `E2E_BOTH_FIRST=1` — bot-b writes to bot-a at the same moment bot-a
//!   writes to bot-b: two sessions are opened at once and their X3dhInits
//!   cross (session.rs `ours_stands`). Its letter must arrive too.
//! * `E2E_BIG_FILE=<bytes>` — after the echoes, bot-a sends bot-b a file of
//!   that size; it goes in parts (libcore::files) and must arrive whole and
//!   equal. The throughput is printed.
//! * `E2E_N_MESSAGES`   — number of messages A sends to B (default: 5).
//! * `E2E_TIMEOUT_SECS` — hard deadline for the whole test (default: 300).
//! * `E2E_WORK_DIR`     — working directory for bot data dirs (default:
//!   `/tmp/e2e-harness`).
//! * `GITHUB_STEP_SUMMARY` — when set (always true in GitHub Actions), the
//!   timing table is appended to this file.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use gipny_libcore::agent::BODY_OFF;
use gipny_libcore::relay_server::{EphemeralRelay, MemStoreLimits};
use gipny_libcore::{
    ContactCard, Db, IdentityCard, SessionEvent, SessionManager, TorNode, WireConsole,
    CONSOLE_COMMAND, CONSOLE_GRANT, CONSOLE_OFF, CONSOLE_OUTPUT, CONSOLE_REVOKE,
};
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
    /// Wall-clock milliseconds from the `TorNode::start` call to its tunnels.
    router_ready_ms: u64,
}

async fn start_bot(
    name: &'static str,
    work_dir: &PathBuf,
    relay_dest: &str,
    dht_seeds: &[String],
) -> Result<(BotHandle, tokio::sync::mpsc::Receiver<SessionEvent>)> {
    let data_dir = work_dir.join(name);
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("{name}: create data dir"))?;

    eprintln!("[e2e] {name}: starting i2p router...");
    let t0 = Instant::now();
    let node = Arc::new(
        TorNode::start(&data_dir, Default::default())
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
    if !dht_seeds.is_empty() {
        put_seeds(&db, dht_seeds).with_context(|| format!("{name}: seed the node table"))?;
    }

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

/// Start one in-process relay per bot on the shared router.
///
/// A published destination is not usable until its tunnels exist, which is
/// what `EphemeralRelay::start` waits for, so this can take a couple of
/// minutes. It runs before the delivery clock starts and has its own deadline.
///
/// Each relay is a *personal* one, as the app starts it: it holds mail and a
/// prekey bundle for its owner's key and refuses everyone else's. Delivery
/// then proves two things the unit tests cannot: that a contact's deposit names
/// the key the relay was started for, and that the refusal does not get in the
/// way of the owner's own traffic.
/// Clients take seeds only from the build. The e2e seed goes where a node
/// that answered before would be: the saved table the node reads when it
/// joins. Unlike a built-in seed it is ordinary there, so a join that fails
/// (the seed's tunnels still building) saves a table without it and the node
/// forgets it after three failures; hence written again before every join.
fn put_seeds(db: &Db, seeds: &[String]) -> Result<()> {
    let peers: Vec<gipny_dht::node::KnownPeer> = seeds
        .iter()
        .map(|d| gipny_dht::node::KnownPeer {
            info: gipny_dht::proto::NodeInfo {
                destination: d.clone(),
                stores: true,
                version: gipny_dht::proto::PROTOCOL_VERSION,
            },
            first_seen_ms: 0,
            last_ok_ms: 0,
        })
        .collect();
    db.dht_peers_save(&peers)?;
    Ok(())
}

/// What bot-b writes first with E2E_BOTH_FIRST.
const GREETING: &str = "hello from bot-b";
/// The text of the letter carrying E2E_BIG_FILE; not echoed.
const BIG_FILE_BODY: &str = "big file";

async fn start_in_process_relays(node: &TorNode, owner_a: [u8; 32], owner_b: [u8; 32]) -> Result<(EphemeralRelay, EphemeralRelay)> {
    let _ = node; // up already: the relays share its router
    eprintln!("[e2e] starting two in-process relays on the in-process router...");
    let t0 = Instant::now();
    let (a, b) = tokio::time::timeout(Duration::from_secs(300), async {
        tokio::join!(
            EphemeralRelay::start(MemStoreLimits::personal(owner_a), None),
            EphemeralRelay::start(MemStoreLimits::personal(owner_b), None),
        )
    })
    .await
    .context("timeout: in-process relays did not come up in 300s")?;
    let (a, b) = (a.context("in-process relay A")?, b.context("in-process relay B")?);
    eprintln!(
        "[e2e] in-process relays up in {} ms (A={}... B={}...)",
        t0.elapsed().as_millis(),
        &a.address()[..20.min(a.address().len())],
        &b.address()[..20.min(b.address().len())],
    );
    Ok((a, b))
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

    // The master's own relay, for the agent to deposit its GRANT/replies on.
    // No `--relay` is passed to the agent below: it hosts a personal relay
    // for itself and tells the master the address through its GRANT message,
    // exactly as any contact's relay is learned.
    // The router first: the relay is a destination on it, and bot-a (below)
    // takes the same one.
    gipny_libcore::embedded::router(&work_dir.join("router"), Default::default()).context("i2p router")?;
    eprintln!("[e2e] starting the master's in-process relay...");
    let t0 = Instant::now();
    let relay = tokio::time::timeout(Duration::from_secs(300), EphemeralRelay::start(MemStoreLimits::default(), None))
        .await
        .context("timeout: in-process relay did not come up in 300s")?
        .context("in-process relay")?;
    let relay_dest = relay.address().to_string();
    let relay_ms = t0.elapsed().as_millis() as u64;
    eprintln!("[e2e] relay up in {relay_ms} ms ({}...)", &relay_dest[..20.min(relay_dest.len())]);

    let (a, mut a_events) = start_bot("bot-a", &work_dir, &relay_dest, &[]).await.context("bot-a start")?;
    let master_card = ContactCard {
        onion: a.onion.clone(),
        sign_pk: a.card.sign_pk,
        dh_pk: a.card.dh_pk,
        relay: Some(relay_dest.clone()),
        name: Some("bot-a".into()),
    }
    .encode();

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
         | relay ready | {relay_ms} ms |\n\
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
    drop(relay);
    if !failures.is_empty() {
        bail!("agent e2e failed:\n  {}", failures.join("\n  "));
    }
    eprintln!("[e2e] SUCCESS — the agent ran {n_commands} commands one at a time, ran the uploaded script, and left on OFF");
    Ok(())
}

// ---------------------------------------------------------------------------
// Relay-network mode: each side away in turn
// ---------------------------------------------------------------------------

/// A bot as the app runs it: session plus its personal in-process relay.
struct LiveBot {
    session: Arc<SessionManager>,
    card: IdentityCard,
    onion: String,
    relay_address: String,
    events: tokio::sync::mpsc::Receiver<SessionEvent>,
    _relay: EphemeralRelay,
}

impl LiveBot {
    /// Start (or restart, on the same data dir) and wait until the relay
    /// network has answered. Every start is a new relay address.
    async fn start(name: &'static str, work_dir: &PathBuf, seeds: &[String], budget: Duration) -> Result<Self> {
        let (bot, events) = start_bot(name, work_dir, "", seeds).await?;
        // The agent's arrangement: the personal relay also answers for the
        // session's own network node.
        let relay = tokio::time::timeout(
            Duration::from_secs(300),
            EphemeralRelay::start(MemStoreLimits::personal(bot.card.sign_pk), Some(bot.session.dht_handler())),
        )
        .await
        .with_context(|| format!("{name}: relay did not come up in 300s"))?
        .with_context(|| format!("{name}: relay"))?;
        let relay_address = relay.address().to_string();
        bot.session.set_relay_onion(&relay_address).with_context(|| format!("{name}: set_relay_onion"))?;
        eprintln!("[e2e] {name}: relay {}...", &relay_address[..20.min(relay_address.len())]);

        // join_dht bootstraps, announces us, publishes our bundle and
        // collects what waits for us. Repeated until the seed has answered,
        // instead of waiting out the 45-minute maintenance tick.
        let session = bot.session.clone();
        let address = relay_address.clone();
        let seeds = seeds.to_vec();
        poll(budget, Duration::from_secs(10), || {
            let (session, address, seeds) = (session.clone(), address.clone(), seeds.clone());
            async move {
                if session.dht_peer_count() == 0 {
                    let _ = put_seeds(&session.db, &seeds);
                }
                session.join_dht(&address).await;
                session.dht_peer_count() > 0
            }
        })
        .await
        .with_context(|| format!("{name}: the seed never answered"))?;
        eprintln!("[e2e] {name}: in the relay network ({} nodes)", bot.session.dht_peer_count());
        Ok(Self { session: bot.session, card: bot.card, onion: bot.onion, relay_address, events, _relay: relay })
    }

    /// Gone completely: tasks, relay and destination, as a closed app.
    fn stop(self, name: &str) {
        self.session.shutdown();
        eprintln!("[e2e] {name}: offline (was at relay {}...)", &self.relay_address[..20.min(self.relay_address.len())]);
    }

    /// Until nothing to `contact` is waiting to go out: every letter is held
    /// by some node of the network (or taken by a live relay).
    async fn drain(&self, name: &str, contact: i64, budget: Duration) -> Result<()> {
        let db = self.session.db.clone();
        poll(budget, Duration::from_secs(3), || {
            let db = db.clone();
            async move { db.list_unsent_outgoing(contact, 1000).map(|v| v.is_empty()).unwrap_or(false) }
        })
        .await
        .with_context(|| format!("{name}: letters to contact {contact} never left"))
    }

    /// Collect `n` bodies with `prefix` from incoming payloads.
    async fn receive(&mut self, name: &str, prefix: &str, n: usize, budget: Duration) -> Result<Vec<(i64, String)>> {
        let mut got: Vec<(i64, String)> = Vec::new();
        tokio::time::timeout(budget, async {
            while got.len() < n {
                match self.events.recv().await {
                    Some(SessionEvent::IncomingPayload { contact_id, payload, .. }) if payload.body.starts_with(prefix) => {
                        eprintln!("[e2e] {name}: received '{}'", payload.body);
                        if !got.iter().any(|(_, b)| *b == payload.body) {
                            got.push((contact_id, payload.body));
                        }
                    }
                    Some(_) => {}
                    None => break,
                }
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("{name}: {}/{n} '{prefix}' letters after {}s", got.len(), budget.as_secs()))?;
        if got.len() < n {
            bail!("{name}: event stream closed with {}/{n} letters", got.len());
        }
        Ok(got)
    }
}

async fn poll<F, Fut>(budget: Duration, every: Duration, mut done: F) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + budget;
    loop {
        if done().await {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("not within {}s", budget.as_secs());
        }
        tokio::time::sleep(every).await;
    }
}

/// Delivery through the relay network while each side is away in turn, which
/// no direct path can do:
///
/// 1. A and B come up, each with a personal relay, and join the network
///    through the seed. B puts its prekey bundle there.
/// 2. They add each other (B learns A's relay, A learns B's), then **B goes
///    away** before any letter is written.
/// 3. A writes N letters. B's relay is gone, so A opens the session from B's
///    bundle in the network and leaves the letters there; then **A goes away**.
/// 4. B comes back on a **new relay address**, collects the N letters from the
///    network and answers each. A's relay is gone too, so the answers go to
///    the network; then B goes away.
/// 5. A comes back, also somewhere new, and collects the N answers.
///
/// Bots run as the agent does: their personal relays serve the network too.
/// Whoever is away takes what it held along, so what arrives was held by the
/// seed or by the side that stayed.
async fn run_dht_offline_mode() -> Result<()> {
    let n: usize = std::env::var("E2E_N_MESSAGES").ok().and_then(|s| s.parse().ok()).unwrap_or(3);
    let timeout_secs: u64 = std::env::var("E2E_TIMEOUT_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(1200);
    let work_dir = PathBuf::from(std::env::var("E2E_WORK_DIR").unwrap_or_else(|_| "/tmp/e2e-harness".into()));
    std::fs::create_dir_all(&work_dir).context("create work dir")?;
    let seeds: Vec<String> = std::env::var("E2E_DHT_SEED_DEST")
        .context("E2E_DHT_OFFLINE needs E2E_DHT_SEED_DEST, the seed's destination")?
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if seeds.is_empty() {
        bail!("E2E_DHT_SEED_DEST is empty");
    }
    eprintln!("[e2e] relay-network run: {} seed(s), n={n}, timeout={timeout_secs}s", seeds.len());

    let t0 = Instant::now();
    let step = Duration::from_secs(timeout_secs / 4);
    let phase = |label: &str| eprintln!("[e2e] ── {label} (+{}s)", t0.elapsed().as_secs());

    phase("1. both join the network");
    let (a, b) = tokio::try_join!(
        LiveBot::start("bot-a", &work_dir, &seeds, step),
        LiveBot::start("bot-b", &work_dir, &seeds, step),
    )?;
    let session = b.session.clone();
    poll(step, Duration::from_secs(10), || async { session.publish_bundle_to_dht().await })
        .await
        .context("bot-b: its prekey bundle never reached the network")?;
    eprintln!("[e2e] bot-b: prekey bundle is in the network");

    phase("2. contacts, then B goes away");
    let a_in_b = b.session.add_contact_via(&a.card, &a.onion, "bot-a", Some(&a.relay_address)).await.context("bot-b: add bot-a")?;
    let (b_card, b_onion, b_relay_before) = (b.card.clone(), b.onion.clone(), b.relay_address.clone());
    b.stop("bot-b");
    let b_in_a = a.session.add_contact_via(&b_card, &b_onion, "bot-b", Some(&b_relay_before)).await.context("bot-a: add bot-b")?;

    phase("3. A writes to an absent B, then goes away");
    for i in 1..=n {
        a.session
            .send_message(b_in_a, format!("dht-{i:03}"), vec![], None, None, None)
            .await
            .with_context(|| format!("bot-a: send dht-{i:03}"))?;
    }
    a.drain("bot-a", b_in_a, step).await?;
    eprintln!("[e2e] bot-a: all {n} letters are in the network");
    let a_relay_before = a.relay_address.clone();
    a.stop("bot-a");

    phase("4. B comes back elsewhere, reads, answers the absent A");
    let mut b = LiveBot::start("bot-b", &work_dir, &seeds, step).await?;
    if b.relay_address == b_relay_before {
        bail!("bot-b came back on the same relay address; the run proves nothing about address change");
    }
    let letters = b.receive("bot-b", "dht-", n, step).await?;
    for (contact_id, body) in &letters {
        if *contact_id != a_in_b {
            bail!("bot-b: '{body}' arrived from contact {contact_id}, expected bot-a ({a_in_b})");
        }
        b.session
            .send_message(*contact_id, format!("re:{body}"), vec![], None, None, None)
            .await
            .with_context(|| format!("bot-b: answer {body}"))?;
    }
    b.drain("bot-b", a_in_b, step).await?;
    eprintln!("[e2e] bot-b: all {n} answers are in the network");
    let b_relay_after = b.relay_address.clone();
    b.stop("bot-b");

    phase("5. A comes back elsewhere and reads the answers");
    let mut a = LiveBot::start("bot-a", &work_dir, &seeds, step).await?;
    if a.relay_address == a_relay_before {
        bail!("bot-a came back on the same relay address; the run proves nothing about address change");
    }
    let answers = a.receive("bot-a", "re:dht-", n, step).await?;

    // Never "ask for a fresh card": A must have found where B is now.
    let b_relay_now = b_relay_after.clone();
    let db = a.session.db.clone();
    poll(step, Duration::from_secs(10), || {
        let (db, want) = (db.clone(), b_relay_now.clone());
        async move {
            db.get_contact(b_in_a).ok().flatten().and_then(|c| c.relay_address).as_deref() == Some(want.as_str())
        }
    })
    .await
    .context("bot-a never learned bot-b's new relay address")?;
    eprintln!("[e2e] bot-a: knows bot-b's new relay address");
    a.stop("bot-a");

    let total = t0.elapsed().as_secs();
    eprintln!("[e2e] SUCCESS — {n} letters and {} answers crossed the relay network with each side away in turn ({total}s)", answers.len());
    if let Ok(summary_path) = std::env::var("GITHUB_STEP_SUMMARY") {
        use std::io::Write;
        let content = format!(
            "### e2e relay network, each side away in turn — ✅ PASS\n\n             {n} letters A→B while B was away, {n} answers B→A while A was away;              both came back on new relay addresses, and A found B's. Total {total} s.\n"
        );
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(&summary_path)
            .and_then(|mut f| f.write_all(content.as_bytes()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    if let Some(bin) = std::env::var_os("E2E_AGENT_BIN") {
        return run_agent_mode(PathBuf::from(bin)).await;
    }
    if std::env::var("E2E_DHT_OFFLINE").is_ok_and(|v| v == "1") {
        return run_dht_offline_mode().await;
    }

    // One relay or two. Two is the interesting case: each bot collects from its
    // own, and a message only arrives if the sender deposits on the *recipient's*
    // relay rather than its own. With a single destination this degenerates to
    // the old shared-relay run, which is still worth having.
    //
    // Or, with E2E_IN_PROCESS_RELAYS=1, no standalone relay at all: each bot's
    // relay is an EphemeralRelay started inside this process, on a destination
    // that exists only in memory. That is the relay every client can run, over
    // real i2p, with the bots reaching it exactly as they would a remote one.
    let in_process = std::env::var("E2E_IN_PROCESS_RELAYS").is_ok_and(|v| v == "1");
    let standalone = if in_process {
        None
    } else {
        let relay_dest = std::env::var("E2E_RELAY_DEST")
            .context("E2E_RELAY_DEST env var is required (contents of dest.pub)")?;
        let a = std::env::var("E2E_RELAY_DEST_A").unwrap_or_else(|_| relay_dest.clone());
        let b = std::env::var("E2E_RELAY_DEST_B").unwrap_or_else(|_| relay_dest.clone());
        Some((a, b))
    };
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
    // With in-process relays the bots start with no relay at all and wait,
    // which is the app's built-in mode exactly: the identity exists first, the
    // relay is started for that identity, and only then does the client learn
    // where it collects.
    let first = standalone.clone().unwrap_or_default();
    let a_result = start_bot("bot-a", &work_dir, &first.0, &[]).await;
    let b_result = start_bot("bot-b", &work_dir, &first.1, &[]).await;
    let (a, mut a_events) = a_result.context("bot-a start")?;
    let (b, mut b_events) = b_result.context("bot-b start")?;

    let (relay_a, relay_b, in_process_relays) = match standalone {
        Some((ra, rb)) => (ra, rb, None),
        None => {
            let (ra, rb) = start_in_process_relays(&a.session.node, a.card.sign_pk, b.card.sign_pk).await?;
            let (dest_a, dest_b) = (ra.address().to_string(), rb.address().to_string());
            a.session.set_relay_onion(&dest_a).context("bot-a: set_relay_onion")?;
            b.session.set_relay_onion(&dest_b).context("bot-b: set_relay_onion")?;
            eprintln!("[e2e] personal relays: each holds mail for its own bot and refuses anyone else's");
            (dest_a, dest_b, Some((ra, rb)))
        }
    };
    let split_relays = relay_a != relay_b;

    // The delivery clock starts here: router and relay start-up have their own
    // deadlines above.
    let t_start = Instant::now();

    if split_relays {
        eprintln!(
            "[e2e] running (bot-a relay={}... bot-b relay={}... n={n_messages} timeout={timeout_secs}s)",
            &relay_a[..20.min(relay_a.len())], &relay_b[..20.min(relay_b.len())]
        );
        eprintln!("[e2e] two relays: delivery proves messages follow the recipient's card");
    } else {
        eprintln!(
            "[e2e] running (shared relay={}... n={n_messages} timeout={timeout_secs}s)",
            &relay_a[..20.min(relay_a.len())]
        );
    }

    // -----------------------------------------------------------------------
    // 2. Cross-add contacts (writes to DB; relay loop will handle the rest).
    // -----------------------------------------------------------------------
    // Each side records where the *other* collects — exactly what a v2 contact
    // card carries in the app.
    let contact_b_in_a = a
        .session
        .add_contact_via(&b.card, &b.onion, "bot-b", Some(&relay_b))
        .await
        .context("bot-a: add_contact(bot-b)")?;
    let contact_a_in_b = b
        .session
        .add_contact_via(&a.card, &a.onion, "bot-a", Some(&relay_a))
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
        let (b_file, b_file_notify) = (b_file.clone(), b_file_notify.clone());
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
        let data: Vec<u8> = (0..big_file).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let t0 = Instant::now();
        a.session
            .send_message(contact_b_in_a, BIG_FILE_BODY.into(), vec![("big.bin".into(), data.clone())], None, None, None)
            .await
            .context("send the big file")?;
        eprintln!("[e2e] bot-a: sent a {big_file}-byte file in parts");
        let left = timeout.saturating_sub(t_start.elapsed());
        tokio::time::timeout(left, async {
            loop {
                let n = b_file_notify.notified();
                if b_file.lock().await.is_some() { break; }
                n.await;
            }
        }).await.context("timeout: the big file never arrived whole")?;
        let got = b_file.lock().await.take().unwrap_or_default();
        if got != data {
            bail!("big file: {} bytes arrived, not equal to the {} sent", got.len(), data.len());
        }
        let secs = t0.elapsed().as_secs_f64();
        eprintln!(
            "[e2e] big file: {big_file} bytes whole and equal in {secs:.1} s ({:.0} KiB/s)",
            big_file as f64 / 1024.0 / secs
        );
    }

    a.session.shutdown();
    b.session.shutdown();

    if echoes.len() < n_messages {
        bail!(
            "delivery assertion failed: received {}/{n_messages} echoes",
            echoes.len()
        );
    }

    // Delivery alone could in principle come from somewhere else; a bundle held
    // in each in-process store shows each bot really authenticated to its relay
    // over i2p and published there.
    if let Some((ra, rb)) = &in_process_relays {
        let (sa, sb) = (ra.stats(), rb.stats());
        eprintln!("[e2e] in-process relay A: {sa:?}");
        eprintln!("[e2e] in-process relay B: {sb:?}");
        if sa.bundles == 0 || sb.bundles == 0 {
            bail!("in-process relay assertion failed: a bot never published to its relay");
        }
    }

    eprintln!("[e2e] SUCCESS — all {n_messages} messages delivered and echoed");
    Ok(())
}
