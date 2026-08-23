# Rust toolchain updates

Ene uses Rust **1.98.0** for the repository toolchain. The version is pinned
in `rust-toolchain.toml`, the checked-in Nix shell, and CI so a new stable
release cannot change lint behavior without a reviewable change.

The weekly dependency workflow checks the latest stable release and opens an
update pull request when the pinned version changes. The generated pull
request must run the complete format, Clippy, test, documentation, and native
Windows checks before merging it.

For a deliberate update:

1. Run `scripts/update-rust-toolchain.sh` from the repository root.
2. Update the Rust overlay input with `nix flake lock --update-input rust-overlay`.
3. Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`.
4. Record the new version and check results in the pull request.

The update job only prepares a normal pull request; toolchain bumps remain
reviewable so new lints are considered with the code they affect.
