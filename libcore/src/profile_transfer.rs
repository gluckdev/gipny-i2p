//! Proposed profile-transfer journals, without network, cryptography or I/O.
//!
//! Authenticated events and durable storage facts must be checked by future
//! adapters. A transition only proposes a journal and its subsequent actions.
//! [`Durable`] releases actions after the adapter reports an atomic, synced
//! compare-and-write. This model does not prove that an adapter actually did it.

mod receiver;
mod source;
pub mod portable;

pub use receiver::{ReceiverEvent, ReceiverJournal, ReceiverPhase};
pub use source::{SourceEvent, SourceJournal, SourcePhase};

use serde::{Deserialize, Serialize};

pub type Challenge = [u8; 32];

/// Immutable identity of one transfer and its already pinned recipient.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub transfer_id: [u8; 16],
    pub recipient_key: [u8; 32],
    pub manifest_digest: [u8; 32],
}

impl Binding {
    pub fn validate(&self) -> Result<(), Error> {
        if self.transfer_id == [0; 16]
            || self.recipient_key == [0; 32]
            || self.manifest_digest == [0; 32]
        {
            return Err(Error::InvalidBinding);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidBinding,
    BindingMismatch,
    InvalidChallenge,
    StaleChallenge,
    InvalidTransition,
    RevisionOverflow,
    StaleProposal,
    StorageNotCommitted,
    RereadRequired,
    InvalidJournal,
}

/// Instructions for a single owned, idempotent executor. They do no work here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    ShowSourceConfirmation,
    RequestReceiverReady(Challenge),
    EraseSource,
    SendActivation,
    SendAbort,
    ResumeSource,
    FinishSource,
    SendPrepared,
    SendReady(Challenge),
    ActivateReceiver,
    EnableReceiver,
    SendActivated,
    DiscardQuarantine,
}

/// The action binding is carried explicitly to prevent use for another attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedActions {
    pub binding: Binding,
    pub revision: u64,
    pub actions: Vec<Action>,
}

pub trait Journal: Clone + PartialEq {
    fn binding(&self) -> Binding;
    fn revision(&self) -> u64;
    fn validate(&self) -> Result<(), Error>;
    /// Recovery may skip intermediate committed revisions, but never reverse
    /// a decision or substitute a different confirmation challenge.
    fn can_follow(&self, previous: &Self) -> bool;
    /// Safe repeats after the current durable journal was read back.
    fn recovery_actions(&self) -> Vec<Action>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal<J> {
    before: J,
    next: J,
    actions: Vec<Action>,
}

impl<J> Proposal<J> {
    pub fn next_journal(&self) -> &J { &self.next }
}

/// `NotCommitted` guarantees that the expected journal is still current.
/// A timeout, cancelled write, partial write or lost response is `Unknown`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreOutcome {
    Committed,
    NotCommitted,
    Conflict,
    Unknown,
}

/// Future adapters must hold the same owner lock used by profile admission.
/// CAS and persistence are one operation; a successful write alone is not sync.
pub trait JournalStore<J> {
    fn compare_and_sync(&mut self, expected: &J, next: &J) -> StoreOutcome;
    /// Missing/corrupt/unreadable state must return an error, never a fresh state.
    fn read_current(&mut self) -> Result<J, Error>;
}

pub struct Durable<J> {
    binding: Binding,
    known: Option<J>,
    last_confirmed: J,
}

impl<J: Journal> Durable<J> {
    /// The initial Frozen/Receiving journal must already have been persisted.
    pub fn from_durable(journal: J) -> Result<Self, Error> {
        journal.validate()?;
        Ok(Self { binding: journal.binding(), known: Some(journal.clone()), last_confirmed: journal })
    }

    pub fn current(&self) -> Result<&J, Error> {
        self.known.as_ref().ok_or(Error::RereadRequired)
    }

    pub fn commit(&mut self, proposal: Proposal<J>, store: &mut impl JournalStore<J>)
        -> Result<CommittedActions, Error>
    {
        let current = self.current()?;
        if current != &proposal.before { return Err(Error::StaleProposal); }
        proposal.next.validate()?;
        if proposal.next.binding() != self.binding { return Err(Error::BindingMismatch); }
        match store.compare_and_sync(current, &proposal.next) {
            StoreOutcome::Committed => {
                let result = CommittedActions { binding: self.binding,
                    revision: proposal.next.revision(), actions: proposal.actions };
                self.last_confirmed = proposal.next.clone();
                self.known = Some(proposal.next);
                Ok(result)
            }
            StoreOutcome::NotCommitted => Err(Error::StorageNotCommitted),
            StoreOutcome::Conflict | StoreOutcome::Unknown => {
                self.known = None;
                Err(Error::RereadRequired)
            }
        }
    }

    /// Readback is mandatory after an ambiguous result. It also supplies actions
    /// lost in a crash after commit and before their executor received them.
    pub fn reread(&mut self, store: &mut impl JournalStore<J>) -> Result<CommittedActions, Error> {
        self.known = None;
        let journal = store.read_current()?;
        journal.validate()?;
        if journal.binding() != self.binding { return Err(Error::BindingMismatch); }
        if !journal.can_follow(&self.last_confirmed)
            || journal.revision() < self.last_confirmed.revision()
            || (journal.revision() == self.last_confirmed.revision() && journal != self.last_confirmed)
        {
            return Err(Error::InvalidJournal);
        }
        let result = CommittedActions { binding: self.binding,
            revision: journal.revision(), actions: journal.recovery_actions() };
        self.last_confirmed = journal.clone();
        self.known = Some(journal);
        Ok(result)
    }
}

pub(super) fn check_binding(expected: Binding, actual: Binding) -> Result<(), Error> {
    actual.validate()?;
    if actual == expected { Ok(()) } else { Err(Error::BindingMismatch) }
}

pub(super) fn check_challenge(challenge: Challenge) -> Result<(), Error> {
    if challenge == [0; 32] { Err(Error::InvalidChallenge) } else { Ok(()) }
}

#[cfg(test)]
mod tests;
