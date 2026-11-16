use super::crypto::{OwnInitialization, Peer, Prekey, Ratchet, Secret32, State};
use super::manifest::{Manifest, Record, Section, SectionInfo};
use super::*;
use crate::crypto::{Identity, PreKeyPair, RatchetState};
use sha2::{Digest, Sha256};

fn prekey(id: u64, byte: u8) -> Prekey {
    let pair = PreKeyPair::from_secret([byte; 32]);
    Prekey {
        id,
        secret: Secret32::new([byte; 32]),
        public: *pair.public(),
        created_at_ms: Some(123),
    }
}

fn state() -> State {
    let card = Identity::from_bytes([1; 32], [2; 32]).card();
    State {
        version: VERSION,
        sign_seed: Secret32::new([1; 32]),
        dh_secret: Secret32::new([2; 32]),
        sign_pk: card.sign_pk,
        dh_pk: card.dh_pk,
        current_signed_prekey: 2,
        signed_prekeys: List::new(vec![prekey(1, 3), prekey(2, 4)]).unwrap(),
        one_time_prekeys: List::new(vec![prekey(3, 5)]).unwrap(),
        // IDs 4..99 were already consumed: not max(remaining) + 1.
        next_prekey_id: 100,
        peers: List::new(vec![]).unwrap(),
    }
}

fn manifest() -> Manifest {
    let mut sections = Vec::new();
    for section in Section::ALL {
        let bytes: &[u8] = if section == Section::Crypto {
            b"crypto"
        } else {
            b""
        };
        sections.push(SectionInfo {
            section,
            length: bytes.len() as u64,
            sha256: Sha256::digest(bytes).into(),
        });
    }
    Manifest {
        version: VERSION,
        profile_instance: [1; 16],
        frozen_revision: 19,
        sections: List::new(sections).unwrap(),
        records: List::new(vec![Record {
            section: Section::Crypto,
            offset: 0,
            length: 6,
            sha256: Sha256::digest(b"crypto").into(),
        }])
        .unwrap(),
    }
}

