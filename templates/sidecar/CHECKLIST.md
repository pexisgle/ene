# Sidecar integration checklist

Every sidecar-enabled provider must satisfy all of these before it ships:

- **Binary resolution order**: explicit config `server_path` → host-injected
  artifact path (catalog-managed) → bundled `plugins/<name>` directory →
  `PATH`. Never download a sidecar binary from an arbitrary URL.
- **Loopback only**: the engine binds `127.0.0.1` on a free port; the plugin
  generates a per-process API key when the engine supports one.
- **Spawn lock**: concurrent requests must not spawn two engines. Hold the
  lock across the whole startup wait.
- **Health probe**: poll the engine's health endpoint (default interval
  250 ms); on timeout, kill and reap the child and clean the work directory.
- **Drop / config-change kill**: `Drop` must kill the child synchronously
  (`start_kill`), and a host config change must restart the sidecar via the
  same reset path.
- **Work directory**: per-process directory under the plugin's temp/data
  area; presets are written there and the directory is removed on restart.
- **Model resolution**: catalog-managed weights arrive as `model_path`
  (host-injected CAS path); URL weights go through `ene_ai::ModelFetcher`
  (HTTPS-only, magic-byte validation, `.part` + atomic rename).
- **Sandbox**: the sidecar inherits the plugin's OS sandbox; the CAS root is
  read-only. No host environment variables or credentials are forwarded.
- **Tests**: spawn/health/timeout/Drop contract tests plus an end-to-end
  test against a mock engine (see `plugins/provider/voicevox/src/tests.rs`).
