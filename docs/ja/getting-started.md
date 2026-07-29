# Ene スタートアップガイド

本ガイドでは、システム要件、開発環境のセットアップ、ワークスペースのビルド、およびアプリケーション (`ene-cli` および `ene-desktop`) の起動手順について解説します。

---

## 開発環境と前提条件

Ene は Rust 2024 エディションのワークスペースとして構築されており、以下のシステムツールが必要です：

- **Rust ツールチェーン**: 1.85 以上 (Rust 2024 エディション対応)
- **C/C++ コンパイラ**: `clang` / `gcc` および `cmake` (`llama-cpp-4` や `libsqlite3-sys` のビルド用)
- **グラフィックスライブラリ**: Vulkan / Wayland / X11 開発用ヘッダー (`ene-desktop` および `ene-vrm` の `wgpu` 用)
- **オーディオライブラリ**: `alsa` / `jack` 開発用ヘッダー (`ene-voice` の `cpal` 用)

### 推奨環境: Nix + direnv (Linux)

リポジトリには完全再構築可能な Nix flake 設定がチェックインされています：

```bash
# direnv に設定ファイルの読み込みを許可
direnv allow

# Rust / Cargo のバージョン確認
cargo --version
```

または、直接 Nix 環境内でコマンドを実行することも可能です：

```bash
nix develop --command cargo check
```

---

## ワークスペースのビルド

Ene は複数のワークスペースパッケージで構成されています。 `apps/ene-cli` がデフォルトメンバーに指定されているため、 `--workspace` または `-p <パッケージ名>` を指定しない標準の `cargo` コマンドは `ene-cli` を対象とします。

### コンパイルチェック

```bash
# デフォルトパッケージ (ene-cli) のチェック
cargo check

# ワークスペース全体のチェック
cargo check --workspace
```

### バイナリのビルド

```bash
# CLI REPL のビルド
cargo build -p ene-cli

# デスクトップ GUI のビルド
cargo build -p ene-desktop

# 全ツールおよびプロバイダプラグインのビルド
cargo build --workspace --bins
```

---

## アプリケーションの実行

### 1. Ene CLI (`ene-cli`)

CLI アプリケーションは、Ene との対話、記憶の検査、セッション管理、ツールプラグインのテストを行える対話型 REPL を提供します。

```bash
# デフォルト設定および組み込みキャラクターカードで実行
cargo run -p ene-cli

# カスタムキャラクターカードまたは設定を指定して実行
cargo run -p ene-cli -- --character Alicia
```

#### 便利な REPL コマンド
- `/help` — 使用可能な REPL スラッシュコマンド一覧を表示。
- `/memory list` — アクティブセッションの想起記憶ファクトを表示。
- `/tool list` — 登録済みツールプラグインおよび有効な MCP サーバーを表示。
- `/session archive` — 現在のセッションをアーカイブしコンテキストをリセット。

---

### 2. Ene Desktop (`ene-desktop`)

`ene-desktop` は、3D VRM アバターのアニメーション、音声合成、リアルタイム感情・パフォーマンス演出を備えた GUI デスクトップアプリを起動します。

```bash
# デスクトップ GUI の実行
cargo run -p ene-desktop
```

---

## ワークスペースの検証

コードの修正をコミットする前には、コードフォーマット、Lint、テストスイートの検証を行ってください：

```bash
# 1. フォーマットチェック
cargo fmt --all -- --check

# 2. ワークスペース Lint
cargo clippy --workspace -- -D warnings

# 3. ワークスペース ユニット & 統合テスト
cargo test --workspace
```

---

## 次のステップ

- [システムアーキテクチャ](architecture.md) で全体の設計を確認する。
- [設定ガイド](configuration.md) で LLM API キー (OpenAI, Anthropic, Ollama) を設定する。
- [記憶システム](concepts/memory-system.md) および [IPC プラグインシステム](concepts/plugins-and-mcp.md) の詳細を学ぶ。
