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
    /// Like `Auth`, but the signature covers [`auth_v2_message`]: bound to
    /// this relay's destination, so a relay cannot pass on a challenge from
    /// another one and log in there as its client. Only this grants
    /// collecting and publishing; plain `Auth` is kept for depositing into
    /// relays older clients reach.
    AuthV2 {
        sign_pk: [u8; 32],
        #[serde(with = "BigArray")]
        signature: [u8; 64],
    },
}

const AUTH_V2_CONTEXT: &[u8] = b"gipny-relay-auth-v2";

/// What an `AuthV2` signature covers.
pub fn auth_v2_message(destination_hash: &[u8; 32], challenge: &[u8; 32]) -> Vec<u8> {
    [AUTH_V2_CONTEXT, destination_hash, challenge].concat()
}

/// SHA-256 of a destination's binary form — the value its `.b32.i2p` name
/// encodes — from either spelling of the address. Both sides of `AuthV2`
/// derive it, so they have to agree whichever one a client was given.
pub fn destination_hash(address: &str) -> Option<[u8; 32]> {
    let a = address.trim();
    if let Some(host) = a.strip_suffix(".b32.i2p").or_else(|| a.strip_suffix(".B32.I2P")) {
        return base32_decode(host)?.try_into().ok();
    }
    let raw = i2p_base64_decode(a)?;
    use sha2::Digest;
    Some(sha2::Sha256::digest(&raw).into())
}

fn i2p_base64_decode(s: &str) -> Option<Vec<u8>> {
    let body = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in body.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'~' => 63,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c.to_ascii_lowercase() {
            c @ b'a'..=b'z' => c - b'a',
            c @ b'2'..=b'7' => c - b'2' + 26,
            _ => return None,
        };
        acc = (acc << 5) | v as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// What a relay answers, in an `Error` frame, to a publish or a deposit for a
/// key it does not hold mail for: the relay built into somebody's app serves
/// its owner and nobody else. A client reads it to tell "wrong relay" from a
/// passing fault.
pub const ERR_NOT_SERVED: &str = "this relay does not serve that recipient";

/// The answer to publishing or acking after a plain `Auth` login.
pub const ERR_NEEDS_AUTH_V2: &str = "log in with AuthV2 to collect or publish";

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
// TODO(i2p): bake in the deployed relay's i2p destination (the base64 string the
// relay server prints as "I2P DESTINATION" on first run). Empty until deployed —
// users can meanwhile paste a relay destination in Settings (overrides this).
pub const DEFAULT_RELAY: &str = "";

pub type Result<T> = std::result::Result<T, RelayError>;

#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    #[error("net")] Net(#[from] NetError),
    #[error("io")] Io(#[from] std::io::Error),
    #[error("codec")] Codec,
    #[error("auth failed")] AuthFailed,
    #[error("protocol: {0}")] Proto(String),
    #[error("closed")] Closed,
    /// The relay hung up on `AuthV2` — what a relay from before it does.
    #[error("relay predates AuthV2")] AuthV2Unsupported,
}

impl From<bincode::Error> for RelayError { fn from(_: bincode::Error) -> Self { Self::Codec } }

pub struct RelayClient {
    pub out_tx: mpsc::Sender<ClientToRelay>,
    pub in_rx: Arc<Mutex<mpsc::Receiver<RelayToClient>>>,
}

/// Connect to our own relay. Failures count toward SAM session health.
pub async fn connect(
    node: &Arc<TorNode>,
    onion: &str,
    identity: &Arc<Identity>,
) -> Result<RelayClient> {
    let Some(hash) = destination_hash(onion) else {
        let stream = node.connect_relay(onion, RELAY_PORT).await?;
        return handshake(stream.into_inner(), identity, None).await;
    };
    let stream = node.connect_relay(onion, RELAY_PORT).await?;
    match handshake(stream.into_inner(), identity, Some(&hash)).await {
        Err(e) if predates_auth_v2(&e) => {
            let stream = node.connect_relay(onion, RELAY_PORT).await?;
            handshake(stream.into_inner(), identity, None).await
        }
        r => r,
    }
}

