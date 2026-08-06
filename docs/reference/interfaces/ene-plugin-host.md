# `ene-plugin-host` interface

## Role

Host-side plugin supervision and capability routing: discovery, spawn,
handshake, health, circuit breaking, IPC provider bridges, MCP client, and
the single provider registry.

## Public modules

| Module | Contents |
|---|---|
| `manager` | `PluginHostManager` (lifecycle, registries, `ProviderHost` impl), factory handle types |
| `tool_registry` | `ToolRegistry` trait, `CompositeToolRegistry`, `DeferredCallResult`, `compute_tool_version_hash` |
| `ipc_plugin` | `IpcPluginConnection` (client to one plugin binary), `SetConfigOutcome` |
| `ipc_provider` / `ipc_stt` / `ipc_tts` / `ipc_vad` / `embedding` | IPC-backed provider bridges implementing `ene-ai` traits |
| `factory` / `stt_factory` / `tts_factory` | Provider factory adapters |
| `capability_registry` | `CapabilityRegistry`, `CapabilityDeclaration`, `evaluate_capability_gate` |
| `capability_service` | `CapabilityMediator`, `CapabilityCallHandler` (plugin-to-plugin mediation) |
| `mcp_config` / `mcp_registry` | `McpServerConfig`, `McpTransport`, `McpToolRegistry` |
| `config` | `PluginConfig`, `PluginEntry` (per-plugin enable/config/profiles) |
| `credential_registry` | `CredentialRegistry` (x-ene-credentials resolution) |
| `circuit_breaker` | `CircuitBreaker`, `BreakerState` |
| `health` | `PluginHealthEvent`, `DisabledReason` |
| `redact` | `redact_config`, `redact_config_unschematized` |
| `wav` | Shared WAV encode/decode for provider audio |
| `admission`, `error` | Resource-class admission; `PluginHostError`, `ToolHostError` |

## Dependencies

- Depends on: `ene-plugin-proto`, `ene-ai`, `ene-config`, `ene-connector`.
- Used by: `ene-runtime`, `ene-cli`, `ene-desktop`.

## Refactoring notes

- `PluginHostManager` implements `ene_ai::ProviderHost` — it is the **one
  provider registry** (LLM/embedding/TTS/STT/VAD). Task binding and
  failover stay in `ene-ai`; do not duplicate a registry here or in
  consumers.
- Health probing goes *through* each provider plugin (minimal chat ping),
  not host-side HTTP probing — the plugin owns endpoint knowledge.
- `BUILTIN_PLUGIN_NAMES` and the default plugin list are the discovery
  contract; adding a plugin means updating both plus the packaging scripts.
- The host is the redaction boundary for plugin config values and the
  credential resolution point — secrets must not cross it unredacted.
- MCP child processes inherit no environment except `env_passthrough`;
  HTTP URLs are SSRF-validated before connection. Keep those checks in the
  registry path.
