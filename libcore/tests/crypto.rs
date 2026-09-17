//! Crypto-core tests: X3DH, Double Ratchet, and attachment chunk encryption.
//!
//! This is the layer the product's confidentiality claim rests on, and it had no
//! tests at all. The cases below are the ones where a silent regression would be
//! invisible from the app: a ratchet that still round-trips but stops advancing
//! keys, a decrypt that ignores the associated data it is supposed to bind, or a
//! chunk cipher that does not bind the chunk index and lets chunks be reordered.

use gipny_libcore::crypto::{
    x3dh_initiate, x3dh_respond, AttachmentCipher, Identity, PreKeyBundle, PreKeyPair,
    RatchetState,
};

const AD: &[u8] = b"associated-data";

/// Alice and Bob complete X3DH; returns both live ratchet states.
fn establish() -> (RatchetState, RatchetState) {
    let alice = Identity::generate();
    let bob = Identity::generate();
    let bob_spk = PreKeyPair::generate();
    let bob_opk = PreKeyPair::generate();
    let bundle = PreKeyBundle::new(&bob, &bob_spk, Some((7, &bob_opk)));

    let (a_state, initial) = x3dh_initiate(&alice, &bundle, b"hello bob", AD).unwrap();
    let (b_state, pt) = x3dh_respond(&bob, &bob_spk, Some(&bob_opk), &initial, AD).unwrap();
    assert_eq!(pt, b"hello bob");
    (a_state, b_state)
}

#[test]
fn identity_signs_and_verifies() {
    let id = Identity::generate();
    let sig = id.sign(b"message");
    assert!(id.card().verify(b"message", &sig));
    assert!(!id.card().verify(b"other message", &sig));

    let mut tampered = sig;
    tampered[0] ^= 1;
    assert!(!id.card().verify(b"message", &tampered));
}

#[test]
fn identity_survives_a_round_trip_through_bytes() {
    let id = Identity::generate();
    let restored = Identity::from_bytes(*id.sign_seed(), *id.dh_secret());
    assert_eq!(id.card(), restored.card());
    assert_eq!(id.fingerprint(), restored.fingerprint());

    // Distinct identities must not collide on the fingerprint shown to users for
    // out-of-band verification.
    assert_ne!(id.fingerprint(), Identity::generate().fingerprint());
}

#[test]
fn prekey_bundle_rejects_a_swapped_prekey() {
    let bob = Identity::generate();
    let spk = PreKeyPair::generate();
    let mut bundle = PreKeyBundle::new(&bob, &spk, None);
    assert!(bundle.verify().is_ok());

    // Substituting someone else's prekey is exactly the attack the signature is
    // there to stop.
    bundle.signed_prekey = *PreKeyPair::generate().public();
    assert!(bundle.verify().is_err());
}

#[test]
fn x3dh_works_without_a_one_time_prekey() {
    let alice = Identity::generate();
    let bob = Identity::generate();
    let bob_spk = PreKeyPair::generate();
    let bundle = PreKeyBundle::new(&bob, &bob_spk, None);

    let (_, initial) = x3dh_initiate(&alice, &bundle, b"no opk", AD).unwrap();
    let (_, pt) = x3dh_respond(&bob, &bob_spk, None, &initial, AD).unwrap();
    assert_eq!(pt, b"no opk");
}

#[test]
fn x3dh_responder_needs_the_matching_one_time_prekey() {
    let alice = Identity::generate();
    let bob = Identity::generate();
    let bob_spk = PreKeyPair::generate();
    let bob_opk = PreKeyPair::generate();
    let bundle = PreKeyBundle::new(&bob, &bob_spk, Some((1, &bob_opk)));

    let (_, initial) = x3dh_initiate(&alice, &bundle, b"hi", AD).unwrap();

    // Answering with the wrong one-time prekey derives a different secret.
    let wrong = PreKeyPair::generate();
    assert!(x3dh_respond(&bob, &bob_spk, Some(&wrong), &initial, AD).is_err());
    // ...and so does forgetting it entirely.
    assert!(x3dh_respond(&bob, &bob_spk, None, &initial, AD).is_err());
}

#[test]
fn x3dh_binds_the_associated_data() {
    let alice = Identity::generate();
    let bob = Identity::generate();
    let bob_spk = PreKeyPair::generate();
    let bundle = PreKeyBundle::new(&bob, &bob_spk, None);

    let (_, initial) = x3dh_initiate(&alice, &bundle, b"bound", AD).unwrap();
    assert!(x3dh_respond(&bob, &bob_spk, None, &initial, b"different ad").is_err());
}

#[test]
fn ratchet_carries_a_conversation_in_both_directions() {
    let (mut a, mut b) = establish();

    for i in 0..5u8 {
        let msg = format!("a->b {i}");
        let (h, ct) = a.encrypt(msg.as_bytes(), AD).unwrap();
        assert_eq!(b.decrypt(&h, &ct, AD).unwrap(), msg.as_bytes());

        let reply = format!("b->a {i}");
        let (h, ct) = b.encrypt(reply.as_bytes(), AD).unwrap();
        assert_eq!(a.decrypt(&h, &ct, AD).unwrap(), reply.as_bytes());
    }
}

