//! Memory Arbiter — validates, deduplicates, and resolves contradictions
//! for [`MemoryCandidate`] items before they are persisted.
//!
//! Issue #75: sits between deterministic/LLM extractors and the typed
//! memory store. Extractors produce candidates; the arbiter decides whether
//! to persist, ignore, supersede, dispute, or mark memories for deletion.

use std::collections::HashMap;

use ene_memory::{
    AffectAnnotation, MemoryConfidence, MemoryItem, MemoryKind, MemorySalience, MemoryScope,
    MemorySource, MemoryStatus, MemoryStore, NewMemoryItem,
};
use tracing::debug;
use unicode_normalization::UnicodeNormalization;

use super::candidate::{MemoryCandidate, TurnInput};
use crate::config::CognitionMemoryConfig;
use crate::error::CognitionError;

/// Provenance of a memory candidate, used to set [`MemorySource`] on persist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateProvenance {
    /// Produced by the deterministic extractor (#70).
    Deterministic,
    /// Produced by the LLM extractor (#66).
    LlmExtracted,
}

/// Optional semantic duplicate match for a candidate (from vector search).
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticMatch {
    /// ID of the matched existing memory.
    pub memory_id: i64,
    /// Cosine similarity score.
    pub similarity: f32,
    /// The matched memory item (for content comparison).
    pub memory: MemoryItem,
}

/// Tunable thresholds for arbitration decisions.
#[derive(Debug, Clone)]
pub struct ArbiterOptions {
    /// Minimum confidence required to persist a candidate.
    pub min_confidence: f32,
    /// New candidate must exceed existing confidence by at least this delta to supersede.
    pub supersede_confidence_delta: f32,
    /// Similarity at or above which two memories are considered semantically duplicate.
    pub semantic_similarity_threshold: f32,
    /// When confidence gap is below this, mark as disputed instead of superseding.
    pub dispute_confidence_gap: f32,
}

impl Default for ArbiterOptions {
    fn default() -> Self {
        Self {
            min_confidence: 0.65,
            supersede_confidence_delta: 0.05,
            semantic_similarity_threshold: 0.85,
            dispute_confidence_gap: 0.15,
        }
    }
}

impl ArbiterOptions {
    /// Build options from [`CognitionMemoryConfig`].
    #[must_use]
    pub fn from_config(config: &CognitionMemoryConfig) -> Self {
        Self {
            min_confidence: config.min_confidence_to_persist as f32,
            ..Self::default()
        }
    }
}

/// Context for a single arbitration batch.
#[derive(Debug, Clone)]
pub struct ArbiterContext<'a> {
    /// The conversation turn the candidates were extracted from.
    pub turn: TurnInput<'a>,
    /// Character identifier for scoped memories.
    pub character_id: &'a str,
    /// User identifier (may be empty).
    pub user_id: &'a str,
    /// Optional session/turn reference stored as `source_ref`.
    pub source_ref: Option<&'a str>,
    /// How the candidates were produced.
    pub provenance: CandidateProvenance,
    /// Decision thresholds.
    pub options: ArbiterOptions,
    /// Pre-computed semantic matches per candidate index (optional).
    pub semantic_matches: HashMap<usize, Vec<SemanticMatch>>,
}

/// Machine-readable reason for an arbiter decision (for tracing and tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArbiterReasonCode {
    /// Candidate confidence is below the configured threshold.
    LowConfidence,
    /// Title or content is empty.
    EmptyFields,
    /// `source_quote` is not found in the turn text.
    SourceQuoteNotInTurn,
    /// Deletion candidate is missing `deletion_target_key`.
    MissingDeletionTarget,
    /// No existing memory matched the deletion target.
    DeletionTargetNotFound,
    /// Exact duplicate of an existing active memory.
    ExactDuplicate,
    /// Semantic duplicate with identical content.
    SemanticDuplicate,
    /// New evidence supersedes an existing memory.
    ContradictionSupersede,
    /// Weak contradiction — existing memory marked disputed.
    ContradictionDisputed,
    /// User requested deletion of a matched memory.
    DeletionRequest,
    /// Contradiction is ambiguous — defer to user confirmation.
    AskUserConfirmation,
    /// Candidate passed validation and has no conflicts.
    ValidNewMemory,
    /// Duplicate candidate within the same batch.
    BatchDuplicate,
}

/// Human-readable decision reason with structured code.
#[derive(Debug, Clone, PartialEq)]
pub struct ArbiterReason {
    /// Structured reason code.
    pub code: ArbiterReasonCode,
    /// Additional detail for logs and debugging.
    pub detail: String,
}

