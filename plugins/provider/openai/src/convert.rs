//! Message, tool, and SSE-chunk conversion between the ene provider-agnostic
//! JSON format and the OpenAI Chat Completions wire format.
//!
//! The ene `LlmMessage` serializes as `{"role": "system"|"user"|"assistant"|"tool", ...}`
//! (see `ene-ai`'s `message` module); the OpenAI API expects a flat message
//! array with the same roles plus typed content parts for multimodal inputs.

use std::collections::HashMap;

use ene_plugin::{PluginError, PluginStreamChunk, TokenUsage};
use serde_json::{Value, json};

/// Converts ene chat messages to OpenAI Chat Completions messages.
///
/// Accepts the `LlmMessage` wire shape. A user message with exactly one text
/// part becomes a plain string `content`; anything else becomes a content
/// part array (`text` / `image_url` parts). Assistant tool calls carry the
/// API-sanitized tool name; the original name is restored from the
/// `name_mapping` when streaming deltas come back.
pub(crate) fn to_openai_messages(messages: &[Value]) -> Result<Vec<Value>, PluginError> {
    let mut out = Vec::with_capacity(messages.len());
    for msg in messages {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("");
        match role {
            "system" => {
                let content = msg.get("content").and_then(Value::as_str).unwrap_or("");
                out.push(json!({ "role": "system", "content": content }));
            }
            "user" => out.push(json!({ "role": "user", "content": user_content(msg) })),
            "assistant" => out.push(assistant_content(msg)),
            "tool" => {
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": msg.get("tool_call_id").and_then(Value::as_str).unwrap_or(""),
                    "content": msg.get("content").and_then(Value::as_str).unwrap_or(""),
                }));
            }
            _ => {
                return Err(PluginError::provider(format!(
                    "unknown message role: {role:?}"
                )));
            }
        }
    }
    Ok(out)
}

/// The `content` field of a user message: a plain string for a single text
/// part, otherwise a content part array (text and/or image parts).
fn user_content(msg: &Value) -> Value {
    let Some(parts) = msg.get("parts").and_then(Value::as_array) else {
        return msg.get("content").cloned().unwrap_or_else(|| json!(""));
    };

    let mut oa_parts = Vec::new();
    for part in parts {
        if let Some(text_obj) = part.get("Text")
            && let Some(text) = text_obj.get("text").and_then(Value::as_str)
        {
            oa_parts.push(json!({ "type": "text", "text": text }));
        } else if let Some(image_obj) = part.get("Image")
            && let Some(url) = image_obj.get("base64_image_data").and_then(Value::as_str)
        {
            // OpenAI accepts a data URI directly in `image_url.url`; the
            // provider decides the media type from the URI prefix.
            oa_parts.push(json!({ "type": "image_url", "image_url": { "url": url } }));
        }
    }

    if oa_parts.is_empty() {
        return json!("");
    }
    if oa_parts.len() == 1 && oa_parts[0].get("type").and_then(Value::as_str) == Some("text") {
        return oa_parts[0]
            .get("text")
            .cloned()
            .unwrap_or_else(|| json!(""));
    }
    Value::Array(oa_parts)
}

/// An assistant message: optional text plus API-sanitized tool calls.
fn assistant_content(msg: &Value) -> Value {
    let mut out = json!({ "role": "assistant" });
    if let Some(content) = msg.get("content").and_then(Value::as_str)
        && !content.is_empty()
    {
        out["content"] = json!(content);
    }
    if let Some(tool_calls) = msg.get("tool_calls").and_then(Value::as_array) {
        let calls: Vec<Value> = tool_calls
            .iter()
            .map(|call| {
                json!({
                    "id": call.get("id").and_then(Value::as_str).unwrap_or(""),
                    "type": "function",
                    "function": {
                        "name": sanitize_tool_name(call.get("name").and_then(Value::as_str).unwrap_or("")),
                        "arguments": call.get("arguments").and_then(Value::as_str).unwrap_or(""),
                    }
                })
            })
            .collect();
        out["tool_calls"] = Value::Array(calls);
    }
    out
}

