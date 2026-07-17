mod proto;
mod storage;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rand::RngCore;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, RwLock};
use yosemite::{style, DestinationKind, RouterApi, Session, SessionOptions};

use crate::proto::*;
use crate::storage::Storage;

type Connections = Arc<RwLock<HashMap<[u8; 32], mpsc::Sender<RelayToClient>>>>;

/// Default SAMv3 port. The relay is server-side infrastructure: run go-i2p (or
/// i2pd) as a system service exposing SAMv3 here (see gipny-relay.service).
const DEFAULT_SAM_PORT: u16 = 7656;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let data_dir = std::env::var("GIPNY_RELAY_DATA").unwrap_or_else(|_| "./relay-data".to_string());
    let data_dir = PathBuf::from(data_dir);
    std::fs::create_dir_all(&data_dir)?;

    let storage = Arc::new(Storage::open(&data_dir.join("relay.db"))?);
    let connections: Connections = Arc::new(RwLock::new(HashMap::new()));

    let sam_port: u16 = std::env::var("GIPNY_SAM_PORT").ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_SAM_PORT);

    eprintln!("[relay] connecting to SAMv3 bridge on 127.0.0.1:{sam_port}...");
    let (dest_pub, privkey) = load_or_create_identity(&data_dir, sam_port).await?;

    let opts = SessionOptions {
        nickname: "gipny-relay".to_string(),
        destination: DestinationKind::Persistent { private_key: privkey },
        samv3_tcp_port: sam_port,
        // Servers must publish their leaseSet so clients can reach them.
        publish: true,
        // Relay payloads are already E2E-encrypted/padded; SAM gzip is wasted work.
        gzip: false,
        ..Default::default()
    };
    let mut session = Session::<style::Stream>::new(opts).await
        .map_err(|e| anyhow::anyhow!("SAM session: {e}"))?;

    eprintln!("========================================================");
    eprintln!("[relay] I2P DESTINATION (bake into client DEFAULT_RELAY):");
    eprintln!("{dest_pub}");
    eprintln!("========================================================");

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

    loop {
        let stream = match session.accept().await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[relay] accept err: {e}");
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        let storage = storage.clone();
        let connections = connections.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, storage, connections).await {
                eprintln!("[relay] client disconnected: {}", e);
            }
        });
    }
}

/// Load the persistent i2p identity, generating it on first run.
/// Returns `(public_destination, private_key)`.
async fn load_or_create_identity(data_dir: &Path, sam_port: u16) -> anyhow::Result<(String, String)> {
    let key_path = data_dir.join("dest.key");
    let pub_path = data_dir.join("dest.pub");
    if let (Ok(k), Ok(p)) = (std::fs::read_to_string(&key_path), std::fs::read_to_string(&pub_path)) {
        let (k, p) = (k.trim().to_string(), p.trim().to_string());
        if !k.is_empty() && !p.is_empty() {
            return Ok((p, k));
        }
    }
    eprintln!("[relay] generating persistent destination (first run)...");
    let (dest, key) = RouterApi::new(sam_port).generate_destination().await
        .map_err(|e| anyhow::anyhow!("generate destination: {e}"))?;
    std::fs::write(&key_path, &key)?;
    std::fs::write(&pub_path, &dest)?;
    Ok((dest, key))
}

async fn handle_client<S>(
    mut stream: S,
    storage: Arc<Storage>,
    connections: Connections,
) -> anyhow::Result<()>
where S: AsyncRead + AsyncWrite + Unpin + Send
{
    let mut challenge = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut challenge);
    send_frame(&mut stream, &RelayToClient::Challenge(challenge)).await?;

    let auth: ClientToRelay = recv_frame(&mut stream).await?;
    let (sign_pk, signature) = match auth {
        ClientToRelay::Auth { sign_pk, signature } => (sign_pk, signature),
        _ => anyhow::bail!("expected Auth first"),
    };

    let vk = VerifyingKey::from_bytes(&sign_pk).map_err(|e| anyhow::anyhow!("bad pk: {}", e))?;
    let sig = Signature::from_bytes(&signature);
    if vk.verify(&challenge, &sig).is_err() {
        send_frame(&mut stream, &RelayToClient::AuthFail).await?;
        anyhow::bail!("bad signature");
    }
    send_frame(&mut stream, &RelayToClient::AuthOk).await?;
    eprintln!("[relay] auth ok {}", hex_short(&sign_pk));

    let (push_tx, mut push_rx) = mpsc::channel::<RelayToClient>(512);
    connections.write().await.insert(sign_pk, push_tx.clone());

    let cursor = Arc::new(tokio::sync::Mutex::new(0i64));
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

    let result = client_loop(&mut stream, &mut push_rx, &push_tx, sign_pk, &storage, &connections, cursor.clone()).await;
    refresh_handle.abort();
    connections.write().await.remove(&sign_pk);
    eprintln!("[relay] client gone {}", hex_short(&sign_pk));
    result
}

async fn client_loop<S>(
    stream: &mut S,
    push_rx: &mut mpsc::Receiver<RelayToClient>,
    push_tx: &mpsc::Sender<RelayToClient>,
    sign_pk: [u8; 32],
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
                        storage.ack(id as i64)?;
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
                    ClientToRelay::Auth { .. } => {}
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
    let data = bincode::serialize(frame)?;
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
    Ok(bincode::deserialize(&buf)?)
}

fn hex_short(b: &[u8]) -> String {
    let mut s = String::new();
    for &x in &b[..8.min(b.len())] { s.push_str(&format!("{:02x}", x)); }
    s
}