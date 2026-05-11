use std::collections::HashMap;

use chacha20poly1305::{aead::Aead, aead::Payload, KeyInit, XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;
use sha2::{Digest, Sha256};
use thiserror::Error;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize;

pub type Result<T> = std::result::Result<T, CryptoError>;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("crypto")] Crypto,
    #[error("codec")] Codec,
    #[error("bad signature")] BadSignature,
    #[error("too many skipped")] TooManySkipped,
    #[error("mac")] Mac,
    #[error("state")] State,
}

impl From<bincode::Error> for CryptoError { fn from(_: bincode::Error) -> Self { Self::Codec } }

const MAX_SKIP: u32 = 100_000;
const X3DH_INFO: &[u8] = b"gipny/x3dh/v1";
const RK_INFO: &[u8] = b"gipny/dr/root/v1";
const MSG_INFO: &[u8] = b"gipny/dr/msg/v1";

#[derive(Clone)]
pub struct Identity {
    sign_seed: [u8; 32],
    dh_sk: [u8; 32],
}

impl Drop for Identity {
    fn drop(&mut self) {
        self.sign_seed.zeroize();
        self.dh_sk.zeroize();
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdentityCard {
    pub sign_pk: [u8; 32],
    pub dh_pk: [u8; 32],
}

impl Identity {
    pub fn generate() -> Self {
        let mut sign_seed = [0u8; 32];
        let mut dh_sk = [0u8; 32];
        OsRng.fill_bytes(&mut sign_seed);
        OsRng.fill_bytes(&mut dh_sk);
        Self { sign_seed, dh_sk }
    }
    pub fn from_bytes(sign_seed: [u8; 32], dh_sk: [u8; 32]) -> Self { Self { sign_seed, dh_sk } }
    pub fn sign_seed(&self) -> &[u8; 32] { &self.sign_seed }
    pub fn dh_secret(&self) -> &[u8; 32] { &self.dh_sk }
    fn signing(&self) -> SigningKey { SigningKey::from_bytes(&self.sign_seed) }
    fn static_secret(&self) -> StaticSecret { StaticSecret::from(self.dh_sk) }
    pub fn card(&self) -> IdentityCard {
        IdentityCard {
            sign_pk: self.signing().verifying_key().to_bytes(),
            dh_pk: PublicKey::from(&self.static_secret()).to_bytes(),
        }
    }
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] { self.signing().sign(msg).to_bytes() }
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
    secret: [u8; 32],
    public: [u8; 32],
}

impl Drop for PreKeyPair { fn drop(&mut self) { self.secret.zeroize(); } }

impl PreKeyPair {
    pub fn generate() -> Self {
        let mut secret = [0u8; 32];
        OsRng.fill_bytes(&mut secret);
        let public = PublicKey::from(&StaticSecret::from(secret)).to_bytes();
        Self { secret, public }
    }
    pub fn from_secret(secret: [u8; 32]) -> Self {
        let public = PublicKey::from(&StaticSecret::from(secret)).to_bytes();
        Self { secret, public }
    }
    pub fn secret(&self) -> &[u8; 32] { &self.secret }
    pub fn public(&self) -> &[u8; 32] { &self.public }
    fn static_secret(&self) -> StaticSecret { StaticSecret::from(self.secret) }
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
) -> [u8; 32] {
    let dh1 = my_ik_sk.diffie_hellman(their_spk);
    let dh2 = my_eph_sk.diffie_hellman(their_ik_dh);
    let dh3 = my_eph_sk.diffie_hellman(their_spk);
    let mut ikm = Vec::with_capacity(160);
    ikm.extend_from_slice(&[0xffu8; 32]);
    ikm.extend_from_slice(dh1.as_bytes());
    ikm.extend_from_slice(dh2.as_bytes());
    ikm.extend_from_slice(dh3.as_bytes());
    if let Some(opk) = their_opk {
        let dh4 = my_eph_sk.diffie_hellman(opk);
        ikm.extend_from_slice(dh4.as_bytes());
    }
    let hk = Hkdf::<Sha256>::new(Some(&[0u8; 32]), &ikm);
    let mut sk = [0u8; 32];
    hk.expand(X3DH_INFO, &mut sk).expect("hkdf");
    ikm.zeroize();
    sk
}

fn x3dh_ikm_responder(
    my_ik_sk: &StaticSecret,
    my_spk_sk: &StaticSecret,
    my_opk_sk: Option<&StaticSecret>,
    their_ik_dh: &PublicKey,
    their_eph: &PublicKey,
) -> [u8; 32] {
    let dh1 = my_spk_sk.diffie_hellman(their_ik_dh);
    let dh2 = my_ik_sk.diffie_hellman(their_eph);
    let dh3 = my_spk_sk.diffie_hellman(their_eph);
    let mut ikm = Vec::with_capacity(160);
    ikm.extend_from_slice(&[0xffu8; 32]);
    ikm.extend_from_slice(dh1.as_bytes());
    ikm.extend_from_slice(dh2.as_bytes());
    ikm.extend_from_slice(dh3.as_bytes());
    if let Some(opk) = my_opk_sk {
        let dh4 = opk.diffie_hellman(their_eph);
        ikm.extend_from_slice(dh4.as_bytes());
    }
    let hk = Hkdf::<Sha256>::new(Some(&[0u8; 32]), &ikm);
    let mut sk = [0u8; 32];
    hk.expand(X3DH_INFO, &mut sk).expect("hkdf");
    ikm.zeroize();
    sk
}

pub fn x3dh_initiate(
    mine: &Identity,
    their: &PreKeyBundle,
    plaintext: &[u8],
    ad: &[u8],
) -> Result<(RatchetState, X3dhInitial)> {
    their.verify()?;
    let mut eph_bytes = [0u8; 32];
    OsRng.fill_bytes(&mut eph_bytes);
    let eph_sk = StaticSecret::from(eph_bytes);
    let eph_pk = PublicKey::from(&eph_sk);
    let their_ik_dh = PublicKey::from(their.identity.dh_pk);
    let their_spk = PublicKey::from(their.signed_prekey);
    let their_opk = their.one_time_prekey.map(PublicKey::from);
    let sk = x3dh_ikm_initiator(
        &mine.static_secret(), &eph_sk,
        &their_ik_dh, &their_spk, their_opk.as_ref(),
    );
    let mut state = RatchetState::new_alice(&sk, &their.signed_prekey);
    let (header, ct) = state.encrypt(plaintext, ad)?;
    eph_bytes.zeroize();
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
    let my_opk_sk = my_opk.map(|p| p.static_secret());
    let sk = x3dh_ikm_responder(
        &mine.static_secret(),
        &my_spk.static_secret(),
        my_opk_sk.as_ref(),
        &their_ik_dh, &their_eph,
    );
    let mut state = RatchetState::new_bob(&sk, my_spk);
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
    dhs_sk: [u8; 32],
    dhs_pk: [u8; 32],
    dhr: Option<[u8; 32]>,
    rk: [u8; 32],
    cks: Option<[u8; 32]>,
    ckr: Option<[u8; 32]>,
    ns: u32,
    nr: u32,
    pn: u32,
    skipped: HashMap<([u8; 32], u32), [u8; 32]>,
}

impl Drop for RatchetState {
    fn drop(&mut self) {
        self.dhs_sk.zeroize();
        self.rk.zeroize();
        if let Some(v) = self.cks.as_mut() { v.zeroize(); }
        if let Some(v) = self.ckr.as_mut() { v.zeroize(); }
        for (_, v) in self.skipped.iter_mut() { v.zeroize(); }
    }
}

impl Clone for RatchetState {
    fn clone(&self) -> Self {
        Self {
            dhs_sk: self.dhs_sk,
            dhs_pk: self.dhs_pk,
            dhr: self.dhr,
            rk: self.rk,
            cks: self.cks,
            ckr: self.ckr,
            ns: self.ns,
            nr: self.nr,
            pn: self.pn,
            skipped: self.skipped.clone(),
        }
    }
}

impl RatchetState {
    pub fn new_alice(sk: &[u8; 32], their_dh_pk: &[u8; 32]) -> Self {
        let mut dhs_sk = [0u8; 32];
        OsRng.fill_bytes(&mut dhs_sk);
        let dhs_pk = PublicKey::from(&StaticSecret::from(dhs_sk)).to_bytes();
        let dh = StaticSecret::from(dhs_sk).diffie_hellman(&PublicKey::from(*their_dh_pk));
        let (rk, cks) = kdf_rk(sk, dh.as_bytes());
        Self {
            dhs_sk, dhs_pk, dhr: Some(*their_dh_pk),
            rk, cks: Some(cks), ckr: None,
            ns: 0, nr: 0, pn: 0,
            skipped: HashMap::new(),
        }
    }

