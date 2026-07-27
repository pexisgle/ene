# IPC Plugin System & MCP Integration

This document covers Ene's out-of-process IPC plugin architecture, Protocol v4 wire specs, Model Context Protocol (MCP) server integration, and built-in tool plugins.

---

## 1. Out-of-Process Plugin Architecture

To guarantee process isolation, stability, and security, all external capabilities—tool plugins, custom LLM providers, and MCP servers—run as independent sub-processes managed by `PluginHostManager` (`ene-plugin-host`).

```text
Ene Host Application (ene-runtime)
  │
  └── PluginHostManager (ene-plugin-host)
        │
        ├── IPC Protocol v4 (Length-prefixed JSON over stdio)
        │     ├── ene-plugin-anthropic (Anthropic LLM Provider Plugin)
        │     ├── ene-plugin-app       (GUI Launcher Tool)
        │     ├── ene-plugin-browser   (CDP Browser Automation Tool)
        │     ├── ene-plugin-fs        (Sandboxed Filesystem Tool)
        │     ├── ene-plugin-utility   (Calculator & Todo Tool)
        │     └── ene-plugin-web       (Web Search & Scraper Tool)
        │
        └── Model Context Protocol (MCP) Bridge (ene-connector)
              └── External MCP Servers (Node.js / Python / Go MCP processes)
```

---

## 2. IPC Protocol v4 Specification

Plugins communicate over `stdin`/`stdout` using **IPC Protocol v4**:

- **Framing**: Every packet begins with a 4-byte little-endian `u32` payload size followed by UTF-8 JSON.
- **Handshake Negotiation**: The host sends `PluginIpcRequest::Handshake { version: VersionRange::host_supported() }`, i.e. `VersionRange { min: 3, max: 4 }` — not a single pinned value. The plugin intersects that range with its own supported range via `VersionRange::negotiate` and responds with `HandshakeAck { version, capabilities: PluginCapabilities }`, where `version` is the highest version common to both sides.
- **Request Correlation**: All async requests and responses include a mandatory `request_id` (`Uuid`).
- **Capabilities**: Plugins advertise supported capabilities (`tools`, `llm_providers`, `stt_providers`, `tts_providers`).

### Versioning policy (N-1 backward compatibility)

Tool and provider plugins ship as independent out-of-process binaries. Bumping `PLUGIN_IPC_PROTOCOL_VERSION` does not recompile plugin binaries that are already installed, so the host maintains **one version of backward compatibility**:

- The host always advertises `[PLUGIN_IPC_MIN_SUPPORTED_VERSION, PLUGIN_IPC_PROTOCOL_VERSION]` during the handshake (`VersionRange::host_supported()` in `crates/ene-plugin-proto/src/ipc.rs`), rather than a single pinned version. A plugin built against the previous protocol version can still connect and negotiate at that older version.
- A plugin binary is not required to support a range — it may keep declaring `VersionRange { min: N, max: N }` for whatever version it was built against. The compatibility responsibility is concentrated in the host, not pushed onto every plugin author.
- **Bumping the protocol version**: when `PLUGIN_IPC_PROTOCOL_VERSION` is bumped, `PLUGIN_IPC_MIN_SUPPORTED_VERSION` must be bumped by the same amount, dropping support for the oldest previously-supported version.
- **When a bump is required**: only for changing the meaning of an existing message, adding a required field, or removing/renaming an enum variant. New fields should use `#[serde(default)]` so older/newer peers stay wire-compatible without a version bump.
- **Feature gating**: the host stores the negotiated version on `IpcPluginConnection` (`ene-plugin-host`) and exposes it via `negotiated_version()`. Behavior that depends on a message introduced after the minimum supported version should gate on it — e.g. `supports_cancel_stream()` gates `PluginIpcRequest::CancelStream` (introduced in v4) so a v3 plugin isn't sent a message it cannot deserialize; the connection falls back to its existing timeout-based stream termination instead.
- **Negotiation failure diagnostics**: when a plugin's proposed range and the host's supported range do not overlap, the plugin's `HandshakeAck` error and the host's `PluginHostError::HandshakeFailed` / `ProtocolMismatch` both name the ranges on both sides (e.g. "host supports 3..=4, plugin supports 2..=2"), so a developer can tell the plugin binary needs rebuilding rather than seeing a generic handshake failure.

---

## 3. Built-In Plugin Catalog

| Plugin Binary | Namespace | Description | Stateful? |
|---|---|---|---|
| `ene-plugin-app` | `app.*` | System application launcher & window control | No |
| `ene-plugin-browser` | `browser.*` | Headless Chrome/CDP web browser automation | Yes (Session store) |
| `ene-plugin-fs` | `fs.*` | Sandboxed filesystem operations with undo ledger | Yes (DB IPC socket) |
| `ene-plugin-utility` | `utility.*` | Calculator, datetime, active todo list manager | Yes (DB IPC socket) |
| `ene-plugin-web` | `web.*` | Web search and markdown page scraper | No |
| `ene-plugin-anthropic` | Provider | Anthropic Claude provider plugin | No |

---

## 4. MCP (Model Context Protocol) Integration

`ene-connector` and `ene-plugin-host` seamlessly integrate external MCP servers:

1. **Discovery & Launch**: Host reads `plugins.mcp_servers` configuration and spawns target MCP server binaries over `stdio` or HTTP/SSE.
2. **Tool Translation**: MCP tools are automatically translated into `ToolSpec` items and registered into the `CompositeToolRegistry`.
3. **Execution Routing**: Tool calls generated by the LLM are routed through the MCP bridge and returned cleanly to `ene-runtime`.

---

## 5. Writing a Custom Tool Plugin

Developers can quickly author new tool plugins using `ene-plugin` and `#[derive(ToolAction)]`:

```rust
use ene_plugin::prelude::*;

#[derive(Debug, Deserialize, ToolAction)]
#[tool_action(name = "custom.greet", description = "Generates a personalized greeting.")]
pub struct GreetAction {
    pub name: String,
}

impl GreetAction {
    pub async fn run(&self) -> Result<String, ToolError> {
        Ok(format!("Hello, {}!", self.name))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = ActionSetProvider::new().register::<GreetAction>();
    run_plugin_server(Box::new(ToolPluginAdapter(provider))).await?;
    Ok(())
}
```
