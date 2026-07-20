# `ene-ai` AI Provider & Local GGUF Inference Specifications

The `ene-ai` crate defines Ene's core abstractions for LLM (Large Language Model) chat completions and embedding providers. It provides wrappers for OpenAI-compatible web APIs and handles local GGUF model files for in-process quantized inference.

---

## 1. Provider Interfaces (Traits)

### `LlmProvider`
*   **Signature**:
    ```rust
    #[async_trait]
    pub trait LlmProvider: Send + Sync {
        async fn stream_chat(
            &self,
            messages: &[LlmMessage],
            tools: &[ToolSpec],
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>, LlmProviderError>;
        fn model_name(&self) -> &str;
    }
    ```
*   **Description**: Takes the compiled message payload and candidate tool list, returning an async token stream (`LlmResponseChunk`).

### `EmbeddingProvider`
*   **Signature**:
    ```rust
    #[async_trait]
    pub trait EmbeddingProvider: Send + Sync {
        async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError>;
        fn model_name(&self) -> &str;
    }
    ```
*   **Description**: Computes dense vector arrays (float arrays representing semantic features) in batches.

---

## 2. Cloud and Local Implementations

### 1. `OpenAiProvider` (Cloud API Integration)
*   **Details**: Wraps `async-openai` and sends requests over HTTPS.
*   **Error Handling**: Translates rate-limit, network, and token validation errors into `LlmProviderError::ApiError`.

### 2. `LocalLlamaCppProvider` (In-Process Llama.cpp)
*   **Details**: Leverages `llama-cpp-2` to bind C++ `llama.cpp` hooks directly in-process. Manages quantized weights, allocates thread contexts, and evaluates token sampling.
*   **Hardware Acceleration**: Integrates with system GPU drivers (CUDA or Metal) dynamically linked via Nix flakes during compiler bootstrapping.

---

## 3. Quantized Model Download Manager (`gguf.rs`)

Manages caching and downloading GGUF weight files (such as `nomic-embed` and lightweight LLMs for decision making).

*   **Target Directory**: `ene_config::paths::models_dir()` (defaults to `~/.gemini/antigravity/models/`).
*   **`ensure_gguf_available`**:
    Checks if the local model file exists and is complete. If not, launches an async downloader using `reqwest`.
*   **`prefetch_configured_gguf`**:
    Invoked during startup. Inspects the `AiConfig` layout and downloads all required embedding, classification, or vision-projection (MMProj) model weight files in parallel.
