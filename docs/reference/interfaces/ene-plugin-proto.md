# `ene-plugin-proto` interface

## Role

The **wire ABI**: plugin IPC protocol v7, tool types, capability
declarations, host-service framing, and transport. Business logic never
lives here.

## Public modules

| Module | Contents |
|---|---|
| `ipc` | `PLUGIN_IPC_PROTOCOL_VERSION` (7), `PLUGIN_IPC_MIN_SUPPORTED_VERSION`, `VersionRange`, `PluginIpcRequest`/`PluginIpcResponse`, framing helpers (`read/write_plugin_request/response`), `ConfigFieldError`, `ConfigOption`, `VadEvent` |
| `tool_ipc` | Tool IPC v2: `IpcRequest`/`IpcResponse`, `CallContext`, `DeferredStatus`, `ToolConfigAccessor`, `IPC_PROTOCOL_VERSION` |
| `tool_types` | `ToolSpec`, `ToolCategory`, `ToolName`, `ToolRagProfile`, `ToolResult` |
| `tool_error` | `ToolError`, `ErrorKind`, interactive prompt types (`UserInputPrompt`, `QuestionItem`, `MultiAnswer`) |
| `tool_provider` | `ToolProvider` trait (legacy tool-binary contract) |
| `capabilities` | `PluginCapabilities`, `LlmProviderSpec`, `TtsProviderSpec`, `SttProviderSpec`, `VadProviderSpec`, `CapabilityRef`, `CapabilityRequirement`, `ConcurrencyHint`, `ResourceClass`, `DEFAULT_SAMPLE_RATE` |
| `capability_service` | `CapabilityCall(Result/Error)`, `CapabilityServiceHandler`, passenger framing helpers |
| `host_service` | `HostServiceId`, `HostServiceRequest/Response`, `HOST_SERVICE_MAX_MESSAGE_SIZE`, framing helpers |
| `sandbox` | `SandboxConfigData` |
| `transport` | `IpcStream`, `IpcListener`, `cleanup_path` (UDS / named pipes) |
| `usage` | `TokenUsage` |
| `error` | `PluginError`, `ProviderErrorKind` |

## Dependencies

- Depends on: nothing internal (serde, rmp-serde, tokio, thiserror, …).
- Used by: `ene-plugin`, `ene-plugin-host`, `ene-plugin-db`, `ene-store`
  (DB IPC), `ene-ai` (shared `TokenUsage`), `ene-plugin-macros`, provider
  and tool plugins.

## Refactoring notes

- **Additive only.** Prefer `#[serde(default)]` fields and new enum variants
  over renames/removals. Version-gate behaviour on the negotiated protocol
  version (pattern: `supports_vad()`), never on assumptions.
- Host advertises `VersionRange { min: N-1, max: N }`; plugins declare the
  single version they build against. Bumping the protocol means bumping the
  min supported version by the same amount.
- Frames are 4-byte little-endian length prefixes; handshake is JSON, later
  frames use the negotiated wire format (MessagePack for v6+). Keep the
  framing and version-negotiation helpers here — plugins must never
  reimplement them.
- This crate is the wrong home for business logic: anything semantic
  (routing, validation beyond wire shape) belongs in the host or domain
  crates.
