//! Memory Arbiter — validates, deduplicates, and resolves contradictions
//! for [`MemoryCandidate`] items before they are persisted.
//!
//! Issue #75: sits between deterministic/LLM extractors and the typed
//! memory store. Extractors produce candidates; the arbiter decides whether
//! to persist, ignore, supersede, dispute, or mark memories for deletion.

use std::collections::HashMap;

use ene_ai::{EmbeddingKind, EmbeddingProvider, cosine_similarity};
use ene_core::{
    AffectAnnotation, MemoryConfidence, MemoryItem, MemoryKind, MemoryPort, MemorySalience,
    MemoryScope, MemorySource, MemoryStatus, NewMemoryItem, PendingCandidate,
    PendingCandidateStatus,
};
use tracing::{debug, warn};
use unicode_normalization::UnicodeNormalization;

use super::candidate::{MemoryCandidate, TurnInput};
use crate::config::MindMemoryConfig;
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
    /// Minimum title-embedding cosine similarity for two memories to be treated
    /// as sharing a contradiction key (#351). Only consulted on the embedding
    /// path; without an embedder the arbiter falls back to exact normalized-title
    /// equality.
    pub contradiction_title_similarity_threshold: f32,
}

impl Default for ArbiterOptions {
    fn default() -> Self {
        Self {
            min_confidence: 0.65,
            supersede_confidence_delta: 0.05,
            semantic_similarity_threshold: 0.85,
            dispute_confidence_gap: 0.15,
            contradiction_title_similarity_threshold: 0.82,
        }
    }
}

impl ArbiterOptions {
    /// Build options from [`MindMemoryConfig`].
    pub fn from_config(config: &MindMemoryConfig) -> Self {
        Self {
            min_confidence: config.min_confidence_to_persist as f32,
            contradiction_title_similarity_threshold: config
                .contradiction_title_similarity_threshold,
            ..Self::default()
        }
    }
}

/// Context for a single arbitration batch.
#[derive(Clone)]
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
    /// Affect valence of the turn the candidates were extracted from
    /// (-1.0 ..= 1.0). Used to derive `outcome_rating` for self-reflection (#210).
    pub affect_valence: f32,
    /// Embedding provider for fuzzy contradiction-key matching (#351). `None`
    /// falls back to exact normalized-title equality so contradiction checks
    /// keep working without an embedding model.
    pub embedder: Option<&'a dyn EmbeddingProvider>,
}

impl std::fmt::Debug for ArbiterContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArbiterContext")
            .field("turn", &self.turn)
            .field("character_id", &self.character_id)
            .field("user_id", &self.user_id)
            .field("source_ref", &self.source_ref)
            .field("provenance", &self.provenance)
            .field("options", &self.options)
            .field("semantic_matches", &self.semantic_matches)
            .field("affect_valence", &self.affect_valence)
            .field("has_embedder", &self.embedder.is_some())
            .finish()
    }
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
    /// A tool-derived candidate failed the lightweight validity check (#349).
    ToolDerivedInvalid,
    /// Deletion candidate is missing `deletion_target_key`.
    MissingDeletionTarget,
    /// No existing memory matched the deletion target.
    DeletionTargetNotFound,
    /// Exact duplicate of an existing active memory.
    ExactDuplicate,
    /// Semantic duplicate with identical content.
    SemanticDuplicate,
    /// A repeated tool failure supersedes the prior failure record (#349).
    ToolFailureSupersede,
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    ///
    /// Carries the conflicting memory's identity so the deferred candidate can
    /// record what it would supersede: `existing_memory_id` is persisted and
    /// later propagated to the typed memory's `supersedes_id` on approval, and
    /// `existing_memory_title` is a display-only hint for the approval UI
    /// (#420 review).
    AskConfirmationLater {
        /// Id of the existing memory this candidate conflicts with, if any.
        existing_memory_id: Option<i64>,
        /// Title of that existing memory (display hint only; not persisted).
        existing_memory_title: Option<String>,
    },
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
    /// Outcome rating derived from user response sentiment (#210).
    /// Range: -1.0 (negative) to 1.0 (positive). `None` when not yet evaluated.
    pub outcome_rating: Option<f32>,
}

/// Validates, deduplicates, and resolves contradictions for memory candidates.
#[derive(Debug, Default)]
pub struct MemoryArbiter;

