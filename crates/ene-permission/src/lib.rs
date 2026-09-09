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

/// Outcome of a compare-and-save consent commit.
///
/// An `Ok`-side domain outcome, never an error. Stale expectations return
/// `StaleCurrent` and are never retried automatically; the caller re-reads
/// and retries with a fresh expectation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsentCommitOutcome {
    /// The expected view matched; `record` was stored.
    Committed {
        /// Stored consent record.
        record: ConsentRecord,
    },
    /// The expected view did not match; nothing was stored.
    StaleCurrent {
        /// Current stored record, or `None` when no consent is stored.
        current: Option<ConsentRecord>,
    },
}

/// Persistence boundary for the current consent record.
///
/// Writes use compare-and-save: the caller passes the consent view its
/// intent was built on, and the store commits only when that view is still
/// current. This closes the lost-update window where two intents read the
/// same revision and the second silently overwrites the first.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait ConsentRepository: Send + Sync {
    /// Loads the current consent record, if any.
    async fn load_current(&self) -> Result<Option<ConsentRecord>, PermissionTechnicalError>;

    /// Commits `record` iff `expected` still matches the stored view.
    ///
    /// `expected` comes from the intent's base-view mark as parsed by the
    /// caller: a `consent-rev-N` mark carries `Some((consent id, revision))`
    /// and a `consent-none` mark carries `None`.
    ///
    /// `None` with an existing row returns `StaleCurrent` and never
    /// overwrites; `Some((id, rev))` with a missing row or a differing id or
    /// revision returns `StaleCurrent`; a match stores `record` and returns
    /// `Committed`. Callers map `StaleCurrent` to `StaleBaseView` at ingress,
    /// re-read, and retry.
    async fn compare_and_save(
        &self,
        expected: Option<(String, ConsentRevision)>,
        record: ConsentRecord,
    ) -> Result<ConsentCommitOutcome, PermissionTechnicalError>;
}

/// Durable intent replay for consent assignment.
///
/// Design §18.2 fixes `intent_id` as the idempotency key for management
/// intents. The store binds each committed (or shortcut-succeeded) assign
/// to the intent fingerprint `(target, base)`; a later send with the same
/// intent id either replays (same fingerprint and the route still holds)
/// or conflicts (same id, different content — never rebound). Only the
/// assign path writes and reads these rows: register is propose-only and
/// naturally convergent (`Held` → `Applied` as approval lands), and
/// completion re-derives from state, so neither needs replay rows. A lost
/// reply therefore converges without a fresh intent id, while a reused id
/// with new meaning is declined instead of silently adopting it.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait IntentOutcomeRepository: Send + Sync {
    /// Records one intent's terminal outcome snapshot.
    ///
    /// Upserts on `intent_id`: re-recording refreshes rather than
    /// duplicating. For read-only outcomes (shortcut successes, completion
    /// checks, already-usable registers) this own transaction is atomic
    /// enough — nothing else commits alongside. For state-changing outcomes
    /// use the combined operations below so the commit and its replay row
    /// share one transaction.
    async fn record_intent_outcome(
        &self,
        record: IntentOutcomeRecord,
    ) -> Result<(), PermissionTechnicalError>;

    /// Loads the outcome snapshot for `intent_id`, if any.
    async fn lookup_intent_outcome(
        &self,
        intent_id: &str,
    ) -> Result<Option<IntentOutcomeRecord>, PermissionTechnicalError>;

    /// Assigns the consent route and records the intent outcome atomically.
    ///
    /// One transaction: the compare-and-save plus the replay-row insert, so
    /// a crash between commit and marker can neither strand an approval
    /// without its replay row nor replay a row without its commit. Records
    /// only on [`ConsentCommitOutcome::Committed`]; stale outcomes carry no
    /// row (they recompute honestly on retry). The snapshot carries the
    /// committed revision; replay answers it verbatim.
    async fn assign_with_intent(
        &self,
        expected: Option<(String, ConsentRevision)>,
        record: ConsentRecord,
        intent: IntentOutcomeRecord,
    ) -> Result<ConsentCommitOutcome, PermissionTechnicalError>;

    /// Registers the credential approval request and records the intent
    /// outcome atomically.
    ///
    /// One transaction: the pending insert (or usable recheck) plus the
    /// replay-row insert. Returns the decided snapshot — `Held` when the
    /// pair now pends approval, `Applied` when it is already usable — so
    /// the caller answers from one durable determination.
    async fn request_approval_with_intent(
        &self,
        provider: String,
        label: String,
        intent: IntentOutcomeRecord,
    ) -> Result<IntentOutcomeRecord, PermissionTechnicalError>;
}

