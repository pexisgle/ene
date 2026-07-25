# `ene-store` — APIリファレンス

> **クレート:** `ene-store`
> **役割:** 長期記憶の永続化ストア — タイプ付きメモリ（エピソード記憶/意味記憶/感情記憶など）、会話ログ、メモリスパン、感情状態、コンパニオンのコミットメント、およびツール埋め込みインデックス。

---

## 概要

`ene-store` はEneの長期記憶サブシステムです。ストレージバックエンドには **SQLite** を使用し、すべてのSQLアクセスには **`sea-orm`**（`sqlx-sqlite` バックエンドの非同期ORM）を、コサイン類似度によるベクトル検索には **`sqlite-vec`** を使用しています。

> **アーキテクチャ上の制約:** このクレートはすべてのデータベースアクセスに `sea-orm` + `sqlite-vec` を使用します。Dieselや生の `rusqlite` は使用しません。ツールバイナリはこのクレートに直接リンクしてはいけません — `DbIpcServer` / `ene-tool-db` のIPCクライアント経由でデータベースにアクセスします。

各キャラクターは、共有データベース内で `character_id`（タイプ付きメモリ / 感情 / コミットメント）および `card_name`（会話ログ）をキーとした独立したネームスペースを持ちます。このクレートは以下を保持します:

| レイヤー | テーブル | 役割 |
|---|---|---|
| **会話ログ** | `conversation_logs` | 追記専用の生ターン履歴 |
| **タイプ付きメモリ** | `typed_memories`, `memory_embeddings` | mind ランタイムの主ストア |
| **感情** | `affect_states` | キャラクターごとのPAD（快-不快/覚醒/支配性）感情状態 |
| **コミットメント** | `commitments` | コンパニオンの約束・フォローアップの台帳 |
| **メモリスパン** | `memory_spans` | 生ログに対するローリングなシーン/チャプター圧縮 |
| **ツールインデックス** | `tool_embedding_index` | Tool RAGパイプライン用のマルチベクトル埋め込み |

`MemoryStore` のほぼすべてのメソッドは `async` です（`sea-orm`/`sqlx` プールからコネクションを取得するため）。**同期**メソッドは `spawn_insert_log`、`connection`、`embedding_dim`、`decode_embedding_bytes` のみです。

---

## 初期化

### `init_sqlite_vec`

```rust
pub fn init_sqlite_vec()
```

`sqlite3_auto_extension` を介して `sqlite-vec` 拡張を**プロセス全体で**登録します。`std::sync::Once` によって、プロセスにつき一度しか実行されないよう保護されています。引数は取らず、何も返しません — APIレベルでは失敗しません（登録の失敗は、後で `vec_distance_cosine` のSQLエラーとして表面化します）。`MemoryStore::open` と `MemoryStore::open_in_memory` の内部で自動的に呼び出されるため、自分で呼び出す必要はありません。

### `MemoryStore::open` / `open_in_memory`

```rust
pub struct MemoryStore { /* opaque */ }

impl MemoryStore {
    pub async fn open(path: &Path, embedding_dim: usize) -> Result<Self, MemoryError>;
    pub async fn open_in_memory(embedding_dim: usize) -> Result<Self, MemoryError>;
}
```

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `open` | `pub async fn open(path: &Path, embedding_dim: usize) -> Result<Self, MemoryError>` | SQLiteデータベースファイルを開く（存在しなければ作成する）。`sqlite-vec` を登録し、保留中の `sea-orm-migration` マイグレーションを実行する。 |
| `open_in_memory` | `pub async fn open_in_memory(embedding_dim: usize) -> Result<Self, MemoryError>` | インメモリSQLiteデータベースを開く。テストや後述の使用例で利用される。 |
| `connection` | `pub fn connection(&self) -> &DatabaseConnection` | 内部の `sea_orm::DatabaseConnection` を返す。生のクエリアクセスが必要な高度な呼び出し元向け。 |
| `embedding_dim` | `pub fn embedding_dim(&self) -> usize` | 設定済みの埋め込みベクトル幅を返す。 |
| `decode_embedding_bytes` | `pub fn decode_embedding_bytes(&self, bytes: &[u8]) -> Vec<f32>` | 生の `BLOB` カラムを `f32` ベクトルにデコードする。型付きストアメソッド以外で取得した行を扱う場合に有用。 |

