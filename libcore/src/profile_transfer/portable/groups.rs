//! Group membership uses pinned identity keys, not local contact row numbers.
use super::{
    contacts::{self, Index, MAX_ADDRESS_BYTES, MAX_NAME_BYTES},
    decode, encode,
    text::Text,
    Error, List, MAX_METADATA_BYTES, VERSION,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use zeroize::Zeroizing;

pub const MAX_GROUPS: usize = 1024;
pub const MAX_MEMBERS: usize = 64;

#[derive(Serialize, Deserialize)]
pub struct Member {
    pub sign_pk: [u8; 32],
    pub dh_pk: [u8; 32],
    pub i2p_address: Option<Text<MAX_ADDRESS_BYTES>>,
    pub name: Text<MAX_NAME_BYTES>,
    pub is_self: bool,
}

#[derive(Serialize, Deserialize)]
pub struct Group {
    pub id: [u8; 32],
    pub name: Text<MAX_NAME_BYTES>,
    pub created_at_ms: i64,
    pub pinned_at_ms: Option<i64>,
    pub last_message_at_ms: Option<i64>,
    pub members: List<Member, MAX_MEMBERS>,
}

#[derive(Serialize, Deserialize)]
pub struct Groups {
    pub version: u16,
    pub groups: List<Group, MAX_GROUPS>,
}

impl Groups {
    pub fn validate(&self, contacts: &Index) -> Result<(), Error> {
        if self.version != VERSION {
            return Err(Error::UnsupportedVersion);
        }
        let own = contacts.own_identity();
        let mut group_ids = HashSet::new();
        let mut sign_to_dh = HashMap::new();
        let mut dh_to_sign = HashMap::new();
        for group in self.groups.iter() {
            if group.id == [0; 32] || group.members.is_empty() || !group_ids.insert(group.id) {
                return Err(Error::InvalidGroup);
            }
            contacts::time(group.created_at_ms)?;
            for timestamp in [group.pinned_at_ms, group.last_message_at_ms]
                .into_iter()
                .flatten()
            {
                contacts::time(timestamp)?;
            }
            let mut members = HashSet::new();
            let mut has_self = false;
            for member in group.members.iter() {
                if !members.insert(member.sign_pk)
                    || member.is_self != (member.sign_pk == own.sign_pk)
                {
                    return Err(Error::InvalidGroup);
                }
                contacts::public_identity(&member.sign_pk, &member.dh_pk)?;
                contacts::address(member.i2p_address.as_ref())?;
                contacts.check_pinned(&member.sign_pk, &member.dh_pk)?;
                if sign_to_dh
                    .insert(member.sign_pk, member.dh_pk)
                    .is_some_and(|old| old != member.dh_pk)
                    || dh_to_sign
                        .insert(member.dh_pk, member.sign_pk)
                        .is_some_and(|old| old != member.sign_pk)
                {
                    return Err(Error::InvalidReference);
                }
                has_self |= member.is_self;
            }
            if !has_self {
                return Err(Error::InvalidGroup);
            }
        }
        Ok(())
    }
    pub fn encode(&self, contacts: &Index) -> Result<Zeroizing<Vec<u8>>, Error> {
        self.validate(contacts)?;
        encode(self, MAX_METADATA_BYTES)
    }
    pub fn decode(bytes: &[u8], contacts: &Index) -> Result<Self, Error> {
        let groups: Self = decode(bytes, MAX_METADATA_BYTES)?;
        groups.validate(contacts)?;
        Ok(groups)
    }
}
