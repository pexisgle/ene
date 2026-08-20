# Testing

```sh
cargo test --workspace
cargo test -p ene-session
cargo clippy --workspace --all-targets -- -D warnings
```

A bare `cargo test` only covers `ene-ctl` (the default member). Always pass
`--workspace` or `-p <package>`. CI splits the workspace into three jobs
(core crates, apps, plugins).

Tests live next to the code (`#[cfg(test)]`) or in per-crate `tests/`
directories. They must not unwrap in production paths; test modules opt out
with `#![cfg_attr(test, expect(clippy::unwrap_used, ...))]`.

On NixOS, Cargo may not be on the host PATH — use the direnv wrapper from
the repo root, or `nix develop --command`.
