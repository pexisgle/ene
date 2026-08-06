# Write a tool plugin

This guide creates a new tool plugin from the repository template. A tool
plugin is a small Rust binary that exposes named actions to the character.

## 1. Scaffold

```sh
templates/tool/new-tool.sh my_tool
```

This creates `plugins/tool/my_tool/` with:

- crate/binary name `ene-plugin-my_tool`,
- tool namespace `my_tool` (override with a second argument),
- a provider struct with one `my_tool.echo` action,
- workspace membership (the workspace `members` glob picks it up).

## 2. The anatomy of a tool plugin

```text
plugins/tool/my_tool/
├── Cargo.toml        # bin crate, deps: ene-plugin, serde, schemars, tokio
└── src/
    ├── main.rs       # run_plugin_server(PluginDispatch::new(...))
    ├── provider.rs   # ActionSetProvider: the action list + lifecycle
    └── action.rs     # one struct per action, deriving ToolAction
```

An action is a plain struct: its fields are the JSON arguments, its
`#[tool(...)]` attribute declares the schema metadata, and its `run`
method implements the behaviour:

```rust
use ene_plugin::prelude::*;

#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "my_tool",
    name = "echo",
    summary = "Echo text back.",
    description = "Returns the input text unchanged. Use for testing.",
    category = "Utility"
)]
pub struct EchoAction {
    /// Text to echo.
    #[arg(min_length = 1, max_length = 2000)]
    text: String,
}

impl EchoAction {
    async fn run(&self) -> Result<String, ToolError> {
        Ok(self.text.clone())
    }
}
```

The provider registers actions and is served by `run_plugin_server`:

```rust
use ene_plugin::{ActionSetProvider, PluginDispatch, run_plugin_server};

struct MyToolProvider;

impl ActionSetProvider for MyToolProvider {
    fn actions(&self) -> Vec<Box<dyn ToolAction>> {
        vec![Box::new(EchoAction::default())]
    }
}

#[tokio::main]
async fn main() -> Result<(), PluginError> {
    run_plugin_server(PluginDispatch::new(
        Some(Arc::new(MyToolProvider)),
        None, None, None, None,
    ))
    .await
}
```

## 3. Declaring side effects and background work

The host needs to know what an action touches:

```rust
#[tool(
    namespace = "fs",
    name = "write",
    side_effects = "FileSystem { mutates: true }"
)]
```

Side-effecting actions go through the permission gate (the user approves
once, for the session, or permanently). Actions that run long can declare
`background_capable` — the host then executes them as deferred tasks and
reports completion on the lifecycle bus instead of blocking the turn.

See the [Tool SDK reference](../../reference/tools/sdk.md) for the full
attribute surface.

## 4. Stateful tools

If your tool needs persistent state, use the `db` host service through
`ene-plugin-db`: your plugin gets token-authenticated, prefix-isolated CRUD
inside the host's `memory.db` — no local files, no own database. The
`plugins/tool/counter` sample is the reference implementation (state,
permission gate, DB IPC, integration tests).

## 5. Register and verify

```json
{
  "plugins": {
    "list": {
      "my_tool": { "enable": true }
    }
  }
}
```

```sh
cargo build -p ene-plugin-my_tool
cargo run -p ene-cli -- tool list          # my_tool.echo appears
# in the REPL:
/tool call my_tool.echo '{"text": "hi"}'
```

## 6. House rules

- Names are namespaced: `<namespace>.<action>`.
- Plugin crates are **binary-only** (no `[lib]` target). Add a lib only
  when an integration test or another workspace crate must link the logic.
- Keep the docs in sync: the tool appears in
  [Built-in tools](builtin-tools.md) when it ships in-repo, and the
  bilingual pages under `docs/` / `docs/ja/` are updated with it.
- Lints are the spec: run `cargo clippy --workspace --all-targets -- -D warnings`
  before pushing.
