//! LLM-based memory extractor.
//!
//! Uses a language model to identify memory candidates from a single
//! conversation turn. Returns `Vec<MemoryCandidate>` with structured
//! metadata. Falls back to an empty vec when the LLM returns no valid
//! candidates.
//!
//! The extractor is designed to be called after the deterministic extractor
//! in the memory pipeline. It handles:
//! - Structured output via JSON schema (`additionalProperties: false`)
//! - A configurable call timeout (see [`extract_with_timeout`]; the default is
//!   [`DEFAULT_EXTRACTION_TIMEOUT_SECS`])
//! - Markdown wrapper stripping (```json ... ```)
//! - Confidence capping at 0.9
//! - Unknown `kind` fallback to `Semantic`

use std::fmt::Write;

use ene_ai::{LlmMessage, LlmProvider, UserMessagePart};
use ene_config::PromptLibrary;

use super::candidate::{Locale, MemoryCandidate, TurnInput};
use crate::error::CognitionError;
use ene_store::typed_memory::MemoryKind;

/// Default timeout for the LLM call in seconds, used by [`extract`] when no
/// explicit budget is supplied. Callers that have a
/// [`MindMemoryConfig`](crate::config::MindMemoryConfig) should pass
/// its `extraction_timeout_secs` to [`extract_with_timeout`] instead.
pub const DEFAULT_EXTRACTION_TIMEOUT_SECS: u64 = 30;

/// Maximum confidence the extractor will accept from the LLM.
const MAX_CONFIDENCE: f32 = 0.9;

/// Advisory multiplier applied to confidence when the `source_quote` script
/// does not match the requested locale. Kept mild (not a hard drop) because
/// script heuristics cannot distinguish e.g. romaji Japanese from English,
/// so a mismatch is only a weak signal (Bugbot medium finding).
const LOCALE_MISMATCH_PENALTY: f32 = 0.8;

/// Minimum `source_quote` length (in characters) before the locale-mismatch
/// heuristic is applied. Short quotes such as names or interjections are too
/// ambiguous to penalize reliably.
const LOCALE_MISMATCH_MIN_CHARS: usize = 8;

/// Extracts memory candidates from a conversation turn using an LLM.
///
/// The provider is called with a JSON schema that forces structured output.
/// The function strips any markdown wrappers, parses the JSON, and maps
/// each raw candidate into a [`MemoryCandidate`].
///
/// Returns an empty vec (not an error) when the LLM produces zero candidates.
///
/// Uses [`DEFAULT_EXTRACTION_TIMEOUT_SECS`] for the call timeout. To honour the
/// operator-configured budget, use [`extract_with_timeout`].
pub async fn extract(
    provider: &dyn LlmProvider,
    turn: &TurnInput<'_>,
    locale: Locale,
) -> Result<Vec<MemoryCandidate>, CognitionError> {
    extract_with_timeout(provider, turn, locale, DEFAULT_EXTRACTION_TIMEOUT_SECS).await
}

/// Same as [`extract`], but with an explicit call-timeout budget in seconds.
///
/// This is the entry point the runtime should use so the timeout can be driven
/// by `MindMemoryConfig::extraction_timeout_secs` (issue #66).
pub async fn extract_with_timeout(
    provider: &dyn LlmProvider,
    turn: &TurnInput<'_>,
    locale: Locale,
    timeout_secs: u64,
) -> Result<Vec<MemoryCandidate>, CognitionError> {
    let prompts = PromptLibrary::load(match locale {
        Locale::Ja => "ja",
        Locale::En => "en",
    });

    let conversation = build_conversation_text(turn);

    let system_prompt = prompts.extractor().system.clone();
    let user_prompt = prompts.extractor().render_user_prompt(&conversation);

    let schema = extraction_schema();

    let messages = vec![
        LlmMessage::System {
            content: system_prompt,
        },
        LlmMessage::User {
            parts: vec![UserMessagePart::Text { text: user_prompt }],
        },
    ];

    let raw = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        provider.chat_completion(&messages, Some(schema)),
    )
    .await
    .map_err(|_| {
        CognitionError::ExtractionFailed(format!("LLM extraction timed out after {timeout_secs}s"))
    })?
    .map_err(|e| CognitionError::ExtractionFailed(format!("LLM provider error: {e}")))?;

    parse_candidates_json(&raw, locale)
}

