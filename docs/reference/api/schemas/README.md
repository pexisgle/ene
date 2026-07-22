# API schemas (v1)

Hand-authored JSON Schema documents for the stable public runtime boundary (#189).
These are **not** generated artifacts; edit them when the public wire contract changes.

| Schema | Path |
|---|---|
| Chat events (`PublicChatEvent`) | [`public-chat-event.v1.json`](./public-chat-event.v1.json) |
| Diagnostics | [`diagnostic-event.v1.json`](./diagnostic-event.v1.json) |

Rust source of truth: `ene_runtime::public_api` (`API_VERSION = "1"`).
See [API v1 ADR](../../architecture/api-v1.md) for versioning rules.
