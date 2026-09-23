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

/// Owner text input candidate arriving at the Host boundary.
///
/// A proposal, never an acceptance: attribution checks and round issuance
/// happen Host-side in [`check_intake`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SubmitClientInputCandidate {
    pub companion: RawId,
    pub client: ClientId,
    pub claimed_generation: Option<PresenceGeneration>,
    pub round: RoundIntent,
    pub input_ref: ClientInputRef,
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

/// Full intake premise evaluated by [`check_intake`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntakePremise {
    pub candidate: SubmitClientInputCandidate,
    pub attribution: PresenceAttribution,
    pub companion: CompanionAvailability,
    pub live: LiveReachabilityRef,
    pub open_round: Option<OpenRound>,
}

/// Opaque revalidation reason.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RevalidationReason {
    MissingGenerationView,
    UnknownCompanion,
    StoppedCompanion,
    MissingCommandId,
    InputOverLimit,
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

#[cfg(test)]
mod tests {
    use super::{
        ClientInputRef, CompanionAvailability, IntakePremise, OpenRound, RevalidationReason,
        RoundId, RoundIntakeOutcome, RoundIntent, SubmitClientInputCandidate, check_intake,
        new_round,
    };
    use ene_presence::{
        ClientId, LiveReachabilityRef, PresenceAttribution, PresenceGeneration, PresenceState,
    };
    use ene_primitive::RawId;

    fn client() -> ClientId {
        ClientId::from_raw(RawId::new())
    }

    fn input_candidate(
        companion: RawId,
        claimant: ClientId,
        generation: Option<PresenceGeneration>,
        round: RoundIntent,
    ) -> SubmitClientInputCandidate {
        SubmitClientInputCandidate {
            companion,
            client: claimant,
            claimed_generation: generation,
            round,
            input_ref: ClientInputRef {
                text: String::from("hello companion"),
                lang: String::from("en"),
            },
        }
    }

    fn premise(
        candidate: SubmitClientInputCandidate,
        attribution: PresenceAttribution,
        live: LiveReachabilityRef,
        open_round: Option<OpenRound>,
    ) -> IntakePremise {
        IntakePremise {
            candidate,
            attribution,
            companion: CompanionAvailability::Running,
            live,
            open_round,
        }
    }

    fn live_for(claimant: ClientId) -> LiveReachabilityRef {
        LiveReachabilityRef {
            client: claimant,
            connection_live: true,
        }
    }

    fn attribution_for(
        companion: RawId,
        claimant: ClientId,
        generation: PresenceGeneration,
    ) -> PresenceAttribution {
        PresenceAttribution {
            companion,
            state: PresenceState::Present,
            active_client: Some(claimant),
            generation,
        }
    }

    #[test]
    fn missing_generation_view_needs_revalidation() {
        let companion = RawId::new();
        let claimant = client();
        let candidate = input_candidate(companion, claimant, None, RoundIntent::Auto);
        let fact = attribution_for(companion, claimant, PresenceGeneration::first());
        let outcome = check_intake(premise(candidate, fact, live_for(claimant), None));
        assert_eq!(
            outcome,
            RoundIntakeOutcome::NeedsRevalidation {
                reason: RevalidationReason::MissingGenerationView,
            }
        );
    }

    #[test]
    fn unknown_companion_needs_revalidation() {
        let companion = RawId::new();
        let claimant = client();
        let candidate = input_candidate(
            companion,
            claimant,
            Some(PresenceGeneration::first()),
            RoundIntent::Auto,
        );
        let fact = attribution_for(companion, claimant, PresenceGeneration::first());
        let base = premise(candidate, fact, live_for(claimant), None);
        let unknown = IntakePremise {
            companion: CompanionAvailability::Unknown,
            ..base
        };
        let outcome = check_intake(unknown);
        assert_eq!(
            outcome,
            RoundIntakeOutcome::NeedsRevalidation {
                reason: RevalidationReason::UnknownCompanion,
            }
        );
    }

    #[test]
    fn stopped_companion_needs_revalidation() {
        let companion = RawId::new();
        let claimant = client();
        let candidate = input_candidate(
            companion,
            claimant,
            Some(PresenceGeneration::first()),
            RoundIntent::Auto,
        );
        let fact = attribution_for(companion, claimant, PresenceGeneration::first());
        let base = premise(candidate, fact, live_for(claimant), None);
        let stopped = IntakePremise {
            companion: CompanionAvailability::Stopped,
            ..base
        };
        let outcome = check_intake(stopped);
        assert_eq!(
            outcome,
            RoundIntakeOutcome::NeedsRevalidation {
                reason: RevalidationReason::StoppedCompanion,
            }
        );
    }

