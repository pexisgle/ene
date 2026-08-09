# Testing

Ene's test suite follows the classic test pyramid: many fast unit tests,
a solid layer of integration tests, and a few heavy end-to-end sidecar
contracts that skip themselves when the environment cannot run them.

## Test layers

| Layer | Where | What it covers | Count |
|---|---|---|---|
| Unit | `#[cfg(test)]` modules next to the code | Pure logic, algorithms, config, formatting | ~3,400 |
| Integration | `tests/` directories per crate | Public APIs, IPC contracts, DB-backed flows through real handles | ~240 |
| Sidecar E2E | `plugins/provider/llama-server/tests`, `plugins/provider/local-llm/tests` | Real plugin binaries over IPC with pinned GGUF fixtures | few |

Unit tests avoid the network, the filesystem, and the clock wherever
possible. Where a wall clock is unavoidable (the persistent scheduler),
tests inject a virtual clock and poke the scheduler instead of sleeping.

## Running tests

Cargo is not on the host PATH on NixOS; use the direnv wrapper from the
repo root:

```sh
rtk direnv exec . rtk cargo test --workspace        # everything
rtk direnv exec . rtk cargo test -p ene-runtime     # one crate
rtk direnv exec . rtk cargo test -p ene-runtime --test scheduler
rtk direnv exec . rtk cargo clippy --workspace --all-targets -- -D warnings
```

A bare `cargo test` only covers `ene-cli` (the default member). Always
pass `--workspace` or `-p <package>` explicitly. CI splits the workspace
into three test jobs (core crates, apps, plugins) so a failing or flaky
package can be rerun in isolation.

## Test policies

- **Snapshots (insta)**: stable output contracts — the session export JSON
  and the composed prompt packet — are locked with snapshots under
  `src/snapshots/`. A wording or format change fails the test until the
  snapshot is reviewed (`cargo insta review`) and committed.
- **Property tests (proptest)**: secret redaction (`ene-connector`) and
  text truncation (`ene-util`) use randomized property tests in addition to
  example tests. New pure-logic crates should consider the same.
- **Sidecar fixtures**: llama.cpp contract tests download pinned GGUF
  fixtures into a blake3-verified cache. If the network or the sidecar
  binary is unavailable the tests skip; a pinned-hash mismatch fails loudly.
- **Flaky tests**: rerun the job once. If it reproduces, fix the timing
  dependence deterministically (virtual clock, controlled release, bounded
  polling) — raising a timeout is the last resort, not a fix.