/// Connect to a contact's relay, to deposit a message where they collect.
///
/// Unlike [`connect`], a failure here says nothing about our own session: a
/// contact's relay may be offline, retired, or an ephemeral one that died with
/// its app. Counting those toward session health let five unreachable contact
/// relays tear down a session that was carrying our own traffic fine, and do it
/// again every cooldown.
pub async fn connect_peer(
    node: &Arc<TorNode>,
    onion: &str,
    identity: &Arc<Identity>,
) -> Result<RelayClient> {
    let Some(hash) = destination_hash(onion) else {
        let stream = node.connect_service(onion, RELAY_PORT).await?;
        return handshake(stream.into_inner(), identity, None).await;
    };
    let stream = node.connect_service(onion, RELAY_PORT).await?;
    match handshake(stream.into_inner(), identity, Some(&hash)).await {
        Err(e) if predates_auth_v2(&e) => {
            let stream = node.connect_service(onion, RELAY_PORT).await?;
            handshake(stream.into_inner(), identity, None).await
        }
        r => r,
    }
}

/// Retrying with plain `Auth` is safe to allow: a current relay grants that
/// nothing but depositing, so a relay that fakes being old gains nothing.
fn predates_auth_v2(e: &RelayError) -> bool {
    matches!(e, RelayError::AuthV2Unsupported)
}

