#![cfg(target_arch = "wasm32")]
use gipny_libcore::{
    crypto::{self, Identity, PreKeyBundle, PreKeyPair, RatchetState},
    session::{decode_payload, encode_payload, pad_payload, unpad_payload, WirePayload},
};
use wasm_bindgen_test::*;
wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn shared_x3dh_ratchet_survives_storage_and_rejects_replay() {
    let alice = Identity::try_generate().unwrap();
    let bob = Identity::try_generate().unwrap();
    let prekey = PreKeyPair::try_generate().unwrap();
    let opk = PreKeyPair::try_generate().unwrap();
    let bundle = PreKeyBundle::new(&bob, &prekey, Some((9, &opk)));
    let ad = [alice.card().dh_pk, bob.card().dh_pk].concat();
    let (mut sent, initial) = crypto::x3dh_initiate(&alice, &bundle, b"first", &ad).unwrap();
    let (received, plaintext) =
        crypto::x3dh_respond(&bob, &prekey, Some(&opk), &initial, &ad).unwrap();
    assert_eq!(plaintext, b"first");
    let saved = received.to_bytes().unwrap();
    let mut received = RatchetState::from_bytes(&saved).unwrap();
    let (second_header, second) = sent.encrypt(b"second", &ad).unwrap();
    let (third_header, third) = sent.encrypt(b"third", &ad).unwrap();
    assert_eq!(
        received.decrypt(&third_header, &third, &ad).unwrap(),
        b"third"
    );
    let mut received = RatchetState::from_bytes(&received.to_bytes().unwrap()).unwrap();
    assert_eq!(
        received.decrypt(&second_header, &second, &ad).unwrap(),
        b"second"
    );
    assert!(received.decrypt(&second_header, &second, &ad).is_err());
    let (reply_header, reply) = received.encrypt(b"reply", &ad).unwrap();
    assert_eq!(sent.decrypt(&reply_header, &reply, &ad).unwrap(), b"reply");
    let mut forged = bundle.clone();
    forged.signed_prekey[0] ^= 1;
    assert!(crypto::x3dh_initiate(&alice, &forged, b"wrong peer", &ad).is_err());
}

#[wasm_bindgen_test]
fn native_message_codec_and_padding_are_shared_with_the_browser() {
    let mut message = WirePayload::simple(
        17,
        "Сообщение из браузера".into(),
        Vec::new(),
        100,
        Some(3600),
    );
    message.sender_name = Some("Alice".into());
    let wire = encode_payload(&message).unwrap();
    let padded = pad_payload(&wire);
    assert!(padded.len() > wire.len());
    let decoded = decode_payload(&unpad_payload(&padded).unwrap()).unwrap();
    assert_eq!(decoded.body, message.body);
    assert_eq!(decoded.origin_msg_id, 17);
    assert_eq!(decoded.sender_name.as_deref(), Some("Alice"));
    assert!(unpad_payload(&[255; 5]).is_none());
}
