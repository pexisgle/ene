# ene

ene は、ローカルで動く AI コンパニオン型エージェントハーネスです。

## 状態

現在、プロダクト要件と設計を再定義しています。望ましい挙動の正は [`docs/requirements/`](docs/requirements/README.md) で対話的に再構築します。既存コードは現在実装の証拠であり、旧設計文書は Git 履歴に残しますが、現行仕様とはみなしません。

要件定義中はドキュメントを一時的に日本語のみで管理します。英語版は要件と設計が安定してから再整備します。

## ワークスペース

Cargo workspace は `crates/*`, `apps/*`, `plugins/tool/*`, `plugins/provider/*` から構成されます。各 package の正確な依存関係と公開 API は `Cargo.toml` と rustdoc (`cargo doc --workspace --no-deps`) を参照してください。

## 前提条件

- Rust toolchain は `rust-toolchain.toml` に固定されています。
- Linux では checked-in Nix flake を利用できます。
- native Windows build には MSVC Rust toolchain、Visual Studio C++ Build Tools、Windows SDK が必要です。

## ビルド / テスト

```bash
nix develop --command cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## 要件定義

- [`docs/requirements/`](docs/requirements/README.md)

旧 planning / design documents は working tree から削除し、必要な場合のみ Git 履歴から参照します。
