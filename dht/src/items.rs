//! What gipny puts into the network, built and opened on the owners' devices.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;

use crate::crypto::{self, day_of, DhtKey, DAY_MS};

/// Mail and introductions wait this long for their recipient.
pub const MAIL_TTL_MS: u64 = 7 * DAY_MS;
/// Address records are republished while their owner runs; a stale one dies
/// quickly on its own.
pub const ADDRESS_TTL_MS: u64 = 2 * 3600 * 1000;
pub const BUNDLE_TTL_MS: u64 = 7 * DAY_MS;

/// An item ready to store: where, what, and (for mail) the hash of the token
/// that allows deleting it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedItem {
    pub key: DhtKey,
    pub value: Vec<u8>,
    pub delete_hash: Option<[u8; 32]>,
    pub expires_at_ms: u64,
}

/// Opened mail: the envelope for the session layer, and the token that
/// proves to storing nodes we may delete it.
#[derive(Debug, PartialEq, Eq)]
pub struct OpenedMail {
    pub envelope: Vec<u8>,
    pub delete_token: [u8; 32],
}

#[derive(Serialize, Deserialize)]
struct MailPlain {
    delete_token: [u8; 32],
    envelope: Vec<u8>,
}

fn mail_plain(envelope: &[u8]) -> ([u8; 32], Vec<u8>) {
    let token = crypto::random_32();
    let plain = bincode::serialize(&MailPlain { delete_token: token, envelope: envelope.to_vec() })
        .expect("serialize to memory");
    (token, plain)
}

fn open_plain(plain: &[u8]) -> Option<OpenedMail> {
    let m: MailPlain = bincode::deserialize(plain).ok()?;
    Some(OpenedMail { envelope: m.envelope, delete_token: m.delete_token })
}

pub fn delete_hash(token: &[u8; 32]) -> [u8; 32] {
    crypto::sha256(&[b"gipny-dht-delete-v1", token])
}

/// Mail to a contact, under today's key for that direction of the pair.
pub fn mail(pair: &[u8; 32], to_sign_pk: &[u8; 32], now_ms: u64, envelope: &[u8]) -> Option<PreparedItem> {
    let key = crypto::mail_key(pair, to_sign_pk, day_of(now_ms));
    let (token, plain) = mail_plain(envelope);
    Some(PreparedItem {
        key,
        value: crypto::seal(pair, b"mail", &key, &plain)?,
        delete_hash: Some(delete_hash(&token)),
        expires_at_ms: now_ms + MAIL_TTL_MS,
    })
}

pub fn open_mail(pair: &[u8; 32], key: &DhtKey, value: &[u8]) -> Option<OpenedMail> {
    open_plain(&crypto::open(pair, b"mail", key, value)?)
}

/// A first letter to someone whose card is all we have. The envelope names
/// us, so it is sealed to their public key rather than to anything a card
/// holder could compute.
pub fn intro(recipient_sign_pk: &[u8; 32], recipient_dh_pk: &[u8; 32], now_ms: u64, envelope: &[u8]) -> Option<PreparedItem> {
    let card = crypto::card_secret(recipient_sign_pk, recipient_dh_pk);
    let key = crypto::intro_key(&card, day_of(now_ms));
    let (token, plain) = mail_plain(envelope);
    Some(PreparedItem {
        key,
        value: crypto::seal_to(recipient_dh_pk, &key, &plain)?,
        delete_hash: Some(delete_hash(&token)),
        expires_at_ms: now_ms + MAIL_TTL_MS,
    })
}

pub fn open_intro(my_dh_sk: &[u8; 32], key: &DhtKey, value: &[u8]) -> Option<OpenedMail> {
    open_plain(&crypto::open_from(my_dh_sk, key, value)?)
}

/// The keys a recipient checks for mail from one contact over the last
/// `days` days, newest first.
pub fn mail_keys_to_poll(pair: &[u8; 32], my_sign_pk: &[u8; 32], now_ms: u64, days: u32) -> Vec<DhtKey> {
    let today = day_of(now_ms);
    (0..days).filter_map(|d| today.checked_sub(d)).map(|day| crypto::mail_key(pair, my_sign_pk, day)).collect()
}

pub fn intro_keys_to_poll(my_sign_pk: &[u8; 32], my_dh_pk: &[u8; 32], now_ms: u64, days: u32) -> Vec<DhtKey> {
    let card = crypto::card_secret(my_sign_pk, my_dh_pk);
    let today = day_of(now_ms);
    (0..days).filter_map(|d| today.checked_sub(d)).map(|day| crypto::intro_key(&card, day)).collect()
}

