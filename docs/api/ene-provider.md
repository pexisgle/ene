# `ene-provider` — API Reference

> **Crate:** `ene-provider`  
> **Role:** Trait definitions and built-in implementations for LLM and embedding providers.

---

## Overview

`ene-provider` defines the provider abstraction layer that decouples the Ene runtime from specific AI service vendors. All LLM calls and embedding operations flow through the two core traits: `LlmProvider` and `EmbeddingProvider`.

Providers are registered at startup via `LlmProviderRegistry` and can be swapped via configuration without changing application code.

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

The core interface for language model backends.

```rust
pub trait LlmProvider: Send + Sync {
    /// Human-readable provider name (e.g. `"openai"`).
    fn name(&self) -> &str;

    /// Opens a streaming chat completion.
    ///
    /// Returns a `Stream` that yields `LlmResponseChunk` fragments as the model generates text.
    fn create_chat_stream(
        &self,
        messages: &[LlmMessage],
        tools: &[ToolSpec],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, String>> + Send>>, String>;

    /// Performs a blocking (non-streaming) chat completion.
    ///
    /// Pass `json_schema` to request structured JSON output conforming to a schema.
    fn chat_completion(
        &self,
        messages: &[LlmMessage],
        json_schema: Option<serde_json::Value>,
    ) -> Result<String, String>;
}
```

### Notes

- `create_chat_stream` is used for all interactive turns where the user sees streamed output.
- `chat_completion` is used for internal tasks such as session summarization that require structured output.

---

## `EmbeddingProvider` Trait

Interface for text embedding and semantic utility operations.

```rust
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a single text string with the given purpose hint.
    fn embed(&self, text: &str, kind: EmbeddingKind) -> Result<Vec<f32>, EmbeddingError>;

    /// Convenience wrapper: embed a query string (`EmbeddingKind::Query`).
    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbeddingError>;

    /// Embed multiple texts in a single batched call.
    fn embed_batch(
        &self,
        items: &[(&str, EmbeddingKind)],
    ) -> Result<Vec<Vec<f32>>, EmbeddingError>;

    /// HyDE: generate a hypothetical answer document for the given query,
    /// then embed it. Returns the embedded vector of the hypothetical document.
    fn hyde(&self, query: &str) -> Result<String, EmbeddingError>;

    /// Score `candidates` (tool specs) against `query` for re-ranking.
    /// Returns a score per candidate in the same order.
    fn rerank(
        &self,
        query: &str,
        candidates: &[ToolSpec],
    ) -> Result<Vec<f32>, EmbeddingError>;

    /// Number of dimensions in the output vectors.
    fn dimensions(&self) -> usize;

    /// Model identifier string.
    fn model_name(&self) -> &str;
}
```

### `EmbeddingKind`

A hint that tells the provider how the text will be used. Some providers (e.g., `e5-mistral`) use different prefixes per kind.

```rust
pub enum EmbeddingKind {
    /// A session or conversation summary.
    Summary,

    /// Descriptive text about a tool or entity.
    Description,

    /// A tool's capability text.
    Capability,

    /// An example interaction.
    Example,

    /// Negative example (for contrastive indexing).
    Negative,

    /// A user search query.
    Query,

    /// A hypothetical document embedding (HyDE).
    Hyde,
}
```

---

## Message Types

### `LlmMessage`

The unified message format sent to any LLM provider.

```rust
pub enum LlmMessage {
    /// Instructions to the model. Typically the character card + injected context.
    System { content: String },

    /// A user message, which may include text and inline images.
    User { parts: Vec<UserMessagePart> },

    /// A previous assistant response, optionally with tool call records.
    Assistant {
        content: Option<String>,
        tool_calls: Option<Vec<LlmToolCall>>,
    },

    /// The result of a tool call, keyed by `tool_call_id`.
    Tool { tool_call_id: String, content: String },
}
```

### `UserMessagePart`

```rust
pub enum UserMessagePart {
    /// Plain text fragment.
    Text { text: String },

    /// Base64-encoded image data (multimodal models).
    Image { base64_image_data: String },
}
```

### `LlmResponseChunk`

A single streaming fragment from the LLM.

```rust
pub struct LlmResponseChunk {
    /// A text fragment, if this chunk contains text.
    pub text_delta: Option<String>,

    /// Tool call fragments, if this chunk contains tool call data.
    pub tool_calls_delta: Option<Vec<LlmToolCallChunk>>,
}
```

