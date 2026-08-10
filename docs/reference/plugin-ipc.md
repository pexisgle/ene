# Plugin IPC protocol

This page describes the wire protocol between the host and plugin
binaries. The authoritative implementation is
`crates/ene-plugin-proto/src/ipc.rs`; this page is the readable summary.

## Version

Current protocol version: **8** (`PLUGIN_IPC_PROTOCOL_VERSION`).

- The host advertises `VersionRange { min: N-1, max: N }` (N-1 backward
  compatibility). Plugins should declare the single version they were
  built against.
- The negotiated version is the highest common version; the handshake
  fails when ranges do not overlap.
- v8 added the **Broker channel**: the host-service socket gains `Artifact`,
  `File`, `Network`, `Process`, and `Platform` passengers (plus
  `Credential`), and `SandboxConfigData` carries the broker socket and the
  per-plugin temp directory. Plugins have no direct OS access; every
  operation is mediated.
- v7 added out-of-process VAD (`ProcessVadChunk`); v6 switched frames after
  the handshake from JSON to MessagePack. Hosts gate new request variants
  on the negotiated version, so an older plugin never receives them.

## Framing

Every frame is:

```text
4-byte little-endian length prefix | payload
```

- Handshake exchange: always JSON.
- Frames after the handshake: negotiated `WireFormat` (MessagePack for
  v6+, JSON for v5 and below).
- Maximum message size: 64 MiB.
- All non-streaming and streaming messages carry a `request_id` (UUID)
  for correlation.

## Handshake

```text
host  ── supported VersionRange + handshake request ──▶ plugin
plugin ── negotiated version + PluginCapabilities + ack ──▶ host
```

`PluginCapabilities` advertises:

- tools (count + RAG profiles),
- LLM providers (`LlmProviderSpec`: kind, models, streaming, vision,
  context window, concurrency),
- TTS/STT/VAD provider specs (voices, formats, sample rate, frame size),
- capability declarations (`provides` / `requires`),
- resource class and admission hints.

## Messages

| Class | Examples |
|---|---|
| Tool IPC (v2 lineage) | `ToolCall` / `ToolResult` / `ToolError` (structured, IPC-serializable), `ToolSpec` |
| Plugin IPC (v8) | `CreateChatStream`, `StreamChunk`, `StreamEnd`, `StreamError`, embeddings, `SynthesizeTts`, `Transcribe`, `ProcessVadChunk` |
| Deferred tasks | `DeferredStatus` — background tool completion reported asynchronously |
| Host services | `HostServiceRequest` / `HostServiceResponse` — multiplexed passengers on a shared socket (`db`, `capability`, and the v8 brokers: `file`, `network`, `process`, `credential`, `artifact`, `platform`) |
| Broker channel | `BrokerRequest` / `BrokerResponse` — typed, host-mediated operations; identity is pinned to the authenticated plugin token |
| Capability calls | `CapabilityCall` — plugin-to-plugin mediation through the host |

## Transport

- Unix domain sockets on Unix, named pipes on Windows
  (`IpcListener` / `IpcStream` / `cleanup_path`).
- The host also passes a sandbox config (`SandboxConfigData`) describing
  the plugin's working directory, permission context, and resource limits.

## Host services

The shared host-service socket multiplexes passengers:

| Passenger | Since | Purpose |
|---|---|---|
| `db` | v3 | typed CRUD against `memory.db` (`ene-plugin-db`) |
| `capability` | v6 | plugin-to-plugin capability calls |
| `file`, `network`, `process`, `credential`, `artifact`, `platform` | v8 | the broker channel — host-mediated filesystem, downloads/web, processes, credentials, signed artifacts, and platform features |

Stateful plugins reach the host's database through the **`db` passenger**:
`ene-plugin-db` provides typed CRUD (list/insert/update/delete/search)
over the shared host-service socket. Authentication is a per-plugin token;
each plugin's tables are prefix-isolated.

The **`capability` passenger** mediates plugin-to-plugin capability calls:
the caller's declared `requires` authorize the request, the host resolves
the provider from the capability registry, and forwards over the
provider's connection.

The v8 **broker passengers** are the only way a plugin touches the OS.
`ene-plugin-broker` is the plugin-side client; the host implements the
handlers in `ene-plugin-host` (see
[Sandbox, broker & approvals](../concepts/sandbox-and-approvals.md)).

## Version gates in the host

The host guards new protocol features with `negotiated_version()` checks
(e.g. `supports_vad()`), so a mixed fleet of plugin binaries (v6 and v7)
works against one host. When bumping the protocol, bump the min supported
version by the same amount, which drops support for the now-oldest version.

## Authoring

You do not hand-roll this protocol: plugin binaries use
`ene_plugin::run_plugin_server` with `PluginDispatch` (tool + provider
traits), and the host uses `ene-plugin-host`'s `PluginHostManager`. See
[Tool SDK](tools/sdk.md) and [Derive macros](tools/derive-macro.md).
