use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SourcePhase {
    Frozen,
    AwaitingConfirmation,
    Confirming(Challenge),
    CommitDecided(Challenge),
    Erased(Challenge),
    Completed(Challenge),
    Aborted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceEvent {
    /// Adapter verified the pinned receiver's authenticated durable Prepared.
    ReceiverPrepared,
    /// A local user selected OK. Generate this nonzero nonce with a CSPRNG;
    /// persist it before transmitting RequestReceiverReady. Reuse it on retry.
    UserOk(Challenge),
    /// Authenticated response to this attempt's persisted fresh challenge.
    ReceiverReady(Challenge),
    UserAbort,
    /// Strict adapter verified deletion, including profile keys and scratch.
    ErasureCompleted,
    /// Authenticated receipt of the receiver's durable activation.
    ReceiverActivated,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceJournal {
    binding: Binding,
    revision: u64,
    phase: SourcePhase,
}

impl SourceJournal {
    /// The caller has already frozen all profile mutation/admission.
    pub fn frozen(binding: Binding) -> Result<Self, Error> {
        binding.validate()?;
        Ok(Self { binding, revision: 0, phase: SourcePhase::Frozen })
    }

    pub fn phase(&self) -> SourcePhase { self.phase }
    pub fn allows_profile_use(&self) -> bool { self.phase == SourcePhase::Aborted }

    pub fn propose(&self, binding: Binding, event: SourceEvent) -> Result<Proposal<Self>, Error> {
        check_binding(self.binding, binding)?;
        self.validate()?;
        use SourceEvent as E;
        use SourcePhase as P;
        let phase = match (self.phase, event) {
            (P::Frozen, E::ReceiverPrepared) => P::AwaitingConfirmation,
            (_, E::ReceiverPrepared) => self.phase,
            (P::AwaitingConfirmation, E::UserOk(challenge)) => {
                check_challenge(challenge)?;
                P::Confirming(challenge)
            }
            (P::Confirming(saved) | P::CommitDecided(saved) | P::Erased(saved) | P::Completed(saved), E::UserOk(challenge)) => {
                check_challenge(challenge)?;
                if challenge != saved { return Err(Error::StaleChallenge); }
                self.phase
            }
            (P::Confirming(saved), E::ReceiverReady(challenge)) => {
                check_challenge(challenge)?;
                if challenge != saved { return Err(Error::StaleChallenge); }
                P::CommitDecided(saved)
            }
            (P::CommitDecided(saved) | P::Erased(saved) | P::Completed(saved), E::ReceiverReady(challenge)) => {
                check_challenge(challenge)?;
                if challenge != saved { return Err(Error::StaleChallenge); }
                self.phase
            }
            (P::Frozen | P::AwaitingConfirmation | P::Confirming(_) | P::Aborted, E::UserAbort) => P::Aborted,
            (P::CommitDecided(challenge), E::ErasureCompleted) => P::Erased(challenge),
            (P::Erased(_) | P::Completed(_), E::ErasureCompleted) => self.phase,
            (P::Erased(challenge), E::ReceiverActivated) => P::Completed(challenge),
            (P::Completed(_), E::ReceiverActivated) => self.phase,
            _ => return Err(Error::InvalidTransition),
        };
        let mut next = self.clone();
        if phase != self.phase {
            next.revision = self.revision.checked_add(1).ok_or(Error::RevisionOverflow)?;
            next.phase = phase;
        }
        let actions = next.recovery_actions();
        Ok(Proposal { before: self.clone(), next, actions })
    }
}

impl Journal for SourceJournal {
    fn binding(&self) -> Binding { self.binding }
    fn revision(&self) -> u64 { self.revision }
    fn can_follow(&self, previous: &Self) -> bool {
        use SourcePhase as P;
        if self.binding != previous.binding { return false; }
        match (previous.phase, self.phase) {
            (P::Frozen, _) => true,
            (P::AwaitingConfirmation, next) => next != P::Frozen,
            (P::Confirming(_), P::Aborted) => true,
            (P::Confirming(old), P::Confirming(new) | P::CommitDecided(new) | P::Erased(new) | P::Completed(new))
            | (P::CommitDecided(old), P::CommitDecided(new) | P::Erased(new) | P::Completed(new))
            | (P::Erased(old), P::Erased(new) | P::Completed(new))
            | (P::Completed(old), P::Completed(new)) => old == new,
            (P::Aborted, P::Aborted) => true,
            _ => false,
        }
    }
    fn validate(&self) -> Result<(), Error> {
        self.binding.validate()?;
        let minimum = match self.phase {
            SourcePhase::Frozen => 0,
            SourcePhase::AwaitingConfirmation | SourcePhase::Aborted => 1,
            SourcePhase::Confirming(_) => 2,
            SourcePhase::CommitDecided(_) => 3,
            SourcePhase::Erased(_) => 4,
            SourcePhase::Completed(_) => 5,
        };
        if self.revision < minimum || (self.phase == SourcePhase::Frozen && self.revision != 0) {
            return Err(Error::InvalidJournal);
        }
        match self.phase {
            SourcePhase::Confirming(c) | SourcePhase::CommitDecided(c)
                | SourcePhase::Erased(c) | SourcePhase::Completed(c) => check_challenge(c),
            _ => Ok(()),
        }
    }
    fn recovery_actions(&self) -> Vec<Action> {
        use SourcePhase as P;
        match self.phase {
            P::Frozen => vec![],
            P::AwaitingConfirmation => vec![Action::ShowSourceConfirmation],
            P::Confirming(challenge) => vec![Action::RequestReceiverReady(challenge)],
            P::CommitDecided(_) => vec![Action::EraseSource],
            P::Erased(_) => vec![Action::SendActivation],
            P::Completed(_) => vec![Action::FinishSource],
            P::Aborted => vec![Action::SendAbort, Action::ResumeSource],
        }
    }
}