impl ArbiterReason {
    fn new(code: ArbiterReasonCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

/// Action the arbiter recommends for a single candidate.
#[derive(Debug, Clone)]
pub enum ArbiterAction {
    /// Insert a new typed memory.
    Persist(NewMemoryItem),
    /// Do not persist or modify anything.
    Ignore,
    /// Mark an existing memory as user-deleted.
    MarkUserDeleted {
        /// ID of the memory to mark deleted.
        memory_id: i64,
    },
    /// Insert a replacement and mark the prior row superseded.
    Supersede {
        /// New memory payload (with `supersedes_id` set to the old row).
        new_item: NewMemoryItem,
        /// ID of the memory being replaced.
        superseded_id: i64,
    },
    /// Mark an existing memory as disputed.
    MarkDisputed {
        /// ID of the disputed memory.
        memory_id: i64,
    },
    /// Defer persistence until the user confirms (weak contradiction).
    AskConfirmationLater,
}

/// Decision for one candidate.
#[derive(Debug, Clone)]
pub struct CandidateDecision {
    /// The original candidate.
    pub candidate: MemoryCandidate,
    /// Recommended action.
    pub action: ArbiterAction,
    /// Why this action was chosen.
    pub reason: ArbiterReason,
}

/// Result of applying decisions to the store.
#[derive(Debug, Clone)]
pub struct AppliedDecision {
    /// The decision that was applied.
    pub decision: CandidateDecision,
    /// ID of a newly inserted memory, if any.
    pub inserted_id: Option<i64>,
    /// Whether a status update was applied to an existing memory.
    pub updated_existing: bool,
}

/// Validates, deduplicates, and resolves contradictions for memory candidates.
#[derive(Debug, Default)]
pub struct MemoryArbiter;

impl MemoryArbiter {
    /// Evaluate all candidates against existing memories without touching the store.
    #[must_use]
    pub fn evaluate_all(
        candidates: &[MemoryCandidate],
        existing: &[MemoryItem],
        ctx: &ArbiterContext<'_>,
    ) -> Vec<CandidateDecision> {
        let mut decisions = Vec::with_capacity(candidates.len());
        let mut batch_seen = std::collections::HashSet::new();

        for (idx, candidate) in candidates.iter().enumerate() {
            let semantic = ctx.semantic_matches.get(&idx).cloned().unwrap_or_default();
            let mut decision = Self::evaluate_one(candidate, existing, ctx, &semantic);

            if candidate.should_persist && passes_validation(candidate, ctx) {
                let key = dedup_key(candidate);
                if !batch_seen.insert(key) {
                    decision = CandidateDecision {
                        candidate: candidate.clone(),
                        action: ArbiterAction::Ignore,
                        reason: ArbiterReason::new(
                            ArbiterReasonCode::BatchDuplicate,
                            "duplicate candidate in same batch",
                        ),
                    };
                }
            }

            log_decision(&decision);
            decisions.push(decision);
        }

        decisions
    }

    /// Load active memories, evaluate candidates, and apply decisions.
    pub async fn arbitrate_and_apply(
        store: &MemoryStore,
        candidates: &[MemoryCandidate],
        ctx: &ArbiterContext<'_>,
    ) -> Result<Vec<AppliedDecision>, CognitionError> {
        let existing = store
            .get_typed_memories_by_character(ctx.character_id, None, 10_000, 0)
            .await
            .map_err(CognitionError::Memory)?;

        let active_existing: Vec<MemoryItem> = existing
            .into_iter()
            .filter(|m| is_arbitration_visible(m.status))
            .collect();

        let decisions = Self::evaluate_all(candidates, &active_existing, ctx);
        Self::apply_decisions(store, &decisions).await
    }

    /// Apply a slice of decisions to the memory store.
    pub async fn apply_decisions(
        store: &MemoryStore,
        decisions: &[CandidateDecision],
    ) -> Result<Vec<AppliedDecision>, CognitionError> {
        let mut applied = Vec::with_capacity(decisions.len());

        for decision in decisions {
            let result = Self::apply_one(store, decision).await?;
            applied.push(result);
        }

        Ok(applied)
    }

    pub(crate) fn evaluate_one(
        candidate: &MemoryCandidate,
        existing: &[MemoryItem],
        ctx: &ArbiterContext<'_>,
        semantic_matches: &[SemanticMatch],
    ) -> CandidateDecision {
        if !candidate.should_persist {
            return Self::evaluate_deletion(candidate, existing, ctx);
        }

        if let Some(reason) = Self::validate_candidate(candidate, ctx) {
            return CandidateDecision {
                candidate: candidate.clone(),
                action: ArbiterAction::Ignore,
                reason,
            };
        }

        if let Some(existing_id) = find_exact_duplicate(candidate, existing) {
            return CandidateDecision {
                candidate: candidate.clone(),
                action: ArbiterAction::Ignore,
                reason: ArbiterReason::new(
                    ArbiterReasonCode::ExactDuplicate,
                    format!("matches existing memory id={existing_id}"),
                ),
            };
        }

        if let Some(decision) =
            Self::check_semantic_matches(candidate, semantic_matches, ctx, existing)
        {
            return decision;
        }

        if let Some(decision) = Self::check_contradiction(candidate, existing, ctx) {
            return decision;
        }

        let new_item = candidate_to_new_item(candidate, ctx);
        CandidateDecision {
            candidate: candidate.clone(),
            action: ArbiterAction::Persist(new_item),
            reason: ArbiterReason::new(
                ArbiterReasonCode::ValidNewMemory,
                "candidate passed validation",
            ),
        }
    }

    fn evaluate_deletion(
        candidate: &MemoryCandidate,
        existing: &[MemoryItem],
        ctx: &ArbiterContext<'_>,
    ) -> CandidateDecision {
        if let Some(reason) = Self::validate_candidate(candidate, ctx) {
            return CandidateDecision {
                candidate: candidate.clone(),
                action: ArbiterAction::Ignore,
                reason,
            };
        }

        let Some(target) = candidate.deletion_target_key.as_deref() else {
            return CandidateDecision {
                candidate: candidate.clone(),
                action: ArbiterAction::Ignore,
                reason: ArbiterReason::new(
                    ArbiterReasonCode::MissingDeletionTarget,
                    "deletion candidate without deletion_target_key",
                ),
            };
        };

        let matches = find_deletion_targets(target, existing);
        let Some(memory_id) = matches.first().copied() else {
            return CandidateDecision {
                candidate: candidate.clone(),
                action: ArbiterAction::Ignore,
                reason: ArbiterReason::new(
                    ArbiterReasonCode::DeletionTargetNotFound,
                    format!("no memory matched deletion target '{target}'"),
                ),
            };
        };

        CandidateDecision {
            candidate: candidate.clone(),
            action: ArbiterAction::MarkUserDeleted { memory_id },
            reason: ArbiterReason::new(
                ArbiterReasonCode::DeletionRequest,
                format!("mark memory id={memory_id} as user deleted"),
            ),
        }
    }

    fn validate_candidate(
        candidate: &MemoryCandidate,
        ctx: &ArbiterContext<'_>,
    ) -> Option<ArbiterReason> {
        if candidate.confidence < ctx.options.min_confidence {
            return Some(ArbiterReason::new(
                ArbiterReasonCode::LowConfidence,
                format!(
                    "confidence {:.2} below threshold {:.2}",
                    candidate.confidence, ctx.options.min_confidence
                ),
            ));
        }

        if candidate.title.trim().is_empty() || candidate.content.trim().is_empty() {
            return Some(ArbiterReason::new(
                ArbiterReasonCode::EmptyFields,
                "title or content is empty",
            ));
        }

        if !source_quote_valid(candidate, &ctx.turn) {
            return Some(ArbiterReason::new(
                ArbiterReasonCode::SourceQuoteNotInTurn,
                "source_quote not found in turn text",
            ));
        }

        None
    }

    fn check_semantic_matches(
        candidate: &MemoryCandidate,
        semantic_matches: &[SemanticMatch],
        ctx: &ArbiterContext<'_>,
        existing: &[MemoryItem],
    ) -> Option<CandidateDecision> {
        let best_match = semantic_matches
            .iter()
            .filter(|m| m.similarity >= ctx.options.semantic_similarity_threshold)
            .max_by(|a, b| {
                a.similarity
                    .partial_cmp(&b.similarity)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

        if let Some(m) = best_match {
            if same_memory_content(candidate, &m.memory) {
                return Some(CandidateDecision {
                    candidate: candidate.clone(),
                    action: ArbiterAction::Ignore,
                    reason: ArbiterReason::new(
                        ArbiterReasonCode::SemanticDuplicate,
                        format!(
                            "semantic duplicate of id={} (sim={:.2})",
                            m.memory_id, m.similarity
                        ),
                    ),
                });
            }

            return Self::contradiction_decision(
                candidate,
                &m.memory,
                ctx,
                ArbiterReasonCode::ContradictionSupersede,
                ArbiterReasonCode::ContradictionDisputed,
                ArbiterReasonCode::AskUserConfirmation,
                format!(
                    "semantic conflict with id={} (sim={:.2})",
                    m.memory_id, m.similarity
                ),
            );
        }

        // Also check kind-specific contradictions not caught by semantic search.
        let _ = existing;
        None
    }

    fn check_contradiction(
        candidate: &MemoryCandidate,
        existing: &[MemoryItem],
        ctx: &ArbiterContext<'_>,
    ) -> Option<CandidateDecision> {
        if !is_contradiction_kind(candidate.kind) {
            return None;
        }

        for mem in existing {
            if mem.kind != candidate.kind {
                continue;
            }
            if !same_contradiction_key(candidate, mem) {
                continue;
            }
            if same_memory_content(candidate, mem) {
                return Some(CandidateDecision {
                    candidate: candidate.clone(),
                    action: ArbiterAction::Ignore,
                    reason: ArbiterReason::new(
                        ArbiterReasonCode::ExactDuplicate,
                        format!(
                            "same {} content as id={}",
                            kind_label(candidate.kind),
                            mem.id.unwrap_or(-1)
                        ),
                    ),
                });
            }

            return Self::contradiction_decision(
                candidate,
                mem,
                ctx,
                ArbiterReasonCode::ContradictionSupersede,
                ArbiterReasonCode::ContradictionDisputed,
                ArbiterReasonCode::AskUserConfirmation,
                format!("contradicts existing id={}", mem.id.unwrap_or(-1)),
            );
        }

        None
    }

    fn contradiction_decision(
        candidate: &MemoryCandidate,
        existing: &MemoryItem,
        ctx: &ArbiterContext<'_>,
        supersede_code: ArbiterReasonCode,
        dispute_code: ArbiterReasonCode,
        ask_code: ArbiterReasonCode,
        detail: String,
    ) -> Option<CandidateDecision> {
        let existing_id = existing.id?;
        let existing_conf = existing.confidence.get();
        let delta = candidate.confidence - existing_conf;

        if delta >= ctx.options.supersede_confidence_delta {
            let mut new_item = candidate_to_new_item(candidate, ctx);
            new_item.supersedes_id = Some(existing_id);
            return Some(CandidateDecision {
                candidate: candidate.clone(),
                action: ArbiterAction::Supersede {
                    new_item,
                    superseded_id: existing_id,
                },
                reason: ArbiterReason::new(supersede_code, detail),
            });
        }

        if delta > -ctx.options.dispute_confidence_gap {
            return Some(CandidateDecision {
                candidate: candidate.clone(),
                action: ArbiterAction::MarkDisputed {
                    memory_id: existing_id,
                },
                reason: ArbiterReason::new(dispute_code, detail),
            });
        }

        if candidate.confidence >= ctx.options.min_confidence {
            return Some(CandidateDecision {
                candidate: candidate.clone(),
                action: ArbiterAction::AskConfirmationLater,
                reason: ArbiterReason::new(ask_code, detail),
            });
        }

        None
    }

    async fn apply_one(
        store: &MemoryStore,
        decision: &CandidateDecision,
    ) -> Result<AppliedDecision, CognitionError> {
        match &decision.action {
            ArbiterAction::Persist(item) => {
                let id = store
                    .insert_typed_memory(item)
                    .await
                    .map_err(CognitionError::Memory)?;
                Ok(AppliedDecision {
                    decision: decision.clone(),
                    inserted_id: Some(id),
                    updated_existing: false,
                })
            }
            ArbiterAction::Supersede {
                new_item,
                superseded_id,
            } => {
                let id = store
                    .supersede_typed_memory(new_item, *superseded_id)
                    .await
                    .map_err(CognitionError::Memory)?;
                Ok(AppliedDecision {
                    decision: decision.clone(),
                    inserted_id: Some(id),
                    updated_existing: true,
                })
            }
            ArbiterAction::MarkUserDeleted { memory_id } => {
                let updated = store
                    .update_typed_memory_status(*memory_id, MemoryStatus::UserDeleted)
                    .await
                    .map_err(CognitionError::Memory)?;
                Ok(AppliedDecision {
                    decision: decision.clone(),
                    inserted_id: None,
                    updated_existing: updated,
                })
            }
            ArbiterAction::MarkDisputed { memory_id } => {
                let updated = store
                    .update_typed_memory_status(*memory_id, MemoryStatus::Disputed)
                    .await
                    .map_err(CognitionError::Memory)?;
                Ok(AppliedDecision {
                    decision: decision.clone(),
                    inserted_id: None,
                    updated_existing: updated,
                })
            }
            ArbiterAction::Ignore | ArbiterAction::AskConfirmationLater => Ok(AppliedDecision {
                decision: decision.clone(),
                inserted_id: None,
                updated_existing: false,
            }),
        }
    }
}

fn candidate_to_new_item(candidate: &MemoryCandidate, ctx: &ArbiterContext<'_>) -> NewMemoryItem {
    let source = match ctx.provenance {
        CandidateProvenance::Deterministic => MemorySource::Inferred,
        CandidateProvenance::LlmExtracted => MemorySource::LlmExtracted,
    };

    let scope = match candidate.kind {
        MemoryKind::UserProfile | MemoryKind::Preference => MemoryScope::User,
        MemoryKind::Relationship | MemoryKind::Reflection => MemoryScope::Shared,
        _ => MemoryScope::Character,
    };

    let salience = match candidate.kind {
        MemoryKind::Commitment | MemoryKind::Preference | MemoryKind::UserProfile => {
            MemorySalience::new(0.7)
        }
        MemoryKind::Affective => MemorySalience::new(0.8),
        _ => MemorySalience::default(),
    };

    // `commitment_due` is intentionally not mapped to `valid_until` yet — natural
    // language due-date parsing is deferred (see issue #75 follow-up).
    let (valid_from, valid_until) = (None, None);

    NewMemoryItem {
        scope,
        character_id: ctx.character_id.to_string(),
        user_id: ctx.user_id.to_string(),
        kind: candidate.kind,
        title: candidate.title.clone(),
        content: candidate.content.clone(),
        source,
        source_ref: ctx.source_ref.map(str::to_string),
        confidence: MemoryConfidence::new(candidate.confidence),
        salience,
        affect: AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from,
        valid_until,
        status: MemoryStatus::Active,
        supersedes_id: None,
    }
}

fn passes_validation(candidate: &MemoryCandidate, ctx: &ArbiterContext<'_>) -> bool {
    MemoryArbiter::validate_candidate(candidate, ctx).is_none()
}

fn normalize_text(s: &str) -> String {
    s.nfkc()
        .collect::<String>()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn dedup_key(candidate: &MemoryCandidate) -> (MemoryKind, String) {
    (candidate.kind, normalize_text(&candidate.title))
}

fn source_quote_valid(candidate: &MemoryCandidate, turn: &TurnInput<'_>) -> bool {
    if candidate.source_quote.trim().is_empty() {
        return candidate.kind == MemoryKind::Procedure && !turn.tool_results.is_empty();
    }

    let quote = normalize_text(&candidate.source_quote);
    let user = normalize_text(turn.user_message);
    if user.contains(&quote) {
        return true;
    }
    if let Some(asst) = turn.assistant_message
        && normalize_text(asst).contains(&quote)
    {
        return true;
    }
    false
}

fn is_arbitration_visible(status: MemoryStatus) -> bool {
    matches!(
        status,
        MemoryStatus::Active | MemoryStatus::Faded | MemoryStatus::Disputed
    )
}

fn is_contradiction_kind(kind: MemoryKind) -> bool {
    matches!(
        kind,
        MemoryKind::Preference
            | MemoryKind::UserProfile
            | MemoryKind::Semantic
            | MemoryKind::Relationship
    )
}

fn kind_label(kind: MemoryKind) -> &'static str {
    kind.as_str()
}

fn same_memory_content(candidate: &MemoryCandidate, mem: &MemoryItem) -> bool {
    normalize_text(&candidate.content) == normalize_text(&mem.content) && candidate.kind == mem.kind
}

fn same_contradiction_key(candidate: &MemoryCandidate, mem: &MemoryItem) -> bool {
    let c_title = normalize_text(&candidate.title);
    let m_title = normalize_text(&mem.title);
    if c_title == m_title {
        return true;
    }

    match candidate.kind {
        MemoryKind::Preference => c_title.starts_with(&m_title) || m_title.starts_with(&c_title),
        MemoryKind::UserProfile => {
            c_title.contains("nickname")
                || c_title.contains("呼び方")
                || m_title.contains("nickname")
                || m_title.contains("呼び方")
        }
        _ => false,
    }
}

fn find_exact_duplicate(candidate: &MemoryCandidate, existing: &[MemoryItem]) -> Option<i64> {
    let key = dedup_key(candidate);
    let content_norm = normalize_text(&candidate.content);

    for mem in existing {
        if mem.kind != key.0 {
            continue;
        }
        if normalize_text(&mem.title) == key.1 && normalize_text(&mem.content) == content_norm {
            return mem.id;
        }
    }
    None
}

fn find_deletion_targets(target: &str, existing: &[MemoryItem]) -> Vec<i64> {
    let target_norm = normalize_text(target);
    let mut scored: Vec<(i64, usize)> = existing
        .iter()
        .filter_map(|mem| {
            let id = mem.id?;
            let title = normalize_text(&mem.title);
            let content = normalize_text(&mem.content);
            let score = if title.contains(&target_norm) || content.contains(&target_norm) {
                target_norm.len()
            } else {
                0
            };
            if score > 0 { Some((id, score)) } else { None }
        })
        .collect();

    scored.sort_by_key(|b| std::cmp::Reverse(b.1));
    scored.into_iter().map(|(id, _)| id).collect()
}

fn log_decision(decision: &CandidateDecision) {
    let matched_id = match &decision.action {
        ArbiterAction::MarkUserDeleted { memory_id }
        | ArbiterAction::MarkDisputed { memory_id } => Some(*memory_id),
        ArbiterAction::Supersede { superseded_id, .. } => Some(*superseded_id),
        _ => None,
    };

    debug!(
        component = "MemoryArbiter",
        kind = ?decision.candidate.kind,
        confidence = decision.candidate.confidence,
        action = ?decision.action,
        reason_code = ?decision.reason.code,
        reason_detail = %decision.reason.detail,
        matched_memory_id = ?matched_id,
        "memory arbitration decision"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_writer::candidate::ToolResultSummary;
    use chrono::Utc;

    fn ctx<'a>(turn: TurnInput<'a>) -> ArbiterContext<'a> {
        ArbiterContext {
            turn,
            character_id: "ene",
            user_id: "user1",
            source_ref: Some("session-1"),
            provenance: CandidateProvenance::Deterministic,
            options: ArbiterOptions::default(),
            semantic_matches: HashMap::new(),
        }
    }

    fn sample_candidate(confidence: f32) -> MemoryCandidate {
        MemoryCandidate {
            kind: MemoryKind::Semantic,
            title: "project X".to_string(),
            content: "User is working on project X".to_string(),
            source_quote: "remember project X".to_string(),
            confidence,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
        }
    }

    fn decision_action(decisions: &[CandidateDecision]) -> &ArbiterAction {
        &decisions.first().expect("one decision").action
    }

    #[test]
    fn low_confidence_is_rejected() {
        let turn = TurnInput {
            user_message: "remember project X",
            assistant_message: None,
            tool_results: &[],
        };
        let decisions = MemoryArbiter::evaluate_all(&[sample_candidate(0.4)], &[], &ctx(turn));
        assert!(matches!(decision_action(&decisions), ArbiterAction::Ignore));
        assert_eq!(decisions[0].reason.code, ArbiterReasonCode::LowConfidence);
    }

    #[test]
    fn source_quote_mismatch_is_rejected() {
        let turn = TurnInput {
            user_message: "hello there",
            assistant_message: None,
            tool_results: &[],
        };
        let decisions = MemoryArbiter::evaluate_all(&[sample_candidate(0.9)], &[], &ctx(turn));
        assert!(matches!(decision_action(&decisions), ArbiterAction::Ignore));
        assert_eq!(
            decisions[0].reason.code,
            ArbiterReasonCode::SourceQuoteNotInTurn
        );
    }

    #[test]
    fn valid_candidate_persists() {
        let turn = TurnInput {
            user_message: "remember project X",
            assistant_message: None,
            tool_results: &[],
        };
        let decisions = MemoryArbiter::evaluate_all(&[sample_candidate(0.9)], &[], &ctx(turn));
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::Persist(_)
        ));
        assert_eq!(decisions[0].reason.code, ArbiterReasonCode::ValidNewMemory);
    }

    #[tokio::test]
    async fn exact_duplicate_is_ignored() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let item = NewMemoryItem {
            scope: MemoryScope::Character,
            character_id: "ene".to_string(),
            user_id: "user1".to_string(),
            kind: MemoryKind::Semantic,
            title: "project X".to_string(),
            content: "User is working on project X".to_string(),
            source: MemorySource::Inferred,
            source_ref: None,
            confidence: MemoryConfidence::new(0.8),
            salience: MemorySalience::default(),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
        };
        let id = store.insert_typed_memory(&item).await.unwrap();
        let existing = store.get_typed_memory(id).await.unwrap().unwrap();

        let turn = TurnInput {
            user_message: "remember project X",
            assistant_message: None,
            tool_results: &[],
        };
        let decisions =
            MemoryArbiter::evaluate_all(&[sample_candidate(0.9)], &[existing], &ctx(turn));
        assert!(matches!(decision_action(&decisions), ArbiterAction::Ignore));
        assert_eq!(decisions[0].reason.code, ArbiterReasonCode::ExactDuplicate);
    }

