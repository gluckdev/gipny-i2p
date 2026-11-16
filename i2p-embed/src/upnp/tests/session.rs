use super::*;

#[test]
fn focus_and_network_churn_never_delete_or_recreate_mappings() {
    let scratch = Scratch::new();
    let registry = Registry::open(&scratch.0).unwrap();
    let home = Fake::new(&registry.secret, "uuid:home");
    let mut env = FakeEnv::new(&[&home]);
    let t0 = Instant::now();
    let mut session = Session::new(registry, t0);
    assert_eq!(session.cycle(&mut env, t0, &never), RENEW);
    assert_eq!(home.state.borrow().entries.len(), 2);
    let (adds, queries) = (home.state.borrow().adds, home.state.borrow().queries);
    // A burst of focus events: at most one look per RECHECK, and a look is
    // one root-description fetch, no SOAP at all.
    for second in 1..=20 {
        let now = t0 + Duration::from_secs(second);
        session.network(false, now);
        assert!(session.cycle(&mut env, now, &never) > Duration::ZERO);
    }
    assert_eq!(home.state.borrow().verifies, 0, "rate-limited");
    let now = t0 + RECHECK;
    assert_eq!(session.cycle(&mut env, now, &never), RENEW - RECHECK);
    // Offline then online again (Android switching networks back).
    session.network(true, now + Duration::from_secs(5));
    assert_eq!(
        session.cycle(&mut env, now + Duration::from_secs(5), &never),
        BACKOFF_CAP
    );
    session.network(false, now + Duration::from_secs(6));
    let later = now + RECHECK;
    session.cycle(&mut env, later, &never);
    let state = home.state.borrow();
    assert_eq!(state.verifies, 2);
    assert_eq!(
        (state.adds, state.queries, state.deletes),
        (adds, queries, 0)
    );
    assert_eq!(state.entries.len(), 2);
    drop(state);
    assert_eq!(env.searches, 1, "the gateway was kept");
    // The scheduled pass verifies and renews in place.
    session.cycle(&mut env, t0 + RENEW, &never);
    assert_eq!(home.state.borrow().deletes, 0);
    assert_eq!(home.state.borrow().verifies, 3);
    assert_eq!(env.published, vec![WAN], "published once");
}

#[test]
fn a_different_gateway_is_left_alone_and_the_journal_keeps_both() {
    let scratch = Scratch::new();
    let registry = Registry::open(&scratch.0).unwrap();
    let home = Fake::new(&registry.secret, "uuid:home");
    let office = Fake::new(&registry.secret, "uuid:office");
    let mut env = FakeEnv::new(&[&home, &office]);
    let t0 = Instant::now();
    let mut session = Session::new(registry, t0);
    session.cycle(&mut env, t0, &never);
    home.state.borrow_mut().moved = true;
    let now = t0 + RECHECK;
    session.network(false, now);
    session.cycle(&mut env, now, &never);
    assert_eq!(env.searches, 2);
    assert_eq!(home.state.borrow().deletes, 0, "old gateway untouched");
    assert_eq!(office.state.borrow().entries.len(), 2);
    let labels: HashSet<_> = office
        .state
        .borrow()
        .entries
        .values()
        .map(|e| e.1.clone())
        .collect();
    assert_eq!(labels, HashSet::from([office.key.label.clone()]));
    assert_eq!(session.registry.rules_on(&home.key.id).len(), 2);
    assert_eq!(session.registry.rules_on(&office.key.id).len(), 2);
}

#[test]
fn foreign_ports_and_one_failing_rule_keep_the_gateway_and_healthy_mappings() {
    let scratch = Scratch::new();
    let registry = Registry::open(&scratch.0).unwrap();
    let home = Fake::new(&registry.secret, "uuid:home");
    home.foreign("UDP", 34568);
    let mut env = FakeEnv::new(&[&home]);
    let t0 = Instant::now();
    let mut session = Session::new(registry, t0);
    for pass in 0..4u32 {
        session.cycle(&mut env, t0 + RENEW * pass, &never);
    }
    assert_eq!(env.searches, 1);
    assert_eq!(home.state.borrow().deletes, 0);
    assert_eq!(
        home.state.borrow().entries[&("UDP", 34568)].1,
        "other application"
    );
    assert_eq!(session.states[1].1, Outcome::Foreign);
    // A SOAP error on one rule: the other one stays, so does the gateway.
    home.state.borrow_mut().failing.insert("UDP");
    for pass in 4..8u32 {
        assert_eq!(session.cycle(&mut env, t0 + RENEW * pass, &never), RETRY);
    }
    assert_eq!(env.searches, 1);
    assert_eq!(home.state.borrow().deletes, 0);
    // Everything failing twice in a row: forget the handle, delete nothing.
    home.state.borrow_mut().failing.insert("TCP");
    session.cycle(&mut env, t0 + RENEW * 8, &never);
    assert!(session.gateway.is_some());
    session.cycle(&mut env, t0 + RENEW * 9, &never);
    assert!(session.gateway.is_none());
    assert_eq!(home.state.borrow().deletes, 0);
    assert!(home.state.borrow().entries.contains_key(&("TCP", 34567)));
}

