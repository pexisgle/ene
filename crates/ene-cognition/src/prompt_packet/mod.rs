//! Sectioned prompt packet composition (#87 seed for #100).

use ene_memory::ActiveCommitmentPrompt;
use ene_provider::{LlmMessage, UserMessagePart};

use crate::character::{IdentityKernel, StyleExample};
use crate::lifecycle::{HistoryEntry, PromptPacketMeta};
use crate::recall::{RecalledMemory, format_recalled_content};

/// A sectioned prompt structure with independent logical layers.
#[derive(Debug, Clone, Default)]
pub struct PromptPacket {
    /// Immutable character identity (never truncated).
    pub identity_kernel: Option<String>,
    /// Style example anchors (budgeted; droppable on overflow).
    pub style_examples: Vec<String>,
    /// Recalled typed memories.
    pub recalled_memories: Vec<RecalledMemory>,
    /// Active commitments for prompt injection.
    pub commitments: Vec<ActiveCommitmentPrompt>,
    /// Affect summary line (optional).
    pub affect_summary: Option<String>,
    /// Recent conversation history.
    pub history: Vec<HistoryEntry>,
    /// Expression PHI placed after history, before the user turn.
    pub post_history_block: Option<String>,
    /// Current user input.
    pub user_input: String,
    /// Maximum approximate characters for truncatable sections.
    pub max_chars: usize,
    /// Maximum approximate characters for the style-examples section.
    pub style_max_chars: usize,
}

impl PromptPacket {
    /// Convert the packet into LLM messages (system + history + PHI + user).
    #[must_use]
    pub fn to_llm_messages(&self) -> (Vec<LlmMessage>, PromptPacketMeta) {
        let mut system_parts = Vec::new();

        let identity_included = if let Some(kernel) = &self.identity_kernel {
            system_parts.push(kernel.clone());
            true
        } else {
            false
        };

        let style_block = self.render_style_block();
        let style_included = style_block.is_some();
        if let Some(block) = style_block {
            system_parts.push(block);
        }

        if !self.recalled_memories.is_empty() {
            let mut block = String::from("## Recalled Memories\n");
            for memory in &self.recalled_memories {
                let line = format_recalled_content(memory);
                block.push_str("- ");
                block.push_str(&line);
                block.push('\n');
            }
            system_parts.push(block);
        }

        if !self.commitments.is_empty() {
            let mut block = String::from("## Active Commitments\n");
            for c in &self.commitments {
                block.push_str("- ");
                block.push_str(&c.title);
                if !c.description.is_empty() {
                    block.push_str(": ");
                    block.push_str(&c.description);
                }
                block.push('\n');
            }
            system_parts.push(block);
        }

        if let Some(affect) = &self.affect_summary {
            system_parts.push(format!("## Current Mood\n{affect}"));
        }

        let mut system_text = system_parts.join("\n\n");
        if self.max_chars > 0 && system_text.chars().count() > self.max_chars {
            if let Some(kernel) = &self.identity_kernel {
                let kernel_char_len = kernel.chars().count();
                if kernel_char_len < self.max_chars {
                    let budget = self.max_chars - kernel_char_len;
                    let tail = system_text[kernel.len()..]
                        .chars()
                        .take(budget)
                        .collect::<String>();
                    system_text = format!("{kernel}{tail}");
                } else {
                    system_text = kernel.chars().take(self.max_chars).collect();
                }
            } else {
                system_text = system_text.chars().take(self.max_chars).collect();
            }
        }

        let mut messages = vec![LlmMessage::System {
            content: system_text,
        }];

        for entry in &self.history {
            let msg = match entry.role.as_str() {
                "assistant" => LlmMessage::Assistant {
                    content: Some(entry.content.clone()),
                    tool_calls: None,
                },
                "system" => LlmMessage::System {
                    content: entry.content.clone(),
                },
                _ => LlmMessage::User {
                    parts: vec![UserMessagePart::Text {
                        text: entry.content.clone(),
                    }],
                },
            };
            messages.push(msg);
        }

        let post_history_included = self
            .post_history_block
            .as_ref()
            .is_some_and(|block| !block.trim().is_empty());
        if post_history_included {
            messages.push(LlmMessage::System {
                content: self.post_history_block.clone().unwrap_or_default(),
            });
        }

        messages.push(LlmMessage::User {
            parts: vec![UserMessagePart::Text {
                text: self.user_input.clone(),
            }],
        });

        let meta = PromptPacketMeta {
            identity_kernel_included: identity_included,
            style_example_count: if style_included {
                self.style_examples.len()
            } else {
                0
            },
            recalled_memory_count: self.recalled_memories.len(),
            post_history_included,
        };

        (messages, meta)
    }

