# API スキーマ (v1)

安定パブリックランタイム境界向けの手編集 JSON Schema です (#189)。
生成物ではないため、ワイヤ契約変更時に直接編集します。

| スキーマ | パス |
|---|---|
| チャットイベント (`PublicChatEvent`) | [`public-chat-event.v1.json`](../../api/schemas/public-chat-event.v1.json) |
| 診断 | [`diagnostic-event.v1.json`](../../api/schemas/diagnostic-event.v1.json) |

Rust の正本: `ene_runtime::public_api`（`API_VERSION = "1"`）。
バージョニング規則は [API v1 ADR](../../architecture/api-v1.md) を参照。
