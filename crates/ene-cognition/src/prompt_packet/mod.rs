//! Sectioned prompt packet composition (#87 seed for #100).

use ene_memory::ActiveCommitmentPrompt;
use ene_provider::{LlmMessage, UserMessagePart};

use crate::character::IdentityKernel;
use crate::lifecycle::{HistoryEntry, PromptPacketMeta};
use crate::recall::{RecalledMemory, format_recalled_content};

/// A sectioned prompt structure with independent logical layers.
#[derive(Debug, Clone, Default)]
pub struct PromptPacket {
    /// Immutable character identity (never truncated).
    pub identity_kernel: Option<String>,
    /// Recalled typed memories.
    pub recalled_memories: Vec<RecalledMemory>,
    /// Active commitments for prompt injection.
    pub commitments: Vec<ActiveCommitmentPrompt>,
    /// Affect summary line (optional).
    pub affect_summary: Option<String>,
    /// Recent conversation history.
    pub history: Vec<HistoryEntry>,
    /// Current user input.
    pub user_input: String,
    /// Maximum approximate characters for truncatable sections.
    pub max_chars: usize,
}

impl PromptPacket {
    /// Convert the packet into LLM messages (system + history + user).
    #[must_use]
    pub fn to_llm_messages(&self) -> (Vec<LlmMessage>, PromptPacketMeta) {
        let mut system_parts = Vec::new();

        let identity_included = if let Some(kernel) = &self.identity_kernel {
            system_parts.push(kernel.clone());
            true
        } else {
            false
        };

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
        if self.max_chars > 0 && system_text.len() > self.max_chars {
            if let Some(kernel) = &self.identity_kernel {
                let kernel_len = kernel.len();
                if kernel_len < self.max_chars {
                    let budget = self.max_chars - kernel_len;
                    let tail = system_text[kernel_len..]
                        .chars()
                        .take(budget)
                        .collect::<String>();
                    system_text = format!("{kernel}{tail}");
                } else {
                    system_text = kernel.clone();
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

        messages.push(LlmMessage::User {
            parts: vec![UserMessagePart::Text {
                text: self.user_input.clone(),
            }],
        });

        let meta = PromptPacketMeta {
            identity_kernel_included: identity_included,
            recalled_memory_count: self.recalled_memories.len(),
        };

        (messages, meta)
    }

    /// Build a packet from composed inputs.
    #[must_use]
    pub fn compose(
        kernel: IdentityKernel,
        recalled: Vec<RecalledMemory>,
        commitments: Vec<ActiveCommitmentPrompt>,
        affect_summary: Option<String>,
        history: Vec<HistoryEntry>,
        user_input: impl Into<String>,
        max_prompt_tokens: usize,
    ) -> Self {
        // Rough char budget from token limit (4 chars/token heuristic).
        let max_chars = max_prompt_tokens.saturating_mul(4);
        Self {
            identity_kernel: Some(kernel.text),
            recalled_memories: recalled,
            commitments,
            affect_summary,
            history,
            user_input: user_input.into(),
            max_chars,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_memory::{
        AffectAnnotation, MemoryConfidence, MemoryItem, MemoryKind, MemorySalience, MemoryScope,
        MemoryScoreBreakdown, MemorySource, MemoryStatus,
    };

    #[test]
    fn identity_kernel_always_in_system_message() {
        let kernel = IdentityKernel {
            name: "Ene".into(),
            text: "KERNEL_MARKER".into(),
        };
        let packet = PromptPacket::compose(kernel, vec![], vec![], None, vec![], "hello", 12_000);
        let (messages, meta) = packet.to_llm_messages();
        assert!(meta.identity_kernel_included);
        let LlmMessage::System { content } = &messages[0] else {
            panic!("expected system message");
        };
        assert!(content.contains("KERNEL_MARKER"));
    }

    #[test]
    fn recalled_memories_appear_in_system() {
        let kernel = IdentityKernel {
            name: "Ene".into(),
            text: "K".into(),
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
        let packet = PromptPacket::compose(kernel, recalled, vec![], None, vec![], "hi", 12_000);
        let (messages, meta) = packet.to_llm_messages();
        assert_eq!(meta.recalled_memory_count, 1);
        let LlmMessage::System { content } = &messages[0] else {
            panic!("expected system");
        };
        assert!(content.contains("matcha"));
    }
}
