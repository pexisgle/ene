# `ene-ai` & `ene-ai-local`

> **Crates**: `ene-ai` (provider traits, message/streaming types, OpenAI-compatible provider, registry) | `ene-ai-local` (local GGUF inference via `llama-cpp-4`)

Together, `ene-ai` and `ene-ai-local` provide LLM chat-completion and text-embedding abstractions for Ene. `ene-ai` defines the generic message/streaming types and provider traits plus a global provider registry and the built-in OpenAI-compatible implementation; local inference (GGUF/llama.cpp) lives in `ene-ai-local`, and local audio (STT/TTS/VAD) lives in the separate `ene-voice` crate.

---

## Architectural boundaries

- `ene-ai` owns the provider abstraction layer: message/streaming types, the provider traits, health monitoring/failover routing, and retry policy. It has no persistence or cognitive-logic dependency.
- Out-of-process LLM providers (e.g. an Anthropic plugin) are bridged into the same `LlmProvider` trait via an IPC adapter owned by `ene-plugin-host`, not by `ene-ai` itself — `ene-ai` only defines the trait the adapter implements.
- `ene-ai-local` depends on `ene-ai` (to implement its provider traits) and `ene-config`, and performs in-process inference — no network calls.

## Design rationale

- **Why a provider trait instead of a concrete client type**: `LlmProvider`/`EmbeddingProvider` let cloud providers (OpenAI-compatible), local GGUF inference, and out-of-process plugin providers (via IPC) all satisfy the same interface, so `ene-mind`/`ene-runtime` code that streams a completion or embeds text does not need to know which backend is serving the request.
- **Why local inference is a separate crate**: `llama-cpp-4` pulls in GPU backend build complexity (`vulkan`/`cuda` Cargo features) that cloud-only deployments don't need; splitting it out keeps `ene-ai` lightweight for consumers that only use remote providers.

## API reference

Struct and method signatures are not duplicated here — they drift. Generate rustdoc for the authoritative, current API:

```sh
cargo doc -p ene-ai --open
cargo doc -p ene-ai-local --open
```

Start at the `LlmProvider` / `EmbeddingProvider` traits in `ene-ai`.

---

## Related
- [Configuration Reference](../configuration.md)
- [System Architecture](../architecture.md)
