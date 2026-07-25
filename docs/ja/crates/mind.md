# `ene-mind` — API リファレンス

> **クレート**: `ene-mind` | **役割**: 認知ターンエンジン、プロンプト予算制御、感情、記憶想起

`ene-mind` は Ene の純粋な認知コアです。プロンプト構築、セッション管理、アイデンティティ保護、PAD 感情動態、プロアクティブ発話評価、演出調停、およびバックグラウンド記憶書込を所有します。

---

## アーキテクチャ境界保証
`ene-mind` は `ene-runtime`, `ene-plugin-host`, `ene-vrm` に**一切依存・インポートしません**。

---

## 主要モジュールと型

### `SessionManager`
チャットセッション状態、対話履歴文脈、および自動セッション圧縮・分割を管理します：

```rust
pub struct SessionManager { /* ... */ }
```

### `PromptComposer`
明示的なトークン予算配分を備えた `PromptPacket` を構築します：

```rust
pub struct PromptComposer { /* ... */ }

impl PromptComposer {
    pub async fn compose(
        &self,
        input: &str,
        session: &Session,
        recalled: &[ScoredMemory],
    ) -> Result<PromptPacket, CognitionError>;
}
```

### `PadEmotion`
3次元 Pleasure-Arousal-Dominance 空間でキャラクターの感情状態を管理します：

```rust
pub struct PadEmotion {
    pub pleasure: f32,  // [-1.0, 1.0]
    pub arousal: f32,   // [-1.0, 1.0]
    pub dominance: f32, // [-1.0, 1.0]
}

impl PadEmotion {
    pub fn update(&mut self, delta: PadDelta);
    pub fn to_performance_cue(&self) -> PerformanceCue;
}
```

### `ProactiveEngine`
ユーザーの放置時間や文脈を評価し、自主的なプロアクティブ発話をトリガーします：

```rust
pub struct ProactiveEngine { /* ... */ }
```

### `MemoryWriter`
ターントランスクリプトからエピソード/セマンティックファクトを非同期に抽出し、 `ene-store` へ永続化します。

---

## 関連ドキュメント
- [ターンとセッション](../concepts/turn-and-session.md)
- [記憶と想起](../concepts/memory-system.md)
