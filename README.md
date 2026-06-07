# ene

ene は、ローカルでキャラクター（VRM / VRMA）を扱い、AIを使った会話やアプリ・CLI を提供する Rust ワークスペースです。

## 概要
- **ワークスペース構成**: このリポジトリは複数クレートを含む Cargo ワークスペースです。
- **主なクレート**:
- `ene-core`: LLM対話、セッション管理、長期記憶、ツール実行基盤を統合したコアライブラリ。
- `ene-desktop`: デスクトップ GUI アプリ（Bevy ベース）。
- `ene-cli`: CLI クライアント（ヘッドレスやスクリプト用途）。

## リポジトリ構造（抜粋）
- `crates/ene-core/` — コアライブラリ
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
- `crates/ene-core` は `async-openai` 等を使い非同期でモデルと通信します。
- GUI は `bevy` と `bevy_vrm1`、`bevy_egui` 等を利用しています。

## 資産（assets）
プロジェクトには `assets/characters`、`assets/vrm`、`assets/vrma` にサンプルファイルが含まれます。カスタムキャラクターを追加する場合はこれらのフォルダを参照してください。
