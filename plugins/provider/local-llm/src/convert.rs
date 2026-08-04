//! Conversion between the plugin wire format and the local engine types.

use ene_ai::EmbeddingError;
use ene_ai::error::LlmProviderError;
use ene_ai::message::{LlmMessage, LlmResponseChunk, LlmToolCallChunk};
use ene_plugin::{PluginError, PluginStreamChunk};
use serde_json::{Value, json};

/// Restores host-serialized [`LlmMessage`] values (the host sends the
/// message's own serde shape, so direct deserialization round-trips).
pub(crate) fn to_llm_messages(messages: &[Value]) -> Result<Vec<LlmMessage>, PluginError> {
    messages
        .iter()
        .map(|message| {
            serde_json::from_value(message.clone())
                .map_err(|e| PluginError::provider(format!("invalid chat message: {e}")))
        })
        .collect()
}

/// Maps every [`LlmProviderError`] variant onto a message-only
/// [`PluginError::Provider`]: the wire's typed kinds (auth / rate limit /
/// truncation) do not apply to a local engine, and the messages already
/// describe busy / timeout / cancelled distinctly.
pub(crate) fn map_llm_error(err: &LlmProviderError) -> PluginError {
    PluginError::provider(err.to_string())
}

/// Maps embedding errors without panicking.
pub(crate) fn map_embed_error(err: &EmbeddingError) -> PluginError {
    PluginError::provider(err.to_string())
}

/// Maps a generic [`LlmResponseChunk`] onto the plugin stream chunk. The
/// local core never emits tool-call deltas (no `Capability::Tools`), but the
/// conversion is written generally so a future core change cannot silently
/// drop them.
pub(crate) fn map_stream_chunk(chunk: LlmResponseChunk) -> PluginStreamChunk {
    PluginStreamChunk {
        text_delta: chunk.text_delta,
        tool_calls_delta: chunk.tool_calls_delta.map(|calls| {
            calls
                .into_iter()
                .map(|call| tool_call_to_value(&call))
                .collect()
        }),
        usage: chunk.usage,
    }
}

/// JSON shape the host's `parse_tool_call_delta` reads back.
fn tool_call_to_value(call: &LlmToolCallChunk) -> Value {
    json!({
        "index": call.index,
        "id": call.id,
        "name": call.name,
        "arguments": call.arguments,
    })
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect for concise assertions"
)]
mod tests {
    use ene_ai::error::LlmProviderError;
    use ene_ai::message::{LlmMessage, UserMessagePart};
    use ene_plugin::{PluginError, TokenUsage};

    use super::*;

    #[test]
    fn host_messages_round_trip_through_llm_message() {
        let messages = vec![
            LlmMessage::System {
                content: "You are helpful.".to_string(),
            },
            LlmMessage::User {
                parts: vec![
                    UserMessagePart::Text {
                        text: "Look at this: ".to_string(),
                    },
                    UserMessagePart::Image {
                        base64_image_data: "data:image/png;base64,AAAA".to_string(),
                    },
                ],
            },
            LlmMessage::Assistant {
                content: Some("Seen.".to_string()),
                tool_calls: None,
            },
        ];
        let wire: Vec<Value> = messages
            .iter()
            .map(|m| serde_json::to_value(m).expect("serialize message"))
            .collect();
        assert_eq!(to_llm_messages(&wire).expect("restore messages"), messages);
    }

    #[test]
    fn malformed_host_message_is_typed_error() {
        let wire = vec![json!({ "role": "bogus" })];
        assert!(to_llm_messages(&wire).is_err());
    }

    #[test]
    fn llm_provider_errors_map_without_panicking() {
        for err in [
            LlmProviderError::Busy { queue_depth: 2 },
            LlmProviderError::Timeout,
            LlmProviderError::Cancelled,
            LlmProviderError::LocalLlm("model file not found".to_string()),
        ] {
            let mapped = map_llm_error(&err);
            assert!(
                matches!(mapped, PluginError::Provider(_)),
                "every local error maps to Provider: {mapped:?}"
            );
        }
    }

    #[test]
    fn stream_chunk_maps_text_usage_and_tool_calls() {
        let chunk = LlmResponseChunk {
            text_delta: Some("hello".to_string()),
            tool_calls_delta: Some(vec![LlmToolCallChunk {
                index: 0,
                id: Some("call-1".to_string()),
                name: Some("ns.tool".to_string()),
                arguments: Some("{}".to_string()),
            }]),
            usage: Some(TokenUsage::new(10, 5, 15)),
        };
        let mapped = map_stream_chunk(chunk);
        assert_eq!(mapped.text_delta.as_deref(), Some("hello"));
        assert_eq!(
            mapped.tool_calls_delta.expect("tool calls mapped"),
            vec![json!({
                "index": 0,
                "id": "call-1",
                "name": "ns.tool",
                "arguments": "{}",
            })]
        );
        assert_eq!(
            mapped.usage,
            Some(TokenUsage::new(10, 5, 15)),
            "usage passes through to the final chunk"
        );
    }
}
