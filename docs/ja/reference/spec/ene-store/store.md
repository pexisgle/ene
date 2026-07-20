# `MemoryStore` / SQLite 接続および DB 操作仕様

`MemoryStore` 構造体は、SQLite (sqlite-vec) の接続プールを保持し、チャット対話履歴ログ、長期の構造化メモリレコード、および埋め込みベクトルの CRUD トランザクション処理を管理します。

---

## 1. 構造体の定義とインスタンス化

### `MemoryStore` (パブリック / 構造体)
```rust
#[derive(Clone)]
pub struct MemoryStore {
    db: DatabaseConnection,
    embedding_dim: usize,
}
```

#### `init_sqlite_vec`
*   **シグネチャ**: `pub fn init_sqlite_vec()`
*   **説明**: `sqlite-vec` ベクトル検索用のバイナリ拡張モジュールを SQLite のオートエクステンションフック（`sqlite3_auto_extension`）にグローバル登録します。重複登録によるエラーを防ぐため `std::sync::Once` で保護されています。

#### `open`
*   **シグネチャ**: `pub async fn open(path: &Path, embedding_dim: usize) -> Result<Self, MemoryError>`
*   **プロセス**:
    1.  `init_sqlite_vec` を実行して、sqlite-vec 拡張機能のロード状況を確認します。
    2.  指定された UDS パスまたはファイルパスを使用して `sqlite://` スキーム接続オプションを作成します。
    3.  WAL（Write-Ahead Logging）ジャーナルモード、同期モード NORMAL、およびビジータイムアウト（5000ms）を適用します。
    4.  データベースマイグレーション（`Migrator::up(&db, None)`）を実行してスキーマを最新化します。
    5.  接続をバインドした `MemoryStore` のインスタンスを返します。

#### `open_in_memory`
*   **シグネチャ**: `pub async fn open_in_memory(embedding_dim: usize) -> Result<Self, MemoryError>`
*   **説明**: テスト環境用に、オンメモリ SQLite 接続（`sqlite::memory:`）を開いて初期化します。

#### `connection`
*   **シグネチャ**: `pub const fn connection(&self) -> &DatabaseConnection`
*   **説明**: SeaORM の基礎となるデータベース接続プールハンドルを返します。

#### `embedding_dim`
*   **シグネチャ**: `pub const fn embedding_dim(&self) -> usize`
*   **説明**: 使用するように構成されているテキスト埋め込みモデルの次元数設定を返します。

#### `apply_pragmas`
*   **シグネチャ**: `async fn apply_pragmas(db: &DatabaseConnection) -> Result<(), MemoryError>`
*   **説明**: WAL ジャーナルモードの有効化、同期モードの設定、ビジータイムアウト閾値の指定、および外部キー制約チェックの有効化を実行します。

---

## 2. 対話メッセージテキストログ

#### `insert_log`
*   **シグネチャ**: `pub async fn insert_log(&self, session_id: &str, card_name: &str, role: &str, content: &str) -> Result<i64, MemoryError>`
*   **説明**: 生の会話発話行レコードを `conversation_logs` テーブルに挿入します。

#### `insert_conversation_turn`
*   **シグネチャ**: `pub async fn insert_conversation_turn(&self, session_id: &str, card_name: &str, user_message: &str, assistant_response: &str) -> Result<(i64, i64), MemoryError>`
*   **説明**: ユーザー発言とアクターの完了応答メッセージを一括トランザクションでデータベースに保存し、それぞれの主キー ID をタプルで返します。

#### `spawn_insert_log`
*   **シグネチャ**: `pub fn spawn_insert_log(store: &Arc<Self>, session_id: &str, card_name: &str, role: &str, content: &str)`
*   **説明**: メインループスレッドのブロッキングを防ぐため、会話ログの書き込み操作を非同期でバックグラウンドに投入（Spawn）して実行します。

#### `get_logs_by_session`
*   **シグネチャ**: `pub async fn get_logs_by_session(&self, session_id: &str) -> Result<Vec<(String, String, DateTime<Utc>)>, MemoryError>`
*   **説明**: 特定のセッション UUID に対応する全メッセージ履歴ログ（発話役割、コンテンツテキスト、タイムスタンプ）を時系列で取得します。

---

## 3. ツールスペックインデックス操作 (ツール RAG)

