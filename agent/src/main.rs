//! gipny-agent — headless console agent.
//!
//! Runs a Gipny identity, registers a master contact by their card, and
//! executes shell commands that the master sends over the encrypted channel.
//! Everything rides the ordinary message pipeline: commands arrive as
//! CONSOLE_COMMAND messages, replies go back as CONSOLE_OUTPUT messages.
//!
//! Usage:
//!   gipny-agent --data <dir> --master <gipny:v2:…card…>
//!
//! Optional flags:
//!   --name <display name>   what the agent calls itself (default: hostname)
//!   --relay <onion>         relay to use; defaults to master card's relay (v2) or DEFAULT_RELAY
//!   --sam <port>            attach to a running SAM bridge on this port instead of
//!                           spawning our own router (for dev/CI)
//!   --cwd <dir>             working directory for commands (default: home dir)
//!   --timeout <secs>        per-command timeout (default: 120 s)
//!
//! The agent prints its own contact card to stdout on start so the master can
//! add it to their contacts. Agent mode is turned on automatically for the
//! master contact.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::signal;

use gipny_libcore::{
    ContactCard, Db, I2pNode, SessionManager, SessionEvent,
    WireConsole, CONSOLE_COMMAND, CONSOLE_GRANT, CONSOLE_OFF, CONSOLE_REVOKE,
    DEFAULT_RELAY,
};
use gipny_libcore::agent::{self, ExecOptions, BODY_GRANT, BODY_REVOKE};
use gipny_libcore::router::RouterSettings;
use gipny_libcore::crypto::AttachmentCipher;

fn usage() -> ! {
    eprintln!(
        "Usage: gipny-agent --data <dir> --master <card> [--name <name>] \
        [--relay <onion>] [--sam <port>] [--cwd <dir>] [--timeout <secs>]"
    );
    std::process::exit(1);
}

#[derive(Default)]
struct Args {
    data: Option<PathBuf>,
    master_card: Option<String>,
    name: Option<String>,
    relay: Option<String>,
    sam_port: Option<u16>,
    cwd: Option<PathBuf>,
    timeout_secs: Option<u64>,
}

fn parse_args() -> Args {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1).peekable();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--data"    => a.data        = it.next().map(PathBuf::from),
            "--master"  => a.master_card = it.next(),
            "--name"    => a.name        = it.next(),
            "--relay"   => a.relay       = it.next(),
            "--sam"     => a.sam_port    = it.next().and_then(|s| s.parse().ok()),
            "--cwd"     => a.cwd         = it.next().map(PathBuf::from),
            "--timeout" => a.timeout_secs = it.next().and_then(|s| s.parse().ok()),
            "--help" | "-h" => usage(),
            other => { eprintln!("unknown flag: {other}"); usage(); }
        }
    }
    a
}

