# Tool SDK Reference

> **Crates**: `ene-plugin` | `ene-plugin-proto` | `ene-plugin-db` | `ene-plugin-macros`

This is the reference for authoring **tool plugins**: out-of-process
binaries that expose namespaced actions and are driven by the host over
plugin IPC. For the step-by-step authoring guide see
[Write a Tool](../guide/tools/write-a-tool.md).

The names in this document are the real ones. Earlier design documents
referred to `ene-tool` / `ene-tool-derive` / `ene-tool-proto` /
`ene-tool-host` / `ene-tool-db` / `run_tool_server`; those crates were
merged into the `ene-plugin-*` family and the old names are obsolete.

---

## Crate map

| Crate | Role | Used by |
|---|---|---|
| `ene-plugin` | Authoring facade: `ToolAction`, `ActionSetProvider`, `SingleActionProvider`, `prelude::tool`, `run_plugin_server`, `PluginDispatch`, `ToolProviderPlugin` | Tool binaries |
| `ene-plugin-proto` | Wire ABI: IPC framing, handshake, `ToolSpec`, `ToolError`, `SideEffects`, `SandboxConfigData`, `VersionRange` (re-exported through `ene-plugin`) | Tool binaries + host |
| `ene-plugin-macros` | Proc-macro derives: `ToolAction`, `ToolSpec` (re-exported through `ene-plugin::prelude::tool`) | Tool binaries |
| `ene-plugin-db` | DB IPC client: `DbClient`, `DbSchema`, `DbFilter`, `DbValue`, `batch` | Stateful tool binaries |
| `ene-plugin-host` | Host-side supervision, capability routing, registration (consumes plugins) | Core only — never a dependency of a tool |

## Authoring surface

One import line covers the whole tool authoring surface:

```rust
use ene_plugin::prelude::*;
```

This brings in the `ToolAction` and `ToolSpec` derive macros, the
`ToolAction` trait (imported anonymously — the generated code resolves
it), `ToolError`, `schemars::JsonSchema`, `serde::Deserialize`,
`async_trait::async_trait`, and `ActionSetProvider` /
`SingleActionProvider`. The `ToolSpec` type itself is re-exported at the
crate root (`ene_plugin::ToolSpec`), not through the prelude.

### The action pattern

Every action is a struct whose fields are the JSON arguments. The
`#[derive(ToolAction)]` macro generates:

- an inherent `const TOOL_NAME: &'static str` and `spec() -> ToolSpec`
  (JSON Schema built by `schemars` over the struct, plus the
  `#[tool(...)]` metadata),
- an `impl ToolAction` whose `name()` / `definition()` / `rag_profile()`
  forward to those,
- `execute(&self, arguments: &str)` that deserializes the arguments
  (mapping parse failures to `ToolError::InvalidArguments`), copies
  `#[tool(skip)]` fields from `self`, and calls the hand-written
  `async fn run(&self) -> Result<String, ToolError>`.

```rust
#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "calc",
    name = "evaluate",
    summary = "Evaluate a mathematical expression.",
    description = "Evaluates a math expression such as \"2 + 3\".",
    category = "Utility",
    keywords_primary = "calculate, compute, math",
    side_effects = "ReadOnly"
)]
pub struct EvaluateAction {
    /// The expression to evaluate.
    expression: String,
}

impl EvaluateAction {
    async fn run(&self) -> Result<String, ToolError> {
        // validate, compute, return a JSON string
    }
}
```

### `ToolAction` trait

```rust
pub trait ToolAction: Send + Sync {
    fn name(&self) -> &'static str;
    fn definition(&self) -> ToolSpec;
    fn rag_profile(&self) -> ToolRagProfile;
    async fn execute(&self, arguments: &str) -> Result<String, ToolError>;
}
```

