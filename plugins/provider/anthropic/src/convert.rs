//! Message and tool format conversion between the ene provider-agnostic
//! JSON format and the Anthropic Messages API format.
//!
//! The ene `LlmMessage` serializes as `{"role": "system"|"user"|"assistant"|"tool", ...}`.
//! Anthropic expects system prompts in a top-level `system` field, alternating
//! `user`/`assistant` messages with typed content blocks, and tool results
//! embedded inside `user` messages.

use serde_json::{Value, json};

/// Extracts system messages and converts the remaining messages to the
/// Anthropic Messages API format.
///
/// Returns `(system, messages)` where `system` is the concatenated system
/// prompt (or `None` if no system messages were present) and `messages` is
/// the Anthropic-format message array with typed content blocks.
///
/// Consecutive `tool` role messages are merged into a single `user` message
/// containing multiple `tool_result` blocks, as required by the Anthropic API.
pub(crate) fn to_anthropic_messages(messages: &[Value]) -> (Option<String>, Vec<Value>) {
    let mut system_parts: Vec<String> = Vec::new();
    let mut anthropic_messages: Vec<Value> = Vec::new();

    for msg in messages {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("");
        match role {
            "system" => {
                if let Some(content) = msg.get("content").and_then(Value::as_str)
                    && !content.is_empty()
                {
                    system_parts.push(content.to_string());
                }
            }
            "user" => {
                let content = convert_user_content(msg);
                anthropic_messages.push(json!({ "role": "user", "content": content }));
            }
            "assistant" => {
                let content = convert_assistant_content(msg);
                anthropic_messages.push(json!({ "role": "assistant", "content": content }));
            }
            "tool" => {
                let tool_result = json!({
                    "type": "tool_result",
                    "tool_use_id": msg.get("tool_call_id").and_then(Value::as_str).unwrap_or(""),
                    "content": msg.get("content").and_then(Value::as_str).unwrap_or(""),
                });
                if let Some(last) = anthropic_messages.last_mut() {
                    let is_user = last.get("role").and_then(Value::as_str) == Some("user");
                    if is_user
                        && let Some(arr) = last.get_mut("content").and_then(Value::as_array_mut)
                    {
                        arr.push(tool_result);
                        continue;
                    }
                }
                anthropic_messages.push(json!({ "role": "user", "content": [tool_result] }));
            }
            _ => {}
        }
    }

    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };

    (system, anthropic_messages)
}

/// Converts user message parts (text and image) to Anthropic content blocks.
///
/// Handles the ene `UserMessagePart` format (`{"Text": {"text": ...}}` and
/// `{"Image": {"base64_image_data": "data:...;base64,..."}}`), as well as a
/// plain string `content` field as a fallback.
fn convert_user_content(msg: &Value) -> Vec<Value> {
    let mut blocks = Vec::new();

    if let Some(parts) = msg.get("parts").and_then(Value::as_array) {
        for part in parts {
            if let Some(text_obj) = part.get("Text") {
                if let Some(text) = text_obj.get("text").and_then(Value::as_str) {
                    blocks.push(json!({ "type": "text", "text": text }));
                }
            } else if let Some(image_obj) = part.get("Image")
                && let Some(data_uri) = image_obj.get("base64_image_data").and_then(Value::as_str)
                && let Some((media_type, data)) = parse_data_uri(data_uri)
            {
                blocks.push(json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": media_type,
                        "data": data,
                    }
                }));
            }
        }
    }

    if blocks.is_empty()
        && let Some(content) = msg.get("content").and_then(Value::as_str)
    {
        blocks.push(json!({ "type": "text", "text": content }));
    }

    blocks
}

fn convert_assistant_content(msg: &Value) -> Vec<Value> {
    let mut blocks = Vec::new();

    if let Some(content) = msg.get("content").and_then(Value::as_str)
        && !content.is_empty()
    {
        blocks.push(json!({ "type": "text", "text": content }));
    }

    if let Some(tool_calls) = msg.get("tool_calls").and_then(Value::as_array) {
        for tc in tool_calls {
            let id = tc.get("id").and_then(Value::as_str).unwrap_or("");
            let name = tc.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments_str = tc.get("arguments").and_then(Value::as_str).unwrap_or("{}");
            let input: Value = serde_json::from_str(arguments_str).unwrap_or_else(|_| json!({}));
            blocks.push(json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": input,
            }));
        }
    }

    blocks
}

