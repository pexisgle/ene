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
//! Ask-owner and wait-for-condition revalidation are deferred:
//! [`LiveAuthorizationDecision::NeedsRevalidation`] only means reload current
//! consent and retry, never prompt.
//!
//! The `(consumer, capability, purpose)` allowlist below is the closed world
//! for this stage. Future stages may widen it, but only by extending the
//! explicit match in [`check_live_authorization`], never by default-allow.

mod intent;

use std::collections::HashMap;

use ene_primitive::{RawId, RevisionInner};
use thiserror::Error;

pub use intent::{
    AssignConsentIntent, AssignConsentResolution, BaseViewExpectation, assign_consent,
    base_view_expectation,
};

/// Single-use authorization token for one inference use.
///
/// Wraps a [`RawId`] rather than a bare UUID so the opaque-identity
/// discipline of `ene-primitive` applies: no string rendering, no prefix
/// matching, equality only within this newtype. A value is valid for one
/// [`EvaluationTracker::consume`] call with the matching [`EvalFingerprint`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PermissionEvaluationId(pub RawId);

/// Monotonic order of one consent identity's revisions.
///
/// Follows the [`RevisionInner`] discipline: the inner count travels only
/// inside its `(consent id, revision)` pair, no bare `u64` revision crosses a
/// public boundary in this crate, and [`Self::checked_next`] reports
/// exhaustion instead of aliasing `u64::MAX`, so a new revision can never
/// silently share the previous one's value.
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

    /// Callers must treat [`None`] as revision exhaustion and refuse the
    /// commit rather than writing [`u64::MAX`] again with new content.
    #[must_use]
    pub fn checked_next(&self) -> Option<Self> {
        self.0.checked_next().map(Self)
    }
}

/// The principal asking to run inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConsumerKind {
    /// The companion dialogue loop acting for the owner.
    CompanionDialogue,
    /// The learning formation pass acting for one companion.
    CompanionLearning,
}

/// The capability the consumer wants to exercise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapabilityKind {
    /// General dialogue generation.
    Dialogue,
    /// Experience Summary and Memory formation judgement.
    Learning,
}

impl CapabilityKind {
    /// Stable wire and storage name. One owner for the vocabulary: the
    /// management grammar, the consent table, and the inference attempt all
    /// render capabilities through this function.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dialogue => "dialogue",
            Self::Learning => "learning",
        }
    }

    /// Parses the [`Self::as_str`] vocabulary, closed world.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "dialogue" => Some(Self::Dialogue),
            "learning" => Some(Self::Learning),
            _ => None,
        }
    }
}

/// The purpose binding one inference use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PurposeKind {
    /// A normal dialogue response turn.
    DialogueResponse,
    /// Form one Experience Summary and its Memory changes.
    MemoryFormation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferenceUseCandidate {
    pub consumer: ConsumerKind,
    pub capability: CapabilityKind,
    /// Provider name as configured (exact match against consent).
    pub provider_ref: String,
    /// Model name as configured (exact match against consent).
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

/// Closed-world fingerprint one evaluation id is bound to.
///
/// Tuple order is `(consumer, capability, provider, model, purpose)`.
/// [`EvaluationTracker`] stores this at mint time and requires an equal
/// value at consume time.
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
    /// Consent the caller acted on, as `(consent id, revision)`.
    ///
    /// `None` means the caller holds no consent view; the decision then
    /// depends on whether stored consent exists (see
    /// [`check_live_authorization`]).
    pub expected_consent: Option<(String, ConsentRevision)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveAuthorizationDecision {
    /// Allowed for exactly one use under the carried evaluation id.
    AllowForThisUse(PermissionEvaluationId),
    Deny(DenyCode),
    /// The caller's consent view is stale; reload current consent and retry.
    ///
    /// Ask-owner and wait-for-condition variants are deliberately absent:
    /// revalidation here means reloading current consent, never prompting.
    NeedsRevalidation,
}

/// Why one live-authorization query was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DenyCode {
    /// The `(consumer, capability, purpose)` triple is outside the closed world.
    NotInAllowlist,
    /// Consent is missing or does not cover this provider/model.
    ConsentStale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentRecord {
    /// Capability this assignment authorizes. One consent record exists per
    /// capability, and a record never authorizes another capability even
    /// when provider, model, and credential coincide.
    pub capability: CapabilityKind,
    /// Consent identity; revisions order under this id.
    pub id: String,
    pub rev: ConsentRevision,
    /// Provider name; matched exactly.
    pub provider: String,
    /// Model name; matched exactly.
    pub model: String,
    pub credential_id: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PermissionTechnicalError {
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
    Committed { record: ConsentRecord },
    StaleCurrent { current: Option<ConsentRecord> },
}

/// Read boundary for the current consent record.
///
/// This trait loads; writes go through
/// [`IntentOutcomeRepository::assign_with_intent`], which commits only when
/// the caller's base-view expectation still matches and records the decision
/// atomically with the write. That closes the lost-update window where two
/// intents read the same revision and the second silently overwrites the
/// first.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait ConsentRepository: Send + Sync {
    /// Loads the current consent for one capability.
    ///
    /// Capabilities never share a record: a dialogue assignment does not
    /// authorize learning, and vice versa, even when the stored route
    /// (provider, model, credential) is identical.
    async fn load_current(
        &self,
        capability: CapabilityKind,
    ) -> Result<Option<ConsentRecord>, PermissionTechnicalError>;
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
    /// One transaction: check the intent key first, then read current for
    /// `capability`, and — only when the stored route already equals the
    /// requested one — insert the `Stored` snapshot for the current revision.
    /// An existing row is never rewritten. Returns `Decided(Hit)` (recorded,
    /// answer the current revision without bumping), `Decided(Miss)` (nothing
    /// recorded; the caller continues through compare-and-save), or the
    /// stored row on replay/conflict. State-changing assigns still go through
    /// [`IntentOutcomeRepository::assign_with_intent`].
    async fn shortcut_with_intent(
        &self,
        capability: CapabilityKind,
        provider: String,
        model: String,
        credential_id: String,
        fingerprint: IntentFingerprint,
    ) -> Result<IntentResolution<ShortcutIntentOutcome>, PermissionTechnicalError>;
}

/// Result of a write-once intent claim: either this call decided, or an
/// earlier row already did.
///
/// The row is immutable: a second write under the same id — same content or
/// not — never overwrites.
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
    pub target: String,
    /// Base-view mark text the intent was built on.
    pub base: String,
    /// Rationale origin text (`conversation` or `management-surface`).
    pub rationale_origin: String,
    pub rationale_quote: Option<String>,
}

