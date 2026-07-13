# セッション API — `ene-mind` に吸収済み

> **状態:** 独立したセッションクレートは削除済み。
> セッション型は [`ene-mind`](ene-mind.md)（`ene_mind::session` およびクレートルート再エクスポート）にあります。

セッション API は `ene_mind` からインポートしてください:

```rust
use ene_mind::{ConversationSession, SessionConfig, SessionId, SplitReason};
```

詳細は mind API ドキュメントと [API v2](../architecture/api-v2.md) を参照してください。
