# Write a Tool

This guide walks through creating a new tool plugin from the template,
implementing actions, wiring permissions and DB state, testing, and
registering the result. The reference for the API surface and wire ABI is
[Tool SDK Reference](../reference/tools/sdk.md). A complete working
example ships as `plugins/tool/counter` (DB-backed counter with a
permission-gated reset); read it alongside this guide.

## 1. Scaffold from the template

```sh
templates/tool/new-tool.sh my_tool
```

This creates `plugins/tool/my_tool/` with the crate and binary
`ene-plugin-my_tool`, namespace `my_tool`, and one `my_tool.echo` action.
The `plugins/tool/*` glob in the workspace `Cargo.toml` picks the new
directory up automatically — no manifest edit is needed.

Template layout:

```text
plugins/tool/my_tool/
├── Cargo.toml          # workspace deps, [[bin]] ene-plugin-my_tool
└── src/
    ├── main.rs         # run_plugin_server entry, fatal-error path
    ├── action.rs       # one derive-based action with validation + tests
    └── provider.rs     # ActionSetProvider wrapper
```

## 2. Implement an action

An action is a struct whose fields are the JSON arguments. Derive
`ToolAction` (which implies `ToolSpec`) and write `run`:

```rust
use ene_plugin::prelude::*;

#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "my_tool",
    name = "greet",
    summary = "Greet a person.",
    description = "Returns a greeting for the given name.",
    category = "Utility",
    keywords_primary = "greet, hello",
    side_effects = "ReadOnly"
)]
pub struct GreetAction {
    /// The name to greet.
    #[arg(min_length = 1, max_length = 100)]
    name: String,
}

impl GreetAction {
    async fn run(&self) -> Result<String, ToolError> {
        Ok(serde_json::json!({ "greeting": format!("Hello, {}!", self.name) }).to_string())
    }
}
```

The derive generates the spec, the `name()`/`definition()` forwarders,
and `execute()` (deserialize → copy `#[tool(skip)]` fields → call `run`).
Tool names become `my_tool.greet`; the dispatch name and the spec name
are the same string by construction.

### Schema and validation

- Field docs become JSON Schema descriptions; `#[arg(...)]` adds
  constraints (`minimum`/`maximum`, `min_length`/`max_length`,
  `min_items`/`max_items`, `enum_values`, `default`, `description`,
  `hidden`, `internal`, `skip`).
- **Schema constraints are not runtime checks.** The generated
  `execute()` only fails on JSON parse errors. Enforce business rules in
  `run` and return `ToolError::InvalidArguments { message }`:

```rust
if self.name.trim().is_empty() {
    return Err(ToolError::InvalidArguments {
        message: "name must not be empty".to_string(),
    });
}
```

- Prefer typed fields over post-parse string munging (enums over
  free-form strings with `enum_values`).
- State that is not an argument (shared store, session, config) lives in
  `#[tool(skip)]` fields with `#[serde(skip, default = "...")]` — the
  derive re-copies them from `self` in `execute()`.

## 3. Wire the provider

Register the actions with `ActionSetProvider` and thread lifecycle state
through hooks:

```rust
let inner = ActionSetProvider::new(vec![
    Box::new(action::GreetAction::default()),
    Box::new(action::IncrementAction::new(state.clone())),
])
.with_set_call_context_hook(|conversation_id, turn_id| {
    state.set_session_id(conversation_id);
    state.gate().on_call_context(conversation_id, turn_id);
})
.with_sandbox_hook(|sandbox| {
    if let Some(socket) = &sandbox.db_socket {
        state.set_db_socket(socket.clone());
    }
    state.set_db_auth_token(sandbox.db_auth_token.clone());
});
```

If you wrap `ActionSetProvider` in your own `ToolProvider` impl (needed
for shared state or deferred execution), forward every lifecycle method
you use — `set_sandbox`, `set_call_context`, `approve_permission`,
`allow_pattern`, `revoke_pattern`, `set_config`, `config_schema` — or
the hooks silently never fire.

## 4. Declare side effects and permissions

### Side effects

`side_effects` in `#[tool(...)]` is execution metadata: only
`"ReadOnly"` allows bounded parallel dispatch. Everything else — and an
omitted attribute ("unknown") — runs sequentially. Declare honestly:

| Action | Declaration | Why |
|---|---|---|
| Read-only lookup | `"ReadOnly"` | No observable effect; parallelizable |
| Stateful write that is not idempotent | *(none)* | Unknown → fail-closed sequential. There is no "DB write" category and claiming `ReadOnly` would mark a write parallelizable |
| Truly idempotent write | `"Idempotent"` | Same args ⇒ same effect |
| Data loss possible | `"Destructive"` | Prompts extra care; pair with the permission flow below |

### Permissions and destructive actions

Destructive or privacy-relevant actions must ask the user. The flow:

1. `run` calls an approval gate **before** performing the operation.
2. The gate returns `ToolError::PermissionRequired { request_id, action,
   target, description }` when the user has not approved.
3. The host prompts; on approval it calls `approve_permission(request_id)`
   (or `allow_pattern(action, target)` for "allow for this session") and
   **re-invokes the tool with identical arguments**.
4. The retried call must compute the *same* `request_id` and find the
   recorded approval — so ids are deterministic hashes of
   `action:target:description`, not random UUIDs.

```rust
self.state.gate().check(
    "MyToolDelete",                     // canonical action name
    "my_tool:delete",                   // stable target id, no private content
    "Delete the selected item",         // user-facing preview
)?;
```

