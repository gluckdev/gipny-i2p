use super::*;
use std::collections::{HashSet, VecDeque};

const CHALLENGE: Challenge = [4; 32];
fn binding() -> Binding {
    Binding { transfer_id: [1; 16], recipient_key: [2; 32], manifest_digest: [3; 32] }
}
fn source() -> SourceJournal { SourceJournal::frozen(binding()).unwrap() }
fn receiver() -> ReceiverJournal { ReceiverJournal::receiving(binding()).unwrap() }
fn advance_source(state: SourceJournal, event: SourceEvent) -> SourceJournal {
    state.propose(binding(), event).unwrap().next
}
fn confirming() -> SourceJournal {
    advance_source(advance_source(source(), SourceEvent::ReceiverPrepared), SourceEvent::UserOk(CHALLENGE))
}
fn decided() -> SourceJournal { advance_source(confirming(), SourceEvent::ReceiverReady(CHALLENGE)) }
fn prepared() -> ReceiverJournal { receiver().propose(binding(), ReceiverEvent::StagePrepared).unwrap().next }

#[derive(Clone, Copy)]
enum Fault { None, BeforeWrite, LostReplyBeforeWrite, LostReplyAfterSync, Conflict }
struct Store<J> { saved: J, fault: Fault, writes: usize }
impl<J> Store<J> {
    fn new(saved: J, fault: Fault) -> Self { Self { saved, fault, writes: 0 } }
}
impl<J: Clone + PartialEq> JournalStore<J> for Store<J> {
    fn compare_and_sync(&mut self, expected: &J, next: &J) -> StoreOutcome {
        self.writes += 1;
        if &self.saved != expected { return StoreOutcome::Conflict; }
        match self.fault {
            Fault::None => { self.saved = next.clone(); StoreOutcome::Committed }
            Fault::BeforeWrite => StoreOutcome::NotCommitted,
            Fault::LostReplyBeforeWrite => StoreOutcome::Unknown,
            Fault::LostReplyAfterSync => { self.saved = next.clone(); StoreOutcome::Unknown }
            Fault::Conflict => StoreOutcome::Conflict,
        }
    }
    fn read_current(&mut self) -> Result<J, Error> { Ok(self.saved.clone()) }
}

#[test]
fn confirmation_requires_prepared_then_exact_fresh_challenge() {
    assert_eq!(source().propose(binding(), SourceEvent::UserOk(CHALLENGE)).unwrap_err(), Error::InvalidTransition);
    assert_eq!(source().propose(binding(), SourceEvent::ErasureCompleted).unwrap_err(), Error::InvalidTransition);
    let ready = advance_source(source(), SourceEvent::ReceiverPrepared);
    assert_eq!(ready.recovery_actions(), vec![Action::ShowSourceConfirmation]);
    assert_eq!(ready.propose(binding(), SourceEvent::UserOk([0; 32])).unwrap_err(), Error::InvalidChallenge);
    assert_eq!(ready.propose(binding(), SourceEvent::ReceiverReady(CHALLENGE)).unwrap_err(), Error::InvalidTransition);
    let state = confirming();
    assert_eq!(state.recovery_actions(), vec![Action::RequestReceiverReady(CHALLENGE)]);
    assert_eq!(state.propose(binding(), SourceEvent::ReceiverReady([9; 32])).unwrap_err(), Error::StaleChallenge);
    assert_eq!(state.propose(binding(), SourceEvent::UserOk([9; 32])).unwrap_err(), Error::StaleChallenge);
    assert_eq!(decided().recovery_actions(), vec![Action::EraseSource]);
}

