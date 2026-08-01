//! Sectioned prompt packet composition (#87).

#![expect(
    clippy::arithmetic_side_effects,
    reason = "prompt metadata sums per-bucket recalled-memory counts into PromptPacketMeta"
)]
mod section;

pub use section::{PromptSection, PromptSectionKind};

use chrono::{DateTime, Utc};
use ene_ai::{LlmMessage, UserMessagePart};
use ene_core::{ActiveCommitmentPrompt, MemoryKind, MemorySource};

use crate::lifecycle::{HistoryEntry, PromptPacketMeta};
use crate::recall::{RecallReason, RecalledMemory};

/// A sectioned prompt structure with independent logical layers.
#[derive(Debug, Clone, Default)]
pub struct PromptPacket {
    /// Ordered prompt sections (system + user input metadata).
    pub sections: Vec<PromptSection>,
    /// Recent conversation history rendered as separate LLM messages.
    pub history: Vec<HistoryEntry>,
}

impl PromptPacket {
    /// Look up a section by kind (first match).
    pub fn section(&self, kind: PromptSectionKind) -> Option<&PromptSection> {
        self.sections.iter().find(|s| s.kind == kind)
    }

    /// Whether a section has non-empty content.
    pub fn section_included(&self, kind: PromptSectionKind) -> bool {
        self.section(kind)
            .is_some_and(|s| !s.content.trim().is_empty())
    }

    /// Convert the packet into LLM messages (system + history + PHI + user).
    pub fn to_llm_messages(&self) -> (Vec<LlmMessage>, PromptPacketMeta) {
        let mut system_parts = Vec::new();

        for kind in PromptSectionKind::render_order() {
            if matches!(
                *kind,
                PromptSectionKind::OutputContract | PromptSectionKind::UserInput
            ) {
                continue;
            }
            if let Some(section) = self.section(*kind)
                && let Some(block) = section.render_system_block()
            {
                system_parts.push(block);
            }
        }

        let system_text = system_parts.join("\n\n");
        let mut messages = vec![LlmMessage::System {
            content: system_text,
        }];

        for entry in &self.history {
            let msg = match entry.role {
                ene_ai::Role::Assistant => LlmMessage::Assistant {
                    content: Some(entry.content.clone()),
                    tool_calls: None,
                },
                ene_ai::Role::System => LlmMessage::System {
                    content: entry.content.clone(),
                },
                ene_ai::Role::User => LlmMessage::User {
                    parts: vec![UserMessagePart::Text {
                        text: entry.content.clone(),
                    }],
                },
            };
            messages.push(msg);
        }

        let post_history_included = self.section_included(PromptSectionKind::OutputContract);
        if post_history_included {
            let content = self
                .section(PromptSectionKind::OutputContract)
                .map(|s| s.content.clone())
                .unwrap_or_default();
            messages.push(LlmMessage::System { content });
        }

        let user_input = self
            .section(PromptSectionKind::UserInput)
            .map(|s| s.content.clone())
            .unwrap_or_default();
        messages.push(LlmMessage::User {
            parts: vec![UserMessagePart::Text { text: user_input }],
        });

        let semantic_count = self
            .section(PromptSectionKind::SemanticContext)
            .map_or(0, |s| {
                s.content.lines().filter(|l| l.starts_with("- ")).count()
            });
        let profile_count = self.section(PromptSectionKind::UserProfile).map_or(0, |s| {
            s.content.lines().filter(|l| l.starts_with("- ")).count()
        });
        let episodic_count = self
            .section(PromptSectionKind::EpisodicMemories)
            .map_or(0, |s| {
                s.content.lines().filter(|l| l.starts_with("- ")).count()
            });

        let meta = PromptPacketMeta {
            identity_kernel_included: self.section_included(PromptSectionKind::IdentityKernel),
            style_example_count: if self.section_included(PromptSectionKind::StyleExamples) {
                self.section(PromptSectionKind::StyleExamples)
                    .map_or(0, |s| s.content.split("\n\n").count())
            } else {
                0
            },
            recalled_memory_count: semantic_count + profile_count + episodic_count,
            post_history_included,
            scene_summary_included: self.section_included(PromptSectionKind::SceneState),
            dropped_sections: Vec::new(),
            history_messages_detached: 0,
            packed_tokens: 0,
            // Populated by the budget packer in the engine; this conversion
            // only renders an already-packed packet and cannot recover IDs.
            injected_memory_ids: Vec::new(),
        };

        (messages, meta)
    }
}

