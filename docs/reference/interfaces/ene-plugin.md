# `ene-plugin` interface

## Role

Plugin **authoring facade**: the traits plugin binaries implement and the
server entry point that speaks the wire protocol. The one-line import for
plugin authors is `use ene_plugin::prelude::*;`.

## Public modules

| Module | Contents |
|---|---|
| `plugin` | Traits: `ToolPlugin`, `LlmPlugin`, `EmbedPlugin`, `TtsPlugin`, `SttPlugin`, `VadPlugin`, `CapabilityProvider`, `ConfigurablePlugin`; streaming chunk types (`PluginStream`, `PluginStreamChunk`, `PluginCompletion`, `PluginTranscription`) |
| `action` | `ToolAction`, `ToolSpecArgs` |
| `tool_provider` | `ActionSetProvider`, `SingleActionProvider`, `ToolProviderPlugin` (legacy adapter) |
| `server` | `PluginDispatch`, `run_plugin_server` |
| `capability` | `CapabilityClient` (host-service `capability` passenger client) |
| `compat` | Compatibility adapters for legacy `ToolProvider` |
| `prelude` | `prelude::tool` (actions + macros), `prelude::provider` (provider traits + `ene-infer` re-exports), glob re-exports |

## Key re-exports (from `ene-plugin-proto` and `ene-infer`)

- Wire types: `ToolSpec`, `ToolError`, `ToolResult`, `PluginError`,
  `PluginCapabilities`, provider specs, `VersionRange`, `TokenUsage`,
  `DeferredStatus`, `SandboxConfigData`, `IpcListener`/`IpcStream`.
- Local-model discipline: `LocalModel`, `EngineHandle`, `EngineConfig`,
  `JobContext`, `StopReason`, `EngineError` (via `prelude::provider`).

## Dependencies

- Depends on: `ene-plugin-proto`, `ene-infer`, `ene-plugin-macros`.
- Used by: every plugin binary (`plugins/tool/*`, `plugins/provider/*`);
  `ene-plugin-host` tests (dev).

## Refactoring notes

- The **prelude is the contract** for authors. Re-exports there are the
  supported surface; adding to it is additive, removing from it breaks
  every plugin.
- `PluginDispatch::new` takes five positional implementations
  (tool, llm, embed, tts, stt); VAD and capability mediation are builder
  steps (`with_vad`, `with_capability_provider`,
  `with_capability_declarations`). Do not grow the positional list.
- `ene-infer` is re-exported so local-inference plugins use the host's own
  concurrency discipline instead of hand-rolled `spawn_blocking`.
- Plugin crates are binary-only; keep the facade traits the only library
  surface plugin code compiles against.
