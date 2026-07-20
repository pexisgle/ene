# `Commitment` / 約束・タスク台帳仕様

`Commitment` は、会話中に発生したマスコットの約束、予定、ユーザーへのタスク（未完了コミットメント）を追跡・永続化するモジュールです。回収処理（Recall）に組み込まれ、「約束の履行忘れ」を防止します。

---

## 1. データ構造

### `CommitmentStatus` (公開 / 列挙型)
約束の現在の状態を表します。
*   `Active`: 未完了かつ有効。プロンプトや検索で常に参照されます。
*   `Done`: 履行完了。
*   `Cancelled`: キャンセルまたは撤回。
*   `Stale`: 期限切れ、または別の約束に上書きされ不要になった状態。

### `Commitment` (公開 / 構造体)
データベース内の約束レコード。
```rust
pub struct Commitment {
    pub id: Option<i64>,
    pub character_id: String,
    pub user_id: String,
    pub title: String,
    pub description: String,
    pub status: CommitmentStatus,
    pub due_at: Option<DateTime<Utc>>,
    pub due_label: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}
```
*   `due_at`: システムが評価可能な期限日時。
*   `due_label`: 抽出時にLLMが検出した生の文字列（例: 「明日」「来週の月曜」）。

---

## 2. データベース操作

`MemoryStore`（`store/mod.rs`）は、約束を管理するための以下の低レベルCRUDメソッドを提供します。

### `insert_commitment`
*   **シグネチャ**: `pub async fn insert_commitment(&self, row: &NewCommitment) -> Result<i64, MemoryError>`
*   **解説**: 新しい約束をデータベースに追加し、生成された主キーIDを返却します。

### `list_active_commitments`
*   **シグネチャ**: `pub async fn list_active_commitments(&self, character_id: &str, user_id: Option<&str>, limit: usize) -> Result<Vec<Commitment>, MemoryError>`
*   **解説**: 特定のキャラクターとユーザーに紐づく、ステータスが `Active` な約束レコードを指定件数（`limit`）までロードします。

### `update_commitment_status`
*   **シグネチャ**: `pub async fn update_commitment_status(&self, id: i64, status: CommitmentStatus, fail_reason: Option<&str>) -> Result<bool, MemoryError>`
*   **解説**: IDで指定された約束のステータスを更新します。ステータスが `Done` に変更された場合、同期的現在時刻を `completed_at` に記録します。

---

## 3. プロンプトインジェクション用データ (`ActiveCommitmentPrompt`)

ベクトル類似度検索（Vector Recall）の結果に左右されず、未完了タスクをプロンプトの固定セクションに確実に差し込むため、軽量のテキスト表現である `ActiveCommitmentPrompt` を使用します。

*   **構造**:
    -   `id`: コミットメントID。
    -   `title`: 短い見出し。
    -   `description`: 約束の内容説明。
    -   `due_label`: 期限表現。
*   **変換処理**:
    `CommitmentLedger::active_prompt_candidates(rows: &[Commitment]) -> Vec<ActiveCommitmentPrompt>` を経由し、データベース行オブジェクトからプロンプトに適した最小限の文字列表現へ変換します。
