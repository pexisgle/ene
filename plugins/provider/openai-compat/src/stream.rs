use ene_plugin_ipc::{IpcError, LlmGeneration, LlmStreamSink, LlmToolCall};
use serde_json::Value;

pub(crate) struct SseBuf {
    pending: String,
}

pub(crate) enum SseEvent {
    Data(Value),
    Done,
}

impl SseBuf {
    pub(crate) fn new() -> Self {
        Self {
            pending: String::new(),
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.pending.push_str(&String::from_utf8_lossy(bytes));
        let mut events = Vec::new();
        loop {
            let normalized = self.pending.replace('\r', "");
            let Some(idx) = normalized.find("\n\n") else {
                self.pending = normalized;
                break;
            };
            let block = normalized[..idx].to_owned();
            self.pending = normalized[idx + 2..].to_owned();
            events.extend(parse_block(&block));
        }
        events
    }

    pub(crate) fn finish(&mut self) -> Vec<SseEvent> {
        let rest = std::mem::take(&mut self.pending);
        parse_block(&rest)
    }
}

fn parse_block(block: &str) -> Vec<SseEvent> {
    let mut data_lines = Vec::new();
    for line in block.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim_start());
        }
    }
    if data_lines.is_empty() {
        return Vec::new();
    }
    let data = data_lines.join("\n");
    if data.trim() == "[DONE]" {
        return vec![SseEvent::Done];
    }
    match serde_json::from_str::<Value>(&data) {
        Ok(value) => vec![SseEvent::Data(value)],
        Err(_) => Vec::new(),
    }
}

#[derive(Default)]
struct ToolCallAcc {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
pub(crate) struct ChatStreamState {
    text: String,
    thinking: String,
    tool_calls: Vec<ToolCallAcc>,
    finish_reason: String,
    model_id: String,
    input_tokens: u32,
    output_tokens: u32,
}

impl ChatStreamState {
    pub(crate) async fn apply(
        &mut self,
        value: &Value,
        sink: &mut dyn LlmStreamSink,
    ) -> Result<(), IpcError> {
        if let Some(model) = value.get("model").and_then(Value::as_str)
            && !model.is_empty()
        {
            model.clone_into(&mut self.model_id);
        }
        if let Some(usage) = value.get("usage") {
            self.input_tokens = usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .map_or(self.input_tokens, u64_to_u32);
            self.output_tokens = usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .map_or(self.output_tokens, u64_to_u32);
        }
        let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return Ok(());
        };
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str)
            && !reason.is_empty()
            && reason != "null"
        {
            reason.clone_into(&mut self.finish_reason);
        }
        let delta = choice.get("delta").cloned().unwrap_or(Value::Null);
        let text = delta_text(&delta);
        if !text.is_empty() {
            self.text.push_str(&text);
            sink.on_text(&text).await?;
        }
        let thinking = delta_thinking(&delta);
        if !thinking.is_empty() {
            self.thinking.push_str(&thinking);
            sink.on_thinking(&thinking).await?;
        }
        apply_tool_call_delta(&mut self.tool_calls, &delta);
        Ok(())
    }

    pub(crate) fn into_generation(self) -> LlmGeneration {
        let thinking = if self.thinking.is_empty() {
            None
        } else {
            Some(self.thinking)
        };
        let finish_reason = if self.finish_reason.is_empty() {
            if self.tool_calls.iter().any(|call| !call.name.is_empty()) {
                "tool_calls".to_owned()
            } else {
                "stop".to_owned()
            }
        } else {
            self.finish_reason
        };
        LlmGeneration {
            text: self.text,
            thinking,
            inner: Vec::new(),
            tool_calls: self
                .tool_calls
                .into_iter()
                .filter(|call| !call.name.is_empty())
                .map(|call| LlmToolCall {
                    id: call.id,
                    name: call.name,
                    arguments: serde_json::from_str(&call.arguments).unwrap_or(Value::Null),
                })
                .collect(),
            finish_reason,
            model_id: self.model_id,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
        }
    }
}

pub(crate) async fn consume_chat_sse(
    mut response: reqwest::Response,
    sink: &mut dyn LlmStreamSink,
) -> Result<LlmGeneration, IpcError> {
    let mut parser = SseBuf::new();
    let mut state = ChatStreamState::default();
    loop {
        let chunk = response
            .chunk()
            .await
            .map_err(|err| IpcError::Call(err.to_string()))?;
        let Some(bytes) = chunk else {
            break;
        };
        if apply_events(&mut state, sink, parser.push(&bytes)).await? {
            break;
        }
    }
    drop(apply_events(&mut state, sink, parser.finish()).await);
    Ok(state.into_generation())
}