    pub fn new_bob(sk: &[u8; 32], spk: &PreKeyPair) -> Self {
        Self {
            dhs_sk: *spk.secret(),
            dhs_pk: *spk.public(),
            dhr: None,
            rk: *sk,
            cks: None, ckr: None,
            ns: 0, nr: 0, pn: 0,
            skipped: HashMap::new(),
        }
    }

    pub fn encrypt(&mut self, plaintext: &[u8], ad: &[u8]) -> Result<(RatchetHeader, Vec<u8>)> {
        let cks = self.cks.as_mut().ok_or(CryptoError::State)?;
        let (next, mk) = kdf_ck(cks);
        *cks = next;
        let header = RatchetHeader { dh: self.dhs_pk, pn: self.pn, n: self.ns };
        self.ns += 1;
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
            return aead_decrypt(&mk, ad, header, ct);
        }
        if Some(header.dh) != self.dhr {
            self.skip_message_keys(header.pn)?;
            self.dh_ratchet(header);
        }
        self.skip_message_keys(header.n)?;
        let ckr = self.ckr.as_mut().ok_or(CryptoError::State)?;
        let (next, mk) = kdf_ck(ckr);
        *ckr = next;
        self.nr += 1;
        aead_decrypt(&mk, ad, header, ct)
    }

