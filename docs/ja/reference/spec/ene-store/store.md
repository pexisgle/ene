# `MemoryStore` / SQLite接続とデータベース操作仕様

`MemoryStore` 構造体は、SQLite (sqlite-vec) コネクションを保持し、セッション履歴、型付き長期記憶、および埋め込みベクトルの CRUD 操作を提供するデータベースゲートウェイです。

---

## 1. 構造体定義とインスタンス化

### `MemoryStore` (公開 / 構造体)
```rust
#[derive(Clone)]
pub struct MemoryStore {
    db: DatabaseConnection,
    embedding_dim: usize,
}
```
*   `db`: SeaORM のデータベース接続プールオブジェクト。
*   `embedding_dim`: 埋め込みモデルの出力次元数（例: `text-embedding-3-small` の場合は `1536`）。

### コンストラクタ

#### `open`
*   **シグネチャ**: `pub async fn open(db_path: &Path, embedding_dim: usize) -> Result<Self, MemoryError>`
*   **挙動**:
    1.  `init_sqlite_vec()` を呼び出し、拡張モジュールを初期化。
    2.  `sqlite:PATH` 接続文字列を生成。
    3.  WALモード、NORMAL同期、5秒ビジータイムアウトオプションを適用し、`Database::connect` を実行。
    4.  `Migrator::up(&db, None)` を実行し、全テーブル定義を作成・更新。

#### `open_in_memory`
*   **シグネチャ**: `pub async fn open_in_memory(embedding_dim: usize) -> Result<Self, MemoryError>`
*   **挙動**: 接続文字列 `sqlite::memory:` を用いて、メモリ上に閉じた一時的なデータベースを作成します（テスト用）。

---

## 2. ログおよびシーンサマリーの操作

### `insert_conversation_log`
*   **シグネチャ**: `pub async fn insert_conversation_log(&self, session_id: &str, role: Role, content: &str) -> Result<i64, MemoryError>`
*   **解説**: `conversation_logs` テーブルに対話行を追加します。

### `insert_memory_span`
*   **シグネチャ**: `pub async fn insert_memory_span(&self, span: &NewMemorySpan) -> Result<i64, MemoryError>`
*   **解説**: 会話セッション分割時に、要約されたスパン範囲と要約テキストを `memory_spans` テーブルに保存します。

### `get_active_scene_summary`
*   **シグネチャ**: `pub async fn get_active_scene_summary(&self, session_id: &str) -> Result<Option<ActiveSceneSummaryRow>, MemoryError>`
*   **解説**: 最も新しい圧縮レベル（通常はレベル0 = Scene）のシーン要約を抽出し、プロンプトインジェクション用に返却します。

---

## 3. ベクトルバリデーション

データベースにベクトルを挿入する前に、`validate_embedding` にて厳格な検証を行います。
```rust
fn validate_embedding(embedding: &[f32], expected_dim: usize) -> Result<(), MemoryError>
```
*   **次元数チェック**: 挿入ベクトルの長さが `embedding_dim` と完全に一致するか。不一致の場合は `InvalidEmbedding` エラー（類似度空間の歪みを防止するため）。
*   **非有限値チェック**: ベクトル内に `NaN` または `Infinity` が含まれていないか。sqlite-vec 拡張は NaN を含むベクトルとの距離計算時にクエリ全体が NaN 毒汚染される性質があるため、ここで遮断します。

---

## 4. 忘却バッチ更新

### `apply_natural_decay_batch`
*   **シグネチャ**:
    ```rust
    pub async fn apply_natural_decay_batch(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        now: DateTime<Utc>,
        decay_half_life_days: f64,
        limit: usize,
    ) -> Result<NaturalDecayReport, MemoryError>
    ```
*   **解説**:
    1.  指定された半減期（`decay_half_life_days`）に基づき、最後のアクセス日時から現在時刻 `now` までの経過時間から、記憶の現想起スコアを SQL 内で一括計算します。
    2.  想起スコアが `0.3` 未満に減衰した `Active` 記憶のステータスを `Faded` に更新。
    3.  想起スコアが `0.1` 未満に減衰した `Faded` 記憶のステータスを `Archived` に更新。
    4.  1回のバジェト（`limit` = 通常256）内で更新されたレコード件数を `NaturalDecayReport` として返却します。
