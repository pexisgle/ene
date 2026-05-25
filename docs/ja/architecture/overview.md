# アーキテクチャ概要

ene は `ene-core` を中心としたモジュラーな Rust ワークスペースであり、
LLM 統合、ツール呼び出し、長期記憶、セッション管理を結合します。

## クレート依存関係グラフ

```
ene-desktop ──┐
ene-cli ──┼── ene-core ──── ene-tool-host ──── ene-tool-proto
            │                    │
            │               ene-tools/* (IPC 子プロセス)
            │
      ene-core 内部依存:
        ├── ene-config    (設定, パス, スキーマ生成)
        ├── ene-embedding (ベクトル埋め込み)
        ├── ene-memory    (長期記憶ストア)
        ├── ene-session   (会話履歴, 自動分割)
        ├── ene-tool-host (ツールプロセス管理, MCP)
        └── ene-tool-proto (プロトコル型, ToolProvider トレイト)
```

## レイヤー説明

### 設定レイヤー
- **`ene-config`** — `figment` ベースの JSON 設定。`define_config!` と `define_label_enum!` マクロを提供し、宣言的な設定構造体定義を可能にします。プラットフォーム対応のパス解決と自動 `settings.schema.json` 生成を管理します。

### コアランタイムレイヤー
- **`ene-core`** — 統一ランタイムファサード。すべてのサブシステムを `AiRuntime::init()` の背後にカプセル化します。`run_ai_with_tools()` でツールオーケストレーション付きのストリーミング LLM 補完を提供します。

### AI サブシステム
- **`ene-embedding`** — ベクトル埋め込み生成。`ApiEmbeddingProvider` (OpenAI 互換) と `GgufEmbeddingProvider` (candle/GGUF、ローカル、GPU 不要) の 2 つのバックエンド。
- **`ene-memory`** — SQLite + sqlite-vec エピソディック記憶。会話要約、キーファクト、ツール埋め込みをコサイン類似度ベクトル検索で保存。
- **`ene-session`** — 会話履歴バッファ、`CharacterCardV3` 読み込み、感情トークン解析 (`<|emo:name|>`)、およびタイムアウトと話題変化に基づく自動セッション分割。

### ツール基盤レイヤー
- **`ene-tool-proto`** — プロトコル契約。`ToolProvider` トレイト、`IpcRequest`/`IpcResponse` ワイヤ形式、`SandboxConfigData`、`run_tool_server()` ヘルパーを定義。
- **`ene-tool-host`** — ツールライフサイクル管理。ツールバイナリを子プロセスとして起動 (Unix ドメインソケット / Windows 名前付きパイプ)、クラッシュ耐性 (指数バックオフ、最大 5 回再起動) でラップ、MCP サーバー対応、埋め込み類似度による Tool RAG フィルタリングを提供。
- **`ene-tools-common`** — ツールクレートが消費する共通ユーティリティ (HTML→Markdown、スマート切り詰め)。

### ツールプロバイダ (IPC 子プロセス)
- **`ene-tools-fs`** — ファイルシステム操作: `read`, `write`, `edit`, `delete`, `glob`, `grep`, `patch`, `shell`, `undo`。全操作がサンドボックス設定を尊重。
- **`ene-tools-web`** — Web アクセス: `webfetch` (URL→text/markdown/html) と `websearch` (複数バックエンド)。
- **`ene-tools-utility`** — ユーティリティツール: `question`, `todo`, `get_current_time`, `get_system_info`。
- **`ene-tools-app`** — OS レベルの GUI 自動化: ウィンドウ管理、キーボード/マウス入力、スクリーンショット、クリップボード。
- **`ene-tools-browser`** — CDP 経由の Chromium 自動化: ナビゲーション、クリック、タイピング、コンテンツ抽出、スクリーンショット。

### アプリケーション
- **`ene-cli`** — 対話型ターミナル REPL。`/` コマンドでセッションとメモリを管理。
- **`ene-desktop`** — Bevy ベース GUI。VRM キャラクターレンダリング、常時最前面の透明オーバーレイ、システムトレイ、egui 設定 UI。

## データフロー (簡略)

```
ユーザー入力 → AiRuntime → 記憶検索 → build_messages()
    → LLM API (ストリーム) → AiStreamEvent パイプライン
        → TextDelta → 表示
        → ToolCall → IPC→ツールバイナリ → ToolCallResult → LLM API (ループ)
        → Finished

セッション境界チェック → summarize_conversation() → MemoryStore.insert_summary()
```
