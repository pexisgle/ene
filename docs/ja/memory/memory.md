# 長期記憶

SQLite + sqlite-vec + Diesel による、ベクトル類似度検索と LLM 駆動要約を備えたエピソディック記憶。

## 初期化

`AiRuntime::init()` 内で実行:
1. `embedding` 設定から埋め込みプロバイダを作成
2. `memory.enabled == true` の場合、`MemoryStore::open()` を呼び出し
3. sqlite-vec 拡張の登録とマイグレーションを実行

## MemoryStore

```rust
pub struct MemoryStore {
    pool: r2d2::Pool<SqliteConnection>,
    pub embedding_dim: usize,
}
```

`r2d2` コネクションプーリングを使用。各操作でプールからコネクションを取得します。

### データベーステーブル

```sql
conversation_summaries (
    id INTEGER PRIMARY KEY,
    session_id TEXT,
    card_name TEXT,
    summary TEXT,
    embedding BLOB,     -- f32 ベクトル (バイナリ)
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
    role TEXT,          -- "user" or "assistant"
    content TEXT,
    created_at TEXT
)

tool_embeddings (
    tool_name TEXT PRIMARY KEY,
    version_hash TEXT,
    embedding BLOB,
    created_at TEXT
)
```

### 要約

| メソッド | 説明 |
|---------|------|
| `open(path, dims)` | 永続ストアを開き、マイグレーションを実行 |
| `open_in_memory(dims)` | テスト用インメモリストア |
| `insert_summary(id, card, summary, facts, emb, ended)` | 要約 + キーファクトをトランザクションで挿入。空 `value` はファクト削除扱い |
| `search_summaries(query_emb, card, limit, threshold)` | `vec_distance_cosine` によるコサイン類似度検索 |
| `list_recent_summaries(card, limit)` | `created_at DESC` で最新取得 |
| `delete_summary(id)` | カスケード削除 (関連キーファクトも削除) |
| `count_summaries(card)` | キャラクターの要約数をカウント |

### キーファクト

| メソッド | 説明 |
|---------|------|
| `get_all_keyfacts(card)` | キーごとに最新の値を取得 (`ROW_NUMBER() PARTITION BY key ORDER BY created_at DESC`) |
| `upsert_keyfact(card, key, value)` | 新しい行を挿入 (クエリ時に最新が採用) |
| `delete_keyfact(card, key)` | キーの全行を削除 |
| `count_keyfacts(card)` | 個別キー数をカウント |

### 会話ログ

| メソッド | 説明 |
|---------|------|
| `insert_log(id, card, role, content)` | 単一メッセージを記録 |
| `get_logs_by_session(id)` | セッションの全メッセージを取得 |

### ツール埋め込み

| メソッド | 説明 |
|---------|------|
| `upsert_tool_embedding(name, hash, emb)` | UPSERT ツール埋め込み |
| `list_tool_embeddings()` | 全 (name, hash, vector) を一覧 |
| `delete_tool_embedding(name)` | ツール埋め込みを削除 |
| `search_tools(query_emb, limit, threshold)` | Tool RAG 用コサイン類似度ツール検索 |

## EmbeddingProvider

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError>;
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbeddingError>;
    fn dimensions(&self) -> usize;
    fn model_name(&self) -> &str;
}
```

実装:
- `ApiEmbeddingProvider` — OpenAI 互換 API
- `GgufEmbeddingProvider` — Candle によるローカル GGUF 推論 (GPU 不要)

## 要約

`summarize_conversation()` が LLM を呼び出して構造化された要約を生成:

```rust
pub struct ConversationSummaryResult {
    pub summary: String,
    pub topics: Vec<String>,
    pub key_facts: Vec<KeyFact>,
}
```

専用の要約モデルは `memory.summarization_model` と `memory.summarization_base_url` で設定できます (空の場合はメイン LLM にフォールバック)。

## プロンプト注入形式

`format_summaries_for_prompt()` が呼び出された要約をプロンプト用に整形:

```
[Past Conversation Summaries — relevant previous conversations]
- (5 minutes ago) Summary: ...
- (2 hours ago) Summary: ...
```
