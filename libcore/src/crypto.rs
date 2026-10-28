use std::collections::HashMap;

use chacha20poly1305::{aead::Aead, aead::Payload, KeyInit, XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::rand_core::TryRng;
use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;
use sha2::{Digest, Sha256};
use thiserror::Error;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, Zeroizing};
use crate::memory::{Sealed, SecretKind};

/// Fills `dst` from the operating system's RNG, the source every key in this
/// crate comes from. A system without a working RNG cannot make keys at all,
/// so that is a hard failure rather than something to fall back from.
pub fn fill_random(dst: &mut [u8]) {
    rand::rngs::SysRng.try_fill_bytes(dst).expect("system RNG unavailable");
}

/// `N` bytes from the operating system's RNG, as a value.
///
/// For salts, nonces and the like. There is no zeroed placeholder to forget to
/// fill, and nothing constant for a reader — or a static analyser — to follow
/// into a cryptographic parameter. Secrets that must not be copied around are
/// filled in place with [`fill_random`] instead.
pub fn random_array<const N: usize>() -> [u8; N] {
    use rand::RngExt;
    rand::rand_core::UnwrapErr(rand::rngs::SysRng).random()
}


pub type Result<T> = std::result::Result<T, CryptoError>;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("crypto")] Crypto,
    #[error("codec")] Codec,
    #[error("bad signature")] BadSignature,
    #[error("too many skipped")] TooManySkipped,
    #[error("mac")] Mac,
    #[error("state")] State,
    #[error(transparent)] Memory(#[from] crate::memory::MemoryError),
}

impl From<bincode::Error> for CryptoError { fn from(_: bincode::Error) -> Self { Self::Codec } }

const MAX_SKIP: u32 = 1_000;
const X3DH_INFO: &[u8] = b"gipny/x3dh/v1";
const RK_INFO: &[u8] = b"gipny/dr/root/v1";
const MSG_INFO: &[u8] = b"gipny/dr/msg/v1";

#[derive(Clone)]
pub struct Identity {
    secret: Sealed<64>,
    card: IdentityCard,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdentityCard {
    pub sign_pk: [u8; 32],
    pub dh_pk: [u8; 32],
}

impl Identity {
    pub fn generate() -> Self {
        Self::try_generate().expect("protected identity memory unavailable")
    }

    pub fn try_generate() -> Result<Self> { Self::from_sealed(Sealed::random(SecretKind::Identity)?) }

    pub fn from_bytes(sign_seed: [u8; 32], dh_sk: [u8; 32]) -> Self {
        Self::try_from_bytes(sign_seed, dh_sk).expect("protected identity memory unavailable")
    }

    pub fn try_from_bytes(sign_seed: [u8; 32], dh_sk: [u8; 32]) -> Result<Self> {
        let sign_seed = Zeroizing::new(sign_seed);
        let dh_sk = Zeroizing::new(dh_sk);
        let mut bytes = Zeroizing::new([0; 64]);
        bytes[..32].copy_from_slice(&*sign_seed);
        bytes[32..].copy_from_slice(&*dh_sk);
        Self::from_sealed(Sealed::from_slice(SecretKind::Identity, &*bytes)?)
    }

    fn from_sealed(secret: Sealed<64>) -> Result<Self> {
        let card = {
            let guard = secret.open()?;
            let sign: &[u8; 32] = guard[..32].try_into().map_err(|_| CryptoError::State)?;
            let dh: &[u8; 32] = guard[32..].try_into().map_err(|_| CryptoError::State)?;
            IdentityCard { sign_pk: SigningKey::from_bytes(sign).verifying_key().to_bytes(),
                dh_pk: PublicKey::from(&StaticSecret::from(*dh)).to_bytes() }
        };
        Ok(Self { secret, card })
    }

