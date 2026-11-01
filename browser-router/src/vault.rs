//! Encrypted browser profile records, independent of OPFS I/O.
//!
//! The worker writes alternating slots and acknowledges a transaction only
//! after flush. The outer checksum detects torn writes before credential use;
//! AEAD authenticates the complete header, generation and profile contents.
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, Payload},
    KeyInit, XChaCha20Poly1305, XNonce,
};
use gipny_libcore::crypto::random_array;
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

const MAGIC: &[u8; 8] = b"GIPNYBW1";
const MAX_HEADER: usize = 32 * 1024;
pub(crate) const MAX_RECORD: usize = 16 * 1024 * 1024;
const PASSWORD_CONTEXT: &[u8] = b"gipny/browser/password-wrap/v1";
const PASSKEY_CONTEXT: &[u8] = b"gipny/browser/passkey-wrap/v1";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WrappedKey {
    nonce: [u8; 24],
    ciphertext: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Passkey {
    pub credential_id: Vec<u8>,
    pub rp_id: String,
    pub salt: [u8; 32],
    wrapped: WrappedKey,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    version: u8,
    profile: String,
    memory_kib: u32,
    iterations: u32,
    salt: [u8; 16],
    password: WrappedKey,
    passkeys: Vec<Passkey>,
}

#[derive(Clone)]
pub(crate) struct Vault {
    header: Header,
    key: Zeroizing<[u8; 32]>,
    pub generation: u64,
}

pub(crate) struct Record<'a> {
    header: Header,
    pub generation: u64,
    aad: &'a [u8],
    nonce: &'a [u8],
    ciphertext: &'a [u8],
}

fn password_key(pass: &str, salt: &[u8; 16]) -> Result<Zeroizing<[u8; 32]>, String> {
    if pass.is_empty() || pass.len() > 4096 {
        return Err("invalid passphrase length".into());
    }
    let params = Params::new(65536, 3, 1, Some(32)).map_err(|_| "browser KDF parameters")?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0; 32]);
    let count = argon.params().block_count();
    let mut blocks = Zeroizing::new(Vec::<argon2::Block>::new());
    blocks
        .try_reserve_exact(count)
        .map_err(|_| "browser KDF memory unavailable")?;
    blocks.resize_with(count, argon2::Block::default);
    argon
        .hash_password_into_with_memory(pass.as_bytes(), salt, key.as_mut_slice(), &mut *blocks)
        .map_err(|_| "browser KDF failed")?;
    Ok(key)
}

fn password_aad(header: &Header) -> Vec<u8> {
    [
        PASSWORD_CONTEXT,
        header.profile.as_bytes(),
        &header.salt,
        &header.memory_kib.to_le_bytes(),
        &header.iterations.to_le_bytes(),
    ]
    .concat()
}

fn wrap(key: &[u8; 32], secret: &[u8; 32], aad: &[u8]) -> Result<WrappedKey, String> {
    let nonce = random_array();
    let ciphertext = XChaCha20Poly1305::new(key.into())
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: secret, aad })
        .map_err(|_| "profile key wrapping failed")?;
    Ok(WrappedKey { nonce, ciphertext })
}

fn unwrap(key: &[u8; 32], wrapped: &WrappedKey, aad: &[u8]) -> Result<Zeroizing<[u8; 32]>, String> {
    let secret = Zeroizing::new(
        XChaCha20Poly1305::new(key.into())
            .decrypt(
                XNonce::from_slice(&wrapped.nonce),
                Payload {
                    msg: &wrapped.ciphertext,
                    aad,
                },
            )
            .map_err(|_| "wrong credentials or damaged profile")?,
    );
    Ok(Zeroizing::new(
        secret
            .as_slice()
            .try_into()
            .map_err(|_| "profile key length")?,
    ))
}

