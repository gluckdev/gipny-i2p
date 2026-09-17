//! Everything the network stores is sealed before it leaves the owner, and
//! stored under a key only its readers can compute. A storing node sees
//! uniformly random bytes under a uniformly random key, rounded up to a size
//! bucket; it learns neither sender, recipient, kind of item, nor exact size.

use chacha20poly1305::{aead::Aead, aead::Payload, KeyInit, XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::rand_core::TryRng;
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

pub type DhtKey = [u8; 32];

pub const DAY_MS: u64 = 24 * 3600 * 1000;

/// Smallest and largest padded size; every sealed value is a power of two
/// in between, plus the fixed AEAD overhead.
const MIN_BUCKET: usize = 256;
pub const MAX_PLAINTEXT: usize = 4 * 1024 * 1024 - 64;

pub fn fill_random(dst: &mut [u8]) {
    rand::rngs::SysRng.try_fill_bytes(dst).expect("system RNG unavailable");
}

pub fn random_32() -> [u8; 32] {
    let mut b = [0u8; 32];
    fill_random(&mut b);
    b
}

pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// UTC day number: mailbox keys roll over daily so no key lives long enough
/// to become a watchable mailbox.
pub fn day_of(now_ms: u64) -> u32 {
    (now_ms / DAY_MS) as u32
}

fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8]) -> Zeroizing<[u8; 32]> {
    let hk = Hkdf::<Sha256>::new(Some(salt), ikm);
    let mut out = Zeroizing::new([0u8; 32]);
    hk.expand(info, &mut out[..]).expect("hkdf output fits");
    out
}

fn hmac(key: &[u8; 32], parts: &[&[u8]]) -> [u8; 32] {
    let mut m = <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("hmac takes any key");
    for p in parts {
        m.update(p);
    }
    m.finalize().into_bytes().into()
}

/// A secret only two identities can compute: X25519 of their static DH keys,
/// bound to both signing keys. Needs nothing from the session protocol, so it
/// exists from the moment each has the other's card.
///
/// `None` when the DH output is all zeros, i.e. a low-order public key.
pub fn pair_secret(
    my_dh_sk: &[u8; 32],
    their_dh_pk: &[u8; 32],
    my_sign_pk: &[u8; 32],
    their_sign_pk: &[u8; 32],
) -> Option<Zeroizing<[u8; 32]>> {
    let shared = StaticSecret::from(*my_dh_sk).diffie_hellman(&PublicKey::from(*their_dh_pk));
    if !shared.was_contributory() {
        return None;
    }
    let (lo, hi) = if my_sign_pk <= their_sign_pk { (my_sign_pk, their_sign_pk) } else { (their_sign_pk, my_sign_pk) };
    let mut ikm = Zeroizing::new(Vec::with_capacity(96));
    ikm.extend_from_slice(shared.as_bytes());
    ikm.extend_from_slice(lo);
    ikm.extend_from_slice(hi);
    Some(hkdf(b"gipny-dht-pair-v1", &ikm, b"pair"))
}

/// Derived from a public card. Not secret from anyone holding the card, but a
/// storing node without it cannot tell whose keys it is looking at.
pub fn card_secret(sign_pk: &[u8; 32], dh_pk: &[u8; 32]) -> [u8; 32] {
    *hkdf(b"gipny-dht-card-v1", &[&sign_pk[..], &dh_pk[..]].concat(), b"card")
}

/// Mail from one of a pair to the other, on a given day.
pub fn mail_key(pair: &[u8; 32], to_sign_pk: &[u8; 32], day: u32) -> DhtKey {
    hmac(pair, &[b"mail", to_sign_pk, &day.to_le_bytes()])
}

/// First contact, before there is a pair: anyone with the card can drop here.
pub fn intro_key(card: &[u8; 32], day: u32) -> DhtKey {
    hmac(card, &[b"intro", &day.to_le_bytes()])
}

/// Where `owner` collects now, readable by the other of the pair only.
pub fn addr_key(pair: &[u8; 32], owner_sign_pk: &[u8; 32]) -> DhtKey {
    hmac(pair, &[b"addr", owner_sign_pk])
}