async fn apply_events(
    state: &mut ChatStreamState,
    sink: &mut dyn LlmStreamSink,
    events: Vec<SseEvent>,
) -> Result<bool, IpcError> {
    for event in events {
        match event {
            SseEvent::Done => return Ok(true),
            SseEvent::Data(value) => state.apply(&value, sink).await?,
        }
    }
    Ok(false)
}

fn delta_text(delta: &Value) -> String {
    match delta.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn delta_thinking(delta: &Value) -> String {
    delta
        .get("reasoning")
        .or_else(|| delta.get("reasoning_content"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn apply_tool_call_delta(acc: &mut Vec<ToolCallAcc>, delta: &Value) {
    let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) else {
        return;
    };
    for call in calls {
        let index = call.get("index").and_then(Value::as_u64).unwrap_or(0);
        let Ok(index) = usize::try_from(index) else {
            continue;
        };
        while acc.len() <= index {
            acc.push(ToolCallAcc::default());
        }
        let slot = &mut acc[index];
        if let Some(id) = call.get("id").and_then(Value::as_str) {
            slot.id.push_str(id);
        }
        if let Some(function) = call.get("function") {
            if let Some(name) = function.get("name").and_then(Value::as_str) {
                slot.name.push_str(name);
            }
            if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                slot.arguments.push_str(arguments);
            }
        }
    }
}

fn u64_to_u32(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;

    struct CollectingSink {
        text: Vec<String>,
        thinking: Vec<String>,
    }

    #[async_trait]
    impl LlmStreamSink for CollectingSink {
        async fn on_text(&mut self, text: &str) -> Result<(), IpcError> {
            self.text.push(text.to_owned());
            Ok(())
        }

        async fn on_thinking(&mut self, text: &str) -> Result<(), IpcError> {
            self.thinking.push(text.to_owned());
            Ok(())
        }
    }

    #[test]
    fn parses_split_sse_frames() {
        let mut buf = SseBuf::new();
        let first = buf.push(b"data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n");
        assert_eq!(first.len(), 1);
        let second = buf
            .push(b"data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\ndata: [DONE]\n\n");
        assert_eq!(second.len(), 2);
        assert!(matches!(second[1], SseEvent::Done));
    }

    #[tokio::test]
    async fn accumulates_content_and_tool_calls() {
        let mut sink = CollectingSink {
            text: Vec::new(),
            thinking: Vec::new(),
        };
        let mut state = ChatStreamState::default();
        state
            .apply(
                &json!({
                    "model": "gpt-test",
                    "choices": [{
                        "delta": { "content": "hel" }
                    }]
                }),
                &mut sink,
            )
            .await
            .unwrap();
        state
            .apply(
                &json!({
                    "choices": [{
                        "delta": { "content": "lo" },
                        "finish_reason": "stop"
                    }],
                    "usage": { "prompt_tokens": 3, "completion_tokens": 2 }
                }),
                &mut sink,
            )
            .await
            .unwrap();
        let generation = state.into_generation();
        assert_eq!(sink.text, ["hel", "lo"]);
        assert_eq!(generation.text, "hello");
        assert_eq!(generation.model_id, "gpt-test");
        assert_eq!(generation.input_tokens, 3);
        assert_eq!(generation.finish_reason, "stop");
    }

    #[tokio::test]
    async fn tool_call_deltas_are_not_speech() {
        let mut sink = CollectingSink {
            text: Vec::new(),
            thinking: Vec::new(),
        };
        let mut state = ChatStreamState::default();
        state
            .apply(
                &json!({
                    "choices": [{
                        "delta": {
                            "tool_calls": [{
                                "index": 0,
                                "id": "c1",
                                "function": { "name": "utility__calc", "arguments": "" }
                            }]
                        }
                    }]
                }),
                &mut sink,
            )
            .await
            .unwrap();
        state
            .apply(
                &json!({
                    "choices": [{
                        "delta": {
                            "tool_calls": [{
                                "index": 0,
                                "function": { "arguments": "{\"expr\":\"1+1\"}" }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
                &mut sink,
            )
            .await
            .unwrap();
        let generation = state.into_generation();
        assert!(sink.text.is_empty());
        assert_eq!(generation.tool_calls.len(), 1);
        assert_eq!(generation.tool_calls[0].name, "utility__calc");
        assert_eq!(generation.tool_calls[0].arguments["expr"], "1+1");
        assert_eq!(generation.finish_reason, "tool_calls");
    }
}
