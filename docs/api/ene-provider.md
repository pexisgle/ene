# `ene-provider` — API Reference

> **Crate:** `ene-provider`
> **Role:** Trait definitions and built-in implementations for LLM and embedding providers.

---

## Overview

`ene-provider` defines the provider abstraction layer that decouples the Ene runtime from specific AI service vendors. All LLM calls and embedding operations flow through two core `async` traits: `LlmProvider` and `EmbeddingProvider`. Both report failures through typed errors (`LlmProviderError`, `EmbeddingError`) rather than `String`, so callers can dispatch on the variant (e.g. show a "rate limited" notice on `LlmProviderError::RateLimit`).

Providers are registered at startup via `LlmProviderRegistry` and can be swapped via configuration (`ProviderConfig`) without changing application code.

```mermaid
flowchart LR
    Core[ene-core] -->|dyn LlmProvider| Registry[LlmProviderRegistry]
    Core -->|dyn EmbeddingProvider| EP[EmbeddingProvider]
    Registry --> OAI[OpenAiProvider]
    EP --> Cloud[CloudEmbeddingProvider]
    EP --> Local[GgufEmbeddingProvider\n(ene-embedding)]
    EP --> Hybrid[HybridRerankProvider]
```

---

## `LlmProvider` Trait

The core interface for language model backends. Every method is `async`.

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn create_chat_stream(
        &self,
        messages: &[LlmMessage],
        tools: &[ene_tool_proto::ToolSpec],
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
        LlmProviderError,
    >;

    async fn chat_completion(
        &self,
        messages: &[LlmMessage],
        json_schema: Option<serde_json::Value>,
    ) -> Result<String, LlmProviderError>;
}
```

### Method Table

| Method | Signature | Description |
|---|---|---|
| `name` | `fn name(&self) -> &str` | Human-readable provider name (e.g. `"openai-compatible"`). Not `async`. |
| `create_chat_stream` | `async fn create_chat_stream(&self, messages: &[LlmMessage], tools: &[ToolSpec]) -> Result<Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>, LlmProviderError>` | Opens a streaming chat completion. Used for all interactive turns where the user sees streamed output. |
| `chat_completion` | `async fn chat_completion(&self, messages: &[LlmMessage], json_schema: Option<serde_json::Value>) -> Result<String, LlmProviderError>` | Non-streaming completion, optionally constrained to a JSON Schema. Used for internal tasks (e.g. session summarization, rerank scoring) that need structured output. |

---

## `EmbeddingProvider` Trait

Interface for text embedding and semantic utility operations. Every method is `async` except `dimensions`/`model_name`/`has_reranker`, which have defaults or are trivial getters.

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str, kind: EmbeddingKind) -> Result<Vec<f32>, EmbeddingError>;

    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        self.embed(text, EmbeddingKind::Query).await
    }

    async fn embed_batch(
        &self,
        items: &[(&str, EmbeddingKind)],
    ) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        /* default: serial loop calling embed() once per item */
        unimplemented!()
    }

    async fn hyde(&self, query: &str) -> Result<String, EmbeddingError> {
        Ok(query.to_string())
    }

    fn has_reranker(&self) -> bool {
        false
    }

    async fn rerank(
        &self,
        query: &str,
        candidates: &[ene_tool_proto::ToolSpec],
    ) -> Result<Vec<f32>, EmbeddingError> {
        /* default: cosine similarity between embed_query(query) and embed_batch(candidates) */
        unimplemented!()
    }

    fn dimensions(&self) -> usize;
    fn model_name(&self) -> &str;
}
```

### Method Table

| Method | Required? | Default behavior |
|---|---|---|
| `embed(text, kind)` | **Required** | — |
| `embed_query(text)` | Optional | Delegates to `embed(text, EmbeddingKind::Query)`. Override only if the provider needs a different query-prefix code path. |
| `embed_batch(items)` | Optional | Serial loop calling `embed` once per item. Override for real batching/parallelism. |
| `hyde(query)` | Optional | Echoes `query` back unchanged (no-op HyDE). Override to generate a real hypothetical document (typically via an LLM). |
| `has_reranker()` | Optional | `false`. Override to advertise a native reranker so callers can skip a manual `rerank()` call when it would just add latency. |
| `rerank(query, candidates)` | Optional | Embeds `query` via `embed_query`, embeds each candidate's `"{summary} {description}"` via `embed_batch` with `EmbeddingKind::Description`, and scores each with [`cosine_similarity`](#cosine_similarity). Returned scores are aligned with `candidates` (same length). |
| `dimensions()` | **Required** | — Dimensionality of the output vectors. |
| `model_name()` | **Required** | — Human-readable model identifier string. |

### `EmbeddingKind`