The `ApprovalGate` in `plugins/tool/counter/src/approval.rs` implements
the full pattern: per-turn expiry driven by `set_call_context`,
session-wide allow patterns, revocation. `target` must be a stable
identifier without private content — the description is user-facing only
and never enters the host audit trail.

## 5. Timeouts

Every external call needs a bound:

- HTTP: set a client timeout (`reqwest::Client::builder().timeout(...)`)
  — see the geo tool (10 s) — and cap response body sizes.
- In-action waits: `tokio::time::timeout(Duration, future)` and map the
  elapsed case to `ToolError::timeout("...")`.
- DB/IO failures: map to `ToolError::internal(...)` (or `::io_error` for
  I/O) — never leak raw `DbError` strings into the model-facing error
  unless they are already user-meaningful.

## 6. Cancellation and deferred execution

`ToolAction` is synchronous request–response; the host cancels a deferred
task, not an in-flight sync call. For background work:

1. Declare `background_capable = true` on the action.
2. Replace `ActionSetProvider` with a hand-written `ToolProvider` that
   overrides `call_tool_deferred` (return `DeferredOutcome::Deferred {
   task_id }` after starting the work), `poll_deferred` (return
   `Pending`/`Completed`/`Cancelled`/`Unknown`), and `cancel_deferred`
   (signal the task's cancellation handle).

The utility tool's `TaskRegistry` is the reference implementation.
Cooperative cancellation: check the cancellation flag at natural
interruption points rather than aborting mid-mutation.

## 7. Stateful tools: DB IPC

Never open your own SQLite connection — the plugin talks to the shared
`memory.db` through the host-service `db` passenger over IPC
(`ene-plugin-db`).

1. Declare the schema once at store construction; table/index names must
   carry the plugin prefix (`my_tool_`):

```rust
DbSchema {
    prefix: "my_tool_".to_string(),
    tables: vec![DbTable { /* ... */ }],
    indexes: vec![],
}
```

2. Connect lazily from the sandbox handshake data (`db_socket` +
   `db_auth_token`), like the counter tool's `ensure_store()`.
3. Wrap the `DbClient` in `Arc<tokio::sync::Mutex<>>` — every operation
   takes `&mut self` and the store is shared across actions and
   concurrent calls.
4. Multi-row writes that must stand or fall together go through
   `DbClient::batch` (single transaction, capped at 10,000 ops).
5. Respect the per-plugin quota (`plugins.list.<name>.db_quota_mb`,
   default 256 MiB); handle `QUOTA_EXCEEDED` as an internal error and
   keep deletes ungated so a full plugin can free space.

## 8. Tests and mocks

### Unit tests (in the binary crate)

`#[cfg(test)] mod tests` inside the action/provider modules, with
`#[tokio::test]` for `run`. The crate-level
`#![cfg_attr(test, expect(clippy::unwrap_used, reason = "..."))]` in the
template covers test assertions.

### Mock recipes

- **DB**: abstract the store behind a trait
  (`CounterStore` in the counter sample) with an in-memory double
  (`InMemoryCounterStore`). Install it via a `#[cfg(test)]` seam
  (`CounterState::set_test_store`) so actions run without a DB server.
- **Permission denied**: run the action against a fresh gate and assert
  `ToolError::PermissionRequired`; then extract `request_id`, call
  `gate.approve_request(&request_id)`, run again, and assert success.
- **Invalid request**: call `execute` with malformed JSON (assert
  `InvalidArguments`) and with semantically invalid values (assert the
  `InvalidArguments` message from `run`).
- **Not found / dispatch**: call the provider with an unknown name and
  assert `ToolError::NotFound`.

### IPC integration tests

`plugins/tool/counter/tests/ipc.rs` is the recipe: spawn the real binary
(`env!("CARGO_BIN_EXE_ene-plugin-counter")`) with `ENE_PLUGIN_SOCKET`
set to a unique path, connect an `IpcStream`, perform the handshake, and
drive `ListTools` / `CallTool` / `ApprovePermission` over the wire. This
works in a bin-only crate — no `[lib]` target needed.

These tests intentionally run without a DB server: actions validate
arguments and check permissions *before* touching the store, so
permission-denied and invalid-request cases are exercised end-to-end,
and a post-approval retry fails with `ErrorKind::Internal` at the store
boundary, proving the sandbox→store wiring.

## 9. Register and verify

1. Build the binary (`cargo build -p ene-plugin-my_tool`); the host
   discovers plugins by executable name `ene-plugin-<name>`.
2. Enable the plugin: add `"my_tool": { "enable": true }` to
   `plugins.list` in `settings.json`, or to `default_plugin_list()` in
   `crates/ene-plugin-host/src/config.rs` for a built-in.
3. Run the app and check `/tool list` shows the new actions; `/tool call
   my_tool.greet '{"name":"Ene"}'` round-trips the action.

## 10. Compatibility, naming, logging

- Keep the plugin pinned to its build-time protocol version; the host
  negotiates N-1. Add fields with `#[serde(default)]`, never remove or
  rename wire variants.
- Plugin name: `[a-zA-Z0-9_-]`; binary `ene-plugin-<name>`; namespace
  `[a-zA-Z0-9_.:]` (hyphens become underscores); DB prefix `<name>_`.
- stdout is the IPC channel — `tracing` only, never `println!`. Log the
  fatal path with `tracing::error!` + `std::process::exit(1)`, and never
  log tokens or user-private content.

## Related

- [Tool SDK Reference](../reference/tools/sdk.md)
- [Plugins & MCP concepts](../../concepts/plugins-and-mcp.md)
- Per-tool guides: [Random](../guide/tools/random.md), [Geo](../guide/tools/geo.md),
  [Git](../guide/tools/git.md)