/// Durable terminal outcome snapshot of one management intent: its
/// fingerprint plus the outcome that content produced. An exact retry
/// (same id, same fingerprint) replays the snapshot verbatim — never
/// re-executed, never rebound; the same id with different content is a
/// conflict the caller clarifies.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IntentOutcomeRecord {
    pub fingerprint: IntentFingerprint,
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
    StoredAsRuleView { revision: String },
    /// Applied as a one-time approval.
    AppliedAsOneTime,
    /// Held for a pending Owner decision.
    HeldByOperation,
    /// Too ambiguous or contradictory to decide.
    NeedsClarification,
    /// The consent identity ran out of distinct revisions: committing again
    /// would reuse `u64::MAX` with new content, so nothing was written.
    RevisionExhausted,
    /// The base view had moved underneath the intent.
    StaleBaseView {
        /// Current mark the sender should build on next time.
        current: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutIntentOutcome {
    /// Route already holds: snapshot recorded, answer the current record.
    Hit { current: ConsentRecord },
    /// Route differs: answer through the normal path. Nothing recorded.
    Miss,
}

/// Renders one capability's consent state as a mark segment:
/// `consent-{capability}-none` when absent, `consent-{capability}-rev-N`
/// otherwise.
///
/// Single grammar owner for capability marks: the store renders replay
/// snapshots with it and the Host renders live answers with it, so the two
/// can never disagree on what a mark names.
#[must_use]
pub fn consent_mark(capability: CapabilityKind, rev: Option<u64>) -> String {
    let name = capability.as_str();
    match rev {
        Some(number) => format!("consent-{name}-rev-{number}"),
        None => format!("consent-{name}-none"),
    }
}

/// Renders the combined management view mark for both capabilities:
/// `consent-dialogue-...;consent-learning-...`.
///
/// The mark stays opaque to the Client; clients echo it and the Host parses
/// the segment for the capability the intent names.
#[must_use]
pub fn consent_view_mark(dialogue_rev: Option<u64>, learning_rev: Option<u64>) -> String {
    format!(
        "{};{}",
        consent_mark(CapabilityKind::Dialogue, dialogue_rev),
        consent_mark(CapabilityKind::Learning, learning_rev),
    )
}

/// Parses the state one base-view mark names for `capability`.
///
/// Accepts the combined view mark and a single-capability segment. Stage 2
/// marks (`consent-none`, `consent-rev-N`) name the dialogue capability
/// implicitly and stay parseable for stored journals and in-flight clients;
/// they never authorize learning. Returns `None` when no segment for
/// `capability` parses.
#[must_use]
pub fn parse_consent_mark(mark: &str, capability: CapabilityKind) -> Option<Option<u64>> {
    let qualified = format!("consent-{}-", capability.as_str());
    for segment in mark.split(';').map(str::trim) {
        if let Some(state) = segment.strip_prefix(&qualified) {
            return parse_consent_state(state);
        }
        // Stage 2 marks name the dialogue capability implicitly. An
        // unparseable legacy-shaped segment is skipped, not treated as the
        // answer, so a later well-formed segment can still match.
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
/// Tracks minted evaluation ids and enforces single use.
///
/// Holds the issued `RawId -> EvalFingerprint` map only: a successful
/// consume removes the entry, so presence means unused and absence means
/// unknown or already consumed. Minting binds an id to a candidate
/// fingerprint; consuming requires the same fingerprint and succeeds at
/// most once per id.
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
/// 1. A `(consumer, capability, purpose)` triple outside the closed world
///    denies with [`DenyCode::NotInAllowlist`]. The current world is
///    `(CompanionDialogue, Dialogue, DialogueResponse)` and
///    `(CompanionLearning, Learning, MemoryFormation)`.
/// 2. Consent comparison: when stored consent exists and differs from
///    `expected_consent`, the caller's view is stale and the decision is
///    [`LiveAuthorizationDecision::NeedsRevalidation`].
/// 3. With no stored consent, a stored record for a different capability, or
///    a provider/model mismatch against the stored record, the decision
///    denies with [`DenyCode::ConsentStale`]. A record authorizes only the
///    capability it names.
/// 4. Otherwise the candidate is allowed for exactly one use: a fresh id is
///    minted from `tracker` and returned in
///    [`LiveAuthorizationDecision::AllowForThisUse`].
///
/// Setup completeness is not checked here: only the caller that resolved a
/// registered credential ref and confirmed its bearer exists builds a query.
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
        // Same provider, model, credential, and revision view: only the
        // capability differs. The stored dialogue record must not cover the
        // learning candidate.
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
        // Stage 2 marks name the dialogue capability implicitly.
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
        // A learning segment before the dialogue segment must not short
        // circuit the search for the dialogue segment.
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
