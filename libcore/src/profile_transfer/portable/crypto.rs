//! Secret crypto state, shared by native and WASM transfer adapters.
//!
//! Local vault keys, device handles and I2P router identities are deliberately
//! absent. Keys/ratchet bytes are wiped on drop and have no Debug implementation.

use super::{decode, encode, options, Error, List, MAX_ID, MAX_METADATA_BYTES, VERSION};
use crate::crypto::{Identity, RatchetState};
use bincode::Options;
use serde::{
    de::{Error as _, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize, Serializer,
};
use std::collections::HashSet;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub const MAX_PEERS: usize = 1024;
pub const MAX_PREKEYS: usize = 256;
pub const MAX_ALTERNATES: usize = 3;
pub const MAX_RATCHET_BYTES: usize = 128 * 1024;

#[derive(Serialize, Zeroize, ZeroizeOnDrop)]
#[serde(transparent)]
pub struct Secret32([u8; 32]);

impl Secret32 {
    pub fn new(bytes: [u8; 32]) -> Self {
        let bytes = Zeroizing::new(bytes);
        Self(*bytes)
    }
    pub fn expose(&self) -> &[u8; 32] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Secret32 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Key;
        impl<'de> Visitor<'de> for Key {
            type Value = Secret32;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("exactly 32 secret bytes")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                // An ordinary derived array decoder does not wipe a partially
                // filled stack buffer when a later element is missing/invalid.
                let mut bytes = Zeroizing::new([0u8; 32]);
                for index in 0..32 {
                    bytes[index] = seq
                        .next_element()?
                        .ok_or_else(|| A::Error::invalid_length(index, &self))?;
                }
                if seq.next_element::<u8>()?.is_some() {
                    return Err(A::Error::invalid_length(33, &self));
                }
                Ok(Secret32::new(*bytes))
            }
        }
        // Preserve fixed-array bincode representation: no Vec length prefix.
        d.deserialize_tuple(32, Key)
    }
}

#[derive(Serialize, Deserialize)]
pub struct Ratchet {
    format: u16,
    bytes: SecretBytes,
}

struct SecretBytes(Zeroizing<Vec<u8>>);

impl Serialize for SecretBytes {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for SecretBytes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Bytes;
        impl<'de> Visitor<'de> for Bytes {
            type Value = SecretBytes;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("bounded secret ratchet bytes")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let hint = seq.size_hint().unwrap_or(0);
                if hint > MAX_RATCHET_BYTES {
                    return Err(A::Error::custom("ratchet limit"));
                }
                // Truncated/invalid input must wipe the partially decoded copy too.
                let mut bytes = Zeroizing::new(Vec::with_capacity(hint));
                while let Some(byte) = seq.next_element::<u8>()? {
                    if bytes.len() == MAX_RATCHET_BYTES {
                        return Err(A::Error::custom("ratchet limit"));
                    }
                    if bytes.len() == bytes.capacity() {
                        // Formats without a length hint grow via an explicitly
                        // wiped old allocation, rather than Vec::realloc.
                        let capacity = (bytes.len().saturating_mul(2))
                            .max(64)
                            .min(MAX_RATCHET_BYTES);
                        let mut larger = Zeroizing::new(Vec::with_capacity(capacity));
                        larger.extend_from_slice(&bytes);
                        bytes = larger;
                    }
                    bytes.push(byte);
                }
                Ok(SecretBytes(bytes))
            }
        }
        d.deserialize_seq(Bytes)
    }
}

impl Ratchet {
    pub fn capture(state: &RatchetState) -> Result<Self, Error> {
        Ok(Self {
            format: 1,
            bytes: SecretBytes(encode(state, MAX_RATCHET_BYTES as u64)?),
        })
    }
    pub fn restore(&self) -> Result<RatchetState, Error> {
        if self.format != 1 {
            return Err(Error::UnsupportedVersion);
        }
        // The legacy convenience from_bytes accepts trailing bytes. Transfer
        // records use exact fixed-int decoding and the shared skipped-key cap.
        options(MAX_RATCHET_BYTES as u64)
            .deserialize(&self.bytes.0)
            .map_err(|_| Error::InvalidRatchet)
    }
}

