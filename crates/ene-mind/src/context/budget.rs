//! Priority-ordered prompt packing against the model's context window.
//!
//! Packing budgets against a single number — the tokens the prompt may occupy
//! once the response reserve and safety margin are set aside — and fills it
//! in priority order: required sections are always kept, and when the prompt
//! overflows the window the lowest-priority droppable sections are shed first.
//! Each section's *size* is bounded by its content producer (recall result
//! limit, lorebook token budget, identity-kernel cap), not by packing, so
//! packing only decides *which* sections survive.
//!
//! During the async compression gap [`pack_prompt`] synchronously detaches the
//! oldest span from the prompt-visible history instead of shedding sections —
//! see [`PackInput::compression_pending`].

#![expect(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "mind pipeline uses intentional turn/score/index arithmetic; history/token helpers index into bounds-checked conversational buffers"
)]
use ene_core::ActiveCommitmentPrompt;

use ene_config::UserPersona;

use crate::character::{
    AuthorsNote, IdentityKernel, StyleExample, apply_authors_note, render_authors_note,
};
use crate::lifecycle::HistoryEntry;
use crate::prompt_packet::{
    PromptPacket, PromptSection, PromptSectionKind, classify_recalled_memories,
    render_commitments_block,
};
use crate::recall::{RecalledMemory, format_recalled_content};

use super::tokens::estimate_tokens;

/// Prompt-packing capacity derived from the model's effective context window.
///
/// Packing fills this single window in priority order rather than allocating
/// fixed per-section sub-budgets.
#[derive(Debug, Clone, Copy)]
pub struct ContextBudget {
    /// Total prompt token ceiling: the model's available window.
    pub total_tokens: usize,
}

impl ContextBudget {
    /// Build a packing budget for a prompt that may occupy `total_tokens`.
    ///
    /// `total_tokens` is the effective window's `available` figure — the model
    /// window minus the response reserve and safety margin — optionally capped
    /// by `mind.context.max_prompt_tokens`.
    #[must_use]
    pub const fn with_capacity(total_tokens: usize) -> Self {
        Self { total_tokens }
    }
}

/// Metadata about budget packing decisions.
#[derive(Debug, Clone, Default)]
pub struct BudgetMeta {
    /// Sections dropped due to overflow (lowest priority first).
    pub dropped: Vec<PromptSectionKind>,
    /// Oldest history messages removed to fit the total token ceiling.
    pub history_messages_dropped: usize,
    /// Oldest history messages *detached* from the prompt while a compression
    /// summary was pending: the `[0, n)` leading range was dropped from
    /// the prompt-visible history synchronously, without waiting for the
    /// summary. Both this and [`Self::history_messages_dropped`] are
    /// prompt-copy-only operations — neither removes anything from the
    /// session/DB history; the detached messages stay in the log until a later
    /// compression covers them.
    pub history_messages_detached: usize,
    /// Approximate total tokens after packing (sections + history + author's note).
    pub packed_tokens: usize,
    /// IDs of recalled memories that actually survived into the packed prompt.
    ///
    /// A memory recalled by search may still be dropped by the budget manager
    /// (its whole section dropped, or trimmed within the section). Only the
    /// survivors — the ones actually composed into the packed prompt — should
    /// have their access counters bumped, so recall does not reinforce
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
    /// Localized instruction appended to the `CharacterState` section.
    pub character_state_note: Option<String>,
    /// Active scene summary from rolling compression.
    pub scene_summary: Option<String>,
    /// Recent conversation history.
    pub history: Vec<HistoryEntry>,
    /// Expression PHI / output contract block.
    pub output_contract: Option<String>,
    /// Note about a previously interrupted response.
    pub interruption_note: Option<String>,
    /// Author's note: depth-based instruction injection (roleplay enhancement).
    pub authors_note: Option<AuthorsNote>,
    /// Optional structured user persona for `{{user_persona}}` macro expansion.
    pub user_persona: Option<UserPersona>,
    /// Whether a rolling-compression task is in flight and its summary has not
    /// yet been applied to the session history.
    ///
    /// During this async compression gap the session history has not shrunk,
    /// so an overflowing prompt would otherwise shed sections (memories, style
    /// examples, …) while waiting for the summary. When this flag is set,
    /// packing detaches the oldest span from the prompt-visible history
    /// synchronously instead, keeping the section pack intact. In the compose
    /// path the pre-window span is already truncated from the prompt by the
    /// caller, so the messages detached here are the oldest of the recent
    /// window, which the pending summary does not cover. They stay in the
    /// session/DB history until a later compression covers them — comparable
    /// exposure to the old last-resort trim, up to one exchange coarser
    /// (detachment sheds two-message chunks, while the trim removes the exact
    /// minimum). Detachment is a prompt-construction-time operation only:
    /// nothing is removed from the session or DB history.
    pub compression_pending: bool,
    /// Current user input.
    pub user_input: String,
    /// Section-renderer language (`classifier_language` from the mind config):
    /// "ja" selects Japanese wording, anything else falls back to English —
    /// used for e.g. commitment deadline expressions. Belongs in a
    /// language pack.
    pub lang: String,
    /// Reference instant for relative expressions such as commitment
    /// deadlines. `None` uses a single `Utc::now()` captured per pack,
    /// shared by every bullet in a block; an explicit value pins the instant
    /// (tests).
    pub now: Option<chrono::DateTime<chrono::Utc>>,
}

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