/// Converts tool specs to the Anthropic tool definition format.
///
/// `ToolSpec` JSON (`{ name, description, parameters, ... }`) is mapped to
/// the Anthropic format (`{ name, description, input_schema }`).
pub(crate) fn to_anthropic_tools(tools: &[Value]) -> Vec<Value> {
    tools
        .iter()
        .filter_map(|tool| {
            let name = tool.get("name").and_then(Value::as_str)?;
            let description = tool
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("");
            let input_schema = tool
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| json!({ "type": "object" }));
            Some(json!({
                "name": name,
                "description": description,
                "input_schema": input_schema,
            }))
        })
        .collect()
}

/// Parses a `data:<media_type>;base64,<data>` URI into `(media_type, data)`.
fn parse_data_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix("data:")?;
    let (header, data) = rest.split_once(',')?;
    let media_type = header.strip_suffix(";base64")?;
    if media_type.is_empty() || data.is_empty() {
        return None;
    }
    Some((media_type.to_string(), data.to_string()))
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "unit tests use unwrap for concise assertions"
)]
#[expect(
    clippy::indexing_slicing,
    reason = "test assertions index into known-length vectors"
)]
mod tests {
    use super::*;

    #[test]
    fn system_messages_extracted_and_joined() {
        let messages = vec![
            json!({"role": "system", "content": "You are helpful."}),
            json!({"role": "system", "content": "Be concise."}),
            json!({"role": "user", "parts": [{"Text": {"text": "Hello"}}]}),
        ];
        let (system, msgs) = to_anthropic_messages(&messages);
        assert_eq!(system.as_deref(), Some("You are helpful.\n\nBe concise."));
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
    }

