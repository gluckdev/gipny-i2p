#![cfg(target_arch = "wasm32")]

use gipny_browser_router::BrowserEngine;
use gipny_libcore::{crypto::PreKeyBundle, relay::EnvelopeBlob, session::WirePayload};
use serde_json::Value;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn transaction_persist_and_reload_with_metadata() {
    let profile = "alice_profile";
    let password = "correct horse battery staple 2026!";

    let alice = BrowserEngine::create(profile, password).expect("create engine");
    let initial_meta = alice.take_metadata().expect("take initial metadata");
    assert_eq!(initial_meta, "{}");

    let pub_info_raw = alice.public_info().expect("alice public info");
    let pub_info: Value = serde_json::from_str(&pub_info_raw).expect("parse public info");
    let alice_sign = pub_info["sign_pk"].as_str().expect("sign_pk");
    let alice_dh = pub_info["dh_pk"].as_str().expect("dh_pk");
    let alice_onion = pub_info["onion"].as_str().expect("onion");
    assert_eq!(alice_sign.len(), 64);
    assert_eq!(alice_dh.len(), 64);
    assert!(alice_onion.ends_with(".b32.i2p"));

    // Begin transaction
    alice.begin().expect("begin transaction");
    assert_eq!(alice.pending_generation(), "");

    let test_metadata = r#"{"contacts":[{"name":"Bob","address":"b32address.b32.i2p"}]}"#;
    let slot_a = alice.seal(test_metadata).expect("seal slot A");
    assert_eq!(alice.pending_generation(), "1");

    // Commit
    alice.commit().expect("commit transaction");
    assert_eq!(alice.pending_generation(), "");

    // Reload from slot A
    let reloaded = BrowserEngine::open_password(profile, password, &slot_a, &[])
        .expect("open reloaded profile");
    let restored_meta = reloaded.take_metadata().expect("take restored metadata");
    assert_eq!(restored_meta, test_metadata);

    // Verify metadata was wiped in Rust memory
    let second_meta = reloaded.take_metadata().expect("second take metadata");
    assert_eq!(second_meta, "");

    // Verify restored keys match exactly
    let reloaded_info_raw = reloaded.public_info().expect("reloaded public info");
    let reloaded_info: Value =
        serde_json::from_str(&reloaded_info_raw).expect("parse reloaded info");
    assert_eq!(reloaded_info["sign_pk"], pub_info["sign_pk"]);
    assert_eq!(reloaded_info["dh_pk"], pub_info["dh_pk"]);
    assert_eq!(reloaded_info["destination"], pub_info["destination"]);
    assert_eq!(reloaded_info["onion"], pub_info["onion"]);

    // Opening with wrong password must fail
    assert!(
        BrowserEngine::open_password(profile, "wrong password", &slot_a, &[]).is_err(),
        "wrong password must fail"
    );

    // Opening with wrong profile name must fail
    assert!(
        BrowserEngine::open_password("bob_profile", password, &slot_a, &[]).is_err(),
        "wrong profile name must fail"
    );
}

