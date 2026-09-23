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
//! check infrastructure failure), never for stale / denied outcomes.
//!
//! Liveness is a premise, never part of the atomic section: the caller
//! evaluates reachability out of band into a [`LiveReachabilityRef`] and the
//! repository compares-and-commits attribution without performing I/O inside
//! the atomic section.
//!
//! Wire mapping (read-only): [`PresenceAttribution`] maps to/from
//! `ene_api::v1::presence::PresenceAttributionWire` at the Host ingress
//! boundary. The wire `generation: u64` travels inside
//! [`PresenceGeneration`]; the wire `active_client` travels inside
//! `Option<ClientId>`. This crate performs no wire mapping itself.

use ene_primitive::{GenerationInner, RawId};

mod erasure;
pub use erasure::{PresenceErasureOutcome, PresenceErasureParticipant, PresenceErasureRepository};

/// Client instance identity.
///
/// Wraps a [`RawId`]; distinct from connection identity, incarnation, and
/// presence generation. Never converted to any other domain newtype.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientId(RawId);

impl ClientId {
    #[must_use]
    pub fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }

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

    #[must_use]
    pub fn from_u64(value: u64) -> Self {
        Self(GenerationInner::from_u64(value))
    }

    #[must_use]
    pub fn as_u64(self) -> u64 {
        self.0.as_u64()
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
    pub companion: RawId,
    pub state: PresenceState,
    pub active_client: Option<ClientId>,
    pub generation: PresenceGeneration,
}

/// Typed expected currentness for compare-before-commit.
///
/// Carried by callers as comparison material; never authority on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PresenceCheckRef {
    pub expected_generation: PresenceGeneration,
    pub expected_state: PresenceState,
    pub expected_active: Option<ClientId>,
}

/// Out-of-band liveness premise for one [`ClientId`].
///
/// Evaluated Host-side outside the atomic section and handed in as a
/// premise. Liveness never runs inside the compare-and-commit section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LiveReachabilityRef {
    pub client: ClientId,
    pub connection_live: bool,
}

/// Thin reason for beginning a presence transition.
///
/// Host-side vocabulary only; a subset of the wire move reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThinMoveReason {
    InitialAttach,
    DisconnectObserved,
    RestartRecovery,
    /// Stop accepted: the transition moves the attribution to
    /// [`PresenceState::Stopped`]. Stop outranks move and recovery, so it is
    /// recorded as its own reason rather than folded into a move.
    Stop,
}

/// Relocation hint for one companion.
///
/// [`last_client`](Self::last_client) is the client of the last confirmed
/// [`PresenceState::Present`] attribution. It is history, not a current
/// reference: an [`PresenceState::InTransition`] candidate lives in
/// [`PresenceAttribution::active_client`] and must never overwrite it, and a
/// [`PresenceState::NoActive`] fallback that fails to confirm must not
/// promote it either.
///
/// [`recovery_destination`](Self::recovery_destination) is a non-current
/// reference valid only during [`PresenceState::RecoveryWait`]. Restart
/// recovery (§6.4) sets it to the client that was `Present` before the Host
/// restart; leaving `RecoveryWait` — a transition begin, a confirm, or a stop
/// — clears it. A normal disconnect never sets it, and the record alone never
/// establishes presence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RelocationHint {
    pub companion: RawId,
    pub last_client: Option<ClientId>,
    pub recovery_destination: Option<ClientId>,
}

/// One fallback candidate premise for the normal-disconnect fallback.
///
/// The caller derives each flag out of band (connection currentness,
/// transport classification, device permission) and the selection function
/// only compares them; none of them is inferred from the hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FallbackCandidate {
    pub client: ClientId,
    /// The connection layer reports this client's connection as the current
    /// authenticated one.
    pub current_authenticated: bool,
    /// The OS transport classification confirms a same-machine peer.
    pub same_machine: bool,
    /// The device permission currently allows the client.
    pub device_permitted: bool,
}

/// Selects the normal-disconnect fallback target (CCT §10.4).
///
/// Only a candidate that is current-authenticated, same-machine verified,
/// device-permitted, and not the closing client is eligible; among the
/// eligible ones the lexicographically smallest [`ClientId`] bytes win.
/// [`None`] means no eligible candidate, which callers confirm as
/// [`PresenceState::NoActive`]: the Host never guesses a client and never
/// auto-starts one.
#[must_use]
pub fn select_fallback_candidate(
    closing: ClientId,
    candidates: &[FallbackCandidate],
) -> Option<ClientId> {
    candidates
        .iter()
        .filter(|candidate| {
            candidate.current_authenticated
                && candidate.same_machine
                && candidate.device_permitted
                && candidate.client != closing
        })
        .map(|candidate| candidate.client)
        .min_by_key(|client| *client.as_raw().as_uuid().as_bytes())
}

/// One companion whose startup normalization was refused.
///
/// The refusal names the companion and the reason; the stored attribution is
/// left untouched (never guessed, never rewritten).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartupNormalizationFailure {
    pub companion: RawId,
    pub reason: StartupNormalizationFailureReason,
}

/// Why one companion's startup normalization was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupNormalizationFailureReason {
    /// No encodable generation successor exists; the old attribution stays
    /// and activity must not start from it.
    GenerationExhausted,
    /// A `RecoveryWait` row carries no recovery destination, so the recovery
    /// intent cannot be preserved.
    MissingRecoveryDestination,
    /// A `Present` row carries no active client to recover toward.
    MalformedAttribution,
    /// A `companion` row exists without a presence attribution row.
    MissingAttribution,
}