/// Builds a plain-text conversation block from a `TurnInput`.
fn build_conversation_text(turn: &TurnInput<'_>) -> String {
    let mut out = String::new();
    if !turn.user_message.is_empty() {
        out.push_str("User: ");
        out.push_str(turn.user_message);
        out.push('\n');
    }
    if let Some(assistant) = turn.assistant_message
        && !assistant.is_empty()
    {
        out.push_str("Assistant: ");
        out.push_str(assistant);
        out.push('\n');
    }
    for tr in turn.tool_results {
        let _ = writeln!(
            out,
            "Tool({}): {}",
            tr.tool_name,
            if tr.success { "success" } else { "failed" }
        );
        if !tr.summary.is_empty() {
            out.push_str(&tr.summary);
            out.push('\n');
        }
    }
    out
}

/// Returns the JSON schema for the structured extraction output.
fn extraction_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "candidates": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "description": "Memory kind: Semantic, UserProfile, Preference, Procedure, Commitment, or Affective"
                        },
                        "title": {
                            "type": "string",
                            "description": "Short identifier for this memory (2-5 words)"
                        },
                        "content": {
                            "type": "string",
                            "description": "Full content of the memory to persist"
                        },
                        "source_quote": {
                            "type": "string",
                            "description": "Exact user text that triggered this extraction (max 100 chars)"
                        },
                        "confidence": {
                            "type": "number",
                            "description": "Confidence score 0.0-0.9 (never 1.0)"
                        },
                        "should_persist": {
                            "type": "boolean",
                            "description": "True for most candidates, false for deletion requests"
                        },
                        "deletion_target_key": {
                            "type": ["string", "null"],
                            "description": "Short identifier for memory to forget, null if not a deletion"
                        },
                        "commitment_due": {
                            "type": ["string", "null"],
                            "description": "Date/time string if deadline mentioned, null otherwise"
                        }
                    },
                    "required": ["kind", "title", "content", "source_quote", "confidence", "should_persist", "deletion_target_key", "commitment_due"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["candidates"],
        "additionalProperties": false
    })
}

/// Parses the raw LLM response into `Vec<MemoryCandidate>`.
///
/// Handles:
/// - Direct JSON
/// - Markdown-wrapped JSON (```json ... ```)
/// - Prose with embedded JSON object
fn parse_candidates_json(
    raw: &str,
    locale: Locale,
) -> Result<Vec<MemoryCandidate>, CognitionError> {
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    // Try direct parse first
    if let Ok(wrapper) = serde_json::from_str::<RawWrapper>(cleaned) {
        return Ok(wrapper
            .candidates
            .into_iter()
            .map(|r| raw_to_candidate(r, locale))
            .collect());
    }

    // Try extracting embedded JSON object
    if let Some(start) = cleaned.find('{')
        && let Some(end) = cleaned.rfind('}')
    {
        let json_str = &cleaned[start..=end];
        if let Ok(wrapper) = serde_json::from_str::<RawWrapper>(json_str) {
            return Ok(wrapper
                .candidates
                .into_iter()
                .map(|r| raw_to_candidate(r, locale))
                .collect());
        }
    }

    // Non-JSON response — treat as extraction failure
    Err(CognitionError::ExtractionFailed(format!(
        "LLM response was not valid JSON for the extractor schema. First 200 chars: {}",
        raw.chars().take(200).collect::<String>()
    )))
}

/// Raw JSON wrapper matching the schema's `candidates` array.
#[derive(serde::Deserialize)]
struct RawWrapper {
    candidates: Vec<RawCandidate>,
}

/// Raw JSON candidate before conversion to `MemoryCandidate`.
#[derive(serde::Deserialize)]
struct RawCandidate {
    kind: String,
    title: String,
    content: String,
    source_quote: String,
    confidence: f32,
    should_persist: bool,
    #[serde(default)]
    deletion_target_key: Option<String>,
    #[serde(default)]
    commitment_due: Option<String>,
}

