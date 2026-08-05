# クイックスタート

このページでは、チェックアウトした状態からチャットが動くまでの手順を
説明します。ビルド環境の詳細はリポジトリの `AGENTS.md` を参照してください。

## 1. 必要なもの

- **Linux** が唯一の開発・CI 対象プラットフォームです。Windows は Linux
  からのクロスコンパイルで生成します（macOS は非対応）。
- **Rust 1.85 以上**（ワークスペースは edition 2024）。CI は stable
  ツールチェーンを使います。
- ネイティブ依存: Vulkan・ALSA・OpenSSL・`libclang`・`mold`・
  Wayland/X11 開発パッケージ。チェックイン済みの Nix flake が一式提供します:

```sh
nix develop --command cargo build --workspace
```

リポジトリで `direnv` が有効なら、素の `cargo` がそのまま使えます。

## 2. ビルド

```sh
# 全部（アプリ・クレート・プラグイン）
cargo build --workspace

# CLI だけ（最速の反復）
cargo build -p ene-cli
```

リリースビルド（`--release`）も対応しています。リリースプロファイルは
意図的に `panic = "unwind"` を維持しています。ランタイムの耐障害性が
これに依存しているためです（[アーキテクチャ](concepts/architecture.md#フォールトトレランス)参照）。

## 3. CLI を起動する

```sh
cargo run -p ene-cli
```

引数なしで起動すると対話型 REPL になります。`/help` でスラッシュコマンド
一覧を表示し、そのままメッセージを打つと設定中のキャラクターと会話できます。

最初に試すとよいコマンド:

```sh
# 環境ヘルスチェック（設定・プロバイダー・ストア・プラグイン）
cargo run -p ene-cli -- doctor

# ワンショット実行（非対話で実行して終了）
cargo run -p ene-cli -- run "こんにちは！"
```

### 初回起動時の動作

初回起動時に、同梱アセット（サンプルキャラクター・プロンプトパック）が
アセットディレクトリへ展開され、デフォルトの `settings.json` が書き出されます。
デバッグビルドではリポジトリの `assets/` を直接使い、リリースビルドでは
OS のアプリケーションデータディレクトリ（Linux は `~/.local/share/ene`、
Windows は `%APPDATA%\ene`）を使います。詳細は
[設定](configuration.md) を参照してください。

## 4. チャットプロバイダーを設定する

チャットには LLM プロバイダーが必要です。同梱のデフォルトカードは
`Alicia` で、デフォルトの `settings.json` はクラウドプロバイダー
`openrouter` 向けですが、**API キーは同梱されていません**。キーを追加するか、
ローカルモデルに切り替えてください。

### クラウドプロバイダー（API キーを環境変数で）

```sh
export OPENROUTER_API_KEY="sk-..."
cargo run -p ene-cli
```

設定値は `ENE_` 接頭辞の環境変数で上書きできます。ネストは `__` で区切ります。
例: チャットモデルの変更:

```sh
ENE_AI__TASKS__CHAT__MODEL="openai/gpt-5.6-luna" cargo run -p ene-cli
```

### 完全ローカル（ネットワーク不要）

`local-llm` プロバイダープラグインは、お使いのマシンで llama.cpp により
GGUF モデルを実行します。`settings.json` の `ai.tasks.*.provider = "local"`
と `ai.local_models.<name>` を編集するか、環境変数で上書きしてください。
モデルファイルは初回使用時に Hugging Face からダウンロードされます:

```sh
ENE_AI__TASKS__CHAT__PROVIDER="local" \
ENE_AI__TASKS__CHAT__MODEL="gemma-4-e2b" \
cargo run -p ene-cli
```

TTS/STT・埋め込みを含む完全なプロバイダー設定は
[設定 → AI](configuration.md#ai) と
[コンセプト → プラグイン](concepts/plugins-and-mcp.md) を参照してください。

## 5. デスクトップアプリを起動する

```sh
cargo run -p ene-desktop
```

3D VRM アバターとチャットペインを持つウィンドウが開きます。2 つの任意引数で
VRM モデルと VRMA モーションクリップを指定できます。機能とプラットフォーム
注意は[デスクトップガイド](apps/desktop.md)を参照してください。

## 6. セットアップを確認する

| 確認内容 | コマンド |
|---|---|
| 設定・プロバイダー・ストア・プラグインの健康状態 | `ene doctor`（REPL では `/doctor`） |
| 登録済みツール | `ene tool list`（REPL では `/tool list`） |
| 読み込まれたキャラクター | `ene characters` |
| メモリストア | `ene memory list` |
| テスト | `cargo test --workspace` |
| リント（CI ゲート） | `cargo clippy --workspace --all-targets -- -D warnings` |

## 次に読むもの

- ターンがシステム内をどう流れるか: [アーキテクチャ](concepts/architecture.md)
- 設定の調整: [設定](configuration.md)
- オリジナルキャラクターの追加: [キャラクターカード](concepts/character-cards.md)
- キャラクターに新しい能力を: [ツールを書く](guides/tools/write-a-tool.md)
