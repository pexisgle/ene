# 長期記憶

SQLite + sqlite-vec + Diesel ベースのエピソディック記憶。ベクトル類似度検索と LLM 駆動の要約を提供。

## 初期化

ランタイムは `EneHandle::open` 中に記憶を初期化します:

1. `embedding` / AI タスク設定から埋め込みプロバイダを作成
2. `store.enabled == true` なら `MemoryStore::open_with_options()` を呼び出し（`StoreConfig` から）
3. `store.integrity_check_on_open` が有効なら `PRAGMA integrity_check` を実行
4. 適用済み SeaORM マイグレーションとバイナリを照合し、DB スキーマが新しい場合はオープンを拒否
5. 未適用マイグレーションがあり `store.backup_on_migrate` が有効なら `{db}.bak.{timestamp}` バックアップを作成してからマイグレーション。失敗時はバックアップから復元
6. sqlite-vec 拡張を登録し、ストア / 埋め込みを `session.memory` にアタッチ

**バックアップ / 復旧 (#239):** `/store backup|list-backups|restore|integrity` または `ene store …` を使用。マイグレーション失敗時は事前バックアップを復元し、半適用状態を残しません。保持数は `store.max_backups`。

記憶は `EneStateSnapshot` 経由でも CLI コマンド (`/memory search`) で利用可能。

## MemoryStore

```rust
pub struct MemoryStore {
    db: DatabaseConnection, // プライベート; sea-orm 接続 (sqlx バックエンドの SQLite プール)
    embedding_dim: usize,   // プライベート; embedding_dim() ゲッターを使用
}
```

`sea-orm` を使用 (内部で `sqlx` の組み込み接続プールを利用)。各操作はプールから非同期に接続を取得。

### データベーステーブル

初期スキーママイグレーションが作成する主要テーブル:

- `conversation_logs` — 生のターン記録
- `typed_memories` / `memory_embeddings` / `memory_links` / `memory_spans` — 認知型付きメモリ
- `affect_states` / `pending_affect_proposals` — 感情レジャー
- `pending_memory_writes` — 遅延ポストターン記憶書き込みの再試行キュー (#240)
- `commitments` — コンパニオンコミットメントレジャー
- `tool_embedding_index` / `__tool_schemas` — Tool RAG + ツール DB IPC メタデータ

列レベルの詳細は [型付きメモリと Memory Arbiter](#typed-memory--memory-arbiter-cognitive-runtime) およびコミットメント節を参照。

### 遅延メモリ書き込みの再試行 (#240)

ポストターンの LLM 抽出と忘却は `Terminal` の後に実行されます。失敗時ランタイムは:

1. `OwnedPostTurnInput` JSON を `pending_memory_writes` にエンキュー
2. `DiagnosticEvent::MemoryWrite` を発行（チャット `EneEvent` ではない）
3. 次回の遅延メモリタスク（および起動時ドレイン）で指数バックオフ再試行

CLI: `/memory status` で pending/permanent 件数、`/memory pending` で一覧、`/memory retry` で即時ドレイン。

### 会話ログ

| メソッド | 説明 |
|---------|------|
| `open(path, dims)` | 永続ストアを開き、マイグレーションを実行 |
| `open_in_memory(dims)` | テスト用のインメモリストア |
| `insert_log(id, card, role, content)` | 単一メッセージを記録 |
| `get_logs_by_session(id)` | セッションの全メッセージを取得 |

### ツール埋め込み (マルチベクトル)

各ツールはフィールドごとに複数の埋め込み行を持ちます (`summary`, `description`, `capability`, `example`, `negative`)。フィールドごとのアプローチにより、`search_tools` はフィールド間の max-pool で関連性を集約できます。`field_key` は単一行フィールドでは `""`、`ToolRagProfile` の例行では `ex_N` です。`model_name` により異なるモデルでの再埋め込みが可能です。

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
- `GgufEmbeddingProvider` — llama-cpp-2 によるローカル GGUF 推論（last-token pooling）、シリアルバッチ
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

各ターン後、**LLM 抽出器（主経路）** が `MemoryCandidate` を生成する。決定論的パターンは明示的な「覚えて／忘れて」のみ: 「覚えて」は LLM 成功時はヒント（失敗・空・無効時はフォールバック）、「忘れて」は安全ネットとして常に Arbiter へ渡す。好み・予定・呼び名などのソフトシグナルは LLM のみ。ツール接地候補は LLM がターンを担当しないときの設定付きフォールバック。**Memory Arbiter**（`ene-mind::memory_writer::MemoryArbiter`）は、既存記憶と照合してから `MemoryStore::insert_typed_memory` または `MemoryStore::supersede_typed_memory` を呼ぶ。

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

`RecallPlanner::to_memory_search_options` は、plan と単一の query embedding から `MemoryStore::search` 用の `Query` を組み立てる helper です。使用するのは `plan.search.primary_query_text`（最初の semantic query）のみです。`semantic_queries` / `episodic_queries` / `required_kinds` は plan hints として残り、multi-query 展開・kind フィルタは後続の recall execution が担当します。

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

### MMR Diversification（#78）

ハイブリッド検索の後、downstream recall execution は `MemoryDiversifyPipeline` による決定論的 MMR 多様化を適用します。

1. `MemoryStore::search` が hybrid `total` 順の `ScoredMemory` を返す。
2. **クラスタ dedup** で近傍重複候補（title + content の lexical Jaccard 類似度 ≥ `mmr_duplicate_cluster_threshold`）をマージし、クラスタ内最高スコアの代表 1 件のみを残す。
3. **Greedy MMR** で `RecallPlan.budget.result_limit` 件まで選択する。`λ * relevance - (1-λ) * max_similarity_to_selected` を用い、`relevance` は pool 内最大値で正規化した `score_breakdown.total`、pairwise 類似度は同じ lexical 指標。selected set に未登場の recall source 種別を持つ候補には `mmr_source_diversity_bonus` を小幅加算する。
4. **Kind quota** で semantic / episodic / user profile / commitment の最低枠（`mmr_min_slots_*`）を確保する。`RecallPlan.required_kinds` に含まれる kind（`preference` / `relationship` / `affective` / `procedure` など）も予算が許せば最低 1 枠を確保する。minimum の合計が `result_limit` を超える場合は、commitment → user profile → preference → semantic → episodic → relationship → affective → procedure → reflection の優先順で枠を割り当てる。

**順序とスコア:** MMR はリスト順序のみを変更する。各結果の `score_breakdown.total` は hybrid-search スコアのまま。

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
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT NULL
)
```

| メソッド | 説明 |
|----------|------|
| `insert_commitment(item)` | 新しい commitment 行を挿入 |
| `get_commitment(id)` | 主キーで取得 |
| `list_active_commitments(character_id, user_id, limit)` | プロンプト注入用の active 行（ベクトル検索なし） |
| `complete_commitment(id)` | `done` に遷移 |
| `cancel_commitment(id)` | `cancelled` に遷移 |
| `mark_stale_commitments(now)` | 期限切れの `active` 行を `stale` に遷移 |

**期限:** 抽出器は `MemoryCandidate::commitment_due` を生成し、ledger 行では `due_label` として保存する。自然言語の期限を `due_at` に parse する処理は未実装のため（[Cognitive Runtime ADR](../architecture/cognitive-runtime.md#companion-commitment-ledger) 参照）、`mark_stale_commitments` が対象にするのは `due_at` が明示的に入っている行のみ。

**ランタイム接続:** `ene-mind::CommitmentLedger::arbitrate_apply_and_sync` は commitment 候補を **ledger-first**（唯一の SoT、#124）で書き込み、他 kind は Memory Arbiter で調停する。任意の typed 行は `typed_memories.commitment_id` で参照できる。`active_prompt_candidates` は Active Commitments `PromptPacket` セクション（#87）向けの軽量 DTO を生成する。CLI の list/complete コマンドは `/commitments`（#94）で利用できる。

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