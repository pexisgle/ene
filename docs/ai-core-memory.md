# 長期記憶システム

SQLite + sqlite-vec（ベクトル拡張）+ Diesel ORM による埋め込みベースの長期記憶。

## 初期化

`init_memory()` が設定に基づいて MemoryStore と EmbeddingProvider を生成する。

```rust
pub fn init_memory(
    settings: &AiSettings
) -> Result<(Arc<MemoryStore>, Arc<dyn EmbeddingProvider>), String>
```

1. DB パス解決（明示指定 or `{card_dir}/memory.db`）
2. 親ディレクトリ作成
3. EmbeddingProvider 作成（`create_embedding_provider()`、Api/Local を選択）
4. MemoryStore::open(db_path, dimensions) でテーブル作成・マイグレーション

## MemoryStore

```rust
pub struct MemoryStore {
    conn: Mutex<SqliteConnection>,
    embedding_dim: usize,
}
```

`Send + Sync`（`unsafe impl`）、Mutex で全アクセスを直列化。

### テーブル構造

```sql
conversation_summaries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT,
    card_name TEXT,
    summary TEXT,
    embedding BLOB,       -- f32 ベクトルのバイナリ（リトルエンディアン）
    ended_at TEXT,
    created_at DATETIME
)

conversation_keyfacts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    card_name TEXT,
    summary_id INTEGER REFERENCES conversation_summaries(id),
    key TEXT,
    value TEXT,
    created_at DATETIME
)

conversation_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT,
    card_name TEXT,
    role TEXT,
    content TEXT,
    created_at DATETIME
)

tool_embeddings (
    tool_name TEXT PRIMARY KEY,
    version_hash TEXT,
    embedding BLOB,
    created_at DATETIME
)
```

### 要約操作

| メソッド | 説明 |
|----------|------|
| `insert_summary(session_id, card_name, summary, key_facts, embedding, ended_at)` | 要約 + キーファクトをトランザクションで挿入（空valueのキーファクトは削除扱い） |
| `search_summaries(query_embedding, card_name, limit, threshold)` | `vec_distance_cosine` でコサイン類似度検索 |
| `list_recent_summaries(card_name, limit)` | `created_at DESC` で一覧 |

### キーファクト操作

| メソッド | 説明 |
|----------|------|
| `get_all_keyfacts(card_name)` | `ROW_NUMBER() PARTITION BY key ORDER BY created_at DESC` で各キーの最新値を取得 |
| `upsert_keyfact(card_name, key, value)` | 新規行挿入（クエリ時に最新が採用される） |
| `delete_keyfact(card_name, key)` | 該当行全削除 |

### ツール埋め込み操作

| メソッド | 説明 |
|----------|------|
| `upsert_tool_embedding(tool_name, version_hash, embedding)` | UPSERT |
| `search_tools(query_embedding, limit, threshold)` | Tool RAG 用コサイン類似度検索 |

## EmbeddingProvider

```rust
pub trait EmbeddingProvider: Send + Sync {
    fn embed(&self, text: &str) -> Result<Vec<f32>, AiCoreError>;
    fn embed_query(&self, text: &str) -> Result<Vec<f32>, AiCoreError>;
    fn dimensions(&self) -> usize;
    fn model_name(&self) -> String;
}
```

2 実装:

| 実装 | 説明 |
|------|------|
| `ApiEmbeddingProvider` | OpenAI 互換 API 経由の埋め込み |
| `GgufEmbeddingProvider` | Candle フレームワークによる GGUF 量子化モデルのローカル推論 |

## 要約（Summarizer）

`summarize_conversation()` が LLM を呼び出して会話要約を生成する。

```rust
pub struct ConversationSummaryResult {
    pub summary: String,
    pub topics: Vec<String>,
    pub key_facts: Vec<KeyFact>,
}
```

- JSON Schema で構造化された応答を LLM から取得
- 既存のキーファクトを入力として渡し、update/delete/keep の判断を LLM に委ねる
- パース失敗時はフォールバック（JSON 抽出→生テキスト）

## 要約フォーマット（プロンプト注入用）

`format_summaries_for_prompt()` が検索結果を以下の形式に整形する:

```
[Past Conversation Summaries — relevant previous conversations]
- (5 minutes ago) Summary: ...
- (2 hours ago) Summary: ...
```
