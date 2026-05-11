use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex};

use crate::crypto::Identity;
use crate::net::{NetError, TorNode};

#[derive(Serialize, Deserialize, Debug)]
pub enum ClientToRelay {
    Auth {
        sign_pk: [u8; 32],
        #[serde(with = "BigArray")]
        signature: [u8; 64],
    },
    Publish { bundle: Vec<u8> },
    GetBundle { pk: [u8; 32] },
    Send { to: [u8; 32], blob: Vec<u8> },
    Ack { id: u64 },
    Ping,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum RelayToClient {
    Challenge([u8; 32]),
    AuthOk,
    AuthFail,
    Bundle { pk: [u8; 32], bundle: Option<Vec<u8>> },
    Incoming { id: u64, from: [u8; 32], blob: Vec<u8> },
    Deposited { id: u64 },
    Pong,
    Error(String),
}

#[derive(Serialize, Deserialize, Debug)]
pub enum EnvelopeBlob {
    X3dhInit(crate::crypto::X3dhInitial),
    Ratchet { header: crate::crypto::RatchetHeader, ciphertext: Vec<u8> },
}

pub const RELAY_PORT: u16 = 443;
pub const MAX_FRAME: u32 = 16 * 1024 * 1024;
pub const DEFAULT_RELAY: &str = "jl6oxc6h3dr2s6y6y3mmrrpmormyxk5hahrurwfswcep7rxt2hsk5wid.onion";

pub type Result<T> = std::result::Result<T, RelayError>;

#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    #[error("net")] Net(#[from] NetError),
    #[error("io")] Io(#[from] std::io::Error),
    #[error("codec")] Codec,
    #[error("auth failed")] AuthFailed,
    #[error("protocol: {0}")] Proto(String),
    #[error("closed")] Closed,
}

impl From<bincode::Error> for RelayError { fn from(_: bincode::Error) -> Self { Self::Codec } }

pub struct RelayClient {
    pub out_tx: mpsc::Sender<ClientToRelay>,
    pub in_rx: Arc<Mutex<mpsc::Receiver<RelayToClient>>>,
}

pub async fn connect(
    node: &Arc<TorNode>,
    onion: &str,
    identity: &Arc<Identity>,
) -> Result<RelayClient> {
    let stream = node.connect_relay(onion, RELAY_PORT).await?;
    let mut stream = stream.into_inner();

    let challenge = match recv::<_, RelayToClient>(&mut stream).await? {
        RelayToClient::Challenge(c) => c,
        _ => return Err(RelayError::Proto("expected Challenge".into())),
    };
    let signature = identity.sign(&challenge);
    let sign_pk = identity.card().sign_pk;
    send(&mut stream, &ClientToRelay::Auth { sign_pk, signature }).await?;

    match recv::<_, RelayToClient>(&mut stream).await? {
        RelayToClient::AuthOk => {}
        RelayToClient::AuthFail => return Err(RelayError::AuthFailed),
        _ => return Err(RelayError::Proto("expected AuthOk".into())),
    }

    let (in_tx, in_rx) = mpsc::channel::<RelayToClient>(256);
    let (out_tx, mut out_rx) = mpsc::channel::<ClientToRelay>(256);

    let (mut read_half, mut write_half) = tokio::io::split(stream);

    tokio::spawn(async move {
        loop {
            match recv::<_, RelayToClient>(&mut read_half).await {
                Ok(f) => {
                    eprintln!("[relay-wire] recv {}", frame_kind(&f));
                    if in_tx.send(f).await.is_err() { eprintln!("[relay-wire] in_tx closed, recv-task exit"); break; }
                }
                Err(e) => { eprintln!("[relay-wire] recv err: {:?}, recv-task exit", e); break; }
            }
        }
    });

    tokio::spawn(async move {
        while let Some(f) = out_rx.recv().await {
            eprintln!("[relay-wire] send {}", out_kind(&f));
            if let Err(e) = send(&mut write_half, &f).await {
                eprintln!("[relay-wire] send err: {:?}, send-task exit", e);
                break;
            }
        }
    });

    Ok(RelayClient { out_tx, in_rx: Arc::new(Mutex::new(in_rx)) })
}

fn out_kind(f: &ClientToRelay) -> String {
    match f {
        ClientToRelay::Auth { .. } => "Auth".into(),
        ClientToRelay::Publish { bundle } => format!("Publish({}B)", bundle.len()),
        ClientToRelay::GetBundle { pk } => format!("GetBundle({})", hex_short(pk)),
        ClientToRelay::Send { to, blob } => format!("Send(to={}, {}B)", hex_short(to), blob.len()),
        ClientToRelay::Ack { id } => format!("Ack({})", id),
        ClientToRelay::Ping => "Ping".into(),
    }
}

fn frame_kind(f: &RelayToClient) -> String {
    match f {
        RelayToClient::Challenge(_) => "Challenge".into(),
        RelayToClient::AuthOk => "AuthOk".into(),
        RelayToClient::AuthFail => "AuthFail".into(),
        RelayToClient::Bundle { pk, bundle } => format!("Bundle(pk={}, present={})", hex_short(pk), bundle.is_some()),
        RelayToClient::Incoming { id, from, blob } => format!("Incoming(id={}, from={}, {}B)", id, hex_short(from), blob.len()),
        RelayToClient::Deposited { id } => format!("Deposited({})", id),
        RelayToClient::Pong => "Pong".into(),
        RelayToClient::Error(e) => format!("Error({})", e),
    }
}

fn hex_short(b: &[u8]) -> String {
    let mut s = String::new();
    for &x in &b[..8.min(b.len())] { s.push_str(&format!("{:02x}", x)); }
    s
}

async fn send<W, T>(w: &mut W, f: &T) -> Result<()>
where W: AsyncWrite + Unpin, T: serde::Serialize
{
    let data = bincode::serialize(f)?;
    if data.len() > MAX_FRAME as usize { return Err(RelayError::Proto("frame too large".into())); }
    w.write_all(&(data.len() as u32).to_be_bytes()).await?;
    w.write_all(&data).await?;
    w.flush().await?;
    Ok(())
}

async fn recv<R, T>(r: &mut R) -> Result<T>
where R: AsyncRead + Unpin, T: serde::de::DeserializeOwned
{
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME { return Err(RelayError::Proto("frame too large".into())); }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).await?;
    Ok(bincode::deserialize(&buf)?)
}