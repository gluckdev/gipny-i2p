//! Classic X25519 NTCP2 initiator probe. Protocol framing follows the I2P
//! NTCP2 specification and pinned emissary's initiator.rs; cryptography uses
//! emissary's public implementations. This does not run Router<BrowserRuntime>.

use crate::{configure_forwarder_authenticated, BrowserRuntime, BrowserTcpStream};
use emissary_core::{
    crypto::{
        aes::cbc::Aes, base64_encode, chachapoly::ChaChaPoly, hmac::Hmac, noise::NoiseContext,
        sha256::Sha256, siphash::SipHash, EphemeralPrivateKey, SigningPrivateKey, StaticPrivateKey,
        StaticPublicKey,
    },
    primitives::{RouterAddress, RouterInfo},
    runtime::{AsyncRead, AsyncWrite, Runtime, TcpStream},
    Config,
};
use futures::future::{poll_fn, select, Either};
use rand::{Rng, RngExt};
use std::{net::SocketAddr, pin::Pin, time::Duration};
use wasm_bindgen::prelude::*;
use zeroize::Zeroizing;

#[derive(Debug)]
pub enum ProbeError {
    RouterInfo,
    Network,
    StaleRouterInfo,
    NoClassicNtcp2,
    Io(&'static str),
    Authentication(&'static str),
    ClockSkew,
    InvalidFrame,
    RemoteTermination(u8),
    Timeout,
}

pub struct VerifiedPeer {
    pub router: RouterInfo,
    pub address: SocketAddr,
    pub static_key: StaticPublicKey,
    pub iv: [u8; 16],
}

/// Signature verification precedes use of every target or static key. Published
/// IPv4 NTCP2 only for this spike; PQ and IPv6 remain separate experiments.
pub fn verified_peer(bytes: &[u8]) -> Result<VerifiedPeer, ProbeError> {
    if bytes.len() > 16 * 1024 {
        return Err(ProbeError::RouterInfo);
    }
    let router = RouterInfo::parse::<BrowserRuntime>(bytes).map_err(|_| ProbeError::RouterInfo)?;
    if router.net_id != 2 {
        return Err(ProbeError::Network);
    }
    let now = BrowserRuntime::time_since_epoch().as_millis() as u64;
    let published = *router.published.date();
    if now.saturating_sub(published) > 36 * 60 * 60 * 1000
        || published.saturating_sub(now) > 120_000
    {
        return Err(ProbeError::StaleRouterInfo);
    }
    let (address, static_key, iv) = router
        .addresses()
        .find_map(|address| match address {
            RouterAddress::Ntcp2 {
                static_key: StaticPublicKey::X25519(key),
                iv: Some(iv),
                socket_address: Some(address),
                ..
            } if address.is_ipv4() && address.port() != 0 => {
                Some((*address, StaticPublicKey::X25519(*key), *iv))
            }
            _ => None,
        })
        .ok_or(ProbeError::NoClassicNtcp2)?;
    Ok(VerifiedPeer {
        router,
        address,
        static_key,
        iv,
    })
}

#[derive(Debug)]
pub struct ProbeReceipt {
    pub peer: String,
    pub target: SocketAddr,
    pub authenticated_bytes: usize,
    pub blocks: usize,
    pub elapsed_ms: u128,
}

async fn read_exact(stream: &mut BrowserTcpStream, mut bytes: &mut [u8]) -> Result<(), ProbeError> {
    while !bytes.is_empty() {
        let n = poll_fn(|cx| Pin::new(&mut *stream).poll_read(cx, bytes))
            .await
            .map_err(|_| ProbeError::Io("read"))?;
        if n == 0 {
            return Err(ProbeError::Io("EOF"));
        }
        bytes = &mut bytes[n..];
    }
    Ok(())
}

async fn write_all(stream: &mut BrowserTcpStream, mut bytes: &[u8]) -> Result<(), ProbeError> {
    while !bytes.is_empty() {
        let n = poll_fn(|cx| Pin::new(&mut *stream).poll_write(cx, bytes))
            .await
            .map_err(|_| ProbeError::Io("write"))?;
        if n == 0 {
            return Err(ProbeError::Io("zero write"));
        }
        bytes = &bytes[n..];
    }
    poll_fn(|cx| Pin::new(&mut *stream).poll_flush(cx))
        .await
        .map_err(|_| ProbeError::Io("flush"))
}

pub async fn probe(bytes: &[u8]) -> Result<ProbeReceipt, ProbeError> {
    let peer = verified_peer(bytes)?;
    let work = Box::pin(handshake(peer));
    let timeout = BrowserRuntime::timer(Duration::from_secs(20));
    match select(work, timeout).await {
        Either::Left((result, _)) => result,
        Either::Right(_) => Err(ProbeError::Timeout),
    }
}

async fn handshake(peer: VerifiedPeer) -> Result<ProbeReceipt, ProbeError> {
    let started = web_time::Instant::now();
    let mut stream = BrowserTcpStream::connect(peer.address)
        .await
        .ok_or(ProbeError::Io("connect"))?;
    let local_secret = Zeroizing::new(BrowserRuntime::rng().random::<[u8; 32]>());
    let local_static = StaticPrivateKey::from_bytes(*local_secret);
    let local_signing = SigningPrivateKey::random(BrowserRuntime::rng());
    let local_address =
        RouterAddress::new_unpublished_ntcp2(*local_secret, "0.0.0.0:0".parse().unwrap());
    let local_info = RouterInfo::new::<BrowserRuntime>(
        &Config::default(),
        Some(local_address),
        None,
        None,
        None,
        &local_static,
        &local_signing,
        true,
    )
    .serialize(&local_signing);
    let ck = Sha256::new()
        .update(b"Noise_XKaesobfse+hs2+hs3_25519_ChaChaPoly_SHA256")
        .finalize_new();
    let h = Sha256::new().update(ck).finalize_new();
    let mut noise = NoiseContext::new(ck, h);
    let ephemeral = EphemeralPrivateKey::random(BrowserRuntime::rng());
    let public = ephemeral.public().to_vec();
    noise.mix_hash(&peer.static_key).mix_hash(&public);
    let key = Zeroizing::new(noise.mix_key(&ephemeral, &peer.static_key));
    let hash = peer.router.identity.hash();
    let mut aes = Aes::new_encryptor(&hash, &peer.iv);
    let encrypted_x = aes.encrypt(&public);
    let next_iv = aes.iv();
    let mut padding = [0_u8; 32];
    BrowserRuntime::rng().fill_bytes(&mut padding);
    let mut options = vec![0_u8; 16];
    options[0] = 2; // network
    options[1] = 2; // protocol version
    options[2..4].copy_from_slice(&(padding.len() as u16).to_be_bytes());
    options[4..6].copy_from_slice(&((local_info.len() + 20) as u16).to_be_bytes());
    options[8..12]
        .copy_from_slice(&(BrowserRuntime::time_since_epoch().as_secs() as u32).to_be_bytes());
    ChaChaPoly::new(key.as_ref())
        .encrypt_with_ad_new(noise.state(), &mut options)
        .map_err(|_| ProbeError::Authentication("request"))?;
    noise.mix_hash(&options).mix_hash(padding);
    let request: Vec<u8> = encrypted_x
        .into_iter()
        .chain(options)
        .chain(padding)
        .collect();
    write_all(&mut stream, &request).await?;

    let mut created = [0_u8; 64];
    read_exact(&mut stream, &mut created).await?;
    let y = Aes::new_decryptor(&hash, &next_iv).decrypt(&created[..32]);
    let remote_ephemeral = StaticPublicKey::try_from_bytes(&y).ok_or(ProbeError::InvalidFrame)?;
    noise.mix_hash(y);
    let key = Zeroizing::new(noise.mix_key(&ephemeral, &remote_ephemeral));
    let mut options = created[32..].to_vec();
    ChaChaPoly::new(key.as_ref())
        .decrypt_with_ad(noise.state(), &mut options)
        .map_err(|_| ProbeError::Authentication("SessionCreated"))?;
    if options.len() != 16 {
        return Err(ProbeError::InvalidFrame);
    }
    let remote_time = u32::from_be_bytes(options[8..12].try_into().unwrap()) as u64;
    if BrowserRuntime::time_since_epoch()
        .as_secs()
        .abs_diff(remote_time)
        > 120
    {
        return Err(ProbeError::ClockSkew);
    }
    let padding_length = u16::from_be_bytes(options[2..4].try_into().unwrap()) as usize;
    let mut padding = vec![0; padding_length];
    read_exact(&mut stream, &mut padding).await?;
    noise.mix_hash(&created[32..]).mix_hash(padding);
    let mut confirmed = local_static.public().to_vec();
    ChaChaPoly::with_nonce(key.as_ref(), 1)
        .encrypt_with_ad_new(noise.state(), &mut confirmed)
        .map_err(|_| ProbeError::Authentication("local static key"))?;
    noise.mix_hash(&confirmed);
    let key = Zeroizing::new(noise.mix_key(&local_static, &remote_ephemeral));
    let mut info_block = vec![2];
    info_block.extend_from_slice(&((local_info.len() + 1) as u16).to_be_bytes());
    info_block.push(0);
    info_block.extend_from_slice(&local_info);
    ChaChaPoly::new(key.as_ref())
        .encrypt_with_ad_new(noise.state(), &mut info_block)
        .map_err(|_| ProbeError::Authentication("local RouterInfo"))?;
    noise.mix_hash(&info_block);
    confirmed.extend_from_slice(&info_block);
    let temp = Zeroizing::new(Hmac::new(noise.chaining_key()).update([]).finalize_new());
    let send = Zeroizing::new(Hmac::new(temp.as_ref()).update([1]).finalize_new());
    let receive = Zeroizing::new(
        Hmac::new(temp.as_ref())
            .update(send.as_ref())
            .update([2])
            .finalize_new(),
    );
    let mut sip = SipHash::new_initiator(temp.as_ref(), noise.state());
    write_all(&mut stream, &confirmed).await?;

    let mut header = [0_u8; 2];
    read_exact(&mut stream, &mut header).await?;
    let length = sip.deobfuscate(u16::from_be_bytes(header)) as usize;
    if length < 16 {
        return Err(ProbeError::InvalidFrame);
    }
    let mut ciphertext = vec![0_u8; length];
    read_exact(&mut stream, &mut ciphertext).await?;
    let plaintext = ChaChaPoly::new(receive.as_ref())
        .decrypt(ciphertext)
        .map_err(|_| ProbeError::Authentication("data phase"))?;
    let blocks = validate_blocks(&plaintext, &peer.router)?;

    // Close politely after one authenticated response; this probe is not a
    // router and must not leave a persistent, unusable peer session behind.
    let mut termination = vec![4, 0, 9];
    termination.extend_from_slice(&1_u64.to_be_bytes());
    termination.push(0);
    let encrypted = ChaChaPoly::new(send.as_ref())
        .encrypt(&termination)
        .map_err(|_| ProbeError::Authentication("termination"))?;
    let mut frame = sip.obfuscate(encrypted.len() as u16).to_be_bytes().to_vec();
    frame.extend(encrypted);
    let _ = write_all(&mut stream, &frame).await;
    Ok(ProbeReceipt {
        peer: base64_encode(hash),
        target: peer.address,
        authenticated_bytes: plaintext.len(),
        blocks,
        elapsed_ms: started.elapsed().as_millis(),
    })
}

fn validate_blocks(mut bytes: &[u8], peer: &RouterInfo) -> Result<usize, ProbeError> {
    let mut count = 0;
    while !bytes.is_empty() {
        if bytes.len() < 3 {
            return Err(ProbeError::InvalidFrame);
        }
        let kind = bytes[0];
        let size = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
        if size > bytes.len() - 3 {
            return Err(ProbeError::InvalidFrame);
        }
        let payload = &bytes[3..3 + size];
        if kind == 4 {
            return Err(if size >= 9 {
                ProbeError::RemoteTermination(payload[8])
            } else {
                ProbeError::InvalidFrame
            });
        }
        if kind == 2 {
            let remote = payload
                .get(1..)
                .and_then(|b| RouterInfo::parse::<BrowserRuntime>(b).ok())
                .ok_or(ProbeError::RouterInfo)?;
            if remote.identity.hash() != peer.identity.hash() {
                return Err(ProbeError::Authentication("peer identity"));
            }
        }
        count += 1;
        bytes = &bytes[3 + size..];
    }
    Ok(count)
}

/// Browser API for the transport experiment. A result means an authenticated
/// data-phase response, not merely a successful TCP/WebSocket connection.
#[wasm_bindgen]
pub async fn ntcp2_probe(
    router_info: Vec<u8>,
    endpoint: String,
    capability: String,
) -> Result<String, JsValue> {
    configure_forwarder_authenticated(&endpoint, &capability).map_err(JsValue::from_str)?;
    let result = probe(&router_info)
        .await
        .map_err(|e| JsValue::from_str(&format!("{e:?}")))?;
    Ok(format!(
        "peer={} target={} authenticated_bytes={} blocks={} elapsed_ms={}",
        result.peer, result.target, result.authenticated_bytes, result.blocks, result.elapsed_ms
    ))
}
