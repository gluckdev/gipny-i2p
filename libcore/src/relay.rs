use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex};

use crate::crypto::Identity;
use crate::net::{NetError, I2pNode};

include!("relay_wire.rs");

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

#[path = "relay_sender.rs"]
pub(crate) mod sender;
pub use sender::RelaySender;

pub struct RelayClient {
    pub out_tx: RelaySender,
    pub in_rx: Arc<Mutex<mpsc::Receiver<RelayToClient>>>,
}

/// Connect to our own relay. Failures count toward SAM session health.
pub async fn connect(
    node: &Arc<I2pNode>,
    i2p_address: &str,
    identity: &Arc<Identity>,
) -> Result<RelayClient> {
    let stream = node.connect_relay(i2p_address, RELAY_PORT).await?;
    start_terminal(
        stream.into_inner(),
        i2p_address,
        identity,
        &identity.card().sign_pk,
    )
    .await
}

pub async fn connect_local(
    stream: std::pin::Pin<Box<dyn crate::net::DuplexStream>>,
    address: &str,
    identity: &Arc<Identity>,
) -> Result<RelayClient> {
    start_terminal(stream, address, identity, &identity.card().sign_pk).await
}

pub async fn connect_peer(
    node: &Arc<I2pNode>,
    address: &str,
    identity: &Arc<Identity>,
    owner: &[u8; 32],
) -> Result<RelayClient> {
    let stream = node.connect_service(address, RELAY_PORT).await?;
    start_terminal(stream.into_inner(), address, identity, owner).await
}

async fn start_terminal(
    mut stream: std::pin::Pin<Box<dyn crate::net::DuplexStream>>,
    address: &str,
    identity: &Identity,
    owner: &[u8; 32],
) -> Result<RelayClient> {
    authenticate_terminal(&mut stream, address, identity, owner).await?;
    Ok(sender::start(stream, *owner, address))
}

pub async fn authenticate_terminal<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    address: &str,
    identity: &Identity,
    owner: &[u8; 32],
) -> Result<()> {
    let hash = crate::card::address_hash(address).map_err(|_| RelayError::AuthFailed)?;
    let RelayToClient::Challenge(challenge) = recv(stream).await? else {
        return Err(RelayError::AuthFailed);
    };
    let mut nonce = [0; 32];
    crate::crypto::fill_random(&mut nonce);
    let requester = identity.card().sign_pk;
    let signature = identity.sign(&framed_auth_message(&hash, &challenge, &nonce));
    send(
        stream,
        &ClientToRelay::AuthFramedV1 {
            sign_pk: requester,
            nonce,
            signature,
        },
    )
    .await?;
    let RelayToClient::AuthFramedOkV1 { proof } = recv(stream).await? else {
        return Err(RelayError::AuthFailed);
    };
    let card = crate::crypto::IdentityCard {
        sign_pk: *owner,
        dh_pk: [0; 32],
    };
    if !card.verify(
        &terminal_proof_message(&hash, &requester, &challenge, &nonce),
        &proof,
    ) {
        return Err(RelayError::AuthFailed);
    }
    Ok(())
}

/// Reach a relay the way a contact would — over i2p, through our tunnels and
/// its — and hang up at its first frame. Says whether the relay can be reached
/// from the network, which reading our own relay over a pipe no longer shows.
/// Never logs in: an owner login would take over the relay's live push.
pub async fn probe(node: &Arc<I2pNode>, i2p_address: &str) -> Result<()> {
    let stream = node.connect_service(i2p_address, RELAY_PORT).await?;
    let mut stream = stream.into_inner();
    match recv::<_, RelayToClient>(&mut stream).await? {
        RelayToClient::Challenge(_) => Ok(()),
        _ => Err(RelayError::Proto("expected Challenge".into())),
    }
}

fn out_kind(f: &ClientToRelay) -> String {
    match f {
        ClientToRelay::SendPartV1 { .. } => "part".into(),
        ClientToRelay::Reserved11 | ClientToRelay::Reserved12 | ClientToRelay::Reserved13 | ClientToRelay::Reserved14 | ClientToRelay::ViewOnceOfferV1(_) | ClientToRelay::ViewOncePartV1 { .. } => "live".into(),
        ClientToRelay::Auth { .. } => "Auth".into(),
        ClientToRelay::Publish { bundle } => format!("Publish({}B)", bundle.len()),
        ClientToRelay::GetBundle { pk } => format!("GetBundle({})", hex_short(pk)),
        ClientToRelay::Send { to, blob } => format!("Send(to={}, {}B)", hex_short(to), blob.len()),
        ClientToRelay::Ack { id } => format!("Ack({})", id),
        ClientToRelay::Ping => "Ping".into(),
        ClientToRelay::AuthFramedV1 { .. } => "AuthFramedV1".into(),
        ClientToRelay::AuthV2 { .. } => "AuthV2".into(),
        ClientToRelay::Dht(b) => format!("Dht({}B)", b.len()),
    }
}

