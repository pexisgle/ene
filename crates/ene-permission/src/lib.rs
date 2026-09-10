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

#![cfg_attr(
    test,
    allow(
        clippy::expect_used,
        clippy::unwrap_used,
        clippy::panic,
        reason = "test fixtures may unwrap values whose failure would be a fixture bug"
    )
)]

use std::collections::HashMap;

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

/// Durable intent replay for management intents.
///
/// Design §18.2 fixes `intent_id` as the idempotency key: a transport retry
/// carries the same id, and a new judgment mints a new one. The store binds
/// each decided outcome to the intent fingerprint; a later send with the
/// same intent id either replays the stored snapshot verbatim — never
/// re-executed — or conflicts (same id, different content, answered without
/// side effects). Re-evaluation always means a new id: even a stale or
/// clarifying answer replays under its own id, so the same key can never
/// observe two different outcomes.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait IntentOutcomeRepository: Send + Sync {
    /// Records one intent's outcome snapshot write-once.
    ///
    /// The intent row is immutable: a second write under the same id never
    /// overwrites. Returns how the claim resolved so the caller answers
    /// from the durable determination, never from a locally decided outcome
    /// the store did not keep.
    async fn record_intent_outcome(
        &self,
        record: IntentOutcomeRecord,
    ) -> Result<IntentResolution<()>, PermissionTechnicalError>;

    /// Loads the outcome snapshot for `intent_id`, if any.
    async fn lookup_intent_outcome(
        &self,
        intent_id: &str,
    ) -> Result<Option<IntentOutcomeRecord>, PermissionTechnicalError>;

    /// Assigns the consent route and records the intent outcome atomically.
    ///
    /// One transaction: check the intent key first, then compare-and-save
    /// plus the replay-row insert. An existing row is never rewritten — an
    /// exact fingerprint replays the stored snapshot, a conflicting one
    /// clarifies — so concurrent same-id sends cannot fork the answer and a
    /// crash between commit and marker can neither strand an approval
    /// without its replay row nor replay a row without its commit. Commits
    /// record the `Stored` snapshot built from the committed revision;
    /// stale attempts record the `Stale` snapshot with the current mark.
    /// Replay answers either verbatim.
    async fn assign_with_intent(
        &self,
        expected: Option<(String, ConsentRevision)>,
        record: ConsentRecord,
        fingerprint: IntentFingerprint,
    ) -> Result<IntentResolution<ConsentCommitOutcome>, PermissionTechnicalError>;

    /// Registers the credential approval request and records the intent
    /// outcome atomically.
    ///
    /// One transaction: check the intent key first, then the pending insert
    /// (or usable recheck) plus the replay-row insert. An existing row is
    /// never rewritten. Returns the resolution — `Decided` carrying the
    /// snapshot that was just stored (`Held` when the pair now pends
    /// approval, `Applied` when it is already usable), `Replay` carrying
    /// the prior snapshot, or `Conflict` — so the caller answers from one
    /// durable determination.
    async fn request_approval_with_intent(
        &self,
        provider: String,
        label: String,
        fingerprint: IntentFingerprint,
    ) -> Result<IntentResolution<IntentOutcomeRecord>, PermissionTechnicalError>;

    /// Claims a setup completion and records its outcome atomically.
    ///
    /// One transaction: check the intent key first, then compare the
    /// expected base mark against current and record the decided snapshot
    /// together — `Applied` when the base matches, a row exists, and the
    /// bearer is present; `Clarify` when the premise is empty or the bearer
    /// is absent; `Stale` (with the current mark) when the base moved. An
    /// existing row is never rewritten: exact replays and conflicts return
    /// the stored snapshot instead. Returns the resolution so the caller
    /// answers from one durable determination. The bearer gate rides in as
    /// a flag because completion means consent-plus-bearer; it is
    /// Host-observed just before the call, and the transaction re-verifies
    /// everything durable around it.
    async fn complete_with_intent(
        &self,
        expected_base: String,
        bearer_present: bool,
        fingerprint: IntentFingerprint,
    ) -> Result<IntentResolution<IntentOutcomeRecord>, PermissionTechnicalError>;

    /// Claims a same-route shortcut and records its outcome atomically.
    ///
    /// One transaction: check the intent key first, then read current, and
    /// — only when the stored route already equals the requested one —
    /// insert the `Stored` snapshot for the current revision. An existing
    /// row is never rewritten. Returns `Decided(Hit)` (recorded, answer the
    /// current revision without bumping), `Decided(Miss)` (nothing recorded;
    /// the caller continues through compare-and-save), or the stored row on
    /// replay/conflict. State-changing assigns still go through
    /// [`IntentOutcomeRepository::assign_with_intent`].
    async fn shortcut_with_intent(
        &self,
        provider: String,
        model: String,
        credential_id: String,
        fingerprint: IntentFingerprint,
    ) -> Result<IntentResolution<ShortcutIntentOutcome>, PermissionTechnicalError>;
}

