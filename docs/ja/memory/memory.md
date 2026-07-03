# 長期記憶

SQLite + sqlite-vec + Diesel ベースのエピソディック記憶。ベクトル類似度検索と LLM 駆動の要約を提供。

## 初期化

`EneActor` が `reconfigure()` 中に記憶を初期化:

1. `embedding` 設定から埋め込みプロバイダを作成
2. `memory.enabled == true` なら `MemoryStore::open()` を呼び出し
3. sqlite-vec 拡張を登録しマイグレーションを実行
4. ストアと埋め込みを `session.memory` にアタッチ

記憶は `EneStateSnapshot` 経由でも CLI コマンド (`/memory search`, `/session summaries`) で利用可能。

## MemoryStore

```rust
pub struct MemoryStore {
    db: DatabaseConnection, // プライベート; sea-orm 接続 (sqlx バックエンドの SQLite プール)
    embedding_dim: usize,   // プライベート; embedding_dim() ゲッターを使用
}
```

`sea-orm` を使用 (内部で `sqlx` の組み込み接続プールを利用)。各操作はプールから非同期に接続を取得。

### データベーステーブル

```sql
conversation_summaries (
    id INTEGER PRIMARY KEY,
    session_id TEXT,
    card_name TEXT,
    summary TEXT,
    embedding BLOB,     -- f32 ベクトルとしてバイナリ
    created_at TEXT,    -- RFC3339
    ended_at TEXT       -- RFC3339
)

conversation_keyfacts (
    id INTEGER PRIMARY KEY,
    card_name TEXT,
    summary_id INTEGER REFERENCES conversation_summaries(id),
    key TEXT,
    value TEXT,
    created_at TEXT
)

conversation_logs (
    id INTEGER PRIMARY KEY,
    session_id TEXT,
    card_name TEXT,
    role TEXT,          -- "user" または "assistant"
    content TEXT,
    created_at TEXT
)

tool_embedding_index (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tool_name TEXT NOT NULL,
    field TEXT NOT NULL CHECK (field IN ('summary','description','capability','example','negative')),
    field_key TEXT NOT NULL,        -- "" for ToolSpec, action name for ActionSpec
    version_hash TEXT NOT NULL,
    model_name TEXT NOT NULL,
    embedding BLOB NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(tool_name, field, field_key, model_name)
)

__tool_schemas (
    prefix TEXT PRIMARY KEY,    -- ツール名プレフィックス (例: "fs_", "utility_")
    schema_json TEXT,           -- 完全な JSON スキーマ宣言
    fingerprint TEXT,           -- schema_json の blake3 ハッシュ
    created_at TEXT             -- RFC3339
)
```

`__tool_schemas` テーブルは、ツール DB IPC サーバーがどのツールがテーブルスキーマを宣言したかを追跡するためのメタデータレジストリです。ツール固有のテーブル (例: `fs_undo_entries`, `utility_todo_items`) は、ツールが接続してスキーマを宣言する際に動的に作成されます。

### 要約

| メソッド | 説明 |
|---------|------|
| `open(path, dims)` | 永続ストアを開き、マイグレーションを実行 |
| `open_in_memory(dims)` | テスト用のインメモリストア |
| `insert_summary(session_id, card, summary, facts, emb, ended)` | 要約 + キーファクトをトランザクションで挿入。空の `value` はファクトを削除。 |
| `search_summaries(query_emb, card, limit, threshold)` | `vec_distance_cosine` によるコサイン類似度検索 |
| `list_recent_summaries(card, limit)` | `created_at DESC` で最新順 |
| `delete_summary(id)` | カスケード削除 (関連キーファクトも削除) |
| `count_summaries(card)` | キャラクターの要約数をカウント |

### キーファクト

| メソッド | 説明 |
|---------|------|
| `get_all_keyfacts(card)` | キーごとの最新値 (`ROW_NUMBER() PARTITION BY key ORDER BY created_at DESC`) |
| `upsert_keyfact(card, key, value)` | 新しい行を挿入 (クエリ時に最新が選択) |
| `delete_keyfact(card, key)` | キーの全行を削除 |
| `count_keyfacts(card)` | ユニークキー数をカウント |

### 会話ログ

| メソッド | 説明 |
|---------|------|
| `insert_log(id, card, role, content)` | 単一メッセージを記録 |
| `get_logs_by_session(id)` | セッションの全メッセージを取得 |

### ツール埋め込み (マルチベクトル)

