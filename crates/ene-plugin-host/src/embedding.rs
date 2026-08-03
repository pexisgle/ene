//! IPC-backed embedding provider bridging the plugin wire protocol to
//! `ene_ai::EmbeddingProvider`.
//!
//! [`IpcEmbeddingProvider`] holds a shared connection to a plugin binary and
//! translates `EmbeddingProvider` trait calls into `EmbedBatch` IPC
//! messages. [`IpcEmbeddingProviderFactory`] builds instances from the
//! configured embedding task, applying the same API-key trust gate as
//! [`IpcLlmProviderFactory`](crate::factory::IpcLlmProviderFactory).

use std::sync::Arc;

use async_trait::async_trait;
use ene_ai::RetryPolicy;
use ene_ai::traits::{EmbeddingError, EmbeddingKind};
use ene_plugin_proto::PluginIpcResponse;

use crate::config::PluginConfig;
use crate::error::PluginHostError;
use crate::ipc_plugin::IpcPluginConnection;

/// An `EmbeddingProvider` that delegates to a plugin binary over IPC.
///
/// Created by [`IpcEmbeddingProviderFactory`]; model, dimensions, and query
/// prefix are captured from the resolved embedding task at creation time.
pub struct IpcEmbeddingProvider {
    kind: String,
    conn: Arc<IpcPluginConnection>,
    model: String,
    dimensions: usize,
    /// Optional retrieval-query prefix (`tasks.embedding.query_prefix`),
    /// applied to [`EmbeddingKind::Query`] items before they cross the IPC
    /// boundary (the wire format carries no kinds).
    query_prefix: Option<String>,
    provider_config: serde_json::Value,
    retry_policy: RetryPolicy,
}

impl IpcEmbeddingProvider {
    /// Creates a new IPC-backed embedding provider.
    pub fn new(
        kind: String,
        conn: Arc<IpcPluginConnection>,
        model: String,
        dimensions: usize,
        query_prefix: Option<String>,
        provider_config: serde_json::Value,
        retry_policy: RetryPolicy,
    ) -> Self {
        Self {
            kind,
            conn,
            model,
            dimensions,
            query_prefix,
            provider_config,
            retry_policy,
        }
    }
}

/// Applies the configured query prefix exactly once to a text input, based
/// on its [`EmbeddingKind`]. Mirrors the contract the in-process cloud
/// provider used so `embed_query` does not double-prefix.
fn apply_kind_prefix(text: &str, kind: EmbeddingKind, prefix: Option<&str>) -> String {
    match (kind, prefix) {
        (EmbeddingKind::Query, Some(p)) => format!("{p}{text}"),
        _ => text.to_string(),
    }
}

/// Maps a [`PluginHostError`] into the [`EmbeddingError`] domain.
///
/// Transport failures become [`EmbeddingError::Provider`] (retryable);
/// everything else is a provider-level error too, but the host's transport
/// retry re-issues the whole IPC call, so upstream plugin errors are not
/// retried — matching the chat path's `map_host_error`.
fn map_host_error(e: PluginHostError) -> EmbeddingError {
    match e {
        PluginHostError::TransportFailed { message } => EmbeddingError::Provider(message),
        other => EmbeddingError::Provider(other.to_string()),
    }
}

#[async_trait]
impl ene_ai::EmbeddingProvider for IpcEmbeddingProvider {
    async fn embed_batch(
        &self,
        items: &[(&str, EmbeddingKind)],
    ) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        if items.is_empty() {
            return Ok(Vec::new());
        }

        let inputs: Vec<String> = items
            .iter()
            .map(|(text, kind)| apply_kind_prefix(text, *kind, self.query_prefix.as_deref()))
            .collect();

        let request_id = uuid::Uuid::new_v4().to_string();
        let embeddings = self
            .retry_policy
            .run(EmbeddingError::is_retryable, || {
                let conn = Arc::clone(&self.conn);
                let request_id = request_id.clone();
                let kind = self.kind.clone();
                let provider_config = self.provider_config.clone();
                let model = self.model.clone();
                let dimensions = self.dimensions;
                let inputs = inputs.clone();
                async move {
                    conn.embed_batch(
                        request_id,
                        kind,
                        provider_config,
                        model,
                        u32::try_from(dimensions).ok(),
                        inputs,
                    )
                    .await
                    .map_err(map_host_error)
                }
            })
            .await?;

