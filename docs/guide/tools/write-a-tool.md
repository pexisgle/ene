# Write a Tool

Add a new tool plugin binary that ene can discover and call over IPC.

## Steps

1. **Create** a binary crate, e.g. `cargo new --bin plugins/tool/<name>` in this workspace (or an external repo using the published/git crates).
2. **Define actions** with `#[derive(ToolAction)]` / `ToolSpec` attributes on argument structs; implement `async fn run`.
3. **Provide** them via `ene_tool_common::ActionSetProvider` (or `ene_tool_common::prelude::*`) instead of hand-writing a dispatch loop.
4. **Wrap and serve** — wrap the provider with `ene_plugin::ToolPluginAdapter` and serve with `run_plugin_server(Box::new(ToolPluginAdapter(provider))).await` in `main`.
5. **Install** the binary where ene looks (`builtin_plugins_dir` / `user_plugins_dir`, e.g. under the app data `plugins/` folder). Binaries must follow the `ene-plugin-{name}` naming convention.
6. **Enable** it in settings under `plugins.list` with `"enable": true` and optional flattened config.
7. **Document** under [guide/tools](.) (EN) and `docs/ja/guide/tools/` (JA).
8. **Verify** with `cargo run -p ene-cli` → `/tool list`.

## Minimal enable snippet

```json
{
  "plugins": {
    "list": {
      "my-tool": { "enable": true }
    }
  }
}
```

## Dig deeper

- [SDK guide](../../reference/tools/sdk.md) — full walkthrough and adapters
- [Derive macro](../../reference/tools/derive-macro.md)
- [Tool IPC overview](../../reference/tools/overview.md)
- [Tool catalog](overview.md)
