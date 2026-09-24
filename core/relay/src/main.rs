mod dht;
mod dht_store;
mod proto;
mod storage;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rand::Rng;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, RwLock};

use crate::dht::DhtHandler;
use crate::proto::*;
use crate::storage::Storage;

type Connections = Arc<RwLock<HashMap<[u8; 32], mpsc::Sender<RelayToClient>>>>;


/// A relay-network connection is closed after this many requests, or when it
/// sits idle this long (same limits as libcore's relay_server).
const DHT_MAX_REQUESTS: usize = 64;
const DHT_IDLE: Duration = Duration::from_secs(60);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // `--dht` makes the relay a node of the relay network (a seed, when its
    // destination is baked into releases as GIPNY_DHT_SEEDS). `--no-store`
    // keeps it a router-only node; `--seeds` or GIPNY_DHT_SEEDS name other
    // nodes to join through.
    let mut dht_on = false;
    let mut dht_stores = true;
    let mut seeds = split_list(&std::env::var("GIPNY_DHT_SEEDS").unwrap_or_default());
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dht" => dht_on = true,
            "--no-store" => dht_stores = false,
            "--seeds" => seeds.extend(split_list(&args.next().unwrap_or_default())),
            other => anyhow::bail!("unknown argument {other:?} (expected --dht, --no-store, --seeds LIST)"),
        }
    }

    let data_dir = std::env::var("GIPNY_RELAY_DATA").unwrap_or_else(|_| "./relay-data".to_string());
    let data_dir = PathBuf::from(data_dir);
    std::fs::create_dir_all(&data_dir)?;

    let storage = Arc::new(Storage::open(&data_dir.join("relay.db"))?);
    let connections: Connections = Arc::new(RwLock::new(HashMap::new()));

    // The i2p router runs inside this process (i2p-embed): no SAM, no router
    // to install beside it, no local port. Its state lives in the data dir.
    let router_dir = data_dir.join("router");
    std::fs::create_dir_all(&router_dir)?;
    eprintln!("[relay] starting the i2p router in-process ({})...", router_dir.display());
    // GIPNY_I2P_LOGLEVEL: diagnostics only (CI's e2e), i2pd's own log level.
    let loglevel = std::env::var("GIPNY_I2P_LOGLEVEL").ok()
        .filter(|l| matches!(l.as_str(), "critical" | "error" | "warn" | "info" | "debug"))
        .map(|l| format!("--loglevel={l}"));
    let router = Arc::new(i2p_embed::Router::start(&[
        format!("--datadir={}", router_dir.display()),
        loglevel.unwrap_or_else(|| "--loglevel=warn".into()),
        "--sam.enabled=false".into(),
        "--http.enabled=false".into(),
        "--httpproxy.enabled=false".into(),
        "--socksproxy.enabled=false".into(),
        "--upnp.enabled=false".into(),
        // Against the reseed certificates i2p-embed compiles in.
        "--reseed.verify=true".into(),
    ], router_dir.join("i2pd.log").to_str()).map_err(|e| anyhow::anyhow!("i2p router: {e}"))?);

    let (dest_pub, privkey) = load_or_create_identity(&data_dir)?;

    let destination_hash = destination_hash(&dest_pub)
        .ok_or_else(|| anyhow::anyhow!("dest.pub is not an i2p destination"))?;
    eprintln!("========================================================");
    eprintln!("[relay] I2P DESTINATION (bake into client DEFAULT_RELAY):");
    eprintln!("{dest_pub}");
    eprintln!("========================================================");
    let dest = i2p_embed::Destination::new(&router, Some(&privkey), &i2p_embed::DestinationOptions { publish: true, ..Default::default() })
        .map_err(|e| anyhow::anyhow!("relay destination: {e}"))?;
    dest.ready(Duration::from_secs(1800)).await.map_err(|e| anyhow::anyhow!("relay tunnels: {e}"))?;
    eprintln!("[relay] tunnels up; accepting");

    let dht = if dht_on {
        Some(dht::start(&data_dir, router.clone(), &dest_pub, dht::Options { stores: dht_stores, seeds })?)
    } else {
        None
    };

    let storage_gc = storage.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3600));
        tick.tick().await;
        loop {
            tick.tick().await;
            if let Err(e) = storage_gc.gc() { eprintln!("[relay] gc err: {}", e); }
            if let Ok((b, m)) = storage_gc.stats() {
                eprintln!("[relay] stats: bundles={} pending_messages={}", b, m);
            }
        }
    });

    let mut inbound = dest.accept();
    let stop = shutdown_signal();
    tokio::pin!(stop);
    loop {
        tokio::select! {
            stream = inbound.recv() => {
                let Some(stream) = stream else { anyhow::bail!("the relay destination stopped accepting") };
                let storage = storage.clone();
                let connections = connections.clone();
                let dht = dht.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_client(stream, storage, connections, destination_hash, dht).await {
                        eprintln!("[relay] client disconnected: {}", e);
                    }
                });
            }
            // systemd's stop, or Ctrl-C: return, so the router in this process
            // is stopped before exit — its netDb written out, its LeaseSet
            // no longer served — rather than killed mid-flight.
            () = &mut stop => {
                eprintln!("[relay] stopping");
                return Ok(());
            }
        }
    }
}