各ツールはフィールドごとに複数の埋め込み行を持ちます (`summary`, `description`, `capability`, `example`, `negative`)。フィールドごとのアプローチにより、`search_tools` はフィールド間の max-pool で関連性を集約できます。`field_key` はトップレベルの ToolSpec 埋め込みとアクションごとの ActionSpec 埋め込みを区別します。`model_name` により異なるモデルでの再埋め込みが可能です。

| メソッド | 説明 |
|---------|------|
| `upsert_tool_embedding_field(name, field, field_key, model, hash, emb)` | 単一フィールドの埋め込みを UPSERT |
| `list_tool_embedding_fields()` | 全 `(name, field, field_key, model, hash, vector)` 行を列挙 |
| `delete_tool_embeddings(name)` | ツールの全フィールド行を削除 |
| `search_tools(query_emb, limit, threshold)` | 全フィールドでコサイン類似度、ツールごとに max-pool で Tool RAG |

## ツール DB IPC サーバー

永続ストレージを必要とするツール (例: undo 用の `ene-tool-fs`、todo 用の `ene-tool-utility`) は、`ene-memory` を直接リンクするのではなく、ツールごとの IPC サーバー経由でデータベースにアクセスします。

### アーキテクチャ

```
Core (ene-core)                     ツールバイナリ (例: ene-tool-fs)
┌─────────────────────┐             ┌──────────────────────┐
│ DbIpcServer         │  Unix sock  │ DbClient             │
│  - リッスン:        │◄───────────►│  - connect()         │
│    ene-db-{name}.sock│             │  - declare_schema()  │
│  - プレフィックス   │             │  - insert/select/... │
│    検証             │             └──────────────────────┘
│  - スキーマ強制     │
│  - sea-orm 経由で   │
│    memory.db に     │
│    ディスパッチ     │
└─────────────────────┘
```

### セキュリティモデル

- 各ツールは `DeclareSchema` でプレフィックス (例: `fs_`, `utility_`) を付けてテーブルを宣言
- 全テーブル名はツールのプレフィックスで始まる必要がある
- 全カラム参照は宣言済みスキーマに対して検証される
- 内部テーブル (`sqlite_*`, `__tool_schemas`, コアテーブル) へのアクセスはブロック
- DDL は公開されない — ツールは宣言済みテーブルで CRUD 操作のみ使用可能

### ene-tool-db クレート

`ene-tool-db` クレートが以下を提供:
- `DbValue` — 型安全な値列挙型 (Null/Bool/Int/Float/Text/Blob)
- `DbFilter` — 構造化フィルタ式 (Eq/Ne/Lt/Gt/In/Like/And/Or/Not/...)
- `DbSchema` / `DbTable` / `DbColumn` / `DbIndex` — スキーマ宣言型
- `DbClient` — ツールごとの Unix ソケットに接続する非同期クライアント
- `DbRequest` / `DbResponse` — IPC メッセージ型

## EmbeddingProvider

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str, kind: EmbeddingKind)
        -> Result<Vec<f32>, EmbeddingError>;
    async fn embed_query(&self, text: &str)
        -> Result<Vec<f32>, EmbeddingError>;
    async fn embed_batch(
        &self, items: &[(&str, EmbeddingKind)],
    ) -> Result<Vec<Vec<f32>>, EmbeddingError>;
    async fn hyde(&self, query: &str) -> Result<String, EmbeddingError>;
    async fn rerank(&self, query: &str, candidates: &[ToolSpec])
        -> Result<Vec<f32>, EmbeddingError>;
    fn dimensions(&self) -> usize;
    fn model_name(&self) -> &str;
}

pub enum EmbeddingKind { Summary, Description, Capability, Example, Negative, Query, Hyde }