#[test]
fn external_address_is_published_only_after_a_confirmed_mapping() {
    let scratch = Scratch::new();
    let registry = Registry::open(&scratch.0).unwrap();
    let home = Fake::new(&registry.secret, "uuid:home");
    home.foreign("TCP", 34567);
    home.foreign("UDP", 34568);
    let mut env = FakeEnv::new(&[&home]);
    let t0 = Instant::now();
    let mut session = Session::new(registry, t0);
    session.cycle(&mut env, t0, &never);
    assert!(env.published.is_empty());
    home.state.borrow_mut().failing.extend(["TCP", "UDP"]);
    session.cycle(&mut env, t0 + RENEW, &never);
    assert!(env.published.is_empty());
    home.state.borrow_mut().failing.clear();
    home.state.borrow_mut().entries.clear();
    session.cycle(&mut env, t0 + RENEW + RETRY, &never);
    session.cycle(&mut env, t0 + RENEW * 2 + RETRY, &never);
    assert_eq!(env.published, vec![WAN]);
}

#[test]
fn shutdown_removes_only_permanent_leases_and_honours_the_deadline() {
    let scratch = Scratch::new();
    let registry = Registry::open(&scratch.0).unwrap();
    let finite = Fake::new(&registry.secret, "uuid:finite");
    let mut env = FakeEnv::new(&[&finite]);
    let t0 = Instant::now();
    let mut session = Session::new(registry, t0);
    session.cycle(&mut env, t0, &never);
    let queries = finite.state.borrow().queries;
    session.shutdown(&never);
    assert_eq!(
        finite.state.borrow().queries,
        queries,
        "finite leases are not touched"
    );
    assert_eq!(finite.state.borrow().entries.len(), 2);
    assert_eq!(
        session.registry.rules_on(&finite.key.id).len(),
        2,
        "kept for next start"
    );

    let permanent = Fake::new(&session.registry.secret, "uuid:permanent");
    permanent.state.borrow_mut().permanent_only = true;
    let mut env = FakeEnv::new(&[&permanent]);
    let mut session = Session::new(Registry::open(&scratch.0).unwrap(), t0);
    session.cycle(&mut env, t0, &never);
    assert_eq!(
        session
            .states
            .iter()
            .filter(|s| s.1 == Outcome::Confirmed { permanent: true })
            .count(),
        2
    );
    let exhausted = || true;
    session.shutdown(&exhausted);
    assert_eq!(
        permanent.state.borrow().deletes,
        0,
        "deadline passed: no new exchange"
    );

    let mut env = FakeEnv::new(&[&permanent]);
    let mut session = Session::new(Registry::open(&scratch.0).unwrap(), t0);
    session.cycle(&mut env, t0, &never);
    session.shutdown(&never);
    assert_eq!(permanent.state.borrow().deletes, 2);
    assert!(permanent.state.borrow().entries.is_empty());
    assert!(session.registry.rules_on(&permanent.key.id).is_empty());
    assert_eq!(session.registry.rules_on(&finite.key.id).len(), 2);
}

#[test]
fn discovery_backs_off_and_resets_on_network_change() {
    let schedule: Vec<u64> = (1..=8).map(|n| backoff(n).as_secs()).collect();
    assert_eq!(schedule, [60, 120, 240, 480, 960, 1800, 1800, 1800]);
    assert_eq!(backoff(u32::MAX), BACKOFF_CAP);
    let scratch = Scratch::new();
    let mut env = FakeEnv::new(&[]);
    let t0 = Instant::now();
    let mut session: Session<Fake> = Session::new(Registry::open(&scratch.0).unwrap(), t0);
    let mut now = t0;
    for expected in [60, 120, 240, 480] {
        let wait = session.cycle(&mut env, now, &never);
        assert_eq!(wait.as_secs(), expected);
        // Nothing happens before it is due.
        assert_eq!(
            session
                .cycle(&mut env, now + Duration::from_secs(1), &never)
                .as_secs(),
            expected - 1
        );
        now += wait;
    }
    assert_eq!(session.cycle(&mut env, now, &never).as_secs(), 960);
    assert_eq!(env.searches, 5);
    session.network(false, now + Duration::from_secs(1));
    let wait = session.cycle(&mut env, now + Duration::from_secs(1), &never);
    assert_eq!(
        wait.as_secs(),
        29,
        "not before RECHECK after the last search"
    );
    assert_eq!(
        session.cycle(&mut env, now + RECHECK, &never),
        RETRY,
        "backoff restarted"
    );
    assert_eq!(env.searches, 6);
}

#[test]
fn disabled_listeners_remove_our_mappings_but_a_stopping_router_does_not() {
    let scratch = Scratch::new();
    let registry = Registry::open(&scratch.0).unwrap();
    let home = Fake::new(&registry.secret, "uuid:home");
    let mut env = FakeEnv::new(&[&home]);
    let t0 = Instant::now();
    let mut session = Session::new(registry, t0);
    session.cycle(&mut env, t0, &never);
    env.ports = Ports::Stopped;
    session.cycle(&mut env, t0 + RENEW, &never);
    assert_eq!(home.state.borrow().deletes, 0);
    env.ports = Ports::Disabled;
    session.cycle(&mut env, t0 + RENEW, &never);
    assert_eq!(home.state.borrow().deletes, 2);
    assert!(session.registry.entries.is_empty());
}