#[test]
fn every_binding_component_is_checked_before_any_transition() {
    let mut cases = [binding(); 3];
    cases[0].transfer_id[0] ^= 1;
    cases[1].recipient_key[0] ^= 1;
    cases[2].manifest_digest[0] ^= 1;
    for other in cases {
        assert_eq!(confirming().propose(other, SourceEvent::ReceiverReady(CHALLENGE)).unwrap_err(), Error::BindingMismatch);
        assert_eq!(prepared().propose(other, ReceiverEvent::SourceErased).unwrap_err(), Error::BindingMismatch);
        assert_eq!(prepared().propose(other, ReceiverEvent::SourceAbort).unwrap_err(), Error::BindingMismatch);
    }
}

#[test]
fn abort_is_terminal_and_commit_cannot_be_cancelled() {
    for state in [source(), advance_source(source(), SourceEvent::ReceiverPrepared), confirming()] {
        let aborted = advance_source(state, SourceEvent::UserAbort);
        assert!(aborted.allows_profile_use());
        assert_eq!(advance_source(aborted.clone(), SourceEvent::UserAbort), aborted);
        assert_eq!(aborted.propose(binding(), SourceEvent::ReceiverReady(CHALLENGE)).unwrap_err(), Error::InvalidTransition);
    }
    let erased = advance_source(decided(), SourceEvent::ErasureCompleted);
    let completed = advance_source(erased.clone(), SourceEvent::ReceiverActivated);
    for state in [decided(), erased, completed] {
        assert!(!state.allows_profile_use());
        assert_eq!(state.propose(binding(), SourceEvent::UserAbort).unwrap_err(), Error::InvalidTransition);
        assert_eq!(advance_source(state.clone(), SourceEvent::ReceiverReady(CHALLENGE)), state);
    }
}

#[test]
fn prepared_is_retained_and_activation_requires_erased_then_local_commit() {
    let state = prepared();
    for event in [ReceiverEvent::LocalAbort, ReceiverEvent::Timeout, ReceiverEvent::ActivationCompleted] {
        assert_eq!(state.propose(binding(), event).unwrap_err(), Error::InvalidTransition);
    }
    assert_eq!(receiver().propose(binding(), ReceiverEvent::SourceErased).unwrap_err(), Error::InvalidTransition);
    let activating = state.propose(binding(), ReceiverEvent::SourceErased).unwrap().next;
    assert!(!activating.allows_profile_use());
    assert_eq!(activating.propose(binding(), ReceiverEvent::SourceAbort).unwrap_err(), Error::InvalidTransition);
    let active = activating.propose(binding(), ReceiverEvent::ActivationCompleted).unwrap().next;
    assert!(active.allows_profile_use());
    assert_eq!(active.propose(binding(), ReceiverEvent::SourceErased).unwrap().next, active);
    let aborted = state.propose(binding(), ReceiverEvent::SourceAbort).unwrap().next;
    assert_eq!(aborted.propose(binding(), ReceiverEvent::SourceErased).unwrap_err(), Error::InvalidTransition);
}

#[test]
fn definite_failed_write_releases_no_actions_and_can_retry() {
    let initial = confirming();
    let proposal = initial.propose(binding(), SourceEvent::ReceiverReady(CHALLENGE)).unwrap();
    let mut driver = Durable::from_durable(initial.clone()).unwrap();
    let mut store = Store::new(initial.clone(), Fault::BeforeWrite);
    assert_eq!(driver.commit(proposal.clone(), &mut store), Err(Error::StorageNotCommitted));
    assert_eq!(driver.current().unwrap(), &initial);
    store.fault = Fault::None;
    assert_eq!(driver.commit(proposal, &mut store).unwrap().actions, vec![Action::EraseSource]);
}

