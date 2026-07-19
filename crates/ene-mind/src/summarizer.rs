//! LLM-driven conversation summarizer.
//!
//! Called during session splits to convert raw conversation history into structured summaries
//! with natural-language descriptions and user-relevant key facts.
//!
//! Prompt templates are loaded from the `PromptLibrary` so all user-facing strings
//! stay out of compiled code and can be localised without recompilation.

use std::fmt::Write;

use ene_config::PromptLibrary;
use ene_store::{KeyFact, MemoryError};
use serde::{Deserialize, Serialize};

/// Structured conversation summary returned by the LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSummaryResult {
    /// Natural-language conversation summary
    pub summary: String,
    /// Important facts about the user (key-value format)
    pub key_facts: Vec<KeyFact>,
}

/// Summarizes a conversation history using an LLM, returning a structured
/// summary and key facts. The prompt includes existing keyfacts for
/// comparison, allowing updates, deletions, and retentions via the
/// `key_facts` output.
///
/// All prompt strings are sourced from `PromptLibrary::load("en")` so they
/// can be localised by swapping the locale file.
pub async fn summarize_conversation(
    provider: &dyn ene_ai::LlmProvider,
    history: &[ene_ai::LlmMessage],
    character_name: &str,
    user_name: &str,
    existing_facts: &[KeyFact],
) -> Result<ConversationSummaryResult, MemoryError> {
    if history.is_empty() {
        return Ok(ConversationSummaryResult {
            summary: String::new(),
            key_facts: vec![],
        });
    }

    let prompts = PromptLibrary::load("en");

    // Build conversation text from message history (tool messages excluded —
    // their content is already attributed to the assistant turn).
    let mut conversation_text = String::new();
    for message in history {
        match message {
            ene_ai::LlmMessage::User { parts } => {
                let mut text = String::new();
                for part in parts {
                    if let ene_ai::UserMessagePart::Text { text: t } = part {
                        text.push_str(t);
                    }
                }
                let _ = writeln!(conversation_text, "{user_name}: {text}");
            }
            ene_ai::LlmMessage::Assistant { content, .. } => {
                let text = content.as_deref().unwrap_or("");
                let _ = writeln!(conversation_text, "{character_name}: {text}");
            }
            ene_ai::LlmMessage::System { content } => {
                let _ = writeln!(conversation_text, "System: {content}");
            }
            // Tool responses are intentionally excluded from the summary:
            // their content was already attributed to the assistant turn that
            // produced the corresponding tool call, and re-quoting the raw
            // result would inflate the summary with data that does not reflect
            // the dialogue.
            ene_ai::LlmMessage::Tool { .. } => {}
        }
    }

    let no_facts_placeholder = &prompts.summarizer().no_facts_placeholder;
    let existing_facts_str = if existing_facts.is_empty() {
        no_facts_placeholder.clone()
    } else {
        existing_facts
            .iter()
            .map(|f| format!("- {}: {}", f.key, f.value))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let system_prompt = prompts.summarizer().render_system(
        user_name,
        character_name,
        &existing_facts_str,
        &conversation_text,
    );

    let user_prompt =
        prompts
            .summarizer()
            .render_user_prompt(user_name, &existing_facts_str, &conversation_text);

    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "summary": {
                "type": "string",
                "description": "2–4 sentence summary of the conversation's key events, decisions, and outcomes"
            },
            "key_facts": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "key": {
                            "type": "string",
                            "description": "Fact identifier. Use same key to update, 'previous_<key>' to archive, empty value to delete."
                        },
                        "value": {
                            "type": "string",
                            "description": "Fact value. Empty string means delete this fact."
                        }
                    },
                    "required": ["key", "value"],
                    "additionalProperties": false
                },
                "description": "Current and updated facts about the user"
            }
        },
        "required": ["summary", "key_facts"],
        "additionalProperties": false
    });

    let messages = vec![
        ene_ai::LlmMessage::System {
            content: system_prompt,
        },
        ene_ai::LlmMessage::User {
            parts: vec![ene_ai::UserMessagePart::Text { text: user_prompt }],
        },
    ];

    let content = tokio::time::timeout(
        std::time::Duration::from_mins(2),
        provider.chat_completion(&messages, Some(schema)),
    )
    .await
    .map_err(|_| {
        MemoryError::ApiRequestError(
            "summarization: chat completion timed out after 120s".to_string(),
        )
    })?
    .map_err(|e| MemoryError::ApiRequestError(format!("summarization: {e}")))?;

    parse_summary_json(&content)
}

/// Extracts and parses JSON from the LLM response.
///
/// Returns a structured [`MemoryError::ApiRequestError`] when the response
/// cannot be parsed. The previous implementation silently stored the raw
/// LLM text as the summary when JSON parsing failed, which meant a
/// markdown-wrapped or prose response would be persisted as a "summary" and
/// surface later in the recalled context as noise.
fn parse_summary_json(raw: &str) -> Result<ConversationSummaryResult, MemoryError> {
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    if let Ok(result) = serde_json::from_str::<ConversationSummaryResult>(cleaned) {
        return Ok(result);
    }

    if let Some(start) = cleaned.find('{')
        && let Some(end) = cleaned.rfind('}')
    {
        let json_str = &cleaned[start..=end];
        if let Ok(result) = serde_json::from_str::<ConversationSummaryResult>(json_str) {
            return Ok(result);
        }
    }

    Err(MemoryError::ApiRequestError(format!(
        "[Summarizer] LLM response was not valid JSON for the expected summary schema; \
         refusing to persist raw prose as a summary. First 200 chars: {}",
        raw.chars().take(200).collect::<String>()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_summary_json_valid_input() {
        let json =
            r#"{"summary": "Test summary.", "key_facts": [{"key": "job", "value": "fact1"}]}"#;
        let result = parse_summary_json(json).unwrap();
        assert_eq!(result.summary, "Test summary.");
        assert_eq!(result.key_facts.len(), 1);
        assert_eq!(result.key_facts[0].key, "job");
        assert_eq!(result.key_facts[0].value, "fact1");
    }

    #[test]
    fn parse_summary_json_strips_markdown_fences() {
        let json = "```json\n{\"summary\": \"test\", \"key_facts\": []}\n```";
        let result = parse_summary_json(json).unwrap();
        assert_eq!(result.summary, "test");
    }

    /// Regression test: the parser must NOT silently fall back to the raw LLM
    /// prose as a "summary". A non-JSON response is a structured error.
    #[test]
    fn parse_summary_json_non_json_input_returns_error() {
        let raw = "This is not valid JSON at all";
        let result = parse_summary_json(raw);
        assert!(result.is_err(), "expected an error, got {result:?}");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not valid JSON") || msg.contains("API request failed"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn prompt_library_summarizer_system_is_non_empty() {
        let lib = PromptLibrary::built_in_english();
        let system = &lib.summarizer().system;
        assert!(
            !system.is_empty(),
            "summarizer.system prompt must not be empty"
        );
        // Verify key structural elements are present
        assert!(system.contains("key_facts"), "should mention key_facts");
        assert!(system.contains("summary"), "should mention summary");
    }
}