    /// Explicit synchronous export for the encrypted database boundary. Do not
    /// call another protected-key operation from the closure (one page lock).
    pub fn with_secret_bytes<R>(&self, f: impl FnOnce(&[u8; 32], &[u8; 32]) -> R) -> Result<R> {
        let guard = self.secret.open()?;
        Ok(f(guard[..32].try_into().map_err(|_| CryptoError::State)?,
            guard[32..].try_into().map_err(|_| CryptoError::State)?))
    }

    pub(crate) fn with_signing_key<R>(&self, f: impl FnOnce(&SigningKey) -> R) -> Result<R> {
        self.with_secret_bytes(|sign, _| f(&SigningKey::from_bytes(sign)))
    }

    fn static_secret(&self) -> Result<StaticSecret> {
        self.with_secret_bytes(|_, dh| StaticSecret::from(*dh))
    }

    pub fn card(&self) -> IdentityCard { self.card.clone() }
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.with_signing_key(|key| key.sign(msg).to_bytes()).expect("protected identity access failed")
    }
    pub fn fingerprint(&self) -> [u8; 32] { self.card().fingerprint() }
}

impl IdentityCard {
    pub fn verify(&self, msg: &[u8], sig: &[u8; 64]) -> bool {
        let Ok(vk) = VerifyingKey::from_bytes(&self.sign_pk) else { return false };
        vk.verify(msg, &Signature::from_bytes(sig)).is_ok()
    }
    pub fn fingerprint(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(&self.sign_pk);
        h.update(&self.dh_pk);
        h.finalize().into()
    }
}

#[derive(Clone)]
pub struct PreKeyPair {
    secret: Sealed<32>,
    public: [u8; 32],
}