    fn skip_message_keys(&mut self, until: u32) -> Result<()> {
        if self.nr + MAX_SKIP < until { return Err(CryptoError::TooManySkipped); }
        if let Some(ckr) = self.ckr.as_mut() {
            let dhr = self.dhr.ok_or(CryptoError::State)?;
            while self.nr < until {
                let (next, mk) = kdf_ck(ckr);
                *ckr = next;
                self.skipped.insert((dhr, self.nr), mk);
                self.nr += 1;
            }
        }
        Ok(())
    }

    fn dh_ratchet(&mut self, header: &RatchetHeader) {
        self.pn = self.ns;
        self.ns = 0;
        self.nr = 0;
        self.dhr = Some(header.dh);
        let their_dh = PublicKey::from(header.dh);
        let dh_in = StaticSecret::from(self.dhs_sk).diffie_hellman(&their_dh);
        let (rk, ckr) = kdf_rk(&self.rk, dh_in.as_bytes());
        self.rk = rk;
        self.ckr = Some(ckr);
        let mut new_sk = [0u8; 32];
        OsRng.fill_bytes(&mut new_sk);
        self.dhs_sk.zeroize();
        self.dhs_sk = new_sk;
        self.dhs_pk = PublicKey::from(&StaticSecret::from(self.dhs_sk)).to_bytes();
        let dh_out = StaticSecret::from(self.dhs_sk).diffie_hellman(&their_dh);
        let (rk2, cks) = kdf_rk(&self.rk, dh_out.as_bytes());
        self.rk = rk2;
        self.cks = Some(cks);
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> { bincode::serialize(self).map_err(Into::into) }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> { bincode::deserialize(bytes).map_err(Into::into) }
}

fn kdf_rk(rk: &[u8; 32], dh_out: &[u8]) -> ([u8; 32], [u8; 32]) {
    let hk = Hkdf::<Sha256>::new(Some(rk), dh_out);
    let mut out = [0u8; 64];
    hk.expand(RK_INFO, &mut out).expect("hkdf");
    let mut new_rk = [0u8; 32]; new_rk.copy_from_slice(&out[..32]);
    let mut ck = [0u8; 32]; ck.copy_from_slice(&out[32..]);
    out.zeroize();
    (new_rk, ck)
}

fn kdf_ck(ck: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let mut m1 = <Hmac<Sha256> as Mac>::new_from_slice(ck).expect("hmac");
    m1.update(&[0x02]);
    let next: [u8; 32] = m1.finalize().into_bytes().into();
    let mut m2 = <Hmac<Sha256> as Mac>::new_from_slice(ck).expect("hmac");
    m2.update(&[0x01]);
    let mk: [u8; 32] = m2.finalize().into_bytes().into();
    (next, mk)
}

fn aead_from_mk(mk: &[u8; 32]) -> ([u8; 32], [u8; 24]) {
    let hk = Hkdf::<Sha256>::new(Some(&[0u8; 32]), mk);
    let mut out = [0u8; 56];
    hk.expand(MSG_INFO, &mut out).expect("hkdf");
    let mut key = [0u8; 32]; key.copy_from_slice(&out[..32]);
    let mut nonce = [0u8; 24]; nonce.copy_from_slice(&out[32..]);
    out.zeroize();
    (key, nonce)
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
    let cipher = XChaCha20Poly1305::new((&key).into());
    let aad = build_aad(ad, header);
    cipher.encrypt(XNonce::from_slice(&nonce), Payload { msg: plaintext, aad: &aad })
        .map_err(|_| CryptoError::Crypto)
}

fn aead_decrypt(mk: &[u8; 32], ad: &[u8], header: &RatchetHeader, ct: &[u8]) -> Result<Vec<u8>> {
    let (key, nonce) = aead_from_mk(mk);
    let cipher = XChaCha20Poly1305::new((&key).into());
    let aad = build_aad(ad, header);
    cipher.decrypt(XNonce::from_slice(&nonce), Payload { msg: ct, aad: &aad })
        .map_err(|_| CryptoError::Mac)
}

pub struct AttachmentCipher { key: [u8; 32] }

impl Drop for AttachmentCipher { fn drop(&mut self) { self.key.zeroize(); } }

impl AttachmentCipher {
    pub fn generate() -> Self {
        let mut k = [0u8; 32];
        OsRng.fill_bytes(&mut k);
        Self { key: k }
    }
    pub fn from_key(key: [u8; 32]) -> Self { Self { key } }
    pub fn key(&self) -> &[u8; 32] { &self.key }

    fn nonce(idx: u64) -> [u8; 24] {
        let mut n = [0u8; 24];
        n[..8].copy_from_slice(&idx.to_le_bytes());
        n
    }

    pub fn encrypt_chunk(&self, idx: u64, aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
        let cipher = XChaCha20Poly1305::new((&self.key).into());
        cipher.encrypt(XNonce::from_slice(&Self::nonce(idx)), Payload { msg: plaintext, aad })
            .map_err(|_| CryptoError::Crypto)
    }

    pub fn decrypt_chunk(&self, idx: u64, aad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
        let cipher = XChaCha20Poly1305::new((&self.key).into());
        cipher.decrypt(XNonce::from_slice(&Self::nonce(idx)), Payload { msg: ciphertext, aad })
            .map_err(|_| CryptoError::Mac)
    }
}