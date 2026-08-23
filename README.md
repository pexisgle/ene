# ene

ene は、ローカルで動く AI コンパニオン型エージェントハーネスです。コアデーモン
(`ene-core`) が会話・記憶・ツール・承認を持ち、stage / CLI / Web がその同一 API に
接続します。

## 概要
- **ワークスペース構成**: 複数クレートを含む Cargo ワークスペースです。
- **主なクレート / アプリ**:
  - `ene-session`: 追記専用の会話ログと usage 台帳
  - `ene-kernel`: 対話レーン (`prompt` / `steer` / `follow_up` / `abort` / `compact`)
  - `ene-companion`: soul・感情・記憶・内面・能動発話・キャラパッケージ
  - `ene-daemon` (`apps/ene-core`, バイナリ `ene-core`): コアデーモン
  - `ene-stage`: ネイティブ stage クライアント (egui + wgpu)
  - `ene-ctl`: CLI クライアント

## リポジトリ構造（抜粋）
- `crates/ene-session/` — セッションログ
- `crates/ene-kernel/` — 対話レーン
- `apps/ene-core/` — コアデーモン
- `apps/ene-stage/` — stage クライアント
- `apps/ene-ctl/` — CLI
- `plugins/tool/` — 同梱ツール (`fs` / `exec` / `web` / `utility` / `app` / `mcp`)
- `assets/` — サンプルキャラクターやアセット

## 前提条件
- Rust 1.98.0（ツールチェーンは `rust-toolchain.toml` に固定）。
- Linux は Nix を推奨します。checked-in の flake が Vulkan、ALSA、OpenSSL、clang、mold などのネイティブ依存を提供します。
- Windows は stable MSVC Rust、Visual Studio C++ Build Tools、Windows SDK が必要です。

## ビルド

Linux では Nix 開発シェル内でビルドしてください。

```bash
nix develop --command cargo build --workspace
```

`direnv` を有効化している場合は、リポジトリ内で plain `cargo` を実行できます。Native Windows では必要な Build Tools を入れた PowerShell から `cargo build --workspace` を実行してください。

## 実行

```bash
cargo run -p ene-ctl -- --help
cargo run -p ene-stage
cargo run -p ene-daemon
```

## テスト

```bash
cargo test --workspace
```

## 開発メモ
- ホスト契約は HTTP/WS (`ene-api`)。詳細は [`docs/index.md`](docs/index.md) および
  [`docs/concepts/architecture.md`](docs/concepts/architecture.md)。
- 設計文書は [`plans/harness-redesign/`](plans/harness-redesign/README.md)。