        // OpenAI returns embeddings in input order. Fail loudly if the count
        // or per-vector dimension is wrong rather than silently truncating,
        // which would mask server-side batching bugs.
        if embeddings.len() != items.len() {
            return Err(EmbeddingError::DimensionMismatch(format!(
                "Embedding response count {} does not match request count {}",
                embeddings.len(),
                items.len()
            )));
        }
        for (i, emb) in embeddings.iter().enumerate() {
            if emb.len() != self.dimensions {
                return Err(EmbeddingError::DimensionMismatch(format!(
                    "item {i}: expected {} dims, got {}",
                    self.dimensions,
                    emb.len()
                )));
            }
        }
        Ok(embeddings)
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

/// Factory that creates [`IpcEmbeddingProvider`] instances for a provider
/// kind served by a plugin binary.
pub struct IpcEmbeddingProviderFactory {
    kind: String,
    conn: Arc<IpcPluginConnection>,
    plugin_name: String,
    /// Whether the plugin is one of the trusted built-ins that ship with Ene
    /// (matched against the compiled-in
    /// [`BUILTIN_PLUGIN_NAMES`](crate::manager::BUILTIN_PLUGIN_NAMES) list).
    /// Built-in plugins are always trusted to receive credentials.
    builtin: bool,
}

impl IpcEmbeddingProviderFactory {
    /// Creates a new factory for the given provider kind, sharing the
    /// plugin connection.
    ///
    /// `plugin_name` and `builtin` drive the API key trust gate: credentials
    /// are only forwarded when `builtin` is `true` or the plugin has an
    /// explicit entry in `plugins.list` (see [`Self::create_provider`]).
    pub fn new(
        kind: String,
        conn: Arc<IpcPluginConnection>,
        plugin_name: String,
        builtin: bool,
    ) -> Self {
        Self {
            kind,
            conn,
            plugin_name,
            builtin,
        }
    }

    /// Whether this plugin is trusted to receive resolved API credentials.
    fn is_trusted(&self, plugin_config: &PluginConfig) -> bool {
        self.builtin || plugin_config.list.contains_key(&self.plugin_name)
    }
}

impl ene_ai::EmbeddingProviderFactory for IpcEmbeddingProviderFactory {
    fn provider_kind(&self) -> &str {
        &self.kind
    }

    /// Creates an embedding provider for the configured embedding task.
    ///
    /// The provider definition is located by kind (with the legacy
    /// `openai_compatible` alias folded onto `openai`), mirroring
    /// [`IpcLlmProviderFactory`](crate::factory::IpcLlmProviderFactory)'s
    /// lookup; the model, dimensions, and query prefix come from
    /// `ai.tasks.embedding`.
    fn create_embedding_provider(
        &self,
        config: &ene_config::EneConfig,
    ) -> Result<Arc<dyn ene_ai::EmbeddingProvider>, EmbeddingError> {
        let ai_config = config.get_section::<ene_ai::AiConfig>().unwrap_or_default();
        let plugin_config = config.get_section::<PluginConfig>().unwrap_or_default();
        let embedding = &ai_config.tasks.embedding;

        let model = embedding
            .model
            .clone()
            .filter(|m| !m.trim().is_empty())
            .ok_or_else(|| {
                EmbeddingError::Init(
                    "embedding task requires a model for this provider kind".to_string(),
                )
            })?;

        let trusted = self.is_trusted(&plugin_config);
        let provider_config = ai_config
            .providers
            .values()
            .find(|def| crate::factory::provider_def_kind_matches(def, &self.kind))
            .map_or_else(
                || serde_json::json!({}),
                |def| {
                    if !trusted {
                        tracing::warn!(
                            component = "IpcEmbeddingProviderFactory",
                            plugin = %self.plugin_name,
                            kind = %self.kind,
                            "plugin is neither built-in nor listed in plugins.list; \
                             withholding API credentials"
                        );
                    }
                    crate::factory::build_provider_config(def, trusted)
                },
            );

        let dimensions = embedding.dimensions.unwrap_or(1536);
        let query_prefix = embedding
            .query_prefix
            .clone()
            .filter(|p| !p.trim().is_empty());
        let retry_policy = ai_config.retry.to_policy();

        Ok(Arc::new(IpcEmbeddingProvider::new(
            self.kind.clone(),
            Arc::clone(&self.conn),
            model,
            dimensions,
            query_prefix,
            provider_config,
            retry_policy,
        )))
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "unit tests use unwrap for concise assertions"
)]
mod tests {
    use super::*;

    #[test]
    fn apply_kind_prefix_query_with_prefix() {
        assert_eq!(
            apply_kind_prefix("hello", EmbeddingKind::Query, Some("Q:")),
            "Q:hello"
        );
    }

    #[test]
    fn apply_kind_prefix_query_without_prefix() {
        assert_eq!(
            apply_kind_prefix("hello", EmbeddingKind::Query, None),
            "hello"
        );
    }

    #[test]
    fn apply_kind_prefix_non_query_never_prefixed() {
        assert_eq!(
            apply_kind_prefix("hello", EmbeddingKind::Summary, Some("Q:")),
            "hello"
        );
    }
}
