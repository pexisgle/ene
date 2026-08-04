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
/// everything else is [`EmbeddingError::Other`] (not retried by the policy),
/// mirroring the chat path's `map_host_error` — the plugin has already
/// retried its own upstream errors, so the host must not re-issue the IPC
/// call (which would re-run the plugin's retry budget).
fn map_host_error(e: PluginHostError) -> EmbeddingError {
    match e {
        PluginHostError::TransportFailed { message } => EmbeddingError::Provider(message),
        other => EmbeddingError::Other(other.to_string()),
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
        validate_embeddings(&embeddings, items.len(), self.dimensions)?;
        Ok(embeddings)
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

fn validate_embeddings(
    embeddings: &[Vec<f32>],
    expected_count: usize,
    expected_dimensions: usize,
) -> Result<(), EmbeddingError> {
    if embeddings.len() != expected_count {
        return Err(EmbeddingError::DimensionMismatch(format!(
            "Embedding response count {} does not match request count {}",
            embeddings.len(),
            expected_count
        )));
    }
    for (i, emb) in embeddings.iter().enumerate() {
        if emb.len() != expected_dimensions {
            return Err(EmbeddingError::DimensionMismatch(format!(
                "item {i}: expected {expected_dimensions} dims, got {}",
                emb.len()
            )));
        }
        if emb.iter().any(|value| !value.is_finite()) {
            return Err(EmbeddingError::DimensionMismatch(format!(
                "item {i}: embedding contains a non-finite value"
            )));
        }
    }
    Ok(())
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

        let dimensions = resolved_embedding_dimensions(&ai_config, embedding, &model)?;
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

/// The embedding dimensionality the host reports for the configured task.
///
/// Local GGUF models declare their real dimensionality on the model entry
/// (`ai.local_models.<name>.dimensions`): the store schema is opened with it
/// before the plugin host starts, so the cloud default (1536) must never be
/// substituted here — the plugin would reject the mismatch on the first
/// batch.
fn resolved_embedding_dimensions(
    ai_config: &ene_ai::AiConfig,
    embedding: &ene_ai::TaskRef,
    model: &str,
) -> Result<usize, EmbeddingError> {
    if ene_ai::AiConfig::is_local_provider(&embedding.provider) {
        ai_config
            .local_models
            .get(model)
            .and_then(|def| def.dimensions)
            .ok_or_else(|| {
                EmbeddingError::Init(format!(
                    "local embedding model {model:?} requires \
                     ai.local_models.{model}.dimensions (the store schema \
                     needs the real vector dimensionality before the plugin \
                     can load the model)"
                ))
            })
    } else {
        Ok(embedding.dimensions.unwrap_or(1536))
    }
}

#[cfg(test)]
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

    #[test]
    fn map_host_error_transport_failure_is_retryable() {
        let err = map_host_error(PluginHostError::TransportFailed {
            message: "connection reset".to_string(),
        });
        assert!(
            matches!(err, EmbeddingError::Provider(_)),
            "transport failures must be retryable, got {err:?}"
        );
        assert!(err.is_retryable());
    }

    #[test]
    fn map_host_error_upstream_plugin_error_is_not_retried() {
        // The plugin already retried its own upstream failures; the host
        // must not re-issue the IPC call and run that budget again.
        let err = map_host_error(PluginHostError::ConnectFailed {
            name: "openai".to_string(),
            reason: "provider rejected the request".to_string(),
        });
        assert!(
            matches!(err, EmbeddingError::Other(_)),
            "non-transport errors must be terminal, got {err:?}"
        );
        assert!(!err.is_retryable());
    }

    #[test]
    fn local_task_uses_declared_model_dimensions() {
        let mut ai = ene_ai::AiConfig::default();
        ai.local_models.insert(
            "jina-v5-small".to_string(),
            ene_ai::LocalModelDef {
                dimensions: Some(1024),
                ..ene_ai::LocalModelDef::default()
            },
        );
        let embedding = ene_ai::TaskRef {
            provider: "local".to_string(),
            model: Some("jina-v5-small".to_string()),
            // The cloud knob must not leak into the local path.
            dimensions: Some(1536),
            ..ene_ai::TaskRef::default()
        };
        assert_eq!(
            resolved_embedding_dimensions(&ai, &embedding, "jina-v5-small")
                .expect("declared dims resolve"),
            1024
        );
    }

    #[test]
    fn local_task_without_declared_dimensions_is_typed_error() {
        let ai = ene_ai::AiConfig::default();
        let embedding = ene_ai::TaskRef {
            provider: "local".to_string(),
            model: Some("jina-v5-small".to_string()),
            ..ene_ai::TaskRef::default()
        };
        let err = resolved_embedding_dimensions(&ai, &embedding, "jina-v5-small")
            .expect_err("missing dims must be a typed init error");
        assert!(
            matches!(err, EmbeddingError::Init(ref message) if message.contains("jina-v5-small.dimensions")),
            "err: {err}"
        );
    }

    #[test]
    fn cloud_task_defaults_to_1536_without_override() {
        let ai = ene_ai::AiConfig::default();
        let embedding = ene_ai::TaskRef::default();
        assert_eq!(
            resolved_embedding_dimensions(&ai, &embedding, "text-embedding-3-small")
                .expect("cloud dims resolve"),
            1536
        );
    }

    #[test]
    fn cloud_task_honors_configured_dimensions() {
        let ai = ene_ai::AiConfig::default();
        let embedding = ene_ai::TaskRef {
            dimensions: Some(256),
            ..ene_ai::TaskRef::default()
        };
        assert_eq!(
            resolved_embedding_dimensions(&ai, &embedding, "text-embedding-3-small")
                .expect("cloud dims resolve"),
            256
        );
    }

    #[test]
    fn rejects_non_finite_embedding_values() {
        assert!(matches!(
            validate_embeddings(&[vec![0.1, f32::NAN, 0.3]], 1, 3),
            Err(EmbeddingError::DimensionMismatch(message))
                if message.contains("non-finite")
        ));
    }
}
