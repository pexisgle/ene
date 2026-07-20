# `ene-ai` AI Provider & Local GGUF Inference Specifications

The `ene-ai` crate defines Ene's core abstractions for LLM (Large Language Model) chat completions and embedding providers. It provides wrappers for OpenAI-compatible web APIs and handles local GGUF model files for in-process quantized inference.

---

## 1. Provider Interfaces (Traits) & Helpers

### `LlmProvider`
*   **Signature**:
    ```rust
    #[async_trait]
    pub trait LlmProvider: Send + Sync {
        async fn create_chat_stream(
            &self,
            messages: &[LlmMessage],
            tools: &[ToolSpec],
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>, LlmProviderError>;
        async fn chat_completion(
            &self,
            messages: &[LlmMessage],
            json_schema: Option<serde_json::Value>,
        ) -> Result<String, LlmProviderError>;
        fn name(&self) -> &str;
    }
    ```

### `EmbeddingProvider`
*   **Signature**:
    ```rust
    #[async_trait]
    pub trait EmbeddingProvider: Send + Sync {
        async fn embed_batch(
            &self,
            items: &[(&str, EmbeddingKind)],
        ) -> Result<Vec<Vec<f32>>, EmbeddingError>;
        fn dimensions(&self) -> usize;
        fn model_name(&self) -> &str;
    }
    ```

#### `collect_chat_completion`
*   **Signature**: `pub async fn collect_chat_completion(provider: &dyn LlmProvider, messages: &[LlmMessage]) -> Result<String, LlmProviderError>`
*   **Description**: Helper that collects all chunks from a chat stream into a single finalized string.

#### `embed`
*   **Signature**: `pub async fn embed<P: EmbeddingProvider + ?Sized>(provider: &P, text: &str, kind: EmbeddingKind) -> Result<Vec<f32>, EmbeddingError>`
*   **Description**: Wrapper to compute the dense embedding vector for a single text block.

#### `embed_query`
*   **Signature**: `pub async fn embed_query<P: EmbeddingProvider + ?Sized>(provider: &P, text: &str) -> Result<Vec<f32>, EmbeddingError>`
*   **Description**: Wrapper to compute the embedding vector for a query, automatically appending search query prefixes if needed.

#### `cosine_similarity`
*   **Signature**: `pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32`
*   **Description**: Calculates the cosine similarity (normalized dot product) between two float vectors.

#### `LlmProviderFactory::register`
*   **Signature**: `pub fn register(factory: Arc<dyn LlmProviderFactory>)`
*   **Description**: Registers an LLM provider constructor factory in the global runtime registry.

#### `LlmProviderFactory::create_provider`
*   **Signature**: `pub fn create_provider(name: &str, config: &ene_config::EneConfig) -> Result<Box<dyn LlmProvider>, LlmProviderError>`
*   **Description**: Instantiates the target LLM provider based on name and config.

---

## 2. OpenAI Provider Implementation (`openai.rs`)

#### `build_openai_client`
*   **Signature**: `pub(crate) fn build_openai_client(base_url: &str, api_key: &str) -> Client<OpenAIConfig>`
*   **Description**: Builds the client connection handle for OpenAI-compatible APIs.

#### `OpenAiProvider::new`
*   **Signature**: `pub fn new(base_url: &str, api_key: &str, model: &str) -> Self`
*   **Description**: Constructs a cloud LLM provider.

#### `OpenAiProvider::create_chat_stream`
*   **Signature**: `async fn create_chat_stream(&self, messages: &[LlmMessage], tools: &[ene_tool_proto::ToolSpec]) -> Result<Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>, LlmProviderError>`
*   **Description**: Sends requests and returns the token stream.

#### `OpenAiProvider::chat_completion`
*   **Signature**: `async fn chat_completion(&self, messages: &[LlmMessage], json_schema: Option<serde_json::Value>) -> Result<String, LlmProviderError>`
*   **Description**: Sends single-flight structured text completion requests.

#### `OpenAiEmbeddingProvider::new`
*   **Signature**: `pub fn new(base_url: &str, api_key: &str, embedding_model: &str, embedding_dimensions: usize, query_prefix: Option<String>) -> Self`
*   **Description**: Constructs a cloud vector embedding provider.

#### `OpenAiEmbeddingProvider::embed_batch`
*   **Signature**: `async fn embed_batch(&self, items: &[(&str, EmbeddingKind)]) -> Result<Vec<Vec<f32>>, EmbeddingError>`
*   **Description**: Requests embeddings in batches.

#### `run_direct_sse_stream`
*   **Signature**: `async fn run_direct_sse_stream(api_base: &str, api_key: &str, body: serde_json::Value, name_mapping: std::collections::HashMap<String, String>, tx: tokio::sync::mpsc::Sender<Result<LlmResponseChunk, LlmProviderError>>) -> Result<(), LlmProviderError>`
*   **Description**: Direct Server-Sent Events (SSE) consumer for custom API backends.

---

## 3. Local GGUF Llama.cpp Provider (`local_llm/` & `llama_cpp/`)

#### `LocalLlamaCppProvider::load`
*   **Signature**: `pub fn load(params: &LocalGgufLoadParams) -> Result<Self, LlmProviderError>`
*   **Description**: Binds to `llama-cpp-2` and loads GGUF model files into memory.

#### `LocalLlamaCppProvider::supports_vision`
*   **Signature**: `pub fn supports_vision(&self) -> bool`
*   **Description**: Returns `true` if a vision projector model (`mmproj`) is linked.

