use std::sync::Arc;

use async_trait::async_trait;
use ene_kernel::{
    ConversationModel, KernelError, ModelGeneration, ModelRequest, TaskBinding, TextDeltaSink,
    TokenEstimation, ToolCall, effective_window, estimate_tokens, fit_prompt,
};
use ene_plugin_ipc::{
    LlmGenerateRequest, LlmImage, LlmMessage, LlmRole, LlmToolCall, LlmToolSchema, ProviderAuth,
};
use ene_registry::Layer;
use ene_session::{Block, InnerAspect, ProjectedMessage, Role, SessionStore, SpillObject};

use crate::CoreDaemon;

const IMAGE_TOKEN_ESTIMATE: u32 = 4096;

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
        self.generate_on_fiber(request, None).await
    }

    async fn generate_streaming(
        &self,
        request: ModelRequest,
        sink: &mut dyn TextDeltaSink,
    ) -> Result<ModelGeneration, KernelError> {
        self.generate_on_fiber(request, Some(sink)).await
    }
}

impl SeamedModel {
    async fn generate_on_fiber(
        &self,
        request: ModelRequest,
        sink: Option<&mut dyn TextDeltaSink>,
    ) -> Result<ModelGeneration, KernelError> {
        let binding = self.binding();
        if binding.is_unconfigured() {
            return Err(KernelError::Model(format!(
                "{} model is not configured",
                self.task
            )));
        }
        let harness = crate::boot::load_harness_settings(self.core.data_dir());
        let reserve = binding
            .max_tokens
            .unwrap_or(harness.context.response_reserve_tokens);
        let window = effective_window(
            None,
            binding.context_window,
            Some(reserve),
            harness.context.safety_margin_ratio,
        );
        let estimation = harness.context.token_estimation;
        let store = self.core.store();
        let vision_store = vision_store_for(&binding, store.as_ref());
        let tool_overhead = estimate_tool_schema_tokens(&self.core, self.layer(), estimation);
        let message_budget = window.available.saturating_sub(tool_overhead);
        if message_budget == 0 {
            return Err(KernelError::Model(
                "context window exhausted by tool definitions".to_owned(),
            ));
        }
        let messages = fit_prompt(
            fold_history(&request.messages, vision_store),
            message_budget,
            |message| estimate_message_tokens(message, estimation),
            |message| matches!(message.role, LlmRole::System),
        );
        let llm_request = map_request(
            messages,
            &binding,
            &self.core.secret_for(self.task),
            &self.core,
            self.layer(),
            reserve,
        );
        let row_id = crate::plugin_profile::task_row_id(self.fiber_task());
        let generation = if let Some(sink) = sink {
            super::llm::generate_llm_streaming(&self.core, &row_id, llm_request, sink).await
        } else {
            super::llm::generate_llm(&self.core, &row_id, llm_request).await
        }
        .map_err(KernelError::Model)?;
        Ok(map_generation(generation))
    }
}
fn vision_store_for<'a>(
    binding: &TaskBinding,
    store: &'a SessionStore,
) -> Option<&'a SessionStore> {
    binding.accepts_images().then_some(store)
}

fn map_request(
    messages: Vec<LlmMessage>,
    binding: &TaskBinding,
    api_key: &str,
    core: &CoreDaemon,
    layer: Layer,
    reserve: u32,
) -> LlmGenerateRequest {
    LlmGenerateRequest {
        messages,
        tools: tool_schemas(core, layer),
        model: binding.model.clone(),
        max_tokens: Some(reserve),
        base_url: binding.base_url.clone(),
        auth: ProviderAuth {
            api_key: api_key.to_owned(),
        },
    }
}

fn pack_text(message: &LlmMessage) -> String {
    let mut text = message.text.clone();
    for call in &message.tool_calls {
        text.push(' ');
        text.push_str(&call.name);
        text.push(' ');
        text.push_str(&call.arguments.to_string());
    }
    text
}

fn estimate_message_tokens(message: &LlmMessage, estimation: TokenEstimation) -> u32 {
    let text = estimate_tokens(&pack_text(message), estimation);
    let image_count = u32::try_from(message.images.len()).unwrap_or(u32::MAX);
    text.saturating_add(IMAGE_TOKEN_ESTIMATE.saturating_mul(image_count))
}

fn estimate_tool_schema_tokens(
    core: &CoreDaemon,
    layer: Layer,
    estimation: TokenEstimation,
) -> u32 {
    estimate_tool_schema_tokens_for(&tool_schemas(core, layer), estimation)
}

