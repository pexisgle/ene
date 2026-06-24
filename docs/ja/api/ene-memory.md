# `ene-memory` — APIリファレンス

> **クレート:** `ene-memory`  
> **役割:** 会話サマリー・キーファクト・会話ログ・ツール埋め込みの永続ベクトルメモリストア。

---

## 概要

`ene-memory` は Ene の長期記憶サブシステムを提供します。ストレージバックエンドとして **SQLite** を使用し、ベクトル類似度検索に **`sqlite-vec`**、すべてのSQLアクセスに **Diesel** を採用しています。

各キャラクターは `card_name` をキーとして共有データベース内に独立した名前空間を持ちます。メモリシステムは以下を保存します：

- **サマリー** — 過去セッションのLLM生成サマリー。セマンティック想起のための埋め込みを含む。
- **キーファクト** — セッションから抽出した構造化されたキーと値のペア（ユーザーの好み、重要な日付など）。
- **会話ログ** — すべてのセッションのすべてのメッセージの不変な記録。
- **ツール埋め込み** — ツールRAGインデックス用のツール仕様フィールドの埋め込み。

> **アーキテクチャ上の制約:** ツールバイナリは `ene-memory` を直接リンクしてはいけません。`DbIpcServer` / `ene-tool-db` IPCクライアントを通じてデータベースにアクセスしてください。

---

## `MemoryStore`

```rust
pub struct MemoryStore { /* 非公開 */ }
```

### 構築

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `open` | `fn open(path: &Path, embedding_dim: usize) -> Result<Self, MemoryError>` | 指定したパスにSQLiteデータベースを開く（または作成する）。埋め込み次元を指定する。 |
| `open_in_memory` | `fn open_in_memory(embedding_dim: usize) -> Result<Self, MemoryError>` | インメモリデータベースを開く。主にテスト用。 |
| `embedding_dim` | `fn embedding_dim(&self) -> usize` | 設定済みの埋め込み次元数を返す。 |

---

## サマリーメソッド

サマリーは主要なメモリ単位で、完了した会話セッションをそれぞれ1件表します。

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `insert_summary` | `fn insert_summary(&self, session_id: &str, card_name: &str, summary: &str, key_facts: &[KeyFact], embedding: &[f32], ended_at: DateTime<Utc>) -> Result<i64, MemoryError>` | 埋め込みベクトル付きで新しいセッションサマリーを挿入する。行IDを返す。 |
| `search_summaries` | `fn search_summaries(&self, query_embedding: &[f32], card_name: &str, limit: usize, similarity_threshold: f32) -> Result<Vec<RecalledSummary>, MemoryError>` | コサイン類似度でサマリーを検索する。`similarity_threshold` 以上の結果を最大 `limit` 件返す。 |
| `list_recent_summaries` | `fn list_recent_summaries(&self, card_name: &str, limit: usize) -> Result<Vec<ConversationSummary>, MemoryError>` | `ended_at` の降順で最新の `limit` 件のサマリーを返す。 |
| `count_summaries` | `fn count_summaries(&self, card_name: &str) -> Result<i64, MemoryError>` | 指定キャラクターのサマリー総数を返す。 |
| `delete_summary` | `fn delete_summary(&self, id: i64) -> Result<usize, MemoryError>` | 行IDでサマリーを削除する。削除された行数を返す。 |

---

## キーファクトメソッド

キーファクトは、セッションから抽出されてキャラクターごとに永続化される構造化 `key=value` ペアです。

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `get_all_keyfacts` | `fn get_all_keyfacts(&self, card_name: &str) -> Result<Vec<KeyFact>, MemoryError>` | 指定キャラクターの全キーファクトを返す。 |
| `upsert_keyfact` | `fn upsert_keyfact(&self, card_name: &str, key: &str, value: &str) -> Result<(), MemoryError>` | 指定キーのファクトを挿入または更新する。 |
| `delete_keyfact` | `fn delete_keyfact(&self, card_name: &str, key: &str) -> Result<usize, MemoryError>` | 指定キーのファクトを削除する。削除された行数を返す。 |
| `count_keyfacts` | `fn count_keyfacts(&self, card_name: &str) -> Result<i64, MemoryError>` | 指定キャラクターのファクト総数を返す。 |

