//! Presence attribution authority and move contracts.
//!
//! [`PresenceAttribution`] is the sole authority for which [`ClientId`], if
//! any, a companion is present on and in which [`PresenceState`] and
//! [`PresenceGeneration`] that fact holds. Callers never establish presence
//! by calling, sending, or observing; they supply premises and the
//! [`PresenceRepository`] implementation compares and commits.
//!
//! Mismatch is an [`Ok`]-side [`MoveDecision`], never an error: a stale
//! `expected` view returns `Ok(MoveDecision::RejectedAsStalePresence { .. })`
//! carrying the current [`PresenceAttribution`]. [`PresenceTechnicalError`]
//! is reserved for infrastructure failure (storage unavailable, reachability
//! check infrastructure failure), never for stale / held / denied outcomes.
//!
//! Liveness is a premise, never part of the atomic section: the caller
//! evaluates reachability out of band into a [`LiveReachabilityRef`] and the
//! repository compares-and-commits attribution without performing I/O inside
//! the atomic section. [`TransportClass`] records the Host-determined
//! transport kind behind that premise; it is evaluated Host-side, never from
//! a Client self-report.
//!
//! Wire mapping (read-only): [`PresenceAttribution`] maps to/from
//! `ene_api::v1::presence::PresenceAttributionWire` at the Host ingress
//! boundary. The wire `generation: u64` travels inside
//! [`PresenceGeneration`]; the wire `active_client` travels inside
//! `Option<ClientId>`. This crate performs no wire mapping itself.

#![cfg_attr(
    test,
    allow(
        clippy::expect_used,
        clippy::unwrap_used,
        clippy::panic,
        reason = "test fixtures may unwrap values whose failure would be a fixture bug"
    )
)]

use ene_primitive::{GenerationInner, RawId};

/// Client instance identity.
///
/// Wraps a [`RawId`]; distinct from connection identity, incarnation, and
/// presence generation. Never converted to any other domain newtype.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientId(RawId);

impl ClientId {
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

    /// Generates a fresh random identity.
    #[must_use]
    pub fn generate() -> Self {
        Self(RawId::new())
    }
}

/// Presence lifecycle interval marker.
///
/// Wraps a [`GenerationInner`]; meaningful only alongside the companion
/// lifecycle it belongs to. Never compared across companions and never
/// substituted for a revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PresenceGeneration(GenerationInner);

impl PresenceGeneration {
    /// Smallest value in a sequence.
    #[must_use]
    pub fn first() -> Self {
        Self(GenerationInner::first())
    }

    /// Successor in the sequence, or [`None`] when no further distinct value
    /// exists.
    #[must_use]
    pub fn checked_next(self) -> Option<Self> {
        self.0.checked_next().map(Self)
    }

    /// Reconstitutes a stored value alongside its companion lifecycle.
    #[must_use]
    pub fn from_u64(value: u64) -> Self {
        Self(GenerationInner::from_u64(value))
    }

    /// Returns the stored value for persistence or boundary tokens.
    #[must_use]
    pub fn as_u64(self) -> u64 {
        self.0.as_u64()
    }

    /// Wraps an existing inner value.
    #[must_use]
    pub fn from_inner(inner: GenerationInner) -> Self {
        Self(inner)
    }

    /// Returns the wrapped inner value.
    #[must_use]
    pub fn as_inner(self) -> GenerationInner {
        self.0
    }
}

/// Presence state vocabulary.
///
/// Each state is a different meaning; states never collapse into a boolean
/// or a single active field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PresenceState {
    /// Present on the active [`ClientId`].
    Present,
    /// Running with no active [`ClientId`]. A legitimate state.
    NoActive,
    /// Mid-transition. No [`ClientId`]-dependent starts on either side.
    InTransition,
    /// Stopped. No fallback or recovery applies.
    Stopped,
    /// Waiting on post-restart recovery. Restart restoration only.
    RecoveryWait,
}

/// Current attribution for one companion: state, active client, generation.
///
/// This is the sole authority for presence. Older facts are dropped and a
/// missing fact is never read as current.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PresenceAttribution {
    /// Companion this attribution belongs to.
    pub companion: RawId,
    /// Current state.
    pub state: PresenceState,
    /// Active [`ClientId`], when present.
    pub active_client: Option<ClientId>,
    /// Presence generation value the fact belongs to.
    pub generation: PresenceGeneration,
}

/// Typed expected currentness for compare-before-commit.
///
/// Carried by callers as comparison material; never authority on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PresenceCheckRef {
    /// Generation value the caller observed.
    pub expected_generation: PresenceGeneration,
    /// State value the caller observed.
    pub expected_state: PresenceState,
    /// Active [`ClientId`] value the caller observed.
    pub expected_active: Option<ClientId>,
}

/// Client-side presence claim arriving at the Host boundary.
///
/// A candidate only: acceptance, attribution checks, and round issuance
/// happen Host-side against [`PresenceAttribution`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientPresenceClaim {
    /// Companion the claim targets.
    pub companion: RawId,
    /// Claiming [`ClientId`].
    pub client: ClientId,
    /// Generation value the claim relies on.
    pub claimed_generation: PresenceGeneration,
    /// Round the claim believes it belongs to, if any.
    pub round: Option<RawId>,
}