impl PreKeyPair {
    pub fn generate() -> Self {
        Self::try_generate().expect("protected prekey memory unavailable")
    }
    pub fn try_generate() -> Result<Self> {
        let secret = Sealed::random(SecretKind::PreKey)?;
        Self::from_sealed(secret)
    }
    pub fn from_secret(secret: [u8; 32]) -> Self {
        Self::try_from_secret(secret).expect("protected prekey memory unavailable")
    }
    pub fn try_from_secret(secret: [u8; 32]) -> Result<Self> {
        let secret = Zeroizing::new(secret);
        Self::from_sealed(Sealed::from_slice(SecretKind::PreKey, &*secret)?)
    }
    fn from_sealed(secret: Sealed<32>) -> Result<Self> {
        let public = { let guard = secret.open()?; PublicKey::from(&StaticSecret::from(*guard)).to_bytes() };
        Ok(Self { secret, public })
    }
    pub fn expose_secret(&self) -> Result<crate::memory::SecretGuard<'_, 32>> { Ok(self.secret.open()?) }
    pub fn public(&self) -> &[u8; 32] { &self.public }
    fn static_secret(&self) -> Result<StaticSecret> { Ok(StaticSecret::from(*self.secret.open()?)) }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreKeyBundle {
    pub identity: IdentityCard,
    pub signed_prekey: [u8; 32],
    #[serde(with = "BigArray")]
    pub signed_prekey_sig: [u8; 64],
    pub one_time_prekey: Option<[u8; 32]>,
    pub one_time_id: Option<i64>,
}

impl PreKeyBundle {
    pub fn new(identity: &Identity, signed: &PreKeyPair, one_time: Option<(i64, &PreKeyPair)>) -> Self {
        let sig = identity.sign(signed.public());
        Self {
            identity: identity.card(),
            signed_prekey: *signed.public(),
            signed_prekey_sig: sig,
            one_time_prekey: one_time.map(|(_, k)| *k.public()),
            one_time_id: one_time.map(|(id, _)| id),
        }
    }
    pub fn verify(&self) -> Result<()> {
        if !self.identity.verify(&self.signed_prekey, &self.signed_prekey_sig) {
            return Err(CryptoError::BadSignature);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct X3dhInitial {
    pub identity: IdentityCard,
    pub ephemeral: [u8; 32],
    pub one_time_id: Option<i64>,
    pub header: RatchetHeader,
    pub ciphertext: Vec<u8>,
}

fn x3dh_ikm_initiator(
    my_ik_sk: &StaticSecret,
    my_eph_sk: &StaticSecret,
    their_ik_dh: &PublicKey,
    their_spk: &PublicKey,
    their_opk: Option<&PublicKey>,
) -> Zeroizing<[u8; 32]> {
    let dh1 = my_ik_sk.diffie_hellman(their_spk);
    let dh2 = my_eph_sk.diffie_hellman(their_ik_dh);
    let dh3 = my_eph_sk.diffie_hellman(their_spk);
    let mut ikm = Zeroizing::new(Vec::with_capacity(160));
    ikm.extend_from_slice(&[0xffu8; 32]);
    ikm.extend_from_slice(dh1.as_bytes());
    ikm.extend_from_slice(dh2.as_bytes());
    ikm.extend_from_slice(dh3.as_bytes());
    if let Some(opk) = their_opk {
        let dh4 = my_eph_sk.diffie_hellman(opk);
        ikm.extend_from_slice(dh4.as_bytes());
    }
    let hk = Hkdf::<Sha256>::new(Some(&[0u8; 32]), &ikm);
    let mut sk = Zeroizing::new([0u8; 32]);
    hk.expand(X3DH_INFO, sk.as_mut_slice()).expect("hkdf");
    sk
}

fn x3dh_ikm_responder(
    my_ik_sk: &StaticSecret,
    my_spk_sk: &StaticSecret,
    my_opk_sk: Option<&StaticSecret>,
    their_ik_dh: &PublicKey,
    their_eph: &PublicKey,
) -> Zeroizing<[u8; 32]> {
    let dh1 = my_spk_sk.diffie_hellman(their_ik_dh);
    let dh2 = my_ik_sk.diffie_hellman(their_eph);
    let dh3 = my_spk_sk.diffie_hellman(their_eph);
    let mut ikm = Zeroizing::new(Vec::with_capacity(160));
    ikm.extend_from_slice(&[0xffu8; 32]);
    ikm.extend_from_slice(dh1.as_bytes());
    ikm.extend_from_slice(dh2.as_bytes());
    ikm.extend_from_slice(dh3.as_bytes());
    if let Some(opk) = my_opk_sk {
        let dh4 = opk.diffie_hellman(their_eph);
        ikm.extend_from_slice(dh4.as_bytes());
    }
    let hk = Hkdf::<Sha256>::new(Some(&[0u8; 32]), &ikm);
    let mut sk = Zeroizing::new([0u8; 32]);
    hk.expand(X3DH_INFO, sk.as_mut_slice()).expect("hkdf");
    sk
}

pub fn x3dh_initiate(
    mine: &Identity,
    their: &PreKeyBundle,
    plaintext: &[u8],
    ad: &[u8],
) -> Result<(RatchetState, X3dhInitial)> {
    their.verify()?;
    let mut eph_bytes = Zeroizing::new([0u8; 32]);
    fill_random(eph_bytes.as_mut_slice());
    let eph_sk = StaticSecret::from(*eph_bytes);
    let eph_pk = PublicKey::from(&eph_sk);
    let their_ik_dh = PublicKey::from(their.identity.dh_pk);
    let their_spk = PublicKey::from(their.signed_prekey);
    let their_opk = their.one_time_prekey.map(PublicKey::from);
    let sk = x3dh_ikm_initiator(
        &mine.static_secret()?, &eph_sk,
        &their_ik_dh, &their_spk, their_opk.as_ref(),
    );
    let mut state = RatchetState::try_new_alice(&sk, &their.signed_prekey)?;
    let (header, ct) = state.encrypt(plaintext, ad)?;
    Ok((state, X3dhInitial {
        identity: mine.card(),
        ephemeral: eph_pk.to_bytes(),
        one_time_id: their.one_time_id,
        header,
        ciphertext: ct,
    }))
}

pub fn x3dh_respond(
    mine: &Identity,
    my_spk: &PreKeyPair,
    my_opk: Option<&PreKeyPair>,
    initial: &X3dhInitial,
    ad: &[u8],
) -> Result<(RatchetState, Vec<u8>)> {
    let their_ik_dh = PublicKey::from(initial.identity.dh_pk);
    let their_eph = PublicKey::from(initial.ephemeral);
    let my_opk_sk = my_opk.map(|p| p.static_secret()).transpose()?;
    let sk = x3dh_ikm_responder(
        &mine.static_secret()?,
        &my_spk.static_secret()?,
        my_opk_sk.as_ref(),
        &their_ik_dh, &their_eph,
    );
    let mut state = RatchetState::try_new_bob(&sk, my_spk)?;
    let pt = state.decrypt(&initial.header, &initial.ciphertext, ad)?;
    Ok((state, pt))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RatchetHeader {
    pub dh: [u8; 32],
    pub pn: u32,
    pub n: u32,
}

#[derive(Serialize, Deserialize)]
pub struct RatchetState {
    #[serde(with = "sealed32")]
    dhs_sk: Sealed<32>,
    dhs_pk: [u8; 32],
    dhr: Option<[u8; 32]>,
    #[serde(with = "sealed32")]
    rk: Sealed<32>,
    #[serde(with = "sealed_option")]
    cks: Option<Sealed<32>>,
    #[serde(with = "sealed_option")]
    ckr: Option<Sealed<32>>,
    ns: u32,
    nr: u32,
    pn: u32,
    #[serde(with = "sealed_skipped")]
    skipped: HashMap<([u8; 32], u32), Sealed<32>>,
}

impl Clone for RatchetState {
    fn clone(&self) -> Self {
        Self {
            dhs_sk: self.dhs_sk.clone(),
            dhs_pk: self.dhs_pk,
            dhr: self.dhr,
            rk: self.rk.clone(),
            cks: self.cks.clone(),
            ckr: self.ckr.clone(),
            ns: self.ns,
            nr: self.nr,
            pn: self.pn,
            skipped: self.skipped.clone(),
        }
    }
}

impl RatchetState {
    pub fn new_alice(sk: &[u8; 32], their_dh_pk: &[u8; 32]) -> Self {
        Self::try_new_alice(sk, their_dh_pk).expect("protected ratchet memory unavailable")
    }

    pub fn try_new_alice(sk: &[u8; 32], their_dh_pk: &[u8; 32]) -> Result<Self> {
        let dhs_sk = Sealed::<32>::random(SecretKind::Ratchet)?;
        let (dhs_pk, dh) = {
            let guard = dhs_sk.open()?;
            let secret = StaticSecret::from(*guard);
            (PublicKey::from(&secret).to_bytes(), secret.diffie_hellman(&PublicKey::from(*their_dh_pk)))
        };
        let (rk, cks) = kdf_rk(sk, dh.as_bytes());
        let rk = seal_ratchet(rk)?;
        let cks = seal_ratchet(cks)?;
        Ok(Self {
            dhs_sk, dhs_pk, dhr: Some(*their_dh_pk),
            rk, cks: Some(cks), ckr: None,
            ns: 0, nr: 0, pn: 0,
            skipped: HashMap::new(),
        })
    }

    pub fn new_bob(sk: &[u8; 32], spk: &PreKeyPair) -> Self {
        Self::try_new_bob(sk, spk).expect("protected ratchet memory unavailable")
    }

    pub fn try_new_bob(sk: &[u8; 32], spk: &PreKeyPair) -> Result<Self> {
        Ok(Self {
            // Both have the same 32-byte meaning; clones retain ciphertext.
            dhs_sk: spk.secret.clone(),
            dhs_pk: *spk.public(),
            dhr: None,
            rk: Sealed::from_slice(SecretKind::Ratchet, sk)?,
            cks: None, ckr: None,
            ns: 0, nr: 0, pn: 0,
            skipped: HashMap::new(),
        })
    }

    pub fn encrypt(&mut self, plaintext: &[u8], ad: &[u8]) -> Result<(RatchetHeader, Vec<u8>)> {
        let mut probe = self.clone();
        let result = probe.encrypt_inner(plaintext, ad)?;
        *self = probe;
        Ok(result)
    }

    fn encrypt_inner(&mut self, plaintext: &[u8], ad: &[u8]) -> Result<(RatchetHeader, Vec<u8>)> {
        let (next, mk) = {
            let cks = self.cks.as_ref().ok_or(CryptoError::State)?.open()?;
            kdf_ck(&cks)
        };
        self.cks = Some(seal_ratchet(next)?);
        let header = RatchetHeader { dh: self.dhs_pk, pn: self.pn, n: self.ns };
        self.ns = self.ns.checked_add(1).ok_or(CryptoError::State)?;
        let ct = aead_encrypt(&mk, ad, &header, plaintext)?;
        Ok((header, ct))
    }

    pub fn decrypt(&mut self, header: &RatchetHeader, ct: &[u8], ad: &[u8]) -> Result<Vec<u8>> {
        let mut probe = self.clone();
        let pt = probe.decrypt_inner(header, ct, ad)?;
        *self = probe;
        Ok(pt)
    }

    fn decrypt_inner(&mut self, header: &RatchetHeader, ct: &[u8], ad: &[u8]) -> Result<Vec<u8>> {
        if let Some(mk) = self.skipped.remove(&(header.dh, header.n)) {
            let guard = mk.open()?;
            return aead_decrypt(&guard, ad, header, ct);
        }
        if Some(header.dh) != self.dhr {
            self.skip_message_keys(header.pn)?;
            self.dh_ratchet(header)?;
        }
        self.skip_message_keys(header.n)?;
        let (next, mk) = {
            let ckr = self.ckr.as_ref().ok_or(CryptoError::State)?.open()?;
            kdf_ck(&ckr)
        };
        self.ckr = Some(seal_ratchet(next)?);
        self.nr = self.nr.checked_add(1).ok_or(CryptoError::State)?;
        aead_decrypt(&mk, ad, header, ct)
    }

    fn skip_message_keys(&mut self, until: u32) -> Result<()> {
        let gap = until.saturating_sub(self.nr);
        if gap > MAX_SKIP || self.skipped.len().saturating_add(gap as usize) > MAX_SKIP as usize {
            return Err(CryptoError::TooManySkipped);
        }
        if self.ckr.is_some() {
            let dhr = self.dhr.ok_or(CryptoError::State)?;
            while self.nr < until {
                let (next, mk) = {
                    let ckr = self.ckr.as_ref().ok_or(CryptoError::State)?.open()?;
                    kdf_ck(&ckr)
                };
                self.ckr = Some(seal_ratchet(next)?);
                self.skipped.insert((dhr, self.nr), seal_ratchet(mk)?);
                self.nr += 1;
            }
        }
        Ok(())
    }

    fn dh_ratchet(&mut self, header: &RatchetHeader) -> Result<()> {
        self.pn = self.ns;
        self.ns = 0;
        self.nr = 0;
        self.dhr = Some(header.dh);
        let their_dh = PublicKey::from(header.dh);
        let dh_in = { let guard = self.dhs_sk.open()?; StaticSecret::from(*guard).diffie_hellman(&their_dh) };
        let (rk, ckr) = { let guard = self.rk.open()?; kdf_rk(&guard, dh_in.as_bytes()) };
        self.rk = seal_ratchet(rk)?;
        self.ckr = Some(seal_ratchet(ckr)?);
        self.dhs_sk = Sealed::random(SecretKind::Ratchet)?;
        let dh_out = {
            let guard = self.dhs_sk.open()?;
            let secret = StaticSecret::from(*guard);
            self.dhs_pk = PublicKey::from(&secret).to_bytes();
            secret.diffie_hellman(&their_dh)
        };
        let (rk2, cks) = { let guard = self.rk.open()?; kdf_rk(&guard, dh_out.as_bytes()) };
        self.rk = seal_ratchet(rk2)?;
        self.cks = Some(seal_ratchet(cks)?);
        Ok(())
    }

    pub fn to_bytes(&self) -> Result<Zeroizing<Vec<u8>>> {
        bincode::serialize(self).map(Zeroizing::new).map_err(Into::into)
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> { bincode::deserialize(bytes).map_err(Into::into) }
}

fn seal_ratchet(key: Zeroizing<[u8; 32]>) -> Result<Sealed<32>> {
    Ok(Sealed::from_slice(SecretKind::Ratchet, &*key)?)
}

// These adapters preserve the pre-existing serde/bincode field layout. Only
// one fixed-size plaintext entry is exposed at a time, at the SQLCipher storage
// boundary. They never serialize the in-memory wrapping key or its metadata.
struct SecretWire(Sealed<32>);
struct SecretWireRef<'a>(&'a Sealed<32>);

impl Serialize for SecretWireRef<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        sealed32::serialize(self.0, serializer)
    }
}

impl<'de> Deserialize<'de> for SecretWire {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        sealed32::deserialize(deserializer).map(Self)
    }
}

mod sealed32 {
    use super::*;
    pub fn serialize<S: serde::Serializer>(key: &Sealed<32>, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let guard = key.open().map_err(serde::ser::Error::custom)?;
        serde::Serialize::serialize(&*guard, serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Sealed<32>, D::Error> {
        let key = Zeroizing::new(<[u8; 32] as serde::Deserialize>::deserialize(deserializer)?);
        Sealed::from_slice(SecretKind::Ratchet, &*key).map_err(serde::de::Error::custom)
    }
}

mod sealed_option {
    use super::*;
    pub fn serialize<S: serde::Serializer>(key: &Option<Sealed<32>>, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match key { Some(key) => serializer.serialize_some(&SecretWireRef(key)), None => serializer.serialize_none() }
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Option<Sealed<32>>, D::Error> {
        Ok(Option::<SecretWire>::deserialize(deserializer)?.map(|wire| wire.0))
    }
}

mod sealed_skipped {
    use super::*;
    use serde::ser::SerializeMap;
    type Keys = HashMap<([u8; 32], u32), Sealed<32>>;
    pub fn serialize<S: serde::Serializer>(keys: &Keys, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(keys.len()))?;
        for (id, key) in keys { map.serialize_entry(id, &SecretWireRef(key))?; }
        map.end()
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Keys, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Keys;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("bounded skipped message keys") }
            fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> std::result::Result<Keys, M::Error> {
                let mut out = HashMap::new();
                while let Some((id, SecretWire(key))) = map.next_entry()? {
                    if out.len() >= MAX_SKIP as usize { return Err(serde::de::Error::custom("too many skipped keys")); }
                    if out.insert(id, key).is_some() { return Err(serde::de::Error::custom("duplicate skipped key")); }
                }
                Ok(out)
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}

fn kdf_rk(rk: &[u8; 32], dh_out: &[u8]) -> (Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>) {
    let hk = Hkdf::<Sha256>::new(Some(rk), dh_out);
    let mut out = [0u8; 64];
    hk.expand(RK_INFO, &mut out).expect("hkdf");
    // Arrays taken from the HKDF output rather than zeroed and copied over:
    // the zeroed placeholders read, to a reviewer and to static analysis alike,
    // as constants that reach a key.
    let (new_rk, ck) = split_32(&out);
    out.zeroize();
    (Zeroizing::new(new_rk), Zeroizing::new(ck))
}

fn kdf_ck(ck: &[u8; 32]) -> (Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>) {
    let mut m1 = <Hmac<Sha256> as KeyInit>::new_from_slice(ck).expect("hmac");
    m1.update(&[0x02]);
    let next: [u8; 32] = m1.finalize().into_bytes().into();
    let mut m2 = <Hmac<Sha256> as KeyInit>::new_from_slice(ck).expect("hmac");
    m2.update(&[0x01]);
    let mk: [u8; 32] = m2.finalize().into_bytes().into();
    (Zeroizing::new(next), Zeroizing::new(mk))
}

fn aead_from_mk(mk: &[u8; 32]) -> (Zeroizing<[u8; 32]>, [u8; 24]) {
    let hk = Hkdf::<Sha256>::new(Some(&[0u8; 32]), mk);
    let mut out = [0u8; 56];
    hk.expand(MSG_INFO, &mut out).expect("hkdf");
    let (key, nonce) = out.split_at(32);
    let pair = (
        Zeroizing::new(<[u8; 32]>::try_from(key).expect("hkdf output is 56 bytes")),
        <[u8; 24]>::try_from(nonce).expect("hkdf output is 56 bytes"),
    );
    out.zeroize();
    pair
}

/// The two 32-byte halves of a 64-byte KDF output.
fn split_32(out: &[u8; 64]) -> ([u8; 32], [u8; 32]) {
    let (a, b) = out.split_at(32);
    (
        <[u8; 32]>::try_from(a).expect("first half of 64 bytes"),
        <[u8; 32]>::try_from(b).expect("second half of 64 bytes"),
    )
}

fn build_aad(ad: &[u8], header: &RatchetHeader) -> Vec<u8> {
    let mut aad = Vec::with_capacity(ad.len() + 40);
    aad.extend_from_slice(ad);
    aad.extend_from_slice(&header.dh);
    aad.extend_from_slice(&header.pn.to_le_bytes());
    aad.extend_from_slice(&header.n.to_le_bytes());
    aad
}

fn aead_encrypt(mk: &[u8; 32], ad: &[u8], header: &RatchetHeader, plaintext: &[u8]) -> Result<Vec<u8>> {
    let (key, nonce) = aead_from_mk(mk);
    let cipher = XChaCha20Poly1305::new((&*key).into());
    let aad = build_aad(ad, header);
    cipher.encrypt(&XNonce::from(nonce), Payload { msg: plaintext, aad: &aad })
        .map_err(|_| CryptoError::Crypto)
}

fn aead_decrypt(mk: &[u8; 32], ad: &[u8], header: &RatchetHeader, ct: &[u8]) -> Result<Vec<u8>> {
    let (key, nonce) = aead_from_mk(mk);
    let cipher = XChaCha20Poly1305::new((&*key).into());
    let aad = build_aad(ad, header);
    cipher.decrypt(&XNonce::from(nonce), Payload { msg: ct, aad: &aad })
        .map_err(|_| CryptoError::Mac)
}

pub struct AttachmentCipher { key: Sealed<32> }

impl AttachmentCipher {
    pub fn generate() -> Self {
        Self { key: Sealed::random(SecretKind::Ratchet).expect("protected attachment memory unavailable") }
    }
    pub fn from_key(key: [u8; 32]) -> Self {
        let key = Zeroizing::new(key);
        Self { key: Sealed::from_slice(SecretKind::Ratchet, &*key).expect("protected attachment memory unavailable") }
    }
    /// Explicit short-lived export for attachment storage/wire metadata. The
    /// returned copy is wiped on Drop; this is not a long-lived key container.
    pub fn key(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(*self.key.open().expect("protected attachment access failed"))
    }

    fn nonce(idx: u64) -> [u8; 24] {
        let mut n = [0u8; 24];
        n[..8].copy_from_slice(&idx.to_le_bytes());
        n
    }

    pub fn encrypt_chunk(&self, idx: u64, aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
        let guard = self.key.open()?;
        let cipher = XChaCha20Poly1305::new((&*guard).into());
        cipher.encrypt(&XNonce::from(Self::nonce(idx)), Payload { msg: plaintext, aad })
            .map_err(|_| CryptoError::Crypto)
    }

    pub fn decrypt_chunk(&self, idx: u64, aad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
        let guard = self.key.open()?;
        let cipher = XChaCha20Poly1305::new((&*guard).into());
        cipher.decrypt(&XNonce::from(Self::nonce(idx)), Payload { msg: ciphertext, aad })
            .map_err(|_| CryptoError::Mac)
    }
}

#[cfg(test)]
mod protected_state_tests {
    use super::*;

    // The exact original field order/types, independent of the new adapters.
    #[derive(Serialize, Deserialize)]
    struct LegacyRatchet {
        dhs_sk: [u8; 32], dhs_pk: [u8; 32], dhr: Option<[u8; 32]>,
        rk: [u8; 32], cks: Option<[u8; 32]>, ckr: Option<[u8; 32]>,
        ns: u32, nr: u32, pn: u32,
        skipped: HashMap<([u8; 32], u32), [u8; 32]>,
    }

    fn legacy() -> LegacyRatchet {
        LegacyRatchet { dhs_sk: [1; 32], dhs_pk: [2; 32], dhr: Some([3; 32]),
            rk: [4; 32], cks: Some([5; 32]), ckr: Some([6; 32]),
            ns: 7, nr: 8, pn: 9, skipped: HashMap::from([(([10; 32], 11), [12; 32])]) }
    }

    #[test]
    fn sealed_ratchet_preserves_legacy_storage_bytes_entry_by_entry() {
        let original = Zeroizing::new(bincode::serialize(&legacy()).unwrap());
        let state = RatchetState::from_bytes(&original).unwrap();
        assert_eq!(&*state.dhs_sk.open().unwrap(), &[1; 32]);
        assert_eq!(&*state.rk.open().unwrap(), &[4; 32]);
        assert_eq!(&*state.cks.as_ref().unwrap().open().unwrap(), &[5; 32]);
        assert_eq!(&*state.skipped[&([10; 32], 11)].open().unwrap(), &[12; 32]);
        let saved = state.to_bytes().unwrap();
        assert_eq!(&*saved, &*original);
        let read_by_old_format: LegacyRatchet = bincode::deserialize(&saved).unwrap();
        assert_eq!(read_by_old_format.nr, 8);
        assert_eq!(read_by_old_format.skipped[&([10; 32], 11)], [12; 32]);
    }

    #[test]
    fn failed_encryption_does_not_advance_sealed_chain() {
        let mut original = legacy();
        original.ns = u32::MAX;
        let bytes = Zeroizing::new(bincode::serialize(&original).unwrap());
        let mut state = RatchetState::from_bytes(&bytes).unwrap();
        assert!(matches!(state.encrypt(b"message", b"ad"), Err(CryptoError::State)));
        let saved = state.to_bytes().unwrap();
        assert_eq!(&*saved, &*bytes);
    }

    #[test]
    fn excessive_gap_and_total_skipped_keys_fail_without_mutation() {
        let bytes = Zeroizing::new(bincode::serialize(&legacy()).unwrap());
        let mut state = RatchetState::from_bytes(&bytes).unwrap();
        // One key is already stored: even a gap of MAX_SKIP would overflow
        // the total bound, before doing any expensive chain advancement.
        let header = RatchetHeader { dh: [3; 32], pn: 0, n: state.nr + MAX_SKIP };
        assert!(matches!(state.decrypt(&header, b"invalid", b"ad"), Err(CryptoError::TooManySkipped)));
        assert_eq!(&*state.to_bytes().unwrap(), &*bytes);
        let mut oversized = legacy();
        oversized.skipped = (0..=MAX_SKIP).map(|n| (([10; 32], n), [12; 32])).collect();
        let bytes = Zeroizing::new(bincode::serialize(&oversized).unwrap());
        assert!(matches!(RatchetState::from_bytes(&bytes), Err(CryptoError::Codec)));
    }
}