    #[test]
    fn stopped_attribution_needs_revalidation() {
        let companion = RawId::new();
        let claimant = client();
        let candidate = input_candidate(
            companion,
            claimant,
            Some(PresenceGeneration::first()),
            RoundIntent::Auto,
        );
        let fact = PresenceAttribution {
            companion,
            state: PresenceState::Stopped,
            active_client: Some(claimant),
            generation: PresenceGeneration::first(),
        };
        let outcome = check_intake(premise(candidate, fact, live_for(claimant), None));
        assert_eq!(
            outcome,
            RoundIntakeOutcome::NeedsRevalidation {
                reason: RevalidationReason::StoppedCompanion,
            }
        );
    }

    #[test]
    fn in_transition_holds_intake() {
        let companion = RawId::new();
        let claimant = client();
        let candidate = input_candidate(
            companion,
            claimant,
            Some(PresenceGeneration::first()),
            RoundIntent::Auto,
        );
        let fact = PresenceAttribution {
            companion,
            state: PresenceState::InTransition,
            active_client: Some(claimant),
            generation: PresenceGeneration::first(),
        };
        let outcome = check_intake(premise(candidate, fact, live_for(claimant), None));
        assert_eq!(outcome, RoundIntakeOutcome::HeldForTransition);
    }

    #[test]
    fn recovery_wait_holds_intake() {
        let companion = RawId::new();
        let claimant = client();
        let candidate = input_candidate(
            companion,
            claimant,
            Some(PresenceGeneration::first()),
            RoundIntent::Auto,
        );
        let fact = PresenceAttribution {
            companion,
            state: PresenceState::RecoveryWait,
            active_client: Some(claimant),
            generation: PresenceGeneration::first(),
        };
        let outcome = check_intake(premise(candidate, fact, live_for(claimant), None));
        assert_eq!(outcome, RoundIntakeOutcome::HeldForTransition);
    }

    #[test]
    fn generation_mismatch_is_stale_with_current_values() {
        let companion = RawId::new();
        let claimant = client();
        let current = PresenceGeneration::from_u64(7);
        let candidate = input_candidate(
            companion,
            claimant,
            Some(PresenceGeneration::first()),
            RoundIntent::Auto,
        );
        let fact = attribution_for(companion, claimant, current);
        let outcome = check_intake(premise(candidate, fact, live_for(claimant), None));
        assert_eq!(
            outcome,
            RoundIntakeOutcome::StaleRound {
                current_round: None,
                current_generation: current,
            }
        );
    }

    #[test]
    fn wrong_active_client_is_stale() {
        let companion = RawId::new();
        let claimant = client();
        let other = client();
        let candidate = input_candidate(
            companion,
            claimant,
            Some(PresenceGeneration::first()),
            RoundIntent::Auto,
        );
        let fact = attribution_for(companion, other, PresenceGeneration::first());
        let outcome = check_intake(premise(candidate, fact, live_for(claimant), None));
        let RoundIntakeOutcome::StaleRound {
            current_generation, ..
        } = outcome
        else {
            panic!("a non-active client must be stale");
        };
        assert_eq!(current_generation, PresenceGeneration::first());
    }

    #[test]
    fn dead_connection_is_stale() {
        let companion = RawId::new();
        let claimant = client();
        let candidate = input_candidate(
            companion,
            claimant,
            Some(PresenceGeneration::first()),
            RoundIntent::Auto,
        );
        let fact = attribution_for(companion, claimant, PresenceGeneration::first());
        let dead = LiveReachabilityRef {
            client: claimant,
            connection_live: false,
        };
        let outcome = check_intake(premise(candidate, fact, dead, None));
        assert!(matches!(outcome, RoundIntakeOutcome::StaleRound { .. }));
    }

    #[test]
    fn mismatched_round_request_is_stale() {
        let companion = RawId::new();
        let claimant = client();
        let open = OpenRound {
            companion,
            client: claimant,
            round: new_round(),
            generation: PresenceGeneration::first(),
        };
        let candidate = input_candidate(
            companion,
            claimant,
            Some(PresenceGeneration::first()),
            RoundIntent::Existing(new_round()),
        );
        let fact = attribution_for(companion, claimant, PresenceGeneration::first());
        let outcome = check_intake(premise(candidate, fact, live_for(claimant), Some(open)));
        let RoundIntakeOutcome::StaleRound {
            current_round,
            current_generation,
        } = outcome
        else {
            panic!("a non-matching existing-round request must be stale");
        };
        assert_eq!(current_round, Some(open.round));
        assert_eq!(current_generation, PresenceGeneration::first());
    }