async fn handshake(
    mut stream: std::pin::Pin<Box<dyn crate::net::DuplexStream>>,
    identity: &Arc<Identity>,
    destination_hash: Option<&[u8; 32]>,
) -> Result<RelayClient> {
    let challenge = match recv::<_, RelayToClient>(&mut stream).await? {
        RelayToClient::Challenge(c) => c,
        _ => return Err(RelayError::Proto("expected Challenge".into())),
    };
    let sign_pk = identity.card().sign_pk;
    let auth = match destination_hash {
        Some(hash) => ClientToRelay::AuthV2 { sign_pk, signature: identity.sign(&auth_v2_message(hash, &challenge)) },
        None => ClientToRelay::Auth { sign_pk, signature: identity.sign(&challenge) },
    };
    send(&mut stream, &auth).await?;

    let answer = match recv::<_, RelayToClient>(&mut stream).await {
        Err(RelayError::Io(_) | RelayError::Codec) if destination_hash.is_some() => {
            return Err(RelayError::AuthV2Unsupported);
        }
        r => r?,
    };
    match answer {
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
        ClientToRelay::AuthV2 { .. } => "AuthV2".into(),
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

pub(crate) async fn send<W, T>(w: &mut W, f: &T) -> Result<()>
where W: AsyncWrite + Unpin, T: serde::Serialize
{
    let data = bincode::serialize(f)?;
    if data.len() > MAX_FRAME as usize { return Err(RelayError::Proto("frame too large".into())); }
    w.write_all(&(data.len() as u32).to_be_bytes()).await?;
    w.write_all(&data).await?;
    w.flush().await?;
    Ok(())
}

pub(crate) async fn recv<R, T>(r: &mut R) -> Result<T>
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
/// Wire-format pins, shared with core/relay.
///
/// core/relay encodes with bincode 2 in `legacy()` mode; this crate uses
/// bincode 1. They must agree to the byte, and both the standalone relay and
/// [`crate::relay_server`] must agree with the clients. These are the same
/// golden bytes as core/relay/src/proto.rs, checked against bincode 1 here, so
/// the enums in the two crates cannot drift apart without one side failing:
///   - enum variant: u32 little-endian
///   - integer fields: little-endian fixed-width
///   - Vec<u8>: u64 LE length prefix + raw bytes
///   - [u8; N]: raw N bytes (no length prefix)
///   - Option<T>: u8 (0=None / 1=Some) then T if present
///
/// New variants go at the end of an enum, in both crates, with a test here.
#[cfg(test)]
mod wire_compat {
    use super::*;

    fn enc<T: Serialize>(v: &T) -> Vec<u8> {
        bincode::serialize(v).expect("encode")
    }

    fn dec<T: serde::de::DeserializeOwned>(buf: &[u8]) -> T {
        bincode::deserialize(buf).expect("decode")
    }

    #[test]
    fn unit_variants() {
        assert_eq!(enc(&ClientToRelay::Ping), [0x05, 0, 0, 0]);
        assert_eq!(enc(&RelayToClient::AuthOk), [0x01, 0, 0, 0]);
        assert_eq!(enc(&RelayToClient::AuthFail), [0x02, 0, 0, 0]);
        assert_eq!(enc(&RelayToClient::Pong), [0x06, 0, 0, 0]);
        assert!(matches!(dec::<ClientToRelay>(&[0x05, 0, 0, 0]), ClientToRelay::Ping));
        assert!(matches!(dec::<RelayToClient>(&[0x06, 0, 0, 0]), RelayToClient::Pong));
    }

    #[test]
    fn u64_fields() {
        let id = 0x0102_0304_0506_0708;
        #[rustfmt::skip]
        let le = [0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01];
        assert_eq!(enc(&ClientToRelay::Ack { id }), [&[0x04, 0, 0, 0][..], &le].concat());
        assert_eq!(enc(&RelayToClient::Deposited { id }), [&[0x05, 0, 0, 0][..], &le].concat());
    }

    #[test]
    fn fixed_arrays_have_no_length_prefix() {
        let mut arr = [0u8; 32];
        arr[0] = 0xAB;
        arr[31] = 0xCD;
        let e = enc(&RelayToClient::Challenge(arr));
        assert_eq!(e.len(), 4 + 32);
        assert_eq!(&e[..4], &[0, 0, 0, 0]);
        assert_eq!((e[4], e[35]), (0xAB, 0xCD));

        let e = enc(&ClientToRelay::Auth { sign_pk: [0xAA; 32], signature: [0xBB; 64] });
        assert_eq!(e.len(), 4 + 32 + 64);
        assert_eq!(&e[..4], &[0, 0, 0, 0]);
        assert!(e[4..36].iter().all(|&b| b == 0xAA));
        assert!(e[36..100].iter().all(|&b| b == 0xBB));
    }

    #[test]
    fn auth_v2_is_the_seventh_variant() {
        let e = enc(&ClientToRelay::AuthV2 { sign_pk: [0xAA; 32], signature: [0xBB; 64] });
        assert_eq!(e.len(), 4 + 32 + 64);
        assert_eq!(&e[..4], &[0x06, 0, 0, 0]);
        assert!(e[4..36].iter().all(|&b| b == 0xAA));
        assert!(e[36..100].iter().all(|&b| b == 0xBB));
    }

    #[test]
    fn send_layout() {
        let e = enc(&ClientToRelay::Send { to: [0x11; 32], blob: vec![1, 2, 3] });
        assert_eq!(e.len(), 4 + 32 + 8 + 3);
        assert_eq!(&e[..4], &[0x03, 0, 0, 0]);
        assert!(e[4..36].iter().all(|&b| b == 0x11));
        assert_eq!(&e[36..44], &[3, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&e[44..], &[1, 2, 3]);
    }

    #[test]
    fn bundle_option_layout() {
        let none = enc(&RelayToClient::Bundle { pk: [0; 32], bundle: None });
        assert_eq!(none.len(), 4 + 32 + 1);
        assert_eq!(&none[..4], &[0x03, 0, 0, 0]);
        assert_eq!(none[36], 0);

        let some = enc(&RelayToClient::Bundle { pk: [0; 32], bundle: Some(vec![0xDE, 0xAD, 0xBE, 0xEF]) });
        assert_eq!(some.len(), 4 + 32 + 1 + 8 + 4);
        assert_eq!(some[36], 1);
        assert_eq!(&some[37..45], &[4, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&some[45..], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn every_variant_round_trips() {
        let client = vec![
            ClientToRelay::Auth { sign_pk: [1; 32], signature: [2; 64] },
            ClientToRelay::Publish { bundle: vec![9, 8, 7] },
            ClientToRelay::GetBundle { pk: [3; 32] },
            ClientToRelay::Send { to: [4; 32], blob: vec![5, 6] },
            ClientToRelay::Ack { id: 42 },
            ClientToRelay::Ping,
            ClientToRelay::AuthV2 { sign_pk: [3; 32], signature: [4; 64] },
        ];
        for m in client {
            let e = enc(&m);
            assert_eq!(e, enc(&dec::<ClientToRelay>(&e)));
        }
        let relay = vec![
            RelayToClient::Challenge([7; 32]),
            RelayToClient::AuthOk,
            RelayToClient::AuthFail,
            RelayToClient::Bundle { pk: [1; 32], bundle: None },
            RelayToClient::Bundle { pk: [2; 32], bundle: Some(vec![3, 4]) },
            RelayToClient::Incoming { id: 99, from: [5; 32], blob: vec![6, 7, 8] },
            RelayToClient::Deposited { id: 123 },
            RelayToClient::Pong,
            RelayToClient::Error("oops".into()),
        ];
        for m in relay {
            let e = enc(&m);
            assert_eq!(e, enc(&dec::<RelayToClient>(&e)));
        }
    }
}

#[cfg(test)]
mod destination {
    use super::*;

    // Generated with Python's base64/hashlib from 391 bytes (i*37+11)%256,
    // in I2P's alphabet ('-' and '~' for '+' and '/').
    const DEST: &str = "CzBVep~E6Q4zWH2ix-wRNluApcrvFDleg6jN8hc8YYar0PUaP2SJrtP4HUJnjLHW-yBFao-02f4jSG2St9wBJktwlbrfBClOc5i94gcsUXabwOUKL1R5nsPoDTJXfKHG6xA1Wn-kye4TOF2Cp8zxFjtgharP9Bk-Y4it0vccQWaLsNX6H0RpjrPY~SJHbJG22wAlSm-Uud4DKE1yl7zhBitQdZq~5AkuU3idwucMMVZ7oMXqDzRZfqPI7RI3XIGmy~AVOl-Eqc7zGD1ih6zR9htAZYqv1PkeQ2iNstf8IUZrkLXa~yRJbpO43QInTHGWu-AFKk90mb7jCC1Sd5zB5gswVXqfxOkOM1h9osfsETZbgKXK7xQ5XoOozfIXPGGGq9D1Gj9kia7T-B1CZ4yx1vsgRWqPtNn-I0htkrfcASZLcJW63wQpTnOYveIHLFF2m8DlCi9UeZ7D6A0yV3yhxusQNVp~pMnuEzhdgqfM8RY7YIWqz~QZPmOIrdL3HEFmi7DV-h9EaQ==";
    const B32: &str = "pltnfwacxdinhnpdnjmzn4royewu6dcbphanacvztwcx4vjfqg6a.b32.i2p";
    const SHA256_HEX: &str = "7ae6d2d802b8d0d3b5e36a5996f22ec12d4f0c4179c0d00ab99d857e552581bc";

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn both_spellings_give_the_same_hash() {
        assert_eq!(hex(&destination_hash(DEST).unwrap()), SHA256_HEX);
        assert_eq!(hex(&destination_hash(B32).unwrap()), SHA256_HEX);
        assert_eq!(destination_hash(&B32.to_uppercase().replace(".B32.I2P", ".b32.i2p")), destination_hash(DEST));
    }

    #[test]
    fn garbage_is_not_a_destination() {
        assert!(destination_hash("not/base64").is_none());
        assert!(destination_hash("abc.b32.i2p").is_none());
    }

    #[test]
    fn the_signed_message_names_the_relay() {
        let a = auth_v2_message(&[1; 32], &[9; 32]);
        let b = auth_v2_message(&[2; 32], &[9; 32]);
        assert_ne!(a, b);
        assert!(a.starts_with(b"gipny-relay-auth-v2"));
    }
}