#[test]
fn uncertain_commit_forces_reread_before_cancel_or_erase() {
    for (fault, committed) in [(Fault::LostReplyBeforeWrite, false), (Fault::LostReplyAfterSync, true)] {
        let initial = confirming();
        let proposal = initial.propose(binding(), SourceEvent::ReceiverReady(CHALLENGE)).unwrap();
        let cancel = initial.propose(binding(), SourceEvent::UserAbort).unwrap();
        let mut driver = Durable::from_durable(initial.clone()).unwrap();
        let mut store = Store::new(initial, fault);
        assert_eq!(driver.commit(proposal, &mut store), Err(Error::RereadRequired));
        assert_eq!(driver.current(), Err(Error::RereadRequired));
        assert_eq!(driver.commit(cancel, &mut store), Err(Error::RereadRequired));
        assert_eq!(store.writes, 1);
        let recovered = driver.reread(&mut store).unwrap();
        assert_eq!(recovered.actions, if committed { vec![Action::EraseSource] }
            else { vec![Action::RequestReceiverReady(CHALLENGE)] });
        if committed {
            assert_eq!(driver.current().unwrap().propose(binding(), SourceEvent::UserAbort).unwrap_err(), Error::InvalidTransition);
        }
    }
}

#[test]
fn uncertain_abort_never_resumes_source_before_readback() {
    for (fault, aborted) in [(Fault::LostReplyBeforeWrite, false), (Fault::LostReplyAfterSync, true)] {
        let initial = confirming();
        let proposal = initial.propose(binding(), SourceEvent::UserAbort).unwrap();
        let mut driver = Durable::from_durable(initial.clone()).unwrap();
        let mut store = Store::new(initial, fault);
        assert_eq!(driver.commit(proposal, &mut store), Err(Error::RereadRequired));
        assert_eq!(driver.current(), Err(Error::RereadRequired));
        let recovered = driver.reread(&mut store).unwrap();
        assert_eq!(driver.current().unwrap().allows_profile_use(), aborted);
        assert_eq!(recovered.actions.contains(&Action::ResumeSource), aborted);
    }
}

#[test]
fn prepared_receipt_waits_for_durable_stage_journal() {
    for (fault, prepared) in [(Fault::BeforeWrite, false), (Fault::LostReplyBeforeWrite, false), (Fault::LostReplyAfterSync, true)] {
        let initial = receiver();
        let proposal = initial.propose(binding(), ReceiverEvent::StagePrepared).unwrap();
        let mut driver = Durable::from_durable(initial.clone()).unwrap();
        let mut store = Store::new(initial, fault);
        assert!(driver.commit(proposal, &mut store).is_err());
        let actions = driver.reread(&mut store).unwrap().actions;
        assert_eq!(actions.contains(&Action::SendPrepared), prepared);
        assert!(!driver.current().unwrap().allows_profile_use());
    }
}

#[test]
fn lost_erasure_and_activation_replies_recover_without_early_activation() {
    let initial = decided();
    let proposal = initial.propose(binding(), SourceEvent::ErasureCompleted).unwrap();
    let mut driver = Durable::from_durable(initial.clone()).unwrap();
    let mut store = Store::new(initial, Fault::LostReplyAfterSync);
    assert_eq!(driver.commit(proposal, &mut store), Err(Error::RereadRequired));
    assert_eq!(driver.reread(&mut store).unwrap().actions, vec![Action::SendActivation]);
    // Simulate process loss: only the store survives.
    let restarted = Durable::from_durable(store.saved).unwrap();
    assert_eq!(restarted.current().unwrap().recovery_actions(), vec![Action::SendActivation]);

    let activating = prepared().propose(binding(), ReceiverEvent::SourceErased).unwrap().next;
    for (fault, active) in [(Fault::LostReplyBeforeWrite, false), (Fault::LostReplyAfterSync, true)] {
        let proposal = activating.propose(binding(), ReceiverEvent::ActivationCompleted).unwrap();
        let mut driver = Durable::from_durable(activating.clone()).unwrap();
        let mut store = Store::new(activating.clone(), fault);
        assert_eq!(driver.commit(proposal, &mut store), Err(Error::RereadRequired));
        driver.reread(&mut store).unwrap();
        assert_eq!(driver.current().unwrap().allows_profile_use(), active);
    }
}

