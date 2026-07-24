# Architecture Overview

ene is a modular Rust workspace centered on the API v1 host contract (`ene-runtime`) and the `ene-mind` cognitive turn pipeline.

## Runtime Architecture

The actor model remains the execution shell (`EneHandle` / actor), while turn intelligence is owned by `ene-mind`.

### Core Turn Flow

```text
User input
  -> before_turn (recall planning + affect update; parallel with Tool RAG / style / scene prefetch)
  -> compose_prompt_packet (sectioned context + budgeting; parallel with pre-turn affect persist)
  -> LLM streaming
  -> output arbitration (Performance cues)
  -> finalize_turn (affect persist; synchronous)
  -> commit session history
  -> Terminal (chat event)
  -> deferred write_memories + forgetting + affect classifier (background)
```

`ene-runtime` integrates this flow and emits a **minimal** chat event bus; diagnostics are separate.

## Target Crate Map (API v1)

| Crate | Role |
|---|---|
| `ene-runtime` | Ready `EneHandle::open`, `TurnId`, single-flight Busy, chat events, diagnostics facade |
| `ene-mind` | Identity, typed memory policy, affect, Performance arbitration, compression, session state |
| `ene-store` | SQLite-vec persistence only (`store.enabled` / `store.db_path`) |
| `ene-ai` | LLM + batch-only embedding providers |
| `ene-plugin-proto` / `ene-plugin` / `ene-plugin-host` | Wire plugin ABI (v3), authoring facade, and process/registry orchestration |
| `ene-config` | Settings, character cards, paths |
| `ene-vrm` | VRM rendering (no mind/runtime dependency) |

See [API v1](api-v1.md) for locked decisions and the dependency graph.

## Memory Model

Typed memory (`episodic`, `semantic`, `preference`, `commitment`, …) with lifecycle statuses. The commitment ledger is the sole source of truth for commitments. Hybrid recall (vector + lexical + recency + salience) is executed by **mind**; **store** accepts text / optional precomputed vectors / filters only.

## Prompt Model

Prompt construction is sectioned (`PromptPacket`) with explicit budgets. Identity and output-contract sections are protected under budget pressure.

## Emotion and Performance

- Affect state is persisted engine-side.
- Final presentation cues are emitted as `EneEvent::Performance` (not standalone `SpecialToken` / `Expression`).
- `PerformanceCue` is owned by `ene-mind`; desktop maps cues to VRM playback without importing mind types into `ene-vrm`.

## Applications

- `ene-cli`: `ConfigStore::try_load` → card → `EneHandle::open`; REPL + diagnostics commands.
- `ene-desktop`: soft config load when needed → `open`; VRM + Performance consumption.

## Plugin System

The unified plugin system is the single out-of-process extension mechanism. All tool binaries (`plugins/tool/*`), LLM provider plugins (`plugins/ene-plugin-*`), and MCP servers are managed by one `PluginHostManager` over IPC protocol v3.

### Crates

| Crate | Role |
|---|---|
| `ene-plugin-proto` | Wire protocol v3: `PluginCapabilities`, `LlmProviderSpec`, tool types (`tool_types`), streaming IPC messages |
| `ene-plugin` | Authoring facade: `Plugin` trait, `ToolPluginAdapter` (wraps `ToolProvider`), `run_plugin_server` entry point |
| `ene-plugin-host` | Host-side: `PluginHostManager` (process supervision, MCP, circuit breaker, health), `ToolRegistry`/`CompositeToolRegistry`, `IpcLlmProvider`, `IpcLlmProviderFactory` |

### IPC Protocol v3

Plugin IPC uses the same 4-byte little-endian length-prefixed JSON framing as the legacy tool IPC, extended with streaming and a richer handshake:

- **Handshake**: host sends `Handshake { version: 3, plugin_config }`, plugin responds with `HandshakeAck { version, capabilities }`.
- **Streaming**: 1 request → N `StreamChunk` → terminal `StreamEnd` or `StreamError`, correlated by `request_id`.
- **Capabilities**: `PluginCapabilities` declares `tools`, `llm_providers`, and future `tts_providers` / `stt_providers`.

Tool binaries are plugins too: a `ToolProvider` is wrapped with `ToolPluginAdapter` and served via `run_plugin_server`, advertising its tool specs through `capabilities.tools`.

### Process Supervision

`PluginHostManager` is the sole manager for all plugins:

- Discovers plugin binaries from `builtin_plugins_dir()` + `user_plugins_dir()` (naming: `ene-plugin-{name}`)
- Spawns each as a child process with `ENE_PLUGIN_SOCKET` env var
- Performs v3 handshake and inspects capabilities
- Routes `capabilities.tools` → `ToolRegistry` adapter, `capabilities.llm_providers` → `IpcLlmProviderFactory`
- Connects configured MCP servers (`plugins.mcp_servers`) and merges them into the composite tool registry
- Periodic health probes with circuit breaker and exponential backoff restarts (max 5)

### LLM Provider Integration

Plugin-provided LLM providers integrate via the global `LlmProviderRegistry`:

1. `PluginHostManager::start` registers `IpcLlmProviderFactory` for each advertised `llm_providers` kind.
2. `EneHandle::open` merges these factories into `LlmProviderRegistry`.
3. `resolve.rs` routes non-`openai_compatible` provider kinds through the registry.
4. `IpcLlmProvider` bridges the IPC streaming protocol to the `LlmProvider` trait.

### Configuration

The `plugins` config section (`plugins.enabled`, `plugins.list.<name>.enable`) controls the system. See [Settings](../configuration/settings.md#plugins--plugin-system).

## Reference

- [API v1 ADR](api-v1.md)
- [Cognitive Runtime ADR](cognitive-runtime.md)
- [Avatar Performance ADR](avatar-performance.md)
- [Proactive Companion Speech ADR](proactive-speech.md)
