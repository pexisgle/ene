# `ene-ai` & `ene-ai-local` — API Reference

> **Crates**: `ene-ai` (Core traits & cloud providers) | `ene-ai-local` (GGUF local LLM inference via `llama-cpp-2`)

Together, `ene-ai` and `ene-ai-local` provide LLM chat completions and text embedding abstractions for Ene.

---

## 1. `ene-ai` (Core Provider Library)

### Core Traits
- `LlmProvider`: Asynchronous LLM chat completion trait supporting streaming token generation.
- `EmbeddingProvider`: Text embedding vector generation trait.

### Implementations
- `OpenAiProvider`: Cloud provider interface for OpenAI models (GPT-4o, text-embedding-3).
- `IpcLlmProvider`: Host adapter translating IPC protocol v4 messages from provider plugins (such as `ene-plugin-anthropic`) to the `LlmProvider` trait.
- `LlmProviderRegistry`: Factory and registry for instantiating providers based on settings.

---

## 2. `ene-ai-local` (Local GGUF LLM Inference)

`ene-ai-local` houses local model execution wrapping `llama-cpp-2`:

- **Local Model Loading**: Loads `.gguf` weight files from disk.
- **Hardware Acceleration**: Automatically selects CUDA, Metal, or CPU backends via `llama-cpp-2` bindings.
- **In-Process Inference**: Exposes `LocalLlmProvider` implementing `LlmProvider` without external network calls.

---

## Related Links
- [Configuration Reference](../configuration.md)
- [System Architecture](../architecture.md)