/// Replaces characters the OpenAI API rejects in tool names with `_`, so
/// namespaced tool names (`namespace.action`) keep a stable, callable shape.
pub(crate) fn sanitize_tool_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Converts ene `ToolSpec` wire values to OpenAI `tools` entries.
pub(crate) fn to_openai_tools(tools: &[Value]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": sanitize_tool_name(t.get("name").and_then(Value::as_str).unwrap_or("")),
                    "description": t.get("description").and_then(Value::as_str).unwrap_or(""),
                    "parameters": t.get("parameters").cloned().unwrap_or_else(|| json!({})),
                }
            })
        })
        .collect()
}

/// Name mapping from sanitized to original tool names, for restoring the
/// original name on streamed tool-call deltas.
pub(crate) fn tool_name_mapping(tools: &[Value]) -> HashMap<String, String> {
    tools
        .iter()
        .filter_map(|t| {
            let name = t.get("name").and_then(Value::as_str)?;
            Some((sanitize_tool_name(name), name.to_string()))
        })
        .collect()
}

/// Parses one SSE data payload (`choices` / `usage` object) into a stream
/// chunk.
///
/// Returns `None` for chunks that carry no text delta, tool-call delta, or
/// usage (e.g. role-only start chunks). Tool-call indices are used as-is;
/// the host renumbers nothing, matching the OpenAI delta contract where
/// `index` identifies the tool call within the array.
pub(crate) fn process_sse_chunk(
    chunk: &Value,
    name_mapping: &HashMap<String, String>,
) -> Option<PluginStreamChunk> {
    let mut text_delta = None;
    let mut tool_calls_delta = None;
    // Usage arrives only on the final SSE chunk (and only when the provider
    // is asked for it via `stream_options.include_usage`).
    let usage = usage_from_json_value(chunk.get("usage"));

    if let Some(choices) = chunk.get("choices").and_then(Value::as_array)
        && let Some(choice) = choices.first()
        && let Some(delta) = choice.get("delta")
    {
        text_delta = text_from_delta(delta);

        if let Some(tc_deltas) = delta.get("tool_calls").and_then(Value::as_array) {
            let mut tc_list = Vec::new();
            for tc in tc_deltas {
                let index = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let id = tc.get("id").and_then(Value::as_str).map(str::to_string);
                let (name, arguments) = if let Some(func) = tc.get("function") {
                    (
                        func.get("name").and_then(Value::as_str).map(str::to_string),
                        func.get("arguments")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    )
                } else {
                    (None, None)
                };
                tc_list.push(json!({
                    "index": index,
                    "id": id,
                    "name": name.map(|n| name_mapping.get(&n).cloned().unwrap_or(n)),
                    "arguments": arguments,
                }));
            }
            tool_calls_delta = Some(tc_list);
        }
    }

    if text_delta.is_none() && tool_calls_delta.is_none() && usage.is_none() {
        return None;
    }
    Some(PluginStreamChunk {
        text_delta,
        tool_calls_delta,
        usage,
    })
}

