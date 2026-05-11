mod proto;
mod storage;

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use arti_client::config::onion_service::OnionServiceConfigBuilder;
use arti_client::config::CfgPath;
use arti_client::{TorClient, TorClientConfig};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use futures::{Stream, StreamExt};
use rand::RngCore;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, RwLock};
use tor_cell::relaycell::msg::Connected;
use tor_hsservice::{HsNickname, RendRequest};
use tor_rtcompat::PreferredRuntime;

use crate::proto::*;
use crate::storage::Storage;

type Runtime = PreferredRuntime;
type Connections = Arc<RwLock<HashMap<[u8; 32], mpsc::Sender<RelayToClient>>>>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let data_dir = std::env::var("GIPNY_RELAY_DATA").unwrap_or_else(|_| "./relay-data".to_string());
    let data_dir = PathBuf::from(data_dir);
    std::fs::create_dir_all(&data_dir)?;

    let storage = Arc::new(Storage::open(&data_dir.join("relay.db"))?);
    let connections: Connections = Arc::new(RwLock::new(HashMap::new()));

    let mut cfg = TorClientConfig::builder();
    cfg.storage().cache_dir(CfgPath::new_literal(data_dir.join("tor/cache")));
    cfg.storage().state_dir(CfgPath::new_literal(data_dir.join("tor/state")));
    cfg.storage().permissions().dangerously_trust_everyone();
    let cfg = cfg.build().map_err(|e| anyhow::anyhow!(e.to_string()))?;

    eprintln!("[relay] bootstrapping tor (first run 60-120s)...");
    let client: TorClient<Runtime> = TorClient::create_bootstrapped(cfg).await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    eprintln!("[relay] bootstrapped");

    let nickname: HsNickname = HS_NICKNAME.parse::<HsNickname>()
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let hs_cfg = OnionServiceConfigBuilder::default()
        .nickname(nickname).build()
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let (service, rend_stream) = client.launch_onion_service(hs_cfg)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    eprintln!("[relay] waiting for onion address (can take 1-3min on first run)...");
    let deadline = std::time::Instant::now() + Duration::from_secs(300);
    let onion = loop {
        if let Some(n) = service.onion_name() { break n.to_string(); }
        if std::time::Instant::now() > deadline { anyhow::bail!("onion address timeout"); }
        tokio::time::sleep(Duration::from_secs(2)).await;
    };

    eprintln!("========================================================");
    eprintln!("[relay] ONION ADDRESS: {}", onion);
    eprintln!("[relay] put this in gipny client settings → relay");
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

    let mut rend_stream: Pin<Box<dyn Stream<Item = RendRequest> + Send>> = Box::pin(rend_stream);
    while let Some(rend) = rend_stream.next().await {
        let storage = storage.clone();
        let connections = connections.clone();
        tokio::spawn(async move {
            let mut streams = match rend.accept().await { Ok(s) => s, Err(_) => return };
            while let Some(sr) = streams.next().await {
                let Ok(ds) = sr.accept(Connected::new_empty()).await else { continue };
                let storage = storage.clone();
                let connections = connections.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_client(ds, storage, connections).await {
                        eprintln!("[relay] client disconnected: {}", e);
                    }
                });
            }
        });
    }
    Ok(())
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