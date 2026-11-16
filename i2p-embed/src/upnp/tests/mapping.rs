use super::*;

#[test]
fn create_only_after_explicit_absence_and_never_overwrite_foreign_rules() {
    let label = "UPnP 0123456789";
    let rule = tcp();
    assert_eq!(decision(&Err(714), &rule, label), Decision::Create);
    for error in [-1, 402, 501, 606, 718] {
        assert_eq!(decision(&Err(error), &rule, label), Decision::Retry);
    }
    let own = || Mapping {
        client: rule.client,
        port: rule.port,
        description: label.into(),
        enabled: true,
        lease: Some(600),
    };
    assert_eq!(decision(&Ok(own()), &rule, label), Decision::Renew);
    let mut foreign = own();
    foreign.description = "other application".into();
    assert_eq!(decision(&Ok(foreign), &rule, label), Decision::Foreign);
    let mut foreign = own();
    foreign.client = Ipv4Addr::new(192, 168, 1, 21);
    assert_eq!(decision(&Ok(foreign), &rule, label), Decision::Foreign);
    let mut foreign = own();
    foreign.port += 1;
    assert_eq!(decision(&Ok(foreign), &rule, label), Decision::Foreign);
}

#[test]
fn conflict_718_is_settled_by_looking_again() {
    let scratch = Scratch::new();
    let mut registry = Registry::open(&scratch.0).unwrap();
    let home = Fake::new(&registry.secret, "uuid:home");
    let rule = tcp();
    home.state
        .borrow_mut()
        .entries
        .insert(("TCP", 34567), (ME, home.key.label.clone(), 0));
    home.state.borrow_mut().add_result = Some(718);
    assert_eq!(
        maintain(&home, &rule, &mut registry, &never),
        Outcome::Confirmed { permanent: true }
    );
    home.state
        .borrow_mut()
        .entries
        .insert(("TCP", 34567), (ME, "UPnP ffffffffff".into(), 0));
    assert_eq!(
        maintain(&home, &rule, &mut registry, &never),
        Outcome::Foreign
    );
    home.state.borrow_mut().entries.clear();
    assert_eq!(
        maintain(&home, &rule, &mut registry, &never),
        Outcome::Failed
    );
}