#[test]
fn conflicting_owner_and_stale_proposal_cannot_overwrite_a_decision() {
    let initial = confirming();
    let commit = initial.propose(binding(), SourceEvent::ReceiverReady(CHALLENGE)).unwrap();
    let cancel = initial.propose(binding(), SourceEvent::UserAbort).unwrap();
    let mut driver = Durable::from_durable(initial.clone()).unwrap();
    let mut store = Store::new(initial, Fault::None);
    driver.commit(commit, &mut store).unwrap();
    assert_eq!(driver.commit(cancel, &mut store), Err(Error::StaleProposal));
    assert_eq!(store.writes, 1);
    let proposal = driver.current().unwrap().propose(binding(), SourceEvent::ErasureCompleted).unwrap();
    store.fault = Fault::Conflict;
    assert_eq!(driver.commit(proposal, &mut store), Err(Error::RereadRequired));
    assert_eq!(driver.current(), Err(Error::RereadRequired));
}

#[test]
fn readback_cannot_swap_binding_or_roll_back_confirmed_revision() {
    let mut driver = Durable::from_durable(decided()).unwrap();
    let mut store = Store::new(source(), Fault::None);
    assert_eq!(driver.reread(&mut store), Err(Error::InvalidJournal));
    assert_eq!(driver.current(), Err(Error::RereadRequired));
    let mut other = binding(); other.manifest_digest[0] ^= 1;
    store.saved = SourceJournal::frozen(other).unwrap();
    assert_eq!(driver.reread(&mut store), Err(Error::BindingMismatch));
    assert_eq!(driver.current(), Err(Error::RereadRequired));
}

#[test]
fn higher_revision_cannot_reverse_decision_or_replace_challenge() {
    let erased = advance_source(decided(), SourceEvent::ErasureCompleted);
    let mut invalid = serde_json::to_value(advance_source(source(), SourceEvent::UserAbort)).unwrap();
    invalid["revision"] = 50.into();
    let invalid: SourceJournal = serde_json::from_value(invalid).unwrap();
    let mut driver = Durable::from_durable(erased.clone()).unwrap();
    let mut store = Store::new(invalid, Fault::None);
    assert_eq!(driver.reread(&mut store), Err(Error::InvalidJournal));
    assert_eq!(driver.current(), Err(Error::RereadRequired));
    let mut invalid = serde_json::to_value(erased).unwrap();
    invalid["revision"] = 50.into();
    invalid["phase"]["Erased"] = serde_json::json!(vec![9u8; 32]);
    store.saved = serde_json::from_value(invalid).unwrap();
    assert_eq!(driver.reread(&mut store), Err(Error::InvalidJournal));

    let activating = prepared().propose(binding(), ReceiverEvent::SourceErased).unwrap().next;
    let mut invalid = serde_json::to_value(prepared()).unwrap();
    invalid["revision"] = 50.into();
    let mut store = Store::new(serde_json::from_value::<ReceiverJournal>(invalid).unwrap(), Fault::None);
    let mut driver = Durable::from_durable(activating).unwrap();
    assert_eq!(driver.reread(&mut store), Err(Error::InvalidJournal));
    assert_eq!(driver.current(), Err(Error::RereadRequired));
}

