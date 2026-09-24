mod action;
mod erasure;
mod intent;
mod usage_cap;

use std::collections::HashMap;

use ene_primitive::{RawId, RevisionInner};
use thiserror::Error;

pub use action::{
    ActionAuthorizationDecision, ActionDenyCode, ActionEvaluationTracker, ActionKind,
    ActionPermissionEvaluationId, ActionUseCandidate, CurrentActionPremise, authorize_action_use,
};
pub use erasure::{PermissionErasureParticipant, PermissionErasureRepository};
pub use intent::{AssignConsentIntent, assign_consent};
pub use usage_cap::{
    SetUsageCapCommand, SetUsageCapOutcome, UsageCap, UsageCapConsumption, UsageCapId, UsageCapRef,
    UsageCapRepository, UsageCapRevision, UsageCapScope, UsageCapStatus, UsageCapStatusQuery,
    UsageCapWindow, UsageReservationRef, UsageReservationState, UtcPeriod, parse_usage_cap_mark,
    usage_cap_mark,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PermissionEvaluationId(RawId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConsentRevision(RevisionInner);

impl ConsentRevision {
    #[must_use]
    pub fn from_u64(value: u64) -> Self {
        Self(RevisionInner::from_u64(value))
    }

    #[must_use]
    pub fn as_u64(&self) -> u64 {
        self.0.as_u64()
    }

    #[must_use]
    pub fn checked_next(&self) -> Option<Self> {
        self.0.checked_next().map(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConsumerKind {
    CompanionDialogue,
    CompanionLearning,
    TaskAgent,
}

impl ConsumerKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CompanionDialogue => "companion_dialogue",
            Self::CompanionLearning => "companion_learning",
            Self::TaskAgent => "task_agent",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "companion_dialogue" => Some(Self::CompanionDialogue),
            "companion_learning" => Some(Self::CompanionLearning),
            "task_agent" => Some(Self::TaskAgent),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapabilityKind {
    Dialogue,
    Learning,
}

impl CapabilityKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dialogue => "dialogue",
            Self::Learning => "learning",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "dialogue" => Some(Self::Dialogue),
            "learning" => Some(Self::Learning),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PurposeKind {
    DialogueResponse,
    MemoryFormation,
    TaskAgentTurn,
}

impl PurposeKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DialogueResponse => "dialogue_response",
            Self::MemoryFormation => "memory_formation",
            Self::TaskAgentTurn => "task_agent_turn",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "dialogue_response" => Some(Self::DialogueResponse),
            "memory_formation" => Some(Self::MemoryFormation),
            "task_agent_turn" => Some(Self::TaskAgentTurn),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferenceUseCandidate {
    pub consumer: ConsumerKind,
    pub capability: CapabilityKind,
    pub provider_ref: String,
    pub model: String,
    pub purpose: PurposeKind,
}

impl InferenceUseCandidate {
    #[must_use]
    fn fingerprint(&self) -> EvalFingerprint {
        EvalFingerprint(
            self.consumer,
            self.capability,
            self.provider_ref.clone(),
            self.model.clone(),
            self.purpose,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct EvalFingerprint(ConsumerKind, CapabilityKind, String, String, PurposeKind);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckLiveAuthorizationQuery {
    pub candidate: InferenceUseCandidate,
    pub expected_consent: Option<(String, ConsentRevision)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveAuthorizationDecision {
    AllowForThisUse(PermissionEvaluationId),
    Deny(DenyCode),
    NeedsRevalidation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DenyCode {
    NotInAllowlist,
    ConsentStale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentRecord {
    pub capability: CapabilityKind,
    pub id: String,
    pub rev: ConsentRevision,
    pub provider: String,
    pub model: String,
    pub credential_id: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PermissionTechnicalError {
    #[error("consent storage unavailable: {reason}")]
    StorageUnavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsentCommitOutcome {
    Committed { record: ConsentRecord },
    StaleCurrent { current: Option<ConsentRecord> },
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait ConsentRepository: Send + Sync {
    async fn load_current(
        &self,
        capability: CapabilityKind,
    ) -> Result<Option<ConsentRecord>, PermissionTechnicalError>;
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait IntentOutcomeRepository: Send + Sync {
    async fn record_intent_outcome(
        &self,
        record: IntentOutcomeRecord,
    ) -> Result<IntentResolution<()>, PermissionTechnicalError>;

    async fn lookup_intent_outcome(
        &self,
        intent_id: &str,
    ) -> Result<Option<IntentOutcomeRecord>, PermissionTechnicalError>;

    async fn assign_with_intent(
        &self,
        expected: Option<(String, ConsentRevision)>,
        record: ConsentRecord,
        fingerprint: IntentFingerprint,
    ) -> Result<IntentResolution<ConsentCommitOutcome>, PermissionTechnicalError>;

    async fn complete_with_intent(
        &self,
        expected_base: String,
        bearer_present: bool,
        fingerprint: IntentFingerprint,
    ) -> Result<IntentResolution<IntentOutcomeRecord>, PermissionTechnicalError>;

    async fn shortcut_with_intent(
        &self,
        capability: CapabilityKind,
        provider: String,
        model: String,
        credential_id: String,
        fingerprint: IntentFingerprint,
    ) -> Result<IntentResolution<ShortcutIntentOutcome>, PermissionTechnicalError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentResolution<T> {
    Decided(T),
    Replay(IntentOutcomeRecord),
    Conflict(IntentOutcomeRecord),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IntentFingerprint {
    pub intent_id: String,
    pub kind: String,
    pub target: String,
    pub base: String,
    pub rationale_origin: String,
    pub rationale_quote: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IntentOutcomeRecord {
    pub fingerprint: IntentFingerprint,
    pub outcome: IntentOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IntentOutcome {
    StoredAsRuleView { revision: String },
    AppliedAsOneTime,
    HeldByOperation,
    NeedsClarification,
    RevisionExhausted,
    StaleBaseView { current: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutIntentOutcome {
    Hit { current: ConsentRecord },
    Miss,
}

#[must_use]
pub fn consent_mark(capability: CapabilityKind, rev: Option<u64>) -> String {
    render_revision_state(&format!("consent-{}-", capability.as_str()), rev)
}

#[must_use]
pub fn consent_current_mark(capability: CapabilityKind, current: Option<&ConsentRecord>) -> String {
    consent_mark(capability, current.map(|record| record.rev.as_u64()))
}

#[must_use]
pub fn consent_view_mark(dialogue_rev: Option<u64>, learning_rev: Option<u64>) -> String {
    format!(
        "{};{}",
        consent_mark(CapabilityKind::Dialogue, dialogue_rev),
        consent_mark(CapabilityKind::Learning, learning_rev),
    )
}

#[must_use]
pub fn parse_consent_mark(mark: &str, capability: CapabilityKind) -> Option<Option<u64>> {
    let qualified = format!("consent-{}-", capability.as_str());
    for segment in mark.split(';').map(str::trim) {
        if let Some(state) = segment.strip_prefix(&qualified) {
            return parse_revision_state(state);
        }
    }
    None
}

fn render_revision_state(prefix: &str, revision: Option<u64>) -> String {
    match revision {
        Some(number) => format!("{prefix}rev-{number}"),
        None => format!("{prefix}none"),
    }
}

fn parse_revision_state(state: &str) -> Option<Option<u64>> {
    if state == "none" {
        return Some(None);
    }
    Some(Some(state.strip_prefix("rev-")?.parse::<u64>().ok()?))
}
#[derive(Debug, Default)]
pub struct EvaluationTracker {
    issued: HashMap<RawId, EvalFingerprint>,
}

impl EvaluationTracker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            issued: HashMap::new(),
        }
    }

    pub(crate) fn mint(&mut self, candidate: &InferenceUseCandidate) -> PermissionEvaluationId {
        let id = PermissionEvaluationId(RawId::new());
        self.issued.insert(id.0, candidate.fingerprint());
        id
    }

    pub fn consume(
        &mut self,
        id: &PermissionEvaluationId,
        candidate: &InferenceUseCandidate,
    ) -> bool {
        match self.issued.get(&id.0) {
            Some(bound) if bound == &candidate.fingerprint() => {
                self.issued.remove(&id.0);
                true
            }
            _ => false,
        }
    }
}

pub fn check_live_authorization(
    query: &CheckLiveAuthorizationQuery,
    current: Option<&ConsentRecord>,
    tracker: &mut EvaluationTracker,
) -> LiveAuthorizationDecision {
    let in_allowlist = matches!(
        (
            query.candidate.consumer,
            query.candidate.capability,
            query.candidate.purpose
        ),
        (
            ConsumerKind::CompanionDialogue,
            CapabilityKind::Dialogue,
            PurposeKind::DialogueResponse
        ) | (
            ConsumerKind::CompanionLearning,
            CapabilityKind::Learning,
            PurposeKind::MemoryFormation
        ) | (
            ConsumerKind::TaskAgent,
            CapabilityKind::Dialogue,
            PurposeKind::TaskAgentTurn
        )
    );
    if !in_allowlist {
        return LiveAuthorizationDecision::Deny(DenyCode::NotInAllowlist);
    }
    let Some(record) = current else {
        return LiveAuthorizationDecision::Deny(DenyCode::ConsentStale);
    };
    if record.capability != query.candidate.capability {
        return LiveAuthorizationDecision::Deny(DenyCode::ConsentStale);
    }
    let current_view = (record.id.clone(), record.rev);
    if query.expected_consent.as_ref() != Some(&current_view) {
        return LiveAuthorizationDecision::NeedsRevalidation;
    }
    if query.candidate.provider_ref != record.provider || query.candidate.model != record.model {
        return LiveAuthorizationDecision::Deny(DenyCode::ConsentStale);
    }
    let id = tracker.mint(&query.candidate);
    LiveAuthorizationDecision::AllowForThisUse(id)
}

#[cfg(test)]
mod tests {
    use super::{
        CapabilityKind, CheckLiveAuthorizationQuery, ConsentRecord, ConsentRevision, ConsumerKind,
        DenyCode, EvaluationTracker, InferenceUseCandidate, LiveAuthorizationDecision, PurposeKind,
        check_live_authorization,
    };

    fn dialogue_candidate() -> InferenceUseCandidate {
        InferenceUseCandidate {
            consumer: ConsumerKind::CompanionDialogue,
            capability: CapabilityKind::Dialogue,
            provider_ref: "acme".to_owned(),
            model: "dialogue-1".to_owned(),
            purpose: PurposeKind::DialogueResponse,
        }
    }

    fn learning_candidate() -> InferenceUseCandidate {
        InferenceUseCandidate {
            consumer: ConsumerKind::CompanionLearning,
            capability: CapabilityKind::Learning,
            provider_ref: "acme".to_owned(),
            model: "dialogue-1".to_owned(),
            purpose: PurposeKind::MemoryFormation,
        }
    }

    fn task_candidate() -> InferenceUseCandidate {
        InferenceUseCandidate {
            consumer: ConsumerKind::TaskAgent,
            capability: CapabilityKind::Dialogue,
            provider_ref: "acme".to_owned(),
            model: "dialogue-1".to_owned(),
            purpose: PurposeKind::TaskAgentTurn,
        }
    }

    fn record_for(capability: CapabilityKind) -> ConsentRecord {
        ConsentRecord {
            capability,
            id: "consent-1".to_owned(),
            rev: ConsentRevision::from_u64(3),
            provider: "acme".to_owned(),
            model: "dialogue-1".to_owned(),
            credential_id: "cred-1".to_owned(),
        }
    }

    fn query_for(
        candidate: &InferenceUseCandidate,
        stored: &ConsentRecord,
    ) -> CheckLiveAuthorizationQuery {
        CheckLiveAuthorizationQuery {
            candidate: candidate.clone(),
            expected_consent: Some((stored.id.clone(), stored.rev)),
        }
    }

    fn authorize(
        query: &CheckLiveAuthorizationQuery,
        current: Option<&ConsentRecord>,
        tracker: &mut EvaluationTracker,
    ) -> LiveAuthorizationDecision {
        check_live_authorization(query, current, tracker)
    }

    #[test]
    fn inference_token_is_single_use_and_bound_to_the_full_fingerprint() {
        let stored = record_for(CapabilityKind::Dialogue);
        let candidate = dialogue_candidate();
        let mut tracker = EvaluationTracker::new();
        let LiveAuthorizationDecision::AllowForThisUse(evaluation) =
            authorize(&query_for(&candidate, &stored), Some(&stored), &mut tracker)
        else {
            panic!("exact current consent must mint an evaluation");
        };

        let mut other_model = candidate.clone();
        other_model.model = String::from("other-model");
        let mut other_provider = candidate.clone();
        other_provider.provider_ref = String::from("other-provider");
        let mut other_consumer = candidate.clone();
        other_consumer.consumer = ConsumerKind::TaskAgent;
        let mut other_capability = candidate.clone();
        other_capability.capability = CapabilityKind::Learning;
        let mut other_purpose = candidate.clone();
        other_purpose.purpose = PurposeKind::TaskAgentTurn;
        for altered in [
            other_model,
            other_provider,
            other_consumer,
            other_capability,
            other_purpose,
        ] {
            assert!(
                !tracker.consume(&evaluation, &altered),
                "a token must not transfer to another inference fingerprint"
            );
        }

        let mut foreign_tracker = EvaluationTracker::new();
        let foreign = foreign_tracker.mint(&candidate);
        assert!(!tracker.consume(&foreign, &candidate));
        assert!(tracker.consume(&evaluation, &candidate));
        assert!(!tracker.consume(&evaluation, &candidate));
    }

    #[test]
    fn authorization_requires_the_exact_current_consent_view() {
        let stored = record_for(CapabilityKind::Dialogue);
        for expected in [
            Some((String::from("other-consent"), stored.rev)),
            Some((stored.id.clone(), ConsentRevision::from_u64(2))),
            None,
        ] {
            let query = CheckLiveAuthorizationQuery {
                expected_consent: expected,
                ..query_for(&dialogue_candidate(), &stored)
            };
            let mut tracker = EvaluationTracker::new();
            assert_eq!(
                authorize(&query, Some(&stored), &mut tracker),
                LiveAuthorizationDecision::NeedsRevalidation,
                "an id/revision view that is not exactly current must be revalidated"
            );
        }

        let mut tracker = EvaluationTracker::new();
        assert_eq!(
            authorize(
                &query_for(&dialogue_candidate(), &stored),
                None,
                &mut tracker
            ),
            LiveAuthorizationDecision::Deny(DenyCode::ConsentStale)
        );

        let learning = record_for(CapabilityKind::Learning);
        assert_eq!(
            authorize(
                &query_for(&dialogue_candidate(), &stored),
                Some(&learning),
                &mut tracker
            ),
            LiveAuthorizationDecision::Deny(DenyCode::ConsentStale),
            "a different capability cannot satisfy the current consent"
        );

        for field in ["provider", "model"] {
            let mut candidate = dialogue_candidate();
            if field == "provider" {
                candidate.provider_ref = String::from("other-provider");
            } else {
                candidate.model = String::from("other-model");
            }
            assert_eq!(
                authorize(&query_for(&candidate, &stored), Some(&stored), &mut tracker),
                LiveAuthorizationDecision::Deny(DenyCode::ConsentStale),
                "the consent binds the exact provider and model"
            );
        }
    }

    #[test]
    fn inference_allowlist_is_the_three_closed_world_triples() {
        for (candidate, capability) in [
            (dialogue_candidate(), CapabilityKind::Dialogue),
            (learning_candidate(), CapabilityKind::Learning),
            (task_candidate(), CapabilityKind::Dialogue),
        ] {
            let stored = record_for(capability);
            let mut tracker = EvaluationTracker::new();
            let LiveAuthorizationDecision::AllowForThisUse(evaluation) =
                authorize(&query_for(&candidate, &stored), Some(&stored), &mut tracker)
            else {
                panic!("the exact closed-world triple must be allowed");
            };
            assert!(tracker.consume(&evaluation, &candidate));
        }

        let stored = record_for(CapabilityKind::Dialogue);
        for candidate in [
            InferenceUseCandidate {
                capability: CapabilityKind::Learning,
                ..dialogue_candidate()
            },
            InferenceUseCandidate {
                consumer: ConsumerKind::CompanionLearning,
                ..dialogue_candidate()
            },
            InferenceUseCandidate {
                purpose: PurposeKind::MemoryFormation,
                ..dialogue_candidate()
            },
            InferenceUseCandidate {
                purpose: PurposeKind::DialogueResponse,
                ..task_candidate()
            },
        ] {
            let mut tracker = EvaluationTracker::new();
            assert_eq!(
                authorize(&query_for(&candidate, &stored), Some(&stored), &mut tracker),
                LiveAuthorizationDecision::Deny(DenyCode::NotInAllowlist),
                "wrong capability, consumer, or purpose must stay denied: {candidate:?}"
            );
        }
    }

    #[test]
    fn consent_revision_exhaustion_is_not_aliased() {
        assert_eq!(
            ConsentRevision::from_u64(0).checked_next(),
            Some(ConsentRevision::from_u64(1))
        );
        assert_eq!(ConsentRevision::from_u64(u64::MAX).checked_next(), None);
    }
}