/// Recalled-memory survivors per section, kept in lockstep with the rendered
/// section bodies at every packing stage.
///
/// `build_sections` seeds them with every recalled memory, and the drop loop
/// clears them when a whole section is dropped — so at any point their IDs are
/// exactly the memories the packed prompt will show.
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

/// Token cost of one history entry's rendered content (content estimate plus
/// the per-message overhead every entry carries).
fn history_entry_tokens_for(content: &str) -> usize {
    estimate_tokens(content).saturating_add(4)
}

/// Token cost of a single history entry (content estimate + per-message overhead).
fn history_entry_tokens(entry: &HistoryEntry) -> usize {
    history_entry_tokens_for(&entry.content)
}

/// Token cost of an author's note if it will be injected into history.
///
/// Counted from the same rendered text [`apply_authors_note`] inserts
/// ([`render_authors_note`]), so the reservation cannot drift from the entry
/// that later lands in history. Inactive / empty notes cost nothing.
fn authors_note_tokens(note: &AuthorsNote) -> usize {
    render_authors_note(note).map_or(0, |content| history_entry_tokens_for(&content))
}

/// Estimate the total token cost of a history slice (per-entry content
/// estimate plus per-message overhead). Shared by prompt packing and the
/// token-based window-pressure compression trigger.
pub fn estimate_history_tokens(history: &[HistoryEntry]) -> usize {
    history.iter().map(history_entry_tokens).sum()
}

/// Minimum history messages kept when trimming for token budget (one exchange).
const MIN_HISTORY_MESSAGES: usize = 2;

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

/// Synchronous fallback for the async compression gap — see
/// [`PackInput::compression_pending`] for the contract. Sheds up to two
/// leading messages per pass (one exchange, or a single message for
/// odd-length histories) from the *prompt-only* history until it fits or the
/// [`MIN_HISTORY_MESSAGES`] one-exchange floor is reached — below it packing
/// falls back to section dropping, so the most recent span always survives.
/// Returns the number of leading messages detached (the `[0, n)` range) for
/// [`BudgetMeta::history_messages_detached`].
fn detach_history_spans(history: &mut Vec<HistoryEntry>, max_tokens: usize) -> usize {
    let mut total = estimate_history_tokens(history);
    let mut detached = 0usize;
    while total > max_tokens && history.len() > MIN_HISTORY_MESSAGES {
        let drop = (history.len() - MIN_HISTORY_MESSAGES).min(2);
        for entry in &history[..drop] {
            total = total.saturating_sub(history_entry_tokens(entry));
        }
        history.drain(0..drop);
        detached += drop;
    }
    detached
}

