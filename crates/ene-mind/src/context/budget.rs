//! Context budget allocation and overflow drop policy (#81).

use std::collections::HashSet;

use ene_store::ActiveCommitmentPrompt;

use crate::character::{IdentityKernel, StyleExample};
use crate::config::ContextConfig;
use crate::error::CognitionError;
use crate::lifecycle::HistoryEntry;
use crate::prompt_packet::{
    PromptPacket, PromptSection, PromptSectionKind, classify_recalled_memories,
    render_commitments_block,
};
use crate::recall::{RecallBudgetHints, RecalledMemory, format_recalled_content};

use super::tokens::{estimate_tokens, truncate_to_tokens};

/// Fixed and dynamic token budgets for prompt packing.
#[derive(Debug, Clone)]
pub struct ContextBudget {
    /// Total prompt token ceiling.
    pub total_tokens: usize,
    /// Per-section dynamic budgets.
    pub section_budgets: [usize; 12],
}

impl ContextBudget {
    /// Build a budget from [`ContextConfig`].
    pub const fn from_config(config: &ContextConfig) -> Self {
        let mut section_budgets = [0usize; 12];
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
const DROP_ORDER: [PromptSectionKind; 6] = [
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

fn set_section_body(sections: &mut [PromptSection], kind: PromptSectionKind, body: String) {
    if let Some(section) = sections.iter_mut().find(|s| s.kind == kind) {
        section.content = body;
    }
}

fn estimate_history_tokens(history: &[HistoryEntry]) -> usize {
    history
        .iter()
        .map(|entry| estimate_tokens(&entry.content).saturating_add(4))
        .sum()
}

/// Minimum history messages kept when trimming for token budget (one exchange).
const MIN_HISTORY_MESSAGES: usize = 2;

fn trim_history_to_budget(history: &mut Vec<HistoryEntry>, max_tokens: usize) -> usize {
    let mut dropped = 0usize;
    while history.len() > MIN_HISTORY_MESSAGES && estimate_history_tokens(history) > max_tokens {
        history.remove(0);
        dropped += 1;
    }
    dropped
}

fn build_sections(input: &PackInput, budget: &ContextBudget) -> Vec<PromptSection> {
    let (semantic, profile, episodic) = classify_recalled_memories(&input.recalled);

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
        sections.push(PromptSection::new(
            PromptSectionKind::SemanticContext,
            memory_section_body(&semantic_owned),
            budget.budget_for(PromptSectionKind::SemanticContext),
        ));
    }

    if !profile.is_empty() {
        let mut profile_owned: Vec<RecalledMemory> = profile.iter().map(|m| (*m).clone()).collect();
        sort_memories_for_drop(&mut profile_owned);
        sections.push(PromptSection::new(
            PromptSectionKind::UserProfile,
            memory_section_body(&profile_owned),
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
        sections.push(PromptSection::new(
            PromptSectionKind::EpisodicMemories,
            memory_section_body(&episodic_owned),
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

    sections
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

fn section_token_total(sections: &[PromptSection]) -> usize {
    sections.iter().map(|s| estimate_tokens(&s.content)).sum()
}

/// Pack a prompt packet under the configured token budget.
#[must_use]
pub fn pack_prompt(input: PackInput, budget: &ContextBudget) -> PackedPrompt {
    let mut sections = build_sections(&input, budget);
    let mut dropped = Vec::new();
    let (mut semantic, mut profile, mut episodic) =
        classify_recalled_memories_owned(&input.recalled);

    for section in &mut sections {
        apply_section_budget(section);
    }

    let mut total =
        section_token_total(&sections).saturating_add(estimate_history_tokens(&input.history));

    if total > budget.total_tokens {
        let mut drop_set = HashSet::new();
        for kind in DROP_ORDER {
            if total <= budget.total_tokens {
                break;
            }
            if let Some(idx) = sections.iter().position(|s| s.kind == kind) {
                let removed_tokens = estimate_tokens(&sections[idx].content);
                sections[idx].content.clear();
                drop_set.insert(kind);
                total = total.saturating_sub(removed_tokens);
            }
        }
        dropped.extend(drop_set);
    }

    // Low-confidence memories drop first within each recalled section.
    if total > budget.total_tokens {
        for (kind, memories) in [
            (PromptSectionKind::EpisodicMemories, &mut episodic),
            (PromptSectionKind::SemanticContext, &mut semantic),
            (PromptSectionKind::UserProfile, &mut profile),
        ] {
            sort_memories_for_drop(memories);
            while total > budget.total_tokens && memories.len() > 1 {
                let removed = memories.remove(0);
                let removed_tokens =
                    estimate_tokens(&memory_section_body(std::slice::from_ref(&removed)));
                set_section_body(&mut sections, kind, memory_section_body(memories));
                total = total.saturating_sub(removed_tokens);
            }
        }
    }

    let section_tokens = section_token_total(&sections);
    let mut history = input.history;
    let mut history_messages_dropped = 0usize;
    if total > budget.total_tokens {
        let history_budget = budget.total_tokens.saturating_sub(section_tokens);
        history_messages_dropped = trim_history_to_budget(&mut history, history_budget);
        total = section_tokens.saturating_add(estimate_history_tokens(&history));
    }

    let packed_tokens = total;

    let packet = PromptPacket { sections, history };

    PackedPrompt {
        packet,
        meta: BudgetMeta {
            dropped,
            history_messages_dropped,
            packed_tokens,
        },
    }
}

fn classify_recalled_memories_owned(
    recalled: &[RecalledMemory],
) -> (
    Vec<RecalledMemory>,
    Vec<RecalledMemory>,
    Vec<RecalledMemory>,
) {
    let (semantic, profile, episodic) = classify_recalled_memories(recalled);
    (
        semantic.iter().map(|m| (*m).clone()).collect(),
        profile.iter().map(|m| (*m).clone()).collect(),
        episodic.iter().map(|m| (*m).clone()).collect(),
    )
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
                contradiction_penalty: 0.0,
                stale_penalty: 0.0,
                commitment_boost: 0.0,
                total: confidence,
            },
            sources: vec![],
        }
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
}
