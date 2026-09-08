//! Live-authorization contracts: evaluation identity, consent order, and the
//! pure allow/deny policy.
//!
//! This crate defines the types one inference use passes through, plus the
//! closed-world policy that judges it. There is no I/O here: consent records
//! arrive as values, and the single policy function
//! [`check_live_authorization`] decides synchronously.
//!
//! Single-use flow: an `AllowForThisUse` decision carries a fresh
//! [`PermissionEvaluationId`] minted from the caller's [`EvaluationTracker`].
//! The consumer must present that id exactly once (to `ene-inference`
//! dispatch); any replay, unknown id, or fingerprint mismatch is rejected.
//! Ask-owner and wait-for-condition revalidation are deferred: [`RevalidationNeed`]
//! carries only the current consent, never an owner prompt.
//!
//! The `(consumer, capability, purpose)` allowlist below is the closed world
//! for this stage. Future stages may widen it, but only by extending the
//! explicit match in [`check_live_authorization`], never by default-allow.

use std::collections::{HashMap, HashSet};

use ene_primitive::RawId;
use thiserror::Error;

/// Single-use authorization token for one inference use.
///
/// Wraps a [`RawId`] rather than a bare UUID so the opaque-identity
/// discipline of `ene-primitive` applies: no string rendering, no prefix
/// matching, equality only within this newtype.
///
/// Each value is minted by [`EvaluationTracker::mint`] and is valid for one
/// [`EvaluationTracker::consume`] call with the matching [`EvalFingerprint`].
/// Replays always fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PermissionEvaluationId(pub RawId);

/// Opaque identity of one allowlist rule.
///
/// Carried for audit correlation only. Rule bodies do not exist yet: the
/// closed-world policy lives directly in [`check_live_authorization`], and a
/// future stage may resolve a `RuleId` to a stored rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RuleId(pub RawId);

/// Monotonic order of one consent identity's revisions.
///
/// Follows the [`ene_primitive::revision::RevisionInner`] discipline: the
/// inner count travels only inside its `(consent id, revision)` pair, and no
/// bare `u64` revision crosses a public boundary in this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConsentRevision(u64);

impl ConsentRevision {
    /// Reconstitutes a stored revision alongside its consent id.
    #[must_use]
    pub fn from_u64(value: u64) -> Self {
        Self(value)
    }

    /// Returns the stored value for persistence or boundary tokens.
    #[must_use]
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

/// The principal asking to run inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConsumerKind {
    /// The companion dialogue loop acting for the owner.
    CompanionDialogue,
}

/// The capability the consumer wants to exercise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapabilityKind {
    /// General dialogue generation.
    Dialogue,
}

/// The purpose binding one inference use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PurposeKind {
    /// A normal dialogue response turn.
    DialogueResponse,
    /// A setup-time probe checking the route works.
    SetupProbe,
}

/// One proposed inference use, before authorization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferenceUseCandidate {
    /// The principal asking to run inference.
    pub consumer: ConsumerKind,
    /// The capability the consumer wants to exercise.
    pub capability: CapabilityKind,
    /// Provider name as configured (exact match against consent).
    pub provider_ref: String,
    /// Model name as configured (exact match against consent).
    pub model: String,
    /// The purpose binding this use.
    pub purpose: PurposeKind,
}

impl InferenceUseCandidate {
    /// Fingerprint this candidate is tracked under.
    ///
    /// The fingerprint is the full closed-world tuple
    /// `(consumer, capability, provider, model, purpose)`; evaluation ids are
    /// bound to it at mint time and checked at consume time.
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

/// Closed-world fingerprint one evaluation id is bound to.
///
/// Tuple order is `(consumer, capability, provider, model, purpose)`.
/// [`EvaluationTracker`] stores this at mint time and requires an equal
/// value at consume time.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EvalFingerprint(
    /// Consumer bound at mint time.
    pub ConsumerKind,
    /// Capability bound at mint time.
    pub CapabilityKind,
    /// Provider bound at mint time.
    pub String,
    /// Model bound at mint time.
    pub String,
    /// Purpose bound at mint time.
    pub PurposeKind,
);

/// Query for a live authorization check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckLiveAuthorizationQuery {
    /// The proposed use under review.
    pub candidate: InferenceUseCandidate,
    /// Consent the caller acted on, as `(consent id, revision)`.
    ///
    /// `None` means the caller holds no consent view; the decision then
    /// depends on whether stored consent exists (see
    /// [`check_live_authorization`]).
    pub expected_consent: Option<(String, ConsentRevision)>,
    /// Whether Host setup completed.
    ///
    /// `false` denies everything with [`DenyCode::SetupIncomplete`],
    /// regardless of consent state.
    pub setup_complete: bool,
}