/// SIGTERM or SIGINT (Ctrl-C on Windows).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let (Ok(mut term), Ok(mut int)) = (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) else {
            return std::future::pending().await;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Load the persistent i2p identity, generating it on first run.
/// Returns `(public_destination, private_key)`.
fn load_or_create_identity(data_dir: &Path) -> anyhow::Result<(String, String)> {
    let key_path = data_dir.join("dest.key");
    let pub_path = data_dir.join("dest.pub");
    if let (Ok(k), Ok(p)) = (std::fs::read_to_string(&key_path), std::fs::read_to_string(&pub_path)) {
        let (k, p) = (k.trim().to_string(), p.trim().to_string());
        if !k.is_empty() && !p.is_empty() {
            return Ok((p, k));
        }
    }
    eprintln!("[relay] generating persistent destination (first run)...");
    // Same format SAM's DEST GENERATE wrote, so a relay made before keeps its
    // address.
    let key = i2p_embed::generate_keys();
    let dest = i2p_embed::public_of(&key).map_err(|e| anyhow::anyhow!("generate destination: {e}"))?;
    std::fs::write(&key_path, &key)?;
    std::fs::write(&pub_path, &dest)?;
    Ok((dest, key))
}

/// Plain `Auth` may deposit and fetch bundles only: its signature covers no
/// relay in particular, so another relay could have passed it on.
async fn handle_client<S>(
    mut stream: S,
    storage: Arc<Storage>,
    connections: Connections,
    destination_hash: [u8; 32],
    dht: Option<DhtHandler>,
) -> anyhow::Result<()>
where S: AsyncRead + AsyncWrite + Unpin + Send
{
    let mut challenge = [0u8; 32];
    rand::rng().fill_bytes(&mut challenge);
    send_frame(&mut stream, &RelayToClient::Challenge(challenge)).await?;

    let auth: ClientToRelay = recv_frame(&mut stream).await?;
    let (sign_pk, signature, signed, owner) = match auth {
        ClientToRelay::AuthV2 { sign_pk, signature } => {
            (sign_pk, signature, auth_v2_message(&destination_hash, &challenge), true)
        }
        ClientToRelay::Auth { sign_pk, signature } => (sign_pk, signature, challenge.to_vec(), false),
        ClientToRelay::Dht(first) => return serve_dht(stream, challenge, first, dht).await,
        _ => anyhow::bail!("expected Auth first"),
    };

    let vk = VerifyingKey::from_bytes(&sign_pk).map_err(|e| anyhow::anyhow!("bad pk: {}", e))?;
    let sig = Signature::from_bytes(&signature);
    if vk.verify(&signed, &sig).is_err() {
        send_frame(&mut stream, &RelayToClient::AuthFail).await?;
        anyhow::bail!("bad signature");
    }
    send_frame(&mut stream, &RelayToClient::AuthOk).await?;
    eprintln!("[relay] auth ok {} ({})", hex_short(&sign_pk), if owner { "v2" } else { "v1, deposit only" });

    let (push_tx, mut push_rx) = mpsc::channel::<RelayToClient>(512);
    let cursor = Arc::new(tokio::sync::Mutex::new(0i64));
    if !owner {
        return client_loop(&mut stream, &mut push_rx, &push_tx, sign_pk, false, &storage, &connections, cursor).await;
    }
    connections.write().await.insert(sign_pk, push_tx.clone());

    let storage_init = storage.clone();
    let push_tx_init = push_tx.clone();
    let cursor_init = cursor.clone();
    tokio::spawn(async move {
        match storage_init.pending_for(&sign_pk) {
            Ok(pending) => {
                if !pending.is_empty() {
                    eprintln!("[relay] pushing {} pending to {}", pending.len(), hex_short(&sign_pk));
                }
                for (id, blob) in pending {
                    {
                        let mut c = cursor_init.lock().await;
                        if id > *c { *c = id; }
                    }
                    if push_tx_init.send(RelayToClient::Incoming { id: id as u64, from: [0u8; 32], blob }).await.is_err() {
                        break;
                    }
                }
            }
            Err(e) => eprintln!("[relay] pending_for err: {}", e),
        }
    });

    let storage_refresh = storage.clone();
    let push_tx_refresh = push_tx.clone();
    let cursor_refresh = cursor.clone();
    let refresh_handle = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        tick.tick().await;
        loop {
            tick.tick().await;
            let cur = *cursor_refresh.lock().await;
            match storage_refresh.pending_above(&sign_pk, cur) {
                Ok(more) => {
                    for (mid, blob) in more {
                        if push_tx_refresh.try_send(RelayToClient::Incoming { id: mid as u64, from: [0u8; 32], blob }).is_err() {
                            break;
                        }
                    }
                }
                Err(_) => {}
            }
        }
    });

    let result = client_loop(&mut stream, &mut push_rx, &push_tx, sign_pk, true, &storage, &connections, cursor.clone()).await;
    refresh_handle.abort();
    // Only our own entry. A client that reconnects registers a new sender under
    // the same key; when the old connection finally errors out, removing by key
    // would drop the new one and leave the recipient unreachable for live push.
    {
        let mut conns = connections.write().await;
        if conns.get(&sign_pk).is_some_and(|tx| tx.same_channel(&push_tx)) {
            conns.remove(&sign_pk);
        }
    }
    eprintln!("[relay] client gone {}", hex_short(&sign_pk));
    result
}