#### `upsert_tool_embedding_field`
*   **シグネチャ**: `pub async fn upsert_tool_embedding_field(&self, tool_name: &str, field: &str, field_key: &str, version_hash: &str, model_name: &str, embedding: &[f32], source_text: &str) -> Result<(), MemoryError>`
*   **説明**: ツールの機能定義テキストの埋め込みベクトル情報を、データベースの `tool_embedding_index` テーブルに新規書き込み、または更新（Upsert）します。

#### `list_tool_embedding_fields`
*   **シグネチャ**: `pub async fn list_tool_embedding_fields(&self) -> Result<Vec<ToolEmbeddingFieldRow>, MemoryError>`
*   **説明**: インデックスされているすべてのツールの定義情報フィールドの一覧を取得します。

#### `list_tool_embedding_hashes`
*   **シグネチャ**: `pub async fn list_tool_embedding_hashes(&self) -> Result<Vec<(String, String, String, String, String)>, MemoryError>`
*   **説明**: インデックスキャッシュとのバージョン照合用に、現在登録されているツール定義のハッシュ、キー、およびモデル情報の配列を返します。

#### `delete_tool_embeddings`
*   **シグネチャ**: `pub async fn delete_tool_embeddings(&self, tool_name: &str) -> Result<usize, MemoryError>`
*   **説明**: 特定のツールに紐づくすべてのインデックスベクトルおよびメタデータ情報を一括削除します。

#### `search_tools`
*   **シグネチャ**: `pub async fn search_tools(&self, query_embedding: &[f32], limit: usize, similarity_threshold: f32) -> Result<Vec<(String, f32)>, MemoryError>`
*   **説明**: ユーザー入力のクエリベクトルに基づいて、`tool_embedding_index` に対し sqlite-vec を用いたコサイン類似度検索を実行し、スコアが `similarity_threshold` を満たす関連ツールの一覧を類似度が高い順にソートして返します。

---

## 4. 感情座標データおよび提案情報

#### `get_affect_state`
*   **シグネチャ**: `pub async fn get_affect_state(&self, character_id: &str) -> Result<crate::AffectState, MemoryError>`
*   **説明**: 該当アクターの感情 PAD パラメータ情報をデータベースから取得します。レコードが存在しない場合はデフォルトの中立感情（0.0）を返します。

#### `upsert_affect_state`
*   **シグネチャ**: `pub async fn upsert_affect_state(&self, state: &crate::AffectState) -> Result<(), MemoryError>`
*   **説明**: 更新されたアクターの感情 PAD 座標値と気分 Mood ラベルを SQLite にコミットします。

#### `upsert_pending_affect_proposal`
*   **シグネチャ**: `pub async fn upsert_pending_affect_proposal(&self, proposal: &crate::PendingAffectProposal) -> Result<(), MemoryError>`
*   **説明**: ターンの終了後にバックグラウンドで分析された新しい感情提案データ（`PendingAffectProposal`）を保留中のテーブルに登録します。

#### `get_pending_affect_proposal`
*   **シグネチャ**: `pub async fn get_pending_affect_proposal(&self, character_id: &str, user_id: &str) -> Result<Option<crate::PendingAffectProposal>, MemoryError>`
*   **説明**: 次のチャットターン開始時に取り込んで適用するために、保留中の感情提案レコードを取得します。

#### `delete_pending_affect_proposal`
*   **シグネチャ**: `pub async fn delete_pending_affect_proposal(&self, character_id: &str, user_id: &str) -> Result<(), MemoryError>`
*   **説明**: 保留中の感情提案レコードを削除します。

#### `take_pending_affect_proposal`
*   **シグネチャ**: `pub async fn take_pending_affect_proposal(&self, character_id: &str, user_id: &str) -> Result<Option<crate::PendingAffectProposal>, MemoryError>`
*   **説明**: 保留中の感情提案データを取得し、同時にデータベースから削除するトランザクション処理を実行します。

---

## 5. 長期 typed_memory に対する CRUD 操作

#### `insert_typed_memory`
*   **シグネチャ**: `pub async fn insert_typed_memory(&self, item: &crate::NewMemoryItem) -> Result<i64, MemoryError>`
*   **説明**: 新しい長期記憶テキストを `typed_memories` に挿入し、同時にその埋め込みベクトル情報を `memory_embeddings` に保存するトランザクションを実行して、自動生成されたレコード ID を返します。

#### `get_typed_memory`
*   **シグネチャ**: `pub async fn get_typed_memory(&self, id: i64) -> Result<Option<crate::MemoryItem>, MemoryError>`
*   **説明**: 指定されたメモリ ID に一致する長期記憶項目を取得します。