/// The owner's prekey bundle, for opening a session while they are away.
pub fn bundle_key(card: &[u8; 32]) -> DhtKey {
    hmac(card, &[b"bundle"])
}

fn pad(plaintext: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if plaintext.len() > MAX_PLAINTEXT {
        return None;
    }
    let need = plaintext.len() + 4;
    let bucket = need.next_power_of_two().max(MIN_BUCKET);
    let mut out = Zeroizing::new(Vec::with_capacity(bucket));
    out.extend_from_slice(&(plaintext.len() as u32).to_le_bytes());
    out.extend_from_slice(plaintext);
    out.resize(bucket, 0);
    Some(out)
}

fn unpad(padded: &[u8]) -> Option<Vec<u8>> {
    let len = u32::from_le_bytes(padded.get(..4)?.try_into().ok()?) as usize;
    padded.get(4..4 + len).map(<[u8]>::to_vec)
}

const NONCE_LEN: usize = 24;

fn aead_seal(key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Option<Vec<u8>> {
    let padded = pad(plaintext)?;
    let mut nonce = [0u8; NONCE_LEN];
    fill_random(&mut nonce);
    let ct = XChaCha20Poly1305::new(key.into())
        .encrypt(&XNonce::from(nonce), Payload { msg: &padded, aad })
        .ok()?;
    Some([&nonce[..], &ct].concat())
}

fn aead_open(key: &[u8; 32], aad: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    if sealed.len() < NONCE_LEN {
        return None;
    }
    let (nonce, ct) = sealed.split_at(NONCE_LEN);
    let nonce: [u8; NONCE_LEN] = nonce.try_into().ok()?;
    let padded = Zeroizing::new(
        XChaCha20Poly1305::new(key.into())
            .decrypt(&XNonce::from(nonce), Payload { msg: ct, aad })
            .ok()?,
    );
    unpad(&padded)
}

/// Seal under a symmetric secret (pair or card). `label` separates the kinds
/// of item, so a value cannot be replayed under another meaning; `dht_key`
/// binds it to the one key it was stored under.
pub fn seal(secret: &[u8; 32], label: &[u8], dht_key: &DhtKey, plaintext: &[u8]) -> Option<Vec<u8>> {
    let key = hkdf(b"gipny-dht-seal-v1", secret, label);
    aead_seal(&key, dht_key, plaintext)
}

pub fn open(secret: &[u8; 32], label: &[u8], dht_key: &DhtKey, sealed: &[u8]) -> Option<Vec<u8>> {
    let key = hkdf(b"gipny-dht-seal-v1", secret, label);
    aead_open(&key, dht_key, sealed)
}

/// Seal to a recipient's public DH key with a fresh ephemeral key. For the
/// first letter, whose contents name the sender: only the recipient's secret
/// opens it.
pub fn seal_to(recipient_dh_pk: &[u8; 32], dht_key: &DhtKey, plaintext: &[u8]) -> Option<Vec<u8>> {
    let mut eph_bytes = Zeroizing::new([0u8; 32]);
    fill_random(&mut eph_bytes[..]);
    let eph = StaticSecret::from(*eph_bytes);
    let eph_pk = PublicKey::from(&eph).to_bytes();
    let shared = eph.diffie_hellman(&PublicKey::from(*recipient_dh_pk));
    if !shared.was_contributory() {
        return None;
    }
    let ikm = Zeroizing::new([shared.as_bytes(), &eph_pk[..], &recipient_dh_pk[..]].concat());
    let key = hkdf(b"gipny-dht-ecies-v1", &ikm, b"intro");
    Some([&eph_pk[..], &aead_seal(&key, dht_key, plaintext)?].concat())
}

pub fn open_from(my_dh_sk: &[u8; 32], dht_key: &DhtKey, sealed: &[u8]) -> Option<Vec<u8>> {
    let eph_pk: [u8; 32] = sealed.get(..32)?.try_into().ok()?;
    let my = StaticSecret::from(*my_dh_sk);
    let my_pk = PublicKey::from(&my).to_bytes();
    let shared = my.diffie_hellman(&PublicKey::from(eph_pk));
    if !shared.was_contributory() {
        return None;
    }
    let ikm = Zeroizing::new([shared.as_bytes(), &eph_pk[..], &my_pk[..]].concat());
    let key = hkdf(b"gipny-dht-ecies-v1", &ikm, b"intro");
    aead_open(&key, dht_key, &sealed[32..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dh_pair() -> ([u8; 32], [u8; 32]) {
        let sk = random_32();
        (sk, PublicKey::from(&StaticSecret::from(sk)).to_bytes())
    }

    #[test]
    fn both_sides_derive_the_same_pair_secret_and_nobody_else_does() {
        let (a_sk, a_pk) = dh_pair();
        let (b_sk, b_pk) = dh_pair();
        let (c_sk, _) = dh_pair();
        let (a_sign, b_sign) = ([1; 32], [2; 32]);
        let ab = pair_secret(&a_sk, &b_pk, &a_sign, &b_sign).unwrap();
        let ba = pair_secret(&b_sk, &a_pk, &b_sign, &a_sign).unwrap();
        assert_eq!(*ab, *ba);
        let cb = pair_secret(&c_sk, &b_pk, &a_sign, &b_sign).unwrap();
        assert_ne!(*ab, *cb);
        // Bound to the signing keys too.
        let other = pair_secret(&a_sk, &b_pk, &a_sign, &[3; 32]).unwrap();
        assert_ne!(*ab, *other);
    }

    #[test]
    fn a_low_order_key_yields_no_secret() {
        let (a_sk, _) = dh_pair();
        assert!(pair_secret(&a_sk, &[0; 32], &[1; 32], &[2; 32]).is_none());
        assert!(seal_to(&[0; 32], &[0; 32], b"x").is_none());
    }

    #[test]
    fn keys_are_separated_by_kind_direction_and_day() {
        let pair = [7; 32];
        let (a, b) = ([1; 32], [2; 32]);
        let keys = [
            mail_key(&pair, &a, 100),
            mail_key(&pair, &b, 100),
            mail_key(&pair, &a, 101),
            addr_key(&pair, &a),
            addr_key(&pair, &b),
            intro_key(&pair, 100),
            bundle_key(&pair),
        ];
        for i in 0..keys.len() {
            for j in i + 1..keys.len() {
                assert_ne!(keys[i], keys[j], "{i} vs {j}");
            }
        }
    }

    #[test]
    fn sealed_values_open_only_with_the_right_secret_label_and_key() {
        let (secret, key) = ([9; 32], [4; 32]);
        let sealed = seal(&secret, b"mail", &key, b"hello").unwrap();
        assert_eq!(open(&secret, b"mail", &key, &sealed).unwrap(), b"hello");
        assert!(open(&[8; 32], b"mail", &key, &sealed).is_none());
        assert!(open(&secret, b"addr", &key, &sealed).is_none());
        assert!(open(&secret, b"mail", &[5; 32], &sealed).is_none());
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(open(&secret, b"mail", &key, &tampered).is_none());
    }

    #[test]
    fn sizes_are_rounded_to_buckets() {
        let (secret, key) = ([9; 32], [4; 32]);
        let small = seal(&secret, b"mail", &key, b"a").unwrap();
        let bigger = seal(&secret, b"mail", &key, &[0u8; 200]).unwrap();
        assert_eq!(small.len(), bigger.len());
        let over = seal(&secret, b"mail", &key, &[0u8; 300]).unwrap();
        assert_eq!(over.len() - small.len(), 256);
        assert!(seal(&secret, b"mail", &key, &vec![0u8; MAX_PLAINTEXT + 1]).is_none());
    }

    #[test]
    fn a_letter_to_a_public_key_opens_only_with_its_secret() {
        let (sk, pk) = dh_pair();
        let (other_sk, _) = dh_pair();
        let key = [3; 32];
        let sealed = seal_to(&pk, &key, b"it is me").unwrap();
        assert_eq!(open_from(&sk, &key, &sealed).unwrap(), b"it is me");
        assert!(open_from(&other_sk, &key, &sealed).is_none());
        assert!(open_from(&sk, &[2; 32], &sealed).is_none());
        // Two seals of the same text share nothing visible.
        let again = seal_to(&pk, &key, b"it is me").unwrap();
        assert_ne!(sealed[..32], again[..32]);
    }
}