/// A relay-network connection: request, answer, until the other side is done.
/// It never logs in, so nothing here touches the mailbox or `Connections`.
async fn serve_dht<S>(mut stream: S, challenge: [u8; 32], first: Vec<u8>, dht: Option<DhtHandler>) -> anyhow::Result<()>
where S: AsyncRead + AsyncWrite + Unpin + Send
{
    let Some(dht) = dht else {
        send_frame(&mut stream, &RelayToClient::Error("not a relay-network node".into())).await?;
        return Ok(());
    };
    let mut request = first;
    for served in 1.. {
        let answer = tokio::task::spawn_blocking({
            let dht = dht.clone();
            move || dht(&challenge, request)
        })
        .await?;
        send_frame(&mut stream, &RelayToClient::Dht(answer)).await?;
        if served >= DHT_MAX_REQUESTS {
            return Ok(());
        }
        request = match tokio::time::timeout(DHT_IDLE, recv_frame::<_, ClientToRelay>(&mut stream)).await {
            Err(_) => return Ok(()),
            Ok(Err(e)) if e.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == std::io::ErrorKind::UnexpectedEof) => return Ok(()),
            Ok(Err(e)) => return Err(e),
            Ok(Ok(ClientToRelay::Dht(bytes))) => bytes,
            Ok(Ok(_)) => anyhow::bail!("only relay-network frames after one"),
        };
    }
    Ok(())
}

