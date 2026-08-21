use std::sync::Arc;

use async_trait::async_trait;
use ene_kernel::{
    ConversationModel, KernelError, ModelGeneration, ModelRequest, TaskBinding, ToolCall,
};
use ene_plugin_ipc::{
    LlmGenerateRequest, LlmImage, LlmMessage, LlmRole, LlmToolCall, LlmToolSchema, ProviderAuth,
};
use ene_registry::Layer;
use ene_session::{Block, InnerAspect, ProjectedMessage, Role, SessionStore, SpillObject};

use crate::CoreDaemon;

/// Dialogue / job model that binds `ai.tasks.<task>` to a configured provider plugin.
pub struct SeamedModel {
    core: Arc<CoreDaemon>,
    task: &'static str,
}

impl SeamedModel {
    #[must_use]
    pub fn new(core: Arc<CoreDaemon>) -> Self {
        Self { core, task: "chat" }
    }

    #[must_use]
    pub fn job(core: Arc<CoreDaemon>) -> Self {
        Self { core, task: "job" }
    }

    fn binding(&self) -> TaskBinding {
        let guard = self.core.ai();
        let ai = guard.lock();
        let specific = match self.task {
            "job" => &ai.tasks.job,
            _ => &ai.tasks.chat,
        };
        if specific.is_unconfigured() {
            ai.tasks.chat.clone()
        } else {
            specific.clone()
        }
    }

    fn layer(&self) -> Layer {
        if self.task == "job" {
            Layer::Job
        } else {
            Layer::Surface
        }
    }

    fn fiber_task(&self) -> &'static str {
        if self.task != "job" {
            return "chat";
        }
        let guard = self.core.ai();
        let ai = guard.lock();
        if ai.tasks.job.is_unconfigured() {
            "chat"
        } else {
            "job"
        }
    }
}

