//! Client input intake checks: pure premise evaluation, no durability.
//!
//! This crate owns no repository trait and no error type because it owns
//! nothing durable. Intake evaluates an [`IntakePremise`] into a
//! [`RoundIntakeOutcome`] with [`check_intake`]; round issuance authority and
//! durable state live with the caller and the store.
//!
//! Companion identity crosses this crate as [`RawId`]:
//! there is no dependency on `ene-companion`. [`ClientId`],
//! [`PresenceGeneration`], [`PresenceAttribution`], [`LiveReachabilityRef`],
//! and [`PresenceState`] are imported from `ene-presence`.
//!
//! Wire mapping (read-only): [`SubmitClientInputCandidate`] maps from
//! `ene_api::v1::round::SubmitTextInput` plus the envelope
//! `presence_generation_view` / `round_view` at Host ingress, and
//! [`RoundIntakeOutcome`] maps to
//! `ene_api::v1::round::RoundIntakeOutcomeWire`. The wire
//! `ene_api::v1::round::PresentationStatus` `Failed`
//! variant maps to [`PresentationStatus::PresentationUnknown`] at ingress:
//! failure is observed as unknown and sticky, never upgraded by resend.
//!
//! Minting versus acceptance: calling [`new_round`] mints a fresh [`RoundId`]
//! but accepts nothing. [`check_intake`] accepts; on an [`RoundIntent::Auto`]
//! request that passes all checks with no matching [`OpenRound`], it mints
//! a fresh [`RoundId`] inside and returns it as accepted, as it always does
//! for [`RoundIntent::New`]. Minting is not authority, acceptance is: the
//! caller still records the returned round as the open round. On an
//! [`RoundIntent::Auto`] request with a matching [`OpenRound`] for the same
//! companion, client, and generation, the open round is returned.

use ene_presence::{
    ClientId, LiveReachabilityRef, PresenceAttribution, PresenceGeneration, PresenceState,
};
use ene_primitive::RawId;

/// Host-issued round identity.
///
/// Wraps a [`RawId`]; old rounds are never rebound to
/// new ones. No conversion exists to any other domain newtype.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RoundId(RawId);

impl RoundId {
    /// Wraps an existing raw identity, for example one read back from storage.
    #[must_use]
    pub fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    /// Returns the wrapped raw identity for storage or transport encoding.
    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }
}

/// Mints a fresh Host-issued [`RoundId`].
///
/// Minting alone accepts nothing; acceptance is decided by [`check_intake`]
/// and recorded by the caller.
#[must_use]
pub fn new_round() -> RoundId {
    RoundId(RawId::new())
}

/// Owner text input reference: transient expression body plus language tag.
///
/// The Host canonicalizes accepted text into History; this ref itself is
/// never durable.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ClientInputRef {
    /// Body text. Redacted from [`core::fmt::Debug`].
    pub text: String,
    /// Opaque language tag.
    pub lang: String,
}

impl core::fmt::Debug for ClientInputRef {
    /// Renders `lang` while redacting `text`.
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
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SubmitClientInputCandidate {
    /// Companion the candidate targets.
    pub companion: RawId,
    /// Claiming [`ClientId`].
    pub client: ClientId,
    /// Generation value the Client saw, if any. [`None`] means the
    /// generation token was missing.
    pub claimed_generation: Option<PresenceGeneration>,
    /// Which round the input wants: join-or-mint, always-mint, or one
    /// specific open round. A single meaning per value — never an
    /// `Option` doing double duty.
    pub round: RoundIntent,
    /// Input body reference.
    pub input_ref: ClientInputRef,
    /// Client-local correspondence ID for matching acks to sends.
    pub local_id: String,
}

impl core::fmt::Debug for SubmitClientInputCandidate {
    /// Renders refs and the redacted input while redacting body text.
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

/// Which round an intake candidate wants to join.
///
/// One value, one meaning: unlike the retired `Option<RoundId>` (where
/// `None` meant both "join the open round" and "mint a fresh one"), each
/// variant names exactly one intention, so replay fingerprints and round
/// projections built downstream rest on a single meaning source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoundIntent {
    /// Join the matching open round; mint a fresh one when none matches.
    Auto,
    /// Always mint a fresh round, even when an open round would match.
    New,
    /// Join this specific round; stale unless it is the matching open round.
    Existing(RoundId),
}

/// Companion availability premise supplied Host-side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CompanionAvailability {
    /// Whether the companion is known.
    pub known: bool,
    /// Whether the companion is running.
    pub running: bool,
}

/// Currently open round premise for round binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OpenRound {
    /// Companion the open round belongs to.
    pub companion: RawId,
    /// [`ClientId`] the open round belongs to.
    pub client: ClientId,
    /// Open [`RoundId`].
    pub round: RoundId,
    /// Generation the open round belongs to.
    pub generation: PresenceGeneration,
}

