use async_trait::async_trait;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use tokio_stream::Stream;

use crate::message::{LlmMessage, LlmResponseChunk};

/// Trait implemented by LLM providers to interface with Ene.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Returns the unique name of this provider.
    fn name(&self) -> &str;

    /// Initiates a chat completion stream with the given messages and tools.
    async fn create_chat_stream(
        &self,
        messages: &[LlmMessage],
        tools: &[ene_tool_proto::ToolDefinition],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, String>> + Send>>, String>;

    /// Executes a non-streaming chat completion with optional JSON schema response.
    async fn chat_completion(
        &self,
        messages: &[LlmMessage],
        json_schema: Option<serde_json::Value>,
    ) -> Result<String, String>;
}

/// Trait for generating vector embeddings from text (used by memory search).
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Generate an embedding vector for the given text (used for indexing / storage).
    async fn embed(&self, text: &str) -> Result<Vec<f32>, String>;
    /// Generate an embedding vector optimised for query / search.
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, String>;
    /// The dimensionality of the embedding vectors produced by this provider.
    fn dimensions(&self) -> usize;
    /// A human-readable name identifying the embedding model.
    fn model_name(&self) -> &str;
}

/// Factory trait to build specific `LlmProvider` instances from workspace configs.
pub trait LlmProviderFactory: Send + Sync {
    /// The unique name of the provider this factory produces.
    fn provider_name(&self) -> &str;

    /// Instantiates the provider based on current `EneConfig` settings.
    fn create_provider(
        &self,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn LlmProvider>, String>;
}

/// Global registry of `LlmProviderFactory` implementations.
pub struct LlmProviderRegistry {
    factories: Mutex<HashMap<String, Arc<dyn LlmProviderFactory>>>,
}

impl LlmProviderRegistry {
    /// Returns the static, thread-safe global registry instance.
    fn global() -> &'static Self {
        static REGISTRY: OnceLock<LlmProviderRegistry> = OnceLock::new();
        REGISTRY.get_or_init(|| Self {
            factories: Mutex::new(HashMap::new()),
        })
    }

    /// Registers a new provider factory.
    pub fn register(factory: Arc<dyn LlmProviderFactory>) {
        let name = factory.provider_name().to_string();
        if let Ok(mut guard) = Self::global().factories.lock() {
            guard.insert(name, factory);
        }
    }

    /// Tries to instantiate a provider by name using the registered factories.
    pub fn create_provider(
        name: &str,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn LlmProvider>, String> {
        let factory = {
            if let Ok(guard) = Self::global().factories.lock() {
                guard.get(name).cloned()
            } else {
                None
            }
        };

        match factory {
            Some(f) => f.create_provider(config),
            None => Err(format!(
                "No LlmProviderFactory registered for provider name: '{}'",
                name
            )),
        }
    }
}
