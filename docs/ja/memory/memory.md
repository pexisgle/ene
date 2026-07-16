# 長期記憶

SQLite + sqlite-vec + Diesel ベースのエピソディック記憶。ベクトル類似度検索と LLM 駆動の要約を提供。

## 初期化

`EneActor` が `reconfigure()` 中に記憶を初期化:

1. `embedding` 設定から埋め込みプロバイダを作成
2. `store.enabled == true` なら `MemoryStore::open()` を呼び出し
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

永続ストレージを必要とするツール (例: undo 用の `ene-tool-fs`、todo 用の `ene-tool-utility`) は、`ene-store` を直接リンクするのではなく、ツールごとの IPC サーバー経由でデータベースにアクセスします。

### アーキテクチャ

```
Core (ene-runtime)                     ツールバイナリ (例: ene-tool-fs)
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

`ene-mind::summarizer::summarize_conversation()` が LLM を呼び出して構造化された要約を生成:

```rust
pub struct ConversationSummaryResult {
    pub summary: String,
    pub topics: Vec<String>,
    pub key_facts: Vec<KeyFact>,
}
```

生成された要約とキーファクトは `ene-store` が永続化します。ストア自体は LLM や埋め込みプロバイダーに依存しません。

## プロンプト注入形式

`ene-runtime::message_builder` が呼び出された要約をプロンプト用にレンダリングします（ストアはプロンプト整形を持ちません — #122）:

```
[Past Conversation Summaries — relevant previous conversations]
- (5 minutes ago) Summary: ...
- (2 hours ago) Summary: ...
```

## 型付き記憶と Memory Arbiter（Cognitive Runtime）

Cognitive Runtime は長期事実を `typed_memories` に保存し、明示的な `MemoryKind` と `MemoryStatus` ライフサイクル（`active`, `faded`, `archived`, `disputed`, `superseded`, `user_deleted`）を持つ。

各ターン後、**LLM 抽出器（主経路）** が `MemoryCandidate` を生成する。決定論的パターンはヒントとして渡し、LLM 成功時は自動永続化しない。LLM 失敗時または無効時は決定論的候補がフォールバックする。ツール接地候補は常に対象。**Memory Arbiter**（`ene-mind::memory_writer::MemoryArbiter`）は、既存記憶と照合してから `MemoryStore::insert_typed_memory` または `MemoryStore::supersede_typed_memory` を呼ぶ。

主要なストア API：

| メソッド | 説明 |
|----------|------|
| `insert_typed_memory(item)` | 新しい型付き記憶行を挿入 |
| `supersede_typed_memory(new_item, old_id)` | 置換を挿入し、旧行を `superseded` にする（トランザクション） |
| `update_typed_memory_status(id, status)` | 低レベルなステータス更新（遷移検証なし） |
| `transition_typed_memory_status(id, status)` | 検証付きライフサイクル遷移（#76） |
| `pin_typed_memory(id, pinned)` | ピン / ピン解除（自然減衰から除外） |
| `apply_natural_decay_batch(...)` | 減衰スコアに基づく `active → faded → archived` 一括処理 |
| `search_typed_memories(embedding, ...)` | アクティブ記憶に対するベクトル類似検索 |
| `search(options)` | 説明可能なスコア内訳付きハイブリッド想起 |
| `list_recallable_typed_memories(character_id, user_id, limit)` | 想起対象（`active` / `faded` / `disputed`）の一覧 |

判断ルールとしきい値は [Cognitive Runtime ADR](../architecture/cognitive-runtime.md) を参照。

### Recall Plan 生成（#72）

`ene-mind::recall::RecallPlanner` は、現在のターン文脈から決定論的に `RecallPlan` を生成します。planner 自体は SQLite 検索や埋め込み provider 呼び出しを行わず、後続段階に渡す検索意図を準備します。

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
- `mind.context` 由来の `RecallBudgetHints`
- `Query` 互換の `RecallSearchHints`（`similarity_threshold`, `min_score`, recency half-life, optional query affect）

`RecallPlanner::to_memory_search_options` は、plan と単一の query embedding から `MemoryStore::search` 用の `Query` を組み立てる helper です。使用するのは `plan.search.primary_query_text`（最初の semantic query）のみです。`semantic_queries` / `episodic_queries` / `required_kinds` / `use_hyde` は plan hints として残り、multi-query 展開・kind フィルタ・HyDE embedding 呼び出しは後続の recall execution が担当します。

### ハイブリッド記憶検索（#73）

型付き記憶の想起は、ベクトル類似度だけでなく複数シグナルを組み合わせられます。`Query` を `MemoryStore::search` に渡すと、`ScoredMemory`（`MemoryScoreBreakdown` と recall source: `vector` / `lexical` / `recent` / `commitment`）が返ります。

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

`Query` では次も指定できます。

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

`MemoryStore::search` は生の `ScoredMemory` を返します。理由付けは `ene-mind::recall` の責務で、後続の recall execution がそれを `RecalledMemory` DTO に変換します。各結果には次が含まれます。

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

### CCv3 キャラクタ記憶インデックス（#82–#84）

mind ランタイムの `CognitionEngine::sync_character_memories` が CCv3 カードデータをキャラクタスコープ typed memory にコンパイルする:

| ソース | `source_ref` プレフィックス | `MemoryKind` | 備考 |
|--------|---------------------------|--------------|------|
| Lorebook エントリ | `ccv3:lorebook:{id}` | `Semantic` | constant は `pinned`。トリガーキーは **content** 先頭の `Triggers: …` |
| `mes_example` チャンク | `ccv3:style:{index}` | `Procedure` | ターンごとに Style Examples セクションへ |

カードから消えた `source_ref` は reindex 時に archive される。同一 `source_ref` で内容が変わった行は **supersede** され再埋め込みされる。セッションは `MemoryContext.ccv3_memory_hash` に lorebook/style の結合ハッシュを保持し、ターン間の冗長 sync を省略する。Store ヘルパー: `list_typed_memories_by_source_prefix`, `get_active_typed_memory_by_source_ref`, `archive_typed_memories_by_source_prefixes`, `supersede_typed_memory`。

### Tool Result Grounding（#92）

ツール実行結果は、cognitive の post-turn writer を通じて typed memory に取り込まれる:

- 各呼び出し結果は `ToolResultSummary { tool_name, success, summary }` として収集される。
- 生出力は `max_summary_chars` で sanitize/truncate され、スクリーンショット payload は固定の sentinel 文字列に置き換えられる。
- 成功結果は `Procedure` 記憶として保存される（`source_ref` プレフィックス: `tool:`）。
- 失敗結果は `Reflection` 記憶として保存され、同じ失敗経路の反復を避けるヒントに使われる。
- 短くユーザー向けの成功結果は、設定有効時に `Episodic` 記憶としても保存できる。

これにより recall に有用な情報を残しつつ、巨大な生ツール出力の丸ごと保存を防ぐ。

### Optional Memory Reranking（#77）

ハイブリッド検索の後、downstream recall execution は `RecalledMemory` への変換前に optional な rerank を実行できます。

1. `MemoryStore::search` が hybrid `total` 順の `ScoredMemory` を返す。
2. `mind.memory.rerank_enabled` が `false`（既定）の場合、順序は変更されない。
3. 有効時、`MemoryRerankPipeline` が上位 `rerank_candidate_limit` 件を LLM reranker に渡す。prompt には recall question と各候補の `content` のみを含め、title / source / kind / user metadata は含めない。limit を超える候補は hybrid 順序のまま rerank 対象の末尾に追加される。
4. timeout、provider error、structured output の不正時は hybrid search 順序にフォールバックする。
5. `RecallResultMapper::map` が（rerank 後の）リストを説明可能な `RecalledMemory` に変換する。

**順序とスコア:** rerank はリスト順序のみを変更する。各結果の `score_breakdown.total` は hybrid-search スコアのままなので、先頭の recalled item が下位より低い `total` を表示することがある。

**プライバシーとコスト:** rerank を有効にすると、複数候補がある recall のたびに保存済み memory content が設定された LLM provider に送信されます。候補数と content 長に比例して latency と token コストが増えます。専用 rerank model を使わない場合は `rerank_candidate_limit` を控えめに保つことを推奨します。parse 失敗時のログには構造的なエラー詳細と response 長のみを記録し、LLM payload 全文は含めません。

**トレース:** rerank latency と状態は `component = "MemoryRerank"`（`elapsed_ms`, `candidate_count`, `reranked_count`, `tail_count`, `outcome`、skip 時は `skip_reason`）でログ出力されます。

### MMR Diversification（#78）

ハイブリッド検索の後、optional LLM rerank の前に、downstream recall execution は `MemoryDiversifyPipeline` による決定論的 MMR 多様化を適用します。

1. `MemoryStore::search` が hybrid `total` 順の `ScoredMemory` を返す。
2. **クラスタ dedup** で近傍重複候補（title + content の lexical Jaccard 類似度 ≥ `mmr_duplicate_cluster_threshold`）をマージし、クラスタ内最高スコアの代表 1 件のみを残す。
3. **Greedy MMR** で `RecallPlan.budget.result_limit` 件まで選択する。`λ * relevance - (1-λ) * max_similarity_to_selected` を用い、`relevance` は pool 内最大値で正規化した `score_breakdown.total`、pairwise 類似度は同じ lexical 指標。selected set に未登場の recall source 種別を持つ候補には `mmr_source_diversity_bonus` を小幅加算する。
4. **Kind quota** で semantic / episodic / user profile / commitment の最低枠（`mmr_min_slots_*`）を確保する。`RecallPlan.required_kinds` に含まれる kind（`preference` / `relationship` / `affective` / `procedure` など）も予算が許せば最低 1 枠を確保する。minimum の合計が `result_limit` を超える場合は、commitment → user profile → preference → semantic → episodic → relationship → affective → procedure → reflection の優先順で枠を割り当てる。
5. `mind.memory.mmr_enabled` が `false` の場合、`result_limit` で truncate するのみで順序は変更しない。
6. optional LLM rerank（#77）は多様化後のリストに対して実行される。各 `ScoredMemory` の hybrid スコアは変更されない。

**順序とスコア:** MMR と rerank はリスト順序のみを変更する。各結果の `score_breakdown.total` は hybrid-search スコアのまま。

**トレース:** 多様化は `component = "Recall"`, `stage = "diversify"`（`input_count`, `pool_count`, `output_count`, `clusters_merged`, `kind_distribution`）でログ出力されます。

### Memory Forgetting Lifecycle（#76）

typed memory はハードデリートではなく明示的なステータス遷移で経年変化する。自然減衰とユーザーの明示的忘却は別パス。

**許可される単一ステップ遷移**（`ene-store::forgetting::validate_transition`）:

| From | To |
|------|-----|
| `active` | `faded`, `superseded`, `user_deleted`, `disputed` |
| `faded` | `archived`, `disputed` |

ライフサイクルの status 更新はすべて `transition_typed_memory_status` 経由（`update_typed_memory_status` も委譲）。`supersede_typed_memory` は従来どおり、後継 insert と `active/faded/disputed → superseded` をトランザクションで処理する。

**`faded_at` 列:** `active → faded` 遷移時に、当時の active 減衰アンカー（`last_accessed_at`、なければ `updated_at`）を記録する。既存の `faded` 行はマイグレーションで `faded_at = updated_at` にバックフィル。archive 減衰は遷移後の `updated_at` ではなく `faded_at`（なければ `created_at`）を使う。

**自然減衰スコア**（`decay_score`。hybrid-recall の `recency_score` とは別）:

```text
retention =
  exp(-ln2 * age_days / half_life)
  * (0.5 + 0.5 * salience)
  * (0.5 + 0.5 * confidence)
  * (0.7 + 0.3 * emotional_impact)