const KIND_ADDRESS: u8 = 1;
const KIND_BUNDLE: u8 = 2;

#[derive(Serialize, Deserialize)]
struct SignedRecord {
    kind: u8,
    owner: [u8; 32],
    /// The issuing time; a reader takes the newest valid record.
    seq: u64,
    expires_at_ms: u64,
    body: Vec<u8>,
    #[serde(with = "BigArray")]
    sig: [u8; 64],
}

fn record_digest(kind: u8, owner: &[u8; 32], seq: u64, expires_at_ms: u64, body: &[u8], key: &DhtKey) -> [u8; 32] {
    crypto::sha256(&[
        b"gipny-dht-record-v1",
        &[kind],
        owner,
        &seq.to_le_bytes(),
        &expires_at_ms.to_le_bytes(),
        &crypto::sha256(&[body]),
        key,
    ])
}

fn build_record(
    signing: &SigningKey, secret: &[u8; 32], label: &[u8], key: DhtKey,
    kind: u8, now_ms: u64, ttl_ms: u64, body: Vec<u8>,
) -> Option<PreparedItem> {
    let owner = signing.verifying_key().to_bytes();
    let expires_at_ms = now_ms + ttl_ms;
    let sig = signing.sign(&record_digest(kind, &owner, now_ms, expires_at_ms, &body, &key)).to_bytes();
    let plain = bincode::serialize(&SignedRecord { kind, owner, seq: now_ms, expires_at_ms, body, sig }).ok()?;
    Some(PreparedItem { key, value: crypto::seal(secret, label, &key, &plain)?, delete_hash: None, expires_at_ms })
}

/// Of all values found under a record's key, the newest one that opens, is
/// signed by `owner`, is of the right kind, and has not expired.
fn best_record(
    secret: &[u8; 32], label: &[u8], key: &DhtKey, kind: u8, owner: &[u8; 32],
    values: &[Vec<u8>], now_ms: u64,
) -> Option<(u64, Vec<u8>)> {
    let vk = VerifyingKey::from_bytes(owner).ok()?;
    values.iter()
        .filter_map(|v| crypto::open(secret, label, key, v))
        .filter_map(|plain| bincode::deserialize::<SignedRecord>(&plain).ok())
        .filter(|r| r.kind == kind && &r.owner == owner && r.expires_at_ms > now_ms)
        .filter(|r| {
            let digest = record_digest(r.kind, &r.owner, r.seq, r.expires_at_ms, &r.body, key);
            vk.verify(&digest, &Signature::from_bytes(&r.sig)).is_ok()
        })
        .max_by_key(|r| r.seq)
        .map(|r| (r.seq, r.body))
}

/// Where we collect now, for one contact to read.
pub fn address_record(signing: &SigningKey, pair: &[u8; 32], now_ms: u64, relay: &str) -> Option<PreparedItem> {
    let owner = signing.verifying_key().to_bytes();
    let key = crypto::addr_key(pair, &owner);
    build_record(signing, pair, b"addr", key, KIND_ADDRESS, now_ms, ADDRESS_TTL_MS, relay.as_bytes().to_vec())
}

/// The contact's newest address and when they published it.
pub fn open_address_record(pair: &[u8; 32], owner_sign_pk: &[u8; 32], values: &[Vec<u8>], now_ms: u64) -> Option<(String, u64)> {
    let key = crypto::addr_key(pair, owner_sign_pk);
    let (seq, body) = best_record(pair, b"addr", &key, KIND_ADDRESS, owner_sign_pk, values, now_ms)?;
    Some((String::from_utf8(body).ok()?, seq))
}

pub fn address_key(pair: &[u8; 32], owner_sign_pk: &[u8; 32]) -> DhtKey {
    crypto::addr_key(pair, owner_sign_pk)
}

/// Our prekey bundle, for someone with our card to start a session while we
/// are away.
pub fn bundle_record(signing: &SigningKey, my_dh_pk: &[u8; 32], now_ms: u64, bundle: &[u8]) -> Option<PreparedItem> {
    let owner = signing.verifying_key().to_bytes();
    let card = crypto::card_secret(&owner, my_dh_pk);
    let key = crypto::bundle_key(&card);
    build_record(signing, &card, b"bundle", key, KIND_BUNDLE, now_ms, BUNDLE_TTL_MS, bundle.to_vec())
}

pub fn bundle_key(owner_sign_pk: &[u8; 32], owner_dh_pk: &[u8; 32]) -> DhtKey {
    crypto::bundle_key(&crypto::card_secret(owner_sign_pk, owner_dh_pk))
}