/// Full intake premise evaluated by [`check_intake`].
#[derive(Clone, PartialEq, Eq)]
pub struct IntakePremise {
    /// Input candidate under evaluation.
    pub candidate: SubmitClientInputCandidate,
    /// Current authoritative [`PresenceAttribution`].
    pub attribution: PresenceAttribution,
    /// Companion availability premise.
    pub companion: CompanionAvailability,
    /// Out-of-band liveness premise.
    pub live: LiveReachabilityRef,
    /// Currently open round, if one is open.
    pub open_round: Option<OpenRound>,
}

impl core::fmt::Debug for IntakePremise {
    /// Renders the premise with body text redacted via the candidate.
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

/// Opaque revalidation reason matched at Host ingress.
///
/// Unknown wire values map to [`RevalidationReason::UnknownReasonTag`];
/// [`check_intake`] itself never emits that variant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RevalidationReason {
    /// The generation view token was missing.
    MissingGenerationView,
    /// The companion is unknown.
    UnknownCompanion,
    /// The companion is stopped.
    StoppedCompanion,
    /// The command carried no idempotency key. Replay safety needs one, so
    /// keyless commands are declined rather than accepted unkeyed.
    MissingCommandId,
    /// The command id arrived with different content than the stored row:
    /// a reused key must never adopt new meaning.
    CommandMismatch,
    /// The wire reason tag matched no known reason. Ingress-only; never
    /// emitted by [`check_intake`].
    UnknownReasonTag,
}

/// Intake outcome: an `Ok`-side domain outcome, never an error.
///
/// An old round maps back to its own round; nothing is rebound onto a new
/// one, and a rejected request is never auto-resent to work around a
/// rejection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoundIntakeOutcome {
    /// Accepted into this Host-issued round.
    AcceptedForRound {
        /// Round the input joined.
        round: RoundId,
    },
    /// The premise round is not current.
    StaleRound {
        /// Current round, if one is open.
        current_round: Option<RoundId>,
        /// Current generation value the sender should observe next time.
        current_generation: PresenceGeneration,
    },
    /// A presence transition holds intake for now.
    HeldForTransition,
    /// The premise needs refreshing before intake.
    NeedsRevalidation {
        /// Reason the premise needs refreshing.
        reason: RevalidationReason,
    },
}

/// Presentation status vocabulary for observations.
///
/// The wire `Failed` status maps to
/// [`PresentationStatus::PresentationUnknown`] at Host ingress: a failed
/// presentation is observed as unknown, sticky, and never upgraded by
/// resend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PresentationStatus {
    /// Presented.
    Presented,
    /// Presentation unknown. Sticky: never upgraded by resend.
    PresentationUnknown,
}

/// Presentation confirmation: an observation, not a report of completion.
///
/// Sending never equals reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConfirmPresentationObservation {
    /// Round the confirmation covers.
    pub round: RoundId,
    /// Observed presentation status.
    pub presented_or_unknown: PresentationStatus,
}

/// Round closure fact handed to the store layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RoundClosureFact {
    /// Companion the closed round belongs to.
    pub companion: RawId,
    /// [`ClientId`] the closed round belongs to.
    pub client: ClientId,
    /// Closed [`RoundId`].
    pub round: RoundId,
    /// Undelivered link, when close left an unpresented item.
    pub undelivered_link: Option<RawId>,
}

