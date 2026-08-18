use std::sync::Arc;

use async_trait::async_trait;
use ene_kernel::{
    ConversationModel, EchoModel, KernelError, ModelGeneration, ModelRequest, TaskBinding, ToolCall,
};
use ene_plugin_ipc::{
    LlmGenerateRequest, LlmMessage, LlmRole, LlmToolCall, LlmToolSchema, ProviderAuth,
};
use ene_registry::Layer;
use ene_session::{InnerAspect, ProjectedMessage, Role};

use crate::CoreDaemon;

/// Dialogue model that binds `ai.tasks.chat` to a provider plugin or Echo.
pub struct SeamedModel {
    core: Arc<CoreDaemon>,
    echo: EchoModel,
}

impl SeamedModel {
    #[must_use]
    pub fn new(core: Arc<CoreDaemon>) -> Self {
        Self {
            core,
            echo: EchoModel,
        }
    }

    fn chat_binding(&self) -> TaskBinding {
        self.core.ai().lock().tasks.chat.clone()
    }
}

#[async_trait]
impl ConversationModel for SeamedModel {
    async fn generate(&self, request: ModelRequest) -> Result<ModelGeneration, KernelError> {
        let binding = self.chat_binding();
        if binding.uses_echo() {
            return self.echo.generate(request).await;
        }
        let llm_request = map_request(
            &request,
            &binding,
            &self.core.secret_for("chat"),
            &self.core,
        );
        let generation = self
            .core
            .supervisor()
            .generate_llm(&binding.plugin, llm_request)
            .await
            .map_err(|err| KernelError::Model(err.to_string()))?;
        if generation.finish_reason == "error" {
            return Err(KernelError::Model(if generation.model_id.is_empty() {
                "provider failed".to_owned()
            } else {
                generation.model_id
            }));
        }
        Ok(map_generation(generation))
    }
}

fn map_request(
    request: &ModelRequest,
    binding: &TaskBinding,
    api_key: &str,
    core: &CoreDaemon,
) -> LlmGenerateRequest {
    LlmGenerateRequest {
        messages: fold_messages(&request.messages),
        tools: tool_schemas(core),
        model: binding.model.clone(),
        max_tokens: binding.max_tokens,
        base_url: binding.base_url.clone(),
        auth: ProviderAuth {
            api_key: api_key.to_owned(),
        },
    }
}

fn tool_schemas(core: &CoreDaemon) -> Vec<LlmToolSchema> {
    core.supervisor()
        .registry()
        .schemas(Layer::Surface)
        .into_iter()
        .filter_map(|schema| {
            Some(LlmToolSchema {
                name: schema.get("name")?.as_str()?.to_owned(),
                description: schema
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                parameters: schema
                    .get("parameters")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({"type": "object"})),
            })
        })
        .collect()
}

fn fold_messages(history: &[ProjectedMessage]) -> Vec<LlmMessage> {
    let mut messages = Vec::new();
    let mut pending_calls: Vec<LlmToolCall> = Vec::new();
    for item in history {
        match item.role {
            Role::Thinking | Role::Inner => {}
            Role::Tool if item.tool_args.is_some() => {
                pending_calls.push(LlmToolCall {
                    id: item
                        .tool_call_id
                        .clone()
                        .unwrap_or_else(|| format!("call_{}", item.seq)),
                    name: item.tool_name.clone().unwrap_or_default(),
                    arguments: item.tool_args.clone().unwrap_or(serde_json::Value::Null),
                });
            }
            Role::Tool => {
                flush_calls(&mut messages, &mut pending_calls);
                messages.push(LlmMessage {
                    role: LlmRole::Tool,
                    text: item.text(),
                    tool_calls: Vec::new(),
                    tool_call_id: item.tool_call_id.clone(),
                    tool_name: item.tool_name.clone(),
                    images: Vec::new(),
                });
            }
            Role::User => {
                flush_calls(&mut messages, &mut pending_calls);
                messages.push(plain(LlmRole::User, item));
            }
            Role::Assistant => {
                flush_calls(&mut messages, &mut pending_calls);
                messages.push(plain(LlmRole::Assistant, item));
            }
            Role::System => {
                flush_calls(&mut messages, &mut pending_calls);
                messages.push(plain(LlmRole::System, item));
            }
        }
    }
    flush_calls(&mut messages, &mut pending_calls);
    messages
}

fn plain(role: LlmRole, item: &ProjectedMessage) -> LlmMessage {
    LlmMessage {
        role,
        text: item.text(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        tool_name: None,
        images: Vec::new(),
    }
}

fn flush_calls(messages: &mut Vec<LlmMessage>, pending: &mut Vec<LlmToolCall>) {
    if pending.is_empty() {
        return;
    }
    messages.push(LlmMessage {
        role: LlmRole::Assistant,
        text: String::new(),
        tool_calls: std::mem::take(pending),
        tool_call_id: None,
        tool_name: None,
        images: Vec::new(),
    });
}

fn map_generation(generation: ene_plugin_ipc::LlmGeneration) -> ModelGeneration {
    ModelGeneration {
        text: generation.text,
        thinking: generation.thinking,
        inner: generation
            .inner
            .into_iter()
            .map(|line| (parse_aspect(&line.aspect), line.text))
            .collect(),
        tool_calls: generation
            .tool_calls
            .into_iter()
            .map(|call| ToolCall {
                name: call.name,
                arguments: call.arguments,
            })
            .collect(),
        finish_reason: generation.finish_reason,
        model_id: generation.model_id,
        input_tokens: generation.input_tokens,
        output_tokens: generation.output_tokens,
    }
}

fn parse_aspect(aspect: &str) -> InnerAspect {
    match aspect {
        "emotion" => InnerAspect::Emotion,
        "action_intent" => InnerAspect::ActionIntent,
        _ => InnerAspect::Thought,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_session::{Block, ProjectedMessage, Role};

    fn msg(
        seq: u64,
        role: Role,
        text: &str,
        tool_name: Option<&str>,
        tool_args: Option<serde_json::Value>,
        tool_call_id: Option<&str>,
    ) -> ProjectedMessage {
        ProjectedMessage {
            seq,
            role,
            blocks: vec![Block::text(text)],
            turn_id: None,
            step_index: None,
            tool_name: tool_name.map(ToOwned::to_owned),
            tool_args,
            tool_call_id: tool_call_id.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn folds_tool_call_then_result() {
        let history = vec![
            msg(1, Role::User, "run it", None, None, None),
            msg(
                2,
                Role::Tool,
                "fs.read {}",
                Some("fs.read"),
                Some(serde_json::json!({"path": "a.txt"})),
                Some("call-1"),
            ),
            msg(3, Role::Tool, "file body", None, None, Some("call-1")),
        ];
        let mapped = fold_messages(&history);
        assert_eq!(mapped.len(), 3);
        assert_eq!(mapped[0].role, LlmRole::User);
        assert_eq!(mapped[1].role, LlmRole::Assistant);
        assert_eq!(mapped[1].tool_calls.len(), 1);
        assert_eq!(mapped[1].tool_calls[0].name, "fs.read");
        assert_eq!(mapped[2].role, LlmRole::Tool);
        assert_eq!(mapped[2].tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(mapped[2].text, "file body");
    }
}
