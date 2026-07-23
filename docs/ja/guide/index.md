# 開発者ガイド

ene の実行方法と拡張方法を説明する開発者向けドキュメントです。

[← ドキュメントホーム](../index.md) · [リファレンス（契約と API）](../reference/index.md) · [English](../../guide/index.md)

## 推奨の読み順

1. [はじめに](getting-started.md) — ビルド、実行、最初のプロバイダ設定
2. [システム概要](system-overview.md) — クレートと 1 ターン
3. [設定](configure.md) — よく触る項目
4. 概念: [ターン](concepts/turns.md) · [セッション](concepts/sessions.md) · [記憶](concepts/memory.md) · [感情](concepts/emotions.md)
5. アプリ: [CLI](apps/cli.md) · [Desktop](apps/desktop.md)
6. [音声パイプライン](voice-pipeline.md) — マイク、STT、TTS、リップシンク
7. [リリース](release.md) — タグ駆動の Linux リリース
8. ツール: [カタログ](tools/overview.md) · [ツールを書く](tools/write-a-tool.md)

## ツール

| ドキュメント | トピック |
|--------------|----------|
| [カタログ](tools/overview.md) | 名前空間と組み込みバイナリ |
| [ファイルシステム](tools/fs.md) | read / write / edit / shell / … |
| [Web](tools/web.md) | fetch / search |
| [ユーティリティ](tools/utility.md) | question、todo、時刻など |
| [GUI 自動化](tools/app.md) | `app` アクション |
| [ブラウザ](tools/browser.md) | CDP ブラウザ操作 |
| [サンドボックス](tools/sandbox.md) | パスとコマンド制限 |
| [ツールを書く](tools/write-a-tool.md) | カスタムバイナリの追加 |

## 契約が必要なとき

ADR、`ene-*` API、ストリーミング内部、設定フルスキーマは [リファレンス](../reference/index.md) へ。