/// Durable fingerprint plus terminal outcome snapshot of one management
/// intent: the intent key, the content it decided on, and the outcome that
/// content produced. An exact retry (same id, same fingerprint) replays the
/// snapshot verbatim — never re-executed, never rebound; the same id with
/// different content is a conflict the caller clarifies. The base premise
/// and the semantically effective rationale ride along, so a refreshed
/// premise under a reused id counts as different content (new premise, new
/// id — same rule as command keys).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IntentOutcomeRecord {
    /// Intent key, as hyphenated UUID text.
    pub intent_id: String,
    /// Intent kind discriminator (`assign`, `register`, or `complete`).
    pub kind: String,
    /// Intent target text.
    pub target: String,
    /// Base-view mark text the intent was built on.
    pub base: String,
    /// Rationale origin text (`conversation` or `management-surface`).
    pub rationale_origin: String,
    /// Rationale quote, if the intent carried one.
    pub rationale_quote: Option<String>,
    /// Terminal outcome snapshot.
    pub outcome: IntentOutcome,
}

/// Terminal management outcome worth replaying.
///
/// Only content-terminal outcomes are recorded: non-terminal answers
/// (`StaleBaseView`, `NeedsClarification`, `DeniedByBoundary`) recompute
/// honestly on retry — replaying a stale mark would send the caller to
/// build on it again instead of converging.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IntentOutcome {
    /// Stored as a rule at this revision view.
    StoredAsRuleView {
        /// Revision view committed by this intent.
        revision: String,
    },
    /// Applied as a one-time approval.
    AppliedAsOneTime,
    /// Held for a pending Owner decision.
    HeldByOperation,
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
        CapabilityKind, CheckLiveAuthorizationQuery, ConsentCommitOutcome, ConsentRecord,
        ConsentRepository, ConsentRevision, ConsumerKind, DenyCode, EvaluationTracker,
        InferenceUseCandidate, LiveAuthorizationDecision, PermissionTechnicalError, PurposeKind,
        check_live_authorization,
    };
    use std::sync::Mutex;

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

    fn block_on<Fut>(future: Fut) -> Fut::Output
    where
        Fut: core::future::Future,
    {
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        let mut pinned = std::pin::pin!(future);
        loop {
            match pinned.as_mut().poll(&mut context) {
                core::task::Poll::Ready(value) => return value,
                core::task::Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    struct FakeConsentRepository {
        current: Mutex<Option<ConsentRecord>>,
    }

    impl FakeConsentRepository {
        fn new(initial: Option<ConsentRecord>) -> Self {
            Self {
                current: Mutex::new(initial),
            }
        }

        fn stored(&self) -> Option<ConsentRecord> {
            match self.current.lock() {
                Ok(guard) => guard.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            }
        }
    }

    impl ConsentRepository for FakeConsentRepository {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake holds a mutex; async matches the repository contract"
        )]
        async fn load_current(&self) -> Result<Option<ConsentRecord>, PermissionTechnicalError> {
            Ok(self.stored())
        }

        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake holds a mutex; async matches the repository contract"
        )]
        async fn compare_and_save(
            &self,
            expected: Option<(String, ConsentRevision)>,
            record: ConsentRecord,
        ) -> Result<ConsentCommitOutcome, PermissionTechnicalError> {
            let mut guard = match self.current.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let matches = match (&expected, &*guard) {
                (None, None) => true,
                (Some((expected_id, expected_rev)), Some(current)) => {
                    expected_id == &current.id && expected_rev == &current.rev
                }
                (None, Some(_)) | (Some(_), None) => false,
            };
            if matches {
                *guard = Some(record.clone());
                Ok(ConsentCommitOutcome::Committed { record })
            } else {
                Ok(ConsentCommitOutcome::StaleCurrent {
                    current: guard.clone(),
                })
            }
        }
    }

    fn revised_record(rev: u64) -> ConsentRecord {
        ConsentRecord {
            id: "consent-1".to_owned(),
            rev: ConsentRevision::from_u64(rev),
            provider: "acme".to_owned(),
            model: "dialogue-1".to_owned(),
            credential_id: "cred-1".to_owned(),
        }
    }

    #[test]
    fn compare_and_save_commits_on_match() {
        let repo = FakeConsentRepository::new(Some(revised_record(3)));
        let next = revised_record(4);
        let outcome = block_on(repo.compare_and_save(
            Some(("consent-1".to_owned(), ConsentRevision::from_u64(3))),
            next.clone(),
        ));
        assert!(outcome.is_ok());
        let Ok(outcome) = outcome else {
            return;
        };
        assert_eq!(
            outcome,
            ConsentCommitOutcome::Committed {
                record: next.clone()
            }
        );
        assert_eq!(repo.stored(), Some(next));
    }

    #[test]
    fn compare_and_save_commits_on_empty_match() {
        let repo = FakeConsentRepository::new(None);
        let next = revised_record(1);
        let outcome = block_on(repo.compare_and_save(None, next.clone()));
        assert!(outcome.is_ok());
        let Ok(outcome) = outcome else {
            return;
        };
        assert_eq!(
            outcome,
            ConsentCommitOutcome::Committed {
                record: next.clone()
            }
        );
        assert_eq!(repo.stored(), Some(next));
    }

    #[test]
    fn compare_and_save_is_stale_on_rev_mismatch() {
        let stored = revised_record(3);
        let repo = FakeConsentRepository::new(Some(stored.clone()));
        let next = revised_record(4);
        let outcome = block_on(repo.compare_and_save(
            Some(("consent-1".to_owned(), ConsentRevision::from_u64(1))),
            next,
        ));
        assert!(outcome.is_ok());
        let Ok(outcome) = outcome else {
            return;
        };
        assert_eq!(
            outcome,
            ConsentCommitOutcome::StaleCurrent {
                current: Some(stored.clone()),
            }
        );
        assert_eq!(repo.stored(), Some(stored));
    }

    #[test]
    fn compare_and_save_is_stale_on_unexpected_existing() {
        let stored = revised_record(3);
        let repo = FakeConsentRepository::new(Some(stored.clone()));
        let next = revised_record(4);
        let outcome = block_on(repo.compare_and_save(None, next));
        assert!(outcome.is_ok());
        let Ok(outcome) = outcome else {
            return;
        };
        assert_eq!(
            outcome,
            ConsentCommitOutcome::StaleCurrent {
                current: Some(stored.clone()),
            }
        );
        assert_eq!(repo.stored(), Some(stored));
    }

    #[test]
    fn compare_and_save_is_stale_on_id_mismatch() {
        let stored = revised_record(3);
        let repo = FakeConsentRepository::new(Some(stored.clone()));
        let next = revised_record(3);
        let outcome = block_on(repo.compare_and_save(
            Some(("consent-2".to_owned(), ConsentRevision::from_u64(3))),
            next,
        ));
        assert!(outcome.is_ok());
        let Ok(outcome) = outcome else {
            return;
        };
        assert_eq!(
            outcome,
            ConsentCommitOutcome::StaleCurrent {
                current: Some(stored.clone()),
            }
        );
        assert_eq!(repo.stored(), Some(stored));
    }

    #[test]
    fn compare_and_save_is_stale_on_expected_but_empty() {
        let repo = FakeConsentRepository::new(None);
        let next = revised_record(1);
        let outcome = block_on(repo.compare_and_save(
            Some(("consent-1".to_owned(), ConsentRevision::from_u64(1))),
            next.clone(),
        ));
        assert!(outcome.is_ok());
        let Ok(outcome) = outcome else {
            return;
        };
        assert_eq!(
            outcome,
            ConsentCommitOutcome::StaleCurrent { current: None }
        );
        assert_eq!(repo.stored(), None);
    }
}