impl MemoryArbiter {
    /// Evaluate all candidates against existing memories without touching the store.
    ///
    /// Contradiction-key matching uses title-embedding similarity when
    /// `ctx.embedder` is configured (#351), sharing one [`TitleMatcher`] across
    /// the whole batch so repeated titles are embedded only once.
    pub async fn evaluate_all(
        candidates: &[MemoryCandidate],
        existing: &[MemoryItem],
        ctx: &ArbiterContext<'_>,
    ) -> Vec<CandidateDecision> {
        let mut decisions = Vec::with_capacity(candidates.len());
        let mut batch_seen = std::collections::HashSet::new();
        let mut matcher = TitleMatcher::new(
            ctx.embedder,
            ctx.options.contradiction_title_similarity_threshold,
        );

        for (idx, candidate) in candidates.iter().enumerate() {
            let semantic = ctx.semantic_matches.get(&idx).cloned().unwrap_or_default();
            let mut decision =
                Self::evaluate_one(candidate, existing, ctx, &semantic, &mut matcher).await;

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
        store: &dyn MemoryPort,
        candidates: &[MemoryCandidate],
        ctx: &ArbiterContext<'_>,
    ) -> Result<Vec<AppliedDecision>, CognitionError> {
        let candidate_kinds: Vec<MemoryKind> = candidates
            .iter()
            .map(|c| c.kind)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        let mut existing: Vec<MemoryItem> = Vec::new();
        for kind in &candidate_kinds {
            let items = store
                .get_typed_memories_by_character(ctx.character_id, Some(*kind), 10_000, 0)
                .await
                .map_err(CognitionError::MemoryPort)?;
            existing.extend(items);
        }

        let active_existing: Vec<MemoryItem> = existing
            .into_iter()
            .filter(|m| is_arbitration_visible(m.status))
            .collect();

        let decisions = Self::evaluate_all(candidates, &active_existing, ctx).await;
        Self::apply_decisions(store, &decisions, ctx).await
    }

    /// Apply a slice of decisions to the memory store.
    ///
    /// **Non-transactional**: each decision is applied independently. If one
    /// fails mid-batch, previously applied decisions are already committed and
    /// will not be rolled back. Callers that need all-or-nothing semantics
    /// should wrap the store connection in a transaction before calling this.
    ///
    /// `ctx.affect_valence` (-1.0 ..= 1.0) is recorded as the `outcome_rating`
    /// of each persisted decision so the self-reflection pipeline (#210) can
    /// score interaction outcomes by user sentiment.
    pub async fn apply_decisions(
        store: &dyn MemoryPort,
        decisions: &[CandidateDecision],
        ctx: &ArbiterContext<'_>,
    ) -> Result<Vec<AppliedDecision>, CognitionError> {
        let mut applied = Vec::with_capacity(decisions.len());

        for decision in decisions {
            let result = Self::apply_one(store, decision, ctx).await?;
            applied.push(result);
        }

        Ok(applied)
    }

    pub(crate) async fn evaluate_one(
        candidate: &MemoryCandidate,
        existing: &[MemoryItem],
        ctx: &ArbiterContext<'_>,
        semantic_matches: &[SemanticMatch],
        matcher: &mut TitleMatcher<'_>,
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

        if let Some(decision) = Self::check_tool_derived(candidate, existing, ctx) {
            return decision;
        }

        if let Some(decision) = Self::check_semantic_matches(candidate, semantic_matches, ctx) {
            return decision;
        }

        if let Some(decision) = Self::check_contradiction(candidate, existing, ctx, matcher).await {
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

        if is_tool_derived_candidate(candidate) && !tool_derived_valid(candidate, &ctx.turn) {
            return Some(ArbiterReason::new(
                ArbiterReasonCode::ToolDerivedInvalid,
                "tool-derived candidate failed validity check",
            ));
        }

        None
    }

    fn check_semantic_matches(
        candidate: &MemoryCandidate,
        semantic_matches: &[SemanticMatch],
        ctx: &ArbiterContext<'_>,
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

        // Kind-specific contradictions not caught by semantic search are handled
        // by `check_contradiction` (#351), which matches titles by embedding
        // similarity rather than exact string equality.
        None
    }

    /// Deduplicate tool-derived memories by their stable per-tool title (#349).
    ///
    /// Tool-grounding candidates (`tool failure:{tool}`, `tool:{tool}`,
    /// `tool outcome:{tool}`) carry an empty `source_quote` and a title that is
    /// constant per tool while the content varies with every failure. The
    /// content-based breakwaters (`find_exact_duplicate`, semantic similarity)
    /// therefore miss repeated failures of the same tool, and rows accumulate
    /// without bound. Matching on the stable `(kind, title)` key and superseding
    /// the prior record keeps exactly one active row per tool, so the row count
    /// no longer grows with the number of failures.
    fn check_tool_derived(
        candidate: &MemoryCandidate,
        existing: &[MemoryItem],
        ctx: &ArbiterContext<'_>,
    ) -> Option<CandidateDecision> {
        if !is_tool_derived_candidate(candidate) {
            return None;
        }

        let candidate_title = normalize_text(&candidate.title);
        for mem in existing {
            if mem.kind != candidate.kind || normalize_text(&mem.title) != candidate_title {
                continue;
            }
            let Some(existing_id) = mem.id else {
                continue;
            };

            let mut new_item = candidate_to_new_item(candidate, ctx);
            new_item.supersedes_id = Some(existing_id);
            return Some(CandidateDecision {
                candidate: candidate.clone(),
                action: ArbiterAction::Supersede {
                    new_item,
                    superseded_id: existing_id,
                },
                reason: ArbiterReason::new(
                    ArbiterReasonCode::ToolFailureSupersede,
                    format!(
                        "repeated tool-derived {} supersedes id={existing_id}",
                        kind_label(candidate.kind)
                    ),
                ),
            });
        }

        None
    }

    async fn check_contradiction(
        candidate: &MemoryCandidate,
        existing: &[MemoryItem],
        ctx: &ArbiterContext<'_>,
        matcher: &mut TitleMatcher<'_>,
    ) -> Option<CandidateDecision> {
        if !is_contradiction_kind(candidate.kind) {
            return None;
        }

        // Scan only the most recent same-kind memories (#351). `existing` is
        // ordered by creation time, newest first; a bounded scan keeps the
        // per-turn embedding cost of the title pass predictable (one batched
        // `embed_batch` per kind) even for characters with thousands of
        // memories. Older memories remain covered by the indexed
        // semantic-similarity pass (`check_semantic_matches`) and the exact
        // content dedup, which are both unbounded.
        let same_kind = existing
            .iter()
            .filter(|m| m.kind == candidate.kind)
            .take(MAX_CONTRADICTION_SCAN);

        // Embed every distinct title once, in a single provider call, before
        // scanning — the per-memory `is_match` lookups then hit the cache.
        let distinct_titles = std::iter::once(candidate.title.as_str())
            .chain(same_kind.clone().map(|m| m.title.as_str()))
            .collect::<std::collections::HashSet<_>>();
        matcher.prefetch(distinct_titles).await;

        for mem in same_kind {
            if !matcher.is_match(&candidate.title, &mem.title).await {
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
                action: ArbiterAction::AskConfirmationLater {
                    existing_memory_id: Some(existing_id),
                    existing_memory_title: Some(existing.title.clone()),
                },
                reason: ArbiterReason::new(ask_code, detail),
            });
        }

        None
    }

    async fn apply_one(
        store: &dyn MemoryPort,
        decision: &CandidateDecision,
        ctx: &ArbiterContext<'_>,
    ) -> Result<AppliedDecision, CognitionError> {
        let affect_valence = ctx.affect_valence;
        match &decision.action {
            ArbiterAction::Persist(item) => {
                let id = store
                    .insert_typed_memory(item)
                    .await
                    .map_err(CognitionError::MemoryPort)?;
                Ok(AppliedDecision {
                    decision: decision.clone(),
                    inserted_id: Some(id),
                    updated_existing: false,
                    outcome_rating: Some(affect_valence),
                })
            }
            ArbiterAction::Supersede {
                new_item,
                superseded_id,
            } => {
                let id = store
                    .supersede_typed_memory(new_item, *superseded_id)
                    .await
                    .map_err(CognitionError::MemoryPort)?;
                Ok(AppliedDecision {
                    decision: decision.clone(),
                    inserted_id: Some(id),
                    updated_existing: true,
                    outcome_rating: Some(affect_valence),
                })
            }
            ArbiterAction::MarkUserDeleted { memory_id } => {
                let updated = store
                    .set_memory_status(*memory_id, MemoryStatus::UserDeleted)
                    .await
                    .map_err(CognitionError::MemoryPort)?;
                Ok(AppliedDecision {
                    decision: decision.clone(),
                    inserted_id: None,
                    updated_existing: updated,
                    outcome_rating: Some(affect_valence),
                })
            }
            ArbiterAction::MarkDisputed { memory_id } => {
                let updated = store
                    .set_memory_status(*memory_id, MemoryStatus::Disputed)
                    .await
                    .map_err(CognitionError::MemoryPort)?;
                Ok(AppliedDecision {
                    decision: decision.clone(),
                    inserted_id: None,
                    updated_existing: updated,
                    outcome_rating: Some(affect_valence),
                })
            }
            ArbiterAction::AskConfirmationLater {
                existing_memory_id,
                existing_memory_title,
            } => {
                // Defer the candidate to the user-approval queue (#174) so it
                // can be reviewed and persisted later instead of being dropped.
                // The conflicting memory's identity (when present) is recorded
                // so approval can propagate it to `supersedes_id` (#420 review).
                let pending = PendingCandidate {
                    id: 0,
                    character_id: ctx.character_id.to_string(),
                    user_id: ctx.user_id.to_string(),
                    title: decision.candidate.title.clone(),
                    content: decision.candidate.content.clone(),
                    kind: decision.candidate.kind,
                    confidence: decision.candidate.confidence,
                    reason_detail: decision.reason.detail.clone(),
                    existing_memory_title: existing_memory_title.clone(),
                    existing_memory_id: *existing_memory_id,
                    source_quote: decision.candidate.source_quote.clone(),
                    status: PendingCandidateStatus::Pending,
                    created_at: chrono::Utc::now(),
                };
                let inserted_id = store
                    .insert_pending_candidate(pending)
                    .await
                    .map_err(CognitionError::MemoryPort)?;
                tracing::debug!(
                    component = "MemoryArbiter",
                    character_id = %ctx.character_id,
                    candidate_id = inserted_id,
                    title = %decision.candidate.title,
                    "Deferred memory candidate to user-approval queue"
                );
                Ok(AppliedDecision {
                    decision: decision.clone(),
                    inserted_id: None,
                    updated_existing: false,
                    outcome_rating: Some(affect_valence),
                })
            }
            ArbiterAction::Ignore => Ok(AppliedDecision {
                decision: decision.clone(),
                inserted_id: None,
                updated_existing: false,
                outcome_rating: Some(affect_valence),
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

    // The store has no dedicated tags column, so candidate tags are serialized
    // into the content as a trailing JSON metadata footer. Downstream consumers
    // (recall, export) can detect partial/interrupted episodes from this marker
    // without a schema migration (#M7).
    let content = append_tags_metadata(&candidate.content, &candidate.tags);

    // Interrupted (barge-in / cancelled) episodes are partial and therefore
    // less reliable: deprioritize them in recall scoring by lowering the
    // persisted confidence (#M7).
    let confidence = if is_interrupted_candidate(candidate) {
        MemoryConfidence::new((candidate.confidence - INTERRUPTED_CONFIDENCE_PENALTY).max(0.0))
    } else {
        MemoryConfidence::new(candidate.confidence)
    };

    NewMemoryItem {
        scope,
        character_id: ctx.character_id.to_string(),
        user_id: ctx.user_id.to_string(),
        kind: candidate.kind,
        title: candidate.title.clone(),
        content,
        source,
        source_ref: ctx.source_ref.map(str::to_string),
        confidence,
        salience,
        affect: AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from,
        valid_until,
        status: MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    }
}

/// Confidence penalty applied to memories extracted from interrupted turns (#M7).
const INTERRUPTED_CONFIDENCE_PENALTY: f32 = 0.15;

/// Tag marker used to flag episodes from interrupted (barge-in) turns (#M7).
pub(crate) const INTERRUPTED_TAG: &str = "interrupted";

/// Whether a candidate originated from an interrupted turn (#M7).
fn is_interrupted_candidate(candidate: &MemoryCandidate) -> bool {
    candidate.tags.iter().any(|tag| tag == INTERRUPTED_TAG)
}

/// Serialize candidate tags into a trailing JSON metadata footer so they
/// survive persistence despite the store lacking a tags column (#M7).
///
/// Returns the original content unchanged when there are no tags.
fn append_tags_metadata(content: &str, tags: &[String]) -> String {
    if tags.is_empty() {
        return content.to_string();
    }
    let footer = serde_json::json!({ "tags": tags }).to_string();
    format!("{content}\n\n<!-- ene:tags {footer} -->")
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
        return matches!(
            candidate.kind,
            MemoryKind::Procedure | MemoryKind::Reflection | MemoryKind::Episodic
        ) && !turn.tool_results.is_empty();
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

const fn is_arbitration_visible(status: MemoryStatus) -> bool {
    matches!(
        status,
        MemoryStatus::Active | MemoryStatus::Faded | MemoryStatus::Disputed
    )
}

const fn is_contradiction_kind(kind: MemoryKind) -> bool {
    matches!(
        kind,
        MemoryKind::Preference
            | MemoryKind::UserProfile
            | MemoryKind::Semantic
            | MemoryKind::Relationship
    )
}

/// Title prefixes for tool-derived memories, as generated by
/// [`super::tool_grounding`]. These candidates carry an empty `source_quote`
/// and a stable per-tool title, so their content varies with every failure
/// while the title stays constant. They are therefore deduplicated by title
/// (superseding the prior record) rather than by volatile content (#349).
const TOOL_TITLE_PREFIXES: &[&str] = &["tool failure:", "tool:", "tool outcome:"];

/// Whether a candidate was produced by tool-result grounding (#349).
///
/// Tool-grounding candidates are the only ones built with an empty
/// `source_quote` *and* a `tool*:` title prefix, so the pair of signals
/// distinguishes them from LLM/deterministic candidates without threading
/// provenance through the arbiter.
fn is_tool_derived_candidate(candidate: &MemoryCandidate) -> bool {
    candidate.source_quote.trim().is_empty()
        && TOOL_TITLE_PREFIXES
            .iter()
            .any(|prefix| candidate.title.starts_with(prefix))
}

/// Lightweight validity check for tool-derived candidates (#349).
///
/// Tool-grounding candidates bypass the `source_quote`-in-turn check (their
/// quote is empty by construction), so they used to pass validation
/// unconditionally. This enforces a minimal bar instead:
///
/// - the tool named in the title was actually called in this turn, and
/// - the content carries a non-empty summary beyond the fixed failure template.
fn tool_derived_valid(candidate: &MemoryCandidate, turn: &TurnInput<'_>) -> bool {
    let Some(tool_name) = candidate
        .title
        .split_once(':')
        .map(|(_, rest)| rest.trim())
        .filter(|name| !name.is_empty())
    else {
        return false;
    };

    if !turn
        .tool_results
        .iter()
        .any(|tr| tr.tool_name.trim() == tool_name)
    {
        return false;
    }

    // Strip the fixed failure template; whatever remains is the per-failure
    // summary, which must be non-empty so boilerplate-only rows are rejected.
    let content = candidate.content.trim();
    let summary = content
        .strip_prefix("Tool '")
        .and_then(|rest| rest.split_once("' failed.").map(|(_, tail)| tail))
        .unwrap_or(content);
    let summary = summary
        .trim()
        .trim_start_matches("Avoid repeating the same failed path without new evidence.")
        .trim();
    !summary.is_empty()
}

const fn kind_label(kind: MemoryKind) -> &'static str {
    kind.as_str()
}

fn same_memory_content(candidate: &MemoryCandidate, mem: &MemoryItem) -> bool {
    normalize_text(&candidate.content) == normalize_text(&mem.content) && candidate.kind == mem.kind
}

/// Cap on how many most-recent same-kind memories the title-based
/// contradiction scan examines per candidate (#351).
///
/// The scan embeds every distinct title once per kind per turn; bounding it
/// keeps the embedding cost predictable on the post-turn write path even for
/// characters with very large memory tables. Newest memories are prioritized
/// (the store returns them first), and older rows remain covered by the
/// indexed semantic-similarity pass and exact-content dedup.
const MAX_CONTRADICTION_SCAN: usize = 512;

/// Decides whether two memory titles refer to the same subject for the purpose
/// of contradiction checking (#351).
///
/// When an [`EmbeddingProvider`] is configured, titles are compared by the
/// cosine similarity of their embeddings and a pair matches once the similarity
/// reaches the configured threshold — this is what lets synonymous titles
/// ("職業" vs "仕事", "住んでいる場所" vs "居住地") collapse into one subject
/// instead of being treated as unrelated, and it removes the old hardcoded
/// `"nickname"` / `"呼び方"` keyword list so matching works in any language.
/// Without an embedder (or if embedding ever fails) the matcher degrades to the
/// deterministic exact fallback (NFKC + lowercase + whitespace-collapsed
/// equality), so contradiction checks never silently stop running.
///
/// Embeddings are cached by title for the lifetime of the matcher, so scanning
/// every existing memory per candidate does not re-embed repeated titles.
pub(crate) struct TitleMatcher<'a> {
    /// Embedding provider; `None` selects the exact-match fallback.
    embedder: Option<&'a dyn EmbeddingProvider>,
    /// Cosine-similarity cutoff for a match (embedding path only).
    threshold: f32,
    /// Embeddings computed so far, keyed by exact title string.
    cache: HashMap<String, Vec<f32>>,
    /// Set once embedding fails, after which matching falls back to exact.
    disabled: bool,
}

impl<'a> TitleMatcher<'a> {
    /// Create a matcher. Embeddings are computed lazily and cached.
    fn new(embedder: Option<&'a dyn EmbeddingProvider>, threshold: f32) -> Self {
        Self {
            embedder,
            threshold,
            cache: HashMap::new(),
            disabled: false,
        }
    }

    /// Embed a single title, caching the result. Returns `None` when no embedder
    /// is available, embedding has been disabled by a prior failure, or the
    /// provider rejects this title.
    async fn embed(&mut self, title: &str) -> Option<Vec<f32>> {
        if !self.use_embedding() {
            return None;
        }
        if let Some(embedding) = self.cache.get(title) {
            return Some(embedding.clone());
        }
        self.embed_one(title).await
    }

    /// Embed one title through the provider, caching and guarding the result.
    async fn embed_one(&mut self, title: &str) -> Option<Vec<f32>> {
        let embedder = self.embedder?;
        match embedder
            .embed_batch(&[(title, EmbeddingKind::Summary)])
            .await
        {
            Ok(mut vectors) if vectors.len() == 1 => {
                let embedding = vectors.swap_remove(0);
                if embedding.iter().all(|v| *v == 0.0) {
                    warn!(
                        component = "MemoryArbiter",
                        "contradiction title embedding returned a zero vector; falling back to exact title matching"
                    );
                    self.disabled = true;
                    return None;
                }
                self.cache.insert(title.to_string(), embedding.clone());
                Some(embedding)
            }
            Ok(vectors) => {
                warn!(
                    component = "MemoryArbiter",
                    returned = vectors.len(),
                    "contradiction title embedding batch size mismatch; falling back to exact title matching"
                );
                self.disabled = true;
                None
            }
            Err(error) => {
                warn!(
                    component = "MemoryArbiter",
                    error = %error,
                    "contradiction title embedding failed; falling back to exact title matching"
                );
                self.disabled = true;
                None
            }
        }
    }

    /// Embed every distinct title in `titles` with a single provider call,
    /// populating the cache so the subsequent per-memory scan hits cache only.
    ///
    /// Called once per contradiction scan with all same-kind titles that are
    /// about to be compared, so the per-turn write path issues a small number
    /// of `embed_batch` calls instead of one round trip per title (#351). A
    /// provider failure disables the embedding path for the rest of the batch,
    /// degrading to exact title matching as documented.
    async fn prefetch(&mut self, titles: impl IntoIterator<Item = &str>) {
        if !self.use_embedding() {
            return;
        }
        let missing: Vec<&str> = titles
            .into_iter()
            .filter(|title| !self.cache.contains_key(*title))
            .collect();
        if missing.is_empty() {
            return;
        }
        let Some(embedder) = self.embedder else {
            return;
        };
        let items: Vec<(&str, EmbeddingKind)> = missing
            .iter()
            .map(|t| (*t, EmbeddingKind::Summary))
            .collect();
        match embedder.embed_batch(&items).await {
            Ok(vectors) if vectors.len() == missing.len() => {
                for (title, vector) in missing.iter().zip(vectors) {
                    if vector.iter().all(|v| *v == 0.0) {
                        warn!(
                            component = "MemoryArbiter",
                            "contradiction title embedding returned a zero vector; falling back to exact title matching"
                        );
                        self.disabled = true;
                        self.cache.clear();
                        return;
                    }
                    self.cache.insert(title.to_string(), vector);
                }
            }
            Ok(vectors) => {
                warn!(
                    component = "MemoryArbiter",
                    returned = vectors.len(),
                    requested = missing.len(),
                    "contradiction title embedding batch size mismatch; falling back to exact title matching"
                );
                self.disabled = true;
            }
            Err(error) => {
                warn!(
                    component = "MemoryArbiter",
                    error = %error,
                    "contradiction title embedding failed; falling back to exact title matching"
                );
                self.disabled = true;
            }
        }
    }

    /// Cosine similarity between two titles' embeddings, embedding on demand.
    async fn similarity(&mut self, left: &str, right: &str) -> Option<f32> {
        let left_embedding = self.embed(left).await?;
        let right_embedding = self.embed(right).await?;
        Some(cosine_similarity(&left_embedding, &right_embedding))
    }

    /// Whether the embedding path is currently usable.
    fn use_embedding(&self) -> bool {
        self.embedder.is_some() && !self.disabled
    }

    /// Whether `candidate_title` refers to the same subject as `existing_title`.
    ///
    /// Uses embedding similarity when available; if embedding is unavailable or
    /// fails mid-call, falls back to exact normalized-title equality.
    async fn is_match(&mut self, candidate_title: &str, existing_title: &str) -> bool {
        if self.use_embedding()
            && let Some(similarity) = self.similarity(candidate_title, existing_title).await
        {
            return similarity >= self.threshold;
        }
        normalize_text(candidate_title) == normalize_text(existing_title)
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
    use ene_store::MemoryStore;

    fn ctx(turn: TurnInput<'_>) -> ArbiterContext<'_> {
        ArbiterContext {
            turn,
            character_id: "ene",
            user_id: "user1",
            source_ref: Some("session-1"),
            provenance: CandidateProvenance::Deterministic,
            options: ArbiterOptions::default(),
            semantic_matches: HashMap::new(),
            affect_valence: 0.0,
            embedder: None,
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
            tags: Vec::new(),
        }
    }

    fn decision_action(decisions: &[CandidateDecision]) -> &ArbiterAction {
        &decisions.first().expect("one decision").action
    }

    /// Build a fully-populated [`MemoryItem`] for contradiction tests (#351).
    fn memory_item(
        id: i64,
        kind: MemoryKind,
        title: &str,
        content: &str,
        confidence: f32,
    ) -> MemoryItem {
        MemoryItem {
            id: Some(id),
            scope: MemoryScope::User,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind,
            title: title.into(),
            content: content.into(),
            source: MemorySource::Inferred,
            source_ref: None,
            confidence: MemoryConfidence::new(confidence),
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
            pinned: false,
            faded_at: None,
            commitment_id: None,
        }
    }

    /// Embedder that maps a title to a unit vector keyed by the topic it
    /// mentions, so synonymous titles ("職業"/"仕事" → occupation, "居住地"/
    /// "住所" → residence) share a vector while unrelated topics are
    /// orthogonal. Deterministic and dependency-free (#351).
    struct TopicEmbedder;

    fn topic_vector(text: &str) -> Vec<f32> {
        let lowered = text.to_lowercase();
        if lowered.contains("職業")
            || lowered.contains("仕事")
            || lowered.contains("occupation")
            || lowered.contains("job")
        {
            vec![1.0, 0.0, 0.0, 0.0]
        } else if lowered.contains("居住地")
            || lowered.contains("住所")
            || lowered.contains("residence")
            || lowered.contains("address")
        {
            vec![0.0, 1.0, 0.0, 0.0]
        } else if lowered.contains("coffee") || lowered.contains("コーヒー") {
            vec![0.0, 0.0, 1.0, 0.0]
        } else {
            // Distinct neutral direction so unknown titles do not collide with
            // any topic axis above.
            vec![0.0, 0.0, 0.0, 1.0]
        }
    }

    #[async_trait::async_trait]
    impl EmbeddingProvider for TopicEmbedder {
        async fn embed_batch(
            &self,
            items: &[(&str, EmbeddingKind)],
        ) -> Result<Vec<Vec<f32>>, ene_ai::EmbeddingError> {
            Ok(items.iter().map(|(text, _)| topic_vector(text)).collect())
        }

        fn dimensions(&self) -> usize {
            4
        }

        fn model_name(&self) -> &'static str {
            "topic-test"
        }
    }

    /// Embedder whose every call fails, to exercise the exact-match fallback
    /// that kicks in once embedding is disabled (#351).
    struct FailingEmbedder;
    #[async_trait::async_trait]
    impl EmbeddingProvider for FailingEmbedder {
        async fn embed_batch(
            &self,
            _items: &[(&str, EmbeddingKind)],
        ) -> Result<Vec<Vec<f32>>, ene_ai::EmbeddingError> {
            Err(ene_ai::EmbeddingError::Provider(
                "forced embedding failure".to_string(),
            ))
        }

        fn dimensions(&self) -> usize {
            4
        }

        fn model_name(&self) -> &'static str {
            "failing-test"
        }
    }

    /// An [`ArbiterContext`] wired to a specific embedder (#351).
    fn ctx_with_embedder<'a>(
        turn: TurnInput<'a>,
        embedder: &'a dyn EmbeddingProvider,
    ) -> ArbiterContext<'a> {
        ArbiterContext {
            embedder: Some(embedder),
            ..ctx(turn)
        }
    }

    /// `TopicEmbedder` that also counts `embed_batch` invocations, to pin the
    /// once-per-scan batching property of the contradiction title pass (#351).
    struct CountingEmbedder {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl EmbeddingProvider for CountingEmbedder {
        async fn embed_batch(
            &self,
            items: &[(&str, EmbeddingKind)],
        ) -> Result<Vec<Vec<f32>>, ene_ai::EmbeddingError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(items.iter().map(|(text, _)| topic_vector(text)).collect())
        }

        fn dimensions(&self) -> usize {
            4
        }

        fn model_name(&self) -> &'static str {
            "counting-test"
        }
    }

    #[tokio::test]
    async fn low_confidence_is_rejected() {
        let turn = TurnInput {
            user_message: "remember project X",
            assistant_message: None,
            tool_results: &[],
        };
        let decisions =
            MemoryArbiter::evaluate_all(&[sample_candidate(0.4)], &[], &ctx(turn)).await;
        assert!(matches!(decision_action(&decisions), ArbiterAction::Ignore));
        assert_eq!(decisions[0].reason.code, ArbiterReasonCode::LowConfidence);
    }

    #[tokio::test]
    async fn source_quote_mismatch_is_rejected() {
        let turn = TurnInput {
            user_message: "hello there",
            assistant_message: None,
            tool_results: &[],
        };
        let decisions =
            MemoryArbiter::evaluate_all(&[sample_candidate(0.9)], &[], &ctx(turn)).await;
        assert!(matches!(decision_action(&decisions), ArbiterAction::Ignore));
        assert_eq!(
            decisions[0].reason.code,
            ArbiterReasonCode::SourceQuoteNotInTurn
        );
    }

    #[tokio::test]
    async fn valid_candidate_persists() {
        let turn = TurnInput {
            user_message: "remember project X",
            assistant_message: None,
            tool_results: &[],
        };
        let decisions =
            MemoryArbiter::evaluate_all(&[sample_candidate(0.9)], &[], &ctx(turn)).await;
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
            pinned: false,
            created_at: None,
            commitment_id: None,
        };
        let id = store.insert_typed_memory(&item).await.unwrap();
        let existing = store.get_typed_memory(id).await.unwrap().unwrap();

        let turn = TurnInput {
            user_message: "remember project X",
            assistant_message: None,
            tool_results: &[],
        };
        let decisions =
            MemoryArbiter::evaluate_all(&[sample_candidate(0.9)], &[existing], &ctx(turn)).await;
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
            pinned: false,
            created_at: None,
            commitment_id: None,
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
            tags: Vec::new(),
        };
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[existing], &ctx(turn)).await;
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
            pinned: false,
            created_at: None,
            commitment_id: None,
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
            tags: Vec::new(),
        };
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[existing], &ctx(turn)).await;
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::MarkUserDeleted { memory_id } if memory_id == &id
        ));
        assert_eq!(decisions[0].reason.code, ArbiterReasonCode::DeletionRequest);
    }

    #[tokio::test]
    async fn procedure_with_empty_source_quote_allowed_when_tools_present() {
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
            tags: Vec::new(),
        };
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[], &ctx(turn)).await;
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::Persist(_)
        ));
    }

    #[tokio::test]
    async fn reflection_with_empty_source_quote_allowed_when_tools_present() {
        let tools = [ToolResultSummary {
            tool_name: "web".to_string(),
            success: false,
            summary: "error: timeout".to_string(),
        }];
        let turn = TurnInput {
            user_message: "search docs",
            assistant_message: None,
            tool_results: &tools,
        };
        let candidate = MemoryCandidate {
            kind: MemoryKind::Reflection,
            title: "tool failure:web".to_string(),
            content: "Tool failed due to timeout".to_string(),
            source_quote: String::new(),
            confidence: 0.7,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
            tags: Vec::new(),
        };
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[], &ctx(turn)).await;
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::Persist(_)
        ));
    }

    fn tool_failure_memory(id: i64, content: &str) -> MemoryItem {
        MemoryItem {
            id: Some(id),
            scope: MemoryScope::Shared,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind: MemoryKind::Reflection,
            title: "tool failure:web".into(),
            content: content.into(),
            source: MemorySource::Inferred,
            source_ref: None,
            confidence: MemoryConfidence::new(0.65),
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
            pinned: false,
            faded_at: None,
            commitment_id: None,
        }
    }

    fn tool_failure_candidate(summary: &str) -> MemoryCandidate {
        MemoryCandidate {
            kind: MemoryKind::Reflection,
            title: "tool failure:web".to_string(),
            content: format!(
                "Tool 'web' failed. Avoid repeating the same failed path without new evidence. {summary}"
            ),
            source_quote: String::new(),
            confidence: 0.65,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
            tags: Vec::new(),
        }
    }

    /// #349: a repeated failure of the same tool (same stable title, different
    /// error text) must supersede the prior record rather than add a new row.
    #[tokio::test]
    async fn repeated_tool_failure_supersedes_prior_record() {
        let tools = [ToolResultSummary {
            tool_name: "web".to_string(),
            success: false,
            summary: "error: connection reset".to_string(),
        }];
        let turn = TurnInput {
            user_message: "search docs",
            assistant_message: None,
            tool_results: &tools,
        };
        let existing = tool_failure_memory(
            11,
            "Tool 'web' failed. Avoid repeating the same failed path without new evidence. error: timeout",
        );
        let decisions = MemoryArbiter::evaluate_all(
            &[tool_failure_candidate("error: connection reset")],
            &[existing],
            &ctx(turn),
        )
        .await;
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::Supersede {
                superseded_id: 11,
                ..
            }
        ));
        assert_eq!(
            decisions[0].reason.code,
            ArbiterReasonCode::ToolFailureSupersede
        );
    }

    /// #349 acceptance: repeated failures of the same tool do not accumulate
    /// unbounded active rows — each new failure supersedes the previous one,
    /// leaving exactly one active Reflection that carries the latest error.
    #[tokio::test]
    async fn repeated_tool_failures_do_not_accumulate_rows() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();

        for error in [
            "error: timeout",
            "error: connection reset",
            "error: dns failure",
        ] {
            let tools = [ToolResultSummary {
                tool_name: "web".to_string(),
                success: false,
                summary: error.to_string(),
            }];
            let turn = TurnInput {
                user_message: "search docs",
                assistant_message: None,
                tool_results: &tools,
            };
            MemoryArbiter::arbitrate_and_apply(
                &store,
                &[tool_failure_candidate(error)],
                &ctx(turn),
            )
            .await
            .unwrap();
        }

        let active = store
            .get_typed_memories_by_character("ene", Some(MemoryKind::Reflection), 100, 0)
            .await
            .unwrap()
            .into_iter()
            .filter(|m| m.status == MemoryStatus::Active)
            .collect::<Vec<_>>();
        assert_eq!(
            active.len(),
            1,
            "repeated failures of the same tool must supersede, not accumulate"
        );
        assert!(
            active[0].content.contains("dns failure"),
            "the latest failure should be the surviving record: {}",
            active[0].content
        );
    }

    /// #349 validity: a tool-derived candidate naming a tool that was not
    /// actually called in the turn is rejected.
    #[tokio::test]
    async fn tool_derived_candidate_with_unknown_tool_is_rejected() {
        let tools = [ToolResultSummary {
            tool_name: "fs".to_string(),
            success: false,
            summary: "error: permission denied".to_string(),
        }];
        let turn = TurnInput {
            user_message: "read file",
            assistant_message: None,
            tool_results: &tools,
        };
        // Title names "web", but the tool actually called was "fs".
        let decisions = MemoryArbiter::evaluate_all(
            &[tool_failure_candidate("error: timeout")],
            &[],
            &ctx(turn),
        )
        .await;
        assert!(matches!(decision_action(&decisions), ArbiterAction::Ignore));
        assert_eq!(
            decisions[0].reason.code,
            ArbiterReasonCode::ToolDerivedInvalid
        );
    }

    /// #349 validity: a tool-derived candidate whose content is only the fixed
    /// failure boilerplate (no real summary) is rejected.
    #[tokio::test]
    async fn tool_derived_candidate_with_boilerplate_only_content_is_rejected() {
        let tools = [ToolResultSummary {
            tool_name: "web".to_string(),
            success: false,
            summary: "error: timeout".to_string(),
        }];
        let turn = TurnInput {
            user_message: "search docs",
            assistant_message: None,
            tool_results: &tools,
        };
        let candidate = MemoryCandidate {
            kind: MemoryKind::Reflection,
            title: "tool failure:web".to_string(),
            content:
                "Tool 'web' failed. Avoid repeating the same failed path without new evidence."
                    .to_string(),
            source_quote: String::new(),
            confidence: 0.7,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
            tags: Vec::new(),
        };
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[], &ctx(turn)).await;
        assert!(matches!(decision_action(&decisions), ArbiterAction::Ignore));
        assert_eq!(
            decisions[0].reason.code,
            ArbiterReasonCode::ToolDerivedInvalid
        );
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
            pinned: false,
            created_at: None,
            commitment_id: None,
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
            tags: Vec::new(),
        };
        let arbiter_ctx = ctx(turn);
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[existing], &arbiter_ctx).await;
        let applied = MemoryArbiter::apply_decisions(&store, &decisions, &arbiter_ctx)
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

    /// `apply_decisions` is documented as non-transactional: when a later
    /// decision fails, the earlier decisions in the batch stay committed and
    /// are not rolled back. This test pins that behavior so a future move to
    /// transactional batching is a deliberate, noticed change.
    #[tokio::test]
    async fn apply_decisions_is_non_transactional_on_partial_failure() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();

        let candidate = sample_candidate(0.9);

        let persist_item = NewMemoryItem {
            scope: MemoryScope::User,
            character_id: "ene".to_string(),
            user_id: "user1".to_string(),
            kind: MemoryKind::Semantic,
            title: "project X".to_string(),
            content: "User is working on project X".to_string(),
            source: MemorySource::Inferred,
            source_ref: None,
            confidence: MemoryConfidence::new(0.9),
            salience: MemorySalience::default(),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        };

        let decisions = vec![
            CandidateDecision {
                candidate: candidate.clone(),
                action: ArbiterAction::Persist(persist_item),
                reason: ArbiterReason::new(ArbiterReasonCode::ValidNewMemory, "first succeeds"),
            },
            CandidateDecision {
                candidate,
                // Superseding a non-existent id fails inside `apply_one`.
                action: ArbiterAction::Supersede {
                    new_item: NewMemoryItem {
                        scope: MemoryScope::User,
                        character_id: "ene".to_string(),
                        user_id: "user1".to_string(),
                        kind: MemoryKind::Semantic,
                        title: "replacement".to_string(),
                        content: "replacement content".to_string(),
                        source: MemorySource::Inferred,
                        source_ref: None,
                        confidence: MemoryConfidence::new(0.9),
                        salience: MemorySalience::default(),
                        affect: AffectAnnotation::default(),
                        relationship_impact: 0.0,
                        valid_from: None,
                        valid_until: None,
                        status: MemoryStatus::Active,
                        supersedes_id: None,
                        pinned: false,
                        created_at: None,
                        commitment_id: None,
                    },
                    superseded_id: 999_999,
                },
                reason: ArbiterReason::new(
                    ArbiterReasonCode::ContradictionSupersede,
                    "second fails",
                ),
            },
        ];

        let turn = TurnInput {
            user_message: "I am working on project X",
            assistant_message: None,
            tool_results: &[],
        };
        let arbiter_ctx = ctx(turn);
        let result = MemoryArbiter::apply_decisions(&store, &decisions, &arbiter_ctx).await;
        assert!(
            result.is_err(),
            "the batch must surface the second decision's failure"
        );

        // The first decision's write survived even though the batch failed.
        let count = store.count_typed_memories("ene", None).await.unwrap();
        assert_eq!(
            count, 1,
            "non-transactional batch must keep the first decision's committed write"
        );
    }

    #[tokio::test]
    async fn batch_duplicate_second_candidate_ignored() {
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
            tags: Vec::new(),
        };
        let second = MemoryCandidate {
            content: "Different detail about project X".to_string(),
            ..first.clone()
        };
        let decisions = MemoryArbiter::evaluate_all(&[first, second], &[], &ctx(turn)).await;
        assert_eq!(decisions.len(), 2);
        assert!(matches!(&decisions[0].action, ArbiterAction::Persist(_)));
        assert!(matches!(decisions[1].action, ArbiterAction::Ignore));
        assert_eq!(decisions[1].reason.code, ArbiterReasonCode::BatchDuplicate);
    }

    #[tokio::test]
    async fn semantic_duplicate_is_ignored() {
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
            pinned: false,
            faded_at: None,
            commitment_id: None,
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
        let decisions =
            MemoryArbiter::evaluate_all(&[sample_candidate(0.9)], &[], &arbiter_ctx).await;
        assert!(matches!(decision_action(&decisions), ArbiterAction::Ignore));
        assert_eq!(
            decisions[0].reason.code,
            ArbiterReasonCode::SemanticDuplicate
        );
    }

    #[tokio::test]
    async fn semantic_supersede_picks_strongest_match() {
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
            pinned: false,
            faded_at: None,
            commitment_id: None,
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
            tags: Vec::new(),
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
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[], &arbiter_ctx).await;
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::Supersede {
                superseded_id: 2,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn semantic_disputed_when_confidence_close() {
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
            pinned: false,
            faded_at: None,
            commitment_id: None,
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
            tags: Vec::new(),
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
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[], &arbiter_ctx).await;
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::MarkDisputed { memory_id: 7 }
        ));
        assert_eq!(
            decisions[0].reason.code,
            ArbiterReasonCode::ContradictionDisputed
        );
    }

    #[tokio::test]
    async fn semantic_ask_confirmation_when_existing_much_stronger() {
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
            pinned: false,
            faded_at: None,
            commitment_id: None,
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
            tags: Vec::new(),
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
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[], &arbiter_ctx).await;
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::AskConfirmationLater { .. }
        ));
        assert_eq!(
            decisions[0].reason.code,
            ArbiterReasonCode::AskUserConfirmation
        );
    }

    /// #420 review (MEDIUM 5): a weak contradiction that defers to the user
    /// records the conflicting memory's identity on the action, and applying
    /// the decision persists it onto the pending candidate so approval can
    /// later propagate it to `supersedes_id`.
    #[tokio::test]
    async fn ask_confirmation_captures_conflicting_memory() {
        use crate::memory_writer::test_support::InMemoryMemoryPort;

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
            pinned: false,
            faded_at: None,
            commitment_id: None,
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
            tags: Vec::new(),
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
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[], &arbiter_ctx).await;

        // The decision carries the conflicting memory's id + title.
        match decision_action(&decisions) {
            ArbiterAction::AskConfirmationLater {
                existing_memory_id,
                existing_memory_title,
            } => {
                assert_eq!(*existing_memory_id, Some(8));
                assert_eq!(existing_memory_title.as_deref(), Some("love: coffee"));
            }
            other => panic!("expected AskConfirmationLater, got {other:?}"),
        }

        // Applying the decision persists both onto the deferred candidate.
        let port = InMemoryMemoryPort::new();
        MemoryArbiter::apply_decisions(&port, &decisions, &arbiter_ctx)
            .await
            .expect("apply");
        let queued = port.pending_candidates();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].existing_memory_id, Some(8));
        assert_eq!(
            queued[0].existing_memory_title.as_deref(),
            Some("love: coffee")
        );
    }

    #[tokio::test]
    async fn deletion_low_confidence_is_rejected() {
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
            tags: Vec::new(),
        };
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[], &ctx(turn)).await;
        assert!(matches!(decision_action(&decisions), ArbiterAction::Ignore));
        assert_eq!(decisions[0].reason.code, ArbiterReasonCode::LowConfidence);
    }

    #[tokio::test]
    async fn deletion_quote_mismatch_is_rejected() {
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
            tags: Vec::new(),
        };
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[], &ctx(turn)).await;
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

    /// Demonstrates the #270 decoupling: the arbiter's cognitive logic
    /// (validation, dedup, contradiction resolution) runs against
    /// `&dyn MemoryPort` and can be exercised with a plain in-memory test
    /// double — no `SQLite`, no `ene_store::MemoryStore` — while still
    /// covering the full persist → supersede flow through the trait.
    #[tokio::test]
    async fn arbitrate_and_apply_works_against_in_memory_port_without_sqlite() {
        use crate::memory_writer::test_support::InMemoryMemoryPort;

        let port = InMemoryMemoryPort::new();
        let turn = TurnInput {
            user_message: "remember project X",
            assistant_message: None,
            tool_results: &[],
        };
        let arbiter_ctx = ctx(turn);

        // First candidate persists as a brand-new memory.
        let applied =
            MemoryArbiter::arbitrate_and_apply(&port, &[sample_candidate(0.9)], &arbiter_ctx)
                .await
                .unwrap();
        assert_eq!(applied.len(), 1);
        assert!(matches!(
            applied[0].decision.action,
            ArbiterAction::Persist(_)
        ));
        let first_id = applied[0].inserted_id.expect("persisted id");

        let stored = port.all_items();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].title, "project X");
        assert_eq!(stored[0].status, MemoryStatus::Active);
        assert_eq!(stored[0].source, MemorySource::Inferred);

        // A higher-confidence candidate with different content for the same
        // (title-adjacent) semantic key supersedes the first row — exercises
        // the arbiter's contradiction-resolution path, not just insertion.
        let mut stronger = sample_candidate(0.97);
        stronger.content = "User is now working on project Y instead".to_string();
        let semantic_matches = HashMap::from([(
            0,
            vec![SemanticMatch {
                memory_id: first_id,
                similarity: 0.9,
                memory: stored[0].clone(),
            }],
        )]);
        let arbiter_ctx_2 = ArbiterContext {
            semantic_matches,
            ..ctx(TurnInput {
                user_message: "remember project X",
                assistant_message: None,
                tool_results: &[],
            })
        };
        let applied_2 = MemoryArbiter::arbitrate_and_apply(&port, &[stronger], &arbiter_ctx_2)
            .await
            .unwrap();
        assert_eq!(applied_2.len(), 1);
        assert!(matches!(
            applied_2[0].decision.action,
            ArbiterAction::Supersede { .. }
        ));

        let final_items = port.all_items();
        let old = final_items
            .iter()
            .find(|m| m.id == Some(first_id))
            .expect("original row still present");
        assert_eq!(old.status, MemoryStatus::Superseded);
        assert!(
            final_items
                .iter()
                .any(|m| m.status == MemoryStatus::Active && m.content.contains("project Y"))
        );
    }

    #[tokio::test]
    async fn apply_records_outcome_rating_from_affect_valence() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let turn = TurnInput {
            user_message: "remember project X",
            assistant_message: None,
            tool_results: &[],
        };
        let arbiter_ctx = ArbiterContext {
            affect_valence: 0.75,
            ..ctx(turn)
        };
        let applied =
            MemoryArbiter::arbitrate_and_apply(&store, &[sample_candidate(0.9)], &arbiter_ctx)
                .await
                .unwrap();

        assert_eq!(applied.len(), 1);
        assert_eq!(
            applied[0].outcome_rating,
            Some(0.75),
            "outcome_rating must mirror the turn affect valence"
        );
    }

    #[tokio::test]
    async fn ask_confirmation_later_defers_to_pending_candidates() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let turn = TurnInput {
            user_message: "I might have switched to tea",
            assistant_message: None,
            tool_results: &[],
        };
        // Existing memory with a close-but-not-dominant confidence gap so the
        // arbiter defers to user confirmation instead of superseding.
        let existing_item = NewMemoryItem {
            scope: MemoryScope::User,
            character_id: "ene".to_string(),
            user_id: "user1".to_string(),
            kind: MemoryKind::Preference,
            title: "drink".to_string(),
            content: "likes coffee".to_string(),
            source: MemorySource::Inferred,
            source_ref: None,
            confidence: MemoryConfidence::new(0.85),
            salience: MemorySalience::default(),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        };
        let existing = store.insert_typed_memory(&existing_item).await.unwrap();
        let existing = store.get_typed_memory(existing).await.unwrap().unwrap();

        let candidate = MemoryCandidate {
            kind: MemoryKind::Preference,
            title: "drink".to_string(),
            content: "likes tea".to_string(),
            source_quote: "I might have switched to tea".to_string(),
            // Much weaker than the existing memory (delta < -dispute_gap) but
            // still above min_confidence, so the arbiter defers to the user.
            confidence: 0.66,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
            tags: Vec::new(),
        };
        let arbiter_ctx = ctx(turn);
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[existing], &arbiter_ctx).await;
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::AskConfirmationLater { .. }
        ));

        let applied = MemoryArbiter::apply_decisions(&store, &decisions, &arbiter_ctx)
            .await
            .unwrap();
        assert_eq!(applied.len(), 1);
        assert!(applied[0].inserted_id.is_none());

        // The candidate was deferred to the user-approval queue.
        let pending = store
            .list_pending_candidates("ene", Some(ene_store::PendingCandidateStatus::Pending))
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].title, "drink");
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
            pinned: false,
            created_at: None,
            commitment_id: None,
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
            tags: Vec::new(),
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
            pinned: false,
            created_at: None,
            commitment_id: None,
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
            tags: Vec::new(),
        };
        let arbiter_ctx = ctx(turn);
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[existing], &arbiter_ctx).await;
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::MarkDisputed { memory_id } if memory_id == &id
        ));

        let applied = MemoryArbiter::apply_decisions(&store, &decisions, &arbiter_ctx)
            .await
            .unwrap();
        assert!(applied[0].updated_existing);

        let mem = store.get_typed_memory(id).await.unwrap().unwrap();
        assert_eq!(mem.status, MemoryStatus::Disputed);
    }

    #[tokio::test]
    async fn commitment_due_not_mapped_to_valid_until() {
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
            tags: Vec::new(),
        };
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[], &ctx(turn)).await;
        let ArbiterAction::Persist(item) = decision_action(&decisions) else {
            panic!("expected Persist");
        };
        assert!(item.valid_until.is_none());
        assert!(item.valid_from.is_none());
    }

    #[tokio::test]
    async fn interrupted_candidate_confidence_is_deprioritized() {
        // An interrupted (barge-in) episode is partial and therefore less
        // reliable: the persisted confidence is reduced by the penalty (#M7).
        let turn = TurnInput {
            user_message: "remember project X",
            assistant_message: None,
            tool_results: &[],
        };
        let mut candidate = sample_candidate(0.9);
        candidate.tags.push(INTERRUPTED_TAG.to_string());
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[], &ctx(turn)).await;
        let ArbiterAction::Persist(item) = decision_action(&decisions) else {
            panic!("expected Persist");
        };
        let expected = 0.9 - INTERRUPTED_CONFIDENCE_PENALTY;
        assert!(
            (item.confidence.get() - expected).abs() < 1e-6,
            "interrupted confidence {} should be penalized to {expected}",
            item.confidence.get()
        );
    }

    #[tokio::test]
    async fn interrupted_candidate_tags_are_serialized_into_content() {
        // The store has no tags column, so tags are serialized into the
        // content as a metadata footer (#M7).
        let turn = TurnInput {
            user_message: "remember project X",
            assistant_message: None,
            tool_results: &[],
        };
        let mut candidate = sample_candidate(0.9);
        candidate.tags.push(INTERRUPTED_TAG.to_string());
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[], &ctx(turn)).await;
        let ArbiterAction::Persist(item) = decision_action(&decisions) else {
            panic!("expected Persist");
        };
        assert!(
            item.content.contains("ene:tags"),
            "content should carry the tags metadata footer: {}",
            item.content
        );
        assert!(item.content.contains(INTERRUPTED_TAG));
    }

    #[tokio::test]
    async fn non_interrupted_candidate_has_no_tags_footer() {
        let turn = TurnInput {
            user_message: "remember project X",
            assistant_message: None,
            tool_results: &[],
        };
        let decisions =
            MemoryArbiter::evaluate_all(&[sample_candidate(0.9)], &[], &ctx(turn)).await;
        let ArbiterAction::Persist(item) = decision_action(&decisions) else {
            panic!("expected Persist");
        };
        assert!(
            !item.content.contains("ene:tags"),
            "untagged content must not gain a metadata footer"
        );
    }

    #[test]
    fn append_tags_metadata_empty_tags_is_identity() {
        let content = "plain content";
        assert_eq!(append_tags_metadata(content, &[]), content);
    }

    // ── #351: contradiction-key matching by title embedding similarity ──

    /// Synonymous titles ("職業" vs "仕事") are matched by embedding similarity
    /// and detected as a contradiction, where exact-title matching missed them.
    #[tokio::test]
    async fn synonym_titles_are_detected_as_contradiction() {
        let turn = TurnInput {
            user_message: "実は仕事は教師です",
            assistant_message: None,
            tool_results: &[],
        };
        let existing = memory_item(11, MemoryKind::UserProfile, "職業", "エンジニア", 0.7);
        let candidate = MemoryCandidate {
            kind: MemoryKind::UserProfile,
            title: "仕事".to_string(),
            content: "教師".to_string(),
            source_quote: "実は仕事は教師です".to_string(),
            confidence: 0.9,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
            tags: Vec::new(),
        };
        let embedder = TopicEmbedder;
        let arbiter_ctx = ctx_with_embedder(turn, &embedder);
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[existing], &arbiter_ctx).await;
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::Supersede { .. }
        ));
        assert_eq!(
            decisions[0].reason.code,
            ArbiterReasonCode::ContradictionSupersede
        );
    }

    /// `Semantic` memories were previously skipped by the `_ => false` arm of
    /// `same_contradiction_key` (which only special-cased `Preference` and
    /// `UserProfile`), so even synonymous titles never contradicted. With
    /// embedding matching they now do.
    #[tokio::test]
    async fn semantic_kind_contradiction_now_matches() {
        let turn = TurnInput {
            user_message: "my job is teaching",
            assistant_message: None,
            tool_results: &[],
        };
        // Synonymous titles ("occupation" / "job") share the occupation vector;
        // the old `_ => false` arm never even compared Semantic titles.
        let existing = memory_item(
            21,
            MemoryKind::Semantic,
            "occupation",
            "works as an engineer",
            0.7,
        );
        let candidate = MemoryCandidate {
            kind: MemoryKind::Semantic,
            title: "job".to_string(),
            content: "works as a teacher".to_string(),
            source_quote: "my job is teaching".to_string(),
            confidence: 0.9,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
            tags: Vec::new(),
        };
        let embedder = TopicEmbedder;
        let arbiter_ctx = ctx_with_embedder(turn, &embedder);
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[existing], &arbiter_ctx).await;
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::Supersede { .. }
        ));
        assert_eq!(
            decisions[0].reason.code,
            ArbiterReasonCode::ContradictionSupersede
        );
    }

    /// The title pass must issue **one** `embed_batch` call per scan, not one
    /// per existing memory: a character with many same-kind memories would
    /// otherwise cost a provider round trip per title on the post-turn write
    /// path (#351).
    #[tokio::test]
    async fn contradiction_scan_embeds_distinct_titles_in_one_batch() {
        let turn = TurnInput {
            user_message: "I prefer lattes",
            assistant_message: None,
            tool_results: &[],
        };
        // 60 same-kind memories with 10 distinct titles, all unrelated to the
        // candidate (each distinct title is embedded exactly once in the batch).
        let titles: Vec<String> = (0..10).map(|i| format!("preference {i}")).collect();
        let existing: Vec<MemoryItem> = (0..60)
            .map(|i| {
                memory_item(
                    i + 100,
                    MemoryKind::Preference,
                    &titles[i as usize % 10],
                    &format!("content {i}"),
                    0.7,
                )
            })
            .collect();
        let candidate = MemoryCandidate {
            kind: MemoryKind::Preference,
            title: "コーヒー".to_string(),
            content: "ラテが好き".to_string(),
            source_quote: "I prefer lattes".to_string(),
            confidence: 0.9,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
            tags: Vec::new(),
        };

        let embedder = CountingEmbedder {
            calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        let arbiter_ctx = ctx_with_embedder(turn, &embedder);
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &existing, &arbiter_ctx).await;

        // No false positive from the unrelated titles.
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::Persist(_)
        ));
        // 10 distinct existing titles + the candidate title, all in one call.
        assert_eq!(embedder.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// `Relationship` memories were also skipped by `_ => false`; they now match
    /// on synonymous titles via embeddings.
    #[tokio::test]
    async fn relationship_kind_contradiction_now_matches() {
        let turn = TurnInput {
            user_message: "my address is Osaka now",
            assistant_message: None,
            tool_results: &[],
        };
        // Synonymous titles ("residence" / "address") share the residence vector.
        let existing = memory_item(
            31,
            MemoryKind::Relationship,
            "residence",
            "lives in Tokyo",
            0.7,
        );
        let candidate = MemoryCandidate {
            kind: MemoryKind::Relationship,
            title: "address".to_string(),
            content: "lives in Osaka".to_string(),
            source_quote: "my address is Osaka now".to_string(),
            confidence: 0.9,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
            tags: Vec::new(),
        };
        let embedder = TopicEmbedder;
        let arbiter_ctx = ctx_with_embedder(turn, &embedder);
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[existing], &arbiter_ctx).await;
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::Supersede { .. }
        ));
        assert_eq!(
            decisions[0].reason.code,
            ArbiterReasonCode::ContradictionSupersede
        );
    }

    /// Unrelated titles stay below the similarity threshold, so no false
    /// contradiction is raised and the candidate persists as new.
    #[tokio::test]
    async fn unrelated_titles_do_not_contradict() {
        let turn = TurnInput {
            user_message: "my address is Kyoto",
            assistant_message: None,
            tool_results: &[],
        };
        // Occupation topic vs residence topic → orthogonal embeddings.
        let existing = memory_item(41, MemoryKind::UserProfile, "職業", "エンジニア", 0.7);
        let candidate = MemoryCandidate {
            kind: MemoryKind::UserProfile,
            title: "住所".to_string(),
            content: "京都".to_string(),
            source_quote: "my address is Kyoto".to_string(),
            confidence: 0.9,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
            tags: Vec::new(),
        };
        let embedder = TopicEmbedder;
        let arbiter_ctx = ctx_with_embedder(turn, &embedder);
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[existing], &arbiter_ctx).await;
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::Persist(_)
        ));
        assert_eq!(decisions[0].reason.code, ArbiterReasonCode::ValidNewMemory);
    }

    /// The old hardcoded `"nickname"` / `"呼び方"` keyword list is gone: a
    /// rephrased profile title that shares no keyword but is semantically the
    /// same subject still matches via embeddings.
    #[tokio::test]
    async fn user_profile_matches_without_hardcoded_keyword() {
        let turn = TurnInput {
            user_message: "call me Taro from now on",
            assistant_message: None,
            tool_results: &[],
        };
        // Neither title contains "nickname" or "呼び方"; both map to the same
        // neutral topic vector, so similarity is 1.0.
        let existing = memory_item(51, MemoryKind::UserProfile, "how to call me", "Ken", 0.7);
        let candidate = MemoryCandidate {
            kind: MemoryKind::UserProfile,
            title: "preferred name".to_string(),
            content: "Taro".to_string(),
            source_quote: "call me Taro from now on".to_string(),
            confidence: 0.9,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
            tags: Vec::new(),
        };
        let embedder = TopicEmbedder;
        let arbiter_ctx = ctx_with_embedder(turn, &embedder);
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[existing], &arbiter_ctx).await;
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::Supersede { .. }
        ));
    }

    /// When the embedder fails, matching degrades to exact normalized-title
    /// equality so contradiction checks never silently stop (#351).
    #[tokio::test]
    async fn embedding_failure_falls_back_to_exact_title() {
        let turn = TurnInput {
            user_message: "I love tea now",
            assistant_message: None,
            tool_results: &[],
        };
        // Identical titles → exact fallback still matches despite embed failure.
        let existing = memory_item(61, MemoryKind::Preference, "drink", "likes coffee", 0.7);
        let candidate = MemoryCandidate {
            kind: MemoryKind::Preference,
            title: "drink".to_string(),
            content: "likes tea".to_string(),
            source_quote: "I love tea now".to_string(),
            confidence: 0.9,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
            tags: Vec::new(),
        };
        let embedder = FailingEmbedder;
        let arbiter_ctx = ctx_with_embedder(turn, &embedder);
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[existing], &arbiter_ctx).await;
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::Supersede { .. }
        ));
        assert_eq!(
            decisions[0].reason.code,
            ArbiterReasonCode::ContradictionSupersede
        );
    }

    /// Without any embedder configured, exact normalized-title equality is the
    /// deterministic fallback (the default test context has `embedder: None`).
    #[tokio::test]
    async fn no_embedder_uses_exact_title_fallback() {
        let turn = TurnInput {
            user_message: "I love tea now",
            assistant_message: None,
            tool_results: &[],
        };
        let existing = memory_item(71, MemoryKind::Preference, "drink", "likes coffee", 0.7);
        let candidate = MemoryCandidate {
            kind: MemoryKind::Preference,
            title: "drink".to_string(),
            content: "likes tea".to_string(),
            source_quote: "I love tea now".to_string(),
            confidence: 0.9,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
            tags: Vec::new(),
        };
        let decisions = MemoryArbiter::evaluate_all(&[candidate], &[existing], &ctx(turn)).await;
        assert!(matches!(
            decision_action(&decisions),
            ArbiterAction::Supersede { .. }
        ));
    }
}
