# ene

ene は、ローカルでキャラクター（VRM / VRMA）を扱い、AIを使った会話やアプリ・CLI を提供する Rust ワークスペースです。

## 概要
- **ワークスペース構成**: このリポジトリは複数クレートを含む Cargo ワークスペースです。
- **主なクレート**:
- `ene-runtime`: ホストファサード（`EneHandle::open`、`TurnId`、チャットイベント、診断）。mind / store / AI / tool-host を束ねる。
- `ene-mind`: 認知ターンパイプライン（セッション、recall、affect、Performance 調停、メモリ書き込み）。
- `ene-store`: SQLite（sea-orm）永続化の専有クレート。
- `ene-ai`: LLM + 埋め込みプロバイダ。
- `ene-desktop`: デスクトップ GUI アプリ（winit + wgpu + egui）。
- `ene-cli`: CLI クライアント（ヘッドレスやスクリプト用途）。

## リポジトリ構造（抜粋）
- `crates/ene-runtime/` — ホストランタイム
- `crates/ene-mind/` — 認知パイプライン
- `crates/ene-store/` — メモリストア
- `crates/ene-ai/` — LLM / 埋め込み
- `apps/ene-desktop/` — GUI アプリケーション
- `apps/ene-cli/` — コマンドラインインターフェース
- `assets/` — サンプルキャラクターやアセット（`characters/`、`vrm/`、`vrma/` 等）

## 前提条件
- Rust（ツールチェーンは `rust-toolchain.toml` を参照）。
- ネイティブ依存（GUI をビルドする場合は GTK や Wayland/Windows 開発ヘッダ等が必要になることがあります）。

## ビルド
ワークスペース全体をビルドするには:

```bash
cargo build --workspace
```

リリースビルド:

```bash
cargo build --workspace --release
```

## 実行
GUI アプリを実行するには:

```bash
cargo run -p ene-desktop --release
```

CLI を実行するには（オプションは `--help` で確認してください）:

```bash
cargo run -p ene-cli -- --help
cargo run -p ene-cli -- <args>
```

## テスト
ワークスペースのテストを実行するには:

```bash
cargo test --workspace
```

## 開発メモ
- ホスト契約は API v1（`EneHandle::open`、必須 `TurnId`）。詳細は [`docs/index.md`](docs/index.md) および [`docs/architecture.md`](docs/architecture.md)。
- GUI は `winit` + `wgpu` + `egui`（状態管理に `bevy_ecs`）を利用しています。

## 資産（assets）
プロジェクトには `assets/characters`、`assets/vrm`、`assets/vrma` にサンプルファイルが含まれます。カスタムキャラクターを追加する場合はこれらのフォルダを参照してください。