/// Outcome of a live authorization check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveAuthorizationDecision {
    /// Allowed for exactly one use under the carried evaluation id.
    AllowForThisUse(PermissionEvaluationId),
    /// Denied; the reason explains which gate failed.
    Deny(DenyReason),
    /// The caller's consent view is stale; re-read and retry.
    ///
    /// Ask-owner and wait-for-condition variants are deliberately absent:
    /// revalidation here means reloading current consent, never prompting.
    NeedsRevalidation(RevalidationNeed),
}

/// Why a live authorization check denied a use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DenyReason {
    /// Machine-readable gate that failed.
    pub code: DenyCode,
    /// Human-readable detail naming the failing gate and values.
    pub detail: String,
}

/// Machine-readable denial gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DenyCode {
    /// Host setup has not completed.
    SetupIncomplete,
    /// The `(consumer, capability, purpose)` triple is outside the closed world.
    NotInAllowlist,
    /// Consent is missing or does not cover this provider/model.
    ConsentStale,
    /// Reserved for a concurrent-supersede signal.
    ///
    /// The current pure policy surfaces replacement as
    /// [`LiveAuthorizationDecision::NeedsRevalidation`] instead; this code
    /// exists so a future mid-flight supersede detector has a stable name.
    Superseded,
}

/// Stale-view signal: the caller must reload current consent and retry.
///
/// This carries only data. Owner prompts and condition waits are deferred to
/// a future stage and must not be smuggled in here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevalidationNeed {
    /// Current stored consent as `(consent id, revision)`.
    pub current_consent: (String, ConsentRevision),
}

/// Stored consent covering one provider/model pair and credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentRecord {
    /// Consent identity; revisions order under this id.
    pub id: String,
    /// Revision of the consent content.
    pub rev: ConsentRevision,
    /// Consented provider name; matched exactly.
    pub provider: String,
    /// Consented model name; matched exactly.
    pub model: String,
    /// Credential the consent is bound to.
    pub credential_id: String,
}

/// Technical failures of the consent store.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PermissionTechnicalError {
    /// The consent store was unreachable or rejected the operation.
    ///
    /// Policy gates never produce this; only store adapters map into it.
    #[error("consent storage unavailable: {reason}")]
    StorageUnavailable {
        /// Backend-supplied cause, without consent content.
        reason: String,
    },
}

/// Persistence boundary for the current consent record.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait ConsentRepository: Send + Sync {
    /// Loads the current consent record, if any.
    async fn load_current(&self) -> Result<Option<ConsentRecord>, PermissionTechnicalError>;

    /// Persists the current consent record, replacing any previous one.
    async fn save_current(&self, record: ConsentRecord) -> Result<(), PermissionTechnicalError>;
}

/// Tracks minted evaluation ids and enforces single use.
///
/// Holds the issued `RawId -> EvalFingerprint` map plus the set of consumed
/// ids. Minting binds an id to a candidate fingerprint; consuming requires
/// the same fingerprint and succeeds at most once per id.
#[derive(Debug, Default)]
pub struct EvaluationTracker {
    /// Fingerprint each issued id was minted for.
    issued: HashMap<RawId, EvalFingerprint>,
    /// Ids already consumed; replays are rejected.
    consumed: HashSet<RawId>,
}

impl EvaluationTracker {
    /// Creates an empty tracker holding no issued ids.
    #[must_use]
    pub fn new() -> Self {
        Self {
            issued: HashMap::new(),
            consumed: HashSet::new(),
        }
    }

    /// Mints a fresh single-use id bound to the candidate's fingerprint.
    pub fn mint(&mut self, candidate: &InferenceUseCandidate) -> PermissionEvaluationId {
        let id = PermissionEvaluationId(RawId::new());
        self.issued.insert(id.0, candidate.fingerprint());
        id
    }

    /// Consumes an id iff it is known, unused, and bound to `expected`.
    ///
    /// Returns `false` for unknown ids, replays, and fingerprint mismatches.
    /// Only a matching presentation burns the id, so a caller that
    /// misconstructed the fingerprint can retry with the correct one, while
    /// a successful consume can never be replayed.
    pub fn consume(&mut self, id: &PermissionEvaluationId, expected: &EvalFingerprint) -> bool {
        match self.issued.get(&id.0) {
            None => false,
            Some(bound) => {
                if bound != expected || self.consumed.contains(&id.0) {
                    return false;
                }
                self.consumed.insert(id.0);
                true
            }
        }
    }
}

