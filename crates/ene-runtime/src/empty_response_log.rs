//! Diagnostics when an LLM stream completes without visible assistant text.

use ene_ai::{LlmMessage, UserMessagePart};
use ene_config::EneConfig;
use ene_mind::PromptPacketMeta;

const PROMPT_PREVIEW_CHARS: usize = 500;
const USER_INPUT_LOG_CHARS: usize = 200;
const RAW_CONTENT_PREVIEW_CHARS: usize = 200;

/// Inputs for [`log_empty_response_if_needed`].
pub struct EmptyResponseContext<'a> {
    pub pipeline: &'static str,
    pub config: &'a EneConfig,
    pub session_id: &'a str,
    pub character_id: &'a str,
    pub user_input: &'a str,
    pub round: usize,
    pub tool_count: usize,
    pub messages: &'a [LlmMessage],
    pub raw_assistant_content: &'a str,
    pub display_buffer: &'a str,
    pub emotion_tokens: &'a [String],
    pub suppress_stream_tokens: bool,
    pub prompt_meta: Option<&'a PromptPacketMeta>,
}

/// Emits structured warnings (and a debug prompt dump) when the user-visible
/// assistant text is empty after a completed stream round.
pub fn log_empty_response_if_needed(ctx: &EmptyResponseContext<'_>) {
    if !ctx.display_buffer.trim().is_empty() {
        return;
    }

    let (provider_name, model) = match ctx.config.get_section::<ene_ai::AiConfig>() {
        Ok(ai) => {
            let provider = ai.tasks.chat.provider.clone();
            match ai.resolve_chat() {
                Ok(chat) => (provider, chat.model),
                Err(_) => ("unknown".to_string(), "unknown".to_string()),
            }
        }
        Err(_) => ("unknown".to_string(), "unknown".to_string()),
    };

    let (system_chars, system_preview) = system_prompt_excerpt(ctx.messages);
    let outline = message_outline(ctx.messages);

    let warn = tracing::warn_span!(
        "empty_assistant_response",
        component = "Streaming",
        pipeline = ctx.pipeline,
        session_id = ctx.session_id,
        character_id = ctx.character_id,
        provider = %provider_name,
        model = %model,
        round = ctx.round,
        tool_count = ctx.tool_count,
        message_count = ctx.messages.len(),
        messages_outline = %outline,
        system_prompt_chars = system_chars,
        system_prompt_preview = %truncate_for_log(system_preview, PROMPT_PREVIEW_CHARS),
        user_input = %truncate_for_log(ctx.user_input, USER_INPUT_LOG_CHARS),
        raw_content_len = ctx.raw_assistant_content.len(),
        raw_content_preview = %truncate_for_log(
            ctx.raw_assistant_content,
            RAW_CONTENT_PREVIEW_CHARS
        ),
        display_buffer_len = ctx.display_buffer.len(),
        emotion_token_count = ctx.emotion_tokens.len(),
        suppress_stream_tokens = ctx.suppress_stream_tokens,
    );

    if let Some(meta) = ctx.prompt_meta {
        warn.record("packed_tokens", meta.packed_tokens);
        warn.record("recalled_memory_count", meta.recalled_memory_count);
        warn.record("style_example_count", meta.style_example_count);
        warn.record("post_history_included", meta.post_history_included);
        warn.record("identity_kernel_included", meta.identity_kernel_included);
        if !meta.dropped_sections.is_empty() {
            warn.record("dropped_sections", format!("{:?}", meta.dropped_sections));
        }
        if meta.history_messages_detached > 0 {
            warn.record("history_messages_detached", meta.history_messages_detached);
        }
    }

    if !ctx.emotion_tokens.is_empty() {
        warn.record("emotion_tokens", format!("{:?}", ctx.emotion_tokens));
    }

    warn.in_scope(|| {
        tracing::warn!(
            "LLM stream completed with no visible assistant text; inspect prompt preview and raw content fields"
        );
    });

    tracing::debug!(
        component = "Streaming",
        event = "empty_assistant_response_prompt_dump",
        session_id = %ctx.session_id,
        messages = ?ctx.messages,
        "Full prompt messages for empty response diagnosis"
    );
}

fn truncate_for_log(text: &str, max_chars: usize) -> String {
    if text.is_empty() {
        return String::new();
    }
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}…({char_count} chars total)")
}

fn message_content_len(message: &LlmMessage) -> usize {
    match message {
        LlmMessage::System { content } | LlmMessage::Tool { content, .. } => {
            content.chars().count()
        }
        LlmMessage::User { parts } => parts
            .iter()
            .map(|part| match part {
                UserMessagePart::Text { text } => text.chars().count(),
                UserMessagePart::Image { .. } => 0,
            })
            .sum(),
        LlmMessage::Assistant { content, .. } => content
            .as_deref()
            .map(str::chars)
            .map_or(0, Iterator::count),
    }
}

fn message_outline(messages: &[LlmMessage]) -> String {
    messages
        .iter()
        .map(|message| {
            let role = match message {
                LlmMessage::System { .. } => "system",
                LlmMessage::User { .. } => "user",
                LlmMessage::Assistant { tool_calls, .. } => {
                    if tool_calls.as_ref().is_some_and(|calls| !calls.is_empty()) {
                        return format!(
                            "assistant_tools({} calls)",
                            tool_calls.as_ref().map_or(0, Vec::len)
                        );
                    }
                    "assistant"
                }
                LlmMessage::Tool { .. } => "tool",
            };
            format!("{role}({}ch)", message_content_len(message))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn system_prompt_excerpt(messages: &[LlmMessage]) -> (usize, &str) {
    let Some(LlmMessage::System { content }) = messages.first() else {
        return (0, "");
    };
    (content.chars().count(), content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_for_log_appends_total_when_longer() {
        let text = "a".repeat(10);
        assert_eq!(truncate_for_log(&text, 4), "aaaa…(10 chars total)");
    }

    #[test]
    fn message_outline_includes_roles_and_lengths() {
        let messages = vec![
            LlmMessage::System {
                content: "sys".into(),
            },
            LlmMessage::User {
                parts: vec![UserMessagePart::Text {
                    text: "hello".into(),
                }],
            },
            LlmMessage::Assistant {
                content: Some("ok".into()),
                tool_calls: None,
            },
        ];
        assert_eq!(
            message_outline(&messages),
            "system(3ch), user(5ch), assistant(2ch)"
        );
    }

    #[test]
    fn log_skips_when_display_buffer_has_text() {
        let config = EneConfig::default();
        log_empty_response_if_needed(&EmptyResponseContext {
            pipeline: "legacy",
            config: &config,
            session_id: "sess",
            character_id: "Alicia",
            user_input: "hi",
            round: 0,
            tool_count: 0,
            messages: &[],
            raw_assistant_content: "",
            display_buffer: "visible",
            emotion_tokens: &[],
            suppress_stream_tokens: false,
            prompt_meta: None,
        });
    }
}
