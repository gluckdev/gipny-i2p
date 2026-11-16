//! Contacts retain trust/admission state; validation never upgrades trust.
use super::{crypto, decode, encode, text::Text, Error, List, MAX_ID, MAX_METADATA_BYTES, VERSION};
use crate::crypto::IdentityCard;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

pub const MAX_CONTACTS: usize = 1024;
pub const MAX_NAME_BYTES: usize = 4096;
pub const MAX_ADDRESS_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Trust {
    Unverified,
    Verified,
    Blocked,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    None,
    Incoming,
    Outgoing,
}

#[derive(Serialize, Deserialize)]
pub struct Contact {
    pub id: u64,
    pub sign_pk: [u8; 32],
    pub dh_pk: [u8; 32],
    /// None means not learned yet, as supported by current native contacts.
    pub i2p_address: Option<Text<MAX_ADDRESS_BYTES>>,
    pub relay_address: Option<Text<MAX_ADDRESS_BYTES>>,
    pub name: Text<MAX_NAME_BYTES>,
    pub trust: Trust,
    pub request: Request,
    pub created_at_ms: i64,
    pub last_seen_ms: Option<i64>,
    pub pinned_at_ms: Option<i64>,
    pub last_message_at_ms: Option<i64>,
    pub is_bot: bool,
    /// The peer granted its console to us; not a local grant to execute jobs.
    pub agent_granted: bool,
    pub save_history: Option<bool>,
}

#[derive(Serialize, Deserialize)]
pub struct Contacts {
    pub version: u16,
    /// Preserve allocator state even when the highest contact was deleted.
    pub next_contact_id: u64,
    pub contacts: List<Contact, MAX_CONTACTS>,
}

/// Checked identity index used to validate other domains. Does not admit/open
/// any profile; actual data and this index remain owned by the receiver stage.
pub struct Index {
    own: IdentityCard,
    by_sign: HashMap<[u8; 32], ([u8; 32], u64)>,
    by_dh: HashMap<[u8; 32], [u8; 32]>,
}

impl Index {
    pub fn own_identity(&self) -> &IdentityCard {
        &self.own
    }
    pub fn contact_id(&self, sign: &[u8; 32]) -> Option<u64> {
        self.by_sign.get(sign).map(|(_, id)| *id)
    }
    pub fn dh_for(&self, sign: &[u8; 32]) -> Option<&[u8; 32]> {
        self.by_sign.get(sign).map(|(dh, _)| dh)
    }
    pub(super) fn check_pinned(&self, sign: &[u8; 32], dh: &[u8; 32]) -> Result<(), Error> {
        if *sign == self.own.sign_pk {
            return if *dh == self.own.dh_pk {
                Ok(())
            } else {
                Err(Error::InvalidReference)
            };
        }
        if self.dh_for(sign).is_some_and(|known| known != dh)
            || self.by_dh.get(dh).is_some_and(|known| known != sign)
            || *dh == self.own.dh_pk
        {
            return Err(Error::InvalidReference);
        }
        Ok(())
    }
    pub fn validate_crypto(&self, state: &crypto::State) -> Result<(), Error> {
        state.validate()?;
        if state.sign_pk != self.own.sign_pk || state.dh_pk != self.own.dh_pk {
            return Err(Error::InvalidIdentity);
        }
        for peer in state.peers.iter() {
            if self.dh_for(&peer.sign_pk) != Some(&peer.dh_pk) {
                return Err(Error::InvalidReference);
            }
        }
        Ok(())
    }
}

impl Contacts {
    pub fn index(&self, own: &IdentityCard) -> Result<Index, Error> {
        if self.version != VERSION {
            return Err(Error::UnsupportedVersion);
        }
        public_identity(&own.sign_pk, &own.dh_pk)?;
        if self.next_contact_id == 0 || self.next_contact_id > MAX_ID {
            return Err(Error::InvalidContact);
        }
        let mut index = Index {
            own: own.clone(),
            by_sign: HashMap::new(),
            by_dh: HashMap::new(),
        };
        let mut ids = HashSet::new();
        for contact in self.contacts.iter() {
            if contact.id == 0
                || contact.id >= self.next_contact_id
                || !ids.insert(contact.id)
                || contact.sign_pk == own.sign_pk
                || contact.dh_pk == own.dh_pk
            {
                return Err(Error::InvalidContact);
            }
            public_identity(&contact.sign_pk, &contact.dh_pk)?;
            address(contact.i2p_address.as_ref())?;
            address(contact.relay_address.as_ref())?;
            time(contact.created_at_ms)?;
            for timestamp in [
                contact.last_seen_ms,
                contact.pinned_at_ms,
                contact.last_message_at_ms,
            ]
            .into_iter()
            .flatten()
            {
                time(timestamp)?;
            }
            if index
                .by_sign
                .insert(contact.sign_pk, (contact.dh_pk, contact.id))
                .is_some()
                || index.by_dh.insert(contact.dh_pk, contact.sign_pk).is_some()
            {
                return Err(Error::Duplicate);
            }
        }
        Ok(index)
    }
    pub fn encode(&self, own: &IdentityCard) -> Result<Zeroizing<Vec<u8>>, Error> {
        self.index(own)?;
        encode(self, MAX_METADATA_BYTES)
    }
    pub fn decode(bytes: &[u8], own: &IdentityCard) -> Result<Self, Error> {
        let contacts: Self = decode(bytes, MAX_METADATA_BYTES)?;
        contacts.index(own)?;
        Ok(contacts)
    }
}

pub(super) fn public_identity(sign: &[u8; 32], dh: &[u8; 32]) -> Result<(), Error> {
    let key = ed25519_dalek::VerifyingKey::from_bytes(sign).map_err(|_| Error::InvalidIdentity)?;
    // A public-point validation probe, never an encryption key/session. All
    // low-order X25519 inputs yield zero with this clamped nonzero scalar.
    let probe = StaticSecret::from([0; 32]).diffie_hellman(&PublicKey::from(*dh));
    if key.is_weak() || probe.as_bytes() == &[0; 32] {
        return Err(Error::InvalidIdentity);
    }
    Ok(())
}
pub(super) fn address(value: Option<&Text<MAX_ADDRESS_BYTES>>) -> Result<(), Error> {
    if value.is_some_and(|s| !crate::card::is_valid_i2p_address(s.as_str())) {
        return Err(Error::InvalidContact);
    }
    Ok(())
}
pub(super) fn time(value: i64) -> Result<(), Error> {
    if value.unsigned_abs() > MAX_ID {
        return Err(Error::Limit);
    }
    Ok(())
}
