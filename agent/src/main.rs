//! gipny-agent — headless console agent.
//!
//! Runs a Gipny identity, registers one master by their contact card, and
//! executes the shell commands that master sends over the encrypted channel.
//! Everything rides the ordinary message pipeline: commands arrive as
//! CONSOLE_COMMAND messages, replies go back as CONSOLE_OUTPUT messages.
//!
//! Usage:
//!   gipny-agent --data <dir> --master <gipny:v2:…card…>
//!
//! `--master` is needed once: the card is kept in `<data>/master.card` and
//! later runs read it from there. `--data` can also come from the
//! `GIPNY_AGENT_DATA` environment variable.
//!
//! Optional flags:
//!   --name <display name>   what the agent calls itself (default: hostname)
//!   --relay <dest>          external relay to collect from, remembered in <data>/relay.txt;
//!                           default: none — the agent hosts a personal relay for itself,
//!                           the same way the app does for its own inbox
//!   --cwd <dir>             working directory for commands (default: home dir)
//!   --timeout <secs>        per-command timeout (default: 120 s)
//!
//! On start the agent writes its own card to `<data>/card.txt`, prints it, and
//! tells the master it is ready (GRANT). The master's app creates the contact
//! by itself from that first message; nothing has to be typed in.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::signal;
use tokio::sync::mpsc;

use gipny_libcore::{
    ContactCard, Db, I2pNode, SessionManager, SessionEvent, Updater, UpdateComponent,
    WireConsole, CONSOLE_COMMAND, CONSOLE_GRANT, CONSOLE_OFF, CONSOLE_REVOKE,
};
use gipny_libcore::agent::{self, ExecOptions, BODY_GRANT, BODY_REVOKE};
use gipny_libcore::router::RouterSettings;
use gipny_libcore::update::{UPDATE_CHECK_INITIAL_SECS, UPDATE_CHECK_INTERVAL_SECS};

const MASTER_CARD_FILE: &str = "master.card";
const RELAY_FILE: &str = "relay.txt";
const OWN_CARD_FILE: &str = "card.txt";
/// Same DB setting key/convention as the app's `dismissed_update_version`:
/// which version this run already downloaded and installed, so a later check
/// this same launch does not redo it every `UPDATE_CHECK_INTERVAL_SECS`.
const SETTING_DISMISSED_UPDATE: &str = "dismissed_update_version";
/// How long the closing REVOKE may take to reach the master before the agent
/// exits anyway. It is queued and retried like any message; this only bounds
/// how long a stopped service lingers.
const REVOKE_WAIT: Duration = Duration::from_secs(60);

fn usage() -> ! {
    eprintln!(
        "Usage: gipny-agent --data <dir> [--master <card>] [--name <name>] \
        [--relay <dest>] [--cwd <dir>] [--timeout <secs>]"
    );
    std::process::exit(1);
}

#[derive(Default)]
struct Args {
    data: Option<PathBuf>,
    master_card: Option<String>,
    name: Option<String>,
    relay: Option<String>,
    cwd: Option<PathBuf>,
    timeout_secs: Option<u64>,
}

fn parse_args() -> Args {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--data"    => a.data        = it.next().map(PathBuf::from),
            "--master"  => a.master_card = it.next(),
            "--name"    => a.name        = it.next(),
            "--relay"   => a.relay       = it.next(),
            "--cwd"     => a.cwd         = it.next().map(PathBuf::from),
            "--timeout" => a.timeout_secs = it.next().and_then(|s| s.parse().ok()),
            "--help" | "-h" => usage(),
            other => { eprintln!("unknown flag: {other}"); usage(); }
        }
    }
    a
}

/// `--data`, or `GIPNY_AGENT_DATA`. Created private to the user: it holds the
/// identity, the plain database and the master's card.
fn data_dir(args: &Args) -> Result<PathBuf> {
    let dir = args.data.clone()
        .or_else(|| std::env::var_os("GIPNY_AGENT_DATA").map(PathBuf::from))
        .context("no data directory: pass --data <dir> or set GIPNY_AGENT_DATA")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)
            .with_context(|| format!("create data dir {}", dir.display()))?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(&dir).with_context(|| format!("create data dir {}", dir.display()))?;
    Ok(dir)
}

fn write_private(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path).with_context(|| format!("write {}", path.display()))?;
    f.write_all(contents.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(())
}

