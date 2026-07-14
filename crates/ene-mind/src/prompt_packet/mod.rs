//! Sectioned prompt packet composition (#87).

mod section;

pub use section::{PromptSection, PromptSectionKind};

use ene_ai::{LlmMessage, UserMessagePart};
use ene_store::{ActiveCommitmentPrompt, MemoryKind, MemorySource};

use crate::character::{IdentityKernel, StyleExample};
use crate::lifecycle::{HistoryEntry, PromptPacketMeta};
use crate::recall::{RecallReason, RecalledMemory, format_recalled_content};

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
    #[must_use]
    pub fn section(&self, kind: PromptSectionKind) -> Option<&PromptSection> {
        self.sections.iter().find(|s| s.kind == kind)
    }

    /// Whether a section has non-empty content.
    #[must_use]
    pub fn section_included(&self, kind: PromptSectionKind) -> bool {
        self.section(kind)
            .is_some_and(|s| !s.content.trim().is_empty())
    }

    /// Convert the packet into LLM messages (system + history + PHI + user).
    #[must_use]
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
            packed_tokens: 0,
        };

        (messages, meta)
    }

    /// Build a packet from composed inputs (legacy helper; prefer [`crate::context::pack_prompt`]).
    #[must_use]
    pub fn compose(
        kernel: IdentityKernel,
        style_examples: Vec<StyleExample>,
        recalled: &[RecalledMemory],
        commitments: &[ActiveCommitmentPrompt],
        affect_summary: Option<String>,
        history: Vec<HistoryEntry>,
        post_history_block: Option<String>,
        user_input: impl Into<String>,
        max_prompt_tokens: usize,
        style_example_budget_tokens: usize,
    ) -> Self {
        let (semantic, profile, episodic) = classify_recalled_memories(recalled);
        let mut sections = Vec::new();

        sections.push(PromptSection::new(
            PromptSectionKind::IdentityKernel,
            kernel.text,
            0,
        ));

        if let Some(affect) = affect_summary {
            sections.push(PromptSection::new(
                PromptSectionKind::CharacterState,
                affect,
                200,
            ));
        }

        if !semantic.is_empty() {
            let body = semantic
                .iter()
                .map(|m| format_recalled_content(m))
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            sections.push(PromptSection::new(
                PromptSectionKind::SemanticContext,
                body,
                max_prompt_tokens / 4,
            ));
        }

        if !profile.is_empty() {
            let body = profile
                .iter()
                .map(|m| format_recalled_content(m))
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            sections.push(PromptSection::new(
                PromptSectionKind::UserProfile,
                body,
                max_prompt_tokens / 4,
            ));
        }

        if !commitments.is_empty() {
            sections.push(PromptSection::new(
                PromptSectionKind::ActiveCommitments,
                render_commitments_block(commitments),
                400,
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
                max_prompt_tokens / 3,
            ));
        }

        if !style_examples.is_empty() {
            let body = style_examples
                .into_iter()
                .map(|e| e.text)
                .collect::<Vec<_>>()
                .join("\n\n");
            sections.push(PromptSection::new(
                PromptSectionKind::StyleExamples,
                body,
                style_example_budget_tokens,
            ));
        }

        if let Some(phi) = post_history_block {
            sections.push(PromptSection::new(
                PromptSectionKind::OutputContract,
                phi,
                0,
            ));
        }

        sections.push(PromptSection::new(
            PromptSectionKind::UserInput,
            user_input.into(),
            0,
        ));

        Self { sections, history }
    }
}

/// Split recalled memories into semantic, profile, and episodic buckets.
#[must_use]
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
#[must_use]
pub fn render_commitments_block(commitments: &[ActiveCommitmentPrompt]) -> String {
    commitments
        .iter()
        .map(|c| {
            if c.description.is_empty() {
                c.title.clone()
            } else {
                format!("{}: {}", c.title, c.description)
            }
        })
        .map(|line| format!("- {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::character::StyleIntent;
    use ene_store::{
        AffectAnnotation, MemoryConfidence, MemoryItem, MemorySalience, MemoryScope,
        MemoryScoreBreakdown, MemoryStatus,
    };

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
        let packet = PromptPacket::compose(
            kernel,
            styles,
            &[],
            &[],
            Some("mood=calm".into()),
            vec![],
            Some("PHI_MARKER".into()),
            "hello",
            12_000,
            600,
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
        let packet = PromptPacket::compose(
            kernel,
            vec![],
            &[],
            &[],
            None,
            vec![],
            None,
            "hello",
            12_000,
            600,
        );
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
        let packet = PromptPacket::compose(
            kernel,
            vec![],
            &[],
            &[],
            None,
            history,
            Some("PHI_MARKER".into()),
            "current",
            12_000,
            600,
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
                contradiction_penalty: 0.0,
                stale_penalty: 0.0,
                commitment_boost: 0.0,
                total: 0.8,
            },
            sources: vec![],
        }];
        let packet = PromptPacket::compose(
            kernel,
            vec![],
            &recalled,
            &[],
            None,
            vec![],
            None,
            "hi",
            12_000,
            600,
        );
        let (messages, meta) = packet.to_llm_messages();
        assert_eq!(meta.recalled_memory_count, 1);
        let LlmMessage::System { content } = &messages[0] else {
            panic!("expected system");
        };
        assert!(content.contains("matcha"));
        assert!(content.contains("User Profile"));
    }
}
