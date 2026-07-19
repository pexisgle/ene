# リファレンス

AI エージェントと深いコントリビューション向けの設計契約、クレート API、実装詳細です。

[← ドキュメントホーム](../index.md) · [開発者ガイド](../guide/index.md) · [English](../../reference/index.md)

## 最初に読む（エージェント）

1. [API v1](architecture/api-v1.md) — ロック済みホスト契約（`EneHandle::open`、`TurnId`、Busy、イベント）
2. [アーキテクチャ概要](architecture/overview.md) — クレートマップとターン流れ
3. [API インデックス](api/index.md) — 各ライブラリクレートの公開面
4. 以降は必要に応じて下のトピックへ

## アーキテクチャ

| ドキュメント | トピック |
|--------------|----------|
| [概要](architecture/overview.md) | クレートマップ、ターン流れ |
| [API v1](architecture/api-v1.md) | ホスト契約 ADR |
| [起動](architecture/startup.md) | Desktop / CLI ブート |
| [認知ランタイム](architecture/cognitive-runtime.md) | mind / 記憶 / 感情 ADR |
| [Avatar Performance](architecture/avatar-performance.md) | `PerformanceCue` ADR |

## API

- [クレート API インデックス](api/index.md) — 依存グラフとクレート別ページ

## ランタイム

| ドキュメント | トピック |
|--------------|----------|
| [ストリーミング](runtime/streaming.md) | アクター、`EneHandle`、ツールループ |
| [ストリーミングイベント](runtime/streaming-events.md) | `EneEvent` バリアント |
| [プロンプト](runtime/prompt.md) | `PromptPacket` 組み立て |
| [セッション](runtime/session.md) | セッションとキャラクターカード |
| [セッション分割](runtime/session-split.md) | 分割 / 圧縮ライフサイクル |
| [感情](runtime/emotions.md) | Affect と Performance マーカー |

## 記憶・設定・ツール

- [長期記憶](memory/memory.md)
- [設定スキーマ](configuration/settings.md)
- [ツール IPC / ホスト](tools/overview.md) · [Tool RAG](tools/tool-rag.md) · [SDK](tools/sdk.md) · [Derive](tools/derive-macro.md)

人間向けツールカタログ: [ガイド → ツール](../guide/tools/overview.md)。