### `LlmToolCall`

A fully-assembled tool call (from history / non-streaming response).

```rust
pub struct LlmToolCall {
    /// Unique ID assigned by the LLM for correlation with `Tool` messages.
    pub id: String,

    /// The name of the tool to invoke.
    pub name: String,

    /// JSON string of the tool arguments.
    pub arguments: String,
}
```

### `LlmToolCallChunk`

A streaming fragment of a tool call. Multiple chunks with the same `index` must be concatenated to form a complete `LlmToolCall`.

```rust
pub struct LlmToolCallChunk {
    /// Index identifying which tool call this fragment belongs to.
    pub index: usize,

    /// ID fragment (present in first chunk for a given index).
    pub id: Option<String>,

    /// Name fragment (present in first chunk for a given index).
    pub name: Option<String>,

    /// Arguments fragment (may be split across many chunks).
    pub arguments: Option<String>,
}
```

### `Role`

```rust
pub enum Role {
    User,
    Assistant,
    System,
}
```

---

## Error Types

### `EmbeddingError`

```rust
pub enum EmbeddingError {
    /// The provider returned an error message.
    Provider(String),

    /// The operation timed out after the given duration.
    Timeout(Duration),
}
```

---

## `LlmProviderRegistry`

A global singleton that maps provider names to factory functions.

```rust
// Register a factory for a named provider.
pub fn register(factory: impl LlmProviderFactory + 'static);

// Instantiate a provider by name, passing provider-specific config.
pub fn create_provider(name: &str, config: &serde_json::Value) -> Result<Box<dyn LlmProvider>, String>;
```

---

## Built-in Implementations

### `OpenAiProvider`

Communicates with OpenAI-compatible HTTP APIs. Supports streaming via SSE and structured JSON output.

```rust
pub struct OpenAiProvider { /* opaque */ }
```

Configured via the `[llm]` section of `settings.json`. Supports any OpenAI-compatible endpoint (OpenAI, Azure, local proxies).

### `OpenAiProviderFactory`

Factory type registered in `LlmProviderRegistry` for the `"openai"` key.

### `CloudEmbeddingProvider`

An `EmbeddingProvider` that delegates to a cloud API (e.g. OpenAI Embeddings). Suitable for production use with higher throughput.

```rust
pub struct CloudEmbeddingProvider { /* opaque */ }
```

### `HybridRerankProvider`

Wraps another `EmbeddingProvider` and adds a re-ranking step using cross-encoder style scoring. Used by the tool RAG index to improve tool selection accuracy.

```rust
pub struct HybridRerankProvider { /* opaque */ }
```

---

## Usage Examples

### Streaming a chat turn

```rust
use ene_provider::{LlmMessage, LlmResponseChunk};
use futures::StreamExt;

let messages = vec![
    LlmMessage::System { content: "You are a helpful assistant.".into() },
    LlmMessage::User {
        parts: vec![ene_provider::UserMessagePart::Text {
            text: "What is the capital of France?".into(),
        }],
    },
];

let mut stream = provider.create_chat_stream(&messages, &[])?;

while let Some(chunk) = stream.next().await {
    let chunk = chunk.map_err(|e| anyhow::anyhow!(e))?;
    if let Some(delta) = chunk.text_delta {
        print!("{}", delta);
    }
}
println!();
```

### Embedding a query

```rust
use ene_provider::{EmbeddingProvider, EmbeddingKind};

let query_vec = provider.embed_query("recent conversations about Rust")?;
// query_vec is a Vec<f32> ready for cosine similarity search
```

### Structured completion

```rust
use serde_json::json;

let schema = json!({
    "type": "object",
    "properties": {
        "summary": { "type": "string" },
        "key_facts": { "type": "array", "items": { "type": "string" } }
    },
    "required": ["summary", "key_facts"]
});

let json_str = provider.chat_completion(&messages, Some(schema))?;
let result: serde_json::Value = serde_json::from_str(&json_str)?;
```

---

## See Also

- [`ene-core`](./ene-core.md) — Runtime that drives providers
- [`ene-embedding`](./ene-embedding.md) — Local GGUF embedding provider
- [`ene-memory`](./ene-memory.md) — Consumes embeddings for vector search