/// Split recalled memories into semantic, profile, and episodic buckets.
pub fn classify_recalled_memories(
    recalled: &[RecalledMemory],
) -> (
    Vec<&RecalledMemory>,
    Vec<&RecalledMemory>,
    Vec<&RecalledMemory>,
) {
    let mut semantic = Vec::new();
    let mut profile = Vec::new();
    let mut episodic = Vec::new();

    for memory in recalled {
        match memory.item.kind {
            MemoryKind::UserProfile | MemoryKind::Preference | MemoryKind::Relationship => {
                profile.push(memory);
            }
            MemoryKind::Semantic | MemoryKind::Procedure
                if memory.reason == RecallReason::CharacterLore
                    || memory.item.source == MemorySource::Ccv3 =>
            {
                semantic.push(memory);
            }
            MemoryKind::Commitment => {}
            _ => episodic.push(memory),
        }
    }

    (semantic, profile, episodic)
}

/// Render active commitments as a bullet list body (without heading).
///
/// Deadlines are appended in parentheses: `- {title}: {description}（期限: 明日）`,
/// with overdue commitments explicitly marked (`（期限切れ）`). `lang` selects
/// the wording ("ja" vs anything else, which falls back to English); the strings
/// belong in a language pack once #338 lands. `now` anchors the relative-deadline
/// computation so every bullet in the block shares one reference instant.
pub fn render_commitments_block(
    commitments: &[ActiveCommitmentPrompt],
    lang: &str,
    now: DateTime<Utc>,
) -> String {
    let japanese = lang.eq_ignore_ascii_case("ja");
    let (open, close) = if japanese {
        ("（", "）")
    } else {
        (" (", ")")
    };

    commitments
        .iter()
        .map(|c| {
            let base = if c.description.is_empty() {
                c.title.clone()
            } else {
                format!("{}: {}", c.title, c.description)
            };
            match render_due_expression(c, now, japanese) {
                Some(due) => format!("{base}{open}{due}{close}"),
                None => base,
            }
        })
        .map(|line| format!("- {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Human-readable deadline expression for a single commitment, or `None` when
/// the commitment carries no deadline at all.
///
/// A parsed `due_at` wins over the raw `due_label`: the label is only a
/// fallback for hints extraction could not parse (e.g. "next time"), and is
/// rendered as-is. Overdue detection uses the ledger's `due_at < now` rule,
/// mirroring `mark_stale_commitments`.
fn render_due_expression(
    commitment: &ActiveCommitmentPrompt,
    now: DateTime<Utc>,
    japanese: bool,
) -> Option<String> {
    let prefix = if japanese { "期限: " } else { "Deadline: " };
    let overdue = if japanese { "期限切れ" } else { "overdue" };

    if let Some(due_at) = commitment.due_at {
        if due_at < now {
            return Some(overdue.to_string());
        }
        // Calendar-day offset (not a raw 24h span), so a deadline late
        // tomorrow renders as "明日"/"tomorrow" rather than "あと1日".
        let days = due_at
            .date_naive()
            .signed_duration_since(now.date_naive())
            .num_days();
        let relative = match (days, japanese) {
            (0, true) => "本日中".to_string(),
            (0, false) => "today".to_string(),
            (1, true) => "明日".to_string(),
            (1, false) => "tomorrow".to_string(),
            (n, true) => format!("あと{n}日"),
            (n, false) => format!("in {n} days"),
        };
        return Some(format!("{prefix}{relative}"));
    }

    commitment
        .due_label
        .as_deref()
        .filter(|label| !label.trim().is_empty())
        .map(|label| format!("{prefix}{label}"))
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::indexing_slicing,
        reason = "tests index into fixed-size fixture vectors"
    )]
    use super::*;
    use crate::character::{IdentityKernel, StyleExample, StyleIntent};
    use crate::format_recalled_content;
    use ene_store::{
        AffectAnnotation, MemoryConfidence, MemoryItem, MemorySalience, MemoryScope,
        MemoryScoreBreakdown, MemoryStatus,
    };

    fn compose_test_packet(
        kernel: IdentityKernel,
        style_examples: Vec<StyleExample>,
        recalled: &[RecalledMemory],
        commitments: &[ActiveCommitmentPrompt],
        affect_summary: Option<String>,
        history: Vec<HistoryEntry>,
        post_history_block: Option<String>,
        user_input: impl Into<String>,
    ) -> PromptPacket {
        let (semantic, profile, episodic) = classify_recalled_memories(recalled);
        let mut sections = Vec::new();

        sections.push(PromptSection::new(
            PromptSectionKind::IdentityKernel,
            kernel.text,
        ));

        if let Some(affect) = affect_summary {
            sections.push(PromptSection::new(
                PromptSectionKind::CharacterState,
                affect,
            ));
        }

        if !semantic.is_empty() {
            let body = semantic
                .iter()
                .map(|m| format_recalled_content(m))
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            sections.push(PromptSection::new(PromptSectionKind::SemanticContext, body));
        }

        if !profile.is_empty() {
            let body = profile
                .iter()
                .map(|m| format_recalled_content(m))
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            sections.push(PromptSection::new(PromptSectionKind::UserProfile, body));
        }

        if !commitments.is_empty() {
            sections.push(PromptSection::new(
                PromptSectionKind::ActiveCommitments,
                render_commitments_block(commitments, "en", chrono::Utc::now()),
            ));
        }

        if !episodic.is_empty() {
            let body = episodic
                .iter()
                .map(|m| format_recalled_content(m))
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            sections.push(PromptSection::new(
                PromptSectionKind::EpisodicMemories,
                body,
            ));
        }

        if !style_examples.is_empty() {
            let body = style_examples
                .into_iter()
                .map(|e| e.text)
                .collect::<Vec<_>>()
                .join("\n\n");
            sections.push(PromptSection::new(PromptSectionKind::StyleExamples, body));
        }

        if let Some(phi) = post_history_block {
            sections.push(PromptSection::new(PromptSectionKind::OutputContract, phi));
        }

        sections.push(PromptSection::new(
            PromptSectionKind::UserInput,
            user_input.into(),
        ));

        PromptPacket { sections, history }
    }

    #[test]
    fn section_order_is_deterministic() {
        let kernel = IdentityKernel {
            name: "Ene".into(),
            text: "KERNEL_MARKER".into(),
            post_history_instructions: None,
        };
        let styles = vec![StyleExample {
            text: "STYLE_MARKER".into(),
            intent: StyleIntent::Greeting,
        }];
        let packet = compose_test_packet(
            kernel,
            styles,
            &[],
            &[],
            Some("mood=calm".into()),
            vec![],
            Some("PHI_MARKER".into()),
            "hello",
        );
        let (messages, meta) = packet.to_llm_messages();
        assert!(meta.identity_kernel_included);
        assert!(meta.post_history_included);
        let LlmMessage::System { content } = &messages[0] else {
            panic!("expected system");
        };
        let kernel_pos = content.find("KERNEL_MARKER").expect("kernel");
        let style_pos = content.find("STYLE_MARKER").expect("style");
        let mood_pos = content.find("mood=calm").expect("mood");
        assert!(kernel_pos < style_pos);
        assert!(kernel_pos < mood_pos);
        assert_eq!(messages.len(), 3);
        let LlmMessage::System { content } = &messages[0] else {
            panic!("expected system");
        };
        assert!(!content.contains("PHI_MARKER"));
        let LlmMessage::System { content } = &messages[1] else {
            panic!("expected PHI system message");
        };
        assert!(content.contains("PHI_MARKER"));
        let LlmMessage::User { .. } = &messages[2] else {
            panic!("expected user message");
        };
    }

    #[test]
    fn identity_kernel_always_in_system_message() {
        let kernel = IdentityKernel {
            name: "Ene".into(),
            text: "KERNEL_MARKER".into(),
            post_history_instructions: None,
        };
        let packet = compose_test_packet(kernel, vec![], &[], &[], None, vec![], None, "hello");
        let (messages, meta) = packet.to_llm_messages();
        assert!(meta.identity_kernel_included);
        let LlmMessage::System { content } = &messages[0] else {
            panic!("expected system message");
        };
        assert!(content.contains("KERNEL_MARKER"));
    }

    #[test]
    fn post_history_block_appears_after_history_before_user() {
        let kernel = IdentityKernel {
            name: "Ene".into(),
            text: "KERNEL".into(),
            post_history_instructions: None,
        };
        let history = vec![HistoryEntry {
            role: ene_ai::Role::User,
            content: "prior".into(),
        }];
        let packet = compose_test_packet(
            kernel,
            vec![],
            &[],
            &[],
            None,
            history,
            Some("PHI_MARKER".into()),
            "current",
        );
        let (messages, meta) = packet.to_llm_messages();
        assert!(meta.post_history_included);
        assert_eq!(messages.len(), 4);
        let LlmMessage::User { .. } = &messages[1] else {
            panic!("expected history user message");
        };
        let LlmMessage::System { content } = &messages[2] else {
            panic!("expected PHI system message");
        };
        assert!(content.contains("PHI_MARKER"));
        let LlmMessage::User { parts } = &messages[3] else {
            panic!("expected current user message");
        };
        let UserMessagePart::Text { text } = &parts[0] else {
            panic!("expected text part");
        };
        assert_eq!(text, "current");
    }

    #[test]
    fn recalled_memories_split_by_kind() {
        let kernel = IdentityKernel {
            name: "Ene".into(),
            text: "K".into(),
            post_history_instructions: None,
        };
        let recalled = vec![RecalledMemory {
            item: MemoryItem {
                id: Some(1),
                scope: MemoryScope::User,
                character_id: "ene".into(),
                user_id: "u".into(),
                kind: MemoryKind::Preference,
                title: "drink".into(),
                content: "matcha".into(),
                source: MemorySource::Conversation,
                source_ref: None,
                confidence: MemoryConfidence::default(),
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
            reason: crate::recall::RecallReason::UserPreference,
            score_breakdown: MemoryScoreBreakdown {
                vector_similarity: 0.8,
                lexical_score: 0.0,
                recency_score: 0.0,
                salience: 0.0,
                confidence: 0.0,
                emotional_match: 0.0,
                relationship: 0.0,
                access_boost: 0.0,
                relevance: 0.8,
                quality_factor: 1.0,
                contradiction_penalty: 0.0,
                stale_penalty: 0.0,
                commitment_boost: 0.0,
                reflection_multiplier: 1.0,
                total: 0.8,
            },
            sources: vec![],
        }];
        let packet = compose_test_packet(kernel, vec![], &recalled, &[], None, vec![], None, "hi");
        let (messages, meta) = packet.to_llm_messages();
        assert_eq!(meta.recalled_memory_count, 1);
        let LlmMessage::System { content } = &messages[0] else {
            panic!("expected system");
        };
        assert!(content.contains("matcha"));
        assert!(content.contains("User Profile"));
    }

    fn sample_commitment(
        title: &str,
        description: &str,
        due_label: Option<&str>,
        due_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> ActiveCommitmentPrompt {
        ActiveCommitmentPrompt {
            id: 1,
            title: title.into(),
            description: description.into(),
            due_label: due_label.map(str::to_string),
            due_at,
        }
    }

    #[test]
    fn commitments_block_renders_future_due_at_as_relative() {
        // #385: a parsed `due_at` in the future becomes a relative expression
        // anchored at the renderer's `now` (calendar-day offset).
        let now = chrono::Utc::now();
        let commitment = sample_commitment(
            "資料を確認する",
            "企画書のレビュー",
            None,
            Some(now + chrono::Duration::days(2)),
        );
        let ja = render_commitments_block(std::slice::from_ref(&commitment), "ja", now);
        assert!(
            ja.contains("（期限: あと2日）"),
            "unexpected ja render: {ja}"
        );
        let en = render_commitments_block(&[commitment], "en", now);
        assert!(
            en.contains(" (Deadline: in 2 days)"),
            "unexpected en render: {en}"
        );
    }

    #[test]
    fn commitments_block_renders_today_and_tomorrow_terms() {
        // #385: natural relative terms for the same-day and next-day cases.
        // Anchor `now` to a fixed instant and derive both due dates from its
        // calendar date at midday, so a late-evening UTC run cannot roll the
        // same-day deadline across midnight (which would flip days to 1).
        let now = chrono::DateTime::<chrono::Utc>::from_timestamp(1_752_537_600, 0)
            .expect("fixed anchor timestamp is in range");
        let today = sample_commitment(
            "t1",
            "",
            None,
            Some(
                now.date_naive()
                    .and_hms_opt(12, 0, 0)
                    .expect("12:00 is valid")
                    .and_utc(),
            ),
        );
        let tomorrow = sample_commitment(
            "t2",
            "",
            None,
            Some(
                now.date_naive()
                    .and_hms_opt(12, 0, 0)
                    .expect("12:00 is valid")
                    .and_utc()
                    + chrono::Duration::days(1),
            ),
        );
        let ja = render_commitments_block(&[today.clone(), tomorrow.clone()], "ja", now);
        assert!(
            ja.contains("（期限: 本日中）"),
            "unexpected ja render: {ja}"
        );
        assert!(ja.contains("（期限: 明日）"), "unexpected ja render: {ja}");
        let en = render_commitments_block(&[today, tomorrow], "en", now);
        assert!(
            en.contains(" (Deadline: today)"),
            "unexpected en render: {en}"
        );
        assert!(
            en.contains(" (Deadline: tomorrow)"),
            "unexpected en render: {en}"
        );
    }

    #[test]
    fn commitments_block_marks_past_due_at_as_overdue() {
        // #385: a `due_at` before `now` is explicitly marked overdue, mirroring
        // the ledger's `mark_stale_commitments` (`due_at < now`).
        let now = chrono::Utc::now();
        let commitment = sample_commitment(
            "report",
            "send to manager",
            None,
            Some(now - chrono::Duration::days(1)),
        );
        let ja = render_commitments_block(std::slice::from_ref(&commitment), "ja", now);
        assert!(ja.contains("（期限切れ）"), "unexpected ja render: {ja}");
        let en = render_commitments_block(&[commitment], "en", now);
        assert!(en.contains(" (overdue)"), "unexpected en render: {en}");
    }

    #[test]
    fn commitments_block_renders_due_label_as_is() {
        // #385: without a parseable `due_at`, the raw `due_label` is rendered
        // verbatim ("期限: 明日"), because the label is the only deadline hint.
        let now = chrono::Utc::now();
        let commitment =
            sample_commitment("資料を確認する", "企画書のレビュー", Some("明日"), None);
        let ja = render_commitments_block(std::slice::from_ref(&commitment), "ja", now);
        assert!(ja.contains("（期限: 明日）"), "unexpected ja render: {ja}");
        let en = render_commitments_block(&[commitment], "en", now);
        assert!(
            en.contains(" (Deadline: 明日)"),
            "unexpected en render: {en}"
        );
    }

    #[test]
    fn commitments_block_without_deadline_is_unchanged() {
        // #385: commitments with neither `due_at` nor `due_label` keep the
        // historical title/description-only rendering.
        let now = chrono::Utc::now();
        let commitment = sample_commitment("buy milk", "from the corner store", None, None);
        let rendered = render_commitments_block(&[commitment], "ja", now);
        assert_eq!(rendered, "- buy milk: from the corner store");
    }

    #[test]
    fn commitments_block_prefers_due_at_over_due_label() {
        // #385: a parseable `due_at` wins over the raw label; the relative
        // expression is not clobbered by a stale/conflicting label.
        let now = chrono::Utc::now();
        let commitment = sample_commitment(
            "資料を確認する",
            "企画書のレビュー",
            Some("来週"),
            Some(now + chrono::Duration::days(2)),
        );
        let ja = render_commitments_block(&[commitment], "ja", now);
        assert!(
            ja.contains("（期限: あと2日）"),
            "unexpected ja render: {ja}"
        );
        assert!(!ja.contains("来週"), "label must not override due_at: {ja}");
    }
}
