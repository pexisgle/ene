# IPC Plugin System & MCP Integration

This document covers Ene's out-of-process IPC plugin architecture, Protocol v5 wire specs, Model Context Protocol (MCP) server integration, and built-in tool plugins.

---

## 1. Out-of-Process Plugin Architecture

To guarantee process isolation, stability, and security, all external capabilities—tool plugins, custom LLM providers, and MCP servers—run as independent sub-processes managed by `PluginHostManager` (`ene-plugin-host`).

```text
Ene Host Application (ene-runtime)
  │
  └── PluginHostManager (ene-plugin-host)
        │
        ├── IPC Protocol v5 (Length-prefixed JSON over stdio)
        │     ├── ene-plugin-anthropic (Anthropic LLM Provider Plugin)
        │     ├── ene-plugin-app       (GUI Launcher Tool)
        │     ├── ene-plugin-browser   (CDP Browser Automation Tool)
        │     ├── ene-plugin-fs        (Sandboxed Filesystem Tool)
        │     ├── ene-plugin-utility   (Calculator & Todo Tool)
        │     └── ene-plugin-web       (Web Search & Scraper Tool)
        │
        └── Model Context Protocol (MCP) Bridge (ene-plugin-host)
              └── External MCP Servers (Node.js / Python / Go MCP processes)
```

---

## 2. IPC Protocol v5 Specification

Plugins communicate over `stdin`/`stdout` using **IPC Protocol v5**:

- **Framing**: Every packet begins with a 4-byte little-endian `u32` payload size followed by UTF-8 JSON.
- **Handshake Negotiation**: The host sends `PluginIpcRequest::Handshake { version: VersionRange::host_supported() }`, i.e. `VersionRange { min: 4, max: 5 }` — not a single pinned value. The plugin intersects that range with its own supported range via `VersionRange::negotiate` and responds with `HandshakeAck { version, capabilities: PluginCapabilities }`, where `version` is the highest version common to both sides.
- **Handshake Timeout**: The host bounds how long it waits for the `HandshakeAck` (`plugins.handshake_timeout_ms`, default 10 s). A plugin that accepts the socket but never replies fails the handshake with `PluginHostError::HandshakeFailed` instead of blocking startup of the remaining plugins. Plugin authors must answer the handshake promptly and defer heavy initialization (model loading, etc.) until afterwards — see `run_plugin_server` in `ene-plugin`.
- **Request Correlation**: All async requests and responses include a mandatory `request_id` (`Uuid`).
- **Capabilities**: Plugins advertise supported capabilities (`tools`, `llm_providers`, `stt_providers`, `tts_providers`), each provider spec also declaring a `concurrency: ConcurrencyHint` (see [§3](#3-provider-concurrency-concurrencyhint)).

### Versioning policy (N-1 backward compatibility)

Tool and provider plugins ship as independent out-of-process binaries. Bumping `PLUGIN_IPC_PROTOCOL_VERSION` does not recompile plugin binaries that are already installed, so the host maintains **one version of backward compatibility**:

- The host always advertises `[PLUGIN_IPC_MIN_SUPPORTED_VERSION, PLUGIN_IPC_PROTOCOL_VERSION]` during the handshake (`VersionRange::host_supported()` in `crates/ene-plugin-proto/src/ipc.rs`), rather than a single pinned version. A plugin built against the previous protocol version can still connect and negotiate at that older version.
- A plugin binary is not required to support a range — it may keep declaring `VersionRange { min: N, max: N }` for whatever version it was built against. The compatibility responsibility is concentrated in the host, not pushed onto every plugin author.
- **Bumping the protocol version**: when `PLUGIN_IPC_PROTOCOL_VERSION` is bumped, `PLUGIN_IPC_MIN_SUPPORTED_VERSION` must be bumped by the same amount, dropping support for the oldest previously-supported version.
- **When a bump is required**: only for changing the meaning of an existing message, adding a required field, or removing/renaming an enum variant. New fields should use `#[serde(default)]` so older/newer peers stay wire-compatible without a version bump.
- **Feature gating**: the host stores the negotiated version on `IpcPluginConnection` (`ene-plugin-host`) and exposes it via `negotiated_version()`. Behavior that depends on a message introduced after the minimum supported version should gate on it — e.g. `supports_set_config()` gates `PluginIpcRequest::SetConfig` (introduced in v5) so a v4 plugin isn't sent a message it cannot deserialize; the host still updates its local cache so the next reconnect handshake delivers the fresh config. Dynamic-config messages (`ListConfigOptions`, `ValidateConfig`, `MigrateConfig`) also require protocol ≥ v5 **and** the matching `PluginCapabilities` flags (`supports_list_config_options`, etc.; serde-default `false` on older v5 binaries that lack those variants).
- **Negotiation failure diagnostics**: when a plugin's proposed range and the host's supported range do not overlap, the plugin's `HandshakeAck` error and the host's `PluginHostError::HandshakeFailed` / `ProtocolMismatch` both name the ranges on both sides (e.g. "host supports 4..=5, plugin supports 3..=3"), so a developer can tell the plugin binary needs rebuilding rather than seeing a generic handshake failure.

---

## 3. Provider Concurrency (`ConcurrencyHint`)

The process boundary protects the *host* from a misbehaving provider plugin — a bad plugin cannot exhaust the host's tokio blocking pool — but on its own it does nothing to protect the *plugin*: nothing stopped the host from opening unbounded concurrent requests against a single plugin binary, and nothing let a plugin say "I am a local model, run me one at a time." `ConcurrencyHint` closes that gap.

Every entry in `PluginCapabilities` — `LlmProviderSpec`, `TtsProviderSpec`, `SttProviderSpec` — carries a `concurrency: ConcurrencyHint` field:

```rust
pub struct ConcurrencyHint {
    /// Max jobs this provider can run at once.
    pub max_in_flight: u32,
    /// Extra jobs to queue before rejecting.
    pub queue_depth: u32,
}
```

### The default is deliberately serial

`ConcurrencyHint::default()` is `max_in_flight: 1, queue_depth: 2` — one job at a time, a shallow queue behind it. This is the load-bearing design decision, not an oversight: a plugin author who has not thought about concurrency at all gets conservative, safe behavior *because* they did not think about it. A plugin that wants more — typically a stateless HTTP proxy to a cloud API, which can safely serve many requests at once — must set `concurrency` explicitly, and doing so is itself evidence the author considered the question. The built-in Anthropic plugin does exactly this:

```rust
fn llm_capabilities(&self) -> Vec<LlmProviderSpec> {
    vec![LlmProviderSpec {
        kind: "anthropic".to_string(),
        // ...
        concurrency: ConcurrencyHint { max_in_flight: 8, queue_depth: 16 },
    }]
}
```

The field is `#[serde(default)]`, like every other field added to this wire protocol since v3 — see [Versioning policy](#versioning-policy-n-1-backward-compatibility) above. A plugin binary built before `ConcurrencyHint` existed simply omits the field on the wire; the host deserializes the missing field as the safe serial default rather than failing the handshake or defaulting to unlimited concurrency. No protocol version bump was needed to add it.

### Host-side enforcement

`ene-plugin-host`'s `IpcLlmProvider` (`crates/ene-plugin-host/src/ipc_provider.rs`) enforces the declared hint with a `ConcurrencyLimiter`: a `tokio::sync::Semaphore` sized to `max_in_flight`, plus up to `queue_depth` callers allowed to wait for a permit. A request beyond both bounds fails fast with `LlmProviderError::Busy` rather than growing the wait queue without limit — the same fail-fast-over-queue-forever discipline `ene-infer` applies on the local-inference side, applied here at the plugin IPC boundary. The limiter is built once per (plugin, provider kind) in `IpcLlmProviderFactory` and shared across every provider instance created for that pair, since a fresh `IpcLlmProvider` is built per call. For a streaming request, the acquired permit is held for the stream's entire lifetime and releases automatically when the stream is dropped — whether it completed naturally or was cancelled mid-flight.

### Local-inference plugin authors: the same discipline, in-process

`ConcurrencyHint` only bounds how many requests the *host* sends to a plugin at once. A plugin that does its own local inference (llama.cpp, whisper.cpp, a local TTS engine) still has to get concurrency right *inside its own process* — the host cannot see or control how that plugin's code juggles the jobs once they arrive. For that, depend on `ene-plugin`'s `prelude` module, which re-exports `ene-infer`'s worker-thread framework (`EngineConfig`, `EngineError`, `EngineHandle`, `JobContext`, `LocalModel`, `StopReason`) alongside the usual plugin-authoring types:

```rust
use ene_plugin::prelude::*;

struct MyLocalModel { /* ... */ }

impl LocalModel for MyLocalModel {
    type Request = MyRequest;
    type Response = MyResponse;
    type Error = MyModelError;

    fn engine_name(&self) -> &str { "my-local-model" }

    fn run(&mut self, req: Self::Request, ctx: &JobContext) -> Result<Self::Response, Self::Error> {
        // Cooperatively check `ctx.should_stop()` at natural interruption points.
        // ...
    }
}

let handle = EngineHandle::spawn(|| Ok(MyLocalModel::load()?), EngineConfig::default());
```

`EngineHandle` owns the model on a dedicated worker thread, runs jobs off a bounded queue that fails fast (`EngineError::Busy`) instead of growing without limit, and recovers from a panicking job by rebuilding the model — the same guarantees the host itself relies on, reached with one `use` line instead of hand-rolled `spawn_blocking`/`block_in_place` concurrency that would just relocate the bug across the process boundary.

---

## 4. Built-In Plugin Catalog

| Plugin Binary | Namespace | Description | Stateful? |
|---|---|---|---|
| `ene-plugin-app` | `app.*` | System application launcher & window control | No |
| `ene-plugin-browser` | `browser.*` | Headless Chrome/CDP web browser automation | Yes (Session store) |
| `ene-plugin-fs` | `fs.*` | Sandboxed filesystem operations with undo ledger | Yes (host-service `db`) |
| `ene-plugin-utility` | `utility.*` | Calculator, datetime, active todo list manager | Yes (host-service `db`) |
| `ene-plugin-web` | `web.*` | Web search and markdown page scraper | No |
| `ene-plugin-anthropic` | Provider | Anthropic Claude provider plugin | No |

All six plugins above are included in the default `plugins.list` and start
automatically on fresh installs.

---

## 5. Tool Database Schema Declaration & Evolution

Stateful tool plugins (`ene-plugin-fs`, `ene-plugin-utility`) persist their
data into the host's `memory.db` through the shared **host-service** socket
(`ene-host-service.sock` / named pipe). The first framed message opens a
passenger service with a pre-shared token; today only `db` is implemented
(`ene-store`'s `host_service` + `db_server`). Reserved ids (`assets`,
`capability`, `credential`) are rejected until implemented. All plugins share
this one socket, so namespace isolation rests on the per-plugin auth token
alone — the per-plugin socket path layer is gone. A plugin never
issues DDL directly: it
declares its tables, columns, and indexes with a `DeclareSchema` request, and
the host creates and owns the physical tables. Every table name must start
with the plugin's prefix (`fs_`, `utility_`), and every index name must carry
that prefix too (SQLite index names share one database-wide namespace, so an
unprefixed index could squat a name a core migration later needs). All
subsequent requests are validated against the declaration.

### Fingerprint-based change detection

On every `DeclareSchema`, the host hashes the declaration (BLAKE3) and
compares it against the `fingerprint` stored in the internal `__tool_schemas`
table:

| Change | Behavior |
|---|---|
| No change | The stored row is left untouched and the existing tables are reused. Re-declaring an identical schema does **not** rewrite the row needlessly. |
| Column added | Applied in place with `ALTER TABLE ... ADD COLUMN`; existing rows receive the column's `DEFAULT` (or `NULL`). The stored declaration is refreshed. |
| Table added | Created via `CREATE TABLE IF NOT EXISTS`. The stored declaration is refreshed. |
| Index added | Applied via `CREATE INDEX IF NOT EXISTS`. |
| Index name without the tool's prefix | **Rejected** with a permission error (index names share SQLite's database-wide namespace). |
| Column type changed | **Rejected** with a `SCHEMA_CONFLICT` error. |
| Table/column removed | **Rejected** with a `SCHEMA_CONFLICT` error. |
| Column added with `PRIMARY KEY`/`UNIQUE`/`AUTOINCREMENT` | **Rejected** with a `SCHEMA_CONFLICT` error. |
| Column added as `NOT NULL` without a `DEFAULT` | **Rejected** with a `SCHEMA_CONFLICT` error. |

`SQLite` cannot change a column's type, drop columns/tables, or add a
constrained column in place, and adding a `NOT NULL` column requires a
`DEFAULT` to populate pre-existing rows — so rather than letting the
validation layer and the physical tables silently diverge — the #423
symptom, where validation passes but an `INSERT` later fails with
`no such column` — the host rejects incompatible changes and asks the
plugin author to reconcile them explicitly.
Additive changes (plain new columns and tables) are safe and applied
automatically.

### Guidance for plugin authors

- Adding columns or tables is safe and is applied automatically to existing
  databases on the next `DeclareSchema`.
- To change a column's type or remove a table, ship a new prefixed table and
  migrate the data yourself, or reconcile the difference in your plugin's own
  logic. The host will not rewrite or drop data on your behalf.

### Atomic batches (`Batch`)

A plugin that must apply several writes atomically sends a single `Batch`
request carrying a list of `DbWriteOp` (`Insert` / `Upsert` / `Update` /
`Delete`). The server validates every operation against the declared schema up
front, then runs the whole list inside **one SQLite transaction**: either every
operation commits, or — if any operation fails — the entire batch is rolled
back and nothing is persisted. The response carries one `DbBatchOpResult` per
operation, in request order; on failure the server returns `Error` naming the
index of the failing operation.

Because the transaction is scoped to a single request — never held across IPC
round-trips — a plugin cannot pin SQLite's write lock open (which would stall
the core's own memory writes), and a dropped connection can never leave a
half-applied batch behind. This is the deliberate alternative to exposing
explicit `Begin`/`Commit`/`Rollback` over IPC: the batch covers the
"write several rows atomically" case (e.g. recording a multi-row undo entry)
without the long-held-lock hazard. `Batch` was added as a new request/response
variant with no protocol-version bump, following the same additive-only
discipline as the stdio protocol above.

### Per-plugin storage quota

Every stateful plugin writes into the **one shared `memory.db`**, so prefix
isolation alone does not stop a plugin from filling the disk: a logging loop in
a third-party (or merely buggy) plugin could grow its tables without bound,
bloating the database and degrading the memory system's queries, backups, and
`PRAGMA integrity_check`. To bound this, each plugin carries a storage quota —
`plugins.list.<name>.db_quota_mb`, default `256` MiB (#424).

Before any storage-growing write (`Insert`/`Upsert`, standalone or inside a
`Batch`), the host measures the plugin's footprint by summing the byte length
of every cell across its declared tables — `SUM(length(CAST(col AS BLOB)))` per
table — and refuses the write with a `QUOTA_EXCEEDED` error once the footprint
reaches the cap. `SQLite` has no per-table size API (the `dbstat` virtual table
is not compiled into the bundled `libsqlite3-sys`, see #350), so this payload
sum is used as a faithful, prefix-scoped proxy; it is a slight underestimate
(it omits per-row overhead and index pages), which is acceptable for a soft
cap. The measurement reads only the plugin's own tables, never the whole
database.

Reads (`Select`/`Count`) and deletes are **never** gated, so a plugin that hits
its quota can always delete rows to free space and resume writing. A batch that
would exceed the quota is rolled back atomically and names the failing
operation. Set `db_quota_mb` to `null` to disable enforcement for a plugin that
legitimately needs unbounded storage.

---

## 6. Plugin Security Model

### Opt-in discovery

Plugin discovery is **opt-in**: only binaries explicitly listed in
`plugins.list` with `enable: true` are started. Dropping a binary into the
plugins directory does **not** cause it to execute — the host logs a warning
suggesting the user add it to configuration. This prevents a "drop binary →
auto-execute" attack vector.

```jsonc
// settings.json (excerpt)
{
  "plugins": {
    "list": {
      "fs": { "enable": true },
      "anthropic": { "enable": true, "env_passthrough": ["ANTHROPIC_API_KEY"] }
    }
  }
}
```

### Binary resolution order (built-in wins)

When the host resolves a plugin name to a binary (`find_plugin_binary`), it
searches a fixed order and the **first match wins**:

1. `<builtin>/ene-plugin-{name}`
2. `<builtin>/{name}`
3. `<user>/ene-plugin-{name}`
4. `<user>/{name}`

Because the built-in directory is searched first, a binary a user places under
the user plugins directory can **never** shadow a built-in of the same name —
the shipped binary always runs. This is the deliberate, security-conservative
choice: an untrusted drop-in cannot silently replace a trusted built-in (which
is what the credential trust gate relies on). The practical consequence is that
a user who drops a binary named after a built-in will see the built-in run
instead of theirs; choose a distinct plugin name to override behavior.

Whether a name counts as a *built-in* for the trust gate is decided by a
compiled-in list of shipped plugin names, **not** by probing the filesystem.
In debug builds the built-in directory resolves to the running executable's
directory (`target/debug/...`), so a filesystem check would let any
`ene-plugin-*` binary dropped there masquerade as a trusted built-in; the
fixed list keeps the trust gate independent of whatever happens to be on disk.

### Environment hardening (`env_clear`)

Every plugin and MCP stdio server is spawned with `env_clear()` — the
inherited environment is wiped and only an explicit whitelist is forwarded:

| Variable | Purpose |
|---|---|
| `PATH` | Locating system executables |
| `HOME` | User config files |
| `TMPDIR` | Temporary files |
| `LANG` | Locale-sensitive output |
| `TZ` | Timezone (only if set) |
| `LD_LIBRARY_PATH` | Shared library loading (Linux) |
| `SystemRoot`, `USERPROFILE`, `APPDATA`, `TEMP`, `PATHEXT` | Windows essentials |
| `ENE_PLUGIN_SOCKET` | IPC channel (plugins only) |

### Per-plugin `env_passthrough`

Plugins that need additional host variables (e.g. API keys) declare them
explicitly via `env_passthrough` in their `plugins.list` entry. A built-in
denylist blocks security-sensitive names (`LD_PRELOAD`, `LD_AUDIT`,
`DYLD_INSERT_LIBRARIES`, `ENE_PLUGIN_SOCKET`, etc.) regardless of
configuration.

MCP stdio servers support the same `env_passthrough` field in their
`plugins.mcp_servers` entry for parity.

### Plugin configuration flow (`set_config` / `set_profiles`)

Every plugin — tool **or** provider — receives its configuration from the
host during the IPC handshake **and** on live updates via
`PluginIpcRequest::SetConfig` (protocol v5+). The `plugins.list.<name>.config`
blob is delivered verbatim via `ConfigurablePlugin::set_config`; the
`plugins.list.<name>.profiles.<profile>` map (for per-model/voice settings) is
delivered via `ConfigurablePlugin::set_profiles`. Both are host-opaque: the
host stores them as-is, never interprets their keys, refreshes the connection
cache before each push, and re-sends them on reconnect. When a settings
hot-reload changes a plugin's config/profiles without changing the enable-set,
the runtime pushes `SetConfig` to the live connection instead of restarting
the plugin host. A peer that negotiated below v5 gets a warn + local-cache
update only (no live IPC). Provider plugins (LLM/embed/TTS/STT) get the same
delivery as tool plugins, so e.g. the Anthropic provider can receive its API
key at handshake time rather than per request.

Do **not** put host-reserved entry keys (`enable`, `checksum`) inside the
nested `config` object — they collide with `plugins.list.<name>` fields. The
host warns when those keys appear in a delivered config blob.

A plugin advertises the JSON Schema its config accepts via
`config_schema()`. Fields marked `x-ene-secret: true` in that schema will be
masked in the UI (planned) and redacted from host logs (see
[`configuration.md`](../configuration.md) for the exact shape).

`GetConfigSchema` may be re-fetched at runtime. Plugins that discover options
only after connecting to an external engine can push
`ConfigSchemaChanged` (routed like `DeferredCompleted`). Opt-in capability
flags unlock `ListConfigOptions` (dynamic enums), `ValidateConfig`
(cross-field errors), and `MigrateConfig` (`config_version` self-migration).
Peers that omit the flags degrade to static schema + host JSON Schema
validation with no migration. UI wiring for these APIs is out of scope here.

### Binary checksum verification (TOFU)

On first activation, the host computes the SHA-256 checksum of the plugin
binary and records it in `plugins.list.<name>.checksum`
(trust-on-first-use). Subsequent launches verify the binary against the
recorded checksum and refuse to start if it has changed. Comparison is
case-insensitive (hex encoding).

The checksum is also re-verified on every supervised restart: before the host
kills and re-spawns a crashed or unresponsive plugin, it recomputes the
on-disk binary's checksum and compares it against the value pinned at startup.
If the binary changed while the host was running (for example, a `cargo build`
replaced it during development), the restart is aborted and the plugin is
**permanently disabled** until the host itself is restarted. This is
intentional — the running instance was verified against the original binary,
so the host refuses to silently exec a different one; restart the host to pick
up the new binary and re-pin its checksum.

### Process supervision & restart budget

Every started plugin is supervised by a periodic health probe
(`plugins.health_interval_ms`, default 30 s; `0` disables probing). On each
tick the host pings the plugin and checks the child process is alive:

- **Healthy** (alive and answered the ping) → the plugin's **restart budget is
  recovered** (reset to zero).
- **Unhealthy** (dead or unresponsive) → the host emits an `Unhealthy` event,
  waits an exponentially-growing backoff delay, restarts the process, and
  reconnects. Each restart consumes one unit of budget.

The restart budget is a **recoverable sliding measure of recent instability,
not a lifetime cap**. After `MAX_RESTARTS` (5) restarts without an intervening
healthy round-trip, the plugin is **permanently disabled**
(`PluginHealthEvent::Disabled`, reason `restart_budget_exhausted`) until the
host itself restarts. But any healthy round-trip clears the budget back to
zero, so a plugin that crashes once a day and otherwise runs well never
accumulates budget, while a genuine crash loop (repeated failures with no
healthy interval) exhausts it and is stopped.

Budget recovery happens on **any** healthy round-trip, independent of the
plugin's capabilities:

- a successful **tool call** (plugins that expose tools), and
- a healthy **health-probe ping** (every supervised plugin).

The health-probe path is what makes recovery work for **provider-only
plugins** (e.g. the built-in `anthropic` provider): they expose no tools, so
they never build a tool registry and never have a successful tool call to
reset their budget. Without probe-based recovery their budget would be a
lifetime limit of five crashes for the whole host session — fatal for a
long-running desktop companion. A plugin that answers pings but fails real
work is still contained: the per-registry circuit breaker trips on consecutive
call failures, so ping-based recovery cannot keep a broken plugin serving
traffic indefinitely.

---

## 7. MCP (Model Context Protocol) Integration

`ene-plugin-host` integrates external MCP servers (the former `ene-connector`
bridge layer was removed in #416 — connection lifecycle lives entirely in the
plugin host):

1. **Discovery & Launch**: Host reads `plugins.mcp_servers` configuration and spawns target MCP server binaries over `stdio` or HTTP/SSE.
2. **Tool Translation**: MCP tools are automatically translated into `ToolSpec` items and registered into the `CompositeToolRegistry`.
3. **Execution Routing**: Tool calls generated by the LLM are routed through the MCP bridge and returned cleanly to `ene-runtime`.

Server names are used verbatim for routing and tool namespacing — no charset
validation is applied — so hyphenated names like `github-mcp` connect just like
any other name (#417).

### HTTP URL validation (SSRF protection)

HTTP MCP endpoints (`transport.type = "http"`) have their URL validated by
`McpToolRegistry::connect_http` **before** any connection is attempted. The
default is deny:

- **HTTPS-only.** `http://` URLs are rejected.
- **Loopback refused.** `127.0.0.0/8` and `::1` are rejected.
- **Link-local always refused.** `169.254.0.0/16` — including the cloud
  metadata endpoint `169.254.169.254` — and `fe80::/10` are rejected under
  every setting.

A rejection is reported in both a tracing log and the returned error
(`PluginHostError::McpHandshake`), naming the server and the reason.

For local development, `plugins.mcp_allow_insecure_urls` (default `false`)
opts into plain-`http://` URLs and loopback endpoints:

```jsonc
// settings.json (excerpt)
{
  "plugins": {
    "mcp_allow_insecure_urls": true,
    "mcp_servers": [
      { "name": "local-dev", "enabled": true,
        "transport": { "type": "http", "url": "http://127.0.0.1:8080/mcp" } }
    ]
  }
}
```

This opt-in never relaxes the link-local block. DNS rebinding — a hostname
that resolves to an internal address — is out of scope: only IP-literal hosts
are inspected. This is no weaker than the previous behavior, which performed no
validation on the live connect path at all.

---

## 8. Writing a Custom Tool Plugin

Developers can quickly author new tool plugins using `ene-plugin`'s `#[derive(ToolAction)]` (via `ene-plugin-macros`) and server entry point. This sketch is illustrative — see an existing plugin under `plugins/tool/*` (e.g. `plugins/tool/app/src/main.rs`) for the current, compiling pattern, or `cargo doc -p ene-plugin-macros --open` for the derive macro's exact requirements:

```rust,ignore
use ene_plugin::prelude::*;

#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(namespace = "custom", name = "greet",
       summary = "Generates a personalized greeting.", category = "Custom",
       keywords_primary = "greet, hello")]
pub struct GreetAction {
    pub name: String,
}

impl GreetAction {
    async fn run(&self) -> Result<String, ToolError> {
        Ok(format!("Hello, {}!", self.name))
    }
}

#[tokio::main]
async fn main() {
    use ene_plugin::{PluginDispatch, ToolProviderPlugin, run_plugin_server};
    use std::sync::Arc;

    let provider = ActionSetProvider::new(vec![Box::new(GreetAction { name: String::new() })]);
    let dispatch = PluginDispatch::new(
        Some(Arc::new(ToolProviderPlugin::new(provider))),
        None,
        None,
        None,
        None,
    );
    if let Err(e) = run_plugin_server(dispatch).await {
        tracing::error!("fatal error: {e}");
        std::process::exit(1);
    }
}
```