/// The master's card: from `--master`, which is then remembered, or from the
/// copy an earlier run left behind. A new `--master` replaces the old one.
fn load_master(args: &Args, data: &Path) -> Result<ContactCard> {
    let path = data.join(MASTER_CARD_FILE);
    let raw = match &args.master_card {
        Some(raw) => {
            let raw = raw.trim().to_string();
            ContactCard::parse(&raw).context("invalid --master card")?;
            write_private(&path, &raw)?;
            raw
        }
        None => std::fs::read_to_string(&path).with_context(|| format!(
            "no master: pass --master <card> once, it is then remembered in {}",
            path.display()
        ))?,
    };
    ContactCard::parse(&raw).with_context(|| format!("invalid master card in {}", path.display()))
}

/// `--relay` is remembered like `--master`: given once, read back afterwards.
fn load_relay(args: &Args, data: &Path) -> Result<Option<String>> {
    let path = data.join(RELAY_FILE);
    match args.relay.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        Some(r) => {
            write_private(&path, r)?;
            Ok(Some(r.to_string()))
        }
        None => Ok(std::fs::read_to_string(&path).ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())),
    }
}

/// Load the encrypted attachments of an incoming command from the data dir.
fn load_attachments(db: &Db, data_dir: &Path, msg_id: i64) -> Vec<(String, Vec<u8>)> {
    let atts = match db.list_attachments(msg_id) {
        Ok(a) => a,
        Err(e) => { eprintln!("[agent] list_attachments({msg_id}): {e}"); return vec![]; }
    };
    let mut out = Vec::with_capacity(atts.len());
    for a in atts {
        let key: [u8; 32] = match a.key.as_slice().try_into() {
            Ok(k) => k,
            Err(_) => { eprintln!("[agent] bad attachment key"); continue; }
        };
        let path = data_dir.join("attachments").join(&a.path);
        match gipny_libcore::files::read_attachment(&path, key, a.size as u64, a.chunk_size) {
            Ok(data) => out.push((a.name, data)),
            Err(e) => eprintln!("[agent] read {}: {e}", path.display()),
        }
    }
    out
}

/// One command waiting for the worker: the row id (its attachments are looked
/// up when it runs) and the body.
struct Job {
    message_id: i64,
    body: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args();
    let data_dir = data_dir(&args)?;
    let master_card = load_master(&args, &data_dir)?;
    // The agent's own relay is separate from the master's: this is only about
    // reaching the master to deliver a message, which needs a v2 card either
    // way.
    if master_card.relay.is_none() {
        bail!(
            "the master's card is a v1 card without a relay, and the agent has no way to \
            reach the master until it gets a v2 card (Settings → my card)"
        );
    }
    let relay_override = load_relay(&args, &data_dir)?;

    // A plain (unencrypted) database: a headless daemon has nobody to type a
    // passphrase. The data dir is 0700 for that reason.
    let db = Arc::new(
        Db::open_plain(&data_dir.join("agent.db")).context("open agent database")?,
    );

    eprintln!("[agent] starting the i2p router (in this process, no ports)…");
    let node = Arc::new(
        I2pNode::start(&data_dir, RouterSettings::default()).await.context("start i2p node")?,
    );
    eprintln!("[agent] i2p address: {}", node.b32_address().unwrap_or_else(|| node.onion_address().to_string()));

    tokio::spawn(run_update_loop(db.clone(), data_dir.clone(), node.clone()));

    let display_name = args.name.clone().unwrap_or_else(agent::hostname);
    let (session, mut events) = SessionManager::start(data_dir.clone(), db.clone(), node.clone())
        .await
        .context("start session manager")?;
    session.set_display_name(&display_name)?;
    let me = session.my_card();

    // Where the agent collects its own mail: a personal relay it hosts for
    // itself by default (exactly what the app does for its own inbox), or an
    // external one if `--relay` names it — the escape hatch for an offline
    // mailbox. Kept alive for the life of the process; dropping it releases
    // the destination.
    let (relay_onion, _hosted_relay) = match relay_override {
        Some(r) => {
            session.set_relay_onion(&r)?;
            (r, None)
        }
        None => {
            eprintln!("[agent] starting the built-in relay (this can take a minute or two)…");
            let relay = gipny_libcore::EphemeralRelay::start(
                gipny_libcore::MemStoreLimits::personal(me.sign_pk),
                Some(session.dht_handler()),
            )
            .await
            .context("start the agent's built-in relay")?;
            let relay = std::sync::Arc::new(relay);
            let address = relay.address().to_string();
            eprintln!("[agent] built-in relay ready");
            // Collect from it over a pipe, not out through i2p and back.
            session.set_local_relay(relay.clone());
            session.set_relay_onion(&address)?;
            let dht_session = session.clone();
            let dht_address = address.clone();
            tokio::spawn(async move {
                dht_session.join_dht(&dht_address).await;
            });
            (address, Some(relay))
        }
    };

