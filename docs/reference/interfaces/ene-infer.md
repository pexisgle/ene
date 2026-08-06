# `ene-infer` interface

## Role

Framework for running **synchronous, single-threaded local models** behind
a uniform async API: dedicated worker thread, bounded queue, cooperative
cancellation, one timeout, and panic recovery.

## Key public items

| Item | Contract |
|---|---|
| `LocalModel` | Sync trait implementors write: `run(&mut self, req, ctx) -> Result<Response, Error>` (+ optional `reset`) |
| `StreamingLocalModel` | Streaming variant for token/audio output |
| `EngineHandle<M>` | `spawn(factory, config)` owns the model on a worker thread; `submit(req, token)` is the bounded, non-blocking async entry |
| `EngineConfig` | `job_timeout` (enforced cooperatively inside the worker) |
| `JobContext`, `StopReason` | Per-job context and stop signalling |
| `EngineError` | `Busy` (queue full), `EngineDown` (panic recovery), timeout, cancelled, … |
| `ChunkReceiver`, `ChunkSink` | Streaming chunk plumbing |
| `conformance::run_all` | Generic regression battery for `LocalModel` implementations (feature `test-util`) |

## Dependencies

- Depends on: nothing internal (tokio, tokio-util, thiserror, tracing).
- Used by: `ene-ai` (`engine_adapter`), `ene-voice` (STT/TTS/VAD engines),
  and re-exported through `ene_plugin::prelude` for plugin authors.

## Refactoring notes

- The worker invariants are the point of the crate: model owned by one
  thread, bounded queue that fails fast, exactly one cooperative timeout,
  `catch_unwind` with model rebuild. Do not add an outer timeout or
  `spawn_blocking` wrappers around `submit` — that is the bug this crate
  exists to prevent.
- `catch_unwind` cannot contain native `abort()` (e.g. `GGML_ASSERT`);
  providers must validate inputs before native code.
- The `conformance` battery is how engine behaviour is pinned across
  providers — run it when changing worker semantics.