    #[test]
    fn no_system_messages_returns_none() {
        let messages = vec![json!({"role": "user", "parts": [{"Text": {"text": "Hi"}}]})];
        let (system, msgs) = to_anthropic_messages(&messages);
        assert!(system.is_none());
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn empty_system_content_skipped() {
        let messages = vec![
            json!({"role": "system", "content": ""}),
            json!({"role": "user", "parts": [{"Text": {"text": "Hi"}}]}),
        ];
        let (system, _) = to_anthropic_messages(&messages);
        assert!(system.is_none());
    }

    #[test]
    fn user_text_parts_converted() {
        let messages = vec![json!({
            "role": "user",
            "parts": [
                {"Text": {"text": "Hello "}},
                {"Text": {"text": "world"}},
            ]
        })];
        let (_, msgs) = to_anthropic_messages(&messages);
        let content = msgs[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Hello ");
        assert_eq!(content[1]["text"], "world");
    }

    #[test]
    fn user_image_part_converted() {
        let messages = vec![json!({
            "role": "user",
            "parts": [
                {"Text": {"text": "What is this?"}},
                {"Image": {"base64_image_data": "data:image/png;base64,iVBORw0KGgo="}},
            ]
        })];
        let (_, msgs) = to_anthropic_messages(&messages);
        let content = msgs[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["type"], "base64");
        assert_eq!(content[1]["source"]["media_type"], "image/png");
        assert_eq!(content[1]["source"]["data"], "iVBORw0KGgo=");
    }

    #[test]
    fn user_plain_content_fallback() {
        let messages = vec![json!({"role": "user", "content": "plain text"})];
        let (_, msgs) = to_anthropic_messages(&messages);
        let content = msgs[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "plain text");
    }

    #[test]
    fn assistant_text_and_tool_calls_converted() {
        let messages = vec![json!({
            "role": "assistant",
            "content": "Let me read that file.",
            "tool_calls": [{
                "id": "call_123",
                "name": "fs.read",
                "arguments": "{\"path\":\"/tmp/test.txt\"}"
            }]
        })];
        let (_, msgs) = to_anthropic_messages(&messages);
        let content = msgs[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Let me read that file.");
        assert_eq!(content[1]["type"], "tool_use");
        assert_eq!(content[1]["id"], "call_123");
        assert_eq!(content[1]["name"], "fs.read");
        assert_eq!(content[1]["input"]["path"], "/tmp/test.txt");
    }

    #[test]
    fn assistant_null_content_omitted() {
        let messages = vec![json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": "call_1",
                "name": "test.tool",
                "arguments": "{}"
            }]
        })];
        let (_, msgs) = to_anthropic_messages(&messages);
        let content = msgs[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "tool_use");
    }

    #[test]
    fn assistant_invalid_arguments_defaults_to_empty_object() {
        let messages = vec![json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": "call_1",
                "name": "test.tool",
                "arguments": "not valid json"
            }]
        })];
        let (_, msgs) = to_anthropic_messages(&messages);
        let content = msgs[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["input"], json!({}));
    }

    #[test]
    fn tool_result_converted_to_user_message() {
        let messages = vec![
            json!({"role": "user", "parts": [{"Text": {"text": "Read a file"}}]}),
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{"id": "call_1", "name": "fs.read", "arguments": "{}"}]
            }),
            json!({"role": "tool", "tool_call_id": "call_1", "content": "file contents here"}),
        ];
        let (_, msgs) = to_anthropic_messages(&messages);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[2]["role"], "user");
        let content = msgs[2]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "call_1");
        assert_eq!(content[0]["content"], "file contents here");
    }

    #[test]
    fn consecutive_tool_results_merged() {
        let messages = vec![
            json!({"role": "user", "parts": [{"Text": {"text": "Do two things"}}]}),
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [
                    {"id": "call_1", "name": "a", "arguments": "{}"},
                    {"id": "call_2", "name": "b", "arguments": "{}"}
                ]
            }),
            json!({"role": "tool", "tool_call_id": "call_1", "content": "result 1"}),
            json!({"role": "tool", "tool_call_id": "call_2", "content": "result 2"}),
        ];
        let (_, msgs) = to_anthropic_messages(&messages);
        assert_eq!(msgs.len(), 3);
        let content = msgs[2]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["tool_use_id"], "call_1");
        assert_eq!(content[1]["tool_use_id"], "call_2");
    }

    #[test]
    fn empty_messages_returns_empty() {
        let (system, msgs) = to_anthropic_messages(&[]);
        assert!(system.is_none());
        assert!(msgs.is_empty());
    }

    #[test]
    fn tools_converted_to_anthropic_format() {
        let tools = vec![
            json!({
                "name": "fs.read",
                "description": "Read a file from disk.",
                "parameters": {
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"]
                },
                "background_capable": false
            }),
            json!({
                "name": "web.search",
                "description": "Search the web.",
                "parameters": {"type": "object"}
            }),
        ];
        let result = to_anthropic_tools(&tools);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["name"], "fs.read");
        assert_eq!(result[0]["description"], "Read a file from disk.");
        assert_eq!(result[0]["input_schema"]["type"], "object");
        assert!(result[0].get("parameters").is_none());
        assert!(result[0].get("background_capable").is_none());
        assert_eq!(result[1]["name"], "web.search");
    }

    #[test]
    fn tool_without_name_skipped() {
        let tools = vec![json!({"description": "no name"})];
        let result = to_anthropic_tools(&tools);
        assert!(result.is_empty());
    }

    #[test]
    fn tool_without_parameters_defaults_to_object() {
        let tools = vec![json!({"name": "test", "description": "desc"})];
        let result = to_anthropic_tools(&tools);
        assert_eq!(result[0]["input_schema"], json!({"type": "object"}));
    }

    #[test]
    fn empty_tools_returns_empty() {
        let result = to_anthropic_tools(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn parse_data_uri_valid() {
        let (media, data) = parse_data_uri("data:image/png;base64,AAAA").unwrap();
        assert_eq!(media, "image/png");
        assert_eq!(data, "AAAA");
    }

    #[test]
    fn parse_data_uri_jpeg() {
        let (media, data) = parse_data_uri("data:image/jpeg;base64,/9j/4AAQ").unwrap();
        assert_eq!(media, "image/jpeg");
        assert_eq!(data, "/9j/4AAQ");
    }

    #[test]
    fn parse_data_uri_missing_prefix() {
        assert!(parse_data_uri("image/png;base64,AAAA").is_none());
    }

    #[test]
    fn parse_data_uri_missing_base64_suffix() {
        assert!(parse_data_uri("data:image/png,AAAA").is_none());
    }

    #[test]
    fn parse_data_uri_empty_data() {
        assert!(parse_data_uri("data:image/png;base64,").is_none());
    }

    #[test]
    fn parse_data_uri_empty_media_type() {
        assert!(parse_data_uri("data:;base64,AAAA").is_none());
    }

    #[test]
    fn full_conversation_roundtrip() {
        let messages = vec![
            json!({"role": "system", "content": "You are a helpful assistant."}),
            json!({"role": "user", "parts": [{"Text": {"text": "What is 2+2?"}}]}),
            json!({"role": "assistant", "content": "The answer is 4."}),
            json!({"role": "user", "parts": [{"Text": {"text": "Thanks!"}}]}),
        ];
        let (system, msgs) = to_anthropic_messages(&messages);
        assert_eq!(system.as_deref(), Some("You are a helpful assistant."));
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[1]["content"][0]["text"], "The answer is 4.");
    }
}
