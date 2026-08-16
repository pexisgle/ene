//! Token usage accounting for LLM responses.
//!
//! This is wire ABI: the same [`TokenUsage`] value travels over the plugin IPC
//! (inside [`PluginIpcResponse::ChatCompletionResult`](crate::PluginIpcResponse)
//! and [`PluginIpcResponse::StreamChunk`](crate::PluginIpcResponse)) and is
//! re-exported by `ene-ai` for its in-process provider types. Defining it here
//! — the crate every provider layer already depends on — keeps a single
//! definition shared across the IPC boundary instead of two that must be
//! converted back and forth.

use serde::{Deserialize, Serialize};

/// Token usage reported by an LLM provider for a single completion.
///
/// Every field is optional because providers report usage inconsistently: some
/// return all three counts, some only a total, some only the completion
/// count, and some nothing at all (in which case the caller falls back to a
/// character-based estimate). Keeping the fields independent lets a consumer
/// use whatever partial information arrives rather than discarding a response
/// that happens to be missing one count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    #[serde(default)]
    pub prompt_tokens: Option<u32>,
    #[serde(default)]
    pub completion_tokens: Option<u32>,
    #[serde(default)]
    pub total_tokens: Option<u32>,
}

impl TokenUsage {
    /// A usage with all three counts populated.
    #[must_use]
    pub const fn new(prompt_tokens: u32, completion_tokens: u32, total_tokens: u32) -> Self {
        Self {
            prompt_tokens: Some(prompt_tokens),
            completion_tokens: Some(completion_tokens),
            total_tokens: Some(total_tokens),
        }
    }

    /// Characters-per-token divisor for the fallback estimate
    /// ([`TokenUsage::estimate`]). A compromise between English (~4 chars per
    /// token) and Japanese (~1 char per token): dividing by 3 over-estimates
    /// the token count of English text and under-estimates Japanese less than
    /// a naive `/4` would. Over-estimating is the safe direction for budgeting
    /// — it keeps a prompt under the model's real window rather than past it.
    const CHARS_PER_TOKEN: usize = 3;

    /// A character-based estimate of the usage for a prompt/completion pair,
    /// for providers that report no usage of their own.
    ///
    /// Every field is populated (the estimate is self-consistent: total is the
    /// sum of the two parts) so a consumer can treat it like a reported usage,
    /// but it is deliberately coarse — see [`Self::CHARS_PER_TOKEN`]. An empty
    /// input estimates to zero tokens.
    #[must_use]
    pub fn estimate(prompt: &str, completion: &str) -> Self {
        fn tokens(chars: usize) -> u32 {
            u32::try_from(chars.div_ceil(TokenUsage::CHARS_PER_TOKEN)).unwrap_or(u32::MAX)
        }
        let prompt_tokens = tokens(prompt.chars().count());
        let completion_tokens = tokens(completion.chars().count());
        Self::new(
            prompt_tokens,
            completion_tokens,
            prompt_tokens.saturating_add(completion_tokens),
        )
    }

    /// The reported total, or `prompt + completion` when both are known and
    /// the provider omitted an explicit total.
    #[must_use]
    pub fn total_or_sum(&self) -> Option<u32> {
        if let Some(total) = self.total_tokens {
            return Some(total);
        }
        match (self.prompt_tokens, self.completion_tokens) {
            (Some(prompt), Some(completion)) => Some(prompt.saturating_add(completion)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_all_none() {
        let usage = TokenUsage::default();
        assert_eq!(usage.prompt_tokens, None);
        assert_eq!(usage.completion_tokens, None);
        assert_eq!(usage.total_tokens, None);
    }

    #[test]
    fn missing_fields_deserialize_to_none() {
        // Load-bearing contract: an older peer omits the usage fields
        // entirely; they must default to `None`, not error.
        let usage: TokenUsage = serde_json::from_str("{}").unwrap();
        assert_eq!(usage, TokenUsage::default());
    }

    #[test]
    fn partial_usage_roundtrips() {
        let json = r#"{"completion_tokens":42}"#;
        let usage: TokenUsage = serde_json::from_str(json).unwrap();
        assert_eq!(usage.completion_tokens, Some(42));
        assert_eq!(usage.prompt_tokens, None);
        let back: TokenUsage =
            serde_json::from_str(&serde_json::to_string(&usage).unwrap()).unwrap();
        assert_eq!(usage, back);
    }

    #[test]
    fn total_or_sum_prefers_reported_total() {
        let usage = TokenUsage::new(10, 20, 99);
        assert_eq!(usage.total_or_sum(), Some(99));
    }

    #[test]
    fn total_or_sum_sums_when_total_missing() {
        let usage = TokenUsage {
            prompt_tokens: Some(10),
            completion_tokens: Some(20),
            total_tokens: None,
        };
        assert_eq!(usage.total_or_sum(), Some(30));
    }

    #[test]
    fn total_or_sum_none_when_incomplete() {
        let usage = TokenUsage {
            prompt_tokens: Some(10),
            completion_tokens: None,
            total_tokens: None,
        };
        assert_eq!(usage.total_or_sum(), None);
    }

    #[test]
    fn estimate_is_self_consistent() {
        let usage = TokenUsage::estimate("hello world", "hi");
        let prompt = usage.prompt_tokens.unwrap();
        let completion = usage.completion_tokens.unwrap();
        assert_eq!(usage.total_tokens, Some(prompt.saturating_add(completion)));
        assert!(prompt > 0, "non-empty prompt estimates to >0 tokens");
        assert!(
            completion > 0,
            "non-empty completion estimates to >0 tokens"
        );
    }

    #[test]
    fn estimate_of_empty_input_is_zero() {
        let usage = TokenUsage::estimate("", "");
        assert_eq!(usage, TokenUsage::new(0, 0, 0));
    }
}