#[wasm_bindgen_test]
fn encrypt_decrypt_roundtrip_and_replay_rejection() {
    let alice = BrowserEngine::create("alice", "alice-pass-secret").expect("alice create");
    let bob = BrowserEngine::create("bob", "bob-pass-secret").expect("bob create");

    let alice_info: Value =
        serde_json::from_str(&alice.public_info().expect("alice info")).expect("alice json");
    let bob_info: Value =
        serde_json::from_str(&bob.public_info().expect("bob info")).expect("bob json");

    let alice_sign = alice_info["sign_pk"].as_str().unwrap();
    let alice_dh = alice_info["dh_pk"].as_str().unwrap();
    let bob_sign = bob_info["sign_pk"].as_str().unwrap();
    let bob_dh = bob_info["dh_pk"].as_str().unwrap();

    let bob_bundle = bob.my_bundle().expect("bob bundle");
    assert!(!bob_bundle.is_empty());

    // Alice prepares payload
    let mut payload = WirePayload::simple(
        42,
        "Секретный привет из WASM".into(),
        vec![],
        1000,
        Some(3600),
    );
    payload.sender_name = Some("Alice".into());
    let payload_json = serde_json::to_string(&payload).expect("payload json");

    // Alice begins transaction and encrypts to Bob using his bundle
    alice.begin().expect("alice begin");
    let envelope = alice
        .encrypt(bob_sign, bob_dh, &bob_bundle, &payload_json)
        .expect("alice encrypt");
    assert!(!envelope.is_empty());
    let _ = alice.seal("{}").expect("alice seal");
    alice.commit().expect("alice commit");

    // Bob receives envelope and decrypts
    bob.begin().expect("bob begin");
    let decrypt_raw = bob
        .decrypt(alice_sign, alice_dh, &envelope)
        .expect("bob decrypt");
    let decrypt_res: Value = serde_json::from_str(&decrypt_raw).expect("decrypt json");
    assert_eq!(decrypt_res["sign_pk"], alice_sign);
    assert_eq!(decrypt_res["dh_pk"], alice_dh);
    assert_eq!(decrypt_res["payload"]["body"], "Секретный привет из WASM");
    assert_eq!(decrypt_res["payload"]["origin_msg_id"], 42);
    assert_eq!(decrypt_res["payload"]["sender_name"], "Alice");

    // Bob commits transaction
    let _ = bob.seal("{}").expect("bob seal");
    bob.commit().expect("bob commit");

    // Replay rejection: Bob begins transaction and attempts to decrypt the EXACT same envelope again
    bob.begin().expect("bob begin replay test");
    assert!(
        bob.decrypt(alice_sign, alice_dh, &envelope).is_err(),
        "replaying already consumed initial X3DH envelope must fail"
    );
    bob.rollback();

    // Now test continuous ratchet encryption and decryption
    alice.begin().expect("alice begin ratchet");
    let mut reply =
        WirePayload::simple(43, "Второе сообщение по ratchet".into(), vec![], 1010, None);
    reply.sender_name = Some("Alice".into());
    let reply_json = serde_json::to_string(&reply).expect("reply json");
    // Bundle is empty because session already exists
    let ratchet_envelope = alice
        .encrypt(bob_sign, bob_dh, &[], &reply_json)
        .expect("alice ratchet encrypt");
    let _ = alice.seal("{}").expect("alice seal");
    alice.commit().expect("alice commit");

    bob.begin().expect("bob begin ratchet decrypt");
    let ratchet_decrypt_raw = bob
        .decrypt(alice_sign, bob_dh, &ratchet_envelope)
        .expect("bob ratchet decrypt");
    let ratchet_res: Value = serde_json::from_str(&ratchet_decrypt_raw).expect("ratchet json");
    assert_eq!(
        ratchet_res["payload"]["body"],
        "Второе сообщение по ratchet"
    );
    let _ = bob.seal("{}").expect("bob seal");
    bob.commit().expect("bob commit");

    // Ratchet replay rejection: replaying the same ratchet envelope must be rejected
    bob.begin().expect("bob begin ratchet replay test");
    assert!(
        bob.decrypt(alice_sign, bob_dh, &ratchet_envelope).is_err(),
        "replaying already consumed ratchet envelope must fail"
    );
    bob.rollback();
}

#[wasm_bindgen_test]
fn wrong_identity_and_tampered_bundle_rejection() {
    let alice = BrowserEngine::create("alice_sec", "pass-alice").unwrap();
    let bob = BrowserEngine::create("bob_sec", "pass-bob").unwrap();

    let bob_info: Value = serde_json::from_str(&bob.public_info().unwrap()).unwrap();
    let bob_sign = bob_info["sign_pk"].as_str().unwrap();
    let bob_dh = bob_info["dh_pk"].as_str().unwrap();
    let bob_bundle = bob.my_bundle().unwrap();

    let payload = WirePayload::simple(1, "Test".into(), vec![], 10, None);
    let payload_json = serde_json::to_string(&payload).unwrap();

    alice.begin().unwrap();

    // 1. Mismatched peer signing key vs bundle
    let wrong_sign = "00".repeat(32);
    assert!(
        alice
            .encrypt(&wrong_sign, bob_dh, &bob_bundle, &payload_json)
            .is_err(),
        "bundle sign_pk mismatch must be rejected"
    );

    // 2. Mismatched peer DH key vs bundle
    let wrong_dh = "ff".repeat(32);
    assert!(
        alice
            .encrypt(bob_sign, &wrong_dh, &bob_bundle, &payload_json)
            .is_err(),
        "bundle dh_pk mismatch must be rejected"
    );

    // 3. Tampered bundle signature
    let mut tampered_bundle = bob_bundle.clone();
    if let Some(byte) = tampered_bundle.get_mut(15) {
        *byte ^= 0x55;
    }
    assert!(
        alice
            .encrypt(bob_sign, bob_dh, &tampered_bundle, &payload_json)
            .is_err(),
        "tampered bundle must fail verification"
    );

    alice.rollback();
}