/// Converts a raw JSON candidate into a `MemoryCandidate`.
///
/// - Maps unknown `kind` strings to `Semantic` and logs a warning.
/// - Caps confidence at `MAX_CONFIDENCE` (0.9).
fn raw_to_candidate(raw: RawCandidate, locale: Locale) -> MemoryCandidate {
    let kind = match raw.kind.to_lowercase().as_str() {
        "semantic" => MemoryKind::Semantic,
        "userprofile" | "user_profile" | "user profile" => MemoryKind::UserProfile,
        "preference" => MemoryKind::Preference,
        "procedure" => MemoryKind::Procedure,
        "commitment" => MemoryKind::Commitment,
        "affective" => MemoryKind::Affective,
        other => {
            tracing::warn!(
                raw_kind = other,
                "Unknown memory kind from LLM extractor, falling back to Semantic"
            );
            MemoryKind::Semantic
        }
    };

    let confidence = raw.confidence.clamp(0.0, MAX_CONFIDENCE);

    // Locale mismatch guard: if the extractor was called with a locale that
    // doesn't match the language of the source_quote, the candidate is likely
    // noise. We still return it but at reduced confidence.
    let confidence = if locale_mismatch(&raw.source_quote, locale) {
        tracing::debug!(
            source_quote = %raw.source_quote,
            "Locale mismatch detected, reducing confidence"
        );
        confidence * LOCALE_MISMATCH_PENALTY
    } else {
        confidence
    };

    MemoryCandidate {
        kind,
        title: raw.title,
        content: raw.content,
        source_quote: raw.source_quote,
        confidence,
        should_persist: raw.should_persist,
        deletion_target_key: raw.deletion_target_key,
        commitment_due: raw.commitment_due,
    }
}