```

- **active の fade 判定:** `active_decay_anchor`（`last_accessed_at` → `updated_at`）から `age_days`。
- **faded の archive 判定:** `faded_decay_anchor`（`faded_at` → `created_at`）から `age_days`。
- `half_life` は `mind.memory.default_forgetting_half_life_days`。
- `pinned` 記憶は retention `1.0` を返し、自然減衰の対象外。

**閾値（デフォルト）:**

- `retention < 0.40` かつ `active` → `faded`
- `retention < 0.15` かつ `faded` → `archived`

**明示的忘却 vs 自然減衰:**

| パス | トリガー | 結果 |
|------|---------|------|
| ユーザー忘却 | Memory Arbiter `MarkUserDeleted` | 即時 `user_deleted`（減衰をバイパス） |
| 自然減衰 | `decay_enabled` が true のとき `ForgettingLifecycle::apply` | `active → faded → archived` |

`ForgettingLifecycle::apply` はメモリ有効時、各アシスタントターン後に `streaming_cognitive.rs` から実行される。recall 時は表示された記憶に `bump_typed_memory_access` で `last_accessed_at` を更新する。

**プロンプトの不確実性マーカー:** typed recall の `RecalledMemory` をプロンプト文字列に変換する際、`format_recalled_content` が以下を付与:

- `faded` 記憶（および低信頼度の `active`）に `[uncertain] `
- `disputed` 記憶に `[disputed] `

レガシー `conversation_keyfacts` には、明示的な移行で typed memory に変換した場合のみ同じ uncertain/disputed マーカーが付く。通常の mind recall は `recall_context` 行をマージしない。

## レガシーテーブルからの移行

mind ランタイムは **新規 memory を typed memory のみに書き込みます**。移行またはリセットを行うまで、レガシーテーブル（`conversation_summaries`, `conversation_keyfacts`）は **read-only** です。

### マッピング規則（one-shot migration）

| レガシーテーブル | 移行先 | 規則 |
|------------------|--------|------|
| `conversation_summaries` | `typed_memories` (`Episodic`) | `content` ← 要約本文; `confidence = 0.7`, `salience = 0.5`; embedding を `memory_embeddings` にコピー; `source_ref = legacy:summary:{id}` |
| `conversation_keyfacts` | `UserProfile` または `Preference` | `pref_*`, `like`, `dislike` に一致 → `Preference`; それ以外 → `UserProfile`; `title` = key, `content` = value |
| `conversation_logs` | `memory_spans` | user/assistant ペアごとに span; `raw_excerpt` に本文; `compressed_summary` は rolling compression（#79）でランタイム更新 |

ランタイム（認知パス）では、`ene-mind::context::compression` が `mind.context` の閾値超過時にシーンレベル span を書き込む。アクティブなシーン要約は `MemoryStore::get_active_scene_summary` 経由で `PromptPacket` の **Current Scene** セクションに注入される。`conversation_logs` は常に保持される。

移行状態はキャラクターごとに `memory_migration_meta` に記録されます。

### ユーザー選択肢

1. **何もしない（read-only レガシーデータ）** — 移行完了まで、レガシー summaries/keyfacts は通常の mind recall の外に残ります。新規抽出 memory は typed のみ。各ターンの raw log は引き続き `conversation_logs` に追記。
2. **`/memory migrate legacy`** — 単一トランザクションで one-shot 変換 + migration marker 設定。以降 typed-only recall。
3. **`/memory reset legacy --yes`** — レガシーテーブル truncate + typed memory クリア（破壊的操作、確認必須）。memory span は当該カードの log に紐づく session のみ削除。

### strict モード

`mind.memory.require_migration = true` にすると、**レガシー summaries または keyfacts** が残り migration 未完了の場合 recall をブロックします。通常チャットで増える `conversation_logs` だけではブロックされません。`LegacyMemoryNotMigrated` と reset/migrate ガイダンスを返します。

### リセット手順

移行せず初期化する場合:

```bash
ene-cli
/memory reset legacy --yes
```

またはユーザーデータディレクトリの SQLite ファイルを削除して再起動（全キャラクターの memory が失われます）。

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

**ランタイム接続:** `ene-mind::CommitmentLedger::arbitrate_apply_and_sync` は commitment 候補を **ledger-first**（唯一の SoT、#124）で書き込み、他 kind は Memory Arbiter で調停する — typed→ledger の dual-write / `sync_from_applied_decisions` はない。任意の typed 行は `typed_memories.commitment_id` で参照できる。`active_prompt_candidates` は Active Commitments `PromptPacket` セクション（#87）向けの軽量 DTO を生成する。CLI の list/complete コマンドは `/commitments`（#94）で利用できる。

## メモリージャーナル（Desktop UX）

Desktop の **メモリージャーナル** ページ（`ene-desktop` 設定 → Memory）で、typed memory・感情状態・アクティブなコミットメントを確認できる。

### ブラウズモード（デフォルト）

- 現在のキャラクターについて、設定ユーザー **および** グローバル行（`user_id = ""`）を対象に一覧表示する。
- 種別・ステータス・スコープ・信頼度・重要度・由来メタデータ・ピン状態を表示する。
- ライフサイクル操作は `MemoryJournalPresenter` がストア規則に合わせて出し分ける:
  - **Active:** Pin/Unpin、忘却（`user_forget_typed_memory`）、異議
  - **Faded:** Pin/Unpin、アーカイブ（`transition_typed_memory_status`）、異議、復元（`user_restore_typed_memory`）
  - **Archived / UserDeleted / Superseded / Disputed:** Pin/Unpin、復元
- フィルタ: 削除済み・アーカイブ済み・置き換え済みを個別に表示可能。

### 想起デバッグモード

- クエリ入力で `search_typed_memories_explained`（ハイブリッド検索 + #74 想起理由）を実行する。
- `RecallReason` ラベルとスコア内訳（vector / lexical / recency / salience / confidence）を表示する。

### API

| 層 | メソッド |
|----|----------|
| `ene-store` | `list_journal_memories`, `user_restore_typed_memory`, `user_forget_typed_memory` |
| `ene-runtime` | `MemoryQueryHandle::list_journal_memories`, `user_restore_typed_memory`, `search_typed_memories_explained` |