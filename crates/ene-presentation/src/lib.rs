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
//! `ene_api::v1::round::RoundIntakeOutcomeWire`.
//!
//! Minting versus acceptance: a freshly minted [`RoundId`] accepts nothing.
//! [`check_intake`] accepts; on an [`RoundIntent::Auto`]
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

/// Owner text input reference: transient expression body plus language tag.
///
/// The Host canonicalizes accepted text into History; this ref itself is
/// never durable.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ClientInputRef {
    /// Body text. Redacted from [`core::fmt::Debug`].
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
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SubmitClientInputCandidate {
    pub companion: RawId,
    pub client: ClientId,
    /// [`None`] means the generation token was missing.
    pub claimed_generation: Option<PresenceGeneration>,
    /// A single meaning per value — never an `Option` doing double duty.
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

/// Which round an intake candidate wants to join.
///
/// Each variant names exactly one intention, so replay fingerprints and
/// round projections built downstream rest on a single meaning source.
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
///
/// Three states, never a `(known, running)` pair: `known = false,
/// running = true` is unrepresentable, so intake can match once instead of
/// guarding combinations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompanionAvailability {
    /// The companion is not known to the lifecycle store.
    Unknown,
    /// Known but not running; intake is revalidated against its lifecycle.
    Stopped,
    /// Known and running; intake may proceed to attribution checks.
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
#[derive(Clone, PartialEq, Eq)]
pub struct IntakePremise {
    pub candidate: SubmitClientInputCandidate,
    /// Current authoritative [`PresenceAttribution`].
    pub attribution: PresenceAttribution,
    pub companion: CompanionAvailability,
    /// Out-of-band liveness premise.
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

/// Opaque revalidation reason matched at Host ingress.
///
/// Unknown wire values map to [`RevalidationReason::UnknownReasonTag`];
/// [`check_intake`] itself never emits that variant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RevalidationReason {
    MissingGenerationView,
    UnknownCompanion,
    StoppedCompanion,
    /// The command carried no idempotency key. Replay safety needs one, so
    /// keyless commands are declined rather than accepted unkeyed.
    MissingCommandId,
    /// The current input alone exceeds the inference request budget. Emitted
    /// by the Host before acceptance (never by [`check_intake`]); the Owner
    /// shrinks the input and retries.
    InputOverLimit,
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
        round: RoundId,
    },
    StaleRound {
        current_round: Option<RoundId>,
        /// Current generation value the sender should observe next time.
        current_generation: PresenceGeneration,
    },
    HeldForTransition,
    NeedsRevalidation {
        reason: RevalidationReason,
    },
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
    // The one matching open round: same companion, client, and current
    // generation. Every later check reuses this value instead of rebuilding
    // the match.
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

    fn premise() -> IntakePremise {
        let companion = RawId::new();
        let client = ClientId::from_raw(RawId::new());
        let generation = PresenceGeneration::first();
        IntakePremise {
            candidate: SubmitClientInputCandidate {
                companion,
                client,
                claimed_generation: Some(generation),
                round: RoundIntent::Auto,
                input_ref: ClientInputRef {
                    text: String::from("private body"),
                    lang: String::from("en"),
                },
                local_id: String::from("local-1"),
            },
            attribution: PresenceAttribution {
                companion,
                state: PresenceState::Present,
                active_client: Some(client),
                generation,
            },
            companion: CompanionAvailability::Running,
            live: LiveReachabilityRef {
                client,
                connection_live: true,
            },
            open_round: None,
        }
    }

    #[test]
    fn intake_revalidation_and_hold_boundaries() {
        let mut missing = premise();
        missing.candidate.claimed_generation = None;
        assert_eq!(
            check_intake(missing),
            RoundIntakeOutcome::NeedsRevalidation {
                reason: RevalidationReason::MissingGenerationView,
            }
        );
        for (availability, reason) in [
            (
                CompanionAvailability::Unknown,
                RevalidationReason::UnknownCompanion,
            ),
            (
                CompanionAvailability::Stopped,
                RevalidationReason::StoppedCompanion,
            ),
        ] {
            let mut case = premise();
            case.companion = availability;
            assert_eq!(
                check_intake(case),
                RoundIntakeOutcome::NeedsRevalidation { reason }
            );
        }
        let mut stopped = premise();
        stopped.attribution.state = PresenceState::Stopped;
        assert_eq!(
            check_intake(stopped),
            RoundIntakeOutcome::NeedsRevalidation {
                reason: RevalidationReason::StoppedCompanion,
            }
        );
        for state in [PresenceState::InTransition, PresenceState::RecoveryWait] {
            let mut case = premise();
            case.attribution.state = state;
            assert_eq!(check_intake(case), RoundIntakeOutcome::HeldForTransition);
        }
    }

    #[test]
    fn stale_claims_return_current_generation_and_round() {
        let mut base = premise();
        let open = OpenRound {
            companion: base.candidate.companion,
            client: base.candidate.client,
            round: new_round(),
            generation: base.attribution.generation,
        };
        base.open_round = Some(open);
        let stale = |case| {
            assert_eq!(
                check_intake(case),
                RoundIntakeOutcome::StaleRound {
                    current_round: Some(open.round),
                    current_generation: open.generation,
                }
            )
        };
        let mut generation = base.clone();
        generation.candidate.claimed_generation = Some(PresenceGeneration::from_u64(7));
        stale(generation);
        let mut client = base.clone();
        client.attribution.active_client = None;
        stale(client);
        let mut dead = base.clone();
        dead.live.connection_live = false;
        stale(dead);
        let mut wrong_round = base;
        wrong_round.candidate.round = RoundIntent::Existing(new_round());
        stale(wrong_round);
    }

    #[test]
    fn round_intents_join_or_mint_as_requested() {
        let mut base = premise();
        let open = OpenRound {
            companion: base.candidate.companion,
            client: base.candidate.client,
            round: new_round(),
            generation: base.attribution.generation,
        };
        assert!(matches!(
            check_intake(base.clone()),
            RoundIntakeOutcome::AcceptedForRound { .. }
        ));
        base.open_round = Some(open);
        assert_eq!(
            check_intake(base.clone()),
            RoundIntakeOutcome::AcceptedForRound { round: open.round }
        );
        base.candidate.round = RoundIntent::Existing(open.round);
        assert_eq!(
            check_intake(base.clone()),
            RoundIntakeOutcome::AcceptedForRound { round: open.round }
        );
        base.candidate.round = RoundIntent::New;
        assert!(
            matches!(check_intake(base), RoundIntakeOutcome::AcceptedForRound { round } if round != open.round)
        );
    }

    #[test]
    fn debug_redacts_input_through_the_premise() {
        let rendered = format!("{:?}", premise());
        assert!(!rendered.contains("private body"));
        assert!(rendered.contains("local-1"));
    }
}
