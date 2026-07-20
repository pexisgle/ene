# `Commitment` / アクティブコミットメント台帳仕様

`Commitment`（約束）モジュールは、会話中にアクターがユーザーに対して行った約束、スケジュールされたタスク、およびフォローアップ事項を記録、永続化、および監視します。これらはプロンプトパッキング時に優先的にインジェクトされ、アクターが義務を失念するのを防止します。

---

## 1. データ構造

### `CommitmentStatus` (パブリック / 列挙型)
約束のステータス情報：
*   `Active`: 進行中の約束。プロンプトセクションおよび検索候補の対象となります。
*   `Done`: 解決・履行された状態。
*   `Cancelled`: 取り消しまたはキャンセルされた状態。
*   `Stale`: 期限が切れたか、または古い状態。

### `Commitment` (パブリック / 構造体)
SQLite に保存される約束の永続レコード構造：
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

---

## 2. 解析および型変換処理 (`commitment.rs`)

#### `as_str`
*   **シグネチャ**: `pub const fn as_str(self) -> &'static str`
*   **説明**: 列挙型ステータスを SQLite 保存用の文字列データに変換します。

#### `from_db_str`
*   **シグネチャ**: `pub(crate) fn from_db_str(s: &str) -> Self`
*   **説明**: SQLite からロードしたステータス文字列を列挙型データにデシリアライズします。

---

## 3. データベース CRUD 操作

`MemoryStore` 接続プールは、以下の約束に関するデータ操作を提供します：

#### `insert_commitment`
*   **シグネチャ**: `pub async fn insert_commitment(&self, row: &NewCommitment) -> Result<i64, MemoryError>`
*   **説明**: データベースに新しい約束レコードを作成し、そのレコード ID を返します。

#### `list_active_commitments`
*   **シグネチャ**: `pub async fn list_active_commitments(&self, character_id: &str, user_id: Option<&str>, limit: usize) -> Result<Vec<Commitment>, MemoryError>`
*   **説明**: 指定されたキャラクターおよびユーザー ID スコープに合致するアクティブ（Active）な約束レコードを、最大 `limit` 件取得します。

#### `update_commitment_status`
*   **シグネチャ**: `pub async fn update_commitment_status(&self, id: i64, status: CommitmentStatus, fail_reason: Option<&str>) -> Result<bool, MemoryError>`
*   **説明**: 特定の約束レコードのステータスを変更します。状態を `Done`（解決）に変更した場合は、自動的に `completed_at` カラムに現在の日時をスタンプします。

---

## 4. 台帳ビジネスロジック (`CommitmentLedger` in `ene-mind`)

`ene-mind/src/commitments/mod.rs` に存在する `CommitmentLedger` 構造体は、約束に関連する高レベルな統合ビジネスロジックを管理します。

#### `apply_commitment_candidates`
*   **シグネチャ**: `pub async fn apply_commitment_candidates(store: &MemoryStore, ctx: &CommitmentSyncContext<'_>, candidates: &[MemoryCandidate]) -> Result<Vec<i64>, CognitionError>`
*   **説明**: メモリ統合器から送られてきた約束候補レコードを精査し、条件を満たした新しい項目を SQLite に追加します。

#### `arbitrate_apply_and_sync`
*   **シグネチャ**: `pub async fn arbitrate_apply_and_sync(store: &MemoryStore, candidates: &[MemoryCandidate], arbiter_ctx: &ArbiterContext<'_>, sync_ctx: &CommitmentSyncContext<'_>) -> Result<(Vec<AppliedDecision>, Vec<i64>), CognitionError>`
*   **説明**: 通常のメモリ判定の重複仲裁プロセスと、約束テーブルの書き込み同期プロセスを統合して一括処理します。

#### `list_active`
*   **シグネチャ**: `pub async fn list_active(store: &MemoryStore, character_id: &str, user_id: Option<&str>, limit: usize) -> Result<Vec<Commitment>, CognitionError>`
*   **説明**: キャラクターに紐づくアクティブな約束レコードを取得します。

#### `active_prompt_candidates`
*   **シグネチャ**: `pub fn active_prompt_candidates(commitments: &[Commitment]) -> Vec<ActiveCommitmentPrompt>`
*   **説明**: データベースの約束レコード配列を、トークン節約のため簡素化されたプロンプトインジェクト用の軽量モデル配列にマッピング変換します。

#### `complete`
*   **シグネチャ**: `pub async fn complete(store: &MemoryStore, id: i64) -> Result<bool, CognitionError>`
*   **説明**: 約束を `Done`（解決完了）に変更します。

#### `cancel`
*   **シグネチャ**: `pub async fn cancel(store: &MemoryStore, id: i64) -> Result<bool, CognitionError>`
*   **説明**: 約束を `Cancelled`（キャンセル）に変更します。

#### `mark_stale_overdue`
*   **シグネチャ**: `pub async fn mark_stale_overdue(store: &MemoryStore, now: chrono::DateTime<chrono::Utc>) -> Result<usize, CognitionError>`
*   **説明**: 期限日時（`due_at`）が現在時刻（`now`）を過ぎて超過しているアクティブな約束を検索し、一括でステータスを `Stale` に更新します。

#### `cancel_matching_by_title`
*   **シグネチャ**: `pub async fn cancel_matching_by_title(store: &MemoryStore, ctx: &CommitmentSyncContext<'_>, title: &str) -> Result<(), CognitionError>`
*   **説明**: 同じタイトル条件を持つ進行中の約束を検索し、強制的にキャンセル（Cancelled）状態に更新します。

#### `normalize_title`
*   **シグネチャ**: `fn normalize_title(title: &str) -> String`
*   **説明**: 比較マッチングのために、タイトルの文字列を小文字に正規化します。