/// 埋め込み操作のエラー。
pub enum EmbeddingError {
    /// プロバイダーの初期化に失敗 (モデルファイル欠落、API キー不正など)。
    Init(String),
    /// プロバイダーがエラーを返した。
    Provider(String),
    /// 入力が空または空白のみ。
    EmptyInput,
}
```

実装:
- `CloudEmbeddingProvider` — バッチ埋め込みと LLM による HyDE を備えた OpenAI 互換 API
- `GgufEmbeddingProvider` — Candle によるローカル GGUF 推論 (GPU 不要)、シリアルバッチ
- `HybridRerankProvider` — HyDE / リランキング用にオプションの LLM を備えたプライマリ埋め込みラッパー

## 要約

`summarize_conversation()` が LLM を呼び出して構造化された要約を生成:

```rust
pub struct ConversationSummaryResult {
    pub summary: String,
    pub topics: Vec<String>,
    pub key_facts: Vec<KeyFact>,
}
```

専用の要約モデルは `memory.summarization_model` と `memory.summarization_base_url` で設定可能 (空の場合はメイン LLM にフォールバック)。

## プロンプト注入形式

`format_summaries_for_prompt()` が呼び出された要約をプロンプト用にレンダリング:

```
[Past Conversation Summaries — relevant previous conversations]
- (5 minutes ago) Summary: ...
- (2 hours ago) Summary: ...
```

## 型付き記憶と Memory Arbiter（Cognitive Runtime）

Cognitive Runtime は長期事実を `typed_memories` に保存し、明示的な `MemoryKind` と `MemoryStatus` ライフサイクル（`active`, `faded`, `archived`, `disputed`, `superseded`, `user_deleted`）を持つ。

各ターン後、決定論的/LLM 抽出器が `MemoryCandidate` を生成する。**Memory Arbiter**（`ene-cognition::memory_writer::MemoryArbiter`）は、既存記憶と照合してから `MemoryStore::insert_typed_memory` または `MemoryStore::supersede_typed_memory` を呼ぶ。

主要なストア API：

| メソッド | 説明 |
|----------|------|
| `insert_typed_memory(item)` | 新しい型付き記憶行を挿入 |
| `supersede_typed_memory(new_item, old_id)` | 置換を挿入し、旧行を `superseded` にする（トランザクション） |
| `update_typed_memory_status(id, status)` | ライフサイクル遷移（例: `user_deleted`, `disputed`） |
| `search_typed_memories(embedding, ...)` | アクティブ記憶に対するベクトル類似検索 |
| `search_typed_memories_hybrid(options)` | 説明可能なスコア内訳付きハイブリッド想起 |
| `list_recallable_typed_memories(character_id, user_id, limit)` | 想起対象（`active` / `faded` / `disputed`）の一覧 |

判断ルールとしきい値は [Cognitive Runtime ADR](../architecture/cognitive-runtime.md) を参照。

### Recall Plan 生成（#72）

`ene-cognition::recall::RecallPlanner` は、現在のターン文脈から決定論的に `RecallPlan` を生成します。planner 自体は SQLite 検索や埋め込み provider 呼び出しを行わず、後続段階に渡す検索意図を準備します。

入力:

- 現在の user input
- 直近の raw turns
- active scene summary
- 現在の `AffectState`
- active commitments (`ActiveCommitmentPrompt`)
- character id と optional user id

出力:

- facts / preferences / relationship context / lore 向けの `semantic_queries`
- 過去会話や直近ターン文脈向けの `episodic_queries`
- `required_kinds`（常に `Semantic` / `Episodic` を含み、active commitment がある場合は `Commitment` を含む）
- character/user scope 用の `RecallScopeFilter`
- `cognition.context` 由来の `RecallBudgetHints`
- `MemorySearchOptions` 互換の `RecallSearchHints`（`similarity_threshold`, `min_score`, recency half-life, optional query affect）

`RecallPlanner::to_memory_search_options` は、plan と単一の query embedding から `MemoryStore::search_typed_memories_hybrid` 用の `MemorySearchOptions` を組み立てる helper です。使用するのは `plan.search.primary_query_text`（最初の semantic query）のみです。`semantic_queries` / `episodic_queries` / `required_kinds` / `use_hyde` は plan hints として残り、multi-query 展開・kind フィルタ・HyDE embedding 呼び出しは後続の recall execution が担当します。

### ハイブリッド記憶検索（#73）

型付き記憶の想起は、ベクトル類似度だけでなく複数シグナルを組み合わせられます。`MemorySearchOptions` を `MemoryStore::search_typed_memories_hybrid` に渡すと、`ScoredMemory`（`MemoryScoreBreakdown` と recall source: `vector` / `lexical` / `recent` / `commitment`）が返ります。

デフォルトのスコア式:

```text
score =
  vector_similarity * 0.40
