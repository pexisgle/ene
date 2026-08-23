# Rust ツールチェーンの更新

Ene のリポジトリ用ツールチェーンは **Rust 1.98.0** です。バージョンは
`rust-toolchain.toml`、同梱 Nix シェル、CI に固定しているため、新しい stable
リリースで lint の挙動がレビューなしに変わることはありません。

週次の dependency workflow は最新 stable を確認し、固定版が変わっていれば
更新用 pull request を作成します。生成された pull request では format、Clippy、
テスト、ドキュメント、ネイティブ Windows のチェックをすべて実行します。

意図的に更新するときは次の手順です。

1. リポジトリのルートで `scripts/update-rust-toolchain.sh` を実行する。
2. `nix flake lock --update-input rust-overlay` で Rust overlay を更新する。
3. `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` を実行する。
4. pull request に新しいバージョンとチェック結果を記録する。

更新 job は通常の pull request を準備するだけです。新しい lint を影響するコードと
一緒にレビューできるよう、更新は明示的に確認します。
