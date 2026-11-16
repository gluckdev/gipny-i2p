use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReceiverPhase { Receiving, Prepared, Activating, Active, Aborted }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiverEvent {
    /// Adapter validated the complete immutable staging and its durable readback.
    StagePrepared,
    /// Authenticated challenge from the bound source, after its local user OK.
    RequestReady(Challenge),
    /// Authenticated source certificate authorized only after strict erasure.
    SourceErased,
    /// Receiver activation storage work finished durably; admission stays closed
    /// until the resulting Active journal also commits.
    ActivationCompleted,
    SourceAbort,
    LocalAbort,
    Timeout,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiverJournal {
    binding: Binding,
    revision: u64,
    phase: ReceiverPhase,
}

impl ReceiverJournal {
    pub fn receiving(binding: Binding) -> Result<Self, Error> {
        binding.validate()?;
        Ok(Self { binding, revision: 0, phase: ReceiverPhase::Receiving })
    }
    pub fn phase(&self) -> ReceiverPhase { self.phase }
    pub fn allows_profile_use(&self) -> bool { self.phase == ReceiverPhase::Active }

    pub fn propose(&self, binding: Binding, event: ReceiverEvent) -> Result<Proposal<Self>, Error> {
        check_binding(self.binding, binding)?;
        self.validate()?;
        use ReceiverEvent as E;
        use ReceiverPhase as P;
        let phase = match (self.phase, event) {
            (P::Receiving | P::Prepared, E::StagePrepared) => P::Prepared,
            (P::Prepared, E::RequestReady(challenge)) => {
                check_challenge(challenge)?;
                return Ok(Proposal { before: self.clone(), next: self.clone(),
                    actions: vec![Action::SendReady(challenge)] });
            }
            (P::Prepared, E::SourceErased) => P::Activating,
            (P::Activating | P::Active, E::SourceErased) => self.phase,
            (P::Activating | P::Active, E::ActivationCompleted) => P::Active,
            (P::Receiving | P::Prepared | P::Aborted, E::SourceAbort) => P::Aborted,
            (P::Receiving | P::Aborted, E::LocalAbort | E::Timeout) => P::Aborted,
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

impl Journal for ReceiverJournal {
    fn binding(&self) -> Binding { self.binding }
    fn revision(&self) -> u64 { self.revision }
    fn can_follow(&self, previous: &Self) -> bool {
        use ReceiverPhase as P;
        if self.binding != previous.binding { return false; }
        matches!((previous.phase, self.phase),
            (P::Receiving, _)
            | (P::Prepared, P::Prepared | P::Activating | P::Active | P::Aborted)
            | (P::Activating, P::Activating | P::Active)
            | (P::Active, P::Active)
            | (P::Aborted, P::Aborted))
    }
    fn validate(&self) -> Result<(), Error> {
        self.binding.validate()?;
        let minimum = match self.phase {
            ReceiverPhase::Receiving => 0,
            ReceiverPhase::Prepared | ReceiverPhase::Aborted => 1,
            ReceiverPhase::Activating => 2,
            ReceiverPhase::Active => 3,
        };
        if self.revision < minimum || (self.phase == ReceiverPhase::Receiving && self.revision != 0) {
            return Err(Error::InvalidJournal);
        }
        Ok(())
    }
    fn recovery_actions(&self) -> Vec<Action> {
        match self.phase {
            ReceiverPhase::Receiving => vec![],
            ReceiverPhase::Prepared => vec![Action::SendPrepared],
            ReceiverPhase::Activating => vec![Action::ActivateReceiver],
            ReceiverPhase::Active => vec![Action::EnableReceiver, Action::SendActivated],
            ReceiverPhase::Aborted => vec![Action::DiscardQuarantine],
        }
    }
}