/// Pure closed-world policy for one live authorization query.
///
/// Gates, in order:
///
/// 1. `setup_complete == false` denies with [`DenyCode::SetupIncomplete`].
/// 2. A `(consumer, capability, purpose)` triple outside the closed world
///    denies with [`DenyCode::NotInAllowlist`]. The current world is
///    `(CompanionDialogue, Dialogue, DialogueResponse | SetupProbe)`.
/// 3. Consent comparison: when stored consent exists and differs from
///    `expected_consent`, the caller's view is stale and the decision is
///    [`LiveAuthorizationDecision::NeedsRevalidation`] carrying the stored
///    `(id, revision)`.
/// 4. With no stored consent, or a provider/model mismatch against the stored
///    record, the decision denies with [`DenyCode::ConsentStale`].
/// 5. Otherwise the candidate is allowed for exactly one use: a fresh id is
///    minted from `tracker` and returned in
///    [`LiveAuthorizationDecision::AllowForThisUse`].
///
/// This function performs no I/O; the caller supplies the stored record.
pub fn check_live_authorization(
    query: &CheckLiveAuthorizationQuery,
    current: Option<&ConsentRecord>,
    tracker: &mut EvaluationTracker,
) -> LiveAuthorizationDecision {
    if !query.setup_complete {
        return LiveAuthorizationDecision::Deny(DenyReason {
            code: DenyCode::SetupIncomplete,
            detail: "setup has not completed".to_owned(),
        });
    }
    let in_allowlist = matches!(
        (
            query.candidate.consumer,
            query.candidate.capability,
            query.candidate.purpose
        ),
        (
            ConsumerKind::CompanionDialogue,
            CapabilityKind::Dialogue,
            PurposeKind::DialogueResponse | PurposeKind::SetupProbe
        )
    );
    if !in_allowlist {
        return LiveAuthorizationDecision::Deny(DenyReason {
            code: DenyCode::NotInAllowlist,
            detail: "consumer, capability, and purpose are outside the closed world".to_owned(),
        });
    }
    let Some(record) = current else {
        return LiveAuthorizationDecision::Deny(DenyReason {
            code: DenyCode::ConsentStale,
            detail: "no current consent is stored".to_owned(),
        });
    };
    let current_view = (record.id.clone(), record.rev);
    let stale_view = query.expected_consent.as_ref() != Some(&current_view);
    if stale_view {
        return LiveAuthorizationDecision::NeedsRevalidation(RevalidationNeed {
            current_consent: current_view,
        });
    }
    if query.candidate.provider_ref != record.provider || query.candidate.model != record.model {
        let provider = query.candidate.provider_ref.as_str();
        let model = query.candidate.model.as_str();
        return LiveAuthorizationDecision::Deny(DenyReason {
            code: DenyCode::ConsentStale,
            detail: format!("candidate route {provider}/{model} is not covered by current consent"),
        });
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
        ConsentRecord {
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
            setup_complete: true,
        }
    }

    #[test]
    fn revision_round_trips_through_u64() {
        assert_eq!(ConsentRevision::from_u64(7).as_u64(), 7);
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
    fn incomplete_setup_denies_before_consent() {
        let stored = record();
        let query = CheckLiveAuthorizationQuery {
            setup_complete: false,
            ..query_for(&stored)
        };
        let mut tracker = EvaluationTracker::new();
        let decision = check_live_authorization(&query, Some(&stored), &mut tracker);
        assert!(matches!(decision, LiveAuthorizationDecision::Deny(_)));
        let LiveAuthorizationDecision::Deny(reason) = decision else {
            return;
        };
        assert_eq!(reason.code, DenyCode::SetupIncomplete);
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
            LiveAuthorizationDecision::NeedsRevalidation(_)
        ));
        let LiveAuthorizationDecision::NeedsRevalidation(need) = decision else {
            return;
        };
        assert_eq!(need.current_consent, ("consent-1".to_owned(), stored.rev));
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
            LiveAuthorizationDecision::NeedsRevalidation(_)
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
            setup_complete: true,
        };
        let mut tracker = EvaluationTracker::new();
        let decision = check_live_authorization(&query, Some(&stored), &mut tracker);
        assert!(matches!(decision, LiveAuthorizationDecision::Deny(_)));
        let LiveAuthorizationDecision::Deny(reason) = decision else {
            return;
        };
        assert_eq!(reason.code, DenyCode::ConsentStale);
    }

    #[test]
    fn missing_stored_consent_denies_as_stale_consent() {
        let stored = record();
        let query = query_for(&stored);
        let mut tracker = EvaluationTracker::new();
        let decision = check_live_authorization(&query, None, &mut tracker);
        assert!(matches!(decision, LiveAuthorizationDecision::Deny(_)));
        let LiveAuthorizationDecision::Deny(reason) = decision else {
            return;
        };
        assert_eq!(reason.code, DenyCode::ConsentStale);
    }

    #[test]
    fn setup_probe_purpose_is_within_the_closed_world() {
        let stored = record();
        let mut probe = candidate();
        probe.purpose = PurposeKind::SetupProbe;
        let query = CheckLiveAuthorizationQuery {
            candidate: probe,
            expected_consent: Some((stored.id.clone(), stored.rev)),
            setup_complete: true,
        };
        let mut tracker = EvaluationTracker::new();
        let decision = check_live_authorization(&query, Some(&stored), &mut tracker);
        assert!(matches!(
            decision,
            LiveAuthorizationDecision::AllowForThisUse(_)
        ));
    }
}
