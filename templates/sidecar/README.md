# Sidecar execution template

Starting point for a provider plugin that runs a local inference engine as a
child process ("sidecar") instead of linking the native runtime in-process.
This is the pattern used by `plugins/provider/llama-server` (llama.cpp server)
and `plugins/provider/voicevox` (VOICEVOX managed mode).

## When to use it

Use a sidecar when the engine:

- is an existing server binary (llama-server, whisper-server, VOICEVOX, ...),
- must keep running between requests (model resident in memory),
- needs its own process boundary for crash isolation, or
- is too large / impractical to link into the plugin binary.

Keep the engine in-process (no sidecar) when it is a small embedded runtime
with no server surface and one job at a time (e.g. an ONNX VAD).

## What the template provides

- `src/sidecar/lifecycle.rs` — spawn → health-check → timeout kill → Drop
  kill, with a spawn lock so concurrent requests cannot start two engines.
- `src/sidecar/preset.rs` — writing an engine preset/config file into a
  per-process work directory.
- `new-sidecar.sh` — copies the module into `plugins/provider/<name>/src/`
  with placeholder names replaced.
- `Cargo.toml` — the dependency block the provider crate needs.
- `CHECKLIST.md` — the integration checklist every sidecar must satisfy.

## Scaffolding

From the repository root:

```sh
templates/sidecar/new-sidecar.sh my-engine
```

This creates `plugins/provider/my-engine/src/sidecar/` (the plugin crate must
already exist). Then:

1. Declare `mod sidecar;` in `src/main.rs` (or the module that owns the
   provider).
2. Add the dependency block from `Cargo.toml` to the plugin's manifest.
3. Implement the engine-specific parts:
   - binary resolution (`resolve_binary`: explicit config path → host-injected
     artifact path → bundled plugins dir → `PATH`),
   - the health probe (`GET /health`-style check against the engine's API),
   - preset serialization (`preset.rs`),
   - the client that talks to the engine over loopback HTTP.
4. Walk `CHECKLIST.md` before enabling the plugin by default.

## Sandbox notes

The sidecar child runs inside the plugin's OS sandbox boundary: it inherits
the plugin's restricted filesystem view and has no direct network beyond the
loopback interface the plugin opens for it. The host delivers the CAS path of
catalog-managed sidecar binaries and model weights through the plugin config
(`server_path` / `model_path`); plugins must not fetch binaries from arbitrary
URLs.

Full architecture context: `docs/concepts/plugins-and-mcp.md` and
`plans/sandbox-and-downloads.md`.
