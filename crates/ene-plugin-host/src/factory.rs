//! LLM provider factory backed by a plugin IPC connection.
//!
//! [`IpcLlmProviderFactory`] implements [`ene_ai::LlmProviderFactory`] so
//! that plugin-provided LLM providers integrate with the global
//! [`LlmProviderRegistry`](ene_ai::LlmProviderRegistry).
//!
//! ## API key trust gate
//!
//! Resolved API credentials are only forwarded to plugins that are either
//! **built-in** (one of the compiled-in shipped plugin names, see
//! [`BUILTIN_PLUGIN_NAMES`](crate::manager::BUILTIN_PLUGIN_NAMES)) or
//! **explicitly listed** under `plugins.list.<name>` in configuration. Any
//! other plugin receives the provider definition *without* an `api_key`, so
//! an arbitrary user-supplied plugin binary can never silently harvest
//! credentials it was never meant to see.

use std::sync::Arc;

use ene_ai::error::LlmProviderError;
use ene_ai::traits::{LlmProvider, LlmProviderFactory};
use ene_ai::{AiProviderDef, TaskRef};
use ene_plugin_proto::ConcurrencyHint;

use crate::config::PluginConfig;
use crate::ipc_plugin::IpcPluginConnection;
use crate::ipc_provider::{ConcurrencyLimiter, IpcLlmProvider};

/// Factory that creates [`IpcLlmProvider`] instances for a specific
/// provider kind served by a plugin binary.
pub struct IpcLlmProviderFactory {
    kind: String,
    conn: Arc<IpcPluginConnection>,
    /// Name of the plugin binary serving this provider kind. Used for the
    /// `plugins.list.<name>` trust lookup and diagnostics.
    plugin_name: String,
    /// Whether the plugin is one of the trusted built-ins that ship with Ene
    /// (matched against the compiled-in
    /// [`BUILTIN_PLUGIN_NAMES`](crate::manager::BUILTIN_PLUGIN_NAMES) list,
    /// not a filesystem location). Built-in plugins are always trusted to
    /// receive credentials.
    builtin: bool,
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
    /// `plugin_name` and `builtin` drive the API key trust gate: credentials
    /// are only forwarded when `builtin` is `true` or the plugin has an
    /// explicit entry in `plugins.list` (see [`Self::create_provider`]).
    ///
    /// `concurrency` is the [`ConcurrencyHint`] the plugin declared for this
    /// provider kind during the handshake (or the safe serial default if it
    /// declared none); it is built into a single [`ConcurrencyLimiter`]
    /// shared by every provider instance this factory subsequently creates.
    pub fn new(
        kind: String,
        conn: Arc<IpcPluginConnection>,
        plugin_name: String,
        builtin: bool,
        context_window: Option<u32>,
        concurrency: ConcurrencyHint,
    ) -> Self {
        Self {
            kind,
            conn,
            plugin_name,
            builtin,
            context_window,
            limiter: Arc::new(ConcurrencyLimiter::new(concurrency)),
        }
    }

    /// Whether this plugin is trusted to receive resolved API credentials.
    ///
    /// Trust is granted to built-in plugins and to any plugin explicitly
    /// listed under `plugins.list.<name>`.
    fn is_trusted(&self, plugin_config: &PluginConfig) -> bool {
        self.builtin || plugin_config.list.contains_key(&self.plugin_name)
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
        let plugin_config = config.get_section::<PluginConfig>().unwrap_or_default();

        // Honor the active cognitive task's own model / max_tokens overrides
        // rather than always falling back to `tasks.chat`. When the task
        // carries no override, fall back to the chat defaults.
        let model = task
            .model
            .clone()
            .or_else(|| ai_config.tasks.chat.model.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let max_tokens = task.max_tokens.or(ai_config.tasks.chat.max_tokens);

        let trusted = self.is_trusted(&plugin_config);

        let provider_config = ai_config
            .providers
            .values()
            .find(|def| provider_def_kind_matches(def, &self.kind))
            .map_or_else(
                || serde_json::json!({}),
                |def| {
                    if !trusted {
                        tracing::warn!(
                            component = "IpcLlmProviderFactory",
                            plugin = %self.plugin_name,
                            kind = %self.kind,
                            "plugin is neither built-in nor listed in plugins.list; \
                             withholding API credentials"
                        );
                    }
                    build_provider_config(def, trusted)
                },
            );

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

/// Whether a provider definition serves the plugin factory's `kind`.
///
/// The legacy `openai_compatible` alias is folded onto the `openai` plugin
/// kind so pre-plugin configs keep resolving to the `openai` plugin.
pub(crate) fn provider_def_kind_matches(def: &AiProviderDef, kind: &str) -> bool {
    ene_ai::canonical_provider_kind(&def.kind) == kind
}

/// Builds the `provider_config` JSON forwarded to a plugin LLM provider.
///
/// The API key — when present and `trusted` — is sent as a **plain JSON
/// string** under `api_key`, matching the plugin-side contract (see the
/// Anthropic plugin's `resolve_api_key`). When `trusted` is `false`, no
/// `api_key` field is emitted at all, so credentials never reach an
/// untrusted plugin.
pub(crate) fn build_provider_config(def: &AiProviderDef, trusted: bool) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    if !def.base_url.is_empty() {
        map.insert(
            "base_url".to_string(),
            serde_json::Value::String(def.base_url.clone()),
        );
    }
    if trusted {
        let api_key = def.api_key.resolve_api_key();
        if !api_key.is_empty() {
            map.insert("api_key".to_string(), serde_json::Value::String(api_key));
        }
    }
    for (k, v) in &def.extra {
        map.insert(k.clone(), v.clone());
    }
    serde_json::Value::Object(map)
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "tests use expect for concise failure messages"
)]
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

    /// Contract: a trusted plugin receives the API key as a *plain JSON
    /// string* (not a `{source, inline}` object).
    #[test]
    fn trusted_plugin_receives_plain_string_api_key() {
        let def = anthropic_def_with_inline_key();
        let config = build_provider_config(&def, true);

        let api_key = config.get("api_key").expect("api_key present when trusted");
        assert!(
            api_key.is_string(),
            "api_key must be a plain JSON string, got {api_key:?}"
        );
        assert_eq!(api_key.as_str(), Some("sk-test-123"));
    }

    /// Contract: an untrusted plugin never receives an `api_key` field, even
    /// when the provider definition has a resolvable key.
    #[test]
    fn untrusted_plugin_does_not_receive_api_key() {
        let def = anthropic_def_with_inline_key();
        let config = build_provider_config(&def, false);

        assert!(
            config.get("api_key").is_none(),
            "api_key must be withheld from untrusted plugins, got {config:?}"
        );
    }

    /// `base_url` and `extra` fields are forwarded regardless of trust, since
    /// they are not secrets.
    #[test]
    fn non_secret_fields_forwarded_regardless_of_trust() {
        let mut def = anthropic_def_with_inline_key();
        def.base_url = "https://api.example.com".to_string();
        def.extra.insert(
            "region".to_string(),
            serde_json::Value::String("us-east-1".to_string()),
        );

        for trusted in [true, false] {
            let config = build_provider_config(&def, trusted);
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
}