#[async_trait]
impl ConversationModel for SeamedModel {
    async fn generate(&self, request: ModelRequest) -> Result<ModelGeneration, KernelError> {
        let binding = self.binding();
        if binding.is_unconfigured() {
            return Err(KernelError::Model(format!(
                "{} model is not configured",
                self.task
            )));
        }
        let llm_request = map_request(
            &request,
            &binding,
            &self.core.secret_for(self.task),
            &self.core,
            self.layer(),
        );
        let generation = self
            .core
            .supervisor()
            .generate_llm(
                &crate::plugin_profile::task_row_id(self.fiber_task()),
                llm_request,
            )
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
    layer: Layer,
) -> LlmGenerateRequest {
    let store = core.store();
    let vision_store = (!binding.is_unconfigured()).then_some(store.as_ref());
    LlmGenerateRequest {
        messages: fold_history(&request.messages, vision_store),
        tools: tool_schemas(core, layer),
        model: binding.model.clone(),
        max_tokens: binding.max_tokens,
        base_url: binding.base_url.clone(),
        auth: ProviderAuth {
            api_key: api_key.to_owned(),
        },
    }
}

// OpenAI/Anthropic function names must match ^[a-zA-Z0-9_-]+$.
// Host tools are namespaced with `.` (`utility.hash`); the vendor wire uses `__`.
fn to_vendor_tool_name(name: &str) -> String {
    name.replace('.', "__")
}

fn from_vendor_tool_name(name: &str) -> String {
    name.replace("__", ".")
}

fn tool_schemas(core: &CoreDaemon, layer: Layer) -> Vec<LlmToolSchema> {
    core.supervisor()
        .registry()
        .schemas(layer)
        .into_iter()
        .filter_map(|schema| {
            Some(LlmToolSchema {
                name: to_vendor_tool_name(schema.get("name")?.as_str()?),
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

fn llm_image_from_spill(obj: &SpillObject) -> Option<LlmImage> {
    let mime = obj
        .mime
        .clone()
        .unwrap_or_else(|| sniff_image_mime(&obj.bytes).to_owned());
    if !mime.starts_with("image/") {
        return None;
    }
    Some(LlmImage {
        mime,
        base64: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &obj.bytes),
    })
}

fn sniff_image_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        "application/octet-stream"
    }
}

fn fold_history(history: &[ProjectedMessage], store: Option<&SessionStore>) -> Vec<LlmMessage> {
    let mut messages = Vec::new();
    let mut pending_calls: Vec<LlmToolCall> = Vec::new();
    for item in history {
        match item.role {
            Role::Thinking | Role::Inner | Role::Status => {}
            Role::Tool if item.tool_args.is_some() => {
                pending_calls.push(LlmToolCall {
                    id: item
                        .tool_call_id
                        .clone()
                        .unwrap_or_else(|| format!("call_{}", item.seq)),
                    name: to_vendor_tool_name(item.tool_name.as_deref().unwrap_or_default()),
                    arguments: item.tool_args.clone().unwrap_or(serde_json::Value::Null),
                });
            }
            Role::Tool => {
                flush_calls(&mut messages, &mut pending_calls);
                let (text, images) = fold_tool_content(item, store);
                messages.push(LlmMessage {
                    role: LlmRole::Tool,
                    text,
                    tool_calls: Vec::new(),
                    tool_call_id: item.tool_call_id.clone(),
                    tool_name: item.tool_name.as_deref().map(to_vendor_tool_name),
                    images,
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

fn fold_tool_content(
    item: &ProjectedMessage,
    store: Option<&SessionStore>,
) -> (String, Vec<LlmImage>) {
    let mut images = Vec::new();
    let mut omitted = false;
    for block in &item.blocks {
        let Block::ImageRef { artifact_id } = block else {
            continue;
        };
        if let Some(image) = store
            .and_then(|store| store.get_spill(artifact_id).ok().flatten())
            .and_then(|obj| llm_image_from_spill(&obj))
        {
            images.push(image);
        } else {
            omitted = true;
        }
    }
    let text = item.text();
    let text = if omitted && text.is_empty() {
        "[image omitted]".to_owned()
    } else if omitted {
        format!("{text}\n[image omitted]")
    } else {
        text
    };
    (text, images)
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
                name: from_vendor_tool_name(&call.name),
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
    use base64::Engine;
    use ene_plugin_ipc::LlmRole;
    use ene_session::{Block, ProjectedMessage, Role, SessionStore};

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
        let mapped = fold_history(&history, None);
        assert_eq!(mapped.len(), 3);
        assert_eq!(mapped[0].role, LlmRole::User);
        assert_eq!(mapped[1].role, LlmRole::Assistant);
        assert_eq!(mapped[1].tool_calls.len(), 1);
        assert_eq!(mapped[1].tool_calls[0].name, "fs__read");
        assert_eq!(mapped[2].role, LlmRole::Tool);
        assert_eq!(mapped[2].tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(mapped[2].text, "file body");
    }

    #[test]
    fn vendor_tool_name_roundtrip() {
        assert_eq!(to_vendor_tool_name("utility.hash"), "utility__hash");
        assert_eq!(
            to_vendor_tool_name("app.active_window"),
            "app__active_window"
        );
        assert_eq!(from_vendor_tool_name("utility__hash"), "utility.hash");
        assert_eq!(
            from_vendor_tool_name("app__active_window"),
            "app.active_window"
        );
        assert_eq!(
            from_vendor_tool_name(&to_vendor_tool_name("fs.read")),
            "fs.read"
        );
    }

    #[test]
    fn map_generation_decodes_vendor_tool_name() {
        let generation = ene_plugin_ipc::LlmGeneration {
            text: String::new(),
            thinking: None,
            inner: Vec::new(),
            tool_calls: vec![ene_plugin_ipc::LlmToolCall {
                id: "c1".to_owned(),
                name: "utility__calc".to_owned(),
                arguments: serde_json::json!({"expr": "1+1"}),
            }],
            finish_reason: "tool_calls".to_owned(),
            model_id: "test".to_owned(),
            input_tokens: 0,
            output_tokens: 0,
        };
        let mapped = map_generation(generation);
        assert_eq!(mapped.tool_calls[0].name, "utility.calc");
    }

    #[tokio::test]
    async fn fold_history_attaches_llm_image_from_image_ref() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = SessionStore::open(dir.path().join("sessions.db"), "NORMAL")
            .await
            .unwrap();
        let png = [0x89_u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let encoded = base64::engine::general_purpose::STANDARD.encode(png);
        let artifact_id = store.put_spill(&png, Some("image/png")).await.unwrap();
        let history = vec![ProjectedMessage {
            seq: 3,
            role: Role::Tool,
            blocks: vec![
                Block::image_ref(artifact_id),
                Block::text(r#"{"width":1,"height":1}"#),
            ],
            turn_id: None,
            step_index: None,
            tool_name: None,
            tool_args: None,
            tool_call_id: Some("call-1".to_owned()),
        }];
        let mapped = fold_history(&history, Some(&store));
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].images.len(), 1);
        assert_eq!(mapped[0].images[0].mime, "image/png");
        assert_eq!(mapped[0].images[0].base64, encoded);
        assert_eq!(mapped[0].text, r#"{"width":1,"height":1}"#);
    }

    #[test]
    fn fold_messages_omits_images_when_provider_has_no_vision() {
        let history = vec![ProjectedMessage {
            seq: 3,
            role: Role::Tool,
            blocks: vec![Block::image_ref("deadbeef"), Block::text(r#"{"width":1}"#)],
            turn_id: None,
            step_index: None,
            tool_name: None,
            tool_args: None,
            tool_call_id: Some("call-1".to_owned()),
        }];
        let mapped = fold_history(&history, None);
        assert!(mapped[0].images.is_empty());
        assert!(mapped[0].text.contains("[image omitted]"));
        assert!(mapped[0].text.contains(r#""width":1"#));
    }
}
