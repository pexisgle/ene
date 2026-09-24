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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntakePremise {
    pub candidate: SubmitClientInputCandidate,
    pub attribution: PresenceAttribution,
    pub companion: CompanionAvailability,
    pub live: LiveReachabilityRef,
    pub open_round: Option<OpenRound>,
}

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
        RoundIntakeOutcome, RoundIntent, SubmitClientInputCandidate, check_intake, new_round,
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

    fn assert_stale(
        input: IntakePremise,
        current_round: Option<super::RoundId>,
        current_generation: PresenceGeneration,
    ) {
        assert_eq!(
            check_intake(input),
            RoundIntakeOutcome::StaleRound {
                current_round,
                current_generation,
            }
        );
    }

    #[test]
    fn availability_revalidation_and_transition_are_distinct() {
        let companion = RawId::new();
        let claimant = client();
        let generation = PresenceGeneration::first();
        let attribution = attribution_for(companion, claimant, generation);
        let live = live_for(claimant);
        let candidate = input_candidate(companion, claimant, Some(generation), RoundIntent::Auto);
        let cases = [
            (
                premise(
                    input_candidate(companion, claimant, None, RoundIntent::Auto),
                    attribution,
                    live,
                    None,
                ),
                RoundIntakeOutcome::NeedsRevalidation {
                    reason: RevalidationReason::MissingGenerationView,
                },
            ),
            (
                IntakePremise {
                    companion: CompanionAvailability::Unknown,
                    ..premise(candidate.clone(), attribution, live, None)
                },
                RoundIntakeOutcome::NeedsRevalidation {
                    reason: RevalidationReason::UnknownCompanion,
                },
            ),
            (
                IntakePremise {
                    companion: CompanionAvailability::Stopped,
                    ..premise(candidate.clone(), attribution, live, None)
                },
                RoundIntakeOutcome::NeedsRevalidation {
                    reason: RevalidationReason::StoppedCompanion,
                },
            ),
            (
                premise(
                    candidate.clone(),
                    PresenceAttribution {
                        state: PresenceState::Stopped,
                        ..attribution
                    },
                    live,
                    None,
                ),
                RoundIntakeOutcome::NeedsRevalidation {
                    reason: RevalidationReason::StoppedCompanion,
                },
            ),
            (
                premise(
                    candidate.clone(),
                    PresenceAttribution {
                        state: PresenceState::InTransition,
                        ..attribution
                    },
                    live,
                    None,
                ),
                RoundIntakeOutcome::HeldForTransition,
            ),
            (
                premise(
                    candidate,
                    PresenceAttribution {
                        state: PresenceState::RecoveryWait,
                        ..attribution
                    },
                    live,
                    None,
                ),
                RoundIntakeOutcome::HeldForTransition,
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(check_intake(input), expected);
        }
    }

    #[test]
    fn generation_client_liveness_and_round_mismatches_are_stale_with_current_values() {
        let companion = RawId::new();
        let claimant = client();
        let other_client = client();
        let generation = PresenceGeneration::first();
        let current_generation = PresenceGeneration::from_u64(7);
        let attribution = attribution_for(companion, claimant, generation);
        let candidate = input_candidate(companion, claimant, Some(generation), RoundIntent::Auto);

        assert_stale(
            premise(
                input_candidate(
                    companion,
                    claimant,
                    Some(PresenceGeneration::first()),
                    RoundIntent::Auto,
                ),
                attribution_for(companion, claimant, current_generation),
                live_for(claimant),
                None,
            ),
            None,
            current_generation,
        );
        assert_stale(
            premise(
                candidate.clone(),
                attribution_for(companion, other_client, generation),
                live_for(claimant),
                None,
            ),
            None,
            generation,
        );
        assert_stale(
            premise(
                candidate.clone(),
                attribution_for(companion, claimant, generation),
                live_for(other_client),
                None,
            ),
            None,
            generation,
        );
        assert_stale(
            premise(
                candidate.clone(),
                attribution_for(companion, claimant, generation),
                LiveReachabilityRef {
                    client: claimant,
                    connection_live: false,
                },
                None,
            ),
            None,
            generation,
        );

        let open = OpenRound {
            companion,
            client: claimant,
            round: new_round(),
            generation,
        };
        assert_stale(
            premise(
                input_candidate(
                    companion,
                    claimant,
                    Some(generation),
                    RoundIntent::Existing(new_round()),
                ),
                attribution,
                live_for(claimant),
                Some(open),
            ),
            Some(open.round),
            generation,
        );
    }

    #[test]
    fn round_intent_distinguishes_existing_auto_and_new() {
        let companion = RawId::new();
        let claimant = client();
        let generation = PresenceGeneration::first();
        let attribution = attribution_for(companion, claimant, generation);
        let open = OpenRound {
            companion,
            client: claimant,
            round: new_round(),
            generation,
        };

        assert_eq!(
            check_intake(premise(
                input_candidate(
                    companion,
                    claimant,
                    Some(generation),
                    RoundIntent::Existing(open.round),
                ),
                attribution,
                live_for(claimant),
                Some(open),
            )),
            RoundIntakeOutcome::AcceptedForRound { round: open.round }
        );
        assert_eq!(
            check_intake(premise(
                input_candidate(companion, claimant, Some(generation), RoundIntent::Auto,),
                attribution,
                live_for(claimant),
                Some(open),
            )),
            RoundIntakeOutcome::AcceptedForRound { round: open.round }
        );
        let RoundIntakeOutcome::AcceptedForRound { round } = check_intake(premise(
            input_candidate(companion, claimant, Some(generation), RoundIntent::New),
            attribution,
            live_for(claimant),
            Some(open),
        )) else {
            panic!("New must be accepted");
        };
        assert_ne!(round, open.round);

        let mut fresh = Vec::new();
        for _ in 0..2 {
            let RoundIntakeOutcome::AcceptedForRound { round } = check_intake(premise(
                input_candidate(companion, claimant, Some(generation), RoundIntent::Auto),
                attribution,
                live_for(claimant),
                None,
            )) else {
                panic!("Auto without an open round must mint one");
            };
            fresh.push(round);
        }
        assert_ne!(fresh[0], fresh[1]);
    }

    #[test]
    fn public_intake_debug_values_redact_the_input_body() {
        let secret = "private client input";
        let input = ClientInputRef {
            text: String::from(secret),
            lang: String::from("en"),
        };
        let candidate = SubmitClientInputCandidate {
            companion: RawId::new(),
            client: client(),
            claimed_generation: Some(PresenceGeneration::first()),
            round: RoundIntent::Existing(new_round()),
            input_ref: input.clone(),
        };
        let companion = candidate.companion;
        let claimant = candidate.client;
        let intake = premise(
            candidate.clone(),
            attribution_for(companion, claimant, PresenceGeneration::first()),
            live_for(claimant),
            None,
        );
        let candidate_debug = format!("{candidate:?}");
        assert!(!candidate_debug.contains(secret));
        assert!(candidate_debug.contains("en"));
        for rendered in [format!("{input:?}"), format!("{intake:?}")] {
            assert!(!rendered.contains(secret), "body redacted: {rendered}");
        }
    }
}
