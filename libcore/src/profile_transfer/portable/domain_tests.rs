use super::{contacts::*, crypto, groups::*, text::Text, *};
use crate::crypto::{Identity, IdentityCard, PreKeyPair};

fn identity(seed: u8) -> IdentityCard {
    Identity::from_bytes([seed; 32], [seed + 10; 32]).card()
}
fn contact(id: u64, seed: u8) -> Contact {
    let card = identity(seed);
    Contact {
        id,
        sign_pk: card.sign_pk,
        dh_pk: card.dh_pk,
        i2p_address: None,
        relay_address: Some(Text::new(&format!("{}.b32.i2p", "a".repeat(52))).unwrap()),
        name: Text::new("Собеседник 🧄").unwrap(),
        trust: Trust::Blocked,
        request: Request::Incoming,
        created_at_ms: 123,
        last_seen_ms: None,
        pinned_at_ms: Some(456),
        last_message_at_ms: Some(789),
        is_bot: true,
        agent_granted: true,
        save_history: Some(false),
    }
}
fn contacts() -> Contacts {
    Contacts {
        version: VERSION,
        next_contact_id: 100,
        contacts: List::new(vec![contact(2, 2)]).unwrap(),
    }
}
fn member(seed: u8) -> Member {
    let card = identity(seed);
    Member {
        sign_pk: card.sign_pk,
        dh_pk: card.dh_pk,
        i2p_address: None,
        name: Text::new("member").unwrap(),
        is_self: seed == 1,
    }
}
fn group(id: u8, seeds: &[u8]) -> Group {
    Group {
        id: [id; 32],
        name: Text::new("team").unwrap(),
        created_at_ms: 1,
        pinned_at_ms: None,
        last_message_at_ms: Some(5),
        members: List::new(seeds.iter().map(|s| member(*s)).collect()).unwrap(),
    }
}
fn groups() -> Groups {
    Groups {
        version: VERSION,
        groups: List::new(vec![group(5, &[1, 2])]).unwrap(),
    }
}

#[test]
fn contact_roundtrip_does_not_upgrade_trust_admission_or_reset_allocator() {
    let input = contacts();
    let bytes = input.encode(&identity(1)).unwrap();
    let parsed = Contacts::decode(&bytes, &identity(1)).unwrap();
    assert_eq!(parsed.next_contact_id, 100);
    let peer = &parsed.contacts[0];
    assert_eq!(peer.trust, Trust::Blocked);
    assert_eq!(peer.request, Request::Incoming);
    assert!(peer.is_bot && peer.agent_granted);
    assert_eq!(peer.save_history, Some(false));
    assert_eq!(peer.name.as_str(), "Собеседник 🧄");
    assert!(peer.i2p_address.is_none());
    assert_eq!(peer.pinned_at_ms, Some(456));
    assert_eq!(
        parsed.encode(&identity(1)).unwrap().as_slice(),
        bytes.as_slice()
    );
}

#[test]
fn contact_duplicates_key_substitution_and_non_i2p_routes_fail() {
    for peers in [
        vec![contact(2, 2), contact(3, 2)],
        vec![contact(2, 2), contact(2, 3)],
    ] {
        let bad = Contacts {
            version: VERSION,
            next_contact_id: 100,
            contacts: List::new(peers).unwrap(),
        };
        assert!(bad.index(&identity(1)).is_err());
    }
    let mut bad = contact(3, 3);
    bad.dh_pk = identity(2).dh_pk;
    let bad = Contacts {
        version: VERSION,
        next_contact_id: 100,
        contacts: List::new(vec![contact(2, 2), bad]).unwrap(),
    };
    assert!(matches!(bad.index(&identity(1)), Err(Error::Duplicate)));
    for address in [
        "https://example.com/profile",
        "192.168.1.1",
        "",
        "../profile",
    ] {
        let mut bad = contact(2, 2);
        bad.i2p_address = Some(Text::new(address).unwrap());
        let bad = Contacts {
            version: VERSION,
            next_contact_id: 100,
            contacts: List::new(vec![bad]).unwrap(),
        };
        assert!(matches!(
            bad.index(&identity(1)),
            Err(Error::InvalidContact)
        ));
    }
    for dh in [[0; 32], {
        let mut one = [0; 32];
        one[0] = 1;
        one
    }] {
        let mut bad = contact(2, 2);
        bad.dh_pk = dh;
        let bad = Contacts {
            version: VERSION,
            next_contact_id: 100,
            contacts: List::new(vec![bad]).unwrap(),
        };
        assert!(matches!(
            bad.index(&identity(1)),
            Err(Error::InvalidIdentity)
        ));
    }
}

#[test]
fn contact_high_water_version_and_exact_decode_are_required() {
    let mut bad = contacts();
    bad.next_contact_id = 2;
    assert!(matches!(
        bad.index(&identity(1)),
        Err(Error::InvalidContact)
    ));
    let mut bad = contacts();
    bad.next_contact_id = MAX_ID + 1;
    assert!(matches!(
        bad.index(&identity(1)),
        Err(Error::InvalidContact)
    ));
    let mut bad = contacts();
    bad.version += 1;
    assert!(matches!(
        bad.index(&identity(1)),
        Err(Error::UnsupportedVersion)
    ));
    let mut bytes = contacts().encode(&identity(1)).unwrap();
    bytes.push(0);
    assert!(matches!(
        Contacts::decode(&bytes, &identity(1)),
        Err(Error::Codec)
    ));
}

