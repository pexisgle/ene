# ene

ene は、ローカルで動く AI コンパニオン型エージェントハーネスです。コア
(`ene-core`) が会話・記憶・ツール・承認を持ち、stage / CLI / Web が同一 API に
接続します。

## 概要
- **ワークスペース構成**: 複数クレートを含む Cargo ワークスペースです。
- **主なクレート / アプリ**:
  - `ene-session`: 追記専用の会話ログと usage 台帳
  - `ene-kernel`: 対話レーン (`prompt` / `steer` / `follow_up` / `abort` / `compact`)
  - `ene-companion`: soul・感情・記憶・内面・能動発話・キャラパッケージ
  - `ene-access-control`: 承認・監査・credential vault
  - `ene-tool-registry`: 統一ツールレジストリ
  - `ene-plugin-host`: プラグインプロセスの監督と host-side composition
  - `ene-core`: コアプロセス（HTTP/WS、データディレクトリのロック）
  - `ene-stage`: 製品 GUI
  - `ene-ctl`: CLI クライアント

## リポジトリ構造（抜粋）
- `crates/` — ライブラリクレート
- `apps/ene-core/` — コアプロセス
- `apps/ene-stage/` — stage クライアント
- `apps/ene-ctl/` — CLI
- `plugins/tool/` — 同梱ツール (`fs` / `exec` / `web` / `utility` / `app` / `mcp`)
- `plugins/provider/` — LLM / embedding / TTS / STT provider
- `docs/requirements/` — 今後の要件定義の正本
- `docs/concepts/` — 現在の実装を説明する資料（要件ではない）
- `assets/` — サンプルキャラクターやアセット

## 前提条件
- Rust 1.98.0（ツールチェーンは `rust-toolchain.toml` に固定）。
- Linux は Nix を推奨します。checked-in の flake がネイティブ依存を提供します。
- Windows は stable MSVC Rust、Visual Studio C++ Build Tools、Windows SDK が必要です。

## ビルド

```bash
nix develop --command cargo build --workspace
```

`direnv` を有効化している場合は plain `cargo` を実行できます。Native Windows では必要な Build Tools を入れた PowerShell から `cargo build --workspace` を実行してください。

## 実行

```bash
cargo run -p ene-ctl -- --help
cargo run -p ene-stage
cargo run -p ene-core
```

## テスト

```bash
cargo test --workspace
```

## 開発メモ
- 現在実装の入口は [`docs/index.md`](docs/index.md)。
- 今後の製品要件は [`docs/requirements/`](docs/requirements/README.md) で対話的に確定します。
- 過去の大規模な設計計画は working tree から削除し、必要なときだけ Git 履歴を参照します。
