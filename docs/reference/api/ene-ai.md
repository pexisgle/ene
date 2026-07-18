# `ene-ai` — API Reference

> **Crate:** `ene-ai`
> **Path:** `crates/ene-ai`

`ene-ai` is the unified LLM and embedding provider layer (API v1 merge of the former `ene-provider` + `ene-embedding` crates). Chat completions and embeddings flow through `LlmProvider` and `EmbeddingProvider`. The crate-boundary error is [`AiError`](#aierror); nested provider failures use typed payloads (`LlmProviderError`, `EmbeddingError`).

```mermaid
flowchart LR
    Core[ene-runtime / ene-mind] -->|dyn LlmProvider| LLM[LlmProvider]
    Core -->|dyn EmbeddingProvider| EP[EmbeddingProvider]
    LLM --> OpenAI[OpenAiProvider]
    EP --> Cloud[CloudEmbeddingProvider]
    EP --> Local[GgufEmbeddingProvider]
    EP --> Hybrid[HybridRerankProvider]
```

## `EmbeddingProvider` Trait

**Only `embed_batch` (plus metadata) is on the trait.** Single-text / query helpers are **free functions only** — not trait methods.

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

pub async fn embed(
    provider: &dyn EmbeddingProvider,
    text: &str,
    kind: EmbeddingKind,
) -> Result<Vec<f32>, EmbeddingError>;

pub async fn embed_query(
    provider: &dyn EmbeddingProvider,
    text: &str,
) -> Result<Vec<f32>, EmbeddingError>;
```

| Method / fn | Notes |
|---|---|
| `embed_batch(items)` | Required trait method. Output order matches input. Empty batch → empty `Vec`. Empty/whitespace text → `EmptyInput`. Dim mismatch → `DimensionMismatch`. |
| `dimensions()` / `model_name()` | Provider metadata on the trait. |
| `embed` / `embed_query` | **Free functions only** over `embed_batch` (not methods on the trait). |

**Not** on the trait: `hyde`, `has_reranker`, `rerank`. Those live in pipeline helpers:

| Helper | Location |
|---|---|
| `hyde_document(llm, query)` | `ene_ai::hybrid` |
| `rerank_tool_specs(embedder, llm, query, candidates)` | `ene_ai::hybrid` |
| `HybridRerankProvider::{hyde, rerank, has_reranker}` | Inherent methods |
| `CloudEmbeddingProvider::hyde` | Inherent method |

## Local GGUF (`GgufEmbeddingProvider`)

```rust
pub fn create_local_provider(
    local: &ResolvedLocalModel,
) -> Result<Box<dyn EmbeddingProvider>, EneEmbeddingError>;
```

`ResolvedLocalModel` comes from `AiConfig::resolve_embedding()` when `tasks.embedding.provider` is `"local"`. The GGUF is downloaded into `models/gguf/` on first use (prefetched in parallel during `EneHandle::open` when memory or tool-RAG needs an embedder). Download progress is logged as `[GgufDownload] filename ████████░░ 82% 2.6/3.2 GB`.

## `Role` / `HistoryEntry`

```rust
pub enum Role { System, User, Assistant }
```

Used by the single history type `HistoryEntry { role: Role, content: String }` (owned by `ene-mind`, re-exported by `ene-runtime`). There is no separate `ConversationEntry`.

## `AiError`

Crate-boundary error enum (`thiserror`). Prefer matching on `AiError` at host/mind call sites; nested `LlmProviderError` / `EmbeddingError` remain available as payloads for typed matching.

## Related

- [`ene-mind`](./ene-mind.md) — `HistoryEntry`, recall / compression
- [`ene-tool-host`](./ene-tool-host.md) — Tool RAG uses `hyde_document` / `rerank_tool_specs`
- [API v1 ADR](../architecture/api-v1.md)
