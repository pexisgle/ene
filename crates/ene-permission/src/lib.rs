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
pub use erasure::{
    PermissionErasureOutcome, PermissionErasureParticipant, PermissionErasureRepository,
};
pub use intent::{AssignConsentIntent, BaseViewExpectation, assign_consent, base_view_expectation};
pub use usage_cap::{
    SetUsageCapCommand, SetUsageCapOutcome, UsageCap, UsageCapConsumption, UsageCapId, UsageCapRef,
    UsageCapRepository, UsageCapRevision, UsageCapScope, UsageCapStatus, UsageCapStatusQuery,
    UsageCapWindow, UsageReservationRef, UsageReservationState, UtcPeriod, parse_usage_cap_mark,
    usage_cap_mark,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PermissionEvaluationId(pub RawId);

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
    pub fn fingerprint(&self) -> EvalFingerprint {
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
pub struct EvalFingerprint(
    pub ConsumerKind,
    pub CapabilityKind,
    pub String,
    pub String,
    pub PurposeKind,
);

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
    let name = capability.as_str();
    match rev {
        Some(number) => format!("consent-{name}-rev-{number}"),
        None => format!("consent-{name}-none"),
    }
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
            return parse_consent_state(state);
        }
        if capability == CapabilityKind::Dialogue
            && let Some(state) = segment.strip_prefix("consent-")
            && let Some(parsed) = parse_consent_state(state)
        {
            return Some(parsed);
        }
    }
    None
}