`ToolAction` models synchronous request–response actions. Background
(deferred) execution is not part of the trait; see
[Deferred execution](#deferred-execution) below.

### `ActionSetProvider` and hooks

`ActionSetProvider` adapts a `Vec<Box<dyn ToolAction>>` into the legacy
`ToolProvider` surface so a tool binary does not hand-write dispatch:

```rust
let provider = ActionSetProvider::new(vec![
    Box::new(action::GetAction::new(state.clone())),
    Box::new(action::IncrementAction::new(state.clone())),
])
.with_set_call_context_hook(|conversation_id, turn_id| { /* session state */ })
.with_sandbox_hook(|sandbox| { /* DB socket, auth token */ })
.with_approve_permission_hook(|request_id| { /* record approval */ })
.with_allow_pattern_hook(|action, target_pattern| { /* session allow */ })
.with_revoke_pattern_hook(|action, target_pattern| { /* revoke allow */ })
.with_set_config_hook(|config| { /* plugin config */ })
.with_config_schema_hook(|| Some(schema));
```

Each hook maps to one method of the `ToolProvider` trait
(`set_call_context`, `set_sandbox`, `approve_permission`, `allow_pattern`,
`revoke_pattern`, `set_config`, `config_schema`). When a tool wraps
`ActionSetProvider` in its own `ToolProvider` impl (e.g. to add shared
state or deferred execution), the wrapper must forward the lifecycle
methods it uses — otherwise the hooks never fire.

### Server entry point

```rust
#[tokio::main]
async fn main() {
    let provider = provider::MyToolProvider::new();
    if let Err(e) = run_plugin_server(PluginDispatch::new(
        Some(Arc::new(ToolProviderPlugin::new(provider))),
        None, None, None, None,
    )).await {
        tracing::error!("[ene-plugin-my] Fatal error: {e}");
        std::process::exit(1);
    }
}
```

`run_plugin_server` reads the socket path from `ENE_PLUGIN_SOCKET`,
answers the handshake promptly, and dispatches requests. The five
`PluginDispatch` slots are tool / LLM / embed / TTS / STT; a tool binary
only fills the first.

## Tool ABI compatibility table

The plugin ABI is versioned by `PLUGIN_IPC_PROTOCOL_VERSION` and
negotiated per connection. See [Plugins & MCP](../../concepts/plugins-and-mcp.md)
for the full protocol description; the author-relevant rules are:

| Concern | Rule |
|---|---|
| Handshake | Host sends `VersionRange::host_supported()`; plugin intersects its own range and replies with the negotiated version |
| Backward compatibility | Host keeps N-1 support; a plugin may pin `VersionRange { min: N, max: N }` for the version it was built against |
| Adding fields | Use `#[serde(default)]`; additive wire changes do not require a version bump |
| Removing/renaming | A version bump — never do this for a small change |
| New messages | Gate on `negotiated_version()` (host side) and/or capability flags so old peers are never sent messages they cannot parse |
| `ToolSpec` | `side_effects` and `background_capable` default to safe values (`None` / `false`) when older binaries omit them |

## `ToolSpec` fields

`ToolSpec` is what the model sees plus host execution metadata:

- `name: ToolName` — validated namespaced name, e.g. `counter.get`.
- `description` — full markdown description shown to the LLM.
- `parameters` — JSON Schema derived from the struct (the derive forces
  `additionalProperties: false`).
- `background_capable` — `false` by default; `true` opts into deferred
  execution.
- `side_effects` — `None` by default ("unknown"); see below.

### Side effects and parallel dispatch

`SideEffects` is declared in the `#[tool(...)]` attribute:

```rust
side_effects = "ReadOnly"                       // no observable effects
side_effects = "Idempotent"                     // same args ⇒ same effect
side_effects = "Destructive"                    // data loss possible, rollback not guaranteed
side_effects = "FileSystem { mutates: true }"   // file writes
side_effects = "Network { external: true }"     // external network access
side_effects = "System { privileged: true }"    // privileged system access
side_effects = "Browser { mutates_dom: true }"  // DOM mutations
```

The attribute accepts either a bare unit variant (`ReadOnly`,
`Idempotent`, `Destructive`) or the braced struct form shown above; the
braces are part of the string literal.

Parallel dispatch is **fail-closed**: only an explicit
`SideEffects::ReadOnly` makes a tool eligible for bounded parallel
execution; `None` (unknown), `Idempotent`, and every mutating category
keep the tool sequential. A write must never be declared `ReadOnly` —
that would mark it parallelizable. There is no "database write" category;
for a stateful write that is not idempotent, omit the attribute and let
the unknown default keep the tool sequential.

## `ToolError` taxonomy

| Variant | Use |
|---|---|
| `NotFound { tool_name }` | Unknown tool name; `ActionSetProvider` returns this automatically |
| `InvalidName { reason }` | Malformed tool name (host entry points) |
| `DuplicateName { tool_name }` | Registration collision — hard error, no first-wins |
| `InvalidArguments { message }` | Argument parse failure or validation failure |
| `Generic { kind, message }` | Message-only error, discriminated by `ErrorKind` |
| `PermissionRequired { request_id, action, target, description }` | Ask the user before a sensitive operation |
| `UserInputRequired { request_id, prompt }` | Interactive question with options |
| `FileNotFound` / `FileTooLarge` | Filesystem action failures |
| `CommandBlocked` / `ShellTimeout` / `ShellOutputTooLarge` | Shell action sandbox failures |

For `Generic`, prefer the constructors: `ToolError::execution_failed`,
`::permission_denied`, `::io_error`, `::timeout`, `::internal`,
`::ipc_transport`, `::ipc_client`, `::sandbox_violation`.
`ErrorKind::Other` is deprecated — use `Internal` for tool-side unexpected
conditions.

`PermissionRequired` is the structured prompt used by the permission
flow; `description` is user-facing only and is not logged by the host's
audit trail. `target` must be a stable identifier without private content.

## Permission flow

1. The action returns `ToolError::PermissionRequired` **before** doing
   anything sensitive.
2. The host prompts the user and either calls the provider's
   `approve_permission(request_id)` (approve once) or
   `allow_pattern(action, target_pattern)` (approve for the session), or
   the call is dropped.
3. On approval the host **re-invokes the tool with identical arguments**;
   the retried call must produce the *same* `request_id` and recognize the
   recorded approval, otherwise the user is prompted again forever.

The `ApprovalGate` pattern (see `plugins/tool/counter/src/approval.rs`)
implements this: a deterministic request id derived from
`action:target:description`, per-turn expiry driven by `set_call_context`,
and session-wide allow patterns cleared when the conversation changes.

## DB IPC (`ene-plugin-db`)

Stateful tools reach the shared `memory.db` through the host-service `db`
passenger; a tool binary never opens its own SQLite connection.

1. **Declare** the schema at startup with `DbClient::declare_schema`.
   Table and index names must start with the plugin's prefix (e.g.
   `counter_`); the server enforces prefix isolation and identifier
   validation, and DDL runs only as a consequence of `DeclareSchema`.