#### `get_typed_memories_by_character`
*   **シグネチャ**: `pub async fn get_typed_memories_by_character(&self, character_id: &str, kind: Option<crate::MemoryKind>, limit: usize, offset: usize) -> Result<Vec<crate::MemoryItem>, MemoryError>`
*   **説明**: キャラクターに紐づく長期記憶レコード一覧を取得します。特定のメモリ分類（MemoryKind）でのフィルタリングに対応しています。

#### `count_typed_memories`
*   **シグネチャ**: `pub async fn count_typed_memories(&self, character_id: &str, kind: Option<crate::MemoryKind>) -> Result<i64, MemoryError>`
*   **説明**: 指定された条件に合致する長期記憶レコードの総件数をカウントします。

#### `list_typed_memories_by_source_prefix`
*   **シグネチャ**: `pub async fn list_typed_memories_by_source_prefix(&self, character_id: &str, prefix: &str, limit: usize) -> Result<Vec<crate::MemoryItem>, MemoryError>`
*   **説明**: 参照元の名前空間接頭辞（Prefix）が一致するメモリレコード一覧を取得します。

#### `typed_memory_exists_by_source_ref`
*   **シグネチャ**: `pub async fn typed_memory_exists_by_source_ref(&self, character_id: &str, source_ref: &str) -> Result<bool, MemoryError>`
*   **説明**: 同一の参照元 ID を持つメモリレコードが既に存在しているかチェックします。

#### `get_active_typed_memory_by_source_ref`
*   **シグネチャ**: `pub async fn get_active_typed_memory_by_source_ref(&self, character_id: &str, source_ref: &str) -> Result<Option<crate::MemoryItem>, MemoryError>`
*   **説明**: 指定された参照元に対応する、現在アクティブ（Active）状態のメモリレコードを取得します。

#### `archive_typed_memories_by_source_prefixes`
*   **シグネチャ**: `pub async fn archive_typed_memories_by_source_prefixes(&self, character_id: &str, prefixes: &[&str], keep_refs: &std::collections::HashSet<String>) -> Result<usize, MemoryError>`
*   **説明**: 指定された接頭辞に合致し、かつ `keep_refs` 保護セットに含まれていない古いメモリレコードを一括で `Archived` 状態に変更します。

#### `supersede_typed_memory`
*   **シグネチャ**: `pub async fn supersede_typed_memory(&self, new_item: &crate::NewMemoryItem, superseded_id: i64) -> Result<i64, MemoryError>`
*   **説明**: 新しい事実記憶データを挿入し、上書きされた古いメモリ（`superseded_id`）のステータスを `Superseded` に更新するトランザクションを実行します。

#### `update_typed_memory_status`
*   **シグネチャ**: `pub async fn update_typed_memory_status(&self, id: i64, new_status: crate::MemoryStatus) -> Result<bool, MemoryError>`
*   **説明**: 指定したメモリのステータス値を直接上書き更新します。

#### `bump_typed_memory_access`
*   **シグネチャ**: `pub async fn bump_typed_memory_access(&self, id: i64) -> Result<bool, MemoryError>`
*   **説明**: 想起されたメモリのアクセス件数カウンタ（`access_count`）を加算し、最終アクセス日時タイムスタンプを更新します。

#### `transition_typed_memory_status`
*   **シグネチャ**: `pub async fn transition_typed_memory_status(&self, id: i64, new_status: crate::MemoryStatus) -> Result<bool, MemoryError>`
*   **説明**: メモリ状態遷移マシンのルール確認を行った上で、安全にステータスを変更します。

#### `user_restore_typed_memory`
*   **シグネチャ**: `pub async fn user_restore_typed_memory(&self, id: i64) -> Result<bool, MemoryError>`
*   **説明**: アーカイブや減衰、または論争中の記憶データを、手動指示により `Active` 状態に復元します。

#### `user_forget_typed_memory`
*   **シグネチャ**: `pub async fn user_forget_typed_memory(&self, id: i64) -> Result<bool, MemoryError>`
*   **説明**: ユーザーからの削除要求指示に基づき、メモリレコードを `UserDeleted` 状態に変更し、想起対象から除外します。