pub fn open_bundle_record(owner_sign_pk: &[u8; 32], owner_dh_pk: &[u8; 32], values: &[Vec<u8>], now_ms: u64) -> Option<Vec<u8>> {
    let card = crypto::card_secret(owner_sign_pk, owner_dh_pk);
    let key = crypto::bundle_key(&card);
    best_record(&card, b"bundle", &key, KIND_BUNDLE, owner_sign_pk, values, now_ms).map(|(_, body)| body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use x25519_dalek::{PublicKey, StaticSecret};

    struct Who {
        signing: SigningKey,
        dh_sk: [u8; 32],
    }

    impl Who {
        fn new() -> Self {
            Self { signing: SigningKey::from_bytes(&crypto::random_32()), dh_sk: crypto::random_32() }
        }
        fn sign_pk(&self) -> [u8; 32] { self.signing.verifying_key().to_bytes() }
        fn dh_pk(&self) -> [u8; 32] { PublicKey::from(&StaticSecret::from(self.dh_sk)).to_bytes() }
        fn pair_with(&self, other: &Who) -> [u8; 32] {
            *crypto::pair_secret(&self.dh_sk, &other.dh_pk(), &self.sign_pk(), &other.sign_pk()).unwrap()
        }
    }

    const NOW: u64 = 20_000 * DAY_MS + 5_000;

    #[test]
    fn mail_reaches_the_other_of_the_pair_under_a_key_they_poll() {
        let (alice, bob) = (Who::new(), Who::new());
        let item = mail(&alice.pair_with(&bob), &bob.sign_pk(), NOW, b"envelope").unwrap();
        let bob_pair = bob.pair_with(&alice);
        assert!(mail_keys_to_poll(&bob_pair, &bob.sign_pk(), NOW + DAY_MS, 2).contains(&item.key));
        let opened = open_mail(&bob_pair, &item.key, &item.value).unwrap();
        assert_eq!(opened.envelope, b"envelope");
        assert_eq!(Some(delete_hash(&opened.delete_token)), item.delete_hash);
        // Alice's own mailbox for this pair is a different key.
        assert!(!mail_keys_to_poll(&bob_pair, &alice.sign_pk(), NOW, 7).contains(&item.key));
        let eve = Who::new();
        assert!(open_mail(&eve.pair_with(&bob), &item.key, &item.value).is_none());
    }

    #[test]
    fn an_introduction_opens_only_for_its_recipient() {
        let (alice, bob, eve) = (Who::new(), Who::new(), Who::new());
        let item = intro(&bob.sign_pk(), &bob.dh_pk(), NOW, b"hello, it is alice").unwrap();
        assert!(intro_keys_to_poll(&bob.sign_pk(), &bob.dh_pk(), NOW, 1).contains(&item.key));
        assert_eq!(open_intro(&bob.dh_sk, &item.key, &item.value).unwrap().envelope, b"hello, it is alice");
        assert!(open_intro(&eve.dh_sk, &item.key, &item.value).is_none());
        let _ = alice;
    }

    #[test]
    fn the_newest_signed_address_wins_and_forgeries_are_ignored() {
        let (alice, bob, eve) = (Who::new(), Who::new(), Who::new());
        let pair = alice.pair_with(&bob);
        let old = address_record(&alice.signing, &pair, NOW, "old-relay").unwrap();
        let new = address_record(&alice.signing, &pair, NOW + 1000, "new-relay").unwrap();
        assert_eq!(old.key, new.key);
        // Eve knows the pair secret in this test but not Alice's signing key.
        let forged = build_record(&eve.signing, &pair, b"addr", new.key, KIND_ADDRESS, NOW + 5000, ADDRESS_TTL_MS, b"evil".to_vec()).unwrap();
        let values = vec![forged.value, new.value.clone(), old.value];
        let bob_pair = bob.pair_with(&alice);
        let (relay, seq) = open_address_record(&bob_pair, &alice.sign_pk(), &values, NOW + 2000).unwrap();
        assert_eq!((relay.as_str(), seq), ("new-relay", NOW + 1000));
        // Expired records are not used.
        assert!(open_address_record(&bob_pair, &alice.sign_pk(), &[new.value], NOW + 1000 + ADDRESS_TTL_MS).is_none());
    }

    #[test]
    fn a_bundle_record_is_found_from_the_card_alone() {
        let alice = Who::new();
        let item = bundle_record(&alice.signing, &alice.dh_pk(), NOW, b"prekeys").unwrap();
        assert_eq!(item.key, bundle_key(&alice.sign_pk(), &alice.dh_pk()));
        assert_eq!(open_bundle_record(&alice.sign_pk(), &alice.dh_pk(), &[item.value], NOW).unwrap(), b"prekeys");
    }
}
