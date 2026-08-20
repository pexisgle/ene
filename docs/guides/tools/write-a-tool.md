# Write a tool

Harness tools are out-of-process binaries under `plugins/harness/<name>`.
They speak the split IPC in `ene-plugin-ipc` and register through
`ene_registry::run_plugin`.

```sh
cargo new --bin plugins/harness/my-tool
```

Copy `plugins/harness/fs` as the template: a `[[bin]]` only, no `[lib]`,
`ene-plugin-ipc` + `ene-registry`, and a `main` that calls
`ene_registry::run_plugin`. Use namespaced action names
(`my_tool.echo`) and declare `side_effects`. Empty `side_effects` is what
makes a tool eligible for the surface lane.

Verify with `ene-ctl` against a running `ene-core`. Update
[Built-in tools](builtin-tools.md) and the Japanese counterpart when you
add a bundled plugin.

A non-Rust fixture lives at `plugins/tool/dummy-py` (workspace-excluded).
Third-party tools must implement the same core+tool handshake; they do not
link Ene crates.