/// Report of one explicit startup normalization pass (PR §6.4).
///
/// Each companion is normalized in its own short transaction; `failures`
/// records the companions that were refused. A caller that must not serve
/// with an unknown presence state fails startup when this list is non-empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupNormalizationReport {
    /// The committed facts, one per companion whose row changed.
    pub normalized: Vec<PresenceAttribution>,
    /// The companions whose row was left untouched.
    pub failures: Vec<StartupNormalizationFailure>,
}

/// Outcome of one explicit stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopCompanionOutcome {
    /// The stop committed; carries the new `Stopped` fact.
    Stopped(PresenceAttribution),
    /// The attribution was already `Stopped`; nothing was written.
    AlreadyStopped(PresenceAttribution),
    /// No presence attribution row exists for this companion.
    MissingCompanion,
}

/// Outcome of a compare-and-begin-transition attempt.
///
/// An [`Ok`]-side domain outcome, never an error. Stale and denied
/// outcomes are returned as `Ok`, never retried automatically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveDecision {
    /// Transition began toward a new generation.
    TransitioningToNew { generation: PresenceGeneration },
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
}

/// Infrastructure failure for presence operations.
///
/// Stale / denied outcomes are [`MoveDecision`], never this error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PresenceTechnicalError {
    #[error("presence storage unavailable: {reason}")]
    StorageUnavailable {
        /// Operational reason. Never a secret or a body copy.
        reason: String,
    },
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

    /// Loads the relocation hint for one companion.
    ///
    /// Returns [`None`] when no hint row exists. The hint is never authority:
    /// callers must re-derive currentness before establishing presence.
    async fn load_hint(
        &self,
        companion: RawId,
    ) -> Result<Option<RelocationHint>, PresenceTechnicalError>;

    /// Runs the explicit startup normalization (PR §6.4) over every companion
    /// row.
    ///
    /// This is a startup boundary, never a periodic sweep, and the plain
    /// state open must not run it. Each companion is normalized in its own
    /// short `Immediate` transaction (attribution, hint, and transition-log
    /// row together) with [`ThinMoveReason::RestartRecovery`], so one
    /// companion's refusal cannot roll back another's commit. A companion
    /// whose next generation cannot be encoded is refused and left untouched
    /// in [`StartupNormalizationReport::failures`]; storage failures are
    /// [`PresenceTechnicalError`].
    async fn normalize_on_startup(
        &self,
    ) -> Result<StartupNormalizationReport, PresenceTechnicalError>;

    /// Stops one companion's presence: from any state to
    /// [`PresenceState::Stopped`], clearing the active client and the
    /// recovery destination while keeping `last_client` as history.
    ///
    /// Stop outranks moves and recovery; a companion already `Stopped` is
    /// answered idempotently with no write.
    async fn stop_companion(
        &self,
        companion: RawId,
    ) -> Result<StopCompanionOutcome, PresenceTechnicalError>;

    /// Compares `expected` against the current attribution and, on match,
    /// begins a transition toward `to_client` for `reason`.
    ///
    /// Mismatch returns `Ok(MoveDecision::RejectedAsStalePresence { .. })`,
    /// never `Err`. Denied outcomes are likewise `Ok`-side.
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
    use super::{ClientId, FallbackCandidate, RawId, select_fallback_candidate};
    use uuid::Uuid;

    fn client(hex: &str) -> ClientId {
        let uuid = Uuid::parse_str(hex).expect("fixture uuid");
        ClientId::from_raw(RawId::from_uuid(uuid))
    }

    fn candidate(
        client: ClientId,
        current_authenticated: bool,
        same_machine: bool,
        device_permitted: bool,
    ) -> FallbackCandidate {
        FallbackCandidate {
            client,
            current_authenticated,
            same_machine,
            device_permitted,
        }
    }

    #[test]
    fn fallback_requires_current_same_machine_and_permitted() {
        let closing = client("00000000-0000-0000-0000-000000000001");
        let other = client("00000000-0000-0000-0000-000000000002");
        for (current, same_machine, permitted) in [
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            let candidates = [candidate(other, current, same_machine, permitted)];
            assert_eq!(
                select_fallback_candidate(closing, &candidates),
                None,
                "a candidate missing current/same-machine/permitted must not be eligible"
            );
        }
        let eligible = [candidate(other, true, true, true)];
        assert_eq!(select_fallback_candidate(closing, &eligible), Some(other));
    }

    #[test]
    fn fallback_excludes_the_closing_client() {
        let closing = client("00000000-0000-0000-0000-000000000001");
        let candidates = [candidate(closing, true, true, true)];
        assert_eq!(select_fallback_candidate(closing, &candidates), None);
    }

    #[test]
    fn fallback_picks_the_lexicographically_smallest_client_bytes() {
        let closing = client("00000000-0000-0000-0000-000000000001");
        let small = client("00000000-0000-0000-0000-0000000000ff");
        let large = client("ffffffff-ffff-ffff-ffff-ffffffffffff");
        let candidates = [
            candidate(large, true, true, true),
            candidate(small, true, true, true),
        ];
        assert_eq!(select_fallback_candidate(closing, &candidates), Some(small));
    }
}