// Exhaust all reachable interleavings in this finite message abstraction.
// A queued authenticated message can be delayed, dropped or replayed; only a
// durable peer transition can produce it. No forged storage/crypto proof is
// manufactured by this explorer: those are adapter obligations, not a theorem.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct World { source: SourceJournal, receiver: ReceiverJournal, mail: u8, user_ok: bool, prepared: bool, erased: bool }
const PREPARED: u8 = 1;
const REQUEST: u8 = 2;
const READY: u8 = 4;
const ACTIVATE: u8 = 8;
const ABORT: u8 = 16;
const ACTIVE: u8 = 32;
fn queue(world: &mut World, actions: &[Action]) {
    for action in actions {
        world.mail |= match action {
            Action::SendPrepared => PREPARED,
            Action::RequestReceiverReady(_) => REQUEST,
            Action::SendReady(_) => READY,
            Action::SendActivation => ACTIVATE,
            Action::SendAbort => ABORT,
            Action::SendActivated => ACTIVE,
            _ => 0,
        };
    }
}
fn step_source(world: &World, event: SourceEvent) -> Option<World> {
    let proposal = world.source.propose(binding(), event).ok()?;
    let mut next = world.clone();
    if matches!(event, SourceEvent::UserOk(_)) { next.user_ok = true; }
    if event == SourceEvent::ErasureCompleted { next.erased = true; }
    next.source = proposal.next;
    queue(&mut next, &proposal.actions);
    Some(next)
}
fn step_receiver(world: &World, event: ReceiverEvent) -> Option<World> {
    let proposal = world.receiver.propose(binding(), event).ok()?;
    let mut next = world.clone();
    if event == ReceiverEvent::StagePrepared { next.prepared = true; }
    next.receiver = proposal.next;
    queue(&mut next, &proposal.actions);
    Some(next)
}

#[test]
fn exhaustive_message_interleavings_preserve_exclusive_activation() {
    let initial = World { source: source(), receiver: receiver(), mail: 0,
        user_ok: false, prepared: false, erased: false };
    let mut seen = HashSet::from([initial.clone()]);
    let mut work = VecDeque::from([initial]);
    let (mut completed, mut cancelled) = (false, false);
    while let Some(world) = work.pop_front() {
        assert!(!(world.source.allows_profile_use() && world.receiver.allows_profile_use()));
        if world.erased || matches!(world.source.phase(), SourcePhase::CommitDecided(_)) {
            assert!(world.user_ok && world.prepared);
        }
        if world.receiver.allows_profile_use() {
            assert!(world.erased);
            assert!(matches!(world.source.phase(), SourcePhase::Erased(_) | SourcePhase::Completed(_)));
        }
        completed |= matches!(world.source.phase(), SourcePhase::Completed(_)) && world.receiver.allows_profile_use();
        cancelled |= world.source.allows_profile_use() && world.receiver.phase() == ReceiverPhase::Aborted;
        let mut next = vec![];
        for event in [SourceEvent::UserOk(CHALLENGE), SourceEvent::UserAbort, SourceEvent::ErasureCompleted] {
            next.extend(step_source(&world, event));
        }
        for event in [ReceiverEvent::StagePrepared, ReceiverEvent::LocalAbort, ReceiverEvent::Timeout, ReceiverEvent::ActivationCompleted] {
            next.extend(step_receiver(&world, event));
        }
        for bit in [PREPARED, REQUEST, READY, ACTIVATE, ABORT, ACTIVE] {
            if world.mail & bit == 0 { continue; }
            let delivered = match bit {
                PREPARED => step_source(&world, SourceEvent::ReceiverPrepared),
                REQUEST => step_receiver(&world, ReceiverEvent::RequestReady(CHALLENGE)),
                READY => step_source(&world, SourceEvent::ReceiverReady(CHALLENGE)),
                ACTIVATE => step_receiver(&world, ReceiverEvent::SourceErased),
                ABORT => step_receiver(&world, ReceiverEvent::SourceAbort),
                ACTIVE => step_source(&world, SourceEvent::ReceiverActivated),
                _ => unreachable!(),
            };
            next.extend(delivered); // Retain the packet: later duplicate delivery.
            let mut dropped = world.clone(); dropped.mail &= !bit; next.push(dropped);
        }
        let mut recovery = world.clone();
        queue(&mut recovery, &world.source.recovery_actions());
        queue(&mut recovery, &world.receiver.recovery_actions());
        next.push(recovery);
        for value in next {
            if seen.insert(value.clone()) { work.push_back(value); }
        }
        assert!(seen.len() < 100_000, "unbounded model exploration");
    }
    assert!(seen.len() > 100, "exercise reordered and duplicated paths");
    assert!(completed && cancelled);
}