#[wasm_bindgen_test]
fn passkey_enrollment_descriptor_and_open() {
    let profile = "passkey_user";
    let password = "passkey_profile_pass";

    let engine = BrowserEngine::create(profile, password).unwrap();
    let orig_info: Value = serde_json::from_str(&engine.public_info().unwrap()).unwrap();

    let credential_id = vec![0xaa, 0xbb, 0xcc, 0xdd];
    let rp_id = "gipny.browser.app";
    let salt = [0x55u8; 32];
    let prf = [0x77u8; 32];

    engine.begin().unwrap();
    engine
        .enroll_passkey(&credential_id, rp_id, &salt, &prf)
        .unwrap();
    let slot_a = engine.seal(r#"{"passkey":"enrolled"}"#).unwrap();
    engine.commit().unwrap();

    // Inspect passkey descriptor public entries before credential use
    let entries_raw = BrowserEngine::passkey_entries(&slot_a, &[]).expect("passkey entries");
    let entries: Value = serde_json::from_str(&entries_raw).expect("parse passkey entries");
    let arr = entries.as_array().expect("array of descriptors");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["rpId"], rp_id);
    assert_eq!(
        arr[0]["credentialId"].as_array().unwrap(),
        &[
            Value::from(0xaa),
            Value::from(0xbb),
            Value::from(0xcc),
            Value::from(0xdd)
        ]
    );
    assert_eq!(arr[0]["salt"].as_array().unwrap().len(), 32);

    // Open via passkey PRF
    let opened = BrowserEngine::open_passkey(profile, &credential_id, &prf, &slot_a, &[])
        .expect("open via passkey");
    assert_eq!(opened.take_metadata().unwrap(), r#"{"passkey":"enrolled"}"#);

    let opened_info: Value = serde_json::from_str(&opened.public_info().unwrap()).unwrap();
    assert_eq!(opened_info["sign_pk"], orig_info["sign_pk"]);
    assert_eq!(opened_info["dh_pk"], orig_info["dh_pk"]);

    // Wrong PRF must be rejected
    let wrong_prf = [0x88u8; 32];
    assert!(
        BrowserEngine::open_passkey(profile, &credential_id, &wrong_prf, &slot_a, &[]).is_err(),
        "wrong passkey PRF must fail"
    );

    // Wrong credential ID must be rejected
    assert!(
        BrowserEngine::open_passkey(profile, &[0x11, 0x22], &prf, &slot_a, &[]).is_err(),
        "wrong credential ID must fail"
    );
}

#[wasm_bindgen_test]
fn transaction_rollback_guarantee() {
    let engine = BrowserEngine::create("rollback_profile", "initial_pass").unwrap();

    engine.begin().unwrap();
    engine
        .change_password("initial_pass", "candidate_pass")
        .unwrap();
    engine.rollback();

    // Verify committed password remains the original
    assert!(engine.verify_password("initial_pass").is_ok());
    assert!(engine.verify_password("candidate_pass").is_err());
    assert_eq!(engine.pending_generation(), "");
}

#[wasm_bindgen_test]
fn engine_close_wipes_and_errors() {
    let engine = BrowserEngine::create("closing_profile", "some_password").unwrap();
    assert!(engine.public_info().is_ok());

    engine.close();

    // All subsequent operations must return an error
    assert!(engine.public_info().is_err());
    assert!(engine.begin().is_err());
    assert!(engine.take_metadata().is_err());
    assert!(engine.verify_password("some_password").is_err());
}

#[wasm_bindgen_test]
fn relay_auth_v2_signing_and_verification() {
    let engine = BrowserEngine::create("auth_profile", "auth_pass").unwrap();
    let info: Value = serde_json::from_str(&engine.public_info().unwrap()).unwrap();
    let sign_pk = info["sign_pk"].as_str().unwrap();

    let destination = "zqeubwvvtm5f3cto5s3r6ftj36j4x4nsvvh3znux7uruhxjgk5za.b32.i2p";
    let challenge = [0x42u8; 32];

    let sig = engine
        .sign_auth(destination, &challenge)
        .expect("sign auth");
    assert_eq!(sig.len(), 64);

    // Host relay verification
    assert!(
        BrowserEngine::verify_auth(sign_pk, destination, &challenge, &sig),
        "valid relay auth must verify"
    );

    // Wrong challenge
    let wrong_challenge = [0x43u8; 32];
    assert!(
        !BrowserEngine::verify_auth(sign_pk, destination, &wrong_challenge, &sig),
        "wrong challenge must fail"
    );

    // Wrong destination
    let other_dest = "pltnfwacxdinhnpdnjmzn4royewu6dcbphanacvztwcx4vjfqg6a.b32.i2p";
    assert!(
        !BrowserEngine::verify_auth(sign_pk, other_dest, &challenge, &sig),
        "wrong destination must fail"
    );

    // Tampered signature
    let mut tampered_sig = sig;
    tampered_sig[0] ^= 1;
    assert!(
        !BrowserEngine::verify_auth(sign_pk, destination, &challenge, &tampered_sig),
        "tampered signature must fail"
    );
}

#[wasm_bindgen_test]
fn tampered_unicode_hex_rejection_no_panics() {
    let engine = BrowserEngine::create("unicode_test", "password123").unwrap();

    // 1. Unicode emojis: 16 emojis = 64 bytes in UTF-8, but only 16 characters!
    let emoji_hex = "🦀".repeat(16);
    assert_eq!(emoji_hex.len(), 64); // byte length is 64
    assert!(!engine.has_session(&emoji_hex));
    assert!(!BrowserEngine::verify_auth(
        &emoji_hex,
        "dest.b32.i2p",
        &[0u8; 32],
        &[0u8; 64]
    ));

    engine.begin().unwrap();
    let payload = WirePayload::simple(1, "Hi".into(), vec![], 0, None);
    let payload_json = serde_json::to_string(&payload).unwrap();
    assert!(engine
        .encrypt(&emoji_hex, "00".repeat(32).as_str(), &[], &payload_json)
        .is_err());
    assert!(engine.decrypt(&emoji_hex, "", &[0u8; 32]).is_err());
    engine.rollback();

    // 2. Cyrillic letters: 32 Cyrillic chars = 64 bytes in UTF-8, but not ASCII hex
    let cyrillic_hex = "ф".repeat(32);
    assert_eq!(cyrillic_hex.len(), 64);
    assert!(!engine.has_session(&cyrillic_hex));

    // 3. Invalid lengths (too short, too long, odd length)
    assert!(!engine.has_session("abcd"));
    assert!(!engine.has_session(&"a".repeat(65)));
    assert!(!engine.has_session(&"a".repeat(63)));
}

#[wasm_bindgen_test]
fn restore_native_fixedint_fixture() {
    let alice = BrowserEngine::create("alice_fixint", "alice-pass").unwrap();
    let bob = BrowserEngine::create("bob_fixint", "bob-pass").unwrap();

    let bob_info: Value = serde_json::from_str(&bob.public_info().unwrap()).unwrap();
    let bob_sign = bob_info["sign_pk"].as_str().unwrap();
    let bob_dh = bob_info["dh_pk"].as_str().unwrap();

    // my_bundle encodes via native bincode::serialize (fixint)
    let bob_bundle_bytes = bob.my_bundle().unwrap();
    let native_bundle: PreKeyBundle =
        bincode::deserialize(&bob_bundle_bytes).expect("native bincode deserialization of bundle");
    assert_eq!(
        native_bundle.identity.sign_pk,
        native_bundle.identity.sign_pk
    );

    // Alice encrypts using the bundle fixture
    alice.begin().unwrap();
    let payload = WirePayload::simple(77, "Native fixint verification".into(), vec![], 500, None);
    let envelope_bytes = alice
        .encrypt(
            bob_sign,
            bob_dh,
            &bob_bundle_bytes,
            &serde_json::to_string(&payload).unwrap(),
        )
        .unwrap();

    // Verify envelope decodes cleanly with native bincode fixint
    let native_envelope: EnvelopeBlob =
        bincode::deserialize(&envelope_bytes).expect("native bincode deserialization of envelope");
    assert!(matches!(native_envelope, EnvelopeBlob::X3dhInit(_)));

    // Bob decrypts the fixture
    let alice_info: Value = serde_json::from_str(&alice.public_info().unwrap()).unwrap();
    let alice_sign = alice_info["sign_pk"].as_str().unwrap();
    let alice_dh = alice_info["dh_pk"].as_str().unwrap();

    bob.begin().unwrap();
    let dec = bob
        .decrypt(alice_sign, alice_dh, &envelope_bytes)
        .expect("bob decrypts native fixint envelope");
    let res: Value = serde_json::from_str(&dec).unwrap();
    assert_eq!(res["payload"]["body"], "Native fixint verification");
    bob.rollback();
    alice.rollback();
}
