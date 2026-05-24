# 長期記憶システム

SQLite + sqlite-vec + Diesel による埋め込みベースの長期記憶。  
初期化は `AiRuntime::init()` 内で実施される。

## 初期化フロー（概念）
1. `embedding` セクションから埋め込みプロバイダを作成
2. `memory.enabled == true` の場合、`MemoryStore::open()` を実行
3. sqlite-vec 拡張登録とマイグレーションを適用

## MemoryStore

```rust
pub struct MemoryStore {
    pool: r2d2::Pool<SqliteConnection>,
    pub embedding_dim: usize,
}
```

`r2d2` プールを使用し、各操作でコネクションを取得する。

### テーブル構造
```sql
conversation_summaries (
    id INTEGER PRIMARY KEY,
    session_id TEXT,
    card_name TEXT,
    summary TEXT,
    embedding BLOB,
    created_at TEXT, -- RFC3339
    ended_at TEXT    -- RFC3339
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
    role TEXT,
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

### 要約操作
| メソッド | 説明 |
|---|---|
| `insert_summary(session_id, card_name, summary, key_facts, embedding, ended_at)` | 要約 + キーファクトをトランザクションで挿入（空 value は削除扱い） |
| `search_summaries(query_embedding, card_name, limit, threshold)` | `vec_distance_cosine` で類似度検索 |
| `list_recent_summaries(card_name, limit)` | `created_at DESC` で一覧 |

### キーファクト操作
| メソッド | 説明 |
|---|---|
| `get_all_keyfacts(card_name)` | `ROW_NUMBER() PARTITION BY key ORDER BY created_at DESC` で最新のみ取得 |
| `upsert_keyfact(card_name, key, value)` | 新規行挿入（クエリ時に最新が採用される） |
| `delete_keyfact(card_name, key)` | 該当キーを全削除 |

### ツール埋め込み操作
| メソッド | 説明 |
|---|---|
| `upsert_tool_embedding(tool_name, version_hash, embedding)` | UPSERT |
| `search_tools(query_embedding, limit, threshold)` | Tool RAG 用コサイン類似度検索 |

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
- `ApiEmbeddingProvider`（OpenAI 互換 API）
- `GgufEmbeddingProvider`（Candle によるローカル推論）

## 要約（Summarizer）
`summarize_conversation()` が LLM を呼び出して要約を生成する。

```rust
pub struct ConversationSummaryResult {
    pub summary: String,
    pub topics: Vec<String>,
    pub key_facts: Vec<KeyFact>,
}
```

## 要約フォーマット（プロンプト注入用）
`format_summaries_for_prompt()` が検索結果を次の形式に整形する:

```
[Past Conversation Summaries — relevant previous conversations]
- (5 minutes ago) Summary: ...
- (2 hours ago) Summary: ...
```