fn estimate_tool_schema_tokens_for(schemas: &[LlmToolSchema], estimation: TokenEstimation) -> u32 {
    if schemas.is_empty() {
        return 0;
    }
    let Ok(serialized) = serde_json::to_string(schemas) else {
        return u32::MAX;
    };
    estimate_tokens(&serialized, estimation)
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
    use ene_kernel::TaskBinding;
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

    fn configured(plugin: &str, supports_images: bool) -> TaskBinding {
        TaskBinding {
            plugin: plugin.to_owned(),
            model: "test-model".to_owned(),
            supports_images,
            ..TaskBinding::default()
        }
    }

    fn png_tool_history(artifact_id: impl Into<String>) -> Vec<ProjectedMessage> {
        vec![ProjectedMessage {
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
        }]
    }

    #[tokio::test]
    async fn configured_text_only_binding_omits_image_ref() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = SessionStore::open(dir.path().join("sessions.db"), "NORMAL")
            .await
            .unwrap();
        let png = [0x89_u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let artifact_id = store.put_spill(&png, Some("image/png")).await.unwrap();
        let binding = configured("provider.openai", false);
        assert!(!binding.is_unconfigured());
        let mapped = fold_history(
            &png_tool_history(artifact_id),
            vision_store_for(&binding, &store),
        );
        assert!(mapped[0].images.is_empty());
        assert!(mapped[0].text.contains("[image omitted]"));
        assert!(mapped[0].text.contains(r#""width":1"#));
    }

    #[tokio::test]
    async fn configured_vision_binding_attaches_llm_image() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = SessionStore::open(dir.path().join("sessions.db"), "NORMAL")
            .await
            .unwrap();
        let png = [0x89_u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let encoded = base64::engine::general_purpose::STANDARD.encode(png);
        let artifact_id = store.put_spill(&png, Some("image/png")).await.unwrap();
        let binding = configured("provider.openai", true);
        let mapped = fold_history(
            &png_tool_history(artifact_id),
            vision_store_for(&binding, &store),
        );
        assert_eq!(mapped[0].images.len(), 1);
        assert_eq!(mapped[0].images[0].mime, "image/png");
        assert_eq!(mapped[0].images[0].base64, encoded);
        assert!(!mapped[0].text.contains("[image omitted]"));
    }

    #[test]
    fn pack_text_includes_tool_call_payload() {
        let message = LlmMessage {
            role: LlmRole::Assistant,
            text: String::new(),
            tool_calls: vec![LlmToolCall {
                id: "c1".to_owned(),
                name: "fs__read".to_owned(),
                arguments: serde_json::json!({"path": "a.txt"}),
            }],
            tool_call_id: None,
            tool_name: None,
            images: Vec::new(),
        };
        let packed = pack_text(&message);
        assert!(packed.contains("fs__read"));
        assert!(packed.contains("a.txt"));
    }

    #[test]
    fn image_payloads_consume_context_budget() {
        let plain = LlmMessage {
            role: LlmRole::User,
            text: "look".to_owned(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
            images: Vec::new(),
        };
        let with_image = LlmMessage {
            role: LlmRole::User,
            text: "look".to_owned(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
            images: vec![LlmImage {
                mime: "image/png".to_owned(),
                base64: "iVBORw0KGgo=".to_owned(),
            }],
        };
        let plain_cost = estimate_message_tokens(&plain, ene_kernel::TokenEstimation::Chars4);
        let image_cost = estimate_message_tokens(&with_image, ene_kernel::TokenEstimation::Chars4);
        assert_eq!(image_cost, plain_cost.saturating_add(IMAGE_TOKEN_ESTIMATE));
    }

    #[test]
    fn tool_schema_overhead_can_exhaust_available_window() {
        let schemas = vec![LlmToolSchema {
            name: "big".to_owned(),
            description: "d".repeat(10_000),
            parameters: serde_json::json!({"type": "object"}),
        }];
        let overhead =
            estimate_tool_schema_tokens_for(&schemas, ene_kernel::TokenEstimation::Chars4);
        assert!(overhead > 100);
        let messages = vec![LlmMessage::new(LlmRole::User, "short")];
        let msg_cost = estimate_message_tokens(&messages[0], ene_kernel::TokenEstimation::Chars4);
        let packed = ene_kernel::fit_prompt(
            messages,
            msg_cost,
            |message| estimate_message_tokens(message, ene_kernel::TokenEstimation::Chars4),
            |_| false,
        );
        assert_eq!(packed.len(), 1);
        let packed_with_overhead = ene_kernel::fit_prompt(
            packed,
            msg_cost.saturating_sub(overhead),
            |message| estimate_message_tokens(message, ene_kernel::TokenEstimation::Chars4),
            |_| false,
        );
        assert!(packed_with_overhead.is_empty() || overhead >= msg_cost);
    }

    #[test]
    fn token_budget_keeps_system_and_latest_user() {
        let messages = vec![
            LlmMessage::new(LlmRole::System, "contract"),
            LlmMessage::new(LlmRole::User, "old-turn-".repeat(80)),
            LlmMessage::new(LlmRole::Assistant, "ack-old"),
            LlmMessage::new(LlmRole::User, "latest-turn"),
        ];
        let packed = ene_kernel::fit_prompt(
            messages,
            8,
            |message| estimate_message_tokens(message, ene_kernel::TokenEstimation::Chars4),
            |message| matches!(message.role, LlmRole::System),
        );
        assert_eq!(packed[0].text, "contract");
        assert_eq!(
            packed.last().map(|message| message.text.as_str()),
            Some("latest-turn")
        );
        assert!(
            packed
                .iter()
                .all(|message| !message.text.contains("old-turn-")),
            "oversized older turn should be dropped: {packed:?}"
        );
    }
}