---

## 会話ログメソッド

会話ログは、全ユーザー/アシスタントメッセージを追記専用で記録します。

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `insert_log` | `fn insert_log(&self, session_id: &str, card_name: &str, role: &str, content: &str) -> Result<i64, MemoryError>` | ログエントリを挿入する。行IDを返す。 |
| `spawn_insert_log` | `fn spawn_insert_log(store: &Arc<Self>, session_id: &str, card_name: &str, role: &str, content: &str)` | ファイア・アンド・フォーゲットのログ挿入。Tokioタスクを起動し、エラーはログに記録されるが伝播しない。 |
| `get_logs_by_session` | `fn get_logs_by_session(&self, session_id: &str) -> Result<Vec<(String, String, String)>, MemoryError>` | セッションの全ログエントリを `(ロール, 内容, created_at)` タプル（RFC3339 文字列）で返す。 |

---

## ツール埋め込みメソッド

ツール埋め込みはRAGベースのツール選択システムを支えます。

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `upsert_tool_embedding_field` | `fn upsert_tool_embedding_field(&self, tool_name: &str, field: &str, field_key: &str, version_hash: &str, model_name: &str, embedding: &[f32]) -> Result<(), MemoryError>` | ツール仕様の特定フィールドの埋め込みを挿入または更新する。 |
| `list_tool_embedding_fields` | `fn list_tool_embedding_fields(&self) -> Result<Vec<ToolEmbeddingFieldRow>, MemoryError>` | 全ツール埋め込みレコードを返す（古い埋め込みの検出に使用）。 |
| `delete_tool_embeddings` | `fn delete_tool_embeddings(&self, tool_name: &str) -> Result<usize, MemoryError>` | ツールの全埋め込みレコードを削除する。 |
| `search_tools` | `fn search_tools(&self, query_embedding: &[f32], limit: usize, similarity_threshold: f32) -> Result<Vec<(String, f32)>, MemoryError>` | ベクトル類似度でツールを検索する。`(tool_name, スコア)` ペアを返す。 |

---

## コンビニエンスメソッド

### `recall_context`

```rust
pub fn recall_context(
    &self,
    card_name: &str,
    query_embedding: &[f32],
    limit: usize,
    similarity_threshold: f32,
) -> Result<(Vec<RecalledSummary>, Vec<KeyFact>), MemoryError>
```

関連サマリー（ベクトル検索）と指定キャラクターの全キーファクトを1回の呼び出しで取得します。`ene-core` の `fetch_memory_context` が使用します。

---

## グローバル関数

### `init_sqlite_vec`

```rust
pub fn init_sqlite_vec(conn: &mut SqliteConnection) -> Result<(), MemoryError>
```

既存のDiesel接続に `sqlite-vec` 拡張を読み込んで初期化します。`MemoryStore::open` によって自動的に呼び出されます。

### `format_summaries_for_prompt`

```rust
pub fn format_summaries_for_prompt(summaries: &[RecalledSummary]) -> String
```

想起されたサマリーのスライスをLLMシステムプロンプトへの注入に適した読みやすいテキストブロックにフォーマットします。

### `summarize_conversation`

```rust
pub async fn summarize_conversation(
    provider: &dyn LlmProvider,
    history: &[LlmMessage],
    card_name: &str,
    user_name: &str,
    existing_facts: &[KeyFact],
) -> Result<ConversationSummaryResult, MemoryError>
```

LLMを呼び出して、完了したセッションからサマリーを生成し、更新されたキーファクトを抽出します。`ene-session` の `execute_split` が内部的に使用します。

