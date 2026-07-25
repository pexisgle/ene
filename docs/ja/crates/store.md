# `ene-store` — API リファレンス

> **クレート**: `ene-store` | **役割**: データベース & ベクトル永続化層 (SQLite + SeaORM + sqlite-vec)

`ene-store` は、データベース接続、SeaORM エンティティ、SQLite スキーママイグレーション、および `sqlite-vec` によるベクトル類似度検索の唯一の所有者です。

---

## アーキテクチャ境界保証
`ene-store` は `ene-ai` や `ene-mind` に**一切依存・インポートしません**。

---

## 主要構造体と API

### `MemoryStore`
型付きメモリ、セッション対話履歴、およびコミットメントを管理する中心的な永続化インターフェース：

```rust
pub struct MemoryStore { /* ... */ }

impl MemoryStore {
    /// 対象パスの SQLite メモリデータベースを開くか、新規作成します。
    pub async fn open(db_path: impl AsRef<Path>) -> Result<Self, EneMemoryError>;

    /// 抽出された新しい型付きメモリファクトを保存します。
    pub async fn save_memory(&self, memory: &NewMemory) -> Result<MemoryId, EneMemoryError>;

    /// 多要因ハイブリッド想起検索 (ベクトル + 字句 + 新近性 + 顕著性) を実行します。
    pub async fn hybrid_recall(&self, query: &MemoryQuery) -> Result<Vec<ScoredMemory>, EneMemoryError>;

    /// アクティブなセッション履歴に対話ターンを追加します。
    pub async fn append_session_turn(&self, session_id: SessionId, turn: &TurnRecord) -> Result<(), EneMemoryError>;

    /// コミットメント台帳内のアクティブなコミットメント状態を更新します。
    pub async fn update_commitment(&self, id: CommitmentId, status: CommitmentStatus) -> Result<(), EneMemoryError>;
}
```

---

## 関連ドキュメント
- [記憶システムとハイブリッド想起](../concepts/memory-system.md)
- [システムアーキテクチャ](../architecture.md)