/// Out-of-band liveness premise for one [`ClientId`].
///
/// Evaluated Host-side outside the atomic section and handed in as a
/// premise. Liveness never runs inside the compare-and-commit section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LiveReachabilityRef {
    /// [`ClientId`] the premise covers.
    pub client: ClientId,
    /// Whether the connection is currently live.
    pub connection_live: bool,
}

/// Host-determined transport class behind a liveness premise.
///
/// Evaluated Host-side, never from a Client self-report. Kept separate from
/// [`LiveReachabilityRef`] so the ref stays a minimal liveness premise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransportClass {
    /// Client runs on the same machine as the Host.
    SameMachine,
}

/// Thin reason for beginning a presence transition.
///
/// Host-side vocabulary only; a subset of the wire move reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThinMoveReason {
    /// Initial attach of a companion to a client.
    InitialAttach,
    /// A disconnect was observed and fallback is considered.
    DisconnectObserved,
    /// Restart recovery path.
    RestartRecovery,
}

/// Outcome of a compare-and-begin-transition attempt.
///
/// An [`Ok`]-side domain outcome, never an error. Stale, denied, and held
/// outcomes are returned as `Ok`, never retried automatically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveDecision {
    /// Transition began toward a new generation.
    TransitioningToNew {
        /// Generation the transition moved into.
        generation: PresenceGeneration,
    },
    /// The `expected` view was not current; carries the current fact.
    RejectedAsStalePresence {
        /// Current [`PresenceAttribution`] the caller should observe next time.
        current: PresenceAttribution,
    },
    /// Denied by a constraint, with an operational reason.
    DeniedByConstraint {
        /// Operational reason. Never a secret or a body copy.
        reason: String,
    },
    /// Held for safe closure of in-flight work before moving.
    HeldForSafeClosure,
}

/// Infrastructure failure for presence operations.
///
/// Stale / held / denied outcomes are [`MoveDecision`], never this error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PresenceTechnicalError {
    /// Durable storage was unavailable.
    #[error("presence storage unavailable: {reason}")]
    StorageUnavailable {
        /// Operational reason. Never a secret or a body copy.
        reason: String,
    },
    /// The reachability-check infrastructure failed.
    #[error("presence reachability check failed: {reason}")]
    ReachabilityCheckFailed {
        /// Operational reason. Never a secret or a body copy.
        reason: String,
    },
}

/// Durable presence attribution contract.
///
/// The implementor holds [`PresenceAttribution`] as the sole authority and
/// performs generation compare-before-commit in a short atomic section.
/// Liveness arrives as a [`LiveReachabilityRef`] premise; the atomic section
/// never performs reachability I/O itself.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait PresenceRepository {
    /// Loads the current attribution for one companion.
    ///
    /// Returns [`None`] when no attribution is recorded yet. Absence is
    /// reported, never defaulted to a current fact.
    async fn load_attribution(
        &self,
        companion: RawId,
    ) -> Result<Option<PresenceAttribution>, PresenceTechnicalError>;

    /// Compares `expected` against the current attribution and, on match,
    /// begins a transition toward `to_client` for `reason`.
    ///
    /// Mismatch returns `Ok(MoveDecision::RejectedAsStalePresence { .. })`,
    /// never `Err`. Denied and held outcomes are likewise `Ok`-side.
    async fn compare_and_begin_transition(
        &self,
        companion: RawId,
        expected: PresenceCheckRef,
        to_client: Option<ClientId>,
        reason: ThinMoveReason,
    ) -> Result<MoveDecision, PresenceTechnicalError>;

    /// Confirms a transitioning generation once the `live` premise is known.
    ///
    /// `live` is an out-of-band premise evaluated before this call; this
    /// method never performs reachability I/O inside the atomic section.
    /// The confirm may only adopt the client pinned at begin time: when
    /// `live` reports a live connection for a *different* client than the
    /// stored transition target, the row stays untouched and the call
    /// answers stale instead of crowning the newcomer. Authority flows
    /// from the begin decision, never from a later self-report.
    async fn confirm_transition(
        &self,
        companion: RawId,
        transitioning_generation: PresenceGeneration,
        live: LiveReachabilityRef,
    ) -> Result<ConfirmTransitionOutcome, PresenceTechnicalError>;
}

/// Outcome of confirming a transitioning generation.
///
/// Stale / mismatch outcomes are `Ok`-side, never errors: only
/// infrastructure failure is [`PresenceTechnicalError`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmTransitionOutcome {
    /// The transition confirmed (or had already confirmed: idempotent
    /// read-back carries the current fact).
    Confirmed(PresenceAttribution),
    /// The stored transition target differs from the confirming client, or
    /// the generation already moved on; carries the current fact. Nothing
    /// was written.
    RejectedAsStalePresence {
        /// Current [`PresenceAttribution`] the caller should observe next time.
        current: PresenceAttribution,
    },
}

#[cfg(test)]
mod tests {
    use super::ClientId;

    #[test]
    fn generated_client_ids_differ() {
        assert_ne!(ClientId::generate(), ClientId::generate());
    }
}
