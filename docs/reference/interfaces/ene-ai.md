# `ene-ai` interface

## Role

The AI provider layer: generic message/streaming types, provider traits,
task routing, retry/failover policy, context-window math, and model fetching.
Concrete providers ship as plugins; `ene-ai` defines the contracts.

## Public modules

| Module | Contents |
|---|---|
| `traits` | `LlmProvider`, `EmbeddingProvider`, `TtsProvider`, `SttProvider`, `VadEngine`, `ProviderHost`, factory traits, `cosine_similarity`, `embed`/`embed_query` |
| `message` | `LlmMessage`, `LlmResponseChunk`, `LlmToolCall(Chunk)`, `LlmCompletion`, `UserMessagePart` |
| `config` | `AiConfig`, `AiProviderDef`, `AiTasksConfig`, `ApiKeyConfig`, `LocalModelDef`, `RetryConfig`, `FallbackConfig`, `SttConfig`, `TtsConfig`, `VadConfig`, `BUILTIN_PROVIDER_KINDS`, `canonical_provider_kind` |
| `resolve` | `ResolvedChat`, `ResolvedEmbedding`, `ResolvedTts/Stt/Vad`, `probe_provider_health`, `probe_chat_candidates`, `select_healthy_chat`, `validate_settings`, `validate_api_key`, `fetch_model_ids`, `needs_onboarding` |
| `routing` | `AiTaskKind`, `create_chat_provider_for_task`, `create_task_chat_provider` |
| `retry` | `RetryPolicy` |
| `context_window` | `effective_window`, `EffectiveWindow`, `DEFAULT_CONTEXT_WINDOW` |
| `model_fetch` | `ModelFetcher`, `ModelValidator` variants, `validate_https_url` |
| `engine_adapter` | `LocalLlmEngine`, `LocalTtsEngine`, `LocalSttEngine`, `EngineDescriptor`, `ResourceRegistry`, `ResourceClass`, `CapabilitySet` (bridge over `ene-infer`) |
| `plugin_config` | Provider-specific settings relocated into `plugins.list.<name>` |
| `error` / `role` | `AiError`, `LlmProviderError`; `Role` (User/Assistant/System/Tool) |

## Key re-exports

- `TokenUsage` re-exported from `ene-plugin-proto` so in-process providers,
  the IPC bridge, and the wire format share one definition.

## Dependencies

- Depends on: `ene-config`, `ene-infer`, `ene-plugin-proto`.
- Used by: `ene-mind`, `ene-runtime`, `ene-plugin-host`, `ene-voice`,
  provider plugins, `ene-cli`, `ene-desktop`.

## Refactoring notes

- `ProviderHost` is the **registry seam**: `ene-plugin-host` implements it;
  `resolve`/`routing` consult it instead of owning providers. Task→provider
  binding and failover policy live here, not in the host.
- Adding a provider kind means: a provider plugin, a builtin kind entry
  (`BUILTIN_PROVIDER_KINDS`), and any kind-specific config. Typo
  suggestions are part of the UX contract.
- The `engine_adapter` module bridges `ene-infer`'s synchronous local-model
  framework into async provider traits — keep local inference flowing
  through it rather than hand-rolling concurrency in providers.
- `effective_window` logic (advertised vs configured window, response
  reserve, safety margin) is shared by mind's budget calculations; change
  with tests.