#### `apply_natural_decay_batch`
*   **シグネチャ**: `pub async fn apply_natural_decay_batch(&self, character_id: &str, user_id: Option<&str>, now: DateTime<Utc>, decay_half_life_days: f64, limit: usize) -> Result<NaturalDecayReport, MemoryError>`
*   **説明**: 長期記憶の自然減衰バッチトランザクションを実行します：
    1.  メモリが最後にアクセスされてからの経過時間と、設定された減衰の半減期から記憶の維持スコアを算出します。
    2.  維持スコアが `0.3` を下回った `Active` なメモリ行を `Faded` 状態に変更します。
    3.  維持スコアが `0.1` を下回った `Faded` なメモリ行を `Archived` 状態に変更します。
    4.  処理された更新件数データをレポートとしてまとめます。

---

## 6. 想起用ハイブリッド検索処理の実行

#### `search`
*   **シグネチャ**: `pub async fn search(&self, query: &crate::Query<'_>) -> Result<Vec<crate::ScoredMemory>, MemoryError>`
*   **プロセス**:
    1.  UDS ソケットから受け取ったクエリベクトルに基づき、`memory_embeddings` に対してコサイン類似度によるベクトル検索を行い、適合度の高いレコードを抽出します。
    2.  クエリのキーワードトークン配列に基づき、`typed_memories` に対して部分一致等の語彙検索を実行します。
    3.  抽出された全候補を統合し、アクセス回数ボーナス、感情の一致度、および時間的リセンシー減衰の数式を適用して総合評価スコア（`score_candidate`）を算出します。
    4.  スコアが高い順にソートされた `ScoredMemory` 配列を返します。

#### `search_typed_memories`
*   **Signature**: `pub(crate) async fn search_typed_memories(&self, query_embedding: &[f32], character_id: &str, model_name: &str, limit: usize, similarity_threshold: f32) -> Result<Vec<(crate::MemoryItem, f32)>, MemoryError>`
*   **Description**: 指定されたベクトルとの類似検索を実行してレコードを抽出します。

#### `search_typed_memories_vector`
*   **Signature**: `async fn search_typed_memories_vector(&self, query_embedding: &[f32], character_id: &str, model_name: &str, user_id: Option<&str>, statuses: &[&str], limit: usize, similarity_threshold: f32) -> Result<Vec<(crate::MemoryItem, f32)>, MemoryError>`
*   **Description**: sqlite-vec 拡張機能の `vec_distance_cosine` を用いて、DB 内のベクトル距離クエリを低レイテンシで処理します。

#### `list_recallable_typed_memories`
*   **Signature**: `pub async fn list_recallable_typed_memories(&self, character_id: &str, user_id: Option<&str>, limit: usize) -> Result<Vec<crate::MemoryItem>, MemoryError>`
*   **Description**: 現在アクティブ（または想起対象として許容されるステータス）のレコード一覧を取得します。

#### `get_typed_memories_by_commitment_ids`
*   **Signature**: `async fn get_typed_memories_by_commitment_ids(&self, commitment_ids: &[i64]) -> Result<Vec<crate::MemoryItem>, MemoryError>`
*   **Description**: 特定の約束（Commitment）に関連づけられている記憶データを参照します。

#### `list_lexical_typed_memory_candidates`
*   **Signature**: `async fn list_lexical_typed_memory_candidates(&self, query_text: &str, character_id: &str, user_id: Option<&str>, limit: usize) -> Result<Vec<crate::MemoryItem>, MemoryError>`
*   **Description**: タイトルおよび内容カラムに対して、部分一致による高速語彙マッチ検索を実行します。

---

## 7. ベクトル検証および型変換ユーティリティ

#### `validate_embedding`
*   **Signature**: `fn validate_embedding(embedding: &[f32], expected_dim: usize) -> Result<(), MemoryError>`
*   **Description**: 計算されたベクトル次元数がシステム定義値と一致し、値が有限数であるか整合性を検証します。

#### `decode_embedding_bytes`
*   **Signature**: `pub fn decode_embedding_bytes(&self, bytes: &[u8]) -> Vec<f32>`
*   **Description**: SQLite からロードされたバイナリ Blob データを f32 ベクトル配列へとデコードします。

#### `embedding_to_bytes` / `bytes_to_embedding`
*   **Description**: ベクトル配列と SQLite 用バイナリデータの相互変換を行います。

#### `cosine_similarity_expr` / `cosine_similarity_filter`
*   **Description**: sqlite-vec 向けの SeaORM コサイン類似度計算用式およびフィルタ表現式を構築して適用します。