fn passkey_key(
    profile: &str,
    credential_id: &[u8],
    salt: &[u8; 32],
    prf: &[u8],
) -> Result<Zeroizing<[u8; 32]>, String> {
    if prf.len() != 32 || credential_id.is_empty() || credential_id.len() > 1024 {
        return Err("invalid passkey PRF result".into());
    }
    let mut key = Zeroizing::new([0; 32]);
    let info = bincode::serialize(&(PASSKEY_CONTEXT, profile, credential_id))
        .map_err(|_| "passkey context")?;
    Hkdf::<Sha256>::new(Some(salt), prf)
        .expand(&info, key.as_mut_slice())
        .map_err(|_| "passkey KDF")?;
    Ok(key)
}

impl Vault {
    pub fn create(profile: &str, pass: &str) -> Result<Self, String> {
        if profile.is_empty() || profile.len() > 128 || profile.chars().any(char::is_control) {
            return Err("invalid profile name".into());
        }
        let salt = random_array();
        let key = Zeroizing::new(random_array());
        let kek = password_key(pass, &salt)?;
        // The initial wrap is replaced before the header is ever serialized.
        let mut header = Header {
            version: 1,
            profile: profile.into(),
            memory_kib: 65536,
            iterations: 3,
            salt,
            password: WrappedKey {
                nonce: random_array(),
                ciphertext: Vec::new(),
            },
            passkeys: Vec::new(),
        };
        header.password = wrap(&kek, &key, &password_aad(&header))?;
        Ok(Self {
            header,
            key,
            generation: 0,
        })
    }

    pub fn open_password(
        profile: &str,
        pass: &str,
        bytes: &[u8],
    ) -> Result<(Self, Zeroizing<Vec<u8>>), String> {
        let record = Record::parse(bytes)?;
        if record.header.profile != profile {
            return Err("profile name mismatch".into());
        }
        let kek = password_key(pass, &record.header.salt)?;
        let key = unwrap(&kek, &record.header.password, &password_aad(&record.header))?;
        Self::open_record(key, record)
    }

    pub fn open_passkey(
        profile: &str,
        credential_id: &[u8],
        prf: &[u8],
        bytes: &[u8],
    ) -> Result<(Self, Zeroizing<Vec<u8>>), String> {
        let record = Record::parse(bytes)?;
        if record.header.profile != profile {
            return Err("profile name mismatch".into());
        }
        let entry = record
            .header
            .passkeys
            .iter()
            .find(|entry| entry.credential_id == credential_id)
            .ok_or("passkey is not enrolled for this profile")?;
        let kek = passkey_key(profile, credential_id, &entry.salt, prf)?;
        let aad = bincode::serialize(&(
            PASSKEY_CONTEXT,
            profile,
            credential_id,
            entry.rp_id.as_str(),
            entry.salt,
        ))
        .map_err(|_| "passkey context")?;
        let key = unwrap(&kek, &entry.wrapped, &aad)?;
        Self::open_record(key, record)
    }

    fn open_record(
        key: Zeroizing<[u8; 32]>,
        record: Record<'_>,
    ) -> Result<(Self, Zeroizing<Vec<u8>>), String> {
        let plaintext = Zeroizing::new(
            XChaCha20Poly1305::new((&*key).into())
                .decrypt(
                    XNonce::from_slice(record.nonce),
                    Payload {
                        msg: record.ciphertext,
                        aad: record.aad,
                    },
                )
                .map_err(|_| "profile authentication failed")?,
        );
        Ok((
            Self {
                header: record.header,
                key,
                generation: record.generation,
            },
            plaintext,
        ))
    }

