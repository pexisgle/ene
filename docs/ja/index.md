# ene ドキュメント

ene は Rust ワークスペースで実装されたローカル AI キャラクタープラットフォームです。
LLM 駆動の会話、アニメーションする VRM キャラクター、ツール拡張エージェント機能、
長期記憶、自動セッション管理を提供します。

## はじめに

- [アーキテクチャ概要](architecture/overview.md) — クレートマップと依存関係グラフ
- [API v2](architecture/api-v2.md) — ロック済みホスト契約: `EneHandle::open`、`TurnId`、single-flight Busy、最小イベント
- [起動フロー](architecture/startup.md) — デスクトップ (winit+wgpu+egui) と CLI のブートシーケンス
- [認知ランタイムアーキテクチャ](architecture/cognitive-runtime.md) — Identity Kernel、型付きメモリ、感情、表情調停の設計に関するADR
- [API リファクタリング計画](architecture/api-refactor-plan.md) — 歴史的な再構成メモ（ホスト/クレートマップは API v2 が優先）
- [設定](configuration/settings.md) — settings.json の全スキーマリファレンス
- [API リファレンス](api/index.md) — すべてのライブラリクレートの公開APIドキュメント

## コアエンジン

| ドキュメント | トピック |
|-------------|---------|
| [ストリーミングエンジン](core/streaming.md) | アクターベースアーキテクチャ, `EneHandle`, `EneEvent`, ツール呼び出しループ |
| [ストリーミングイベント](core/streaming-events.md) | mind ストリーミングパスが発行する `EneEvent` バリアント |
| [プロンプト構築](core/prompt.md) | メッセージ構築順序, システムプロンプト, 感情プロトコル, 関数呼び出し |
| [セッション管理](core/session.md) | `ConversationSession`, `CharacterCardV3`, CBS 式展開 |
| [セッション分割](core/session-split.md) | タイムアウト, 話題変化検出, 手動分割, 非同期ライフサイクル |
| [感情トークン](core/emotions.md) | `<\|emo:name\|>` 解析, VRM ブレンドシェイプマッピング |

## 記憶

- [長期記憶](memory/memory.md) — `MemoryStore`, 埋め込み, ベクトル検索, 要約

## ツール

- [ツールシステム概要](tools/overview.md) — IPC アーキテクチャ, `ToolHostManager`, Tool RAG
- [ファイルシステムツール](tools/fs.md) — `read`, `write`, `edit`, `glob`, `grep`, `patch`, `shell`, `undo`
- [Web ツール](tools/web.md) — `webfetch`, `websearch`
- [ユーティリティツール](tools/utility.md) — `question`, `todo`, `get_current_time`, `get_system_info`
- [GUI 自動化](tools/app.md) — `app` メガツール (15 アクション)
- [ブラウザ自動化](tools/browser.md) — `browser` メガツール (8 アクション, CDP)
- [セキュリティサンドボックス](tools/sandbox.md) — パス制限, ブロックコマンド, Undo システム
- [Tool RAG](tools/tool-rag.md) — 埋め込みベースのツール選択, HyDE, リランキング
- [SDK ガイド](tools/sdk.md) — `ene-tool-proto` によるサードパーティツール開発
- [Derive Macro](tools/derive-macro.md) — `#[derive(ToolSpec)]` 属性リファレンス

## アプリケーション

- [CLI リファレンス](applications/cli.md) — REPL コマンド, フラグ, キーボードショートカット
- [デスクトップアプリ](applications/desktop.md) — winit+wgpu+egui シェル, VRM パイプライン, オーバーレイ動作

## クレート一覧

| クレート | 種別 | 説明 |
|---------|------|------|
| `ene-config` | Library | 設定管理, スキーマ生成, キャラクターカード, マクロ |
| `ene-runtime` | Library | API v2 ホスト: 準備済み `EneHandle::open`, `TurnId`, ストリーミング, ツール, 記憶統合 |
| `ene-mind` | Library | 認知ランタイム — セッション, Identity Kernel, 型付きメモリ, 感情, Performance 調停, コミットメント |
| `ene-ai` | Library | LLM + 埋め込みプロバイダ (API + ローカル GGUF) |
| `ene-store` | Library | SQLite-vec 記憶ストア (`store.enabled` / `store.db_path`) |
| `ene-tool` | Library | ツール ABI ファサード (proto + common + derive を再エクスポート) |
| `ene-tool-proto` | Library | IPC プロトコル, `ToolProvider` トレイト, `ToolSpec`, `ToolError` |
| `ene-tool-derive` | Proc-macro | `#[derive(ToolSpec)]` によるツールスペック自動生成 |
| `ene-tool-host` | Library | ツールプロセス管理, MCP 対応, Tool RAG |
| `ene-tool-db` | Library | ツール別 DB IPC クライアント (ツールバイナリが使用) |
| `ene-tool-common` | Library | 共通ユーティリティ (`ToolAction` トレイト, HTML 抽出) |
| `ene-common` | Library | 共有ユーティリティ (`Truncate`) |
| `ene-vrm` | Library | VRM 1.0 モデルローダーと MToon レンダラー (mind/runtime 依存なし) |
| `ene-tool-utility` | Binary | ユーティリティツール (question, todo, 時刻, システム情報) |
| `ene-tool-fs` | Binary | ファイルシステムツール (read, write, edit, shell, undo) |
| `ene-tool-web` | Binary | Web ツール (fetch, search) |
| `ene-tool-app` | Binary | GUI 自動化 (キーボード, マウス, スクリーンショット) |
| `ene-tool-browser` | Binary | ブラウザ自動化 (Chromium CDP) |
| `ene-cli` | Binary | 対話型 CLI REPL |
| `ene-desktop` | Binary | winit + wgpu + egui デスクトップシェル (VRM レンダリング) |