#### `LocalLlamaCppProvider::summarize_rgb`
*   **Signature**: `pub async fn summarize_rgb(&self, width: u32, height: u32, rgb: Vec<u8>, system: &str, user: &str) -> Result<String, LlmProviderError>`
*   **Description**: Feeds raw screen frame buffers and text prompts to the local vision model to describe layout contents.

#### `LocalLlamaCppProvider::shutdown`
*   **Signature**: `pub async fn shutdown(&self)`
*   **Description**: Releases context handles and backend memory buffers.

#### `validate_model_path`
*   **Signature**: `pub(crate) fn validate_model_path(path: &str) -> Result<PathBuf, LlmProviderError>`
*   **Description**: Verifies if GGUF files exist on the filesystem.

#### `resolve_gpu_offload`
*   **Signature**: `pub(crate) fn resolve_gpu_offload(acceleration: ProactiveAcceleration, gpu_layers: &str) -> Result<GpuOffload, LlmProviderError>`
*   **Description**: Determines GPU layers to offload to accelerate local inference.

#### `generate_chat`
*   **Signature**: `pub(crate) fn generate_chat(loaded: &LoadedModel, messages: &[LlmMessage], json_schema: Option<&serde_json::Value>, timeout: Duration) -> Result<String, LlmProviderError>`
*   **Description**: Feeds dialog histories and evaluates token generation.

#### `sample_tokens`
*   **Signature**: `fn sample_tokens(loaded: &LoadedModel, ctx: &mut llama_cpp_2::context::LlamaContext<'_>, batch: &mut LlamaBatch, json_schema: Option<&serde_json::Value>, deadline: Instant, max_tokens: i32, mut n_cur: i32) -> Result<String, LlmProviderError>`
*   **Description**: Applies temperature, top-p, and schema grammar limits on generated tokens.

#### `with_backend`
*   **Signature**: `pub(crate) fn with_backend<T, F>(f: F) -> Result<T, LlmProviderError> where F: FnOnce(&LlamaBackend) -> Result<T, LlmProviderError>`
*   **Description**: Safe wrapper implementing global thread serialization locks around C++ backend contexts.

#### `embed_text`
*   **Signature**: `pub(crate) fn embed_text(loaded: &LoadedModel, text: &str) -> Result<Vec<f32>, LlmProviderError>`
*   **Description**: Generates a local text embedding vector.

#### `create_local_provider`
*   **Signature**: `pub fn create_local_provider(local: &ResolvedLocalModel) -> Result<Box<dyn crate::EmbeddingProvider>, EneEmbeddingError>`
*   **Description**: Constructs a local vector embedder instance.

---

## 4. Model Downloading & Cache Manager (`gguf/`)

#### `ensure_gguf_available`
*   **Signature**: `pub async fn ensure_gguf_available(local: &ResolvedLocalModel) -> Result<PathBuf, LlmProviderError>`
*   **Description**: Checks if local GGUF files are present and complete. Starts async down-loaders if missing.

#### `ensure_mmproj_available`
*   **Signature**: `pub async fn ensure_mmproj_available(local: &ResolvedLocalModel) -> Result<Option<PathBuf>, LlmProviderError>`
*   **Description**: Checks and downloads multimodal vision projection weight files.

#### `prefetch_configured_gguf`
*   **Signature**: `pub async fn prefetch_configured_gguf(config: &AiConfig, prefetch_embedding: bool, prefetch_decision: bool) -> Result<(), LlmProviderError>`
*   **Description**: Scans configurations and downloads all required GGUF files in parallel.

#### `download_gguf`
*   **Signature**: `pub async fn download_gguf(url: &str, dest: &Path) -> Result<(), LlmProviderError>`
*   **Description**: Handles download request redirects, creates temporary `.part` files, streams file chunks, and reports progress.

#### `filename_from_url`
*   **Signature**: `pub fn filename_from_url(url: &str) -> Result<String, LlmProviderError>`
*   **Description**: Sanitizes and returns safe filename strings from URLs.

---

## 5. RAG Retrieval Helpers (`hybrid.rs`)

#### `HybridRerankProvider::new`
*   **Signature**: `pub fn new(embedder: Arc<dyn EmbeddingProvider>) -> Self`
*   **Description**: Constructs RAG search setups.

#### `HybridRerankProvider::hyde`
*   **Signature**: `pub async fn hyde(&self, query: &str) -> Result<String, EmbeddingError>`
*   **Description**: Generates hypothetical answer documents using the HyDE model to improve semantic search vector alignments.

#### `HybridRerankProvider::rerank`
*   **Signature**: `pub async fn rerank(&self, query: &str, candidates: &[ene_tool_proto::ToolSpec]) -> Result<Vec<f32>, EmbeddingError>`
*   **Description**: Ranks candidates using cross-encoder models.

#### `hyde_document`
*   **Signature**: `pub async fn hyde_document(llm: Option<&dyn LlmProvider>, query: &str) -> Result<String, EmbeddingError>`
*   **Description**: Formats prompts for HyDE hypothetical documents.

#### `rerank_tool_specs`
*   **Signature**: `pub async fn rerank_tool_specs(embedder: &dyn EmbeddingProvider, rerank_llm: Option<&dyn LlmProvider>, query: &str, candidates: &[ene_tool_proto::ToolSpec]) -> Result<Vec<f32>, EmbeddingError>`
*   **Description**: Computes cross-attention reranking matrix coefficients.