fn build_sections(input: &PackInput) -> (Vec<PromptSection>, MemorySurvivors) {
    let (semantic, profile, episodic) = classify_recalled_memories(&input.recalled);
    let mut survivors = MemorySurvivors::default();

    let mut sections = Vec::new();

    if let Some(text) = &input.platform_contract {
        sections.push(PromptSection::new(
            PromptSectionKind::PlatformContract,
            text.clone(),
        ));
    }

    sections.push(PromptSection::new(
        PromptSectionKind::IdentityKernel,
        input.identity_kernel.text.clone(),
    ));

    if let Some(text) = &input.behavior_contract {
        sections.push(PromptSection::new(
            PromptSectionKind::BehaviorContract,
            text.clone(),
        ));
    }

    if let Some(text) = &input.affect_summary {
        let mut section = PromptSection::new(PromptSectionKind::CharacterState, text.clone());
        section.note.clone_from(&input.character_state_note);
        sections.push(section);
    }

    if let Some(text) = &input.scene_summary {
        sections.push(PromptSection::new(
            PromptSectionKind::SceneState,
            text.clone(),
        ));
    }

    if !semantic.is_empty() {
        let mut semantic_owned: Vec<RecalledMemory> =
            semantic.iter().map(|m| (*m).clone()).collect();
        sort_memories_for_drop(&mut semantic_owned);
        let item_count = semantic_owned.len();
        let body = memory_section_body(&semantic_owned);
        survivors.semantic = semantic_owned;
        sections.push(
            PromptSection::new(PromptSectionKind::SemanticContext, body)
                .with_item_count(item_count),
        );
    }

    if !profile.is_empty() || input.user_persona.is_some() {
        let mut profile_owned: Vec<RecalledMemory> = profile.iter().map(|m| (*m).clone()).collect();
        sort_memories_for_drop(&mut profile_owned);

        // The structured persona block shares the section with the recalled
        // profile memories; it is always kept, and whole bullets are shed (low
        // confidence first) only if the section overflows the window.
        // `item_count` tracks recalled memories only — not persona lines.
        let item_count = profile_owned.len();
        let persona_block = input
            .user_persona
            .as_ref()
            .map(|persona| persona.render_lines("- "))
            .unwrap_or_default();
        let body = render_memory_body(&persona_block, &profile_owned);
        survivors.profile = profile_owned;
        sections.push(
            PromptSection::new(PromptSectionKind::UserProfile, body).with_item_count(item_count),
        );
    }

    if !input.commitments.is_empty() {
        sections.push(PromptSection::new(
            PromptSectionKind::ActiveCommitments,
            render_commitments_block(
                &input.commitments,
                &input.lang,
                input.now.unwrap_or_else(chrono::Utc::now),
            ),
        ));
    }

    if !episodic.is_empty() {
        let mut episodic_owned: Vec<RecalledMemory> =
            episodic.iter().map(|m| (*m).clone()).collect();
        sort_memories_for_drop(&mut episodic_owned);
        let item_count = episodic_owned.len();
        let body = memory_section_body(&episodic_owned);
        survivors.episodic = episodic_owned;
        sections.push(
            PromptSection::new(PromptSectionKind::EpisodicMemories, body)
                .with_item_count(item_count),
        );
    }

    if !input.style_examples.is_empty() {
        let item_count = input.style_examples.len();
        let body = input
            .style_examples
            .iter()
            .map(|e| e.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        sections.push(
            PromptSection::new(PromptSectionKind::StyleExamples, body).with_item_count(item_count),
        );
    }

    if let Some(text) = &input.interruption_note {
        sections.push(PromptSection::new(
            PromptSectionKind::InterruptionNote,
            text.clone(),
        ));
    }

    if let Some(text) = &input.output_contract {
        sections.push(PromptSection::new(
            PromptSectionKind::OutputContract,
            text.clone(),
        ));
    }

    sections.push(PromptSection::new(
        PromptSectionKind::UserInput,
        input.user_input.clone(),
    ));

    (sections, survivors)
}

/// Pack a prompt packet under the model's context window.
///
/// Packing is a priority-ordered fill against `budget.total_tokens` (the
/// effective window's available tokens):
///
/// 1. an active author's note reserves its estimated tokens up front so packing
///    cannot spend the budget the note will occupy after insertion;
/// 2. required sections (platform contract, identity kernel, output contract,
///    user input) are always kept;
/// 3. if the assembled prompt overflows the remaining window, droppable
///    sections are shed whole, lowest-[`PromptSectionKind::priority`] first,
///    until it fits;
/// 4. as a last resort the oldest history messages are trimmed;
/// 5. the author's note is inserted into the post-trim history so its depth is
///    relative to the history the model will see.
///
/// When [`PackInput::compression_pending`] is set, step 3 is preceded by a
/// synchronous detachment of the oldest span from the prompt-visible history
/// (see the field's docs for the contract).
///
/// Section *sizes* are bounded by their content producers (recall result limit,
/// lorebook token budget, identity-kernel cap), not here, so packing only
/// decides *which* sections survive — it never trims within a section.
#[must_use]
pub fn pack_prompt(input: PackInput, budget: &ContextBudget) -> PackedPrompt {
    let (mut sections, mut survivors) = build_sections(&input);
    let mut dropped = Vec::new();

    // Cache each section's estimated token count so the drop pass adjusts the
    // running total without re-scanning every section body.
    let mut section_tokens_cache: Vec<usize> = sections
        .iter()
        .map(|s| estimate_tokens(&s.content))
        .collect();

    let mut history = input.history;
    let mut history_messages_detached = 0usize;
    let mut history_messages_dropped = 0usize;

    // Reserve the note before packing so section/history fill cannot spend the
    // tokens the note will add. Insertion still happens after trimming so depth
    // is relative to the post-trim history length.
    let note_tokens = input.authors_note.as_ref().map_or(0, authors_note_tokens);
    let packing_ceiling = budget.total_tokens.saturating_sub(note_tokens);

    let mut total = section_tokens_cache
        .iter()
        .sum::<usize>()
        .saturating_add(estimate_history_tokens(&history));

    if total > packing_ceiling && input.compression_pending {
        // Synchronous detachment fallback during the async compression gap.
        // Give history the full remaining budget (keeping every section)
        // and detach whole spans from the front until the prompt fits or the
        // one-exchange floor is reached. The detached span is prompt-only —
        // the session/DB history is untouched; the pre-window span is the one
        // being summarized and the detached recent-window messages stay in the
        // log until a later compression covers them.
        let all_section_tokens: usize = section_tokens_cache.iter().sum();
        let history_budget = packing_ceiling.saturating_sub(all_section_tokens);
        history_messages_detached = detach_history_spans(&mut history, history_budget);
        total = all_section_tokens.saturating_add(estimate_history_tokens(&history));
    }

    if total > packing_ceiling {
        // Shed whole droppable sections, lowest priority first. Required
        // sections are never candidates.
        let mut order: Vec<usize> = (0..sections.len())
            .filter(|&i| !sections[i].required)
            .collect();
        order.sort_by(|&a, &b| {
            sections[a]
                .kind
                .priority()
                .cmp(&sections[b].kind.priority())
        });
        for idx in order {
            if total <= packing_ceiling {
                break;
            }
            let kind = sections[idx].kind;
            let removed_tokens = section_tokens_cache[idx];
            // Keep item_count in lockstep with the cleared body so meta counts
            // reflect survivors, not the pre-drop source length.
            sections[idx].clear();
            section_tokens_cache[idx] = 0;
            dropped.push(kind);
            // Keep the survivor vectors in lockstep with the rendered sections:
            // a cleared section renders nothing, so its survivors must not be
            // reported as injected.
            survivors.clear_kind(kind);
            total = total.saturating_sub(removed_tokens);
        }
    }

    let section_tokens: usize = section_tokens_cache.iter().sum();
    if total > packing_ceiling {
        let history_budget = packing_ceiling.saturating_sub(section_tokens);
        history_messages_dropped = trim_history_to_budget(&mut history, history_budget);
        total = section_tokens.saturating_add(estimate_history_tokens(&history));
    }

    if let Some(ref note) = input.authors_note {
        apply_authors_note(&mut history, note);
    }

    let packed_tokens = total.saturating_add(note_tokens);

    let injected_memory_ids = collect_injected_memory_ids(&survivors);

    let packet = PromptPacket { sections, history };

    PackedPrompt {
        packet,
        meta: BudgetMeta {
            dropped,
            history_messages_dropped,
            history_messages_detached,
            packed_tokens,
            injected_memory_ids,
        },
    }
}

/// IDs of recalled memories that actually survived into the packed prompt.
///
/// The survivor vectors are the single source of truth for what was rendered:
/// `build_sections` seeds them with every recalled memory and the drop loop
/// clears them when a section is dropped. Their IDs therefore match exactly
/// what the model will see — the gate the access bump is keyed on, so recall
/// never reinforces a memory it did not surface.
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
    /// so tests can assert exactly which memories survive into the prompt.
    fn sample_memory_with_id(id: i64, kind: MemoryKind, confidence: f32) -> RecalledMemory {
        let mut memory = sample_memory(kind, confidence, &format!("content-{id}"));
        memory.item.id = Some(id);
        memory
    }

    fn default_test_budget() -> ContextBudget {
        ContextBudget::with_capacity(12_000)
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
            character_state_note: None,
            scene_summary: None,
            history: vec![],
            output_contract: None,
            interruption_note: None,
            authors_note: None,
            user_persona: None,
            compression_pending: false,
            user_input: "hello".into(),
            lang: "en".into(),
            now: None,
        }
    }

    #[test]
    fn injected_memory_ids_track_survivors_under_generous_budget() {
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
        // A recalled memory whose section is dropped by the budget must
        // not be reported as injected. A tiny total budget plus a large
        // episodic section guarantees the drop regardless of exact token
        // estimates.
        let budget = ContextBudget::with_capacity(30);
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
        // Direct, deterministic check of the survivor computation: IDs
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
    fn over_capacity_drops_lowest_priority_section_whole() {
        // An over-capacity prompt sheds whole sections lowest-priority first
        // rather than trimming within a section. The survivor vectors and the
        // rendered body must agree: a dropped section's memories are never
        // reported injected.
        //
        // Kernel ("KERNEL" = 2 tokens) + user input ("hello" = 2) = 4 tokens of
        // required overhead. A 10-token window cannot also fit the ~27-token
        // episodic section, so the whole section is dropped.
        let budget = ContextBudget::with_capacity(10);
        let mut low = sample_memory_with_id(1, MemoryKind::Episodic, 0.6);
        low.item.content = "m1 ".repeat(9);
        let mut mid = sample_memory_with_id(2, MemoryKind::Episodic, 0.7);
        mid.item.content = "m2 ".repeat(9);
        let mut high = sample_memory_with_id(3, MemoryKind::Episodic, 0.9);
        high.item.content = "m3 ".repeat(9);
        let input = kernel_only_input(vec![low, mid, high]);
        let packed = pack_prompt(input, &budget);

        assert!(
            packed
                .meta
                .dropped
                .contains(&PromptSectionKind::EpisodicMemories),
            "the over-capacity episodic section must be dropped whole, dropped={:?}",
            packed.meta.dropped
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
            "a dropped section must render nothing, body={:?}",
            episodic.content
        );
        assert_eq!(
            episodic.item_count, 0,
            "a dropped section must report zero items"
        );
        let (_messages, packet_meta) = packed.packet.to_llm_messages();
        assert_eq!(
            packet_meta.recalled_memory_count, 0,
            "meta counts must exclude dropped memories"
        );
    }

    #[test]
    fn item_count_tracks_source_len_not_rendered_bullets() {
        let budget = default_test_budget();
        let mut memory = sample_memory_with_id(1, MemoryKind::Episodic, 0.9);
        memory.item.content = "notes:\n- first\n- second".into();
        let input = PackInput {
            style_examples: vec![StyleExample {
                text: "example with\n\nan internal blank line".into(),
                intent: StyleIntent::Greeting,
            }],
            recalled: vec![memory],
            ..kernel_only_input(vec![])
        };
        let packed = pack_prompt(input, &budget);
        let episodic = packed
            .packet
            .section(PromptSectionKind::EpisodicMemories)
            .expect("episodic section present");
        assert_eq!(episodic.item_count, 1);
        let style = packed
            .packet
            .section(PromptSectionKind::StyleExamples)
            .expect("style section present");
        assert_eq!(style.item_count, 1);
        let (_messages, meta) = packed.packet.to_llm_messages();
        assert_eq!(meta.recalled_memory_count, 1);
        assert_eq!(meta.style_example_count, 1);
    }

    #[test]
    fn dropped_style_examples_zero_item_count() {
        let budget = ContextBudget::with_capacity(10);
        let input = PackInput {
            style_examples: vec![StyleExample {
                text: "style filler ".repeat(80),
                intent: StyleIntent::Greeting,
            }],
            ..kernel_only_input(vec![])
        };
        let packed = pack_prompt(input, &budget);
        assert!(
            packed
                .meta
                .dropped
                .contains(&PromptSectionKind::StyleExamples),
            "style examples should be dropped under a tight budget, dropped={:?}",
            packed.meta.dropped
        );
        let style = packed
            .packet
            .section(PromptSectionKind::StyleExamples)
            .expect("style section present");
        assert_eq!(style.item_count, 0);
        let (_messages, meta) = packed.packet.to_llm_messages();
        assert_eq!(meta.style_example_count, 0);
    }

    #[test]
    fn dropped_section_stays_empty_when_total_still_over_budget() {
        // When a memory section is dropped but the total is still over budget
        // (an oversized *required* section keeps it over), the dropped section
        // must stay empty and its memories must not be reported injected.
        // Clearing the survivors at drop time keeps the section's (now empty)
        // survivor vector consistent with the rendered content.
        let budget = ContextBudget::with_capacity(10);
        let input = PackInput {
            // The platform contract is required, so packing never drops it;
            // its size alone keeps the total over the tiny budget even after
            // every droppable section is shed.
            platform_contract: Some("platform ".repeat(100)),
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
            "a dropped section must render nothing"
        );
        assert_eq!(episodic.item_count, 0);
    }

    #[test]
    fn identity_kernel_survives_tight_budget() {
        let budget = ContextBudget::with_capacity(50);
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
            character_state_note: None,
            scene_summary: Some("scene".repeat(200)),
            history: vec![],
            output_contract: Some("PHI".into()),
            interruption_note: None,
            authors_note: None,
            user_persona: None,
            compression_pending: false,
            user_input: "hello".into(),
            lang: "en".into(),
            now: None,
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
    fn pack_prompt_trims_oldest_history_when_over_budget() {
        use crate::lifecycle::HistoryEntry;

        let budget = ContextBudget::with_capacity(40);
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
            character_state_note: None,
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
            compression_pending: false,
            user_input: "hello".into(),
            lang: "en".into(),
            now: None,
        };
        let packed = pack_prompt(input, &budget);
        assert!(packed.meta.history_messages_dropped > 0);
        assert_eq!(packed.packet.history.len(), 2);
        assert!(packed.meta.packed_tokens <= budget.total_tokens);
    }

    #[test]
    fn pack_prompt_reserves_authors_note_tokens_in_budget() {
        use crate::lifecycle::HistoryEntry;

        let note = AuthorsNote::new("Stay in character and keep the scene tense.".repeat(3), 1);
        let note_tokens = authors_note_tokens(&note);
        assert!(note_tokens > 0, "fixture note must occupy budget");

        // Tight enough that reserving the note forces oldest-history trim, yet
        // still leaves room for the required kernel + one exchange + note.
        let budget = ContextBudget::with_capacity(80);
        let input = PackInput {
            authors_note: Some(note),
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
            ..kernel_only_input(vec![])
        };
        let packed = pack_prompt(input, &budget);

        assert!(
            packed.meta.history_messages_dropped > 0,
            "note reservation must force history trim under this budget"
        );
        assert!(
            packed
                .packet
                .history
                .iter()
                .any(|entry| entry.content.contains("[Author's Note]")),
            "active author's note must still be inserted after trimming"
        );
        let expected = packed
            .packet
            .sections
            .iter()
            .map(|section| estimate_tokens(&section.content))
            .sum::<usize>()
            .saturating_add(estimate_history_tokens(&packed.packet.history));
        assert_eq!(
            packed.meta.packed_tokens, expected,
            "packed_tokens must include the injected note"
        );
        assert!(
            packed.meta.packed_tokens <= budget.total_tokens,
            "note-inclusive packed_tokens must stay within budget, got {}",
            packed.meta.packed_tokens
        );
        assert!(
            packed.meta.packed_tokens >= note_tokens,
            "packed_tokens must account for the reserved note tokens"
        );
    }

    #[test]
    fn interruption_note_is_dropped_when_over_budget() {
        // The advisory interruption note is not required and has the lowest
        // droppable priority, so a tight budget drops it before the identity
        // kernel.
        let budget = ContextBudget::with_capacity(30);
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
            character_state_note: None,
            scene_summary: None,
            history: vec![],
            output_contract: None,
            interruption_note: Some("your previous reply was cut off mid-sentence".repeat(4)),
            authors_note: None,
            user_persona: None,
            compression_pending: false,
            user_input: "hello".into(),
            lang: "en".into(),
            now: None,
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
        let kernel_section = packed
            .packet
            .section(PromptSectionKind::IdentityKernel)
            .expect("identity kernel present");
        assert!(kernel_section.content.contains("KERNEL"));
    }

    /// A commitment prompt row for the priority-ordering tests.
    fn sample_commitment() -> ActiveCommitmentPrompt {
        ActiveCommitmentPrompt {
            id: 1,
            title: "reminder".into(),
            description: "follow up on the report".into(),
            due_label: None,
            due_at: None,
        }
    }

    #[test]
    fn packing_drops_lowest_priority_sections_first() {
        // Over capacity, packing sheds droppable sections in ascending
        // priority order. StyleExamples (20) outranks InterruptionNote (10) for
        // dropping, so the note goes first; CharacterState (70) outranks both,
        // so it survives a window that still fits it once the note is gone.
        let budget = ContextBudget::with_capacity(20);
        let input = PackInput {
            affect_summary: Some("mood=calm".into()),
            style_examples: vec![StyleExample {
                text: "style ".repeat(40),
                intent: StyleIntent::Greeting,
            }],
            interruption_note: Some("interrupted ".repeat(40)),
            ..kernel_only_input(vec![])
        };
        let packed = pack_prompt(input, &budget);

        assert!(
            packed
                .meta
                .dropped
                .contains(&PromptSectionKind::InterruptionNote),
            "the lowest-priority droppable section must drop first, dropped={:?}",
            packed.meta.dropped
        );
        let note_pos = packed
            .meta
            .dropped
            .iter()
            .position(|k| *k == PromptSectionKind::InterruptionNote);
        let style_pos = packed
            .meta
            .dropped
            .iter()
            .position(|k| *k == PromptSectionKind::StyleExamples);
        if let (Some(note), Some(style)) = (note_pos, style_pos) {
            assert!(
                note < style,
                "interruption note (priority 10) must drop before style examples (20)"
            );
        }
    }

    #[test]
    fn required_sections_survive_a_window_smaller_than_the_prompt() {
        // Required sections (platform contract, identity kernel, output
        // contract, user input) are never dropped, even when the window is far
        // smaller than the assembled prompt.
        let budget = ContextBudget::with_capacity(5);
        let input = PackInput {
            platform_contract: Some("platform ".repeat(100)),
            behavior_contract: Some("rules ".repeat(100)),
            affect_summary: Some("mood ".repeat(100)),
            scene_summary: Some("scene ".repeat(100)),
            style_examples: vec![StyleExample {
                text: "style ".repeat(100),
                intent: StyleIntent::Greeting,
            }],
            output_contract: Some("phi ".repeat(100)),
            interruption_note: Some("note ".repeat(100)),
            ..kernel_only_input(vec![])
        };
        let packed = pack_prompt(input, &budget);

        for kind in [
            PromptSectionKind::PlatformContract,
            PromptSectionKind::IdentityKernel,
            PromptSectionKind::OutputContract,
            PromptSectionKind::UserInput,
        ] {
            assert!(
                !packed.meta.dropped.contains(&kind),
                "required section {kind:?} must never be dropped, dropped={:?}",
                packed.meta.dropped
            );
            let section = packed
                .packet
                .section(kind)
                .unwrap_or_else(|| panic!("{kind:?} section present"));
            assert!(
                !section.content.is_empty(),
                "required section {kind:?} must retain its content"
            );
        }
        for kind in [
            PromptSectionKind::BehaviorContract,
            PromptSectionKind::CharacterState,
            PromptSectionKind::SceneState,
            PromptSectionKind::StyleExamples,
            PromptSectionKind::InterruptionNote,
        ] {
            assert!(
                packed.meta.dropped.contains(&kind),
                "droppable section {kind:?} should be dropped under a tiny window, dropped={:?}",
                packed.meta.dropped
            );
        }
    }

    #[test]
    fn higher_priority_section_survives_over_lower_priority_one() {
        // Given two droppable sections that cannot both fit, the
        // higher-priority one is kept and the lower-priority one is dropped.
        // ActiveCommitments (60) outranks StyleExamples (20).
        let budget = ContextBudget::with_capacity(30);
        let input = PackInput {
            commitments: vec![sample_commitment()],
            style_examples: vec![StyleExample {
                text: "style ".repeat(60),
                intent: StyleIntent::Greeting,
            }],
            ..kernel_only_input(vec![])
        };
        let packed = pack_prompt(input, &budget);

        assert!(
            packed
                .meta
                .dropped
                .contains(&PromptSectionKind::StyleExamples),
            "the lower-priority style section should drop, dropped={:?}",
            packed.meta.dropped
        );
        let commitments = packed
            .packet
            .section(PromptSectionKind::ActiveCommitments)
            .expect("commitments section present");
        assert!(
            !commitments.content.is_empty(),
            "the higher-priority commitments section must survive"
        );
    }

    /// An alternating user/assistant history of `turns` exchanges for the
    /// detachment tests.
    fn exchanges(turns: usize) -> Vec<crate::lifecycle::HistoryEntry> {
        let mut history = Vec::with_capacity(turns * 2);
        for i in 0..turns {
            history.push(crate::lifecycle::HistoryEntry {
                role: ene_ai::Role::User,
                content: format!("user message {i} ").repeat(4),
            });
            history.push(crate::lifecycle::HistoryEntry {
                role: ene_ai::Role::Assistant,
                content: format!("assistant reply {i} ").repeat(4),
            });
        }
        history
    }

    #[test]
    fn compression_pending_detaches_oldest_span_and_keeps_sections() {
        // While a compression summary is pending the session history has
        // not shrunk, so an overflowing prompt must detach the oldest span
        // from the prompt-visible history synchronously instead of shedding
        // sections. A memory section that the old ordering would have dropped
        // must survive, and the window constraint must still hold.
        let budget = ContextBudget::with_capacity(60);
        let history = exchanges(5); // 10 long messages: far over budget
        let input = PackInput {
            recalled: vec![sample_memory(MemoryKind::Episodic, 0.9, "important memory")],
            history: history.clone(),
            compression_pending: true,
            ..kernel_only_input(vec![])
        };
        let packed = pack_prompt(input, &budget);

        assert!(
            packed.meta.history_messages_detached > 0,
            "oldest span must be detached while the summary is pending"
        );
        assert!(
            packed.meta.history_messages_dropped == 0,
            "detachment must not fall back to the dropping path"
        );
        assert_eq!(
            packed.meta.history_messages_detached + packed.packet.history.len(),
            history.len(),
            "detachment only hides the leading span from the prompt"
        );
        assert!(
            !packed
                .meta
                .dropped
                .contains(&PromptSectionKind::EpisodicMemories),
            "the memory section must survive the async gap, dropped={:?}, detached={}, packed_tokens={}, history_len={}",
            packed.meta.dropped,
            packed.meta.history_messages_detached,
            packed.meta.packed_tokens,
            packed.packet.history.len()
        );
        assert!(
            packed.meta.packed_tokens <= budget.total_tokens,
            "the window constraint must hold while waiting for the summary"
        );
    }

    #[test]
    fn detach_respects_floor_and_falls_back_to_section_drops() {
        // Detachment never removes the most recent span. When the history
        // is already at the one-exchange floor, packing falls back to the
        // existing section-dropping behavior instead of detaching further.
        let budget = ContextBudget::with_capacity(15);
        let history = exchanges(1); // exactly the floor: 2 messages
        let input = PackInput {
            recalled: vec![sample_memory(
                MemoryKind::Episodic,
                0.9,
                &"big memory ".repeat(20),
            )],
            history,
            compression_pending: true,
            ..kernel_only_input(vec![])
        };
        let packed = pack_prompt(input, &budget);

        assert_eq!(
            packed.meta.history_messages_detached, 0,
            "below the floor nothing may be detached"
        );
        assert_eq!(
            packed.packet.history.len(),
            2,
            "the most recent span always survives"
        );
        assert!(
            packed
                .meta
                .dropped
                .contains(&PromptSectionKind::EpisodicMemories),
            "at the floor the existing section-dropping behavior takes over, dropped={:?}",
            packed.meta.dropped
        );
    }

    #[test]
    fn detach_is_prompt_only_and_leaves_source_history_intact() {
        // Detachment is a prompt-construction-time operation. The caller's
        // history (the session/DB history) is untouched; only the prompt copy
        // loses the leading span.
        let budget = ContextBudget::with_capacity(60);
        let source = exchanges(5);
        let packed = pack_prompt(
            PackInput {
                history: source.clone(),
                compression_pending: true,
                ..kernel_only_input(vec![])
            },
            &budget,
        );

        assert!(packed.meta.history_messages_detached > 0);
        assert_eq!(source.len(), 10, "source history must be unchanged");
        for (got, want) in packed
            .packet
            .history
            .iter()
            .zip(source.iter().skip(packed.meta.history_messages_detached))
        {
            assert_eq!(got.content, want.content);
        }
    }

    #[test]
    fn commitments_section_renders_deadline_through_packing() {
        // The deadline reaches the packed prompt — `due_at` becomes a
        // relative expression in the configured language, overdue is marked.
        // The reference instant is pinned so the calendar-day offsets can
        // never roll across a UTC-midnight boundary between this test and the
        // renderer.
        let now = chrono::DateTime::parse_from_rfc3339("2026-08-01T12:00:00Z")
            .expect("fixed instant")
            .with_timezone(&chrono::Utc);
        let input = PackInput {
            commitments: vec![
                ActiveCommitmentPrompt {
                    id: 1,
                    title: "report".into(),
                    description: "send to manager".into(),
                    due_label: None,
                    due_at: Some(now + chrono::Duration::days(2)),
                },
                ActiveCommitmentPrompt {
                    id: 2,
                    title: "call back".into(),
                    description: String::new(),
                    due_label: None,
                    due_at: Some(now - chrono::Duration::days(1)),
                },
            ],
            lang: "ja".into(),
            now: Some(now),
            ..kernel_only_input(vec![])
        };
        let packed = pack_prompt(input, &default_test_budget());
        let commitments = packed
            .packet
            .section(PromptSectionKind::ActiveCommitments)
            .expect("commitments section present");
        assert!(
            commitments.content.contains("（期限: あと2日）"),
            "future deadline missing from packed prompt: {:?}",
            commitments.content
        );
        assert!(
            commitments.content.contains("（期限切れ）"),
            "overdue marker missing from packed prompt: {:?}",
            commitments.content
        );
    }
}