    #[test]
    fn matching_open_round_is_accepted() {
        let companion = RawId::new();
        let claimant = client();
        let open = OpenRound {
            companion,
            client: claimant,
            round: new_round(),
            generation: PresenceGeneration::first(),
        };
        let candidate = input_candidate(
            companion,
            claimant,
            Some(PresenceGeneration::first()),
            RoundIntent::Existing(open.round),
        );
        let fact = attribution_for(companion, claimant, PresenceGeneration::first());
        let outcome = check_intake(premise(candidate, fact, live_for(claimant), Some(open)));
        assert_eq!(
            outcome,
            RoundIntakeOutcome::AcceptedForRound { round: open.round }
        );
    }

    #[test]
    fn auto_request_reuses_matching_open_round() {
        let companion = RawId::new();
        let claimant = client();
        let open = OpenRound {
            companion,
            client: claimant,
            round: new_round(),
            generation: PresenceGeneration::first(),
        };
        let candidate = input_candidate(
            companion,
            claimant,
            Some(PresenceGeneration::first()),
            RoundIntent::Auto,
        );
        let fact = attribution_for(companion, claimant, PresenceGeneration::first());
        let outcome = check_intake(premise(candidate, fact, live_for(claimant), Some(open)));
        assert_eq!(
            outcome,
            RoundIntakeOutcome::AcceptedForRound { round: open.round }
        );
    }

    #[test]
    fn auto_request_without_open_round_mints_fresh_round() {
        let companion = RawId::new();
        let claimant = client();
        let candidate = input_candidate(
            companion,
            claimant,
            Some(PresenceGeneration::first()),
            RoundIntent::Auto,
        );
        let fact = attribution_for(companion, claimant, PresenceGeneration::first());
        let outcome = check_intake(premise(candidate, fact, live_for(claimant), None));
        assert!(matches!(
            outcome,
            RoundIntakeOutcome::AcceptedForRound { .. }
        ));
        if let RoundIntakeOutcome::AcceptedForRound { round } = outcome {
            assert_ne!(round, RoundId::from_raw(RawId::new()));
        }
    }

    #[test]
    fn new_request_mints_fresh_round_despite_matching_open() {
        let companion = RawId::new();
        let claimant = client();
        let open = OpenRound {
            companion,
            client: claimant,
            round: new_round(),
            generation: PresenceGeneration::first(),
        };
        let candidate = input_candidate(
            companion,
            claimant,
            Some(PresenceGeneration::first()),
            RoundIntent::New,
        );
        let fact = attribution_for(companion, claimant, PresenceGeneration::first());
        let outcome = check_intake(premise(candidate, fact, live_for(claimant), Some(open)));
        assert!(
            matches!(
                outcome,
                RoundIntakeOutcome::AcceptedForRound { round } if round != open.round
            ),
            "New must mint instead of joining, got {outcome:?}"
        );
    }

    #[test]
    fn new_rounds_are_unique() {
        assert_ne!(new_round(), new_round());
    }

    #[test]
    fn input_debug_redacts_body_and_keeps_refs() {
        let candidate = SubmitClientInputCandidate {
            companion: RawId::new(),
            client: client(),
            claimed_generation: Some(PresenceGeneration::first()),
            round: RoundIntent::Existing(new_round()),
            input_ref: ClientInputRef {
                text: String::from("hello companion"),
                lang: String::from("en"),
            },
        };
        let rendered = format!("{candidate:?}");
        assert!(
            !rendered.contains("hello companion"),
            "body redacted: {rendered}"
        );
        assert!(rendered.contains("en"), "lang stays: {rendered}");
    }

    #[test]
    fn premise_debug_redacts_body() {
        let companion = RawId::new();
        let claimant = client();
        let candidate = input_candidate(
            companion,
            claimant,
            Some(PresenceGeneration::first()),
            RoundIntent::Auto,
        );
        let fact = attribution_for(companion, claimant, PresenceGeneration::first());
        let rendered = format!("{:?}", premise(candidate, fact, live_for(claimant), None));
        assert!(
            !rendered.contains("hello companion"),
            "body redacted: {rendered}"
        );
    }
}