#[test]
fn ratchet_keys_actually_advance() {
    let (mut a, mut b) = establish();

    // Same plaintext twice must not produce the same ciphertext — if it does,
    // the chain key is not moving and forward secrecy is gone, while every
    // round-trip test above would still pass.
    let (h1, ct1) = a.encrypt(b"same", AD).unwrap();
    let (h2, ct2) = a.encrypt(b"same", AD).unwrap();
    assert_ne!(ct1, ct2);
    assert_ne!(h1.n, h2.n);

    assert_eq!(b.decrypt(&h1, &ct1, AD).unwrap(), b"same");
    assert_eq!(b.decrypt(&h2, &ct2, AD).unwrap(), b"same");
}

#[test]
fn ratchet_handles_messages_arriving_out_of_order() {
    let (mut a, mut b) = establish();

    let (h1, c1) = a.encrypt(b"first", AD).unwrap();
    let (h2, c2) = a.encrypt(b"second", AD).unwrap();
    let (h3, c3) = a.encrypt(b"third", AD).unwrap();

    // Relay delivery is not ordered; skipped message keys have to be retained.
    assert_eq!(b.decrypt(&h3, &c3, AD).unwrap(), b"third");
    assert_eq!(b.decrypt(&h1, &c1, AD).unwrap(), b"first");
    assert_eq!(b.decrypt(&h2, &c2, AD).unwrap(), b"second");
}

#[test]
fn ratchet_rejects_tampering_and_replay() {
    let (mut a, mut b) = establish();

    let (h, ct) = a.encrypt(b"authentic", AD).unwrap();

    let mut flipped = ct.clone();
    flipped[0] ^= 1;
    assert!(b.decrypt(&h, &flipped, AD).is_err());

    // Wrong associated data must fail even with an untouched ciphertext.
    assert!(b.decrypt(&h, &ct, b"wrong ad").is_err());

    // The genuine message still opens after those failures.
    assert_eq!(b.decrypt(&h, &ct, AD).unwrap(), b"authentic");

    // Replaying it must not: that message key is consumed.
    assert!(b.decrypt(&h, &ct, AD).is_err());
}

#[test]
fn ratchet_state_survives_being_persisted() {
    let (mut a, mut b) = establish();

    let (h, ct) = a.encrypt(b"before save", AD).unwrap();
    assert_eq!(b.decrypt(&h, &ct, AD).unwrap(), b"before save");

    // The app stores this between launches; a lossy round-trip would break
    // conversations only after a restart.
    let saved = b.to_bytes().unwrap();
    let mut restored = RatchetState::from_bytes(&saved).unwrap();

    let (h, ct) = a.encrypt(b"after save", AD).unwrap();
    assert_eq!(restored.decrypt(&h, &ct, AD).unwrap(), b"after save");

    let (h, ct) = restored.encrypt(b"reply", AD).unwrap();
    assert_eq!(a.decrypt(&h, &ct, AD).unwrap(), b"reply");
}

#[test]
fn attachment_chunks_are_bound_to_their_index() {
    let cipher = AttachmentCipher::generate();
    let aad = b"attachment-42";

    let c0 = cipher.encrypt_chunk(0, aad, b"chunk zero").unwrap();
    let c1 = cipher.encrypt_chunk(1, aad, b"chunk one").unwrap();

    assert_eq!(cipher.decrypt_chunk(0, aad, &c0).unwrap(), b"chunk zero");
    assert_eq!(cipher.decrypt_chunk(1, aad, &c1).unwrap(), b"chunk one");

    // Reordering or duplicating chunks must not go unnoticed.
    assert!(cipher.decrypt_chunk(1, aad, &c0).is_err());
    assert!(cipher.decrypt_chunk(0, aad, &c1).is_err());
    // Nor must moving a chunk to a different attachment.
    assert!(cipher.decrypt_chunk(0, b"attachment-43", &c0).is_err());
}

#[test]
fn attachment_key_round_trips_and_is_unique() {
    let cipher = AttachmentCipher::generate();
    let ct = cipher.encrypt_chunk(0, b"aad", b"payload").unwrap();

    let restored = AttachmentCipher::from_key(*cipher.key());
    assert_eq!(restored.decrypt_chunk(0, b"aad", &ct).unwrap(), b"payload");

    let other = AttachmentCipher::generate();
    assert!(other.decrypt_chunk(0, b"aad", &ct).is_err());
}

#[test]
fn random_arrays_are_random() {
    use gipny_libcore::crypto::random_array;
    // Not a statistical test — a guard against a constant or a repeated value,
    // which is how an RNG wired wrong shows up.
    let a: [u8; 32] = random_array();
    let b: [u8; 32] = random_array();
    assert_ne!(a, b);
    assert_ne!(a, [0u8; 32]);
    assert!(a.iter().collect::<std::collections::HashSet<_>>().len() > 8, "{a:?}");
    let n: [u8; 24] = random_array();
    assert_ne!(n, [0u8; 24]);
}