/// Load the encrypted attachments for a message from the data directory.
fn load_attachments(
    db: &Db,
    data_dir: &PathBuf,
    msg_id: i64,
) -> Vec<(String, Vec<u8>)> {
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
        let enc = match std::fs::read(&path) {
            Ok(e) => e,
            Err(e) => { eprintln!("[agent] read {}: {e}", path.display()); continue; }
        };
        match AttachmentCipher::from_key(key).decrypt_chunk(0, &[], &enc) {
            Ok(data) => out.push((a.name, data)),
            Err(e) => eprintln!("[agent] decrypt {}: {e:?}", path.display()),
        }
    }
    out
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args();
    let data_dir = args.data.context("--data is required")?;
    let master_raw = args.master_card.context("--master is required")?;

    // Parse the master's contact card.
    let master_card = ContactCard::parse(&master_raw)
        .context("invalid --master card")?;

    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("create data dir {}", data_dir.display()))?;

    // Open a plain (unencrypted) database. The agent is a headless daemon and
    // the data dir should be on an encrypted filesystem or in a private path.
    let db = Arc::new(
        Db::open_plain(&data_dir.join("agent.db"))
            .context("open agent database")?,
    );

    // If --sam was given, point I2pNode::start at that port via env var
    // (it reads GIPNY_SAM_PORT to attach instead of spawning a new router).
    if let Some(port) = args.sam_port {
        std::env::set_var("GIPNY_SAM_PORT", port.to_string());
        eprintln!("[agent] attaching to SAM on port {port}");
    } else {
        eprintln!("[agent] starting i2p router…");
    }

    // Start the i2p node (spawns the router or attaches via GIPNY_SAM_PORT).
    let node = I2pNode::start(&data_dir, RouterSettings::default())
        .await
        .context("start i2p node")?;
    eprintln!("[agent] i2p address: {}", node.onion_address());
    let node = Arc::new(node);


    // Determine the relay: explicit flag > master card > DEFAULT_RELAY.
    let relay_onion = args.relay
        .or_else(|| master_card.relay.clone())
        .unwrap_or_else(|| DEFAULT_RELAY.to_string());
    eprintln!("[agent] relay: {relay_onion}");

    // Set display name before starting the session so the relay sees it.
    let display_name = args.name
        .unwrap_or_else(|| agent::hostname());

    // Start the session manager.
    let (session, mut events) = SessionManager::start(
        data_dir.clone(),
        db.clone(),
        node.clone(),
    )
    .await
    .context("start session manager")?;
    let _ = session.set_display_name(&display_name);
    let _ = session.set_relay_onion(&relay_onion);
    let session = Arc::new(session);

    // Print our own card so the master can add us.
    let my_card = session.my_card();
    let my_onion = node.onion_address().to_string();
    let sign_hex: String = my_card.sign_pk.iter().map(|b| format!("{b:02x}")).collect();
    let dh_hex: String = my_card.dh_pk.iter().map(|b| format!("{b:02x}")).collect();
    let card_str = format!("gipny:v2:{my_onion}:{sign_hex}:{dh_hex}:{relay_onion}");
    println!("agent card: {card_str}");
    eprintln!("[agent] master sign_pk: {}", agent::hex8(&master_card.sign_pk));

    // Add (or re-use) the master as a contact and enable agent mode.
    let master_id = session
        .add_contact(
            &master_card.identity(),
            &master_card.onion,
            master_card.name.as_deref().unwrap_or("master"),
        )
        .await
        .context("register master contact")?;
    eprintln!("[agent] master contact_id = {master_id}");

    // Tell the master we are ready.
    session
        .send_console(master_id, BODY_GRANT.to_string(), WireConsole::new(CONSOLE_GRANT), vec![])
        .await
        .context("send GRANT to master")?;
    eprintln!("[agent] ready — waiting for commands");

    let exec_opts = ExecOptions {
        timeout: Duration::from_secs(args.timeout_secs.unwrap_or(120)),
        cwd: args.cwd,
        ..Default::default()
    };

    // Event loop.
    let session2 = session.clone();
    let db2 = db.clone();
    let data_dir2 = data_dir.clone();
    let opts2 = exec_opts.clone();
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::IncomingPayload { contact_id, payload, message_id } => {
                    if contact_id != master_id {
                        // Not from our master — ignore.
                        continue;
                    }
                    // Check if this is a console-framed message.
                    let console_bytes = db2
                        .get_setting(&format!("console_{message_id}"))
                        .ok()
                        .flatten();
                    let Some(cb) = console_bytes else { continue; };
                    let console: WireConsole = match bincode::deserialize(&cb) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };
                    match console.kind {
                        CONSOLE_COMMAND => {}
                        CONSOLE_OFF => {
                            eprintln!("[agent] master sent CONSOLE_OFF — exiting");
                            std::process::exit(0);
                        }
                        _ => continue,
                    }

                    // Load the attachments that arrived with the command.
                    let attachments = load_attachments(&db2, &data_dir2, message_id);
                    let body = payload.body.clone();
                    let s = session2.clone();
                    let o = opts2.clone();
                    tokio::spawn(async move {
                        eprintln!("[agent] command: {body:?}");
                        let reply = agent::handle_console_request(&body, &attachments, &o).await;
                        let reply_atts = reply.attachments; // Vec<(String, Vec<u8>)>
                        if let Err(e) = s
                            .send_console(master_id, reply.body, reply.console, reply_atts)
                            .await
                        {
                            eprintln!("[agent] send reply failed: {e}");
                        }
                    });
                }
                _ => {}
            }
        }
    });

    // Wait for Ctrl-C.
    signal::ctrl_c().await.context("signal handler")?;
    eprintln!("[agent] interrupted — notifying master and exiting");
    let _ = session
        .send_console(master_id, BODY_REVOKE.to_string(), WireConsole::new(CONSOLE_REVOKE), vec![])
        .await;
    session.shutdown();
    Ok(())
}
