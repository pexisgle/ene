# Sidecar execution template

Starting point for a provider plugin that runs a local inference engine as a child process ("sidecar") instead of linking the native runtime in-process.

This template documents the mechanics that exist in the current code. It is not a product or architecture requirement. While requirements are being rebuilt, verify integration contracts against `ene-plugin-host`, the provider plugin implementations, tests, and rustdoc rather than historical design documents.

## Scaffolding

From the repository root:

```sh
templates/sidecar/new-sidecar.sh my-engine
```

This creates `plugins/provider/my-engine/src/sidecar/` for an existing provider crate. Adapt the generated lifecycle, health probe, preset/config handling, and engine client to the actual provider implementation, then verify the result against workspace tests and lint gates.