/// Streaming delta text: `content` first, then `reasoning_content` (models
/// that fill `reasoning_content` instead of `content`).
fn text_from_delta(delta: &Value) -> Option<String> {
    if let Some(content) = delta.get("content").and_then(Value::as_str)
        && !content.is_empty()
    {
        return Some(content.to_string());
    }
    delta
        .get("reasoning_content")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// Non-streaming response text: `content` first, then `reasoning_content`.
pub(crate) fn text_from_message(message: &Value) -> String {
    if let Some(content) = message.get("content").and_then(Value::as_str)
        && !content.trim().is_empty()
    {
        return content.to_string();
    }
    message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map_or_else(String::new, str::to_string)
}

/// Extracts [`TokenUsage`] from a raw JSON `usage` object.
///
/// Returns `None` when the object is absent or carries no recognizable count,
/// so a provider that omits usage falls through to the caller's estimate.
pub(crate) fn usage_from_json_value(value: Option<&Value>) -> Option<TokenUsage> {
    let obj = value?;
    let prompt = obj.get("prompt_tokens").and_then(Value::as_u64);
    let completion = obj.get("completion_tokens").and_then(Value::as_u64);
    let total = obj.get("total_tokens").and_then(Value::as_u64);
    if prompt.is_none() && completion.is_none() && total.is_none() {
        return None;
    }
    let to_u32 = |v: Option<u64>| v.and_then(token_count_u32);
    Some(TokenUsage {
        prompt_tokens: to_u32(prompt),
        completion_tokens: to_u32(completion),
        total_tokens: to_u32(total),
    })
}

/// Narrow a provider-reported token count to `u32`.
///
/// Values above [`u32::MAX`] are treated as absent rather than silently
/// truncated — a bad provider sentinel must not enter usage accounting as a
/// plausible-looking number.
fn token_count_u32(n: u64) -> Option<u32> {
    if let Ok(v) = u32::try_from(n) {
        Some(v)
    } else {
        tracing::warn!(
            tokens = n,
            "token usage count exceeds u32::MAX; treating as absent"
        );
        None
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "unit tests use unwrap for concise assertions"
)]
mod tests {
    use super::*;

    fn user_text(text: &str) -> Value {
        json!({ "role": "user", "parts": [{"Text": {"text": text}}] })
    }

    #[test]
    fn single_text_part_becomes_plain_string_content() {
        let out = to_openai_messages(&[user_text("Hello")]).unwrap();
        assert_eq!(out[0]["content"], "Hello");
    }

    #[test]
    fn image_part_becomes_image_url_content() {
        let msg = json!({
            "role": "user",
            "parts": [{"Image": {"base64_image_data": "data:image/png;base64,AAAA"}}]
        });
        let out = to_openai_messages(&[msg]).unwrap();
        assert_eq!(out[0]["content"][0]["type"], "image_url");
        assert_eq!(
            out[0]["content"][0]["image_url"]["url"],
            "data:image/png;base64,AAAA"
        );
    }

    #[test]
    fn mixed_parts_become_content_array() {
        let msg = json!({
            "role": "user",
            "parts": [
                {"Text": {"text": "see"}},
                {"Image": {"base64_image_data": "data:image/png;base64,AAAA"}}
            ]
        });
        let out = to_openai_messages(&[msg]).unwrap();
        let content = out[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image_url");
    }

    #[test]
    fn system_and_tool_messages_round_trip() {
        let msgs = vec![
            json!({"role": "system", "content": "Be concise."}),
            user_text("Hi"),
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{"id": "call_1", "name": "fs.read", "arguments": "{}"}]
            }),
            json!({"role": "tool", "tool_call_id": "call_1", "content": "ok"}),
        ];
        let out = to_openai_messages(&msgs).unwrap();
        assert_eq!(out.len(), 4);
        assert_eq!(out[0]["role"], "system");
        assert_eq!(out[0]["content"], "Be concise.");
        assert_eq!(out[2]["tool_calls"][0]["function"]["name"], "fs_read");
        assert_eq!(out[3]["role"], "tool");
        assert_eq!(out[3]["tool_call_id"], "call_1");
    }

    #[test]
    fn unknown_role_is_rejected() {
        let msgs = vec![json!({"role": "narrator", "content": "x"})];
        assert!(to_openai_messages(&msgs).is_err());
    }

    #[test]
    fn sanitize_tool_name_replaces_invalid_chars() {
        assert_eq!(sanitize_tool_name("fs.read"), "fs_read");
        assert_eq!(sanitize_tool_name("my-tool_2"), "my-tool_2");
    }

    #[test]
    fn tools_convert_with_sanitized_names() {
        let tools = vec![json!({
            "name": "web.search",
            "description": "Search the web.",
            "parameters": {"type": "object"}
        })];
        let out = to_openai_tools(&tools);
        assert_eq!(out[0]["function"]["name"], "web_search");
        assert_eq!(out[0]["function"]["description"], "Search the web.");
        assert_eq!(out[0]["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn sse_text_delta() {
        let chunk = json!({
            "choices": [{"delta": {"content": "Hello"}}]
        });
        let out = process_sse_chunk(&chunk, &HashMap::new()).unwrap();
        assert_eq!(out.text_delta.as_deref(), Some("Hello"));
        assert!(out.tool_calls_delta.is_none());
        assert!(out.usage.is_none());
    }

    #[test]
    fn sse_reasoning_content_delta() {
        let chunk = json!({
            "choices": [{"delta": {"reasoning_content": "thinking..."}}]
        });
        let out = process_sse_chunk(&chunk, &HashMap::new()).unwrap();
        assert_eq!(out.text_delta.as_deref(), Some("thinking..."));
    }

    #[test]
    fn sse_tool_call_delta_restores_original_name() {
        let mut mapping = HashMap::new();
        mapping.insert("web_search".to_string(), "web.search".to_string());
        let chunk = json!({
            "choices": [{"delta": {"tool_calls": [
                {"index": 0, "id": "call_1", "function": {"name": "web_search", "arguments": "{\"q\""}}
            ]}}]
        });
        let out = process_sse_chunk(&chunk, &mapping).unwrap();
        let deltas = out.tool_calls_delta.unwrap();
        assert_eq!(deltas[0]["index"], 0);
        assert_eq!(deltas[0]["id"], "call_1");
        assert_eq!(deltas[0]["name"], "web.search");
        assert_eq!(deltas[0]["arguments"], "{\"q\"");
    }

    #[test]
    fn sse_usage_chunk() {
        let chunk = json!({
            "usage": {"prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30}
        });
        let out = process_sse_chunk(&chunk, &HashMap::new()).unwrap();
        let usage = out.usage.unwrap();
        assert_eq!(usage.prompt_tokens, Some(10));
        assert_eq!(usage.completion_tokens, Some(20));
        assert_eq!(usage.total_tokens, Some(30));
    }

    #[test]
    fn sse_role_only_chunk_is_skipped() {
        let chunk = json!({"choices": [{"delta": {"role": "assistant"}}]});
        assert!(process_sse_chunk(&chunk, &HashMap::new()).is_none());
    }

    #[test]
    fn usage_from_json_value_maps_counts() {
        let value = serde_json::json!({
            "prompt_tokens": 10u64,
            "completion_tokens": 20u64,
            "total_tokens": 30u64,
        });
        let usage = usage_from_json_value(Some(&value)).unwrap();
        assert_eq!(usage.prompt_tokens, Some(10));
        assert_eq!(usage.completion_tokens, Some(20));
        assert_eq!(usage.total_tokens, Some(30));
    }

    #[test]
    fn usage_from_json_value_drops_overflowing_counts() {
        let overflow = u64::from(u32::MAX) + 1;
        let value = serde_json::json!({
            "prompt_tokens": overflow,
            "completion_tokens": 20u64,
            "total_tokens": overflow,
        });
        let usage = usage_from_json_value(Some(&value)).unwrap();
        assert_eq!(usage.prompt_tokens, None);
        assert_eq!(usage.completion_tokens, Some(20));
        assert_eq!(usage.total_tokens, None);
    }

    #[test]
    fn usage_from_json_value_absent_is_none() {
        assert!(usage_from_json_value(None).is_none());
        assert!(usage_from_json_value(Some(&json!({"x": 1}))).is_none());
    }

    #[test]
    fn text_from_message_falls_back_to_reasoning_content() {
        let msg = json!({"content": null, "reasoning_content": "think"});
        assert_eq!(text_from_message(&msg), "think");
        let msg = json!({"content": "  ", "reasoning_content": "think"});
        assert_eq!(text_from_message(&msg), "think");
        let msg = json!({"content": "answer"});
        assert_eq!(text_from_message(&msg), "answer");
    }
}