/// Result of a write-once intent claim: either this call decided, or an
/// earlier row already did.
///
/// Every deciding operation checks the intent key first inside its
/// transaction. The row is immutable: a second write under the same id —
/// same content or not — never overwrites, so concurrent same-id sends
/// cannot fork the answer and a crash between decision and marker is
/// impossible by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentResolution<T> {
    /// No row existed; the fresh decision `T` was stored write-once.
    Decided(T),
    /// Exact fingerprint replay; nothing changed.
    Replay(IntentOutcomeRecord),
    /// Same id, different fingerprint; nothing changed.
    Conflict(IntentOutcomeRecord),
}

/// Durable fingerprint of one management intent: the intent key plus the
/// content it decides on. The base premise and the semantically effective
/// rationale ride along, so a refreshed premise under a reused id counts
/// as different content (new premise, new id — same rule as command keys).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IntentFingerprint {
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
}

/// Durable terminal outcome snapshot of one management intent: its
/// fingerprint plus the outcome that content produced. An exact retry
/// (same id, same fingerprint) replays the snapshot verbatim — never
/// re-executed, never rebound; the same id with different content is a
/// conflict the caller clarifies.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IntentOutcomeRecord {
    /// What the intent asked.
    pub fingerprint: IntentFingerprint,
    /// Terminal outcome snapshot.
    pub outcome: IntentOutcome,
}

/// Management outcome snapshot worth replaying.
///
/// Every decided outcome is recorded — including stale and clarifying
/// answers. A retried id must observe the same answer it observed before;
/// only a fresh id earns a fresh evaluation. (Infrastructure failures are
/// not decisions: an unreadable store holds without recording.)
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
    /// Too ambiguous or contradictory to decide.
    NeedsClarification,
    /// The base view had moved underneath the intent.
    StaleBaseView {
        /// Current mark the sender should build on next time.
        current: String,
    },
}

/// Outcome of a same-route shortcut claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutIntentOutcome {
    /// Route already holds: snapshot recorded, answer the current record.
    Hit {
        /// Current consent record behind the answer.
        current: ConsentRecord,
    },
    /// Route differs: answer through the normal path. Nothing recorded.
    Miss {
        /// Current consent record, if any, for the caller to continue with.
        current: Option<ConsentRecord>,
    },
}

/// Renders the base-view mark for a consent revision: `consent-none` when
/// absent, `consent-rev-N` otherwise.
///
/// Single grammar owner for base-view marks: the store renders replay
/// snapshots with it and the Host renders live answers with it, so the two
/// can never disagree on what a mark names.
#[must_use]
pub fn consent_mark_rev(rev: Option<u64>) -> String {
    match rev {
        Some(number) => format!("consent-rev-{number}"),
        None => String::from("consent-none"),
    }
}
/// Tracks minted evaluation ids and enforces single use.
///
/// Holds the issued `RawId -> EvalFingerprint` map only: a successful
/// consume removes the entry, so presence means unused and absence means
/// unknown or already consumed. Minting binds an id to a candidate
/// fingerprint; consuming requires the same fingerprint and succeeds at
/// most once per id.
#[derive(Debug, Default)]
pub struct EvaluationTracker {
    /// Fingerprint each live issued id was minted for. A successful
    /// [`consume`](EvaluationTracker::consume) removes the entry, so one
    /// map carries both issuance and single-use state: present means
    /// unused, absent means unknown or already consumed.
    issued: HashMap<RawId, EvalFingerprint>,
}

impl EvaluationTracker {
    /// Creates an empty tracker holding no issued ids.
    #[must_use]
    pub fn new() -> Self {
        Self {
            issued: HashMap::new(),
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
    /// Only a matching presentation burns the id — removing it, so a second
    /// consume finds nothing — while a fingerprint mismatch leaves the entry
    /// so the caller can retry with the correct fingerprint.
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