---

## データ型

### `ConversationSummary`

```rust
pub struct ConversationSummary {
    /// データベースの行ID。
    pub id: i64,

    /// このサマリーが作成されたセッション。
    pub session_id: String,

    /// このサマリーが属するキャラクター。
    pub card_name: String,

    /// LLM生成のサマリーテキスト。
    pub summary: String,

    /// サマリーの埋め込みベクトル。
    pub embedding: Vec<f32>,

    /// このサマリーが作成された日時。
    pub created_at: DateTime<Utc>,

    /// セッションが終了した日時。
    pub ended_at: DateTime<Utc>,
}
```

### `RecalledSummary`

```rust
pub struct RecalledSummary {
    /// 基となるサマリーエントリ。
    pub entry: ConversationSummary,

    /// クエリに対するコサイン類似度スコア。範囲: `[-1.0, 1.0]`
    /// （正規化された `[0, 1]` スコアではなく、純粋な角度距離）。
    pub similarity: f32,
}
```

### `KeyFact`

```rust
pub struct KeyFact {
    /// ファクトの識別子（例：`"user_name"`、`"favorite_color"`）。
    pub key: String,

    /// ファクトの値。
    pub value: String,
}
```

### `ConversationSummaryResult`

```rust
pub struct ConversationSummaryResult {
    /// 生成されたサマリーテキスト。
    pub summary: String,

    /// 更新または新規抽出されたキーファクト。
    pub key_facts: Vec<KeyFact>,
}
```

---

## データベースアーキテクチャ

| レイヤー | 技術 |
|--------|------|
| SQL ORM | [`sea-orm`](https://www.sea-ql.org/SeaORM)（`sqlx-sqlite` フィーチャ） |
| コネクションプーリング | [`sqlx::Pool`](https://docs.rs/sqlx)（`sqlx-sqlite` バックエンドに組み込み） |
| ベクトル検索 | [`sqlite-vec`](https://github.com/asg017/sqlite-vec)（SQLite拡張として読み込む） |
| ストレージバックエンド | SQLite（ユーザープロファイルごとに1ファイル） |

> **ルール:** このクレートのすべてのSQLには `sea-orm`（および `sea-orm-migration`）を使用してください。`rusqlite` や `diesel` の導入は**禁止**です（AGENTS.md §7.3の制約事項）。

マイグレーションは `crates/ene-memory/src/migrator/src/m{YYYYMMDD}_{name}/` 配下の Rust モジュールとして定義し（`sea-orm-migration` CLI でスキャフォールド）、`Migrator` の re-export 経由で埋め込みます。

---

## 使用例

```rust
use ene_memory::{MemoryStore, KeyFact};
use std::path::Path;

// データベースを開く（または作成する）
let store = MemoryStore::open(Path::new("data/memory.db"), 1024)?;

// ユーザーに関するファクトをアップサート
store.upsert_keyfact("alice", "favorite_color", "blue")?;
store.upsert_keyfact("alice", "city", "東京")?;

// クエリのコンテキストを想起する
let query_vec: Vec<f32> = vec![/* ... 埋め込みベクトル ... */];
let (summaries, facts) = store.recall_context("alice", &query_vec, 5, 0.7)?;

println!("{}件のサマリーと{}件のファクトを想起しました", summaries.len(), facts.len());
for s in &summaries {
    println!("[{:.2}] {}", s.similarity, s.entry.summary);
}
for f in &facts {
    println!("  {}: {}", f.key, f.value);
}
```

---

## 関連項目

- [`ene-session`](./ene-session.md) — サマリーを作成するセッションスプリットを駆動する
- [`ene-core`](./ene-core.md) — 外部アクセス用 `MemoryQueryHandle`
- [`ene-embedding`](./ene-embedding.md) — 保存と検索のための埋め込みを提供する