2. **CRUD** with `select`, `insert`, `upsert`, `update`, `delete`,
   `count`; filters with `DbFilter::eq` and friends; `Row` is a
   `BTreeMap<String, DbValue>`.
3. **Transactions**: `DbClient::batch` applies a list of `DbWriteOp`s in a
   single SQLite transaction — all or nothing. Batches are capped at
   10,000 operations by the server.
4. **Quota**: `plugins.list.<name>.db_quota_mb` caps the plugin's footprint
   in the shared DB (default 256 MiB); storage-growing writes past the cap
   fail with `QUOTA_EXCEEDED`, reads/deletes stay allowed.
5. **Schema evolution**: additive changes (new table, new column) are
   applied automatically; conflicting changes (type change, dropped
   table/column, new constrained column) are rejected with
   `SchemaConflict`.

The connection parameters come from the sandbox handshake:
`SandboxConfigData.db_socket` (path) and `db_auth_token` (pre-shared
token; the server rejects unauthenticated connections). Wire them through
the `set_sandbox` hook, then lazily build the store on first use.

## Deferred execution

`ToolAction` is synchronous request–response. Background work requires a
hand-written `ToolProvider` implementation that overrides
`call_tool_deferred` / `poll_deferred` / `cancel_deferred` with its own
task registry; `ActionSetProvider` deliberately leaves these at their
sync defaults. Tools must also set `background_capable = true` on the
spec (the derive's `background_capable` attribute). See the utility tool
for a working task-registry example.

## Naming rules

- Plugin names match `[a-zA-Z0-9_-]`. The primary binary convention is
  `ene-plugin-<name>` (host discovery scans for this prefix only);
  `find_plugin_binary` additionally falls back to a bare `<name>`
  executable.
- Tool names are `<namespace>.<action>`: ASCII alphanumeric, `_`, `.`,
  `:`; no leading/trailing `.`/`:`; no consecutive separators. `-` is
  **not** allowed — convert hyphens to underscores in namespaces.
- The namespace usually equals the plugin name (`calc.*`, `geo.*`,
  `counter.*`); `fs` is the exception (`filesystem.*`).
- DB prefixes follow the plugin name: `counter_` for the `counter`
  plugin.

## Logging

- **stdout is the IPC channel.** Never print to stdout — it corrupts the
  wire protocol (`print_stdout` is denied at the workspace level for
  plugin crates).
- Use `tracing` macros; output goes to stderr, structured with fields
  (`tracing::info!(component = "PluginServer", ...)`).
- Fatal errors: `tracing::error!("[ene-plugin-<name>] Fatal error: {e}")`
  then `std::process::exit(1)` — a plugin that dies without a message is
  undebuggable.
- Never log secrets: auth tokens, API keys, or user prompt content
  (permission `description` is deliberately excluded from the host audit
  trail).

## Related

- [Write a Tool guide](../guide/tools/write-a-tool.md)
- [Plugins & MCP concepts](../../concepts/plugins-and-mcp.md)
- [Tool Authoring Crates](../../crates/tool-sdk.md)
- Generated rustdoc: `cargo doc -p ene-plugin --open`, `cargo doc -p ene-plugin-db --open`
