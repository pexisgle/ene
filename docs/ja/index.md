# ene ドキュメント

ene は Rust ワークスペースで実装されたローカル AI キャラクタープラットフォームです。
LLM 駆動の会話、アニメーションする VRM キャラクター、ツール拡張エージェント機能、
長期記憶、自動セッション管理を提供します。

## はじめに

- [アーキテクチャ概要](architecture/overview.md) — クレートマップと依存関係グラフ
- [起動フロー](architecture/startup.md) — デスクトップ (Bevy) と CLI のブートシーケンス
- [設定](configuration/settings.md) — settings.json の全スキーマリファレンス

## コアエンジン

| ドキュメント | トピック |
|-------------|---------|
| [ストリーミングエンジン](core/streaming.md) | `run_ai_with_tools()`, `AiStreamEvent`, ツール呼び出しループ |
| [プロンプト構築](core/prompt.md) | メッセージ構築順序, システムプロンプト, 感情プロトコル, 関数呼び出し |
| [セッション管理](core/session.md) | `ConversationSession`, `CharacterCardV3`, CBS 式展開 |
| [セッション分割](core/session-split.md) | タイムアウト, 話題変化検出, 非同期分割ライフサイクル, Max-pooling |
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
- [SDK ガイド](tools/sdk.md) — `ene-tool-proto` によるサードパーティツール開発

## アプリケーション

- [CLI リファレンス](applications/cli.md) — REPL コマンド, フラグ, キーボードショートカット
- [デスクトップアプリ](applications/desktop.md) — Bevy プラグイン, VRM パイプライン, オーバーレイ動作

## クレート一覧

| クレート | 種別 | 説明 |
|---------|------|------|
| `ene-config` | Library | 設定管理, スキーマ生成, キャラクターカード, マクロ |
| `ene-core` | Library | ストリーミングエンジン, ツール統合, 統一ランタイム |
| `ene-embedding` | Library | 埋め込みプロバイダ (API + ローカル GGUF) |
| `ene-memory` | Library | SQLite-vec 記憶ストア |
| `ene-session` | Library | 会話履歴, セッション分割 |
| `ene-tool-proto` | Library | IPC プロトコル, `ToolProvider` トレイト |
| `ene-tool-host` | Library | ツールプロセス管理, MCP 対応 |
| `ene-tools-common` | Library | 共通ユーティリティ (HTML, 切り詰め) |
| `ene-tools-utility` | Binary | ユーティリティツール (question, todo, 時刻, システム情報) |
| `ene-tools-fs` | Binary | ファイルシステムツール (read, write, edit, shell, undo) |
| `ene-tools-web` | Binary | Web ツール (fetch, search) |
| `ene-tools-app` | Binary | GUI 自動化 (キーボード, マウス, スクリーンショット) |
| `ene-tools-browser` | Binary | ブラウザ自動化 (Chromium CDP) |
| `ene-cli` | Binary | 対話型 CLI REPL |
| `ene-desktop` | Binary | Bevy ベースデスクトップ GUI |
