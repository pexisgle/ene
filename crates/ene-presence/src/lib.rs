use ene_primitive::{GenerationInner, RawId};

mod erasure;
pub use erasure::{PresenceErasureParticipant, PresenceErasureRepository};

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PresenceGeneration(GenerationInner);

impl PresenceGeneration {
    #[must_use]
    pub fn first() -> Self {
        Self(GenerationInner::first())
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PresenceState {
    Present,
    NoActive,
    InTransition,
    Stopped,
    RecoveryWait,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PresenceAttribution {
    pub companion: RawId,
    pub state: PresenceState,
    pub active_client: Option<ClientId>,
    pub generation: PresenceGeneration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PresenceCheckRef {
    pub expected_generation: PresenceGeneration,
    pub expected_state: PresenceState,
    pub expected_active: Option<ClientId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LiveReachabilityRef {
    pub client: ClientId,
    pub connection_live: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThinMoveReason {
    InitialAttach,
    DisconnectObserved,
    RestartRecovery,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RelocationHint {
    pub companion: RawId,
    pub last_client: Option<ClientId>,
    pub recovery_destination: Option<ClientId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FallbackCandidate {
    pub client: ClientId,
    pub current_authenticated: bool,
    pub same_machine: bool,
    pub device_permitted: bool,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartupNormalizationFailure {
    pub companion: RawId,
    pub reason: StartupNormalizationFailureReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupNormalizationFailureReason {
    GenerationExhausted,
    MissingRecoveryDestination,
    MalformedAttribution,
    MissingAttribution,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupNormalizationReport {
    pub normalized: Vec<PresenceAttribution>,
    pub failures: Vec<StartupNormalizationFailure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopCompanionOutcome {
    Stopped(PresenceAttribution),
    AlreadyStopped(PresenceAttribution),
    MissingCompanion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveDecision {
    TransitioningToNew { generation: PresenceGeneration },
    RejectedAsStalePresence { current: PresenceAttribution },
    DeniedByConstraint { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PresenceTechnicalError {
    #[error("presence storage unavailable: {reason}")]
    StorageUnavailable { reason: String },
    #[error("presence reachability check failed: {reason}")]
    ReachabilityCheckFailed { reason: String },
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait PresenceRepository {
    async fn load_attribution(
        &self,
        companion: RawId,
    ) -> Result<Option<PresenceAttribution>, PresenceTechnicalError>;

    async fn load_hint(
        &self,
        companion: RawId,
    ) -> Result<Option<RelocationHint>, PresenceTechnicalError>;

    async fn normalize_on_startup(
        &self,
    ) -> Result<StartupNormalizationReport, PresenceTechnicalError>;

    async fn stop_companion(
        &self,
        companion: RawId,
    ) -> Result<StopCompanionOutcome, PresenceTechnicalError>;

    async fn compare_and_begin_transition(
        &self,
        companion: RawId,
        expected: PresenceCheckRef,
        to_client: Option<ClientId>,
        reason: ThinMoveReason,
    ) -> Result<MoveDecision, PresenceTechnicalError>;

    async fn confirm_transition(
        &self,
        companion: RawId,
        transitioning_generation: PresenceGeneration,
        live: LiveReachabilityRef,
    ) -> Result<ConfirmTransitionOutcome, PresenceTechnicalError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmTransitionOutcome {
    Confirmed(PresenceAttribution),
    RejectedAsStalePresence { current: PresenceAttribution },
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
