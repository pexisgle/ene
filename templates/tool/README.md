# Tool plugin template

Minimal starting point for a new tool plugin. The scaffolded crate is a
workspace member (`plugins/tool/*`), follows the binary-only plugin
convention, and registers under the `ene-plugin-<name>` binary naming
rule.

## Usage

From the repository root:

```sh
templates/tool/new-tool.sh my_tool
```

This creates `plugins/tool/my_tool/` with:

- crate and binary `ene-plugin-my_tool`,
- tool namespace `my_tool` (pass a second argument to override),
- provider struct `MyToolToolProvider` with one `my_tool.echo` action.

The crate-level `#[expect(clippy::unused_async, ...)]` covers sync-bodied
`run` methods — the template action awaits nothing, so the expectation
is needed as shipped. Once every action awaits, remove the expect: an
unfulfilled expectation fails the build.

Then implement your actions in `src/action.rs`, wire lifecycle hooks in
`src/provider.rs`, and register the plugin:

1. Add `"my_tool": { "enable": true }` to `plugins.list` in
   `settings.json` (or to `default_plugin_list()` in
   `crates/ene-plugin-host/src/config.rs` for a built-in).
2. Build and run the app, then verify with `/tool list`.

Full guidance: `docs/guide/tools/write-a-tool.md` (en/ja) and the
reference `docs/reference/tools/sdk.md` (en/ja). For stateful tools with
DB IPC, permission gates, and integration tests, see the
`plugins/tool/counter` sample.
