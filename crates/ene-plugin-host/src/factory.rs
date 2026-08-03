//! LLM provider factory backed by a plugin IPC connection.
//!
//! [`IpcLlmProviderFactory`] implements [`ene_ai::LlmProviderFactory`] so
//! that plugin-provided LLM providers integrate with the global
//! [`LlmProviderRegistry`](ene_ai::LlmProviderRegistry).
//!
//! ## Credentials
//!
//! Provider definitions are forwarded to plugins **without** any `api_key`:
//! secrets resolve exclusively through the host's `credential` passenger, so
//! they never travel over provider IPC config or plugin process env.

use std::sync::Arc;

use ene_ai::error::LlmProviderError;
use ene_ai::traits::{LlmProvider, LlmProviderFactory};
use ene_ai::{AiProviderDef, TaskRef};
use ene_plugin_proto::ConcurrencyHint;

use crate::ipc_plugin::IpcPluginConnection;
use crate::ipc_provider::{ConcurrencyLimiter, IpcLlmProvider};

/// Factory that creates [`IpcLlmProvider`] instances for a specific
/// provider kind served by a plugin binary.
pub struct IpcLlmProviderFactory {
    kind: String,
    conn: Arc<IpcPluginConnection>,
    /// Context window the plugin advertised for this provider kind
    /// (`LlmProviderSpec.context_window`), forwarded to every
    /// [`IpcLlmProvider`] this factory creates so prompt packing can budget
    /// against the model's real limit.
    context_window: Option<u32>,
    /// Shared across every [`IpcLlmProvider`] this factory creates, since a
    /// fresh provider instance is built per call
    /// (`create_task_chat_provider`) — see `ipc_provider`'s module docs for
    /// why the limiter must outlive any single provider instance.
    limiter: Arc<ConcurrencyLimiter>,
}

impl IpcLlmProviderFactory {
    /// Creates a new factory for the given provider kind, sharing the
    /// plugin connection.
    ///
    /// `concurrency` is the [`ConcurrencyHint`] the plugin declared for this
    /// provider kind during the handshake (or the safe serial default if it
    /// declared none); it is built into a single [`ConcurrencyLimiter`]
    /// shared by every provider instance this factory subsequently creates.
    pub fn new(
        kind: String,
        conn: Arc<IpcPluginConnection>,
        context_window: Option<u32>,
        concurrency: ConcurrencyHint,
    ) -> Self {
        Self {
            kind,
            conn,
            context_window,
            limiter: Arc::new(ConcurrencyLimiter::new(concurrency)),
        }
    }
}

impl LlmProviderFactory for IpcLlmProviderFactory {
    fn provider_name(&self) -> &str {
        &self.kind
    }

    fn create_provider(
        &self,
        config: &ene_config::EneConfig,
        task: &TaskRef,
    ) -> Result<Box<dyn LlmProvider>, LlmProviderError> {
        let ai_config = config.get_section::<ene_ai::AiConfig>().unwrap_or_default();

        // Honor the active cognitive task's own model / max_tokens overrides
        // rather than always falling back to `tasks.chat`. When the task
        // carries no override, fall back to the chat defaults.
        let model = task
            .model
            .clone()
            .or_else(|| ai_config.tasks.chat.model.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let max_tokens = task.max_tokens.or(ai_config.tasks.chat.max_tokens);

        let provider_config = ai_config
            .providers
            .values()
            .find(|def| def.kind == self.kind)
            .map_or_else(|| serde_json::json!({}), build_provider_config);

        // Apply the same retry policy as the OpenAI path so plugin providers
        // retry transient (transport / rate-limit) failures consistently.
        let retry_policy = ai_config.retry.to_policy();

        let provider = IpcLlmProvider::new(
            self.kind.clone(),
            Arc::clone(&self.conn),
            model,
            max_tokens,
            provider_config,
            retry_policy,
            self.context_window,
            Arc::clone(&self.limiter),
        );

        Ok(Box::new(provider))
    }
}

/// Builds the `provider_config` JSON forwarded to a plugin LLM provider.
///
/// Never includes an `api_key` field: secrets resolve exclusively through the
/// host's `credential` passenger, so they must not travel over provider IPC
/// config. `base_url` and `extra` are not secrets and are always forwarded.
fn build_provider_config(def: &AiProviderDef) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    if !def.base_url.is_empty() {
        map.insert(
            "base_url".to_string(),
            serde_json::Value::String(def.base_url.clone()),
        );
    }
    for (k, v) in &def.extra {
        map.insert(k.clone(), v.clone());
    }
    serde_json::Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_ai::ApiKeyConfig;

    fn anthropic_def_with_inline_key() -> AiProviderDef {
        AiProviderDef {
            kind: "anthropic".to_string(),
            base_url: String::new(),
            api_key: ApiKeyConfig {
                source: "inline".to_string(),
                inline: "sk-test-123".to_string(),
                env: String::new(),
            },
            context_window: None,
            extra: serde_json::Map::new(),
        }
    }

    /// Contract: the provider config never carries an `api_key` field, even
    /// when the provider definition has a resolvable key — secrets travel
    /// only through the host's credential service.
    #[test]
    fn provider_config_never_emits_api_key() {
        let def = anthropic_def_with_inline_key();
        let config = build_provider_config(&def);

        assert!(
            config.get("api_key").is_none(),
            "api_key must never be emitted, got {config:?}"
        );
    }

    /// `base_url` and `extra` fields are forwarded since they are not
    /// secrets.
    #[test]
    fn non_secret_fields_forwarded() {
        let mut def = anthropic_def_with_inline_key();
        def.base_url = "https://api.example.com".to_string();
        def.extra.insert(
            "region".to_string(),
            serde_json::Value::String("us-east-1".to_string()),
        );

        let config = build_provider_config(&def);
        assert_eq!(
            config.get("base_url").and_then(serde_json::Value::as_str),
            Some("https://api.example.com")
        );
        assert_eq!(
            config.get("region").and_then(serde_json::Value::as_str),
            Some("us-east-1")
        );
    }
}
