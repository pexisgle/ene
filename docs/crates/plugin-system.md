# Plugin System Crates — API Reference

> **Crates**: `ene-plugin-proto` (Protocol v4 wire messages) | `ene-plugin` (Authoring SDK) | `ene-plugin-host` (Process supervisor)

This family of crates forms Ene's unified, out-of-process IPC plugin infrastructure.

---

## 1. `ene-plugin-proto` (Wire Protocol v4)

`ene-plugin-proto` defines IPC wire types, message enums, framing helpers, and handshake data structures:

- **`PluginIpcRequest` / `PluginIpcResponse`**: Protocol v4 requests/responses.
- **`VersionRange`**: Handshake negotiation (`min: u32, max: u32`).
- **`PluginCapabilities`**: Advertises plugin features (`tools`, `llm_providers`, `stt_providers`, `tts_providers`).
- **`ToolSpec`**: JSON schema definition of a tool and its arguments.

---

## 2. `ene-plugin` (Plugin Authoring SDK)

`ene-plugin` is the facade crate for building new tool and provider plugins:

- **`ToolPluginAdapter`**: Wraps an `ActionSetProvider` or `ToolProvider` into an IPC plugin.
- **`run_plugin_server`**: Async entry point serving IPC requests on `stdin`/`stdout`.
- **`prelude`**: Convenient exports (`ToolAction`, `ToolError`, `ActionSetProvider`, `run_plugin_server`).

---

## 3. `ene-plugin-host` (Process Supervisor)

`ene-plugin-host` runs host-side supervision of child plugin processes:

- **`PluginHostManager`**: Spawns plugin binaries, performs Protocol v4 handshake negotiations, and manages process lifecycles.
- **Circuit Breaker**: Detects failing plugin processes and applies backoff restarts.
- **`CompositeToolRegistry`**: Merges tool specifications from built-in plugins, out-of-process plugins, and MCP servers into a single search registry.

---

## Related Links
- [Plugins & MCP Concepts](../concepts/plugins-and-mcp.md)
- [Tool SDK Reference](tool-sdk.md)
