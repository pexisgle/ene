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

pub enum EmbeddingError { Provider(String), Timeout(Duration) }
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