fn parse_consent_state(state: &str) -> Option<Option<u64>> {
    if state == "none" {
        return Some(None);
    }
    let revision = state.strip_prefix("rev-")?.parse::<u64>().ok()?;
    Some(Some(revision))
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

    pub fn mint(&mut self, candidate: &InferenceUseCandidate) -> PermissionEvaluationId {
        let id = PermissionEvaluationId(RawId::new());
        self.issued.insert(id.0, candidate.fingerprint());
        id
    }

    pub fn consume(&mut self, id: &PermissionEvaluationId, expected: &EvalFingerprint) -> bool {
        match self.issued.get(&id.0) {
            Some(bound) if bound == expected => {
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
        BaseViewExpectation, CapabilityKind, CheckLiveAuthorizationQuery, ConsentRecord,
        ConsentRevision, ConsumerKind, DenyCode, EvaluationTracker, InferenceUseCandidate,
        LiveAuthorizationDecision, PurposeKind, base_view_expectation, check_live_authorization,
        consent_mark, consent_view_mark, parse_consent_mark,
    };

    fn candidate() -> InferenceUseCandidate {
        InferenceUseCandidate {
            consumer: ConsumerKind::CompanionDialogue,
            capability: CapabilityKind::Dialogue,
            provider_ref: "acme".to_owned(),
            model: "dialogue-1".to_owned(),
            purpose: PurposeKind::DialogueResponse,
        }
    }

    fn record() -> ConsentRecord {
        record_for(CapabilityKind::Dialogue)
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

    fn query_for(stored: &ConsentRecord) -> CheckLiveAuthorizationQuery {
        CheckLiveAuthorizationQuery {
            candidate: candidate(),
            expected_consent: Some((stored.id.clone(), stored.rev)),
        }
    }

    #[test]
    fn matching_consent_allows_for_exactly_one_use() {
        let stored = record();
        let query = query_for(&stored);
        let mut tracker = EvaluationTracker::new();
        let decision = check_live_authorization(&query, Some(&stored), &mut tracker);
        assert!(matches!(
            decision,
            LiveAuthorizationDecision::AllowForThisUse(_)
        ));
        let LiveAuthorizationDecision::AllowForThisUse(id) = decision else {
            return;
        };
        let fingerprint = candidate().fingerprint();
        assert!(tracker.consume(&id, &fingerprint));
        assert!(!tracker.consume(&id, &fingerprint));
    }

    #[test]
    fn unknown_id_does_not_consume() {
        let stored = record();
        let query = query_for(&stored);
        let mut tracker = EvaluationTracker::new();
        let decision = check_live_authorization(&query, Some(&stored), &mut tracker);
        assert!(matches!(
            decision,
            LiveAuthorizationDecision::AllowForThisUse(_)
        ));
        let LiveAuthorizationDecision::AllowForThisUse(_) = decision else {
            return;
        };
        let mut fresh = EvaluationTracker::new();
        let other = fresh.mint(&candidate());
        assert!(!tracker.consume(&other, &candidate().fingerprint()));
    }

    #[test]
    fn mismatched_fingerprint_rejects_without_burning_the_id() {
        let stored = record();
        let query = query_for(&stored);
        let mut tracker = EvaluationTracker::new();
        let decision = check_live_authorization(&query, Some(&stored), &mut tracker);
        assert!(matches!(
            decision,
            LiveAuthorizationDecision::AllowForThisUse(_)
        ));
        let LiveAuthorizationDecision::AllowForThisUse(id) = decision else {
            return;
        };
        let mut altered = candidate();
        altered.model = "other-model".to_owned();
        assert!(!tracker.consume(&id, &altered.fingerprint()));
        assert!(tracker.consume(&id, &candidate().fingerprint()));
    }

    #[test]
    fn stale_expected_consent_needs_revalidation() {
        let stored = record();
        let query = CheckLiveAuthorizationQuery {
            expected_consent: Some(("consent-1".to_owned(), ConsentRevision::from_u64(1))),
            ..query_for(&stored)
        };
        let mut tracker = EvaluationTracker::new();
        let decision = check_live_authorization(&query, Some(&stored), &mut tracker);
        assert!(matches!(
            decision,
            LiveAuthorizationDecision::NeedsRevalidation
        ));
    }

    #[test]
    fn missing_expected_consent_needs_revalidation_when_stored_exists() {
        let stored = record();
        let query = CheckLiveAuthorizationQuery {
            expected_consent: None,
            ..query_for(&stored)
        };
        let mut tracker = EvaluationTracker::new();
        let decision = check_live_authorization(&query, Some(&stored), &mut tracker);
        assert!(matches!(
            decision,
            LiveAuthorizationDecision::NeedsRevalidation
        ));
    }

    #[test]
    fn provider_mismatch_denies_as_stale_consent() {
        let stored = record();
        let mut off = candidate();
        off.provider_ref = "other".to_owned();
        let query = CheckLiveAuthorizationQuery {
            candidate: off,
            expected_consent: Some((stored.id.clone(), stored.rev)),
        };
        let mut tracker = EvaluationTracker::new();
        let decision = check_live_authorization(&query, Some(&stored), &mut tracker);
        assert!(matches!(decision, LiveAuthorizationDecision::Deny(_)));
        let LiveAuthorizationDecision::Deny(code) = decision else {
            return;
        };
        assert_eq!(code, DenyCode::ConsentStale);
    }

    #[test]
    fn missing_stored_consent_denies_as_stale_consent() {
        let stored = record();
        let query = query_for(&stored);
        let mut tracker = EvaluationTracker::new();
        let decision = check_live_authorization(&query, None, &mut tracker);
        assert!(matches!(decision, LiveAuthorizationDecision::Deny(_)));
        let LiveAuthorizationDecision::Deny(code) = decision else {
            return;
        };
        assert_eq!(code, DenyCode::ConsentStale);
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

    #[test]
    fn learning_formation_requires_a_learning_consent() {
        let stored = record_for(CapabilityKind::Learning);
        let query = CheckLiveAuthorizationQuery {
            candidate: learning_candidate(),
            expected_consent: Some((stored.id.clone(), stored.rev)),
        };
        let mut tracker = EvaluationTracker::new();
        let decision = check_live_authorization(&query, Some(&stored), &mut tracker);
        let LiveAuthorizationDecision::AllowForThisUse(id) = decision else {
            panic!("learning formation must be allowed, got {decision:?}");
        };
        assert!(
            tracker.consume(&id, &learning_candidate().fingerprint()),
            "the learning evaluation is bound to the learning fingerprint"
        );
        assert!(
            !tracker.consume(&id, &candidate().fingerprint()),
            "a dialogue fingerprint must not consume a learning evaluation"
        );
    }

    #[test]
    fn dialogue_consent_never_authorizes_learning() {
        let stored = record();
        let query = CheckLiveAuthorizationQuery {
            candidate: learning_candidate(),
            expected_consent: Some((stored.id.clone(), stored.rev)),
        };
        let mut tracker = EvaluationTracker::new();
        let decision = check_live_authorization(&query, Some(&stored), &mut tracker);
        assert!(
            matches!(
                decision,
                LiveAuthorizationDecision::Deny(code) if code == DenyCode::ConsentStale
            ),
            "a dialogue consent must not authorize learning, got {decision:?}"
        );
    }

    #[test]
    fn dialogue_and_learning_consumers_are_not_interchangeable() {
        let stored = record();
        for mixed in [
            InferenceUseCandidate {
                consumer: ConsumerKind::CompanionDialogue,
                ..learning_candidate()
            },
            InferenceUseCandidate {
                capability: CapabilityKind::Dialogue,
                ..learning_candidate()
            },
            InferenceUseCandidate {
                purpose: PurposeKind::DialogueResponse,
                ..learning_candidate()
            },
            InferenceUseCandidate {
                consumer: ConsumerKind::CompanionLearning,
                ..candidate()
            },
        ] {
            let query = CheckLiveAuthorizationQuery {
                candidate: mixed.clone(),
                expected_consent: Some((stored.id.clone(), stored.rev)),
            };
            let mut tracker = EvaluationTracker::new();
            let decision = check_live_authorization(&query, Some(&stored), &mut tracker);
            assert!(
                matches!(
                    decision,
                    LiveAuthorizationDecision::Deny(code) if code == DenyCode::NotInAllowlist
                ),
                "mixed consumer/capability/purpose must stay outside the closed world: {mixed:?}"
            );
        }
    }

    fn task_agent_candidate() -> InferenceUseCandidate {
        InferenceUseCandidate {
            consumer: ConsumerKind::TaskAgent,
            capability: CapabilityKind::Dialogue,
            provider_ref: "acme".to_owned(),
            model: "dialogue-1".to_owned(),
            purpose: PurposeKind::TaskAgentTurn,
        }
    }

    #[test]
    fn task_agent_turn_inherits_the_dialogue_consent() {
        let stored = record();
        let query = CheckLiveAuthorizationQuery {
            candidate: task_agent_candidate(),
            expected_consent: Some((stored.id.clone(), stored.rev)),
        };
        let mut tracker = EvaluationTracker::new();
        let decision = check_live_authorization(&query, Some(&stored), &mut tracker);
        let LiveAuthorizationDecision::AllowForThisUse(id) = decision else {
            panic!("task agent turn must be allowed under the inherited consent, got {decision:?}");
        };
        assert!(
            tracker.consume(&id, &task_agent_candidate().fingerprint()),
            "the task agent evaluation is bound to the task agent fingerprint"
        );
        assert!(
            !tracker.consume(&id, &candidate().fingerprint()),
            "a dialogue fingerprint must not consume a task agent evaluation"
        );
    }

    #[test]
    fn task_agent_turn_is_not_allowed_with_a_learning_consent() {
        let stored = record_for(CapabilityKind::Learning);
        let query = CheckLiveAuthorizationQuery {
            candidate: task_agent_candidate(),
            expected_consent: Some((stored.id.clone(), stored.rev)),
        };
        let mut tracker = EvaluationTracker::new();
        let decision = check_live_authorization(&query, Some(&stored), &mut tracker);
        assert!(
            matches!(
                decision,
                LiveAuthorizationDecision::Deny(code) if code == DenyCode::ConsentStale
            ),
            "the task agent turn inherits only the dialogue capability consent, got {decision:?}"
        );
    }

    #[test]
    fn task_agent_turn_does_not_masquerade_as_dialogue_or_learning() {
        let stored = record();
        for mixed in [
            InferenceUseCandidate {
                consumer: ConsumerKind::CompanionDialogue,
                ..task_agent_candidate()
            },
            InferenceUseCandidate {
                consumer: ConsumerKind::CompanionLearning,
                ..task_agent_candidate()
            },
            InferenceUseCandidate {
                purpose: PurposeKind::DialogueResponse,
                ..task_agent_candidate()
            },
            InferenceUseCandidate {
                purpose: PurposeKind::MemoryFormation,
                ..task_agent_candidate()
            },
            InferenceUseCandidate {
                capability: CapabilityKind::Learning,
                ..task_agent_candidate()
            },
        ] {
            let query = CheckLiveAuthorizationQuery {
                candidate: mixed.clone(),
                expected_consent: Some((stored.id.clone(), stored.rev)),
            };
            let mut tracker = EvaluationTracker::new();
            let decision = check_live_authorization(&query, Some(&stored), &mut tracker);
            assert!(
                matches!(
                    decision,
                    LiveAuthorizationDecision::Deny(code) if code == DenyCode::NotInAllowlist
                ),
                "a Task Agent turn may only use the explicit task-agent triple: {mixed:?}"
            );
        }
    }

    #[test]
    fn consumer_and_purpose_storage_names_are_closed_world() {
        for consumer in [
            ConsumerKind::CompanionDialogue,
            ConsumerKind::CompanionLearning,
            ConsumerKind::TaskAgent,
        ] {
            assert_eq!(ConsumerKind::from_name(consumer.as_str()), Some(consumer));
        }
        assert_eq!(ConsumerKind::from_name("observer"), None);
        for purpose in [
            PurposeKind::DialogueResponse,
            PurposeKind::MemoryFormation,
            PurposeKind::TaskAgentTurn,
        ] {
            assert_eq!(PurposeKind::from_name(purpose.as_str()), Some(purpose));
        }
        assert_eq!(PurposeKind::from_name("unknown"), None);
    }

    #[test]
    fn base_view_expectation_covers_capability_marks_and_stale_faces() {
        let dialogue = record();
        let learning = record_for(CapabilityKind::Learning);
        assert_eq!(
            base_view_expectation(
                CapabilityKind::Dialogue,
                &consent_view_mark(None, None),
                None
            ),
            BaseViewExpectation::ExpectEmpty
        );
        assert_eq!(
            base_view_expectation(
                CapabilityKind::Dialogue,
                &consent_view_mark(Some(3), Some(1)),
                Some(&dialogue)
            ),
            BaseViewExpectation::ExpectRevision(
                String::from("consent-1"),
                ConsentRevision::from_u64(3)
            )
        );
        assert_eq!(
            base_view_expectation(
                CapabilityKind::Learning,
                &consent_view_mark(Some(3), Some(1)),
                Some(&learning)
            ),
            BaseViewExpectation::ExpectRevision(
                String::from("consent-1"),
                ConsentRevision::from_u64(1)
            )
        );
        assert_eq!(
            base_view_expectation(CapabilityKind::Dialogue, "consent-none", None),
            BaseViewExpectation::ExpectEmpty
        );
        assert_eq!(
            base_view_expectation(CapabilityKind::Dialogue, "consent-rev-3", Some(&dialogue)),
            BaseViewExpectation::ExpectRevision(
                String::from("consent-1"),
                ConsentRevision::from_u64(3)
            )
        );
        assert_eq!(
            base_view_expectation(CapabilityKind::Learning, "consent-rev-3", Some(&learning)),
            BaseViewExpectation::FaceStale,
            "a legacy dialogue mark must never name learning"
        );
        assert_eq!(
            base_view_expectation(CapabilityKind::Dialogue, "consent-rev-2", None),
            BaseViewExpectation::FaceStale
        );
        assert_eq!(
            base_view_expectation(CapabilityKind::Dialogue, "consent-rev-x", Some(&dialogue)),
            BaseViewExpectation::FaceStale
        );
        assert_eq!(
            base_view_expectation(CapabilityKind::Dialogue, "garbage", Some(&dialogue)),
            BaseViewExpectation::FaceStale
        );
    }

    #[test]
    fn mark_helpers_round_trip_both_capabilities() {
        assert_eq!(
            consent_mark(CapabilityKind::Dialogue, None),
            "consent-dialogue-none"
        );
        assert_eq!(
            consent_mark(CapabilityKind::Learning, Some(2)),
            "consent-learning-rev-2"
        );
        let combined = consent_view_mark(Some(3), Some(4));
        assert_eq!(combined, "consent-dialogue-rev-3;consent-learning-rev-4");
        assert_eq!(
            parse_consent_mark(&combined, CapabilityKind::Dialogue),
            Some(Some(3))
        );
        assert_eq!(
            parse_consent_mark(&combined, CapabilityKind::Learning),
            Some(Some(4))
        );
        assert_eq!(
            parse_consent_mark("consent-none", CapabilityKind::Dialogue),
            Some(None)
        );
        assert_eq!(
            parse_consent_mark("consent-none", CapabilityKind::Learning),
            None
        );
        let reordered = "consent-learning-rev-4;consent-dialogue-rev-3";
        assert_eq!(
            parse_consent_mark(reordered, CapabilityKind::Dialogue),
            Some(Some(3))
        );
        assert_eq!(
            parse_consent_mark(reordered, CapabilityKind::Learning),
            Some(Some(4))
        );
    }

    #[test]
    fn revision_exhaustion_is_reported_not_aliased() {
        assert_eq!(
            ConsentRevision::from_u64(0).checked_next(),
            Some(ConsentRevision::from_u64(1))
        );
        assert_eq!(ConsentRevision::from_u64(u64::MAX).checked_next(), None);
    }
}