/// Simple heuristic: if more than 50% of the characters are CJK and the
/// locale is `En`, or vice versa, we have a mismatch.
fn locale_mismatch(text: &str, locale: Locale) -> bool {
    let cjk_count = text
        .chars()
        .filter(|c| {
            let cp = *c as u32;
            (0x4E00..=0x9FFF).contains(&cp)   // CJK Unified Ideographs
            || (0x3040..=0x309F).contains(&cp)  // Hiragana
            || (0x30A0..=0x30FF).contains(&cp) // Katakana
        })
        .count();
    let total = text.chars().count();
    // Short quotes (names, interjections, code tokens) are too ambiguous for
    // the script heuristic to judge reliably, so they are never flagged.
    if total < LOCALE_MISMATCH_MIN_CHARS {
        return false;
    }
    let cjk_ratio = cjk_count as f32 / total as f32;
    match locale {
        Locale::Ja => cjk_ratio < 0.1, // Expected Japanese but mostly non-CJK
        Locale::En => cjk_ratio > 0.5, // Expected English but mostly CJK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type StreamResult = std::pin::Pin<
        Box<
            dyn tokio_stream::Stream<
                    Item = Result<ene_ai::LlmResponseChunk, ene_ai::LlmProviderError>,
                > + Send,
        >,
    >;

    /// Minimal mock provider that returns a fixed response.
    struct MockProvider {
        response: String,
    }

    impl MockProvider {
        fn new(response: impl Into<String>) -> Self {
            Self {
                response: response.into(),
            }
        }
    }

    #[async_trait::async_trait]
    impl ene_ai::LlmProvider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }

        async fn create_chat_stream(
            &self,
            _messages: &[LlmMessage],
            _tools: &[ene_tool_proto::ToolSpec],
        ) -> Result<StreamResult, ene_ai::LlmProviderError> {
            Err(ene_ai::LlmProviderError::Provider(
                "mock: not implemented".to_string(),
            ))
        }

        async fn chat_completion(
            &self,
            _messages: &[LlmMessage],
            _json_schema: Option<serde_json::Value>,
        ) -> Result<String, ene_ai::LlmProviderError> {
            Ok(self.response.clone())
        }
    }

    /// Provider that always returns a timeout-like error.
    struct TimeoutProvider;

    #[async_trait::async_trait]
    impl ene_ai::LlmProvider for TimeoutProvider {
        fn name(&self) -> &str {
            "timeout-mock"
        }

        async fn create_chat_stream(
            &self,
            _messages: &[LlmMessage],
            _tools: &[ene_tool_proto::ToolSpec],
        ) -> Result<StreamResult, ene_ai::LlmProviderError> {
            Err(ene_ai::LlmProviderError::Provider(
                "timeout: not implemented".to_string(),
            ))
        }

        async fn chat_completion(
            &self,
            _messages: &[LlmMessage],
            _json_schema: Option<serde_json::Value>,
        ) -> Result<String, ene_ai::LlmProviderError> {
            // Simulate a slow provider. The extract() timeout cancels this
            // future long before the sleep elapses, so the test stays fast.
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            Ok("{}".to_string())
        }
    }

    /// Provider that returns non-JSON.
    struct GarbageProvider;

    #[async_trait::async_trait]
    impl ene_ai::LlmProvider for GarbageProvider {
        fn name(&self) -> &str {
            "garbage-mock"
        }

        async fn create_chat_stream(
            &self,
            _messages: &[LlmMessage],
            _tools: &[ene_tool_proto::ToolSpec],
        ) -> Result<StreamResult, ene_ai::LlmProviderError> {
            Err(ene_ai::LlmProviderError::Provider(
                "garbage: not implemented".to_string(),
            ))
        }

        async fn chat_completion(
            &self,
            _messages: &[LlmMessage],
            _json_schema: Option<serde_json::Value>,
        ) -> Result<String, ene_ai::LlmProviderError> {
            Ok("I'm sorry, I can't extract memories from that.".to_string())
        }
    }

    fn ja_turn(msg: &str) -> TurnInput<'_> {
        TurnInput {
            user_message: msg,
            assistant_message: None,
            tool_results: &[],
        }
    }

    #[tokio::test]
    async fn valid_json_parsed_to_candidates() {
        let json = r#"{
            "candidates": [{
                "kind": "Semantic",
                "title": "project discussion",
                "content": "User is working on a project called Ene",
                "source_quote": "I'm working on Ene",
                "confidence": 0.85,
                "should_persist": true,
                "deletion_target_key": null,
                "commitment_due": null
            }]
        }"#;
        let provider = MockProvider::new(json);
        let result = extract(&provider, &ja_turn("I'm working on Ene"), Locale::En)
            .await
            .expect("extract failed");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].kind, MemoryKind::Semantic);
        assert_eq!(result[0].title, "project discussion");
        assert!((result[0].confidence - 0.85).abs() < 0.01);
    }

    #[tokio::test]
    async fn markdown_wrapper_stripped() {
        let json = "```json\n{\"candidates\": [{\"kind\": \"Preference\", \"title\": \"likes coffee\", \"content\": \"User likes coffee\", \"source_quote\": \"I love coffee\", \"confidence\": 0.7, \"should_persist\": true, \"deletion_target_key\": null, \"commitment_due\": null}]}\n```";
        let provider = MockProvider::new(json);
        let result = extract(&provider, &ja_turn("I love coffee"), Locale::En)
            .await
            .expect("extract failed");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].kind, MemoryKind::Preference);
    }

    #[tokio::test]
    async fn non_json_returns_extraction_failed() {
        let provider = GarbageProvider;
        let result = extract(&provider, &ja_turn("hello"), Locale::En).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, CognitionError::ExtractionFailed(_)),
            "Expected ExtractionFailed, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn empty_candidates_returns_empty_vec() {
        let json = r#"{"candidates": []}"#;
        let provider = MockProvider::new(json);
        let result = extract(&provider, &ja_turn("just chatting"), Locale::En)
            .await
            .expect("extract failed");
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn confidence_capped_at_max() {
        let json = r#"{
            "candidates": [{
                "kind": "Semantic",
                "title": "test",
                "content": "test content",
                "source_quote": "test quote",
                "confidence": 1.0,
                "should_persist": true,
                "deletion_target_key": null,
                "commitment_due": null
            }]
        }"#;
        let provider = MockProvider::new(json);
        let result = extract(&provider, &ja_turn("test"), Locale::En)
            .await
            .expect("extract failed");
        assert_eq!(result.len(), 1);
        assert!(result[0].confidence <= super::MAX_CONFIDENCE);
    }

    #[tokio::test]
    async fn unknown_kind_falls_back_to_semantic() {
        let json = r#"{
            "candidates": [{
                "kind": "CustomType",
                "title": "test",
                "content": "test content",
                "source_quote": "test quote",
                "confidence": 0.7,
                "should_persist": true,
                "deletion_target_key": null,
                "commitment_due": null
            }]
        }"#;
        let provider = MockProvider::new(json);
        let result = extract(&provider, &ja_turn("test"), Locale::En)
            .await
            .expect("extract failed");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].kind, MemoryKind::Semantic);
    }

    #[tokio::test]
    async fn timeout_returns_extraction_failed() {
        // A short, explicit budget (as would come from
        // `MindMemoryConfig::extraction_timeout_secs`) is honoured and
        // reported in the error message.
        let provider = TimeoutProvider;
        let result = extract_with_timeout(&provider, &ja_turn("test"), Locale::En, 1).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("timed out") && err_msg.contains("1s"),
            "Expected timeout error mentioning the configured budget, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn schema_has_additional_properties_false() {
        // Verify the schema includes additionalProperties: false
        let schema = super::extraction_schema();
        let candidates_schema = &schema["properties"]["candidates"]["items"];
        assert_eq!(candidates_schema["additionalProperties"], false);
        assert_eq!(schema["additionalProperties"], false);
    }

    #[tokio::test]
    async fn locale_mismatch_ja_text_with_en_locale_reduces_confidence() {
        let json = r#"{
            "candidates": [{
                "kind": "Semantic",
                "title": "テスト",
                "content": "テスト内容",
                "source_quote": "プロジェクトの話をしている",
                "confidence": 0.8,
                "should_persist": true,
                "deletion_target_key": null,
                "commitment_due": null
            }]
        }"#;
        let provider = MockProvider::new(json);
        let result = extract(
            &provider,
            &ja_turn("プロジェクトの話をしている"),
            Locale::En,
        )
        .await
        .expect("extract failed");
        assert_eq!(result.len(), 1);
        // Confidence should be reduced due to locale mismatch
        assert!(
            result[0].confidence < 0.8,
            "Expected reduced confidence, got {}",
            result[0].confidence
        );
    }

    #[tokio::test]
    async fn short_quote_is_not_penalized_by_locale_mismatch() {
        // A short CJK quote (e.g. a name) in an English session is too
        // ambiguous to penalize, so confidence is preserved (Bugbot finding).
        let json = r#"{
            "candidates": [{
                "kind": "UserProfile",
                "title": "name",
                "content": "User's name is Yuki",
                "source_quote": "ゆき",
                "confidence": 0.8,
                "should_persist": true,
                "deletion_target_key": null,
                "commitment_due": null
            }]
        }"#;
        let provider = MockProvider::new(json);
        let result = extract(&provider, &ja_turn("ゆき"), Locale::En)
            .await
            .expect("extract failed");
        assert_eq!(result.len(), 1);
        assert!(
            (result[0].confidence - 0.8).abs() < 0.01,
            "short quote must not be penalized, got {}",
            result[0].confidence
        );
    }

    #[tokio::test]
    async fn deletion_candidate_parsed_correctly() {
        let json = r#"{
            "candidates": [{
                "kind": "Semantic",
                "title": "forget project",
                "content": "User wants to forget about project X",
                "source_quote": "Forget about project X",
                "confidence": 0.9,
                "should_persist": false,
                "deletion_target_key": "project-x",
                "commitment_due": null
            }]
        }"#;
        let provider = MockProvider::new(json);
        let result = extract(&provider, &ja_turn("Forget about project X"), Locale::En)
            .await
            .expect("extract failed");
        assert_eq!(result.len(), 1);
        assert!(!result[0].should_persist);
        assert_eq!(result[0].deletion_target_key.as_deref(), Some("project-x"));
    }

    #[tokio::test]
    async fn commitment_due_parsed() {
        let json = r#"{
            "candidates": [{
                "kind": "Commitment",
                "title": "meeting tomorrow",
                "content": "User has a meeting tomorrow",
                "source_quote": "I have a meeting tomorrow at 3pm",
                "confidence": 0.85,
                "should_persist": true,
                "deletion_target_key": null,
                "commitment_due": "tomorrow 15:00"
            }]
        }"#;
        let provider = MockProvider::new(json);
        let result = extract(
            &provider,
            &ja_turn("I have a meeting tomorrow at 3pm"),
            Locale::En,
        )
        .await
        .expect("extract failed");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].commitment_due.as_deref(), Some("tomorrow 15:00"));
    }
}
