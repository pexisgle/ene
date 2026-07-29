# Ene ドキュメント

**Ene** は Rust 2024 で実装されたローカル AI キャラクター基盤です。LLM との会話、リッチなツールプラグイン、長期記憶想起、ローカル音声処理、デスクトップ上での VRM アバターアニメーションを提供します。

[English Documentation](../index.md)

---

## ドキュメントの構成

ドキュメントは以下のわかりやすいセクションに整理されています：

| セクション | 対象者 | 概要 |
|---|---|---|
| **[スタートアップ](getting-started.md)** | ユーザー・開発者 | インストール、依存関係、ビルド、CLI / Desktop アプリの起動方法。 |
| **[アーキテクチャ](architecture.md)** | 開発者・アーキテクト | ワークスペース設計、API v1 ホスト契約、ターンパイプライン、IPC Protocol v4。 |
| **[設定リファレンス](configuration.md)** | 運用者・開発者 | 全設定項目 (`ENE_*` 環境変数、設定ファイル、キャラクターカード)。 |
| **[主要概念](concepts/turn-and-session.md)** | 開発者 | ターン、記憶、音声/アバター、プラグイン、MCP連携の解説。 |
| **[クレートリファレンス](crates/runtime.md)** | 開発者 | ワークスペース内全 16 クレートの公開 API と設計。 |
| **[アプリケーション](apps/cli.md)** | エンドユーザー | `ene-cli` および `ene-desktop` の使用方法。 |

---

## ワークスペース構成図

Ene は **16 のライブラリクレート**、**6 つのプラグインバイナリ**、**2 つのホストアプリケーション** からなるモジュール式 Cargo ワークスペースです：

```
Ene ワークスペース
├── ホストアプリ
│   ├── ene-cli            (CLI REPL アプリケーション)
│   └── ene-desktop        (3D VRM アバター・音声機能付き GUI デスクトップアプリ)
├── コアエンジン
│   ├── ene-runtime        (アクターベースのホストファサード & ターン実行エンジン)
│   ├── ene-mind           (認知エンジン: セッション、プロンプト、感情、プロアクティブ、記憶書込)
│   ├── ene-store          (SQLite + SeaORM + sqlite-vec 記憶・ベクトルストア)
│   ├── ene-config         (設定読み込み、キャラクターカード、スキーマ定義)
│   ├── ene-ai             (コア AI プロバイダトレイト、OpenAI、Anthropic アダプタ)
│   ├── ene-ai-local       (llama-cpp-4 によるローカル LLM 推論)
│   ├── ene-voice          (ローカル STT/TTS/VAD 音声パイプライン)
│   ├── ene-connector      (共有コネクタフレームワーク & MCP ブリッジ)
│   ├── ene-util           (純粋ユーティリティ: 切り詰め、HTML→Markdown)
│   └── ene-vrm            (3D VRM 1.0 ローダー & wgpu レンダラー)
├── プラグインシステム
│   ├── ene-plugin-proto   (IPC ワイヤープロトコル v4 定義)
│   ├── ene-plugin         (プラグイン開発 SDK & アダプタファサード)
│   ├── ene-plugin-host    (プラグインプロセス管理 & スーパーバイザ)
│   ├── ene-tool-sdk       (ツールプラグイン開発 SDK: ToolAction、プロバイダ、ヘルパー)
│   ├── ene-plugin-db      (プラグイン用状態保持 DB IPC クライアント)
│   ├── ene-tool-macros    (ツールアクション用 Proc-macro)
│   └── ene-tool-rag       (ツール仕様の検索拡張生成 RAG)
└── プロセス外プラグイン
    ├── plugins/provider/* (プロバイダプラグイン: anthropic)
    └── plugins/tool/*     (ツールプラグイン: app, browser, fs, utility, web)
```

---

## ナビゲーションリンク

- [セットアップと起動](getting-started.md)
- [システムアーキテクチャと設計](architecture.md)
- [設定リファレンス](configuration.md)
- [ターンとセッション](concepts/turn-and-session.md)
- [記憶と想起](concepts/memory-system.md)
- [音声とアバター](concepts/voice-and-avatar.md)
- [プラグインと MCP システム](concepts/plugins-and-mcp.md)
- [CLI 使用ガイド](apps/cli.md)
- [Desktop 使用ガイド](apps/desktop.md)
