# アーキテクチャ概要

ene は `ene-core` を中心としたモジュラーな Rust ワークスペースであり、
LLM 統合、ツール呼び出し、長期記憶、セッション管理を**チャネルベースのメッセージパッシングによるアクターパターン**で結合します。

## クレート依存関係グラフ

```
ene-desktop ──┐
ene-cli ──┼── ene-core ──── ene-tool-host ──── ene-tool-proto
            │                    │                 │
            │               ene-tools/*            ene-tool-derive
            │          (IPC 子プロセス)       (proc-macro)
            │
      ene-core 内部依存:
        ├── ene-config    (設定, パス, スキーマ生成)
        ├── ene-embedding (ベクトル埋め込み)
        ├── ene-memory    (長期記憶ストア)
        ├── ene-session   (会話履歴, 自動分割)
        ├── ene-provider  (LLM + 埋め込みトレイト, OpenAI 実装)
        └── ene-tool-host (ツールプロセス管理, MCP, Tool RAG)
```

## レイヤー説明

### 設定レイヤー
- **`ene-config`** — `figment` ベースの JSON 設定。`define_config!` と `define_label_enum!` マクロを提供し、宣言的な設定構造体定義を可能にします。プラットフォーム対応のパス解決と自動 `settings.schema.json` 生成を管理します。

### コアランタイムレイヤー
- **`ene-core`** — 統一ランタイムファサード。**アクターベースアーキテクチャ**とチャネルベースのメッセージパッシングを使用します。`EneHandle` が公開 API であり、バックグラウンドの `EneActor` タスクを生成します。コンシューマーは `EneCommand`（mpsc）で通信し、`EneEvent`（broadcast）でイベントを受信します。アクターセッション、設定、ツールレジストリを所有し、ストリーミング、ツールオーケストレーション、権限管理、セッション分割を内部で管理します。

### AI サブシステム
- **`ene-embedding`** — ベクトル埋め込み生成。`CloudEmbeddingProvider` (OpenAI 互換 API) と `GgufEmbeddingProvider` (candle/GGUF、ローカル、GPU 不要) の 2 つのバックエンド。
- **`ene-memory`** — SQLite + sqlite-vec エピソディック記憶。会話要約、キーファクト、ツール埋め込みをコサイン類似度ベクトル検索で保存。
- **`ene-session`** — 会話履歴バッファ、`CharacterCardV3` 読み込み、感情トークン解析 (`<|emo:name|>`)、およびタイムアウトと話題変化に基づく自動セッション分割。

### ツール基盤レイヤー
- **`ene-tool-proto`** — プロトコル契約。`ToolProvider` トレイト、`ToolSpec`/`ToolError` 型、`IpcRequest`/`IpcResponse` ワイヤ形式 (v2)、`SandboxConfigData`、`run_tool_server()` ヘルパーを定義。
- **`ene-tool-derive`** — Proc-macro クレート。`#[derive(ToolSpec)]` が引数構造体の宣言的属性から `ToolSpec` 実装を生成。
- **`ene-tool-host`** — ツールライフサイクル管理。ツールバイナリを子プロセスとして起動 (Unix ドメインソケット / Windows 名前付きパイプ)、クラッシュ耐性 (指数バックオフ、最大 5 回再起動) でラップ、MCP サーバー対応、`ToolRag` 構造体を介した Tool RAG フィルタリング (HyDE、LLM リランキング、カテゴリ別制限) を提供。
- **`ene-tool-common`** — ツールクレートが消費する共通ユーティリティ (`ToolAction` トレイト、HTML→Markdown 抽出)。
- **`ene-provider`** — LLM・埋め込みプロバイダトレイト (`LlmProvider`, `EmbeddingProvider`)、OpenAI 互換実装、HyDE/リランキング用 `HybridRerankProvider`。

### ツールプロバイダ (IPC 子プロセス)
- **`ene-tool-fs`** — ファイルシステム操作: `read`, `write`, `edit`, `delete`, `glob`, `grep`, `patch`, `shell`, `undo`。全操作がサンドボックス設定を尊重。
- **`ene-tool-web`** — Web アクセス: `webfetch` (URL→text/markdown/html) と `websearch` (複数バックエンド)。
- **`ene-tool-utility`** — ユーティリティツール: `question`, `todo`, `get_current_time`, `get_system_info`。
- **`ene-tool-app`** — OS レベルの GUI 自動化: ウィンドウ管理、キーボード/マウス入力、スクリーンショット、クリップボード。
- **`ene-tool-browser`** — CDP 経由の Chromium 自動化: ナビゲーション、クリック、タイピング、コンテンツ抽出、スクリーンショット。

### アプリケーション
- **`ene-cli`** — 対話型ターミナル REPL。`/` コマンドでセッションとメモリを管理。
- **`ene-desktop`** — Bevy ベース GUI。VRM キャラクターレンダリング、常時最前面の透明オーバーレイ、システムトレイ、egui 設定 UI。

## データフロー

```
ユーザー入力
  ↓
コンシューマーが EneCommand::Run { input } を送信
  ↓
EneActor がコマンドを受信
  ↓
記憶検索 → build_messages()
  ↓
ストリームタスク生成 → LLM API (ストリーム)
  ↓
EneEvent パイプライン (broadcast チャンネル):
  → TextDelta → 表示
  → SpecialToken → 感情処理
  → ToolCallStart → ツール実行 → ToolCallResult → LLM API (ループ)
  → PermissionRequired → ユーザー承認 → PermissionDecision
  → UserInputRequired → ユーザー応答 → UserInputResponse
  → Finished
  ↓
ストリームタスクが更新されたセッションを oneshot で送信
  ↓
アクターがセッションを更新、StatusChanged { Idle } を送出
```

## アーキテクチャ

アクターパターンによりスレッド安全性と関心の分離を実現:

| コンポーネント | 役割 |
|-------------|------|
| `EneHandle` | スレッドセーフな公開 API。mpsc でコマンド送信、broadcast でイベント受信。 |
| `EneActor` | バックグラウンドタスク。全変更可能状態 (セッション、設定、レジストリ) を所有。 |
| `EneCommand` | コンシューマー → アクターメッセージ (Run, Cancel, Reconfigure, LoadCharacter, ListTools, CallTool など) |
| `EneEvent` | アクター → コンシューマーイベント (TextDelta, ToolCall*, PermissionRequired, UserInputRequired, Finished など) |
| `stream::run_stream` | Run コマンドごとに生成される内部ストリーミングエンジン。更新されたセッションを oneshot で返す。 |

利点:
- **グローバル状態なし** — 全状態はアクターが所有
- **スレッドセーフ** — チャネルベース通信、ホットパスでのミューテックス競合なし
- **Bevy 対応** — `try_recv()` でノンブロッキング ECS ポーリング、`subscribe()` で複数コンシューマー対応
- **ライフサイクル管理** — 全ハンドルがドロップされるとアクターが終了
