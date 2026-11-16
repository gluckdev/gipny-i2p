use super::*;

#[test]
fn per_gateway_tags_are_stable_short_neutral_and_unlinkable() {
    // RFC 4231 test case 2: the shim's HMAC is plain HMAC-SHA256.
    assert_eq!(
        hex(&hmac(b"Jefe", b"what do ya want for nothing?").unwrap()),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    let (one, two) = ([1u8; 16], [2u8; 16]);
    let home = gateway_key(&one, "uuid:home", HOME).unwrap();
    assert_eq!(
        home,
        gateway_key(&one, "uuid:home", HOME).unwrap(),
        "stable"
    );
    for other in [
        gateway_key(&one, "uuid:office", HOME).unwrap(),
        gateway_key(&one, "uuid:home", Ipv4Addr::new(10, 0, 0, 1)).unwrap(),
        gateway_key(&two, "uuid:home", HOME).unwrap(),
        gateway_key(&one, "", HOME).unwrap(),
    ] {
        assert_ne!(home.id, other.id);
        assert_ne!(home.label, other.label);
        assert!(!owned_label(&other.label, &home.label));
    }
    assert!(home.label.len() <= 16, "{}", home.label);
    assert!(home.label.starts_with(LABEL));
    assert!(!home.label.to_ascii_lowercase().contains("gipny"));
    assert!(
        !home.label.contains(&home.id[..TAG_MATCH]),
        "label is not the journal id"
    );
    // Routers that truncate descriptions still return ours as ours.
    assert!(owned_label(&home.label, &home.label));
    assert!(owned_label(&format!("{} ", home.label), &home.label));
    assert!(owned_label(
        &home.label[..LABEL.len() + TAG_MATCH],
        &home.label
    ));
    assert!(!owned_label(
        &home.label[..LABEL.len() + TAG_MATCH - 1],
        &home.label
    ));
    assert!(!owned_label("", &home.label));
    assert!(!owned_label(LABEL, &home.label));
}

#[test]
fn corrupt_or_missing_owner_file_is_regenerated_atomically() {
    let scratch = Scratch::new();
    let owner = scratch.0.join("upnp-owner");
    let journal = scratch.0.join("upnp-mappings");
    let first = Registry::open(&scratch.0).unwrap().secret;
    assert_eq!(parse_secret(&read_record(&owner, 64).unwrap()), Some(first));
    assert_eq!(
        Registry::open(&scratch.0).unwrap().secret,
        first,
        "kept when valid"
    );
    let gateway = "0".repeat(32);
    let (short, long, wrong) = ("a".repeat(31), "a".repeat(33), "g".repeat(32));
    for broken in ["", "zz", short.as_str(), long.as_str(), wrong.as_str()] {
        std::fs::write(&owner, broken).unwrap();
        std::fs::write(&journal, format!("TCP 34567 192.168.1.20 {gateway}\n")).unwrap();
        let registry = Registry::open(&scratch.0).unwrap();
        assert_ne!(registry.secret, first, "{broken:?}");
        assert_eq!(
            parse_secret(&read_record(&owner, 64).unwrap()),
            Some(registry.secret)
        );
        assert!(
            registry.entries.is_empty(),
            "old entries are unattributable"
        );
        assert_eq!(read_record(&journal, 4096).unwrap(), "");
    }
    std::fs::write(&owner, vec![0xff; 32]).unwrap();
    assert!(Registry::open(&scratch.0).is_ok(), "not UTF-8");
    std::fs::remove_file(&owner).unwrap();
    std::fs::write(&owner, "b".repeat(4096)).unwrap();
    assert!(Registry::open(&scratch.0).is_ok(), "oversized");
    #[cfg(unix)]
    {
        let target = scratch.0.join("elsewhere");
        std::fs::write(&target, "c".repeat(32)).unwrap();
        std::fs::remove_file(&owner).unwrap();
        std::os::unix::fs::symlink(&target, &owner).unwrap();
        let registry = Registry::open(&scratch.0).unwrap();
        assert_ne!(registry.secret, [0xcc; 16], "a symlink is not followed");
        assert!(std::fs::symlink_metadata(&owner).unwrap().is_file());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "c".repeat(32));
    }
    assert!(!scratch.0.join("upnp-owner.pending").exists());
}

#[test]
fn journal_entries_are_bound_to_their_gateway() {
    let scratch = Scratch::new();
    let mut registry = Registry::open(&scratch.0).unwrap();
    let home = Fake::new(&registry.secret, "uuid:home");
    let office = Fake::new(&registry.secret, "uuid:office");
    let rule = tcp();
    assert_eq!(
        maintain(&home, &rule, &mut registry, &never),
        Outcome::Confirmed { permanent: false }
    );
    // The office gateway has never heard of this port (714), and must not
    // retire the home entry for it — or even be asked about it.
    cleanup_except(&office, &mut registry, &[], &never);
    assert_eq!(office.state.borrow().queries, 0);
    assert_eq!(registry.rules_on(&home.key.id), vec![rule.clone()]);
    let reopened = Registry::open(&scratch.0).unwrap();
    assert_eq!(reopened.entries, registry.entries);
    assert_eq!(reopened.entries[0].gateway, home.key.id);
    // The home gateway's own 714 does retire it.
    home.state.borrow_mut().entries.clear();
    cleanup_except(&home, &mut registry, &[], &never);
    assert!(registry.entries.is_empty());
    assert_eq!(home.state.borrow().deletes, 0, "nothing to delete");
    // A full journal makes room from other gateways, never from this one.
    for port in 1..=JOURNAL_LIMIT as u16 {
        assert!(registry.reserve(
            &Rule {
                port,
                ..rule.clone()
            },
            &home.key.id
        ));
    }
    assert!(!registry.reserve(
        &Rule {
            port: 999,
            ..rule.clone()
        },
        &home.key.id
    ));
    assert!(registry.reserve(
        &Rule {
            port: 999,
            ..rule.clone()
        },
        &office.key.id
    ));
    assert_eq!(registry.entries.len(), JOURNAL_LIMIT);
    assert_eq!(registry.rules_on(&home.key.id).len(), JOURNAL_LIMIT - 1);
    assert!(
        !registry.rules_on(&home.key.id).iter().any(|r| r.port == 1),
        "oldest evicted"
    );
}
