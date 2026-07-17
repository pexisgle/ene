# Write a Tool

Add a new tool binary that ene can discover and call over IPC.

## Steps

1. **Create** a binary crate, e.g. `cargo new --bin tools/<name>` in this workspace (or an external repo using the published/git crates).
2. **Define actions** with `#[derive(ToolAction)]` / `ToolSpec` attributes on argument structs; implement `async fn run`.
3. **Provide** them via `ene_tool::ActionSetProvider` (or `ene_tool::prelude::*`) instead of hand-writing a dispatch loop.
4. **Serve** with `run_tool_server(Box::new(provider)).await` in `main` — always a boxed `dyn ToolProvider`, not a generic `run_tool_server::<T>()`.
5. **Install** the binary where ene looks (`builtin_tools_dir` / `user_tools_dir`, e.g. under the app data `tools/` folder).
6. **Enable** it in settings under `tools.tools` with `"enable": true` and optional `config`.
7. **Document** under [guide/tools](.) (EN) and `docs/ja/guide/tools/` (JA).
8. **Verify** with `cargo run -p ene-cli` → `/tool list`.

## Minimal enable snippet

```json
{
  "tools": {
    "tools": {
      "my-tool": { "enable": true, "config": {} }
    }
  }
}
```

## Dig deeper

- [SDK guide](../../reference/tools/sdk.md) — full walkthrough and adapters
- [Derive macro](../../reference/tools/derive-macro.md)
- [Tool IPC overview](../../reference/tools/overview.md)
- [Tool catalog](overview.md)