---

## 会話ログ

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `insert_log` | `async fn insert_log(&self, session_id: &str, card_name: &str, role: &str, content: &str) -> Result<i64, MemoryError>` | ログエントリを1件追記する。 |
| `insert_conversation_turn` | `async fn insert_conversation_turn(&self, session_id: &str, card_name: &str, user_message: &str, assistant_response: &str) -> Result<(i64, i64), MemoryError>` | 便利メソッド: ユーザーログエントリとアシスタントログエントリを1回の呼び出しで挿入する。両方の行IDを返す。 |
| `spawn_insert_log` | `fn spawn_insert_log(store: &Arc<Self>, session_id: &str, card_name: &str, role: &str, content: &str)` | **同期メソッド。** Fire-and-forget: `insert_log` を呼び出す `tokio` タスクを生成する。エラーはログに記録されるが伝播しない。 |
| `get_logs_by_session` | `async fn get_logs_by_session(&self, session_id: &str) -> Result<Vec<(String, String, DateTime<Utc>)>, MemoryError>` | セッションの全 `(role, content, created_at)` タプル。`created_at` の昇順。 |

---

## ツール埋め込み

`ene-plugin-host` のTool RAGパイプラインを支えます。各ツール仕様は**複数の行**として埋め込まれます — フィールドごと（`summary`、`description`、`capability`、`example`、`negative`）に1行ずつ — これによりフィールド単位の重み付けとクエリ時のmaxプール集約が可能になります。

```rust
/// `(tool_name, field, field_key, version_hash, model_name, embedding_vec, source_text)`
pub type ToolEmbeddingFieldRow = (String, String, String, String, String, Vec<f32>, String);
```

`field_key` は同じ `field` を共有する複数の行を区別します（例: 1つのツールに複数の使用例がある場合の `"ex_0"`、`"ex_1"`）。`source_text` はその埋め込みを生成した正確なテキストを保持し、テキストが変化していない場合の再埋め込みをスキップできるようにします。

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `upsert_tool_embedding_field` | `async fn upsert_tool_embedding_field(&self, tool_name: &str, field: &str, field_key: &str, version_hash: &str, model_name: &str, embedding: &[f32], source_text: &str) -> Result<(), MemoryError>` | `(tool_name, field, field_key, model_name)` をキーにアップサートする。 |
| `list_tool_embedding_fields` | `async fn list_tool_embedding_fields(&self) -> Result<Vec<ToolEmbeddingFieldRow>, MemoryError>` | ベクトルと `source_text` を含む完全な行。インメモリRAGインデックスの再構築に使用される。 |
| `list_tool_embedding_hashes` | `async fn list_tool_embedding_hashes(&self) -> Result<Vec<(String, String, String, String, String)>, MemoryError>` | ベクトルを含まない軽量な `(tool_name, field, field_key, version_hash, model_name)` 行 — 再埋め込みが必要なツールの検出に使用される。 |
| `delete_tool_embeddings` | `async fn delete_tool_embeddings(&self, tool_name: &str) -> Result<usize, MemoryError>` | ツールの全フィールド行を削除する。 |
| `search_tools` | `async fn search_tools(&self, embedding: &[f32], limit: usize, similarity_threshold: f32) -> Result<Vec<(String, f32)>, MemoryError>` | すべてのフィールドに対するコサイン類似度を**ツールごとにmaxプール**し、降順で並べる。`(tool_name, score)` を返す。 |

---

## タイプ付きメモリ

タイプ付きメモリモデルは mind ランタイムの主ストアです。各行は `MemoryKind`、`MemoryStatus`、`MemorySource`、および独立した確信度/顕著性スコアを持ちます。

### `MemoryKind`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// 具体的な出来事や会話（いつ何が起きたか）。
    Episodic,
    /// 事実や一般知識（何が真実か）。
    Semantic,
    /// ユーザーの身元・背景・性格に関する情報。
    UserProfile,
    /// ユーザーとコンパニオンの関係性に関する情報。
    Relationship,
    /// 強い感情的顕著性を持つメモリ。
    Affective,
    /// コンパニオンが行った約束・タスク・義務。
    Commitment,
    /// ユーザーの好き嫌いや好み。
    Preference,
    /// ノウハウや手順に関する指示。
    Procedure,
    /// 過去のやり取りについてのコンパニオン自身の内省。
    Reflection,
}
```

### `MemoryStatus`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    /// 現在関連性が高く、取得可能。
    Active,
    /// 減衰したが、優先度は低いものの取得可能。
    Faded,
    /// 通常の回想には表示されないが保持されている。
    Archived,
    /// ユーザーが異議を唱えた、または訂正したメモリ。
    Disputed,
    /// より新しい矛盾するメモリに置き換えられた。
    Superseded,
    /// ユーザーによって明示的に削除された。
    UserDeleted,
}
```