#[derive(Serialize, Deserialize)]
pub struct Prekey {
    pub id: u64,
    pub secret: Secret32,
    pub public: [u8; 32],
    pub created_at_ms: Option<i64>,
}

#[derive(Serialize, Deserialize)]
pub struct OwnInitialization {
    pub dh: [u8; 32],
    /// Native currently has no timestamp for this RAM marker. Absence is
    /// explicit; an importer must not invent a fresh grace period.
    pub created_at_ms: Option<i64>,
}

#[derive(Serialize, Deserialize)]
pub struct Peer {
    pub sign_pk: [u8; 32],
    pub dh_pk: [u8; 32],
    pub primary: Option<Ratchet>,
    pub lost_initialization: Option<Ratchet>,
    pub own_initialization: Option<OwnInitialization>,
    pub alternates: List<Ratchet, MAX_ALTERNATES>,
}

#[derive(Serialize, Deserialize)]
pub struct State {
    pub version: u16,
    pub sign_seed: Secret32,
    pub dh_secret: Secret32,
    pub sign_pk: [u8; 32],
    pub dh_pk: [u8; 32],
    pub current_signed_prekey: u64,
    /// Include still-accepted historical signed keys; never discard on import.
    pub signed_prekeys: List<Prekey, MAX_PREKEYS>,
    pub one_time_prekeys: List<Prekey, MAX_PREKEYS>,
    /// Includes IDs of consumed/deleted keys, not just max(remaining) + 1.
    pub next_prekey_id: u64,
    pub peers: List<Peer, MAX_PEERS>,
}

impl State {
    pub fn validate(&self) -> Result<(), Error> {
        if self.version != VERSION {
            return Err(Error::UnsupportedVersion);
        }
        let identity = Identity::try_from_bytes(*self.sign_seed.expose(), *self.dh_secret.expose())
            .map_err(|_| Error::InvalidIdentity)?;
        let card = identity.card();
        if card.sign_pk != self.sign_pk || card.dh_pk != self.dh_pk {
            return Err(Error::InvalidIdentity);
        }
        if self.next_prekey_id == 0 || self.next_prekey_id > MAX_ID {
            return Err(Error::InvalidPrekey);
        }
        let mut ids = HashSet::new();
        for key in self
            .signed_prekeys
            .iter()
            .chain(self.one_time_prekeys.iter())
        {
            if key.id == 0 || key.id >= self.next_prekey_id || !ids.insert(key.id) {
                return Err(Error::InvalidPrekey);
            }
            let secret = StaticSecret::from(*key.secret.expose());
            if PublicKey::from(&secret).to_bytes() != key.public {
                return Err(Error::InvalidPrekey);
            }
        }
        if !self
            .signed_prekeys
            .iter()
            .any(|key| key.id == self.current_signed_prekey)
        {
            return Err(Error::InvalidPrekey);
        }
        let mut peers = HashSet::new();
        for peer in self.peers.iter() {
            if peer.sign_pk == self.sign_pk
                || peer.sign_pk == [0; 32]
                || peer.dh_pk == [0; 32]
                || !peers.insert(peer.sign_pk)
            {
                return Err(Error::InvalidPeer);
            }
            let verifying = ed25519_dalek::VerifyingKey::from_bytes(&peer.sign_pk)
                .map_err(|_| Error::InvalidPeer)?;
            let dh = StaticSecret::from(*self.dh_secret.expose())
                .diffie_hellman(&PublicKey::from(peer.dh_pk));
            if verifying.is_weak() || dh.as_bytes() == &[0; 32] {
                return Err(Error::InvalidPeer);
            }
            if let Some(initial) = &peer.own_initialization {
                if initial.dh == [0; 32] {
                    return Err(Error::InvalidPeer);
                }
            }
            for ratchet in peer
                .primary
                .iter()
                .chain(peer.lost_initialization.iter())
                .chain(peer.alternates.iter())
            {
                ratchet.restore()?;
            }
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Zeroizing<Vec<u8>>, Error> {
        self.validate()?;
        encode(self, MAX_METADATA_BYTES)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let state: Self = decode(bytes, MAX_METADATA_BYTES)?;
        state.validate()?;
        Ok(state)
    }
}
