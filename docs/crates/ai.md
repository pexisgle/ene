# `ene-ai` (provider abstraction) & the local-llm plugin

> **Crates**: `ene-ai` (provider traits, message/streaming types, registries) | `plugins/provider/local-llm` (`ene-plugin-llama-cpp`, local GGUF inference via `llama-cpp-4`)

`ene-ai` provides LLM chat-completion and text-embedding abstractions for Ene: the generic message/streaming types, the provider traits, and the global provider registries. Concrete cloud providers ship as plugin binaries (`plugins/provider/*`) and are bridged into the same traits by `ene-plugin-host`; local inference (GGUF/llama.cpp) lives in the `ene-plugin-llama-cpp` provider plugin, and local audio (STT/TTS/VAD) lives in the `ene-plugin-whisper` / `ene-plugin-onnx` / `ene-plugin-kokoro` provider plugins (sharing engine code from the separate `ene-voice` crate).

---

## Architectural boundaries

- `ene-ai` owns the provider abstraction layer: message/streaming types, the provider traits, health monitoring/failover routing, and retry policy. It has no persistence or cognitive-logic dependency.
- Out-of-process LLM providers (Anthropic, OpenAI, the local-llm plugin) are bridged into the same `LlmProvider` trait via an IPC adapter owned by `ene-plugin-host`, not by `ene-ai` itself — `ene-ai` only defines the trait the adapter implements. Local GGUF inference (kind `"local"`) is one of those plugin backends; the runtime routes proactive decisions, screen vision, and embeddings through the registry exactly like the cloud kinds, so a llama.cpp crash is contained in the plugin process and the host supervises its restart.
- `ene-ai` hosts a shared, safe model-file downloader (`ModelFetcher`, in the `model_fetch` module): in-flight request coalescing, `.part` file + atomic rename, RAII cleanup of partial downloads, HTTPS-only enforcement, progress reporting, and pluggable post-download validation via the `ModelValidator` trait. The local-llm plugin (GGUF weights) and `ene-voice` (Kokoro ONNX model + `voices.bin`) both use it, so this is the one place model downloads are implemented safely rather than each local-inference engine hand-rolling its own.

## Design rationale

- **Why a provider trait instead of a concrete client type**: `LlmProvider`/`EmbeddingProvider` let cloud providers (OpenAI-compatible and Anthropic plugins), local GGUF inference, and out-of-process plugin providers (via IPC) all satisfy the same interface, so `ene-mind`/`ene-runtime` code that streams a completion or embeds text does not need to know which backend is serving the request.
- **Why local inference is a plugin binary**: `llama-cpp-4` pulls in GPU backend build complexity (`vulkan`/`cuda` Cargo features) that cloud-only deployments don't need; isolating it in the plugin keeps `ene-ai` lightweight for consumers that only use remote providers, and the process boundary contains native crashes (see the crash-isolation contract test).

## API reference

Struct and method signatures are not duplicated here — they drift. Generate rustdoc for the authoritative, current API:

```sh
cargo doc -p ene-ai --open
```

Start at the `LlmProvider` / `EmbeddingProvider` traits in `ene-ai`.

---

## Related
- [Configuration Reference](../configuration.md)
- [System Architecture](../architecture.md)
