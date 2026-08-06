//! Conversion between the plugin wire format and the OpenAI-compatible JSON
//! shapes `llama-server` serves.

use ene_ai::error::LlmProviderError;
use ene_ai::message::{LlmMessage, UserMessagePart};
use ene_plugin::PluginError;
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

/// Converts unified messages to the `messages` array of the `OpenAI` chat
/// completions API. Image parts become `image_url` content blocks carrying
/// the original base64 data URI, matching what llama-server's multimodal chat
/// route accepts.
pub(crate) fn messages_to_oai(messages: &[LlmMessage]) -> Result<Value, PluginError> {
    messages
        .iter()
        .map(|message| match message {
            LlmMessage::System { content } => Ok(json!({
                "role": "system",
                "content": content,
            })),
            LlmMessage::User { parts } => {
                let content: Vec<Value> = parts
                    .iter()
                    .map(|part| match part {
                        UserMessagePart::Text { text } => json!({
                            "type": "text",
                            "text": text,
                        }),
                        UserMessagePart::Image { base64_image_data } => json!({
                            "type": "image_url",
                            "image_url": { "url": base64_image_data },
                        }),
                    })
                    .collect();
                Ok(json!({
                    "role": "user",
                    "content": content,
                }))
            }
            LlmMessage::Assistant {
                content,
                tool_calls,
            } => {
                if tool_calls.as_ref().is_some_and(|calls| !calls.is_empty()) {
                    return Err(PluginError::not_supported(
                        "assistant tool-call messages (local models do not support tool calls)",
                    ));
                }
                Ok(json!({
                    "role": "assistant",
                    "content": content.as_deref().unwrap_or_default(),
                }))
            }
            LlmMessage::Tool {
                tool_call_id,
                content,
            } => Ok(json!({
                "role": "tool",
                "tool_call_id": tool_call_id,
                "content": content,
            })),
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

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect for concise assertions"
)]
mod tests {
    use ene_ai::message::LlmMessage;
    use ene_plugin::PluginStreamChunk;

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
    fn oai_conversion_preserves_roles_and_image_data_uris() {
        let messages = vec![
            LlmMessage::System {
                content: "sys".to_string(),
            },
            LlmMessage::User {
                parts: vec![
                    UserMessagePart::Text {
                        text: "text".to_string(),
                    },
                    UserMessagePart::Image {
                        base64_image_data: "data:image/png;base64,AAAA".to_string(),
                    },
                ],
            },
            LlmMessage::Assistant {
                content: Some("reply".to_string()),
                tool_calls: None,
            },
        ];
        let oai = messages_to_oai(&messages).expect("converts");
        let array = oai.as_array().expect("array");
        assert_eq!(array[0]["role"], "system");
        assert_eq!(array[0]["content"], "sys");
        assert_eq!(array[1]["role"], "user");
        assert_eq!(array[1]["content"][0]["type"], "text");
        assert_eq!(array[1]["content"][1]["type"], "image_url");
        assert_eq!(
            array[1]["content"][1]["image_url"]["url"],
            "data:image/png;base64,AAAA"
        );
        assert_eq!(array[2]["role"], "assistant");
        assert_eq!(array[2]["content"], "reply");
    }

    #[test]
    fn assistant_tool_calls_are_rejected() {
        let messages = vec![LlmMessage::Assistant {
            content: None,
            tool_calls: Some(vec![ene_ai::message::LlmToolCall {
                id: "call-1".to_string(),
                name: "ns.tool".to_string(),
                arguments: "{}".to_string(),
            }]),
        }];
        assert!(messages_to_oai(&messages).is_err());
    }

    #[test]
    fn tool_messages_pass_tool_call_id() {
        let messages = vec![LlmMessage::Tool {
            tool_call_id: "call-1".to_string(),
            content: "result".to_string(),
        }];
        let oai = messages_to_oai(&messages).expect("converts");
        assert_eq!(oai[0]["role"], "tool");
        assert_eq!(oai[0]["tool_call_id"], "call-1");
        assert_eq!(oai[0]["content"], "result");
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
    fn stream_chunk_shape_is_available() {
        let chunk = PluginStreamChunk {
            text_delta: Some("hello".to_string()),
            tool_calls_delta: None,
            usage: None,
        };
        assert_eq!(chunk.text_delta.as_deref(), Some("hello"));
        assert!(chunk.tool_calls_delta.is_none());
        assert!(chunk.usage.is_none());
    }
}