#[test]
fn crypto_roundtrip_preserves_identity_signed_history_and_spent_high_water() {
    let before = state();
    let after = State::decode(&before.encode().unwrap()).unwrap();
    assert_eq!(after.sign_seed.expose(), before.sign_seed.expose());
    assert_eq!(after.dh_secret.expose(), before.dh_secret.expose());
    assert_eq!(
        after
            .signed_prekeys
            .iter()
            .map(|p| p.id)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(after.current_signed_prekey, 2);
    assert_eq!(after.one_time_prekeys.len(), 1);
    assert_eq!(after.next_prekey_id, 100);
    assert_eq!(
        after.encode().unwrap().as_slice(),
        before.encode().unwrap().as_slice()
    );
}

#[test]
fn transferred_ratchet_reads_delayed_skipped_key_and_continues_both_directions() {
    let spk = PreKeyPair::generate();
    let mut sender = RatchetState::new_alice(&[9; 32], spk.public());
    let mut receiver = RatchetState::new_bob(&[9; 32], &spk);
    let first = sender.encrypt(b"delayed", b"bound-ad").unwrap();
    let second = sender.encrypt(b"received", b"bound-ad").unwrap();
    assert_eq!(
        receiver.decrypt(&second.0, &second.1, b"bound-ad").unwrap(),
        b"received"
    );
    let peer_card = Identity::generate().card();
    let mut snapshot = state();
    snapshot.peers = List::new(vec![Peer {
        sign_pk: peer_card.sign_pk,
        dh_pk: peer_card.dh_pk,
        primary: Some(Ratchet::capture(&receiver).unwrap()),
        lost_initialization: Some(Ratchet::capture(&sender).unwrap()),
        own_initialization: Some(OwnInitialization {
            dh: *spk.public(),
            created_at_ms: None,
        }),
        alternates: List::new(vec![Ratchet::capture(&receiver).unwrap()]).unwrap(),
    }])
    .unwrap();
    let imported = State::decode(&snapshot.encode().unwrap()).unwrap();
    let peer = &imported.peers[0];
    assert!(peer.lost_initialization.is_some());
    assert_eq!(peer.alternates.len(), 1);
    assert_eq!(
        peer.own_initialization.as_ref().unwrap().created_at_ms,
        None
    );
    let mut restored = peer.primary.as_ref().unwrap().restore().unwrap();
    assert_eq!(
        restored.decrypt(&first.0, &first.1, b"bound-ad").unwrap(),
        b"delayed"
    );
    assert!(
        restored.decrypt(&first.0, &first.1, b"bound-ad").is_err(),
        "replay remains rejected"
    );
    let reply = restored.encrypt(b"after transfer", b"bound-ad").unwrap();
    assert_eq!(
        sender.decrypt(&reply.0, &reply.1, b"bound-ad").unwrap(),
        b"after transfer"
    );
}

#[test]
fn crypto_rejects_identity_prekey_binding_and_id_reuse() {
    let mut value = state();
    value.sign_pk[0] ^= 1;
    assert_eq!(value.validate(), Err(Error::InvalidIdentity));
    let mut value = state();
    value.current_signed_prekey = 3; // an OTK is not the current signed prekey
    assert_eq!(value.validate(), Err(Error::InvalidPrekey));
    let mut value = state();
    value.one_time_prekeys = List::new(vec![prekey(2, 5)]).unwrap();
    assert_eq!(value.validate(), Err(Error::InvalidPrekey));
    let mut value = state();
    value.next_prekey_id = 3;
    assert_eq!(value.validate(), Err(Error::InvalidPrekey));
    let mut value = state();
    let mut bad = prekey(3, 5);
    bad.public[0] ^= 1;
    value.one_time_prekeys = List::new(vec![bad]).unwrap();
    assert_eq!(value.validate(), Err(Error::InvalidPrekey));
    let mut value = state();
    value.next_prekey_id = MAX_ID + 1;
    assert_eq!(value.validate(), Err(Error::InvalidPrekey));
}

#[test]
fn crypto_rejects_version_trailing_bytes_and_duplicate_peer() {
    let mut value = state();
    value.version += 1;
    assert_eq!(value.validate(), Err(Error::UnsupportedVersion));
    let mut bytes = state().encode().unwrap();
    bytes.push(0);
    assert!(matches!(State::decode(&bytes), Err(Error::Codec)));
    let card = Identity::generate().card();
    let peer = || Peer {
        sign_pk: card.sign_pk,
        dh_pk: card.dh_pk,
        primary: None,
        lost_initialization: None,
        own_initialization: None,
        alternates: List::new(vec![]).unwrap(),
    };
    let mut value = state();
    value.peers = List::new(vec![peer(), peer()]).unwrap();
    assert_eq!(value.validate(), Err(Error::InvalidPeer));
}

#[test]
fn nested_ratchet_rejects_trailing_bytes_and_unsupported_format() {
    let spk = PreKeyPair::generate();
    let raw = RatchetState::new_bob(&[9; 32], &spk).to_bytes().unwrap();
    // Explicit ratchet format u16, byte-vector u64 length, then exact bincode.
    let mut outer = vec![];
    outer.extend_from_slice(&1u16.to_le_bytes());
    outer.extend_from_slice(&((raw.len() + 1) as u64).to_le_bytes());
    outer.extend_from_slice(&raw);
    outer.push(0);
    let parsed: Ratchet = decode(&outer, MAX_METADATA_BYTES).unwrap();
    assert!(matches!(parsed.restore(), Err(Error::InvalidRatchet)));
    outer[0] = 2;
    let parsed: Ratchet = decode(&outer, MAX_METADATA_BYTES).unwrap();
    assert!(matches!(parsed.restore(), Err(Error::UnsupportedVersion)));
}

#[test]
fn hostile_sequence_lengths_are_rejected_before_elements_are_read() {
    let count = u64::MAX.to_le_bytes();
    assert!(decode::<List<u8, 1024>>(&count, MAX_METADATA_BYTES).is_err());
    assert!(List::<u8, 1>::new(vec![0, 1]).is_err());
    let mut ratchet = vec![1, 0];
    ratchet.extend_from_slice(&count);
    assert!(decode::<Ratchet>(&ratchet, MAX_METADATA_BYTES).is_err());
    // Includes the no-size-hint path used by JSON/JS boundary callers.
    assert!(serde_json::from_str::<List<u8, 2>>("[1,2,3]").is_err());
    assert_eq!(
        &*serde_json::from_str::<List<u8, 2>>("[1,2]").unwrap(),
        &[1, 2]
    );
}

#[test]
fn secret_fixed_array_rejects_every_truncation_and_extra_element() {
    let expected = Secret32::new([7; 32]);
    let bytes = encode(&expected, 32).unwrap();
    assert_eq!(bytes.as_slice(), &[7; 32]);
    for length in 0..32 {
        assert!(decode::<Secret32>(&bytes[..length], 32).is_err());
    }
    assert_eq!(decode::<Secret32>(&bytes, 32).unwrap().expose(), &[7; 32]);
    for length in [31, 33] {
        let json = serde_json::to_string(&vec![7u8; length]).unwrap();
        assert!(serde_json::from_str::<Secret32>(&json).is_err());
    }
    assert!(serde_json::from_str::<Secret32>("[7,7,7,\"bad\"]").is_err());
}

#[test]
fn manifest_roundtrip_and_digest_bind_revision_order_and_record_bytes() {
    let value = manifest();
    assert_eq!(value.validate(), Ok(6));
    let imported = Manifest::decode(&value.encode().unwrap()).unwrap();
    assert_eq!(value.digest().unwrap(), imported.digest().unwrap());
    let imported = imported.into_layout().unwrap();
    assert_eq!(imported.verify_record(0, b"crypto"), Ok(()));
    assert_eq!(
        imported.verify_record(0, b"crypt0"),
        Err(Error::InvalidManifest)
    );
    assert_eq!(
        imported.verify_record(0, b"crypt"),
        Err(Error::InvalidManifest)
    );
    assert_eq!(
        imported.verify_record(1, b"crypto"),
        Err(Error::InvalidManifest)
    );
    let mut changed = manifest();
    changed.frozen_revision += 1;
    assert_ne!(value.digest().unwrap(), changed.digest().unwrap());
    let mut changed = manifest();
    changed.profile_instance[0] ^= 1;
    assert_ne!(value.digest().unwrap(), changed.digest().unwrap());
}

#[test]
fn manifest_rejects_holes_overlap_missing_domain_and_wrong_total() {
    for offset in [1, 6, u64::MAX] {
        let mut value = manifest();
        let mut records = value.records.into_vec();
        records[0].offset = offset;
        value.records = List::new(records).unwrap();
        assert_eq!(value.validate(), Err(Error::InvalidManifest));
    }
    let mut value = manifest();
    let mut records = value.records.into_vec();
    records.push(records[0].clone());
    value.records = List::new(records).unwrap();
    assert_eq!(value.validate(), Err(Error::InvalidManifest));
    let mut value = manifest();
    let mut sections = value.sections.into_vec();
    sections.pop();
    value.sections = List::new(sections).unwrap();
    assert_eq!(value.validate(), Err(Error::MissingSection));
    let mut value = manifest();
    let mut sections = value.sections.into_vec();
    sections[0].length += 1;
    value.sections = List::new(sections).unwrap();
    assert_eq!(value.validate(), Err(Error::InvalidManifest));
    let mut value = manifest();
    let mut sections = value.sections.into_vec();
    sections.swap(1, 2);
    value.sections = List::new(sections).unwrap();
    assert_eq!(value.validate(), Err(Error::InvalidManifest));
}

#[test]
fn manifest_partial_record_never_counts_as_verified() {
    let value = manifest();
    let layout = manifest().into_layout().unwrap();
    for end in 0..6 {
        assert!(layout.verify_record(0, &b"crypto"[..end]).is_err());
    }
    let mut bytes = value.encode().unwrap();
    bytes.push(0);
    assert!(Manifest::decode(&bytes).is_err());
}