    // Our own card, for the record and for anyone reading the log.
    let own_card = ContactCard {
        onion: node.onion_address().to_string(),
        sign_pk: me.sign_pk,
        dh_pk: me.dh_pk,
        relay: Some(relay_onion.clone()),
        name: Some(display_name.clone()),
    }.encode();
    write_private(&data_dir.join(OWN_CARD_FILE), &own_card)?;
    println!("agent card: {own_card}");
    eprintln!("[agent] master sign_pk: {}…", agent::hex8(&master_card.sign_pk));

    // The master as a contact, with the relay their card names; idempotent
    // across restarts (INSERT OR IGNORE on the identity).
    let master_id = session
        .add_contact_via(
            &master_card.identity(),
            &master_card.onion,
            master_card.name.as_deref().unwrap_or("master"),
            master_card.relay.as_deref(),
        )
        .await
        .context("register master contact")?;
    eprintln!("[agent] master contact_id = {master_id}");

    // Ready. Delivered whenever the master is next online; their app creates
    // the contact from it.
    session
        .send_console(master_id, BODY_GRANT.to_string(), WireConsole::new(CONSOLE_GRANT), vec![])
        .await
        .context("send GRANT to master")?;
    eprintln!("[agent] ready — waiting for commands");

    let exec_opts = ExecOptions {
        timeout: Duration::from_secs(args.timeout_secs.unwrap_or(120)),
        cwd: args.cwd.clone(),
        ..Default::default()
    };

    // One worker, one command at a time, in the order they arrived: the
    // master reads the output as a terminal, and two commands racing each
    // other would interleave their replies.
    let (queue_tx, mut queue_rx) = mpsc::unbounded_channel::<Job>();
    tokio::spawn({
        let session = session.clone();
        let db = db.clone();
        let data_dir = data_dir.clone();
        async move {
            while let Some(job) = queue_rx.recv().await {
                let files = load_attachments(&db, &data_dir, job.message_id);
                eprintln!(
                    "[agent] exec: {}{}",
                    job.body,
                    if files.is_empty() { String::new() } else { format!(" (+{} files)", files.len()) },
                );
                let reply = agent::handle_console_request(&job.body, &files, &exec_opts).await;
                eprintln!(
                    "[agent] exit={:?} dur={:?}ms truncated={}",
                    reply.console.exit_code, reply.console.duration_ms, reply.console.truncated,
                );
                if let Err(e) = session
                    .send_console(master_id, reply.body, reply.console, reply.attachments)
                    .await
                {
                    eprintln!("[agent] reply failed: {e}");
                }
            }
        }
    });