    pub fn seal_next(&self, plaintext: &[u8]) -> Result<(u64, Vec<u8>), String> {
        if plaintext.len() > MAX_RECORD - MAX_HEADER - 128 {
            return Err("profile storage limit reached".into());
        }
        let generation = self
            .generation
            .checked_add(1)
            .ok_or("profile generation overflow")?;
        let header =
            serde_json::to_vec(&self.header).map_err(|_| "profile header serialization")?;
        if header.len() > MAX_HEADER {
            return Err("profile header size limit".into());
        }
        let nonce: [u8; 24] = random_array();
        let mut bytes = Vec::with_capacity(header.len() + plaintext.len() + 96);
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&generation.to_le_bytes());
        bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&nonce);
        bytes.extend_from_slice(&((plaintext.len() + 16) as u32).to_le_bytes());
        let ciphertext = XChaCha20Poly1305::new((&*self.key).into())
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &bytes,
                },
            )
            .map_err(|_| "profile encryption failed")?;
        bytes.extend_from_slice(&ciphertext);
        bytes.extend_from_slice(&Sha256::digest(&bytes).to_vec());
        Ok((generation, bytes))
    }

    pub fn committed(&mut self, generation: u64) -> Result<(), String> {
        if self.generation.checked_add(1) != Some(generation) {
            return Err("profile commit order mismatch".into());
        }
        self.generation = generation;
        Ok(())
    }

    pub fn verify_password(&self, pass: &str) -> Result<(), String> {
        let kek = password_key(pass, &self.header.salt)?;
        let candidate = unwrap(&kek, &self.header.password, &password_aad(&self.header))?;
        if *candidate != *self.key {
            return Err("wrong passphrase".into());
        }
        Ok(())
    }

    pub fn change_password(&mut self, old: &str, new: &str) -> Result<(), String> {
        self.verify_password(old)?;
        let mut header = self.header.clone();
        header.salt = random_array();
        let kek = password_key(new, &header.salt)?;
        header.password = wrap(&kek, &self.key, &password_aad(&header))?;
        self.header = header;
        Ok(())
    }

    pub fn enroll_passkey(
        &mut self,
        credential_id: Vec<u8>,
        rp_id: String,
        salt: [u8; 32],
        prf: &[u8],
    ) -> Result<(), String> {
        if self.header.passkeys.len() >= 8
            || rp_id.is_empty()
            || rp_id.len() > 253
            || self
                .header
                .passkeys
                .iter()
                .any(|entry| entry.credential_id == credential_id)
        {
            return Err("invalid or duplicate passkey".into());
        }
        let kek = passkey_key(&self.header.profile, &credential_id, &salt, prf)?;
        let aad = bincode::serialize(&(
            PASSKEY_CONTEXT,
            self.header.profile.as_str(),
            &credential_id,
            rp_id.as_str(),
            salt,
        ))
        .map_err(|_| "passkey context")?;
        let wrapped = wrap(&kek, &self.key, &aad)?;
        self.header.passkeys.push(Passkey {
            credential_id,
            rp_id,
            salt,
            wrapped,
        });
        Ok(())
    }

    pub fn profile(&self) -> &str {
        &self.header.profile
    }

    pub fn passkeys(&self) -> &[Passkey] {
        &self.header.passkeys
    }
}

impl<'a> Record<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        if bytes.len() < 100 || bytes.len() > MAX_RECORD || &bytes[..8] != MAGIC {
            return Err("invalid browser profile record".into());
        }
        let checksum_offset = bytes.len() - 32;
        if Sha256::digest(&bytes[..checksum_offset]).as_slice() != &bytes[checksum_offset..] {
            return Err("incomplete browser profile write".into());
        }
        let generation = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let header_len = u32::from_le_bytes(bytes[16..20].try_into().unwrap()) as usize;
        if generation == 0 || header_len > MAX_HEADER || 20 + header_len + 28 > checksum_offset {
            return Err("invalid browser profile header size".into());
        }
        let header: Header = serde_json::from_slice(&bytes[20..20 + header_len])
            .map_err(|_| "invalid browser profile header")?;
        if header.version != 1
            || header.memory_kib != 65536
            || header.iterations != 3
            || header.profile.is_empty()
            || header.profile.len() > 128
            || header.password.ciphertext.len() != 48
            || header.passkeys.len() > 8
            || header.passkeys.iter().any(|entry| {
                entry.credential_id.is_empty()
                    || entry.credential_id.len() > 1024
                    || entry.rp_id.is_empty()
                    || entry.rp_id.len() > 253
                    || entry.wrapped.ciphertext.len() != 48
            })
        {
            return Err("unsupported browser profile parameters".into());
        }
        let nonce_offset = 20 + header_len;
        let aad_len = nonce_offset + 28;
        let cipher_len =
            u32::from_le_bytes(bytes[nonce_offset + 24..aad_len].try_into().unwrap()) as usize;
        if cipher_len < 16 || aad_len.checked_add(cipher_len) != Some(checksum_offset) {
            return Err("invalid browser profile payload size".into());
        }
        Ok(Self {
            header,
            generation,
            aad: &bytes[..aad_len],
            nonce: &bytes[nonce_offset..nonce_offset + 24],
            ciphertext: &bytes[aad_len..checksum_offset],
        })
    }

    pub fn profile(&self) -> &str {
        &self.header.profile
    }
    pub fn passkeys(&self) -> &[Passkey] {
        &self.header.passkeys
    }
}