ステータス間の許可された遷移については、[記憶の忘却ライフサイクル](#忘却ライフサイクル)を参照してください。

### 補助的な値型

```rust
pub enum MemoryScope { Character, User, Shared }

pub enum MemorySource {
    Conversation, UserStated, LlmExtracted, Inferred, Imported, Ccv3,
}

/// `[0.0, 1.0]` にクランプされる。
pub struct MemoryConfidence(f32);
/// `[0.0, 1.0]` にクランプされる。
pub struct MemorySalience(f32);

pub struct AffectAnnotation {
    /// 快-不快（-1.0..=1.0）。
    pub valence: f32,
    /// 興奮-鎮静（-1.0..=1.0）。
    pub arousal: f32,
}
```

### `MemoryItem` / `NewMemoryItem`

```rust
pub struct MemoryItem {
    pub id: Option<i64>,
    pub scope: MemoryScope,
    pub character_id: String,
    pub user_id: String,
    pub kind: MemoryKind,
    pub title: String,
    pub content: String,
    pub source: MemorySource,
    pub source_ref: Option<String>,
    pub confidence: MemoryConfidence,
    pub salience: MemorySalience,
    pub affect: AffectAnnotation,
    pub relationship_impact: f32,
    pub access_count: i64,
    pub last_accessed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub status: MemoryStatus,
    /// 前身へのリンク。supersede操作の後継行にのみ設定される。
    pub supersedes_id: Option<i64>,
    /// ピン留めされたメモリは自然減衰の対象外。
    pub pinned: bool,
    /// メモリが `Faded` ステータスになった時刻（アーカイブ減衰の基準点）。
    pub faded_at: Option<DateTime<Utc>>,
}

/// 新しいメモリ項目を作成するためのペイロード — ストアが管理するフィールド
/// （`id`、`access_count`、`last_accessed_at`、`updated_at`、`faded_at`）は省略される。
pub struct NewMemoryItem {
    pub scope: MemoryScope,
    pub character_id: String,
    pub user_id: String,
    pub kind: MemoryKind,
    pub title: String,
    pub content: String,
    pub source: MemorySource,
    pub source_ref: Option<String>,
    pub confidence: MemoryConfidence,
    pub salience: MemorySalience,
    pub affect: AffectAnnotation,
    pub relationship_impact: f32,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub status: MemoryStatus,
    pub supersedes_id: Option<i64>,
    pub pinned: bool,
    /// 明示的な作成時刻（省略時は挿入時に現在時刻が使われる）。
    pub created_at: Option<DateTime<Utc>>,
}
```

### `Query` / `HybridSearchWeights`

```rust
pub struct Query<'a> {
    pub query_text: &'a str,
    pub embedding: &'a [f32],
    pub character_id: &'a str,
    pub user_id: Option<&'a str>,
    pub model_name: &'a str,
    pub limit: usize,
    pub similarity_threshold: f32,
    pub candidate_pool_size: usize,
    pub query_affect: Option<AffectAnnotation>,
    pub weights: HybridSearchWeights,
    pub decay_half_life_days: f64,
    pub now: DateTime<Utc>,
    /// 結果を返すために必要な最小ハイブリッド合計スコア。
    pub min_score: f32,
    /// アクティブなコミットメント経由で候補が表示された場合のブースト。
    pub commitment_boost: f32,
    /// 純粋な「最近」フォールバック候補を収集する上限数。
    pub recent_fallback_limit: usize,
}

/// ハイブリッド回想スコアの構成要素の重み。デフォルト:
/// `vector 0.40, lexical 0.15, recency 0.10, salience 0.15,`
/// `confidence 0.05, emotional_match 0.05, relationship 0.05, access_boost 0.05`。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HybridSearchWeights {
    pub vector: f32,
    pub lexical: f32,
    pub recency: f32,
    pub salience: f32,
    pub confidence: f32,
    pub emotional_match: f32,
    pub relationship: f32,
    pub access_boost: f32,
}
```

### `ScoredMemory` / `MemoryScoreBreakdown`

```rust
pub struct ScoredMemory {
    pub item: MemoryItem,
    pub breakdown: MemoryScoreBreakdown,
    /// この候補を表示させた取得経路。
    pub sources: Vec<MemoryCandidateSource>,
}

pub struct MemoryScoreBreakdown {
    pub vector_similarity: f32,
    pub lexical_score: f32,
    pub recency_score: f32,
    pub salience: f32,
    pub confidence: f32,
    pub emotional_match: f32,
    pub relationship: f32,
    pub access_boost: f32,
    pub contradiction_penalty: f32,
    pub stale_penalty: f32,
    // ... 加えてランキングに使われる加重合計 `total`。
}

pub enum MemoryCandidateSource { Vector, Lexical, Recent, Commitment }
```

### `MemoryStore` のタイプ付きメモリメソッド

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `insert_typed_memory` | `async fn insert_typed_memory(&self, item: &NewMemoryItem) -> Result<i64, MemoryError>` | 新しい行を挿入し、そのIDを返す。 |
| `get_typed_memory` | `async fn get_typed_memory(&self, id: i64) -> Result<Option<MemoryItem>, MemoryError>` | 主キーで取得する。 |
| `get_typed_memories_by_character` | `async fn get_typed_memories_by_character(&self, character_id: &str, kind: Option<MemoryKind>, limit: usize, offset: usize) -> Result<Vec<MemoryItem>, MemoryError>` | ページネーション付き一覧取得。オプションでkindによるフィルタ。 |
| `count_typed_memories` | `async fn count_typed_memories(&self, character_id: &str, kind: Option<MemoryKind>) -> Result<i64, MemoryError>` | キャラクター（およびオプションのkind）の行数。 |
| `list_typed_memories_by_source_prefix` | `async fn list_typed_memories_by_source_prefix(&self, character_id: &str, prefix: &str, limit: usize) -> Result<Vec<MemoryItem>, MemoryError>` | CCv3カード同期が以前にインデックス化された行を見つけるために使用（例: `"ccv3:lorebook:"`）。 |
| `typed_memory_exists_by_source_ref` | `async fn typed_memory_exists_by_source_ref(&self, character_id: &str, source_ref: &str) -> Result<bool, MemoryError>` | 冪等な再同期のための存在チェック。 |
| `get_active_typed_memory_by_source_ref` | `async fn get_active_typed_memory_by_source_ref(&self, character_id: &str, source_ref: &str) -> Result<Option<MemoryItem>, MemoryError>` | 指定した `source_ref` のアクティブな行を取得する。 |
| `archive_typed_memories_by_source_prefixes` | `async fn archive_typed_memories_by_source_prefixes(&self, character_id: &str, prefixes: &[&str], keep_refs: &HashSet<String>) -> Result<usize, MemoryError>` | 再同期時にもはや存在しない、指定プレフィックス下の行をアーカイブする（例: 削除されたロアブックエントリ）。 |
| `search_typed_memories` | `async fn search_typed_memories(&self, embedding: &[f32], character_id: &str, model_name: &str, limit: usize, similarity_threshold: f32) -> Result<Vec<(MemoryItem, f32)>, MemoryError>` | レガシーなベクトルのみの検索。コサイン類似度だけが必要な呼び出し元向けに引き続き利用可能。 |
| `search` | `async fn search(&self, options: &Query<'_>) -> Result<Vec<ScoredMemory>, MemoryError>` | 主要な回想経路 — ベクトル、字句、新近性、顕著性、確信度、感情、関係性、アクセス、コミットメントの各シグナルを組み合わせる。完全なスコアリング式は [`docs/reference/memory/memory.md`](../memory/memory.md#hybrid-memory-search-73) を参照。 |
| `list_recallable_typed_memories` | `async fn list_recallable_typed_memories(&self, character_id: &str, user_id: Option<&str>, limit: usize) -> Result<Vec<MemoryItem>, MemoryError>` | キャラクター（およびオプションのユーザースコープ）の `Active`/`Faded`/`Disputed` 行。 |
| `supersede_typed_memory` | `async fn supersede_typed_memory(&self, new_item: &NewMemoryItem, superseded_id: i64) -> Result<i64, MemoryError>` | 置き換え行をアトミックに挿入し、以前の行を `Superseded` にマークする。 |
| `update_typed_memory_status` | `async fn update_typed_memory_status(&self, id: i64, new_status: MemoryStatus) -> Result<bool, MemoryError>` | 低レベルのステータス書き込み。内部的には `transition_typed_memory_status` に委譲する。 |
| `transition_typed_memory_status` | `async fn transition_typed_memory_status(&self, id: i64, new_status: MemoryStatus) -> Result<bool, MemoryError>` | 検証付きのライフサイクル遷移 — 許可されていない遷移を拒否する（`forgetting::validate_transition` 参照）。 |
| `bump_typed_memory_access` | `async fn bump_typed_memory_access(&self, id: i64) -> Result<bool, MemoryError>` | `access_count` を増やし `last_accessed_at` を更新する。表示された記憶に対して回想処理から呼び出される。 |
| `pin_typed_memory` | `async fn pin_typed_memory(&self, id: i64, pinned: bool) -> Result<bool, MemoryError>` | メモリをピン留め/解除する（ピン留めされたメモリは自然減衰をスキップする）。 |
| `list_memories_for_decay` | `async fn list_memories_for_decay(&self, character_id: &str, user_id: Option<&str>, statuses: &[MemoryStatus], limit: usize) -> Result<Vec<MemoryItem>, MemoryError>` | 自然減衰処理の対象候補。 |
| `apply_natural_decay_batch` | `async fn apply_natural_decay_batch(&self, character_id: &str, user_id: Option<&str>, now: DateTime<Utc>, half_life_days: f64, limit: usize) -> Result<NaturalDecayReport, MemoryError>` | バッチに対して `forgetting::decay_score` と `target_status_after_decay` を実行し、遷移を適用する。 |
| `upsert_memory_embedding` | `async fn upsert_memory_embedding(&self, memory_item_id: i64, model_name: &str, field: &str, embedding: &[f32]) -> Result<(), MemoryError>` | タイプ付きメモリ行のベクトルを `memory_embeddings` に書き込む。 |

```rust
/// 自然減衰バッチ実行の結果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NaturalDecayReport {
    pub faded_count: usize,
    pub archived_count: usize,
}
```

---

## 感情（Affect）

認知runtimeがターンごとに更新し、再起動を超えて保持される、キャラクター単位（オプションでユーザー単位）の永続的なPAD感情状態。

### `AffectState`

```rust
pub struct AffectState {
    pub character_id: String,
    pub user_id: String,
    /// 快-不快（-1.0..=1.0）。
    pub valence: f32,
    /// 興奮-鎮静（-1.0..=1.0）。
    pub arousal: f32,
    /// 支配-服従（-1.0..=1.0）。
    pub dominance: f32,
    /// ユーザーへの信頼（-1.0..=1.0）。
    pub trust: f32,
    /// ユーザーへの親密度/好意（-1.0..=1.0）。
    pub affinity: f32,
    /// いらだち/苛立ちのレベル（0.0..=1.0）。
    pub irritation: f32,
    /// 好奇心/興味のレベル（0.0..=1.0）。
    pub curiosity: f32,
    /// 疲労/エネルギー消耗（0.0..=1.0）。
    pub fatigue: f32,
    /// 人間が読める気分ラベル（例: `"cheerful"`、`"anxious"`）。
    pub mood_label: String,
    /// 最後の表現/振る舞いの自然言語による説明。
    pub last_expression: String,
    pub discrete_emotions: Vec<DiscreteEmotion>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl AffectState {
    pub fn neutral(character_id: impl Into<String>) -> Self;
    /// すべての数値フィールドを有効な範囲にクランプする。
    pub fn clamp(&mut self);
}
```

### `DiscreteEmotion`

```rust
pub struct DiscreteEmotion {
    /// 例: `"joy"`、`"sadness"`、`"anger"`、`"fear"`、`"surprise"`、`"neutral"`。
    pub label: String,
    /// 強度、`0.0..=1.0`。
    pub intensity: f32,
}

impl DiscreteEmotion {
    pub fn new(label: impl Into<String>, intensity: f32) -> Self;
}
```

### ストアメソッド

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `get_affect_state` | `async fn get_affect_state(&self, character_id: &str) -> Result<AffectState, MemoryError>` | 行がまだ存在しない場合は `AffectState::neutral(character_id)` を返す。 |
| `upsert_affect_state` | `async fn upsert_affect_state(&self, state: &AffectState) -> Result<(), MemoryError>` | 状態をクランプしてから、`character_id` を主キーとしてアップサートする。 |

---

## コミットメント

ベクトル回想に依存しない、コンパニオンの約束やフォローアップ（例: 「次回はXについて話そう」）の台帳。プロンプトへ常に表示できるようにするための独立した仕組みです。

### `CommitmentStatus`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitmentStatus {
    /// 未解決で、プロンプト/回想に表示されるべき。
    Active,
    /// ユーザーまたはコンパニオンが達成済みとマークした。
    Done,
    /// 明示的にキャンセル/取り下げられた。
    Cancelled,
    /// もはや実行不可能（期限切れ、または未完了のまま置き換えられた）。
    Stale,
}
```

### `Commitment` / `NewCommitment`

```rust
pub struct Commitment {
    pub id: Option<i64>,
    pub character_id: String,
    pub user_id: String,
    pub title: String,
    pub description: String,
    pub status: CommitmentStatus,
    pub due_at: Option<DateTime<Utc>>,
    /// 抽出時の生の期限ヒント（例: `"tomorrow"`、`"次回"`）。
    pub due_label: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// 新しいコミットメント行を作成するためのペイロード。
pub struct NewCommitment {
    pub character_id: String,
    pub user_id: String,
    pub title: String,
    pub description: String,
    pub status: CommitmentStatus,
    pub due_at: Option<DateTime<Utc>>,
    pub due_label: Option<String>,
}

/// アクティブコミットメントのプロンプトセクション用の軽量DTO。
pub struct ActiveCommitmentPrompt {
    pub id: i64,
    pub title: String,
    pub description: String,
    pub due_label: Option<String>,
    pub due_at: Option<DateTime<Utc>>,
}
```

### ストアメソッド

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `insert_commitment` | `async fn insert_commitment(&self, item: &NewCommitment) -> Result<i64, MemoryError>` | 新しいコミットメント行を挿入する。 |
| `get_commitment` | `async fn get_commitment(&self, id: i64) -> Result<Option<Commitment>, MemoryError>` | 主キーで取得する。 |
| `list_active_commitments` | `async fn list_active_commitments(&self, character_id: &str, user_id: Option<&str>, limit: usize) -> Result<Vec<Commitment>, MemoryError>` | プロンプト注入用の `Active` 行 — ベクトル検索は関与しない。 |
| `update_commitment_status` | `async fn update_commitment_status(&self, id: i64, new_status: CommitmentStatus) -> Result<bool, MemoryError>` | 汎用的なステータス書き込み。 |
| `complete_commitment` | `async fn complete_commitment(&self, id: i64) -> Result<bool, MemoryError>` | `Done` にマークする。 |
| `cancel_commitment` | `async fn cancel_commitment(&self, id: i64) -> Result<bool, MemoryError>` | `Cancelled` にマークする。 |
| `mark_stale_commitments` | `async fn mark_stale_commitments(&self, now: DateTime<Utc>) -> Result<usize, MemoryError>` | 明示的な `due_at` が過去である期限超過の `Active` 行を `Stale` にマークする。 |

---

## 忘却ライフサイクル

タイプ付きメモリは、ハード削除ではなく明示的なステータス遷移を経て年月とともに変化します。`forgetting.rs` のロジックはすべて純粋な同期処理です — 上記の `MemoryStore` メソッドがそれを呼び出します。

```rust
pub const FADE_THRESHOLD: f32 = 0.40;
pub const ARCHIVE_THRESHOLD: f32 = 0.15;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Invalid memory status transition: {from:?} -> {to:?}")]
pub struct InvalidTransition {
    pub from: MemoryStatus,
    pub to: MemoryStatus,
}
```

| 関数 | シグネチャ | 説明 |
|----------|-----------|-------------|
| `validate_transition` | `fn validate_transition(from: MemoryStatus, to: MemoryStatus) -> Result<(), InvalidTransition>` | `Active → Faded / Superseded / UserDeleted / Disputed` および `Faded → Archived / Disputed` を許可する。それ以外はすべて拒否される。 |
| `emotional_impact` | `fn emotional_impact(affect: AffectAnnotation) -> f32` | `(valence, arousal)` のユークリッドノルムを `[0, 1]` に正規化したもの。 |
| `active_decay_anchor` | `fn active_decay_anchor(item: &MemoryItem) -> DateTime<Utc>` | `last_accessed_at`。無ければ `updated_at` にフォールバック。`Active → Faded` のタイミングに使用。 |
| `faded_decay_anchor` | `fn faded_decay_anchor(item: &MemoryItem) -> DateTime<Utc>` | `faded_at`。無ければ `created_at` にフォールバック。`Faded → Archived` のタイミングに使用。 |
| `decay_score` | `fn decay_score(item: &MemoryItem, now: DateTime<Utc>, half_life_days: f64) -> f32` | ピン留めされたメモリは `1.0` を返す。それ以外は、指数関数的な経過時間による減衰（`exp(-ln2 * age_days / half_life)`）を顕著性・確信度・感情的インパクトでスケーリングし、`[0, 1]` にクランプする。 |
| `target_status_after_decay` | `fn target_status_after_decay(current: MemoryStatus, score: f32) -> Option<MemoryStatus>` | `Active` かつスコアが `FADE_THRESHOLD` 未満 → `Some(Faded)`；`Faded` かつスコアが `ARCHIVE_THRESHOLD` 未満 → `Some(Archived)`；それ以外は `None`。 |

正確な減衰式と本番で使われるデフォルトの閾値については、[`docs/reference/memory/memory.md`](../memory/memory.md#memory-forgetting-lifecycle-76) を参照してください。

---

## メモリスパン & シーン要約

生の会話ログに対するローリング圧縮 — ユーザー/アシスタントのやり取り（またはその連続）ごとに1つの**スパン**があり、オプションでより高い圧縮レベル（シーン → チャプター → アーク）へロールアップされます。ストアは `ene-mind` が生成した要約を永続化するだけで、LLMは呼び出しません。

```rust
pub struct NewMemorySpan {
    pub session_id: String,
    pub turn_start: i32,
    pub turn_end: i32,
    pub raw_excerpt: Option<String>,
    pub compressed_summary: Option<String>,
    /// 0 = シーン、1 = チャプター、2 = アーク。
    pub compression_level: i32,
}

/// プロンプト注入用のアクティブなシーン要約行。
pub struct ActiveSceneSummaryRow {
    pub span_id: i64,
    pub summary: String,
    pub compression_level: i32,
}
```

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `list_session_ids_for_card` | `async fn list_session_ids_for_card(&self, card_name: &str) -> Result<Vec<String>, MemoryError>` | あるキャラクターについてログを持つ全セッションID。 |
| `memory_span_exists` | `async fn memory_span_exists(&self, session_id: &str, turn_start: i32) -> Result<bool, MemoryError>` | スパン挿入前の冪等性チェック。 |
| `insert_memory_span` | `async fn insert_memory_span(&self, span: &NewMemorySpan) -> Result<i64, MemoryError>` | 新しいスパン行を挿入する。 |
| `list_memory_spans_by_session` | `async fn list_memory_spans_by_session(&self, session_id: &str) -> Result<Vec<NewMemorySpan>, MemoryError>` | セッションの全スパン。 |
| `list_memory_spans_by_session_and_level` | `async fn list_memory_spans_by_session_and_level(&self, session_id: &str, compression_level: i32) -> Result<Vec<NewMemorySpan>, MemoryError>` | 圧縮レベルでフィルタしたもの。 |
| `get_active_scene_summary` | `async fn get_active_scene_summary(&self, session_id: &str) -> Result<Option<ActiveSceneSummaryRow>, MemoryError>` | プロンプトの**現在のシーン**セクションに注入される要約を取得する。 |
| `update_span_summary` | `async fn update_span_summary(&self, span_id: i64, summary: &str) -> Result<(), MemoryError>` | 圧縮処理が実行された後、mind ランタイムが生成したスパンの `compressed_summary` を書き込む。 |

---

LLMベースの会話要約は `ene-mind::summarizer` が担当し、`ene-store` は生成された要約の永続化と回想だけを担当します。回想要約のプロンプト整形は `ene-runtime::message_builder` が担当します（ストアには置きません — #122）。

---

## 字句類似度

### `search` — 字句類似度

```rust
pub fn document_lexical_similarity(
    title_a: &str,
    content_a: &str,
    title_b: &str,
    content_b: &str,
) -> f32
```

トークン化された `title + content` のペアに対するJaccard類似度。ハイブリッド型付きメモリのスコアリング、および下流のMMR多様化における候補の重複排除の両方で使用されます（[`docs/reference/memory/memory.md`](../memory/memory.md#mmr-diversification-78) 参照）。

---

## 設定: `StoreConfig`

```rust
pub struct StoreConfig {
    pub enabled: bool = false,
    pub db_path: String,
}

impl StoreConfig {
    pub fn resolve_memory_db_path(&self, character_name: &str) -> std::path::PathBuf;
}
```

`ene_config::define_config!` を通じて `store` 設定セクション配下でロードされます（[`ene-config`](./ene-config.md) 参照）。回想と減衰のポリシーは `MindMemoryConfig` が所有します。

---

## エラー: `MemoryError`

`MemoryError` は `EneMemoryError` の型エイリアスです:

```rust
pub enum EneMemoryError {
    MissingBaseUrl { env_var: String },
    MemoryStoreError(#[from] sea_orm::DbErr),
    MemoryStoreConnectionError(String),
    PromptBuildError(String),
    ApiRequestError(String),
    Config(String),
    Embedding(String),
    /// ストアの `embedding_dim` と長さが合わない、NaN/Infinityを含む、
    /// またはコサイン類似度に使用できないその他の理由。
    InvalidEmbedding(String),
    /// メモリのライフサイクル遷移が許可されなかった（`forgetting::validate_transition` 参照）。
    InvalidTransition { from: MemoryStatus, to: MemoryStatus },
    Other(String),
}

pub type MemoryError = EneMemoryError;
```

---

## 使用例

```rust,no_run
use chrono::Utc;
use ene_store::{
    AffectAnnotation, HybridSearchWeights, MemoryConfidence, MemoryKind, MemorySalience,
    MemoryScope, MemorySource, MemoryStatus, MemoryStore, NewMemoryItem, Query,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store = MemoryStore::open_in_memory(4).await?;

    let id = store
        .insert_typed_memory(&NewMemoryItem {
            scope: MemoryScope::Shared,
            character_id: "Alicia".to_string(),
            user_id: "user1".to_string(),
            kind: MemoryKind::Semantic,
            title: "user_name".to_string(),
            content: "Alice is a designer who loves blue.".to_string(),
            source: MemorySource::Conversation,
            source_ref: Some("session-001".to_string()),
            confidence: MemoryConfidence::new(0.9),
            salience: MemorySalience::new(0.7),
            affect: AffectAnnotation {
                valence: 0.5,
                arousal: 0.0,
            },
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        })
        .await?;

    let results = store
        .search(&Query {
            query_text: "Alice loves blue",
            embedding: None,
            character_id: "Alicia",
            user_id: Some("user1"),
            model_name: "demo",
            limit: 5,
            similarity_threshold: 0.0,
            candidate_pool_size: 16,
            query_affect: None,
            weights: HybridSearchWeights::default(),
            decay_half_life_days: 30.0,
            now: Utc::now(),
            min_score: 0.0,
            commitment_boost: 0.0,
            recent_fallback_limit: 5,
        })
        .await?;
    println!("hits: {}", results.len());
    let _ = id;
    Ok(())
}
```

---

## 関連項目

- [認知Runtime](../architecture/cognitive-runtime.md) — タイプ付きメモリの上に構築されるMemory Arbiter、回想計画、リランキング
- `ene-mind` — このクレートを呼び出すMemory Arbiter、`RecallPlanner`、ターン後メモリライターを所有する
- [`ene-runtime`](./ene-runtime.md) — 外部アクセス用の `MemoryQueryHandle` とアクターレベルの結線
- [`ene-mind`](./ene-mind.md) — `execute_split` を通じたセッション分割とローリング圧縮を駆動する
- [`ene-ai`](./ene-ai.md) — ストレージと検索のための埋め込みを提供する
- [メモリシステム](../memory/memory.md) — 完全な設計ドキュメント: ハイブリッドスコアリング、MMR多様化、コミットメント台帳
- [API v1](../architecture/api-v1.md) — ストア所有（`store.*` のみ；方針は `mind.*`）