fn frame_kind(f: &RelayToClient) -> String {
    match f {
        RelayToClient::SendPartAckV1 { .. } => "part-ack".into(),
        RelayToClient::Reserved13 | RelayToClient::Reserved14 | RelayToClient::Reserved15 | RelayToClient::Reserved16 | RelayToClient::ViewOnceReadyV1 { .. } | RelayToClient::ViewOnceStoredV1 { .. } | RelayToClient::ViewOncePartAckV1 { .. } => "live".into(),
        RelayToClient::Challenge(_) => "Challenge".into(),
        RelayToClient::AuthOk => "AuthOk".into(),
        RelayToClient::AuthFramedOkV1 { .. } => "AuthFramedOkV1".into(),
        RelayToClient::IncomingPartV1 { index, .. } => format!("IncomingPart({index})"),
        RelayToClient::AuthFail => "AuthFail".into(),
        RelayToClient::Bundle { pk, bundle } => format!("Bundle(pk={}, present={})", hex_short(pk), bundle.is_some()),
        RelayToClient::Incoming { id, from, blob } => format!("Incoming(id={}, from={}, {}B)", id, hex_short(from), blob.len()),
        RelayToClient::Deposited { id } => format!("Deposited({})", id),
        RelayToClient::Pong => "Pong".into(),
        RelayToClient::Error(e) => format!("Error({})", e),
        RelayToClient::Dht(b) => format!("Dht({}B)", b.len()),
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
/// Golden frames for the shared native and WASM binary protocol. Preserve
/// earlier discriminants when appending message types.
#[cfg(test)]
mod wire_compat {
    use super::*;

    include!("view_once_wire_golden.rs");

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
    fn dht_frames_are_the_last_variants() {
        let e = enc(&ClientToRelay::Dht(vec![9, 8]));
        assert_eq!(e, [0x07, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 9, 8]);
        let r = enc(&RelayToClient::Dht(vec![7]));
        assert_eq!(r, [0x08, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 7]);
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
        assert!(!same_destination("", ""));
        assert!(!same_destination("not-an-address", "not-an-address"));
        assert!(!same_destination(B32, "abc.b32.i2p"));
        assert!(same_destination(B32, &B32.to_uppercase()));
    }

    #[test]
    fn the_signed_message_names_the_relay() {
        let a = auth_v2_message(&[1; 32], &[9; 32]);
        let b = auth_v2_message(&[2; 32], &[9; 32]);
        assert_ne!(a, b);
        assert!(a.starts_with(b"gipny-relay-auth-v2"));
    }
}

/// Send to the live, authenticated personal terminal.
/// The sender seals only after validating the pinned recipient's signed prekey.
pub async fn send_view_once(
    node: &I2pNode,
    address: &str,
    identity: &Identity,
    peer: &crate::IdentityCard,
    request: crate::view_once::Request,
    body: impl FnOnce() -> Option<zeroize::Zeroizing<Vec<u8>>>,
) -> Result<()> {
    let stream=node.connect_service(address,RELAY_PORT).await?.into_inner();
    send_view_once_on(stream,address,identity,peer,request,body).await
}
pub(crate) async fn send_view_once_on<S:AsyncRead+AsyncWrite+Unpin>(
    mut stream:S, address:&str, identity:&Identity,peer:&crate::IdentityCard,request:crate::view_once::Request,
    body:impl FnOnce()->Option<zeroize::Zeroizing<Vec<u8>>>,
)->Result<()>{
    let unavailable=||RelayError::Proto("one-time message unavailable".into());
    authenticate_terminal(&mut stream,address,identity,&peer.sign_pk).await?;
    send(&mut stream, &ClientToRelay::ViewOnceOfferV1(request)).await?;
    let offer=match recv(&mut stream).await? {
        RelayToClient::ViewOnceStoredV1 { id } if id==request.id => return Ok(()),
        RelayToClient::ViewOnceReadyV1 { offer } => offer,
        _=>return Err(unavailable()),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let envelope = crate::view_once::seal_with(identity, peer, &request, &offer, body, now)
        .map_err(|_| unavailable())?;
    let parts = crate::view_once::parts(&envelope);
    let total = parts.len() as u16;
    for (i, part) in parts.enumerate() {
        let index = i as u16;
        send(
            &mut stream,
            &ClientToRelay::ViewOncePartV1 {
                index,
                total,
                bytes: part.to_vec(),
            },
        )
        .await?;
        let answer = recv(&mut stream).await?;
        if index + 1 == total {
            if !matches!(answer, RelayToClient::ViewOnceStoredV1 { id } if id == request.id) {
                return Err(unavailable());
            }
        } else if !matches!(answer, RelayToClient::ViewOncePartAckV1 { index: ack } if ack == index)
        {
            return Err(unavailable());
        }
    }
    Ok(())
}