/// Choose the newest complete record before trying credentials. A wrong
/// password for a new record must never silently unlock an older password.
pub(crate) fn newest<'a>(records: impl IntoIterator<Item = &'a [u8]>) -> Result<&'a [u8], String> {
    records
        .into_iter()
        .filter_map(|bytes| {
            Record::parse(bytes)
                .ok()
                .map(|record| (record.generation, bytes))
        })
        .max_by_key(|(generation, _)| *generation)
        .map(|(_, bytes)| bytes)
        .ok_or_else(|| "no complete browser profile record".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    #[wasm_bindgen_test]
    fn encrypted_profile_authenticates_and_recovers_a_torn_new_slot() {
        let mut vault = Vault::create("profile", "long test passphrase").unwrap();
        let (generation, first) = vault.seal_next(b"private state").unwrap();
        assert!(!first.windows(13).any(|bytes| bytes == b"private state"));
        vault.committed(generation).unwrap();
        let (_, second) = vault.seal_next(b"new private state").unwrap();
        let mut torn = second.clone();
        torn.truncate(torn.len() / 2);
        assert_eq!(newest([first.as_slice(), torn.as_slice()]).unwrap(), first);
        let selected = newest([first.as_slice(), second.as_slice()]).unwrap();
        let (_, plaintext) =
            Vault::open_password("profile", "long test passphrase", selected).unwrap();
        assert_eq!(plaintext.as_slice(), b"new private state");
        assert!(Vault::open_password("another profile", "long test passphrase", selected).is_err());
        assert!(Vault::open_password("profile", "wrong password", selected).is_err());
        let mut oversized = second.clone();
        let nonce_offset = 20 + u32::from_le_bytes(oversized[16..20].try_into().unwrap()) as usize;
        oversized[nonce_offset + 24..nonce_offset + 28].copy_from_slice(&u32::MAX.to_le_bytes());
        let checksum_at = oversized.len() - 32;
        let checksum = Sha256::digest(&oversized[..checksum_at]);
        oversized[checksum_at..].copy_from_slice(&checksum);
        assert!(Record::parse(&oversized).is_err());
        assert_eq!(
            newest([first.as_slice(), oversized.as_slice()]).unwrap(),
            first
        );
        let mut tampered = second;
        tampered[40] ^= 1;
        assert!(Record::parse(&tampered).is_err());
    }

    #[wasm_bindgen_test]
    fn passkey_prf_is_required_and_a_new_password_does_not_fall_back() {
        let mut vault = Vault::create("profile", "old test passphrase").unwrap();
        let (generation, old) = vault.seal_next(b"state").unwrap();
        vault.committed(generation).unwrap();
        let prf = random_array::<32>();
        let id = vec![1, 2, 3];
        vault
            .enroll_passkey(id.clone(), "example.test".into(), random_array(), &prf)
            .unwrap();
        vault
            .change_password("old test passphrase", "new test passphrase")
            .unwrap();
        let (_, updated) = vault.seal_next(b"state").unwrap();
        let selected = newest([old.as_slice(), updated.as_slice()]).unwrap();
        assert!(Vault::open_password("profile", "old test passphrase", selected).is_err());
        assert!(Vault::open_password("profile", "new test passphrase", selected).is_ok());
        assert!(Vault::open_passkey("profile", &id, &prf, selected).is_ok());
        assert!(Vault::open_passkey("profile", &id, &random_array::<32>(), selected).is_err());
        assert!(Vault::open_passkey("profile", b"another credential", &prf, selected).is_err());
    }
}
