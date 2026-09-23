use ene_presence::{
    ClientId, LiveReachabilityRef, PresenceAttribution, PresenceGeneration, PresenceState,
};
use ene_primitive::RawId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RoundId(RawId);

impl RoundId {
    #[must_use]
    pub fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }
}

#[must_use]
fn new_round() -> RoundId {
    RoundId(RawId::new())
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ClientInputRef {
    pub text: String,
    pub lang: String,
}

impl core::fmt::Debug for ClientInputRef {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ClientInputRef")
            .field("text", &"[redacted]")
            .field("lang", &self.lang)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SubmitClientInputCandidate {
    pub companion: RawId,
    pub client: ClientId,
    pub claimed_generation: Option<PresenceGeneration>,
    pub round: RoundIntent,
    pub input_ref: ClientInputRef,
    pub local_id: String,
}

impl core::fmt::Debug for SubmitClientInputCandidate {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("SubmitClientInputCandidate")
            .field("companion", &self.companion)
            .field("client", &self.client)
            .field("claimed_generation", &self.claimed_generation)
            .field("round", &self.round)
            .field("input_ref", &self.input_ref)
            .field("local_id", &self.local_id)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoundIntent {
    Auto,
    New,
    Existing(RoundId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompanionAvailability {
    Unknown,
    Stopped,
    Running,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OpenRound {
    pub companion: RawId,
    pub client: ClientId,
    pub round: RoundId,
    pub generation: PresenceGeneration,
}

#[derive(Clone, PartialEq, Eq)]
pub struct IntakePremise {
    pub candidate: SubmitClientInputCandidate,
    pub attribution: PresenceAttribution,
    pub companion: CompanionAvailability,
    pub live: LiveReachabilityRef,
    pub open_round: Option<OpenRound>,
}

impl core::fmt::Debug for IntakePremise {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("IntakePremise")
            .field("candidate", &self.candidate)
            .field("attribution", &self.attribution)
            .field("companion", &self.companion)
            .field("live", &self.live)
            .field("open_round", &self.open_round)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RevalidationReason {
    MissingGenerationView,
    UnknownCompanion,
    StoppedCompanion,
    MissingCommandId,
    InputOverLimit,
    UnknownReasonTag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoundIntakeOutcome {
    AcceptedForRound {
        round: RoundId,
    },
    StaleRound {
        current_round: Option<RoundId>,
        current_generation: PresenceGeneration,
    },
    HeldForTransition,
    NeedsRevalidation {
        reason: RevalidationReason,
    },
}

#[must_use]
pub fn check_intake(premise: IntakePremise) -> RoundIntakeOutcome {
    let IntakePremise {
        candidate,
        attribution,
        companion,
        live,
        open_round,
    } = premise;
    let Some(claimed) = candidate.claimed_generation else {
        return RoundIntakeOutcome::NeedsRevalidation {
            reason: RevalidationReason::MissingGenerationView,
        };
    };
    match companion {
        CompanionAvailability::Unknown => {
            return RoundIntakeOutcome::NeedsRevalidation {
                reason: RevalidationReason::UnknownCompanion,
            };
        }
        CompanionAvailability::Stopped => {
            return RoundIntakeOutcome::NeedsRevalidation {
                reason: RevalidationReason::StoppedCompanion,
            };
        }
        CompanionAvailability::Running => {}
    }
    if attribution.state == PresenceState::Stopped {
        return RoundIntakeOutcome::NeedsRevalidation {
            reason: RevalidationReason::StoppedCompanion,
        };
    }
    if attribution.state == PresenceState::InTransition
        || attribution.state == PresenceState::RecoveryWait
    {
        return RoundIntakeOutcome::HeldForTransition;
    }
    let matching_open = open_round.filter(|open| {
        open.companion == candidate.companion
            && open.client == candidate.client
            && open.generation == attribution.generation
    });
    let stale = || RoundIntakeOutcome::StaleRound {
        current_round: matching_open.map(|open| open.round),
        current_generation: attribution.generation,
    };
    if candidate.companion != attribution.companion {
        return stale();
    }
    if claimed != attribution.generation {
        return stale();
    }
    if attribution.active_client != Some(candidate.client) {
        return stale();
    }
    if live.client != candidate.client || !live.connection_live {
        return stale();
    }
    match candidate.round {
        RoundIntent::Existing(requested) => {
            if matching_open.is_some_and(|open| open.round == requested) {
                RoundIntakeOutcome::AcceptedForRound { round: requested }
            } else {
                stale()
            }
        }
        RoundIntent::Auto => match matching_open {
            Some(open) => RoundIntakeOutcome::AcceptedForRound { round: open.round },
            None => RoundIntakeOutcome::AcceptedForRound { round: new_round() },
        },
        RoundIntent::New => RoundIntakeOutcome::AcceptedForRound { round: new_round() },
    }
}
