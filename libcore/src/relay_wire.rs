// Shared native/WASM relay wire protocol and authentication context.
use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;

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
    /// One request to the relay-network node behind this relay, encoded by
    /// `gipny_dht`. Only as a connection's first frame, and never followed by
    /// a login: whoever sends it stays anonymous to the relay.
    Dht(Vec<u8>),
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

/// Same I2P destination across full base64 and compact b32 spellings. Invalid
/// addresses (including two empty strings) never compare equal. Base64 case is
/// significant; only decoded destination hashes are compared.
pub fn same_destination(a: &str, b: &str) -> bool {
    match (crate::card::address_hash(a), crate::card::address_hash(b)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
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
    /// The answer to a [`ClientToRelay::Dht`].
    Dht(Vec<u8>),
}

#[derive(Serialize, Deserialize, Debug)]
pub enum EnvelopeBlob {
    X3dhInit(crate::crypto::X3dhInitial),
    Ratchet { header: crate::crypto::RatchetHeader, ciphertext: Vec<u8> },
}

pub const RELAY_PORT: u16 = 443;
pub const MAX_FRAME: u32 = 16 * 1024 * 1024;
/// Default external relay: empty on purpose. Every app runs its own relay inside
/// its process (RelayMode::Builtin); users can paste an external relay destination
/// in Settings (SETTING_RELAY_ONION), and contact cards plus the DHT seeds baked
/// in from GIPNY_DHT_SEEDS at release time tell clients where to find each other.
pub const DEFAULT_RELAY: &str = "";