/// Evaluates an intake premise into an outcome.
///
/// Order of checks:
/// 1. `claimed_generation` is [`None`] returns
///    `NeedsRevalidation(MissingGenerationView)`.
/// 2. Companion unknown returns `NeedsRevalidation(UnknownCompanion)`;
///    companion not running, or an attribution in [`PresenceState::Stopped`],
///    returns `NeedsRevalidation(StoppedCompanion)`.
/// 3. Attribution in [`PresenceState::InTransition`], or in
///    [`PresenceState::RecoveryWait`] (unresolved by definition at intake;
///    resolution is the presence-layer confirm), returns
///    `HeldForTransition`.
/// 4. Companion mismatch between candidate and attribution, generation
///    mismatch, a non-matching active client, a liveness client mismatch, or
///    a dead connection returns `StaleRound` with the current generation and
///    the open round, if any.
/// 5. Round binding by intent: [`RoundIntent::Existing`] matching the open
///    round for the same companion, client, and generation returns
///    `AcceptedForRound(round)`; any other `Existing` returns `StaleRound`.
///    [`RoundIntent::Auto`] joins a matching open round when one exists and
///    mints a fresh [`RoundId`] otherwise; [`RoundIntent::New`] always
///    mints. Minting is not authority; the caller records the returned
///    round.
#[must_use]
pub fn check_intake(premise: IntakePremise) -> RoundIntakeOutcome {
    let IntakePremise {
        candidate,
        attribution,
        companion,
        live,
        open_round,
    } = premise;
    if candidate.claimed_generation.is_none() {
        return RoundIntakeOutcome::NeedsRevalidation {
            reason: RevalidationReason::MissingGenerationView,
        };
    }
    if !companion.known {
        return RoundIntakeOutcome::NeedsRevalidation {
            reason: RevalidationReason::UnknownCompanion,
        };
    }
    if !companion.running || attribution.state == PresenceState::Stopped {
        return RoundIntakeOutcome::NeedsRevalidation {
            reason: RevalidationReason::StoppedCompanion,
        };
    }
    if attribution.state == PresenceState::InTransition
        || attribution.state == PresenceState::RecoveryWait
    {
        return RoundIntakeOutcome::HeldForTransition;
    }
    let Some(claimed) = candidate.claimed_generation else {
        return RoundIntakeOutcome::NeedsRevalidation {
            reason: RevalidationReason::MissingGenerationView,
        };
    };
    let open_matches = open_round.is_some_and(|open| {
        open.companion == candidate.companion
            && open.client == candidate.client
            && open.generation == attribution.generation
    });
    let current_round = if open_matches {
        open_round.map(|open| open.round)
    } else {
        None
    };
    let stale = || RoundIntakeOutcome::StaleRound {
        current_round,
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
    if let RoundIntent::Existing(requested) = candidate.round {
        let accepted = open_round.is_some_and(|open| {
            open.round == requested
                && open.companion == candidate.companion
                && open.client == candidate.client
                && open.generation == attribution.generation
        });
        if accepted {
            RoundIntakeOutcome::AcceptedForRound { round: requested }
        } else {
            stale()
        }
    } else if matches!(candidate.round, RoundIntent::Auto)
        && let Some(open) = open_round
        && open.companion == candidate.companion
        && open.client == candidate.client
        && open.generation == attribution.generation
    {
        RoundIntakeOutcome::AcceptedForRound { round: open.round }
    } else {
        RoundIntakeOutcome::AcceptedForRound { round: new_round() }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClientInputRef, CompanionAvailability, ConfirmPresentationObservation, IntakePremise,
        OpenRound, PresentationStatus, RevalidationReason, RoundClosureFact, RoundId,
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
            local_id: String::from("local-1"),
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
            companion: CompanionAvailability {
                known: true,
                running: true,
            },
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
            companion: CompanionAvailability {
                known: false,
                running: true,
            },
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
            companion: CompanionAvailability {
                known: true,
                running: false,
            },
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
        assert!(matches!(outcome, RoundIntakeOutcome::StaleRound { .. }));
        if let RoundIntakeOutcome::StaleRound {
            current_generation, ..
        } = outcome
        {
            assert_eq!(current_generation, PresenceGeneration::first());
        }
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
        assert!(matches!(outcome, RoundIntakeOutcome::StaleRound { .. }));
        if let RoundIntakeOutcome::StaleRound {
            current_round,
            current_generation,
        } = outcome
        {
            assert_eq!(current_round, Some(open.round));
            assert_eq!(current_generation, PresenceGeneration::first());
        }
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
    fn round_id_round_trips_through_raw() {
        let raw = RawId::new();
        assert_eq!(RoundId::from_raw(raw).as_raw(), raw);
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
            local_id: String::from("local-1"),
        };
        let rendered = format!("{candidate:?}");
        assert!(
            !rendered.contains("hello companion"),
            "body redacted: {rendered}"
        );
        assert!(rendered.contains("local-1"), "refs stay: {rendered}");
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

    #[test]
    fn observation_and_closure_facts_construct() {
        let observation = ConfirmPresentationObservation {
            round: new_round(),
            presented_or_unknown: PresentationStatus::Presented,
        };
        assert_eq!(
            observation.presented_or_unknown,
            PresentationStatus::Presented
        );
        let unknown = ConfirmPresentationObservation {
            round: new_round(),
            presented_or_unknown: PresentationStatus::PresentationUnknown,
        };
        assert_eq!(
            unknown.presented_or_unknown,
            PresentationStatus::PresentationUnknown
        );
        let fact = RoundClosureFact {
            companion: RawId::new(),
            client: client(),
            round: new_round(),
            undelivered_link: None,
        };
        assert!(fact.undelivered_link.is_none());
        assert_eq!(
            RevalidationReason::UnknownReasonTag,
            RevalidationReason::UnknownReasonTag
        );
    }
}