async fn client_loop<S>(
    stream: &mut S,
    push_rx: &mut mpsc::Receiver<RelayToClient>,
    push_tx: &mpsc::Sender<RelayToClient>,
    sign_pk: [u8; 32],
    owner: bool,
    storage: &Arc<Storage>,
    connections: &Connections,
    cursor: Arc<tokio::sync::Mutex<i64>>,
) -> anyhow::Result<()>
where S: AsyncRead + AsyncWrite + Unpin + Send
{
    loop {
        tokio::select! {
            frame = recv_frame::<_, ClientToRelay>(stream) => {
                let frame = frame?;
                match frame {
                    ClientToRelay::Publish { .. } | ClientToRelay::Ack { .. } if !owner => {
                        send_frame(stream, &RelayToClient::Error(ERR_NEEDS_AUTH_V2.into())).await?;
                    }
                    ClientToRelay::Publish { bundle } => {
                        storage.store_bundle(&sign_pk, &bundle)?;
                    }
                    ClientToRelay::GetBundle { pk } => {
                        let bundle = storage.get_bundle(&pk)?;
                        send_frame(stream, &RelayToClient::Bundle { pk, bundle }).await?;
                    }
                    ClientToRelay::Send { to, blob } => {
                        let id = storage.deposit(&to, &blob)?;
                        send_frame(stream, &RelayToClient::Deposited { id: id as u64 }).await?;
                        let tx_opt = connections.read().await.get(&to).cloned();
                        if let Some(tx) = tx_opt {
                            let pkt = RelayToClient::Incoming { id: id as u64, from: [0u8; 32], blob };
                            tokio::spawn(async move { let _ = tx.send(pkt).await; });
                        }
                    }
                    ClientToRelay::Ack { id } => {
                        storage.ack(&sign_pk, id as i64)?;
                        let cur_val = *cursor.lock().await;
                        match storage.pending_above(&sign_pk, cur_val) {
                            Ok(more) => {
                                for (mid, blob) in more {
                                    {
                                        let mut c = cursor.lock().await;
                                        if mid > *c { *c = mid; }
                                    }
                                    if push_tx.try_send(RelayToClient::Incoming { id: mid as u64, from: [0u8; 32], blob }).is_err() {
                                        break;
                                    }
                                }
                            }
                            Err(e) => eprintln!("[relay] pending_above err: {}", e),
                        }
                    }
                    ClientToRelay::Ping => {
                        send_frame(stream, &RelayToClient::Pong).await?;
                    }
                    ClientToRelay::Auth { .. } | ClientToRelay::AuthV2 { .. } | ClientToRelay::Dht(_) => {}
                }
            }
            push = push_rx.recv() => {
                let Some(msg) = push else { break };
                if let RelayToClient::Incoming { id, .. } = &msg {
                    let i = *id as i64;
                    let mut c = cursor.lock().await;
                    if i > *c { *c = i; }
                }
                send_frame(stream, &msg).await?;
            }
        }
    }
    Ok(())
}

async fn send_frame<W, T>(w: &mut W, frame: &T) -> anyhow::Result<()>
where W: AsyncWrite + Unpin, T: serde::Serialize
{
    let data = bincode::serde::encode_to_vec(frame, bincode::config::legacy())?;
    if data.len() > MAX_FRAME as usize { anyhow::bail!("frame too large"); }
    w.write_all(&(data.len() as u32).to_be_bytes()).await?;
    w.write_all(&data).await?;
    w.flush().await?;
    Ok(())
}

async fn recv_frame<R, T>(r: &mut R) -> anyhow::Result<T>
where R: AsyncRead + Unpin, T: serde::de::DeserializeOwned
{
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME { anyhow::bail!("frame too large"); }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).await?;
    let (val, _) = bincode::serde::decode_from_slice(&buf, bincode::config::legacy())?;
    Ok(val)
}

fn hex_short(b: &[u8]) -> String {
    let mut s = String::new();
    for &x in &b[..8.min(b.len())] { s.push_str(&format!("{:02x}", x)); }
    s
}

fn split_list(s: &str) -> Vec<String> {
    s.split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()).map(str::to_string).collect()
}
