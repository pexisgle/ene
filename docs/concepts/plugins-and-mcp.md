# IPC Plugin System & MCP Integration

This document covers Ene's out-of-process IPC plugin architecture, Protocol v6 wire specs, Model Context Protocol (MCP) server integration, and built-in tool plugins.

---

## 1. Out-of-Process Plugin Architecture

To guarantee process isolation, stability, and security, all external capabilities—tool plugins, custom LLM providers, and MCP servers—run as independent sub-processes managed by `PluginHostManager` (`ene-plugin-host`).

```text
Ene Host Application (ene-runtime)
  │
  └── PluginHostManager (ene-plugin-host)
        │
        ├── IPC Protocol v6 (Length-prefixed frames over stdio)
        │     ├── ene-plugin-anthropic (Anthropic LLM Provider Plugin)
        │     ├── ene-plugin-openai    (OpenAI-Compatible Provider Plugin)
        │     ├── ene-plugin-openai-tts (OpenAI Speech API TTS Provider Plugin)
        │     ├── ene-plugin-llama-cpp (Local GGUF Provider Plugin)
        │     ├── ene-plugin-voicevox  (VOICEVOX / Aivis Speech TTS Provider Plugin)
        │     ├── ene-plugin-app       (GUI Launcher Tool)
        │     ├── ene-plugin-browser   (CDP Browser Automation Tool)
        │     ├── ene-plugin-calc      (Calculation Tool)
        │     ├── ene-plugin-calendar  (Calendar Tool)
        │     ├── ene-plugin-counter   (Sample Stateful Tool)
        │     ├── ene-plugin-fs        (Sandboxed Filesystem Tool)
        │     ├── ene-plugin-random    (Random Generation Tool)
        │     ├── ene-plugin-geo       (Geographic Information Tool)
        │     ├── ene-plugin-git       (Read-only Git Tool)
        │     ├── ene-plugin-utility   (Todo, Question, Timer & Notify Tool)
        │     └── ene-plugin-web       (Web Search & Scraper Tool)
        │
        └── Model Context Protocol (MCP) Bridge (ene-plugin-host)
              └── External MCP Servers (Node.js / Python / Go MCP processes)
```

---

## 2. IPC Protocol v6 Specification

Plugins communicate over `stdin`/`stdout` using **IPC Protocol v6**:

- **Framing**: Every packet begins with a 4-byte little-endian `u32` payload size followed by a payload in the negotiated `WireFormat`. The handshake exchange (request and ack) always uses UTF-8 JSON; once both sides negotiated protocol v6, every later frame is MessagePack (`rmp-serde`, map-encoded). Peers that negotiated v5 or lower keep the original JSON framing for the whole connection, so N-1 plugins are byte-compatible with the pre-v6 wire.
- **Handshake Negotiation**: The host sends `PluginIpcRequest::Handshake { version: VersionRange::host_supported() }`, i.e. `VersionRange { min: 5, max: 6 }` — not a single pinned value. The plugin intersects that range with its own supported range via `VersionRange::negotiate` and responds with `HandshakeAck { version, capabilities: PluginCapabilities }`, where `version` is the highest version common to both sides.
- **Handshake Timeout**: The host bounds how long it waits for the `HandshakeAck` (`plugins.handshake_timeout_ms`, default 10 s). A plugin that accepts the socket but never replies fails the handshake with `PluginHostError::HandshakeFailed` instead of blocking startup of the remaining plugins. Plugin authors must answer the handshake promptly and defer heavy initialization (model loading, etc.) until afterwards — see `run_plugin_server` in `ene-plugin`.
- **Request Correlation**: All async requests and responses include a mandatory `request_id` (`Uuid`).
- **Capabilities**: Plugins advertise supported capabilities (`tools`, `llm_providers`, `stt_providers`, `tts_providers`), each provider spec also declaring a `concurrency: ConcurrencyHint` (see [§3](#3-provider-concurrency-concurrencyhint)). Plugin-to-plugin capability sharing is declared via `provides` / `requires` (see [§4](#4-capability-declarations-provides--requires)).

### Versioning policy (N-1 backward compatibility)

Tool and provider plugins ship as independent out-of-process binaries. Bumping `PLUGIN_IPC_PROTOCOL_VERSION` does not recompile plugin binaries that are already installed, so the host maintains **one version of backward compatibility**:

- The host always advertises `[PLUGIN_IPC_MIN_SUPPORTED_VERSION, PLUGIN_IPC_PROTOCOL_VERSION]` during the handshake (`VersionRange::host_supported()` in `crates/ene-plugin-proto/src/ipc.rs`), rather than a single pinned version. A plugin built against the previous protocol version can still connect and negotiate at that older version.
- A plugin binary is not required to support a range — it may keep declaring `VersionRange { min: N, max: N }` for whatever version it was built against. The compatibility responsibility is concentrated in the host, not pushed onto every plugin author.
- **Bumping the protocol version**: when `PLUGIN_IPC_PROTOCOL_VERSION` is bumped, `PLUGIN_IPC_MIN_SUPPORTED_VERSION` must be bumped by the same amount, dropping support for the oldest previously-supported version.
- **When a bump is required**: only for changing the meaning of an existing message, adding a required field, or removing/renaming an enum variant. New fields should use `#[serde(default)]` so older/newer peers stay wire-compatible without a version bump.
- **Feature gating**: the host stores the negotiated version on `IpcPluginConnection` (`ene-plugin-host`) and exposes it via `negotiated_version()`. Behavior that depends on a message introduced after the minimum supported version should gate on it — e.g. `supports_set_config()` gates `PluginIpcRequest::SetConfig` (introduced in v5) so a v4 plugin isn't sent a message it cannot deserialize; the host still updates its local cache so the next reconnect handshake delivers the fresh config. Dynamic-config messages (`ListConfigOptions`, `ValidateConfig`, `MigrateConfig`) also require protocol ≥ v5 **and** the matching `PluginCapabilities` flags (`supports_list_config_options`, etc.; serde-default `false` on older v5 binaries that lack those variants).
- **Feature gating**: the host stores the negotiated version on `IpcPluginConnection` (`ene-plugin-host`) and exposes it via `negotiated_version()`. Behavior that depends on a message introduced after the minimum supported version should gate on it — e.g. `supports_set_config()` gates `PluginIpcRequest::SetConfig` (introduced in v5). Under the current N-1 window (v5+) every peer knows that variant, so the live push always applies; the check is retained as the version-relative pattern for features introduced above the minimum. Dynamic-config messages (`ListConfigOptions`, `ValidateConfig`, `MigrateConfig`) require protocol ≥ v5 **and** the matching `PluginCapabilities` flags (`supports_list_config_options`, etc.; serde-default `false` on older v5 binaries that lack those variants).
- **Negotiation failure diagnostics**: when a plugin's proposed range and the host's supported range do not overlap, the plugin's `HandshakeAck` error and the host's `PluginHostError::HandshakeFailed` / `ProtocolMismatch` both name the ranges on both sides (e.g. "host supports 5..=6, plugin supports 3..=3"), so a developer can tell the plugin binary needs rebuilding rather than seeing a generic handshake failure.

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

TTS provider plugins follow the same discipline: `ene-plugin-host`'s
`IpcTtsProvider` / `IpcTtsProviderFactory` (`ipc_tts.rs` / `tts_factory.rs`)
implement `ene_ai::TtsProvider` / `TtsProviderFactory` so a plugin's
`tts_providers` capability registers in the global `AudioProviderRegistry`,
keyed by its `TtsProviderSpec.kind` (e.g. `"voicevox"`, selected with
`ai.tts.provider`). A synthesize call is one `SynthesizeSpeech` IPC round-trip
returning a whole audio file (WAV); the host decodes it to PCM and slices it
into `TtsChunk`s, preserving the `TtsProvider::synthesize_stream` contract.
The `voicevox` plugin additionally spawns and supervises a local
VOICEVOX-compatible engine binary in managed mode (`auto_start: true`).

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

### ResourceClass: the in-process admission key is a wire type

The in-process admission budget that serializes local engines contending on the same physical resource (same GPU device, the shared CPU class, or the network-attached class) keys on a `ResourceClass` enum — `Gpu { device: u32 }` / `Cpu` / `Network` — defined in `ene-plugin-proto`'s `capabilities.rs` alongside `ConcurrencyHint`. In-process engines use it as `ene_ai::ResourceClass`; the follow-up host-side resource admission work wires it into the plugin capability specs so a plugin can declare which physical resources it uses, making this the same type on both sides of the boundary rather than a second definition.

---

## 4. Capability Declarations (`provides` / `requires`)

Plugins can share *capabilities* with each other: a plugin that owns a heavy
runtime (an inference engine, a speech synthesizer) declares it in `provides`,
and other plugins declare in `requires` that they need it. The host indexes
every declaration at startup, resolves each `requires` to a provider plugin,
and refuses to register a plugin whose hard requirements are unmet — the
declaration mechanism is the foundation of plugin-to-plugin capability
mediation (see the `gguf-runner` contract below).

### 4.1 Declaration form

`PluginCapabilities` carries two new handshake fields, both `#[serde(default)]`
(older plugin binaries omit them and are treated as declaring nothing — no
protocol version bump was needed):

```json
provides: ["llm/chat@1", "embed@1", "gguf-runner@1"]
requires: ["gguf-runner@^1", "g2p/ja@^1?"]
```

- `provides` entries are capability **references**: `name@major`.
- `requires` entries are capability **requirements**: `name@[^]major[?]`.

The `^` prefix declares compatibility intent (`^1` = "any 1.x"). On today's
wire the reference version is a bare major, so `^1` and `1` match the same
set; the prefix exists so consumers state their intent before minor versions
exist. A trailing `?` marks a **soft** requirement: the plugin can start and
fall back to a built-in implementation when no provider is present. Without
`?` the requirement is **hard**: the host disables the plugin when no provider
matches.

### 4.2 Capability naming and versioning policy

Names are one or more lowercase `[a-z0-9-]` segments joined by `/`, for
example `llm/chat`, `g2p/ja`, `gguf-runner`. The version is a single major
integer — deliberately no minor/patch on the wire. Policy:

- Capability versions are **semver-ish**, not semver: compatible additions
  (new methods, optional fields) stay within a major; any change that breaks
  an existing consumer bumps the major. Treat a capability's ABI with the
  same care as the wire ABI — a third-party declaration is a promise that
  outlives your release.
- Majors start at `1` by convention. `0` parses but is not a published
  contract; a capability in pre-1.0 flux must not be relied on by consumers.
- Adding a capability is compatible; *changing its meaning* (not just its
  implementation) is a major bump.
- Malformed entries (bad charset, missing `@`, non-numeric major, leading
  zeros) are dropped individually with a host warning — one typo never fails
  a plugin's handshake.

### 4.3 Host-side resolution

The host builds a capability registry from every plugin's handshake
declarations before registering any tools or providers:

- **Hard requirement unmet** → the plugin is not registered at all (no tools,
  no providers, no supervision) and a `RequirementsUnmet` health diagnostic
  is emitted listing the missing requirements. Recovery is a host restart or
  reconfiguration with a provider present.
- **Soft requirement unmet** → the plugin starts normally; a warning is
  logged. Falling back is the plugin's responsibility.
- **Deterministic winner**: when several plugins provide the same
  capability, resolution picks the lexicographically smallest plugin name.
  (Config order is not a valid tie-breaker — `plugins.list` is a map.)
  Explicit provider preference is future work.
- **Transitive**: a plugin disabled for unmet requirements does not count as
  a provider, so consumers of its capabilities are disabled too (the gate is
  evaluated to a fixpoint).
- **Self-resolution is allowed**: a plugin's `requires` may be satisfied by
  its own `provides`. Whether a plugin may *call* its own capability through
  mediation is a separate ACL decision.

### 4.4 The `gguf-runner@1` capability contract

`gguf-runner@1` is the capability for **loading any GGUF model and running
inference on it** — the runtime that third-party GGUF model providers borrow
instead of bundling their own llama.cpp (N bundled runtimes would mean N GPU
contexts and N× VRAM). A plugin that wants to serve GGUF models declares

```json
requires: ["gguf-runner@^1"]
```

The runner API is **non-streaming** by design; consumers that need token
streaming should require `llm/chat@1` from the model provider directly.
Mediation (a plugin calling `gguf-runner` through the host) lands together
with the runner implementation; the capability-level contract is fixed here:

| Method | Request | Response |
|---|---|---|
| `generate` | `{ model, prompt, json_schema? }` | `{ text }` |
| `embed` | `{ model, texts: [string] }` | `{ embeddings: [[number]] }` |
| `unload` | `{ model }` | `{ ok: true }` |

`model` identifies a model profile configured on the provider plugin.
`json_schema` (when present) constrains `generate` to structured output.
`unload` releases a loaded model's resident memory (VRAM); it is the hook for
future resource-residency management. These method names and payload shapes
are the contract third parties build against; the wire encoding is defined
with the mediation layer.

The built-in provider that serves `gguf-runner@1` is `ene-plugin-llama-cpp`
(`plugins/provider/local-llm`), which also declares `llm/chat@1` and
`embed@1` and serves both over the provider IPC: chat streaming, JSON-schema
completion, and GGUF embeddings, exercised by the plugin crate's CPU contract
tests against pinned GGUF fixtures.

---

## 5. Built-In Plugin Catalog

| Plugin Binary | Namespace | Description | Stateful? |
|---|---|---|---|
| `ene-plugin-app` | `app.*` | System application launcher & window control | No |
| `ene-plugin-browser` | `browser.*` | Headless Chrome/CDP web browser automation | Yes (Session store) |
| `ene-plugin-calc` | `calc.*` | Math expression evaluation, unit/currency/color conversion | No |
| `ene-plugin-calendar` | `calendar.*` | Local calendar with per-calendar permissions, write confirmation, free-slot search | Yes (host-service `db`) |
| `ene-plugin-counter` | `counter.*` | Sample stateful tool: DB-backed counter with permission-gated reset | Yes (host-service `db`) |
| `ene-plugin-fs` | `fs.*` | Sandboxed filesystem operations with undo ledger | Yes (host-service `db`) |
| `ene-plugin-random` | `random.*` | Random numbers, UUID v4, list picks, and hex colors | No |
| `ene-plugin-geo` | `geo.*` | IP-based location, current weather, solar timezone offset, sunrise/sunset | No |
| `ene-plugin-git` | `git.*` | Read-only git inspection: status, diff, log, branch, remote, blame | No |
| `ene-plugin-homeassistant` | `homeassistant.*` | Home Assistant smart home control: entity state reads, switch/light/plug control, climate temperature setting | No |
| `ene-plugin-utility` | `utility.*` | Question prompts, todo list management, time/system info, countdown timers & desktop notifications (Linux, D-Bus only) | Yes (host-service `db`) |
| `ene-plugin-web` | `web.*` | Web search and markdown page scraper | No |
| `ene-plugin-anthropic` | Provider | Anthropic Claude provider plugin | No |
| `ene-plugin-openai` | Provider | OpenAI-compatible provider plugin (chat, streaming, embeddings) | No |
| `ene-plugin-openai-tts` | Provider | OpenAI Speech API TTS provider (tts-1 / tts-1-hd) — WAV (24 kHz PCM) audio | No |
| `ene-plugin-llama-cpp` | Provider | Local GGUF (llama.cpp) provider plugin — chat streaming, completion, and GGUF embeddings | No |

All seventeen plugins above are included in the default `plugins.list` and start
automatically on fresh installs.

### Filesystem tool reference (`filesystem.*`)

The filesystem plugin exposes read/write/edit/delete operations, glob and
regex search, unified-diff patching, and shell execution. The search actions
are:

**`filesystem.grep`** — search file contents with a regex. Optional
parameters:

| Parameter | Type | Default | Description |
|---|---|---|---|
| `pattern` | string | — (required) | Regex to search for |
| `path` | string | cwd | Base directory or file to search in |
| `include` | string | none | File name glob filter, one pattern per call (e.g. `*.rs`; `{a,b}` brace expansion is not supported) |
| `case_insensitive` | boolean | `false` | Match case-insensitively |
| `line_numbers` | boolean | `true` | Prefix each match with its 1-based line number |
| `context_lines` | integer | `0` | Non-matching context lines printed around each match |
| `count` | boolean | `false` | Print only the per-file and total match counts |

When the pattern contains capture groups, the matched group values are
printed beneath each match as `Captures: 1="…", 2="…"` (non-participating
groups show `(none)`). Results are capped at 100 matches per call unless
`count` is set.

**`filesystem.regex.test`** — test whether a regex matches a string, returning
`true` or `false`. Takes `text` (the string to test) and `pattern` (the
regex). The pattern is matched anywhere in the string, with the same
semantics as `filesystem.grep`; an invalid pattern is reported as an error.
Useful for an agent to decide "does this string match this pattern?" without
touching the filesystem.

---

## 6. Tool Database Schema Declaration & Evolution

Stateful tool plugins (`ene-plugin-fs`, `ene-plugin-utility`,
`ene-plugin-calendar`, `ene-plugin-counter`) persist their data into the
host's `memory.db`
through the shared **host-service** socket
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

## 7. Plugin Security Model

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
      "anthropic": { "enable": true, "env_passthrough": ["ANTHROPIC_API_KEY"] },
      "openai": { "enable": true, "env_passthrough": ["OPENAI_API_KEY"] }
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

A plugin's env fallback only works when the entry forwards it: for example
`calc.currency_convert` falls back to `EXCHANGERATE_HOST_API_KEY`, but the
default `calc` entry forwards no variables — set
`plugins.list.calc.env_passthrough = ["EXCHANGERATE_HOST_API_KEY"]`, or
configure `plugins.list.calc.config.exchangerate_host_access_key` instead.

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
the plugin host. Every peer in the host's N-1 window (v5+) understands
`SetConfig`, so the live push always applies. Provider plugins
(LLM/embed/TTS/STT) get the same delivery as tool plugins, so e.g. the
Anthropic provider can receive its API key at handshake time rather than per
request.

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

### Credential declaration (`x-ene-credentials`)

Plugins that need credentials (API keys, OAuth2 tokens) declare them at the
top level of the schema returned by `config_schema()`, alongside the existing
`x-` markers:

```json
{
  "type": "object",
  "properties": { "voice": { "type": "string" } },
  "x-ene-credentials": [
    { "id": "anthropic", "kind": "api_key", "required": true,
      "header": { "name": "x-api-key", "format": "{value}" },
      "env_fallback": "ANTHROPIC_API_KEY",
      "label": "Anthropic API Key",
      "help_url": "https://console.anthropic.com/settings/keys" },
    { "id": "google.calendar", "kind": "oauth2",
      "scopes": ["https://www.googleapis.com/auth/calendar.readonly"],
      "auth_url": "https://accounts.google.com/o/oauth2/v2/auth",
      "token_url": "https://oauth2.googleapis.com/token",
      "label": "Google Calendar" }
  ]
}
```

Fields common to both kinds: `id` is the stable credential id, `required`
(default `false`) marks the credential as mandatory, `shared` (default
`true`) controls namespace sharing (see below), and `label` / `help_url`
drive the configuration UI. Ids accept `[A-Za-z0-9._-]` and must not start
or end with `.` — `anthropic`, `google.calendar`, and `google-calendar` are
all valid.

- `kind: "api_key"` — a static secret. `header` (optional) tells the client
  how to inject the value; `format` is a template that must contain
  `{value}` (e.g. `Bearer {value}`). `env_fallback` names an environment
  variable the host checks when no value is stored.
- `kind: "oauth2"` — an OAuth2 flow driven by the host. `scopes` lists the
  consent scopes, `auth_url` / `token_url` the authorization and token
  endpoints.

**Sharing policy.** Declarations are shared by default: two plugins that both
declare `anthropic` address the same stored value, so switching providers
does not force re-entering keys. Sharing is limited to plugins that declared
the id — a plugin that never declared `anthropic` is denied even when the
value exists in the vault. A plugin opts out with `"shared": false`, which
resolves its own namespaced value at `<plugin>:<id>`.

The `:` separator makes a private key structurally unable to collide with a
shared declaration: it is in neither the id charset (`[A-Za-z0-9._-]`) nor the
plugin-name charset (`[A-Za-z0-9_-]`), so no shared id can be spelled like a
private key. Plugin A's private `anthropic` (`A:anthropic`) and plugin C
sharing the id `A.anthropic` (`A.anthropic`) resolve to different keys without
any extra uniqueness invariant.

**Validation timing.** Declarations are validated when the plugin starts:
each entry is checked independently, a bad entry is warned about and ignored
(the plugin itself still starts), and duplicate ids keep the first
occurrence. Request-time enforcement lives in the credential service, which
resolves every request against the requesting plugin's registered
declarations and denies undeclared ids. Value format validation (e.g. a
`sk-ant-` prefix) is delegated to the plugin's `ValidateConfig` on save.

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

### Calendar tool: confirmation and privacy controls

`ene-plugin-calendar` implements the interactive permission contract
described above for every mutating operation, layered on top of
**per-calendar permission flags**:

- **Per-calendar permissions.** Every calendar account row carries
  `read_allowed` / `write_allowed` flags. New calendars allow reads and deny
  writes by default (deny-by-default); `calendar.set_permission` changes the
  flags, itself gated behind user approval. Reads (`calendar.list_events`,
  `calendar.find_free_slots`) fail closed on calendars without read
  permission; writes (`calendar.create_event`, `calendar.update_event`,
  `calendar.cancel_event`, `calendar.remove_account`) fail closed without
  write permission.
- **Write confirmation with preview.** Every mutating action returns
  `PermissionRequired` *before* touching the store. The `description` shown
  to the user previews the timezone, the target calendar, and the change —
  `update_event` renders a before/after diff (including timezone-only
  changes). The request id is a deterministic hash of
  `(action, target, description)`, so the host's post-approval
  re-invocation — which replays identical arguments — resolves against the
  recorded approval instead of prompting again, while a changed description
  (different event content) requires a fresh approval. Allow-once approvals
  expire at the turn boundary (the plugin clears them on the host's
  call-context update); "allow for this session" records an
  `(action, target-prefix)` pattern that passes the gate for the rest of
  the conversation.
- **Privacy.** Event *content* (titles, notes, attendees) never appears in
  the plugin's logs or in the host's audit trail: the permission `target` is
  a stable `calendar:<id>` / `calendar:<id>#<event-id>` identifier, and the
  audit log records only `action`, `target`, and the decision, with calendar
  argument payloads (title, notes, attendees, location) masked before
  persistence. Content is surfaced only where it must be: in the approval
  prompt (user-facing) and in the tool result delivered to the LLM. Unlinking
  an account (`calendar.remove_account`) deletes the account row and all of
  its events in one transaction, so the disconnect is reflected immediately.
- **Provider abstraction.** Events are accessed through a
  `CalendarProvider` trait keyed by account kind. Today only the `local`
  kind exists (events stored in the plugin's `calendar_events` table);
  external services (Google Calendar, CalDAV) can be added later as new
  providers behind the same trait once the connector framework provides
  credential handling.

---

## 8. MCP (Model Context Protocol) Integration

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

## 9. Writing a Custom Tool Plugin

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

### Deferred (background) execution

`#[derive(ToolAction)]` actions are synchronous request–response tools.
Tools that must return immediately and deliver their result later — a
countdown timer that fires a desktop notification, or a notification send —
advertise `background_capable` in the `#[tool(...)]` attribute. The host
then invokes them through the deferred IPC path (`call_tool_deferred` →
`task_id` → `poll_deferred` until a terminal status), so the LLM turn is
not blocked.

`ActionSetProvider` intentionally does **not** implement the deferred
methods: task spawning, polling state, and cancellation are specific to
each binary's concurrency model. A plugin that needs background tools
implements `ToolProvider` manually, delegates the synchronous surface to
an inner `ActionSetProvider`, and overrides `call_tool_deferred`,
`poll_deferred`, and `cancel_deferred`. See `plugins/tool/utility`
(`TaskRegistry`, `TimerStartAction`, `NotifySendAction`) for a working
example.

On the host side a deferred task is polled at 100 ms intervals for at most
`tools.deferred_max_polls` polls (default 600 ≈ 60 s). Work that outlives
that budget still runs — the timer keeps counting down and its notification
still fires — but no completion event is delivered to the LLM. Late
outcomes can be checked through the tool's own status surface (e.g.
`utility.timer_stop` with no name lists running and finished timers).
