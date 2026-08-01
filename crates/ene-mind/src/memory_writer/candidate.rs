//! Shared types for deterministic and LLM-based memory extraction.

use ene_core::MemoryKind;
use serde::{Deserialize, Serialize};

/// Language locale for extraction pattern selection.
///
/// Carries a resolved primary language code (e.g. `"ja"`, `"en"`, `"ko"`) so
/// extraction can select per-language packs at runtime. Languages without a
/// dedicated pack fall back to English patterns in the deterministic
/// extractor.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Locale {
    /// Resolved primary language code (e.g. `"ja"`, `"en"`).
    code: String,
}

impl Locale {
    /// Resolves a free-form language tag to a [`Locale`].
    ///
    /// Matching is case-insensitive and keeps only the primary subtag, so
    /// `"ja"`, `"JA"`, and `"ja-JP"` all resolve to `"ja"`. The legacy alias
    /// `"jp"` also maps to `"ja"`. The result is not validated against the
    /// language packs; the loader falls back to English when no pack exists
    /// for the resolved code.
    pub fn resolve(lang: &str) -> Self {
        Self {
            code: ene_config::resolve_language_alias(lang),
        }
    }

    /// The resolved primary language code (e.g. `"ja"`, `"en"`).
    pub fn code(&self) -> &str {
        &self.code
    }
}

/// Summary of a tool call result, used for procedure memory extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultSummary {
    /// Name of the tool that was called.
    pub tool_name: String,
    /// Whether the tool call succeeded.
    pub success: bool,
    /// Brief description of what was done.
    pub summary: String,
}

/// Input for a single conversation turn, passed to extractors.
#[derive(Debug, Clone)]
pub struct TurnInput<'a> {
    /// The user's message text.
    pub user_message: &'a str,
    /// The assistant's response (if available).
    pub assistant_message: Option<&'a str>,
    /// Tool call results from this turn.
    pub tool_results: &'a [ToolResultSummary],
}

/// A candidate memory extracted from a conversation turn.
///
/// This is an intermediate representation — the Memory Arbiter (#75) will
/// decide whether to persist it, merge it with existing memories, or discard it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryCandidate {
    /// The kind of memory this candidate represents.
    pub kind: MemoryKind,
    /// Short title or label (e.g. "project X の話").
    pub title: String,
    /// Full content of the memory.
    pub content: String,
    /// The exact quote from the conversation that triggered extraction.
    pub source_quote: String,
    /// Confidence score (0.0–1.0).
    pub confidence: f32,
    /// Whether this candidate should be persisted as a new memory.
    /// `false` for deletion-request candidates.
    pub should_persist: bool,
    /// For deletion requests: the key to look up the memory to mark as `UserDeleted`.
    pub deletion_target_key: Option<String>,
    /// For commitment candidates: due date or time reference (e.g. "明日", "next week").
    pub commitment_due: Option<String>,
    /// Free-form tags applied to the candidate (e.g. `"interrupted"` when the
    /// source turn was cut short). Empty by default.
    #[serde(default)]
    pub tags: Vec<String>,
}
