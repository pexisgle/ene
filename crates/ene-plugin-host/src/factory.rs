//! LLM provider factory backed by a plugin IPC connection.
//!
//! [`IpcLlmProviderFactory`] implements [`ene_ai::LlmProviderFactory`] so
//! that plugin-provided LLM providers integrate with the global
//! [`LlmProviderRegistry`](ene_ai::LlmProviderRegistry).

use std::sync::Arc;

use ene_ai::error::LlmProviderError;
use ene_ai::traits::{LlmProvider, LlmProviderFactory};
use tokio::sync::Mutex;

use crate::ipc_plugin::IpcPluginConnection;
use crate::ipc_provider::IpcLlmProvider;

/// Factory that creates [`IpcLlmProvider`] instances for a specific
/// provider kind served by a plugin binary.
pub struct IpcLlmProviderFactory {
    kind: String,
    conn: Arc<Mutex<IpcPluginConnection>>,
}

impl IpcLlmProviderFactory {
    /// Creates a new factory for the given provider kind, sharing the
    /// plugin connection.
    pub fn new(kind: String, conn: Arc<Mutex<IpcPluginConnection>>) -> Self {
        Self { kind, conn }
    }
}

impl LlmProviderFactory for IpcLlmProviderFactory {
    fn provider_name(&self) -> &str {
        &self.kind
    }

    fn create_provider(
        &self,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn LlmProvider>, LlmProviderError> {
        let ai_config = config.get_section::<ene_ai::AiConfig>().unwrap_or_default();

        let model = ai_config
            .tasks
            .chat
            .model
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let max_tokens = ai_config.tasks.chat.max_tokens;

        // Build provider_config from the AiProviderDef that matches this kind.
        let provider_config = ai_config
            .providers
            .values()
            .find(|def| def.kind == self.kind)
            .map_or_else(
                || serde_json::json!({}),
                |def| {
                    let mut map = serde_json::Map::new();
                    if !def.base_url.is_empty() {
                        map.insert(
                            "base_url".to_string(),
                            serde_json::Value::String(def.base_url.clone()),
                        );
                    }
                    // Resolve API key value.
                    let api_key_value = resolve_api_key_value(&def.api_key);
                    if !api_key_value.is_empty() {
                        map.insert(
                            "api_key".to_string(),
                            serde_json::Value::String(api_key_value),
                        );
                    }
                    // Merge extra fields.
                    for (k, v) in &def.extra {
                        map.insert(k.clone(), v.clone());
                    }
                    serde_json::Value::Object(map)
                },
            );

        let provider = IpcLlmProvider::new(
            self.kind.clone(),
            Arc::clone(&self.conn),
            model,
            max_tokens,
            provider_config,
        );

        Ok(Box::new(provider))
    }
}

/// Resolves the actual API key string from an [`ApiKeyConfig`](ene_ai::ApiKeyConfig).
fn resolve_api_key_value(api_key: &ene_ai::ApiKeyConfig) -> String {
    match api_key.source.as_str() {
        "inline" => api_key.inline.clone(),
        "env" => std::env::var(&api_key.env).unwrap_or_default(),
        _ => String::new(),
    }
}
