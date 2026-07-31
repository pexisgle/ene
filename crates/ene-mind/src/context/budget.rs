//! Context budget allocation and overflow drop policy (#81).

use std::collections::HashSet;

use ene_core::ActiveCommitmentPrompt;

use ene_config::UserPersona;

use crate::character::{AuthorsNote, IdentityKernel, StyleExample, apply_authors_note};
use crate::config::ContextConfig;
use crate::error::CognitionError;
use crate::lifecycle::HistoryEntry;
use crate::prompt_packet::{
    PromptPacket, PromptSection, PromptSectionKind, classify_recalled_memories,
    render_commitments_block,
};
use crate::recall::{RecallBudgetHints, RecalledMemory, format_recalled_content};

use super::tokens::{estimate_tokens, tokens_to_chars, truncate_to_tokens};

/// Fixed and dynamic token budgets for prompt packing.
#[derive(Debug, Clone)]
pub struct ContextBudget {
    /// Total prompt token ceiling.
    pub total_tokens: usize,
    /// Per-section dynamic budgets.
    pub section_budgets: [usize; 13],
}

impl ContextBudget {
    /// Build a budget from [`ContextConfig`].
    pub const fn from_config(config: &ContextConfig) -> Self {
        let mut section_budgets = [0usize; 13];
        section_budgets[PromptSectionKind::SceneState as usize] = config.scene_summary_tokens;
        section_budgets[PromptSectionKind::SemanticContext as usize] =
            config.semantic_budget_tokens;
        section_budgets[PromptSectionKind::EpisodicMemories as usize] = config.memory_budget_tokens;
        section_budgets[PromptSectionKind::StyleExamples as usize] =
            config.style_example_budget_tokens;
        section_budgets[PromptSectionKind::CharacterState as usize] = 200;
        section_budgets[PromptSectionKind::UserProfile as usize] = config.memory_budget_tokens / 2;
        section_budgets[PromptSectionKind::ActiveCommitments as usize] = 400;
        Self {
            total_tokens: config.max_prompt_tokens,
            section_budgets,
        }
    }

    const fn budget_for(&self, kind: PromptSectionKind) -> usize {
        self.section_budgets[kind as usize]
    }

    /// Build a budget from config, overriding memory section limits with recall hints (#72).
    pub const fn from_config_and_hints(config: &ContextConfig, hints: &RecallBudgetHints) -> Self {
        let mut budget = Self::from_config(config);
        budget.section_budgets[PromptSectionKind::SemanticContext as usize] =
            hints.semantic_budget_tokens;
        budget.section_budgets[PromptSectionKind::EpisodicMemories as usize] =
            hints.memory_budget_tokens;
        budget.section_budgets[PromptSectionKind::UserProfile as usize] =
            hints.memory_budget_tokens / 2;
        budget
    }
}

/// Metadata about budget packing decisions.
#[derive(Debug, Clone, Default)]
pub struct BudgetMeta {
    /// Sections dropped due to overflow (lowest priority first).
    pub dropped: Vec<PromptSectionKind>,
    /// Oldest history messages removed to fit the total token ceiling.
    pub history_messages_dropped: usize,
    /// Approximate total tokens after packing (sections + history).
    pub packed_tokens: usize,
    /// IDs of recalled memories that actually survived into the packed prompt.
    ///
    /// A memory recalled by search may still be dropped by the budget manager
    /// (its whole section dropped, or trimmed within the section). Only the
    /// survivors — the ones actually composed into the packed prompt — should
    /// have their access counters bumped (#345), so recall does not reinforce
    /// memories it never surfaced.
    pub injected_memory_ids: Vec<i64>,
}

/// Result of packing a prompt under budget constraints.
#[derive(Debug, Clone)]
pub struct PackedPrompt {
    /// Final packet ready for LLM conversion.
    pub packet: PromptPacket,
    /// Budget metadata for tracing/tests.
    pub meta: BudgetMeta,
}

/// Inputs for building and packing a prompt packet.
#[derive(Debug, Clone)]
pub struct PackInput {
    /// Desktop/platform contract block.
    pub platform_contract: Option<String>,
    /// Compiled identity kernel.
    pub identity_kernel: IdentityKernel,
    /// Optional behavior contract (runtime rules).
    pub behavior_contract: Option<String>,
    /// Style examples selected for this turn.
    pub style_examples: Vec<StyleExample>,
    /// Recalled typed memories.
    pub recalled: Vec<RecalledMemory>,
    /// Active commitments.
    pub commitments: Vec<ActiveCommitmentPrompt>,
    /// Affect summary line.
    pub affect_summary: Option<String>,
    /// Active scene summary from rolling compression (#79).
    pub scene_summary: Option<String>,
    /// Recent conversation history.
    pub history: Vec<HistoryEntry>,
    /// Expression PHI / output contract block.
    pub output_contract: Option<String>,
    /// Note about a previously interrupted response (#206).
    pub interruption_note: Option<String>,
    /// Author's note: depth-based instruction injection (roleplay enhancement).
    pub authors_note: Option<AuthorsNote>,
    /// Optional structured user persona for `{{user_persona}}` macro expansion.
    pub user_persona: Option<UserPersona>,
    /// Current user input.
    pub user_input: String,
}