    // Event loop. Stopping — on the master's OFF, Ctrl-C or SIGTERM — sends REVOKE
    // and waits for its delivery (bounded), so the master's app shows the
    // console closed rather than an agent that silently went away.
    let mut stopping: Option<(i64, tokio::time::Instant)> = None;
    // systemd stops the service with SIGTERM: the same as Ctrl-C, so the
    // master hears REVOKE and the router in this process stops before exit.
    #[cfg(unix)]
    let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate()).context("SIGTERM handler")?;
    loop {
        #[cfg(unix)]
        let terminate = sigterm.recv();
        #[cfg(not(unix))]
        let terminate = std::future::pending::<Option<()>>();
        let deadline = async {
            match stopping {
                Some((_, at)) => tokio::time::sleep_until(at).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = async { tokio::select! { _ = signal::ctrl_c() => {}, _ = terminate => {} } } => {
                if stopping.is_some() {
                    eprintln!("[agent] interrupted again — exiting now");
                    break;
                }
                eprintln!("[agent] interrupted — telling the master, then exiting");
                stopping = Some(begin_stop(&session, master_id).await);
            }
            _ = deadline => {
                eprintln!("[agent] the master did not confirm the REVOKE in {}s — exiting anyway", REVOKE_WAIT.as_secs());
                break;
            }
            ev = events.recv() => {
                let Some(ev) = ev else { break };
                match ev {
                    SessionEvent::IncomingPayload { contact_id, payload, message_id } => {
                        // Only the master, only in the direct chat, only
                        // console-framed. The contact id is decided by which
                        // ratchet session decrypted the message, never by
                        // anything the message says about itself.
                        if contact_id != master_id || payload.group.is_some() { continue; }
                        let Some(console) = payload.console.as_ref() else { continue };
                        match console.kind {
                            CONSOLE_COMMAND => {
                                if stopping.is_some() { continue; }
                                let _ = queue_tx.send(Job { message_id, body: payload.body.clone() });
                            }
                            CONSOLE_OFF => {
                                if stopping.is_none() {
                                    eprintln!("[agent] master switched the agent off");
                                    stopping = Some(begin_stop(&session, master_id).await);
                                }
                            }
                            _ => {}
                        }
                    }
                    SessionEvent::RelayError { reason } if reason == gipny_libcore::relay::ERR_NOT_SERVED => {
                        // Not a fault that passes: this can only be the relay
                        // named by --relay, and it is somebody else's
                        // personal one, which will never hold our mail. The
                        // agent's own built-in relay always serves itself.
                        eprintln!(
                            "[agent] the relay named by --relay refused this agent: it is somebody else's \
                            personal relay and will never hold this agent's mail. Drop --relay so the agent \
                            hosts its own relay, or point it at a standalone gipny-relay instead, and give \
                            the master the new card from card.txt."
                        );
                        std::process::exit(2);
                    }
                    SessionEvent::MessageDelivered { message_id } => {
                        if stopping.map(|(id, _)| id == message_id).unwrap_or(false) {
                            eprintln!("[agent] REVOKE delivered — exiting");
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    session.shutdown();
    Ok(())
}

/// Checks GitHub for a newer `gipny-agent` release on the same cadence as the
/// desktop app, and replaces this binary's file in place if one is found —
/// no restart, and no disruption to commands already in flight; the next
/// time this process starts (a service restart, a reboot) runs the new build.
async fn run_update_loop(db: Arc<Db>, data_dir: PathBuf, node: Arc<I2pNode>) {
    let updater = Updater::new(node, UpdateComponent::Agent);
    if !updater.is_configured() {
        eprintln!("[agent] no local HTTP proxy this run — auto-update disabled");
        return;
    }
    // Resolved once, before any install: after a first in-place replace,
    // `current_exe()` would resolve to the old, now-deleted file instead of
    // the path a later install needs to overwrite.
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => { eprintln!("[agent] cannot resolve our own binary path — auto-update disabled: {e}"); return; }
    };
    tokio::time::sleep(Duration::from_secs(UPDATE_CHECK_INITIAL_SECS)).await;
    loop {
        match updater.check(env!("CARGO_PKG_VERSION")).await {
            Ok(Some(info)) => {
                let already_handled = db.get_setting(SETTING_DISMISSED_UPDATE).ok().flatten()
                    .is_some_and(|v| v == info.version.as_bytes());
                if already_handled {
                    // Downloaded and installed earlier this run; nothing to
                    // redo until the next actual restart picks it up.
                } else {
                    eprintln!("[agent] update {} found; downloading...", info.version);
                    let dl_dir = data_dir.join("update_dl");
                    let outcome = match updater.download(&info, &dl_dir, |_, _| {}).await {
                        Ok(path) => updater.install(&path, &data_dir, Some(&exe)),
                        Err(e) => Err(e),
                    };
                    match outcome {
                        Ok(outcome) => {
                            let _ = std::fs::remove_dir_all(&dl_dir);
                            let _ = db.set_setting(SETTING_DISMISSED_UPDATE, info.version.as_bytes());
                            eprintln!("[agent] update {}: {:?} — takes effect next start", info.version, outcome);
                        }
                        Err(e) => eprintln!("[agent] update {} failed: {e:?}", info.version),
                    }
                }
            }
            Ok(None) => {}
            Err(e) => eprintln!("[agent] update check failed: {e:?}"),
        }
        tokio::time::sleep(Duration::from_secs(UPDATE_CHECK_INTERVAL_SECS)).await;
    }
}

/// Queues the closing REVOKE and returns its row id with the deadline by
/// which its delivery is awaited. A send that fails to even queue is logged;
/// the deadline then just expires.
async fn begin_stop(session: &SessionManager, master_id: i64) -> (i64, tokio::time::Instant) {
    let id = match session
        .send_console(master_id, BODY_REVOKE.to_string(), WireConsole::new(CONSOLE_REVOKE), vec![])
        .await
    {
        Ok(id) => id,
        Err(e) => { eprintln!("[agent] could not queue REVOKE: {e}"); -1 }
    };
    (id, tokio::time::Instant::now() + REVOKE_WAIT)
}