A hint that tells the provider how the text will be used. Providers may apply different prefixes or chunking strategies per kind (e.g. the built-in providers prefix `Query`/`Hyde` text with `"Query: "`).

```rust
pub enum EmbeddingKind {
    Summary,
    Description,
    Capability,
    Example,
    Negative,
    Query,
    Hyde,
}
```

| Variant | Meaning |
|---|---|
| `Summary` | Concise tool or document summary text. |
| `Description` | Full description text (often long). |
| `Capability` | One embedding per capability/action within a mega-tool. |
| `Example` | One embedding per worked example. |
| `Negative` | Negative-keyword text (soft penalties in Tool RAG). |
| `Query` | A user query — may use a different prefix than documents. |
| `Hyde` | A hypothetical document generated by HyDE. |

---

## `cosine_similarity`

```rust
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32
```

Cosine similarity between two vectors. Returns `0.0` if either input is empty or the two lengths differ (rather than panicking). Both vectors should have the same length for a meaningful result. Used internally by the default `rerank()` implementation and by [`HybridRerankProvider`](#hybridrerankprovider)'s fallback path.

---

## Message Types

### `LlmMessage`

The unified message format sent to any LLM provider. Serialized with `#[serde(rename_all = "snake_case", tag = "role")]`.

```rust
pub enum LlmMessage {
    System { content: String },
    User { parts: Vec<UserMessagePart> },
    Assistant {
        content: Option<String>,
        tool_calls: Option<Vec<LlmToolCall>>,
    },
    Tool { tool_call_id: String, content: String },
}
```

| Variant | Description |
|---|---|
| `System { content }` | Instructions to the model. Typically the character card + injected context. |
| `User { parts }` | A user message, which may include text and inline images (see `UserMessagePart`). |
| `Assistant { content, tool_calls }` | A previous assistant response, optionally with tool call records. |
| `Tool { tool_call_id, content }` | The result of a tool call, keyed by `tool_call_id`. |

### `UserMessagePart`

```rust
pub enum UserMessagePart {
    Text { text: String },
    Image { base64_image_data: String },
}
```

### `LlmResponseChunk`

A single streaming fragment from the LLM.

```rust
pub struct LlmResponseChunk {
    pub text_delta: Option<String>,
    pub tool_calls_delta: Option<Vec<LlmToolCallChunk>>,
}
```

### `LlmToolCall`

A fully-assembled tool call (from history / non-streaming response).

```rust
pub struct LlmToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}
```

### `LlmToolCallChunk`

A streaming fragment of a tool call. Multiple chunks with the same `index` must be concatenated to form a complete `LlmToolCall`.

```rust
pub struct LlmToolCallChunk {
    pub index: usize,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: Option<String>,
}
```

### `Role`

```rust
pub enum Role {
    System,
    User,
    Assistant,
}
```

Represents the role of a message author in conversation *history* storage (distinct from `LlmMessage`, which is the wire-format sent to providers).

---

## `LlmProviderFactory` and `LlmProviderRegistry`

```rust
pub trait LlmProviderFactory: Send + Sync {
    fn provider_name(&self) -> &str;

    fn create_provider(
        &self,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn LlmProvider>, LlmProviderError>;
}
```

A factory builds a concrete `LlmProvider` from the **live `EneConfig`**, not a raw `serde_json::Value` — the factory itself is responsible for extracting whatever config section it needs (typically via `config.get_section::<ProviderConfig>()`).

```rust
pub struct LlmProviderRegistry { /* opaque */ }

impl LlmProviderRegistry {
    pub fn register(factory: Arc<dyn LlmProviderFactory>);

    pub fn create_provider(
        name: &str,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn LlmProvider>, LlmProviderError>;
}
```

A global, `OnceLock`-backed singleton that maps provider names to registered factories.

| Method | Description |
|---|---|
| `register(factory)` | Registers a factory under `factory.provider_name()`. Takes an `Arc<dyn LlmProviderFactory>`, not a bare value — factories are shared, not owned per-call. |
| `create_provider(name, config)` | Looks up the factory registered under `name` and calls `factory.create_provider(config)`. Returns `LlmProviderError::Provider` if no factory is registered under `name`. |

---

## Configuration Types

Defined via `ene_config::define_config!` (see [`ene-config`](./ene-config.md)); re-exported from this crate.

### `ProviderConfig`

```rust
pub struct ProviderConfig {
    pub name: String,             // default: "openai-compatible"
    pub model: String,            // default: "gpt-4o-mini"
    pub base_url: String,         // default: ""
    pub api_key: ApiKeyConfig,
    pub embedding: EmbeddingConfig,
}
```

| Method | Signature | Description |
|---|---|---|
| `resolve_base_url` | `fn resolve_base_url(&self) -> Result<String, ene_config::ConfigError>` | Returns `base_url` if non-empty; otherwise `Err(ConfigError::MissingBaseUrl { .. })`. |
| `resolve_api_key` | `fn resolve_api_key(&self) -> String` | Resolves the API key per `api_key.source`: `"inline"` uses `api_key.inline` (falling back to the `API_TOKEN` env var in debug builds only); `"env"` reads the env var named by `api_key.env` (defaulting to `"OPENAI_API_KEY"` if unset/blank); any other value behaves like `"inline"`. Returns `""` if nothing resolves — never panics. |

### `ApiKeyConfig`

```rust
pub struct ApiKeyConfig {
    pub source: String,  // default: "inline"
    pub inline: String,  // default: ""
    pub env: String,      // default: "OPENAI_API_KEY"
}
```

### `EmbeddingConfig`

```rust
pub struct EmbeddingConfig {
    pub backend: String,               // default: "cloud"
    pub query_prefix: Option<String>,  // default: None
    pub cloud: CloudEmbeddingConfig,
    pub local: LocalEmbeddingConfig,
}
```

`backend` selects between `"cloud"` (uses the same LLM provider's embeddings API) and `"local"` (uses a local GGUF model via [`ene-embedding`](./ene-embedding.md)).

### `CloudEmbeddingConfig` / `LocalEmbeddingConfig`

```rust
pub struct CloudEmbeddingConfig {
    pub model: String,       // default: "text-embedding-3-small"
    pub dimensions: usize,   // default: 1536
}

pub struct LocalEmbeddingConfig {
    pub model: String,         // default: "jina-embeddings-v5-text-small"
    pub quantization: String,  // default: "F16"
}
```

---

## Errors

### `LlmProviderError`

Errors returned by `LlmProvider` implementations at the library boundary.

```rust
pub enum LlmProviderError {
    Auth(String),
    RateLimit(String),
    Network(String),
    Truncated { reason: String, partial_chars: usize },
    ContentFilter(String),
    Provider(String),
}
```

| Variant | Meaning |
|---|---|
| `Auth(String)` | The provider rejected the credentials (typically HTTP 401/403). |
| `RateLimit(String)` | The provider throttled this request (typically HTTP 429). |
| `Network(String)` | A network-level failure (connect refused, DNS, TLS, read timeout) prevented the request from completing — distinct from `Provider`, which is for HTTP-level errors *with* a response. |
| `Truncated { reason, partial_chars }` | The response was cut off because the configured token limit was reached (`finish_reason=length`). `partial_chars` is how much text was returned before the cut, useful for diagnostics. |
| `ContentFilter(String)` | The provider blocked the response (typically `finish_reason=content_filter`); no usable text was returned. |
| `Provider(String)` | Catch-all for provider-specific errors that don't map to the categories above. |

`map_openai_error` (crate-internal) maps `async_openai::error::OpenAIError` into these variants by HTTP status code: 401/403 → `Auth`, 429 → `RateLimit`, other API errors → `Provider`, transport/stream errors → `Network`.

### `EmbeddingError`

```rust
pub enum EmbeddingError {
    Init(String),
    Provider(String),
    EmptyInput,
}
```

| Variant | Meaning |
|---|---|
| `Init(String)` | The embedding model failed to initialize (e.g. GGUF load error). Distinct from `Provider`, which is for transport/API errors. |
| `Provider(String)` | The provider returned a malformed or empty response, or a transport error (HTTP 4xx/5xx, network failure) prevented the request. |
| `EmptyInput` | The supplied text is empty or whitespace-only. Providers refuse to embed it — returning a zero vector would have undefined cosine similarity and silently pollute the store. |

---

## Built-in Implementations

### `OpenAiProvider`

Communicates with OpenAI-compatible HTTP APIs (OpenAI, Azure, local proxies) via `async-openai`. Supports streaming and structured JSON output.

```rust
pub struct OpenAiProvider { /* opaque */ }

impl OpenAiProvider {
    pub fn new(base_url: &str, api_key: &str, model: &str) -> Self;
}
```

`new` builds the underlying `async_openai::Client` immediately; if `base_url` is non-empty it overrides the client's default API base.

### `OpenAiProviderFactory`

```rust
pub struct OpenAiProviderFactory;

impl LlmProviderFactory for OpenAiProviderFactory { /* ... */ }
```

Registered under the name `"openai-compatible"`. Its `create_provider` reads `ProviderConfig` from the passed `EneConfig`, resolves the base URL and API key, and constructs an `OpenAiProvider`.

### `CloudEmbeddingProvider`

An `EmbeddingProvider` that delegates to a cloud embeddings API (OpenAI-compatible). Suitable for production use with higher throughput than the local GGUF provider.

```rust
pub struct CloudEmbeddingProvider { /* opaque */ }

impl CloudEmbeddingProvider {
    pub fn new(
        base_url: &str,
        api_key: &str,
        embedding_model: &str,
        embedding_dimensions: usize,
        query_prefix: Option<String>,
    ) -> Self;

    pub fn with_hyde_model(self, model: String) -> Self;
}
```

| Method | Description |
|---|---|
| `new(...)` | Builds the client and stores `embedding_model`/`embedding_dimensions`/`query_prefix`. `query_prefix` (if set) is prepended exactly once to `Query`-kind text — never to other kinds, and never twice (`embed_query` calls `embed` rather than re-applying the prefix itself). |
| `with_hyde_model(model)` | Builder method. When set, `hyde()` calls the given chat model to generate a real hypothetical document instead of echoing the query back. |

`embed_batch` fails loudly with `EmbeddingError::Provider` if the API returns a different number of embeddings than inputs, rather than silently truncating.

### `HybridRerankProvider`

Wraps a primary `EmbeddingProvider` and adds **optional** LLM-backed HyDE and rerank steps on top.

```rust
pub struct HybridRerankProvider { /* opaque */ }

impl HybridRerankProvider {
    pub fn new(embedder: Arc<dyn EmbeddingProvider>) -> Self;

    pub fn with_llm(
        self,
        hyde_llm: Option<Arc<dyn LlmProvider>>,
        rerank_llm: Option<Arc<dyn LlmProvider>>,
    ) -> Self;
}
```

| Method | Description |
|---|---|
| `new(embedder)` | Wraps `embedder` for all `embed`/`embed_query`/`embed_batch` calls. `hyde()` and `rerank()` fall back to the trait defaults (echo query / cosine similarity) until an LLM is attached. |
| `with_llm(hyde_llm, rerank_llm)` | Attaches separate LLM providers for HyDE generation and rerank scoring. Each is independently optional — pass `None` to keep the default fallback for that task. The two are deliberately separate `Arc<dyn LlmProvider>` instances (not a shared provider + model-name pair) because a model name alone can't override the model an already-constructed provider talks to on the wire. |

`rerank()`, when `rerank_llm` is set, prompts the LLM with all candidates and asks for a JSON array of `0.0..=1.0` scores (`{"scores": [...]}`) in the same order as `candidates`; a malformed or wrong-length response is a typed `EmbeddingError::Provider`, not a silent all-zero fallback. `has_reranker()` returns `true` exactly when `rerank_llm` is `Some`.

---

## Usage

### Streaming a chat turn

```rust,no_run
use ene_provider::{LlmMessage, LlmProvider, UserMessagePart};
use futures::StreamExt;

async fn stream_reply(provider: &dyn LlmProvider) -> Result<(), Box<dyn std::error::Error>> {
    let messages = vec![
        LlmMessage::System { content: "You are a helpful assistant.".into() },
        LlmMessage::User {
            parts: vec![UserMessagePart::Text {
                text: "What is the capital of France?".into(),
            }],
        },
    ];

    let mut stream = provider.create_chat_stream(&messages, &[]).await?;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if let Some(delta) = chunk.text_delta {
            print!("{delta}");
        }
    }
    println!();
    Ok(())
}
```

### Embedding a query

```rust,no_run
use ene_provider::EmbeddingProvider;

async fn embed_query(provider: &dyn EmbeddingProvider) -> Result<Vec<f32>, ene_provider::EmbeddingError> {
    provider.embed_query("recent conversations about Rust").await
}
```

### Structured completion

```rust,no_run
use ene_provider::{LlmMessage, LlmProvider};

async fn summarize(provider: &dyn LlmProvider, messages: &[LlmMessage]) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "summary": { "type": "string" },
            "key_facts": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["summary", "key_facts"]
    });

    let json_str = provider.chat_completion(messages, Some(schema)).await?;
    Ok(serde_json::from_str(&json_str)?)
}
```

### Registering and using a factory

```rust,no_run
use ene_provider::{LlmProviderRegistry, OpenAiProviderFactory};
use std::sync::Arc;

fn setup(config: &ene_config::EneConfig) -> Result<Box<dyn ene_provider::LlmProvider>, ene_provider::LlmProviderError> {
    LlmProviderRegistry::register(Arc::new(OpenAiProviderFactory));
    LlmProviderRegistry::create_provider("openai-compatible", config)
}
```

---

## See Also

- [`ene-core`](./ene-core.md) — Runtime that drives providers
- [`ene-embedding`](./ene-embedding.md) — Local GGUF embedding provider (implements `EmbeddingProvider`)
- [`ene-config`](./ene-config.md) — `EneConfig`, `define_config!`
- [`ene-memory`](./ene-memory.md) — Consumes embeddings for vector search