    fn render_style_block(&self) -> Option<String> {
        if self.style_examples.is_empty() || self.style_max_chars == 0 {
            return None;
        }
        let mut block = String::from("## Style Examples\n");
        for example in &self.style_examples {
            let candidate = if block == "## Style Examples\n" {
                format!("{block}{example}\n")
            } else {
                format!("{block}\n{example}\n")
            };
            if candidate.chars().count() > self.style_max_chars {
                break;
            }
            block = candidate;
        }
        if block == "## Style Examples\n" {
            None
        } else {
            Some(block)
        }
    }

    /// Build a packet from composed inputs.
    #[must_use]
    pub fn compose(
        kernel: IdentityKernel,
        style_examples: Vec<StyleExample>,
        recalled: Vec<RecalledMemory>,
        commitments: Vec<ActiveCommitmentPrompt>,
        affect_summary: Option<String>,
        history: Vec<HistoryEntry>,
        post_history_block: Option<String>,
        user_input: impl Into<String>,
        max_prompt_tokens: usize,
        style_example_budget_tokens: usize,
    ) -> Self {
        // Rough char budget from token limit (4 chars/token heuristic).
        let max_chars = max_prompt_tokens.saturating_mul(4);
        let style_max_chars = style_example_budget_tokens.saturating_mul(4);
        Self {
            identity_kernel: Some(kernel.text),
            style_examples: style_examples.into_iter().map(|e| e.text).collect(),
            recalled_memories: recalled,
            commitments,
            affect_summary,
            history,
            post_history_block,
            user_input: user_input.into(),
            max_chars,
            style_max_chars,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::character::StyleIntent;
    use ene_memory::{
        AffectAnnotation, MemoryConfidence, MemoryItem, MemoryKind, MemorySalience, MemoryScope,
        MemoryScoreBreakdown, MemorySource, MemoryStatus,
    };

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
            vec![],
            vec![],
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
    fn style_examples_appear_after_kernel() {
        let kernel = IdentityKernel {
            name: "Ene".into(),
            text: "KERNEL".into(),
            post_history_instructions: None,
        };
        let styles = vec![StyleExample {
            text: "STYLE_MARKER".into(),
            intent: StyleIntent::Greeting,
        }];
        let packet = PromptPacket::compose(
            kernel,
            styles,
            vec![],
            vec![],
            None,
            vec![],
            None,
            "hi",
            12_000,
            600,
        );
        let (messages, meta) = packet.to_llm_messages();
        assert_eq!(meta.style_example_count, 1);
        let LlmMessage::System { content } = &messages[0] else {
            panic!("expected system");
        };
        assert!(content.contains("KERNEL"));
        assert!(content.contains("STYLE_MARKER"));
        assert!(content.find("KERNEL").unwrap() < content.find("STYLE_MARKER").unwrap());
    }

    #[test]
    fn style_section_dropped_when_budget_zero() {
        let kernel = IdentityKernel {
            name: "Ene".into(),
            text: "K".into(),
            post_history_instructions: None,
        };
        let styles = vec![StyleExample {
            text: "STYLE".into(),
            intent: StyleIntent::Greeting,
        }];
        let mut packet = PromptPacket::compose(
            kernel,
            styles,
            vec![],
            vec![],
            None,
            vec![],
            None,
            "hi",
            12_000,
            600,
        );
        packet.style_max_chars = 0;
        let (_, meta) = packet.to_llm_messages();
        assert_eq!(meta.style_example_count, 0);
    }

    #[test]
    fn post_history_block_appears_after_history_before_user() {
        let kernel = IdentityKernel {
            name: "Ene".into(),
            text: "KERNEL".into(),
            post_history_instructions: None,
        };
        let history = vec![HistoryEntry {
            role: "user".into(),
            content: "prior".into(),
        }];
        let packet = PromptPacket::compose(
            kernel,
            vec![],
            vec![],
            vec![],
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
    fn recalled_memories_appear_in_system() {
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
            },
            reason: crate::recall::RecallReason::UserPreference,
            score_breakdown: ene_memory::MemoryScoreBreakdown {
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
            recalled,
            vec![],
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
    }
}