    #[tokio::test]
    async fn preference_supersede_when_higher_confidence() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let existing_item = NewMemoryItem {
            scope: MemoryScope::User,
            character_id: "ene".to_string(),
            user_id: "user1".to_string(),
            kind: MemoryKind::Preference,
            title: "love: coffee".to_string(),
            content: "I love coffee".to_string(),
            source: MemorySource::Inferred,
            source_ref: None,
            confidence: MemoryConfidence::new(0.7),
            salience: MemorySalience::default(),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
        };
        let id = store.insert_typed_memory(&existing_item).await.unwrap();
        let existing = store.get_typed_memory(id).await.unwrap().unwrap();

        let turn = TurnInput {
            user_message: "I love tea now",
            assistant_message: None,
            tool_results: &[],
        };
        let candidate = MemoryCandidate {
            kind: MemoryKind::Preference,
            title: "love: coffee".to_string(),
            content: "I love tea".to_string(),
            source_quote: "I love tea now".to_string(),
            confidence: 0.9,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
        };
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[existing], &ctx(turn));
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::Supersede { .. }
        ));
        assert_eq!(
            decisions[0].reason.code,
            ArbiterReasonCode::ContradictionSupersede
        );
    }

    #[tokio::test]
    async fn deletion_request_marks_user_deleted() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let existing_item = NewMemoryItem {
            scope: MemoryScope::Character,
            character_id: "ene".to_string(),
            user_id: "user1".to_string(),
            kind: MemoryKind::Semantic,
            title: "project X".to_string(),
            content: "User is working on project X".to_string(),
            source: MemorySource::Inferred,
            source_ref: None,
            confidence: MemoryConfidence::new(0.8),
            salience: MemorySalience::default(),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
        };
        let id = store.insert_typed_memory(&existing_item).await.unwrap();
        let existing = store.get_typed_memory(id).await.unwrap().unwrap();

        let turn = TurnInput {
            user_message: "forget project X",
            assistant_message: None,
            tool_results: &[],
        };
        let candidate = MemoryCandidate {
            kind: MemoryKind::Semantic,
            title: "forget: project X".to_string(),
            content: "User requested to forget: project X".to_string(),
            source_quote: "forget project X".to_string(),
            confidence: 0.9,
            should_persist: false,
            deletion_target_key: Some("project X".to_string()),
            commitment_due: None,
        };
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[existing], &ctx(turn));
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::MarkUserDeleted { memory_id } if memory_id == &id
        ));
        assert_eq!(decisions[0].reason.code, ArbiterReasonCode::DeletionRequest);
    }

    #[test]
    fn procedure_with_empty_source_quote_allowed_when_tools_present() {
        let tools = [ToolResultSummary {
            tool_name: "fs".to_string(),
            success: true,
            summary: "wrote file".to_string(),
        }];
        let turn = TurnInput {
            user_message: "run tool",
            assistant_message: None,
            tool_results: &tools,
        };
        let candidate = MemoryCandidate {
            kind: MemoryKind::Procedure,
            title: "tool success".to_string(),
            content: "[fs] wrote file".to_string(),
            source_quote: String::new(),
            confidence: 0.7,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
        };
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[], &ctx(turn));
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::Persist(_)
        ));
    }

    #[tokio::test]
    async fn apply_supersede_updates_old_memory() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let old_item = NewMemoryItem {
            scope: MemoryScope::User,
            character_id: "ene".to_string(),
            user_id: "user1".to_string(),
            kind: MemoryKind::Preference,
            title: "drink".to_string(),
            content: "likes coffee".to_string(),
            source: MemorySource::Inferred,
            source_ref: None,
            confidence: MemoryConfidence::new(0.7),
            salience: MemorySalience::default(),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
        };
        let old_id = store.insert_typed_memory(&old_item).await.unwrap();
        let existing = store.get_typed_memory(old_id).await.unwrap().unwrap();

        let turn = TurnInput {
            user_message: "I prefer tea",
            assistant_message: None,
            tool_results: &[],
        };
        let candidate = MemoryCandidate {
            kind: MemoryKind::Preference,
            title: "drink".to_string(),
            content: "likes tea".to_string(),
            source_quote: "I prefer tea".to_string(),
            confidence: 0.9,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
        };
        let arbiter_ctx = ctx(turn);
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[existing], &arbiter_ctx);
        let applied = MemoryArbiter::apply_decisions(&store, &decisions)
            .await
            .unwrap();
        assert_eq!(applied.len(), 1);
        assert!(applied[0].inserted_id.is_some());

        let old = store.get_typed_memory(old_id).await.unwrap().unwrap();
        assert_eq!(old.status, MemoryStatus::Superseded);
        assert_eq!(old.supersedes_id, None);

        let new_id = applied[0].inserted_id.unwrap();
        let new_mem = store.get_typed_memory(new_id).await.unwrap().unwrap();
        assert_eq!(new_mem.supersedes_id, Some(old_id));
    }

    #[test]
    fn batch_duplicate_second_candidate_ignored() {
        let turn = TurnInput {
            user_message: "remember project X details",
            assistant_message: None,
            tool_results: &[],
        };
        let first = MemoryCandidate {
            kind: MemoryKind::Semantic,
            title: "project X".to_string(),
            content: "User is working on project X".to_string(),
            source_quote: "remember project X details".to_string(),
            confidence: 0.9,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
        };
        let second = MemoryCandidate {
            content: "Different detail about project X".to_string(),
            ..first.clone()
        };
        let decisions = MemoryArbiter::evaluate_all(&[first, second], &[], &ctx(turn));
        assert_eq!(decisions.len(), 2);
        assert!(matches!(&decisions[0].action, ArbiterAction::Persist(_)));
        assert!(matches!(decisions[1].action, ArbiterAction::Ignore));
        assert_eq!(decisions[1].reason.code, ArbiterReasonCode::BatchDuplicate);
    }

    #[test]
    fn semantic_duplicate_is_ignored() {
        let turn = TurnInput {
            user_message: "remember project X",
            assistant_message: None,
            tool_results: &[],
        };
        let existing = MemoryItem {
            id: Some(42),
            scope: MemoryScope::Character,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind: MemoryKind::Semantic,
            title: "project X".into(),
            content: "User is working on project X".into(),
            source: MemorySource::Inferred,
            source_ref: None,
            confidence: MemoryConfidence::new(0.8),
            salience: MemorySalience::default(),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            access_count: 0,
            last_accessed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
        };
        let mut semantic_matches = HashMap::new();
        semantic_matches.insert(
            0,
            vec![SemanticMatch {
                memory_id: 42,
                similarity: 0.9,
                memory: existing,
            }],
        );
        let arbiter_ctx = ArbiterContext {
            semantic_matches,
            ..ctx(turn)
        };
        let decisions = MemoryArbiter::evaluate_all(&[sample_candidate(0.9)], &[], &arbiter_ctx);
        assert!(matches!(decision_action(&decisions), ArbiterAction::Ignore));
        assert_eq!(
            decisions[0].reason.code,
            ArbiterReasonCode::SemanticDuplicate
        );
    }

    #[test]
    fn semantic_supersede_picks_strongest_match() {
        let turn = TurnInput {
            user_message: "I love tea now",
            assistant_message: None,
            tool_results: &[],
        };
        let weaker = MemoryItem {
            id: Some(1),
            scope: MemoryScope::User,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind: MemoryKind::Preference,
            title: "love: coffee".into(),
            content: "I love coffee".into(),
            source: MemorySource::Inferred,
            source_ref: None,
            confidence: MemoryConfidence::new(0.5),
            salience: MemorySalience::default(),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            access_count: 0,
            last_accessed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
        };
        let stronger = MemoryItem {
            id: Some(2),
            confidence: MemoryConfidence::new(0.7),
            ..weaker.clone()
        };
        let candidate = MemoryCandidate {
            kind: MemoryKind::Preference,
            title: "love: coffee".to_string(),
            content: "I love tea".to_string(),
            source_quote: "I love tea now".to_string(),
            confidence: 0.9,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
        };
        let mut semantic_matches = HashMap::new();
        semantic_matches.insert(
            0,
            vec![
                SemanticMatch {
                    memory_id: 1,
                    similarity: 0.86,
                    memory: weaker,
                },
                SemanticMatch {
                    memory_id: 2,
                    similarity: 0.92,
                    memory: stronger,
                },
            ],
        );
        let arbiter_ctx = ArbiterContext {
            semantic_matches,
            ..ctx(turn)
        };
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[], &arbiter_ctx);
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::Supersede {
                superseded_id: 2,
                ..
            }
        ));
    }

    #[test]
    fn semantic_disputed_when_confidence_close() {
        let turn = TurnInput {
            user_message: "I love tea now",
            assistant_message: None,
            tool_results: &[],
        };
        let existing = MemoryItem {
            id: Some(7),
            scope: MemoryScope::User,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind: MemoryKind::Preference,
            title: "love: coffee".into(),
            content: "I love coffee".into(),
            source: MemorySource::Inferred,
            source_ref: None,
            confidence: MemoryConfidence::new(0.7),
            salience: MemorySalience::default(),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            access_count: 0,
            last_accessed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
        };
        let candidate = MemoryCandidate {
            kind: MemoryKind::Preference,
            title: "love: coffee".to_string(),
            content: "I love tea".to_string(),
            source_quote: "I love tea now".to_string(),
            confidence: 0.72,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
        };
        let mut semantic_matches = HashMap::new();
        semantic_matches.insert(
            0,
            vec![SemanticMatch {
                memory_id: 7,
                similarity: 0.9,
                memory: existing,
            }],
        );
        let arbiter_ctx = ArbiterContext {
            semantic_matches,
            ..ctx(turn)
        };
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[], &arbiter_ctx);
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::MarkDisputed { memory_id: 7 }
        ));
        assert_eq!(
            decisions[0].reason.code,
            ArbiterReasonCode::ContradictionDisputed
        );
    }

    #[test]
    fn semantic_ask_confirmation_when_existing_much_stronger() {
        let turn = TurnInput {
            user_message: "I love tea now",
            assistant_message: None,
            tool_results: &[],
        };
        let existing = MemoryItem {
            id: Some(8),
            scope: MemoryScope::User,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind: MemoryKind::Preference,
            title: "love: coffee".into(),
            content: "I love coffee".into(),
            source: MemorySource::Inferred,
            source_ref: None,
            confidence: MemoryConfidence::new(0.95),
            salience: MemorySalience::default(),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            access_count: 0,
            last_accessed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
        };
        let candidate = MemoryCandidate {
            kind: MemoryKind::Preference,
            title: "love: coffee".to_string(),
            content: "I love tea".to_string(),
            source_quote: "I love tea now".to_string(),
            confidence: 0.66,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
        };
        let mut semantic_matches = HashMap::new();
        semantic_matches.insert(
            0,
            vec![SemanticMatch {
                memory_id: 8,
                similarity: 0.9,
                memory: existing,
            }],
        );
        let arbiter_ctx = ArbiterContext {
            semantic_matches,
            ..ctx(turn)
        };
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[], &arbiter_ctx);
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::AskConfirmationLater
        ));
        assert_eq!(
            decisions[0].reason.code,
            ArbiterReasonCode::AskUserConfirmation
        );
    }

    #[test]
    fn deletion_low_confidence_is_rejected() {
        let turn = TurnInput {
            user_message: "forget project X",
            assistant_message: None,
            tool_results: &[],
        };
        let candidate = MemoryCandidate {
            kind: MemoryKind::Semantic,
            title: "forget: project X".to_string(),
            content: "User requested to forget: project X".to_string(),
            source_quote: "forget project X".to_string(),
            confidence: 0.4,
            should_persist: false,
            deletion_target_key: Some("project X".to_string()),
            commitment_due: None,
        };
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[], &ctx(turn));
        assert!(matches!(decision_action(&decisions), ArbiterAction::Ignore));
        assert_eq!(decisions[0].reason.code, ArbiterReasonCode::LowConfidence);
    }

    #[test]
    fn deletion_quote_mismatch_is_rejected() {
        let turn = TurnInput {
            user_message: "hello there",
            assistant_message: None,
            tool_results: &[],
        };
        let candidate = MemoryCandidate {
            kind: MemoryKind::Semantic,
            title: "forget: project X".to_string(),
            content: "User requested to forget: project X".to_string(),
            source_quote: "forget project X".to_string(),
            confidence: 0.9,
            should_persist: false,
            deletion_target_key: Some("project X".to_string()),
            commitment_due: None,
        };
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[], &ctx(turn));
        assert!(matches!(decision_action(&decisions), ArbiterAction::Ignore));
        assert_eq!(
            decisions[0].reason.code,
            ArbiterReasonCode::SourceQuoteNotInTurn
        );
    }

    #[tokio::test]
    async fn arbitrate_and_apply_persists_valid_candidate() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let turn = TurnInput {
            user_message: "remember project X",
            assistant_message: None,
            tool_results: &[],
        };
        let arbiter_ctx = ctx(turn);
        let applied =
            MemoryArbiter::arbitrate_and_apply(&store, &[sample_candidate(0.9)], &arbiter_ctx)
                .await
                .unwrap();

        assert_eq!(applied.len(), 1);
        assert!(applied[0].inserted_id.is_some());
        assert!(matches!(
            applied[0].decision.action,
            ArbiterAction::Persist(_)
        ));

        let id = applied[0].inserted_id.unwrap();
        let mem = store.get_typed_memory(id).await.unwrap().unwrap();
        assert_eq!(mem.title, "project X");
        assert_eq!(mem.status, MemoryStatus::Active);
        assert_eq!(mem.source, MemorySource::Inferred);
    }

    #[tokio::test]
    async fn arbitrate_and_apply_marks_user_deleted() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let existing_item = NewMemoryItem {
            scope: MemoryScope::Character,
            character_id: "ene".to_string(),
            user_id: "user1".to_string(),
            kind: MemoryKind::Semantic,
            title: "project X".to_string(),
            content: "User is working on project X".to_string(),
            source: MemorySource::Inferred,
            source_ref: None,
            confidence: MemoryConfidence::new(0.8),
            salience: MemorySalience::default(),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
        };
        let id = store.insert_typed_memory(&existing_item).await.unwrap();

        let turn = TurnInput {
            user_message: "forget project X",
            assistant_message: None,
            tool_results: &[],
        };
        let candidate = MemoryCandidate {
            kind: MemoryKind::Semantic,
            title: "forget: project X".to_string(),
            content: "User requested to forget: project X".to_string(),
            source_quote: "forget project X".to_string(),
            confidence: 0.9,
            should_persist: false,
            deletion_target_key: Some("project X".to_string()),
            commitment_due: None,
        };
        let applied = MemoryArbiter::arbitrate_and_apply(&store, &[candidate], &ctx(turn))
            .await
            .unwrap();

        assert_eq!(applied.len(), 1);
        assert!(applied[0].updated_existing);
        assert!(matches!(
            applied[0].decision.action,
            ArbiterAction::MarkUserDeleted { memory_id } if memory_id == id
        ));

        let mem = store.get_typed_memory(id).await.unwrap().unwrap();
        assert_eq!(mem.status, MemoryStatus::UserDeleted);
    }

    #[tokio::test]
    async fn apply_disputed_updates_existing_memory() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let existing_item = NewMemoryItem {
            scope: MemoryScope::User,
            character_id: "ene".to_string(),
            user_id: "user1".to_string(),
            kind: MemoryKind::Preference,
            title: "love: coffee".to_string(),
            content: "I love coffee".to_string(),
            source: MemorySource::Inferred,
            source_ref: None,
            confidence: MemoryConfidence::new(0.7),
            salience: MemorySalience::default(),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
        };
        let id = store.insert_typed_memory(&existing_item).await.unwrap();
        let existing = store.get_typed_memory(id).await.unwrap().unwrap();

        let turn = TurnInput {
            user_message: "I love tea now",
            assistant_message: None,
            tool_results: &[],
        };
        let candidate = MemoryCandidate {
            kind: MemoryKind::Preference,
            title: "love: coffee".to_string(),
            content: "I love tea".to_string(),
            source_quote: "I love tea now".to_string(),
            confidence: 0.72,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
        };
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[existing], &ctx(turn));
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::MarkDisputed { memory_id } if memory_id == &id
        ));

        let applied = MemoryArbiter::apply_decisions(&store, &decisions)
            .await
            .unwrap();
        assert!(applied[0].updated_existing);

        let mem = store.get_typed_memory(id).await.unwrap().unwrap();
        assert_eq!(mem.status, MemoryStatus::Disputed);
    }

    #[test]
    fn commitment_due_not_mapped_to_valid_until() {
        let turn = TurnInput {
            user_message: "remind me tomorrow at 3pm",
            assistant_message: None,
            tool_results: &[],
        };
        let candidate = MemoryCandidate {
            kind: MemoryKind::Commitment,
            title: "meeting".to_string(),
            content: "Meeting at 3pm tomorrow".to_string(),
            source_quote: "remind me tomorrow at 3pm".to_string(),
            confidence: 0.9,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: Some("tomorrow 15:00".to_string()),
        };
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[], &ctx(turn));
        let ArbiterAction::Persist(item) = decision_action(&decisions) else {
            panic!("expected Persist");
        };
        assert!(item.valid_until.is_none());
        assert!(item.valid_from.is_none());
    }
}