/// Validate that configured sub-budgets do not exceed the total ceiling.
pub fn validate_context_config(config: &ContextConfig) -> Result<(), CognitionError> {
    let dynamic_sum = config.scene_summary_tokens
        + config.memory_budget_tokens
        + config.semantic_budget_tokens
        + config.style_example_budget_tokens;
    if dynamic_sum > config.max_prompt_tokens {
        return Err(CognitionError::BudgetExceeded(format!(
            "mind.context sub-budgets sum to {dynamic_sum} tokens but max_prompt_tokens is {}",
            config.max_prompt_tokens
        )));
    }
    Ok(())
}

/// Drop priority for dynamic sections (lowest index = dropped first).
const DROP_ORDER: [PromptSectionKind; 7] = [
    PromptSectionKind::InterruptionNote,
    PromptSectionKind::StyleExamples,
    PromptSectionKind::EpisodicMemories,
    PromptSectionKind::SemanticContext,
    PromptSectionKind::UserProfile,
    PromptSectionKind::ActiveCommitments,
    PromptSectionKind::CharacterState,
];

fn sort_memories_for_drop(memories: &mut [RecalledMemory]) {
    memories.sort_by(|a, b| {
        a.item
            .confidence
            .get()
            .partial_cmp(&b.item.confidence.get())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.score_breakdown
                    .total
                    .partial_cmp(&b.score_breakdown.total)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
}

fn memory_section_body(memories: &[RecalledMemory]) -> String {
    memories
        .iter()
        .map(|m| format!("- {}", format_recalled_content(m)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render a memory section body, joining an optional non-memory `prefix`
/// (e.g. the user-persona block in the profile section) with the memory bullets.
fn render_memory_body(prefix: &str, memories: &[RecalledMemory]) -> String {
    let memory_body = memory_section_body(memories);
    if prefix.is_empty() {
        memory_body
    } else if memory_body.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}\n{memory_body}")
    }
}

/// Fit a memory section into `max_chars`, dropping the lowest-confidence
/// memories until the rendered body fits.
///
/// `memories` must be pre-sorted in drop order (ascending confidence) so
/// `remove(0)` drops the weakest first — the same direction the within-section
/// trim loop in [`pack_prompt`] uses. Unlike the old char-level truncation
/// (which sliced the highest-confidence tail mid-line), whole bullets are
/// dropped, so the surviving vector and the rendered body always agree (#345).
fn fit_memory_bullets(
    prefix: &str,
    memories: &mut Vec<RecalledMemory>,
    max_chars: usize,
) -> String {
    loop {
        let body = render_memory_body(prefix, memories);
        if body.chars().count() <= max_chars || memories.is_empty() {
            return body;
        }
        memories.remove(0);
    }
}

/// Recalled-memory survivors per section, kept in lockstep with the rendered
/// section bodies at every packing stage (#345).
///
/// `build_sections` prunes these to each section's token budget, the drop loop
/// clears them when a section is dropped, and the within-section trim loop
/// removes low-confidence memories from them while rebuilding the body — so at
/// any point their IDs are exactly the memories the packed prompt will show.
#[derive(Debug, Clone, Default)]
struct MemorySurvivors {
    semantic: Vec<RecalledMemory>,
    profile: Vec<RecalledMemory>,
    episodic: Vec<RecalledMemory>,
}

impl MemorySurvivors {
    /// Clear the survivors for `kind`, mirroring that section being dropped.
    fn clear_kind(&mut self, kind: PromptSectionKind) {
        match kind {
            PromptSectionKind::SemanticContext => self.semantic.clear(),
            PromptSectionKind::UserProfile => self.profile.clear(),
            PromptSectionKind::EpisodicMemories => self.episodic.clear(),
            _ => {}
        }
    }
}

fn set_section_body(sections: &mut [PromptSection], kind: PromptSectionKind, body: String) {
    if let Some(section) = sections.iter_mut().find(|s| s.kind == kind) {
        section.content = body;
    }
}

/// Token cost of a single history entry (content estimate + per-message overhead).
fn history_entry_tokens(entry: &HistoryEntry) -> usize {
    estimate_tokens(&entry.content).saturating_add(4)
}

fn estimate_history_tokens(history: &[HistoryEntry]) -> usize {
    history.iter().map(history_entry_tokens).sum()
}

/// Minimum history messages kept when trimming for token budget (one exchange).
const MIN_HISTORY_MESSAGES: usize = 2;

/// Token budget for the advisory interruption note (#M10). Non-zero so the
/// section participates in per-section truncation, and it is listed in
/// [`DROP_ORDER`] so it can be dropped entirely when over budget.
const INTERRUPTION_NOTE_BUDGET_TOKENS: usize = 200;

fn trim_history_to_budget(history: &mut Vec<HistoryEntry>, max_tokens: usize) -> usize {
    // Measure each entry once and build a suffix-sum table so the binary
    // search below accumulates cached counts instead of re-scanning the full
    // message text on every probe.
    let suffix = {
        let mut suffix = vec![0usize; history.len() + 1];
        for (i, entry) in history.iter().enumerate().rev() {
            suffix[i] = suffix[i + 1].saturating_add(history_entry_tokens(entry));
        }
        suffix
    };
    let remaining_tokens = |start: usize| suffix[start.min(history.len())];

    // Binary search for the minimum number of front elements to drain
    // so the remaining history fits within max_tokens.
    let mut lo = 0usize;
    let mut hi = history.len().saturating_sub(MIN_HISTORY_MESSAGES);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if remaining_tokens(mid) > max_tokens {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    // `lo` is the smallest drop count where the remainder fits; ensure we actually needed trimming.
    if lo == 0 && remaining_tokens(0) <= max_tokens {
        return 0;
    }
    // If even dropping to MIN_HISTORY_MESSAGES doesn't fit, drop everything above minimum.
    let drop_count = lo;
    history.drain(0..drop_count);
    drop_count
}

fn build_sections(
    input: &PackInput,
    budget: &ContextBudget,
) -> (Vec<PromptSection>, MemorySurvivors) {
    let (semantic, profile, episodic) = classify_recalled_memories(&input.recalled);
    let mut survivors = MemorySurvivors::default();

    let mut sections = Vec::new();

    if let Some(text) = &input.platform_contract {
        sections.push(PromptSection::new(
            PromptSectionKind::PlatformContract,
            text.clone(),
            0,
        ));
    }

    sections.push(PromptSection::new(
        PromptSectionKind::IdentityKernel,
        input.identity_kernel.text.clone(),
        0,
    ));

    if let Some(text) = &input.behavior_contract {
        sections.push(PromptSection::new(
            PromptSectionKind::BehaviorContract,
            text.clone(),
            0,
        ));
    }

    if let Some(text) = &input.affect_summary {
        sections.push(PromptSection::new(
            PromptSectionKind::CharacterState,
            text.clone(),
            budget.budget_for(PromptSectionKind::CharacterState),
        ));
    }

    if let Some(text) = &input.scene_summary {
        sections.push(PromptSection::new(
            PromptSectionKind::SceneState,
            text.clone(),
            budget.budget_for(PromptSectionKind::SceneState),
        ));
    }

    if !semantic.is_empty() {
        let mut semantic_owned: Vec<RecalledMemory> =
            semantic.iter().map(|m| (*m).clone()).collect();
        sort_memories_for_drop(&mut semantic_owned);
        let body = fit_memory_bullets(
            "",
            &mut semantic_owned,
            tokens_to_chars(budget.budget_for(PromptSectionKind::SemanticContext)),
        );
        survivors.semantic = semantic_owned;
        sections.push(PromptSection::new(
            PromptSectionKind::SemanticContext,
            body,
            budget.budget_for(PromptSectionKind::SemanticContext),
        ));
    }

    if !profile.is_empty() || input.user_persona.is_some() {
        let mut profile_owned: Vec<RecalledMemory> = profile.iter().map(|m| (*m).clone()).collect();
        sort_memories_for_drop(&mut profile_owned);

        // The structured persona block shares the section budget with the
        // recalled profile memories; reserve it before fitting the bullets.
        let persona_block = input
            .user_persona
            .as_ref()
            .map(|persona| persona.render_lines("- "));
        let body = fit_memory_bullets(
            persona_block.as_deref().unwrap_or(""),
            &mut profile_owned,
            tokens_to_chars(budget.budget_for(PromptSectionKind::UserProfile)),
        );
        survivors.profile = profile_owned;
        sections.push(PromptSection::new(
            PromptSectionKind::UserProfile,
            body,
            budget.budget_for(PromptSectionKind::UserProfile),
        ));
    }

    if !input.commitments.is_empty() {
        sections.push(PromptSection::new(
            PromptSectionKind::ActiveCommitments,
            render_commitments_block(&input.commitments),
            budget.budget_for(PromptSectionKind::ActiveCommitments),
        ));
    }

    if !episodic.is_empty() {
        let mut episodic_owned: Vec<RecalledMemory> =
            episodic.iter().map(|m| (*m).clone()).collect();
        sort_memories_for_drop(&mut episodic_owned);
        let body = fit_memory_bullets(
            "",
            &mut episodic_owned,
            tokens_to_chars(budget.budget_for(PromptSectionKind::EpisodicMemories)),
        );
        survivors.episodic = episodic_owned;
        sections.push(PromptSection::new(
            PromptSectionKind::EpisodicMemories,
            body,
            budget.budget_for(PromptSectionKind::EpisodicMemories),
        ));
    }

    if !input.style_examples.is_empty() {
        let body = input
            .style_examples
            .iter()
            .map(|e| e.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        sections.push(PromptSection::new(
            PromptSectionKind::StyleExamples,
            body,
            budget.budget_for(PromptSectionKind::StyleExamples),
        ));
    }

    if let Some(text) = &input.interruption_note {
        sections.push(PromptSection::new(
            PromptSectionKind::InterruptionNote,
            text.clone(),
            INTERRUPTION_NOTE_BUDGET_TOKENS,
        ));
    }

    if let Some(text) = &input.output_contract {
        sections.push(PromptSection::new(
            PromptSectionKind::OutputContract,
            text.clone(),
            0,
        ));
    }

    sections.push(PromptSection::new(
        PromptSectionKind::UserInput,
        input.user_input.clone(),
        0,
    ));

    (sections, survivors)
}

fn apply_section_budget(section: &mut PromptSection) {
    if section.budget_tokens == 0 || section.required {
        return;
    }
    let max_tokens = section.budget_tokens;
    let estimated = estimate_tokens(&section.content);
    if estimated > max_tokens {
        section.content = truncate_to_tokens(&section.content, max_tokens);
    }
}

/// Pack a prompt packet under the configured token budget.
#[must_use]
pub fn pack_prompt(input: PackInput, budget: &ContextBudget) -> PackedPrompt {
    let (mut sections, mut survivors) = build_sections(&input, budget);
    let mut dropped = Vec::new();

    for section in &mut sections {
        apply_section_budget(section);
    }

    // Cache each section's estimated token count so later drop passes adjust
    // the running total without re-scanning every section body.
    let mut section_tokens_cache: Vec<usize> = sections
        .iter()
        .map(|s| estimate_tokens(&s.content))
        .collect();
    let mut section_tokens_sum: usize = section_tokens_cache.iter().sum();

    let mut total = section_tokens_sum.saturating_add(estimate_history_tokens(&input.history));

    if total > budget.total_tokens {
        let mut drop_set = HashSet::new();
        for kind in DROP_ORDER {
            if total <= budget.total_tokens {
                break;
            }
            if let Some(idx) = sections.iter().position(|s| s.kind == kind) {
                let removed_tokens = section_tokens_cache[idx];
                sections[idx].content.clear();
                section_tokens_cache[idx] = 0;
                section_tokens_sum = section_tokens_sum.saturating_sub(removed_tokens);
                drop_set.insert(kind);
                // Keep the survivor vectors in lockstep with the rendered
                // sections: a cleared section renders nothing, so its survivors
                // must not be reported as injected. Clearing them also stops the
                // within-section trim loop below from re-populating a dropped
                // section — the previous divergence where displayed memories
                // were excluded by the `dropped` guard (#345).
                survivors.clear_kind(kind);
                total = total.saturating_sub(removed_tokens);
            }
        }
        dropped.extend(drop_set);
    }

    // Low-confidence memories drop first within each recalled section.
    if total > budget.total_tokens {
        for (kind, memories) in [
            (PromptSectionKind::EpisodicMemories, &mut survivors.episodic),
            (PromptSectionKind::SemanticContext, &mut survivors.semantic),
            (PromptSectionKind::UserProfile, &mut survivors.profile),
        ] {
            sort_memories_for_drop(memories);
            while total > budget.total_tokens && memories.len() > 1 {
                let removed = memories.remove(0);
                let removed_tokens =
                    estimate_tokens(&memory_section_body(std::slice::from_ref(&removed)));
                let body = memory_section_body(memories);
                set_section_body(&mut sections, kind, body);
                if let Some(idx) = sections.iter().position(|s| s.kind == kind) {
                    let new_tokens = estimate_tokens(&sections[idx].content);
                    section_tokens_sum =
                        section_tokens_sum.saturating_sub(section_tokens_cache[idx]);
                    section_tokens_cache[idx] = new_tokens;
                    section_tokens_sum = section_tokens_sum.saturating_add(new_tokens);
                }
                total = total.saturating_sub(removed_tokens);
            }
        }
    }

    let section_tokens = section_tokens_sum;
    let mut history = input.history;
    let mut history_messages_dropped = 0usize;
    if total > budget.total_tokens {
        let history_budget = budget.total_tokens.saturating_sub(section_tokens);
        history_messages_dropped = trim_history_to_budget(&mut history, history_budget);
        total = section_tokens.saturating_add(estimate_history_tokens(&history));
    }

    // Apply author's note after history trimming so the depth is relative to
    // the post-trim history length. This is done after trimming to ensure the
    // note is injected at the correct position in the final history.
    if let Some(ref note) = input.authors_note {
        apply_authors_note(&mut history, note);
    }

    let packed_tokens = total;

    let injected_memory_ids = collect_injected_memory_ids(&survivors);

    let packet = PromptPacket { sections, history };

    PackedPrompt {
        packet,
        meta: BudgetMeta {
            dropped,
            history_messages_dropped,
            packed_tokens,
            injected_memory_ids,
        },
    }
}

/// IDs of recalled memories that actually survived into the packed prompt (#345).
///
/// The survivor vectors are the single source of truth for what was rendered:
/// `build_sections` prunes them to each section's token budget, the drop loop
/// clears them when a section is dropped, and the within-section trim loop
/// removes low-confidence memories from them while rebuilding the body. Their
/// IDs therefore match exactly what the model will see — the gate the access
/// bump is keyed on, so recall never reinforces a memory it did not surface.
fn collect_injected_memory_ids(survivors: &MemorySurvivors) -> Vec<i64> {
    let mut ids = Vec::new();
    for memories in [&survivors.semantic, &survivors.profile, &survivors.episodic] {
        for memory in memories {
            if let Some(id) = memory.item.id {
                ids.push(id);
            }
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::character::StyleIntent;
    use ene_store::{
        AffectAnnotation, MemoryConfidence, MemoryItem, MemoryKind, MemorySalience, MemoryScope,
        MemoryScoreBreakdown, MemorySource, MemoryStatus,
    };

    fn sample_memory(kind: MemoryKind, confidence: f32, content: &str) -> RecalledMemory {
        RecalledMemory {
            item: MemoryItem {
                id: Some(1),
                scope: MemoryScope::User,
                character_id: "ene".into(),
                user_id: "u".into(),
                kind,
                title: "t".into(),
                content: content.into(),
                source: MemorySource::Conversation,
                source_ref: None,
                confidence: MemoryConfidence::new(confidence),
                salience: MemorySalience::default(),
                affect: AffectAnnotation::default(),
                relationship_impact: 0.0,
                access_count: 0,
                last_accessed_at: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                valid_from: None,
                valid_until: None,
                status: MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                faded_at: None,
                commitment_id: None,
            },
            reason: crate::recall::RecallReason::SimilarTopic,
            score_breakdown: MemoryScoreBreakdown {
                vector_similarity: confidence,
                lexical_score: 0.0,
                recency_score: 0.0,
                salience: 0.0,
                confidence,
                emotional_match: 0.0,
                relationship: 0.0,
                access_boost: 0.0,
                relevance: confidence,
                quality_factor: 1.0,
                contradiction_penalty: 0.0,
                stale_penalty: 0.0,
                commitment_boost: 0.0,
                reflection_multiplier: 1.0,
                total: confidence,
            },
            sources: vec![],
        }
    }

    /// Like [`sample_memory`] but with a distinct, settable ID and confidence,
    /// so tests can assert exactly which memories survive into the prompt (#345).
    fn sample_memory_with_id(id: i64, kind: MemoryKind, confidence: f32) -> RecalledMemory {
        let mut memory = sample_memory(kind, confidence, &format!("content-{id}"));
        memory.item.id = Some(id);
        memory
    }

    fn default_test_budget() -> ContextBudget {
        ContextBudget::from_config(&ContextConfig::default())
    }

    fn kernel_only_input(recalled: Vec<RecalledMemory>) -> PackInput {
        PackInput {
            platform_contract: None,
            identity_kernel: IdentityKernel {
                name: "Ene".into(),
                text: "KERNEL".into(),
                post_history_instructions: None,
            },
            behavior_contract: None,
            style_examples: vec![],
            recalled,
            commitments: vec![],
            affect_summary: None,
            scene_summary: None,
            history: vec![],
            output_contract: None,
            interruption_note: None,
            authors_note: None,
            user_persona: None,
            user_input: "hello".into(),
        }
    }

    #[test]
    fn injected_memory_ids_track_survivors_under_generous_budget() {
        // #345: with room for everything, every recalled memory is injected.
        let budget = default_test_budget();
        let input = kernel_only_input(vec![
            sample_memory_with_id(1, MemoryKind::Episodic, 0.9),
            sample_memory_with_id(2, MemoryKind::Preference, 0.9),
            sample_memory_with_id(3, MemoryKind::Affective, 0.9),
        ]);
        let packed = pack_prompt(input, &budget);
        let mut ids = packed.meta.injected_memory_ids.clone();
        ids.sort_unstable();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn injected_memory_ids_empty_when_section_dropped() {
        // #345: a recalled memory whose section is dropped by the budget must
        // not be reported as injected. A tiny total budget plus a large
        // episodic section guarantees the drop regardless of exact token
        // estimates.
        let config = ContextConfig {
            max_prompt_tokens: 30,
            ..ContextConfig::default()
        };
        let budget = ContextBudget::from_config(&config);
        let mut memory = sample_memory_with_id(10, MemoryKind::Episodic, 0.9);
        memory.item.content = "episodic filler content ".repeat(100);
        let input = kernel_only_input(vec![memory]);
        let packed = pack_prompt(input, &budget);
        assert!(
            packed
                .meta
                .dropped
                .contains(&PromptSectionKind::EpisodicMemories),
            "episodic section should be dropped under a tight budget, dropped={:?}",
            packed.meta.dropped
        );
        assert!(
            packed.meta.injected_memory_ids.is_empty(),
            "a dropped memory must not be injected"
        );
    }

    #[test]
    fn collect_injected_memory_ids_reflects_survivor_vectors() {
        // Direct, deterministic check of the survivor computation (#345): IDs
        // are collected per kind. A section that is dropped has its survivor
        // vector cleared by the drop loop (`MemorySurvivors::clear_kind`), so
        // an empty vector yields no IDs — mirroring how `pack_prompt` keeps
        // vector state in sync with rendered content.
        let survivors = MemorySurvivors {
            semantic: vec![sample_memory_with_id(1, MemoryKind::Semantic, 0.9)],
            profile: vec![sample_memory_with_id(2, MemoryKind::Preference, 0.9)],
            episodic: vec![sample_memory_with_id(3, MemoryKind::Episodic, 0.9)],
        };

        let mut all = collect_injected_memory_ids(&survivors);
        all.sort_unstable();
        assert_eq!(all, vec![1, 2, 3]);

        let mut without_episodic = survivors;
        without_episodic.clear_kind(PromptSectionKind::EpisodicMemories);
        let ids = collect_injected_memory_ids(&without_episodic);
        assert!(
            !ids.contains(&3),
            "a dropped section's memories must be excluded"
        );
        assert!(ids.contains(&1));
        assert!(ids.contains(&2));
    }

    #[test]
    fn injected_memory_ids_keep_only_memories_that_fit_section_budget() {
        // #345 regression (over-count): the per-section token budget used to
        // char-truncate the rendered body, silently cutting the highest-
        // confidence tail (after the ascending-confidence sort) out of the
        // visible content while the survivor vectors still reported those
        // memories injected and bumped them. The vectors and the rendered body
        // must agree: memories whose bullets do not fit the section budget are
        // dropped wholesale and never reported injected.
        let config = ContextConfig {
            memory_budget_tokens: 15,
            ..ContextConfig::default()
        };
        let budget = ContextBudget::from_config(&config);
        // All confidences >= 0.5 so no "[uncertain]" qualifier skews the
        // char accounting; sorted ascending they render as id 1, 2, 3.
        let mut low = sample_memory_with_id(1, MemoryKind::Episodic, 0.6);
        low.item.content = "m1 ".repeat(9);
        let mut mid = sample_memory_with_id(2, MemoryKind::Episodic, 0.7);
        mid.item.content = "m2 ".repeat(9);
        let mut high = sample_memory_with_id(3, MemoryKind::Episodic, 0.9);
        high.item.content = "m3 ".repeat(9);
        let input = kernel_only_input(vec![low, mid, high]);
        let packed = pack_prompt(input, &budget);

        // 15 tokens = 60 chars fits two 28-char bullets; the low-confidence
        // head is dropped first, keeping the high-confidence tail.
        let ids = packed.meta.injected_memory_ids.clone();
        assert!(
            ids.contains(&2) && ids.contains(&3),
            "the high-confidence tail must be kept, got {ids:?}"
        );
        assert!(
            !ids.contains(&1),
            "the low-confidence head must be trimmed, got {ids:?}"
        );

        let episodic = packed
            .packet
            .section(PromptSectionKind::EpisodicMemories)
            .expect("episodic section present");
        assert!(
            ids.iter()
                .all(|id| episodic.content.contains(&format!("m{id} "))),
            "every injected id must be rendered, body={:?}",
            episodic.content
        );
        assert!(
            !episodic.content.contains("m1 "),
            "a trimmed memory must not be rendered, body={:?}",
            episodic.content
        );
    }

    #[test]
    fn dropped_section_stays_empty_when_total_still_over_budget() {
        // #345 regression (under-count): when a memory section is dropped but
        // the total is still over budget, the within-section trim loop used to
        // re-populate the cleared section from the survivor vectors — memories
        // the model then saw, yet excluded from `injected_memory_ids` by the
        // `dropped` guard. Clearing the survivors at drop time keeps the
        // section dropped and its (now empty) vector consistent with the
        // rendered content: nothing is reported injected.
        let config = ContextConfig {
            max_prompt_tokens: 10,
            ..ContextConfig::default()
        };
        let budget = ContextBudget::from_config(&config);
        let input = PackInput {
            behavior_contract: Some("rules ".repeat(200)),
            recalled: vec![
                sample_memory_with_id(1, MemoryKind::Episodic, 0.9),
                sample_memory_with_id(2, MemoryKind::Episodic, 0.7),
                sample_memory_with_id(3, MemoryKind::Episodic, 0.5),
            ],
            ..kernel_only_input(vec![])
        };
        let packed = pack_prompt(input, &budget);
        assert!(
            packed
                .meta
                .dropped
                .contains(&PromptSectionKind::EpisodicMemories),
            "episodic section should be dropped, dropped={:?}",
            packed.meta.dropped
        );
        // The undroppable behavior contract keeps `total` over budget even
        // after the drop pass, forcing the trim loop to run.
        assert!(
            packed.meta.packed_tokens > budget.total_tokens,
            "test requires total to remain over budget, got {}",
            packed.meta.packed_tokens
        );
        assert!(
            packed.meta.injected_memory_ids.is_empty(),
            "a dropped section's memories must not be injected"
        );
        let episodic = packed
            .packet
            .section(PromptSectionKind::EpisodicMemories)
            .expect("episodic section present");
        assert!(
            episodic.content.is_empty(),
            "a dropped section must not be re-populated by the trim loop"
        );
    }

    #[test]
    fn identity_kernel_survives_tight_budget() {
        let config = ContextConfig {
            max_prompt_tokens: 50,
            recent_turns: 4,
            scene_summary_tokens: 800,
            memory_budget_tokens: 1_800,
            semantic_budget_tokens: 1_200,
            style_example_budget_tokens: 600,
            scene_turn_threshold: 12,
            chapter_span_threshold: 5,
            arc_span_threshold: 3,
            compression_timeout_secs: 60,
            compression_language: "en".into(),
        };
        let budget = ContextBudget::from_config(&config);
        let kernel = IdentityKernel {
            name: "Ene".into(),
            text: "KERNEL_ALWAYS_PRESENT".into(),
            post_history_instructions: None,
        };
        let input = PackInput {
            platform_contract: None,
            identity_kernel: kernel,
            behavior_contract: Some("rules".repeat(200)),
            style_examples: vec![StyleExample {
                text: "style".repeat(200),
                intent: StyleIntent::Greeting,
            }],
            recalled: vec![
                sample_memory(MemoryKind::Episodic, 0.2, "low"),
                sample_memory(MemoryKind::Episodic, 0.9, "high"),
            ],
            commitments: vec![],
            affect_summary: Some("mood=calm".into()),
            scene_summary: Some("scene".repeat(200)),
            history: vec![],
            output_contract: Some("PHI".into()),
            interruption_note: None,
            authors_note: None,
            user_persona: None,
            user_input: "hello".into(),
        };
        let packed = pack_prompt(input, &budget);
        let kernel_section = packed
            .packet
            .section(PromptSectionKind::IdentityKernel)
            .expect("test packet includes identity kernel");
        assert!(kernel_section.content.contains("KERNEL_ALWAYS_PRESENT"));
        assert!(
            packed
                .meta
                .dropped
                .contains(&PromptSectionKind::StyleExamples)
                || packed.meta.packed_tokens <= budget.total_tokens
        );
    }

    #[test]
    fn low_confidence_memories_sorted_for_drop() {
        let mut memories = vec![
            sample_memory(MemoryKind::Episodic, 0.9, "high"),
            sample_memory(MemoryKind::Episodic, 0.1, "low"),
        ];
        sort_memories_for_drop(&mut memories);
        assert!(memories[0].item.confidence.get() <= memories[1].item.confidence.get());
    }

    #[test]
    fn validate_context_config_rejects_overflow() {
        let config = ContextConfig {
            max_prompt_tokens: 100,
            recent_turns: 4,
            scene_summary_tokens: 800,
            memory_budget_tokens: 1_800,
            semantic_budget_tokens: 1_200,
            style_example_budget_tokens: 600,
            scene_turn_threshold: 12,
            chapter_span_threshold: 5,
            arc_span_threshold: 3,
            compression_timeout_secs: 60,
            compression_language: "en".into(),
        };
        assert!(validate_context_config(&config).is_err());
    }

    #[test]
    fn pack_prompt_trims_oldest_history_when_over_budget() {
        use crate::lifecycle::HistoryEntry;

        let config = ContextConfig {
            max_prompt_tokens: 40,
            recent_turns: 4,
            scene_summary_tokens: 800,
            memory_budget_tokens: 1_800,
            semantic_budget_tokens: 1_200,
            style_example_budget_tokens: 600,
            scene_turn_threshold: 12,
            chapter_span_threshold: 5,
            arc_span_threshold: 3,
            compression_timeout_secs: 60,
            compression_language: "en".into(),
        };
        let budget = ContextBudget::from_config(&config);
        let kernel = IdentityKernel {
            name: "Ene".into(),
            text: "KERNEL".into(),
            post_history_instructions: None,
        };
        let input = PackInput {
            platform_contract: None,
            identity_kernel: kernel,
            behavior_contract: None,
            style_examples: vec![],
            recalled: vec![],
            commitments: vec![],
            affect_summary: None,
            scene_summary: None,
            history: vec![
                HistoryEntry {
                    role: ene_ai::Role::User,
                    content: "old message with lots of text".repeat(4),
                },
                HistoryEntry {
                    role: ene_ai::Role::Assistant,
                    content: "older reply with lots of text".repeat(4),
                },
                HistoryEntry {
                    role: ene_ai::Role::User,
                    content: "recent".into(),
                },
                HistoryEntry {
                    role: ene_ai::Role::Assistant,
                    content: "latest".into(),
                },
            ],
            output_contract: None,
            interruption_note: None,
            authors_note: None,
            user_persona: None,
            user_input: "hello".into(),
        };
        let packed = pack_prompt(input, &budget);
        assert!(packed.meta.history_messages_dropped > 0);
        assert_eq!(packed.packet.history.len(), 2);
        assert!(packed.meta.packed_tokens <= budget.total_tokens);
    }

    #[test]
    fn budget_hints_override_memory_section_limits() {
        use crate::recall::RecallBudgetHints;

        let config = ContextConfig::default();
        let hints = RecallBudgetHints {
            memory_budget_tokens: 999,
            semantic_budget_tokens: 888,
            result_limit: 4,
        };
        let budget = ContextBudget::from_config_and_hints(&config, &hints);
        assert_eq!(budget.budget_for(PromptSectionKind::EpisodicMemories), 999);
        assert_eq!(budget.budget_for(PromptSectionKind::SemanticContext), 888);
    }

    #[test]
    fn interruption_note_is_dropped_when_over_budget() {
        // The advisory interruption note is not required and is listed first in
        // DROP_ORDER, so a tight budget drops it before the identity kernel (#M10).
        let config = ContextConfig {
            max_prompt_tokens: 30,
            recent_turns: 4,
            scene_summary_tokens: 800,
            memory_budget_tokens: 1_800,
            semantic_budget_tokens: 1_200,
            style_example_budget_tokens: 600,
            scene_turn_threshold: 12,
            chapter_span_threshold: 5,
            arc_span_threshold: 3,
            compression_timeout_secs: 60,
            compression_language: "en".into(),
        };
        let budget = ContextBudget::from_config(&config);
        let kernel = IdentityKernel {
            name: "Ene".into(),
            text: "KERNEL".into(),
            post_history_instructions: None,
        };
        let input = PackInput {
            platform_contract: None,
            identity_kernel: kernel,
            behavior_contract: Some("rules ".repeat(40)),
            style_examples: vec![],
            recalled: vec![],
            commitments: vec![],
            affect_summary: None,
            scene_summary: None,
            history: vec![],
            output_contract: None,
            interruption_note: Some("your previous reply was cut off mid-sentence".repeat(4)),
            authors_note: None,
            user_persona: None,
            user_input: "hello".into(),
        };
        let packed = pack_prompt(input, &budget);
        assert!(
            packed
                .meta
                .dropped
                .contains(&PromptSectionKind::InterruptionNote),
            "interruption note should be dropped under a tight budget, dropped={:?}",
            packed.meta.dropped
        );
        // The required identity kernel must survive.
        let kernel_section = packed
            .packet
            .section(PromptSectionKind::IdentityKernel)
            .expect("identity kernel present");
        assert!(kernel_section.content.contains("KERNEL"));
    }
}