+ lexical_score     * 0.15
+ recency_score     * 0.10
+ salience          * 0.15
+ confidence        * 0.05
+ emotional_match   * 0.05
+ relationship      * 0.05
+ access_boost      * 0.05
+ commitment_boost  (active commitments のみ、既定 0.25)
- contradiction_penalty
- stale_penalty
```

`MemorySearchOptions` では次も指定できます。

- `min_score` — この hybrid total 未満の結果を除外
- `commitment_boost` — commitment 由来候補へのブースト（既定 `0.25`）
- `recent_fallback_limit` — 純 recent フォールバック候補の上限（既定 `5`）

挙動:

- `Archived` / `Superseded` / `UserDeleted` は通常のハイブリッド想起から除外されます。
- `Faded` / `Disputed` は recallable vector search の対象になり、必要に応じてペナルティが付きます。
- `Faded` や期限切れ記憶は想起可能ですが `stale_penalty` が付きます。
- lexical 候補は直近更新プールだけでなく、token ベースの DB 検索で recallable 行から集めます。
- 純 recent フォールバックは上限付きで、無関係な recent memory を候補全体に流し込みません。
- commitment ledger に紐づく active commitment は、ベクトル類似度が低くても結果に含まれます。
- `user_id` 指定時は他ユーザーの user-specific memory を除外します。`user_id` が空の character scope 行は引き続き表示対象です。
- 複数ソースから集めた候補は memory id で de-dupe してから順位付けします。

ベクトル類似度のみが必要な既存呼び出し向けに `search_typed_memories(...)` は従来どおり残しています。

### 説明可能な想起理由（#74）

`MemoryStore::search_typed_memories_hybrid` は生の `ScoredMemory` を返します。理由付けは `ene-cognition::recall` の責務で、後続の recall execution がそれを `RecalledMemory` DTO に変換します。各結果には次が含まれます。

- `item` — 型付き記憶行
- `reason` — UX / debug / prompt 向けの単一 `RecallReason`
- `score_breakdown` — ハイブリッド検索と同じ `MemoryScoreBreakdown`
- `sources` — 寄与した recall source（`vector` / `lexical` / `recent` / `commitment`）

`RecallReason` の variant:

| 理由 | 典型的なシグナル |
|------|------------------|
| `similar_topic` | vector/lexical ハイブリッド一致のデフォルト |
| `recent_conversation` | `recent` source または `Episodic` kind |
| `active_promise` | `commitment` source または `Commitment` kind |
| `character_lore` | `MemorySource::Ccv3`（CCv3 lorebook） |
| `user_preference` | `Preference` または `UserProfile` kind |
| `emotional_continuity` | `Affective` kind、または `emotional_match >= 0.85` |
| `pinned` | 将来の user-pinned memory 用（現時点では推論されない） |

`RecallResultMapper::map`、`RecallPlanner::explain_results`、`RecalledMemory::from_scored`、または `explain_scored_memories` でハイブリッド検索結果を変換します。CLI inspect や JSON snapshot 向けにすべて `Serialize` / `Deserialize` 対応です。

理由の優先順位（先に一致したものを採用）: `ActivePromise` → `CharacterLore` → `UserPreference` → `EmotionalContinuity` → `RecentConversation` → `SimilarTopic`。

## Companion Commitment Ledger（約束・タスク台帳）

「次回これを話そう」などのフォローアップは専用の `commitments` テーブルに保存する：

```sql
commitments (
    id INTEGER PRIMARY KEY,
    character_id TEXT NOT NULL,
    user_id TEXT NOT NULL DEFAULT '',
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',  -- active | done | cancelled | stale
    due_at TEXT NULL,
    due_label TEXT NULL,                    -- 抽出時の生の期限ヒント（"tomorrow", "次回"）
    source_memory_id INTEGER NULL REFERENCES typed_memories(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT NULL
)
```

| メソッド | 説明 |
|----------|------|
| `insert_commitment(item)` | 新しい commitment 行を挿入 |
| `get_commitment(id)` | 主キーで取得 |
| `get_commitment_by_source_memory(memory_id)` | typed memory に紐づく ledger 行を検索 |
| `list_active_commitments(character_id, user_id, limit)` | プロンプト注入用の active 行（ベクトル検索なし） |
| `complete_commitment(id)` | `done` に遷移 |
| `cancel_commitment(id)` | `cancelled` に遷移 |
| `mark_stale_commitments(now)` | 期限切れの `active` 行を `stale` に遷移 |

**期限:** 抽出器は `MemoryCandidate::commitment_due` を生成し、ledger 行では `due_label` として保存する。自然言語の期限を `due_at` に parse する処理は未実装のため（[Cognitive Runtime ADR](../architecture/cognitive-runtime.md#companion-commitment-ledger) 参照）、`mark_stale_commitments` が対象にするのは `due_at` が明示的に入っている行のみ。

**ランタイム接続:** `ene-cognition::CommitmentLedger::arbitrate_apply_and_sync` は Memory Arbiter の実行と commitment 行の同期を一括で行う。`active_prompt_candidates` は Active Commitments `PromptPacket` セクション（#87）向けの軽量 DTO を生成する。ターンごとに sync を呼ぶ MemoryWriter オーケストレーションは #100 で接続予定。CLI の list/complete コマンドは #94 で追加予定。