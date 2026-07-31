# Plugin System Crates

> **Crates**: `ene-plugin-proto` (wire protocol) | `ene-plugin` (authoring facade) | `ene-plugin-host` (process supervisor)

This family of crates forms Ene's unified, out-of-process IPC plugin infrastructure: tools, custom LLM/TTS/STT providers, and MCP servers all run as independent sub-processes rather than in-process code.

---

## Architectural boundaries

- `ene-plugin-proto` is wire-protocol concerns only — it must not gain business logic, database access, or AI-provider dependencies. It defines both the tool IPC wire messages and the richer plugin protocol (handshake, capability declarations, streaming LLM messages) plus the cross-platform transport layer (UDS / named pipe framing).
- `ene-plugin` is the authoring facade consumed by plugin binaries; it does not depend on `ene-runtime`, `ene-mind`, or `ene-store`. It is not used by the host.
- `ene-plugin-host` is host-side only: process discovery/spawning, handshake negotiation, capability routing into the tool and LLM provider registries, health probes, and shutdown. It bridges plugin-provided LLM providers into `ene_ai::LlmProvider` via an IPC adapter, and aggregates plugin-provided and MCP tools behind a single tool-registry interface.
- Keep `plugins/tool/*` and `plugins/provider/*` binaries lightweight — they depend on `ene-plugin`, not on arbitrary cross-crate business logic.

## Design rationale

- **Why out-of-process plugins instead of dynamic loading or in-process trait objects**: process isolation means a crashing or misbehaving tool/provider cannot take down the host, and each plugin can be sandboxed, restarted, or version-mismatched independently. The cost is IPC framing and a handshake protocol, which `ene-plugin-proto` centralizes so it isn't reimplemented per plugin.
- **Why a versioned handshake (`VersionRange` negotiation)** rather than a fixed protocol version: it lets the host and a plugin binary compiled against an older/newer `ene-plugin-proto` still agree on a common protocol version instead of hard-failing on any mismatch.
- **Why a circuit breaker in `ene-plugin-host`**: a plugin process that fails repeatedly (e.g. a misconfigured provider) would otherwise be retried on every call; the breaker fails fast after a threshold of consecutive failures and cools down before retrying, instead of hammering a broken process.
- **Why control broadcasts are concurrent and permission approvals are routed**: `CompositeToolRegistry` control methods (`set_call_context`, `allow_pattern`, `revoke_pattern`, and the fallback `approve_permission`) fan out to independent plugin connections, so they run concurrently with `join_all` — the worst-case latency is the slowest single plugin, not the sum over every plugin. Permission approvals go further: a request originates from one tool call, so `approve_permission_for` routes the approval directly to the owning sub-registry in a single round-trip instead of broadcasting (#434). This keeps a user's "allow" from being delayed behind an unrelated plugin that is mid-long-tool-call.
- **Why one supervisor task per plugin instead of a single sequential health loop**: each supervised plugin is monitored by its own independent task, so one plugin's restart backoff (exponential, capped at 30 s) or a slow reconnect can never stall the health monitoring of any other plugin. A single loop that pinged every plugin in turn would let one unhealthy plugin delay probes — and thus detection and restart — for all of them (#432).

## API reference

Struct and method signatures are not duplicated here — they drift. Generate rustdoc for the authoritative, current API:

```sh
cargo doc -p ene-plugin-proto --open
cargo doc -p ene-plugin --open
cargo doc -p ene-plugin-host --open
```

Start at `ene_plugin::run_plugin_server` / `PluginDispatch` for authoring, and `ene_plugin_host::PluginHostManager` / `CompositeToolRegistry` for host-side supervision.

---

## Related
- [Plugins & MCP Concepts](../concepts/plugins-and-mcp.md)
- [Tool SDK Reference](tool-sdk.md)
