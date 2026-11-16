//! Ordered bounded manifest for streamed records. No filesystem paths on wire.

use super::{decode, encode, Error, List, VERSION};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_RECORD_BYTES: u32 = 256 * 1024;
pub const MAX_RECORDS: usize = 65_536;
pub const MAX_TRANSFER_BYTES: u64 = MAX_RECORD_BYTES as u64 * MAX_RECORDS as u64;
const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;
const DIGEST_DOMAIN: &[u8] = b"gipny/profile-transfer/manifest/v1\0";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Section {
    Crypto,
    Contacts,
    Groups,
    Messages,
    Reliability,
    Deletion,
    Attachments,
    FileProgress,
    Preferences,
}

impl Section {
    pub const ALL: [Self; 9] = [
        Self::Crypto,
        Self::Contacts,
        Self::Groups,
        Self::Messages,
        Self::Reliability,
        Self::Deletion,
        Self::Attachments,
        Self::FileProgress,
        Self::Preferences,
    ];
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub section: Section,
    /// Offset within this section. Records must be contiguous, ordered and
    /// non-overlapping; transport reordering does not alter these indices.
    pub offset: u64,
    pub length: u32,
    pub sha256: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionInfo {
    pub section: Section,
    pub length: u64,
    pub sha256: [u8; 32],
}

#[derive(Serialize, Deserialize)]
pub struct Manifest {
    pub version: u16,
    pub profile_instance: [u8; 16],
    pub frozen_revision: u64,
    /// All sections appear exactly once in the order of Section::ALL, even
    /// when empty. A missing domain never silently means 'nothing to import'.
    pub sections: List<SectionInfo, 9>,
    pub records: List<Record, MAX_RECORDS>,
}

/// Immutable, validated layout. It is not a receipt for payload verification
/// or storage durability. Callers cannot mutate offsets after validation.
pub struct Layout {
    manifest: Manifest,
    digest: [u8; 32],
}

impl Manifest {
    /// Validate layout only. All section decoders, cross-references, payload
    /// hashes and receiver durable readbacks remain required before Prepared.
    pub fn validate(&self) -> Result<u64, Error> {
        if self.version != VERSION {
            return Err(Error::UnsupportedVersion);
        }
        if self.profile_instance == [0; 16] {
            return Err(Error::InvalidManifest);
        }
        if self.sections.len() != Section::ALL.len() {
            return Err(Error::MissingSection);
        }
        let empty: [u8; 32] = Sha256::digest([]).into();
        let mut total = 0u64;
        let mut index = 0;
        for (info, expected) in self.sections.iter().zip(Section::ALL) {
            if info.section != expected
                || info.sha256 == [0; 32]
                || (info.length == 0 && info.sha256 != empty)
            {
                return Err(Error::InvalidManifest);
            }
            if expected == Section::Crypto
                && (info.length == 0 || info.length > super::MAX_METADATA_BYTES)
            {
                return Err(Error::Limit);
            }
            total = total.checked_add(info.length).ok_or(Error::Overflow)?;
            if total > MAX_TRANSFER_BYTES {
                return Err(Error::Limit);
            }
            let mut offset = 0u64;
            while let Some(record) = self.records.get(index).filter(|r| r.section == expected) {
                if record.offset != offset
                    || record.length == 0
                    || record.length > MAX_RECORD_BYTES
                    || record.sha256 == [0; 32]
                {
                    return Err(Error::InvalidManifest);
                }
                offset = offset
                    .checked_add(u64::from(record.length))
                    .ok_or(Error::Overflow)?;
                if offset > info.length {
                    return Err(Error::InvalidManifest);
                }
                index += 1;
            }
            if offset != info.length {
                return Err(Error::InvalidManifest);
            }
        }
        if index != self.records.len() {
            return Err(Error::InvalidManifest);
        }
        Ok(total)
    }
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        Ok(encode(self, MAX_MANIFEST_BYTES)?.to_vec())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let manifest: Self = decode(bytes, MAX_MANIFEST_BYTES)?;
        manifest.validate()?;
        Ok(manifest)
    }
    pub fn digest(&self) -> Result<[u8; 32], Error> {
        let mut hash = Sha256::new();
        hash.update(DIGEST_DOMAIN);
        hash.update(self.encode()?);
        Ok(hash.finalize().into())
    }
    pub fn into_layout(self) -> Result<Layout, Error> {
        let digest = self.digest()?;
        Ok(Layout {
            manifest: self,
            digest,
        })
    }
}

impl Layout {
    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }
    pub fn records(&self) -> &[Record] {
        &self.manifest.records
    }
    pub fn sections(&self) -> &[SectionInfo] {
        &self.manifest.sections
    }
    pub fn profile_instance(&self) -> [u8; 16] {
        self.manifest.profile_instance
    }
    pub fn frozen_revision(&self) -> u64 {
        self.manifest.frozen_revision
    }

    /// Checks a complete record, not an incomplete ciphertext fragment. The
    /// caller still owes AEAD verification, local sealing, flush and bitmap CAS.
    pub fn verify_record(&self, index: usize, plain: &[u8]) -> Result<(), Error> {
        let record = self
            .manifest
            .records
            .get(index)
            .ok_or(Error::InvalidManifest)?;
        if plain.len() != record.length as usize
            || Sha256::digest(plain).as_slice() != record.sha256
        {
            return Err(Error::InvalidManifest);
        }
        Ok(())
    }
}