#[test]
fn group_roundtrip_preserves_members_and_validates_cross_domain_key_pinning() {
    let index = contacts().index(&identity(1)).unwrap();
    let bytes = groups().encode(&index).unwrap();
    let parsed = Groups::decode(&bytes, &index).unwrap();
    assert_eq!(parsed.groups[0].members.len(), 2);
    assert!(parsed.groups[0].members[0].is_self);
    assert_eq!(parsed.groups[0].members[1].sign_pk, identity(2).sign_pk);
    assert_eq!(parsed.encode(&index).unwrap().as_slice(), bytes.as_slice());
    let mut changed = group(5, &[1]);
    let mut peer = member(2);
    peer.dh_pk = identity(3).dh_pk;
    changed.members = List::new(vec![member(1), peer]).unwrap();
    let bad = Groups {
        version: VERSION,
        groups: List::new(vec![changed]).unwrap(),
    };
    assert_eq!(bad.validate(&index), Err(Error::InvalidReference));
}

#[test]
fn group_cannot_spoof_self_duplicate_member_or_rebind_unknown_member_across_groups() {
    let index = contacts().index(&identity(1)).unwrap();
    for group in [group(5, &[2]), group(5, &[1, 1]), group(5, &[])] {
        let bad = Groups {
            version: VERSION,
            groups: List::new(vec![group]).unwrap(),
        };
        assert_eq!(bad.validate(&index), Err(Error::InvalidGroup));
    }
    let mut unknown = member(3);
    unknown.dh_pk = identity(4).dh_pk;
    let mut second = group(6, &[1]);
    second.members = List::new(vec![member(1), unknown]).unwrap();
    let bad = Groups {
        version: VERSION,
        groups: List::new(vec![group(5, &[1, 3]), second]).unwrap(),
    };
    assert_eq!(bad.validate(&index), Err(Error::InvalidReference));
    let mut peer = member(2);
    peer.is_self = true;
    let mut second = group(5, &[1]);
    second.members = List::new(vec![member(1), peer]).unwrap();
    assert_eq!(
        Groups {
            version: VERSION,
            groups: List::new(vec![second]).unwrap()
        }
        .validate(&index),
        Err(Error::InvalidGroup)
    );
}

#[test]
fn crypto_peer_mapping_requires_exact_contact_and_own_identity() {
    let own = identity(1);
    let pk = PreKeyPair::from_secret([6; 32]);
    let mut state = crypto::State {
        version: VERSION,
        sign_seed: crypto::Secret32::new([1; 32]),
        dh_secret: crypto::Secret32::new([11; 32]),
        sign_pk: own.sign_pk,
        dh_pk: own.dh_pk,
        current_signed_prekey: 1,
        next_prekey_id: 10,
        signed_prekeys: List::new(vec![crypto::Prekey {
            id: 1,
            secret: crypto::Secret32::new([6; 32]),
            public: *pk.public(),
            created_at_ms: None,
        }])
        .unwrap(),
        one_time_prekeys: List::new(vec![]).unwrap(),
        peers: List::new(vec![]).unwrap(),
    };
    let peer = |seed| {
        let key = identity(seed);
        crypto::Peer {
            sign_pk: key.sign_pk,
            dh_pk: key.dh_pk,
            primary: None,
            own_initialization: None,
            lost_initialization: None,
            alternates: List::new(vec![]).unwrap(),
        }
    };
    let index = contacts().index(&own).unwrap();
    state.peers = List::new(vec![peer(2)]).unwrap();
    assert_eq!(index.validate_crypto(&state), Ok(()));
    state.peers = List::new(vec![peer(3)]).unwrap();
    assert_eq!(index.validate_crypto(&state), Err(Error::InvalidReference));
    assert!(Contacts {
        version: VERSION,
        next_contact_id: 1,
        contacts: List::new(vec![]).unwrap()
    }
    .index(&identity(4))
    .unwrap()
    .validate_crypto(&state)
    .is_err());
}

#[test]
fn utf8_text_bounds_are_bytes_exact_and_reject_invalid_or_truncated_input() {
    assert!(Text::<1>::new("я").is_err());
    let text = Text::<2>::new("я").unwrap();
    let encoded = encode(&text, 10).unwrap();
    assert_eq!(encoded.len(), 10); // fixed u64 byte length + two UTF-8 bytes
    assert_eq!(decode::<Text<2>>(&encoded, 10).unwrap().as_str(), "я");
    for end in 0..encoded.len() {
        assert!(decode::<Text<2>>(&encoded[..end], 10).is_err());
    }
    let mut invalid = 1u64.to_le_bytes().to_vec();
    invalid.push(0xff);
    assert!(decode::<Text<2>>(&invalid, 10).is_err());
    assert!(decode::<Text<2>>(&u64::MAX.to_le_bytes(), 10).is_err());
    assert!(serde_json::from_str::<Text<2>>("[97,98,99]").is_err());
    assert_eq!(
        serde_json::from_str::<Text<2>>("[97,98]").unwrap().as_str(),
        "ab"
    );
}

#[test]
fn utf8_text_empty_and_max_bounds() {
    let empty = Text::<16>::new("").unwrap();
    assert_eq!(empty.as_str(), "");
    let max_text = "a".repeat(16);
    let bounded = Text::<16>::new(&max_text).unwrap();
    assert_eq!(bounded.as_str(), max_text);
    let over_text = "a".repeat(17);
    assert!(Text::<16>::new(&over_text).is_err());
}
