//! Versioned, platform-neutral transfer data. No profile is opened or imported here.
//!
//! These are bounded schema components, not a complete exporter. The manifest
//! describes authenticated records; checking it does not prove their existence,
//! durability, AEAD validity or full-profile semantic completeness.

mod bounded;
pub mod crypto;
pub mod manifest;

pub use bounded::List;

use bincode::Options;
use serde::{de::DeserializeOwned, Serialize};
use zeroize::Zeroizing;

pub const VERSION: u16 = 1;
/// IDs must roundtrip through both native integers and current JS metadata.
pub const MAX_ID: u64 = (1u64 << 53) - 1;
pub const MAX_METADATA_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    UnsupportedVersion,
    Limit,
    Codec,
    InvalidIdentity,
    InvalidPrekey,
    InvalidPeer,
    InvalidRatchet,
    Duplicate,
    InvalidManifest,
    MissingSection,
    Overflow,
}

fn options(limit: u64) -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .reject_trailing_bytes()
        .with_limit(limit)
}

fn encode<T: Serialize>(value: &T, limit: u64) -> Result<Zeroizing<Vec<u8>>, Error> {
    // Fixed schema serializers do not change length between passes. Reserve
    // the bounded exact size so Vec growth does not leave freed secret copies.
    let size = options(limit)
        .serialized_size(value)
        .map_err(|_| Error::Codec)?;
    let size = usize::try_from(size).map_err(|_| Error::Limit)?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(size));
    options(limit)
        .serialize_into(&mut *bytes, value)
        .map_err(|_| Error::Codec)?;
    Ok(bytes)
}

fn decode<T: DeserializeOwned>(bytes: &[u8], limit: u64) -> Result<T, Error> {
    if bytes.len() as u64 > limit {
        return Err(Error::Limit);
    }
    options(limit).deserialize(bytes).map_err(|_| Error::Codec)
}

#[cfg(test)]
mod tests;
