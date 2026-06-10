# `ene-tool-db`

> ツールバイナリ向けの型付き CRUD データベースクライアント — `ene-core` 内の `DbIpcServer` と IPC で通信します。

ツールバイナリは `ene-memory` を直接リンクしては**なりません**。代わりに、`ene-tool-db` が提供する型付きクライアントを使用して、`ene-core` プロセス内で動作する `DbIpcServer` と通信します。サーバーはプレフィックスベースのアクセス制御を強制し、各ツールは自身が宣言したプレフィックスで始まるテーブル（例：`fs_`、`utility_`）のみ読み書きできます。

関連ページ：[AGENTS.md §7.3 のメモリシステムルール](../../AGENTS.md) および [`ene-tool-host`](ene-tool-host.md)。

---

## `DbClient`

データベースサーバーへのメインハンドルです。

```rust
pub struct DbClient { /* private */ }
```

### コンストラクタ

```rust
impl DbClient {
    /// 指定されたソケットパスの DbIpcServer に接続する。
    ///
    /// ソケットパスは、プロセスがスポーンされる際に
    /// `ene-tool-host` が環境変数として提供します。
    pub fn connect(socket_path: &Path) -> Result<Self, DbError>;
}
```

### スキーマ宣言

```rust
impl DbClient {
    /// このツールのスキーマを宣言する。起動時に 1 回だけ、
    /// 読み書きの前に呼び出す必要があります。
    ///
    /// サーバーはまだ存在しないテーブルとインデックスを作成します。
    /// (created_tables, created_indexes) を返します。
    pub fn declare_schema(
        &self,
        schema: DbSchema,
    ) -> Result<(Vec<String>, Vec<String>), DbError>;
}
```

### CRUD メソッド

| メソッド | シグネチャ | 説明 |
|---|---|---|
| `insert` | `(table: &str, row: Row) -> Result<i64, DbError>` | 行を挿入します。新しい `rowid` を返します。 |
| `upsert` | `(table: &str, row: Row, conflict_columns: &[&str]) -> Result<i64, DbError>` | 競合時に挿入または更新します。`rowid` を返します。 |
| `select` | `(table, columns, filter, order_by, limit) -> Result<Vec<Row>, DbError>` | 行をクエリします。`columns` に `None` を渡すと `SELECT *` になります。 |
| `update` | `(table: &str, set: BTreeMap<String, DbValue>, filter: DbFilter) -> Result<u64, DbError>` | 一致する行を更新します。影響を受けた行数を返します。 |
| `delete` | `(table: &str, filter: DbFilter) -> Result<u64, DbError>` | 一致する行を削除します。削除された行数を返します。 |
| `count` | `(table: &str, filter: DbFilter) -> Result<i64, DbError>` | 一致する行を数えます。 |
| `last_insert_rowid` | `() -> Result<i64, DbError>` | 最後に挿入した行の `rowid` を返します。 |
| `ping` | `() -> Result<(), DbError>` | 接続のヘルスチェックを行います。 |
| `shutdown` | `() -> Result<(), DbError>` | クライアント接続を正常に閉じます。 |

### 使用例

```rust
use ene_tool_db::{DbClient, DbSchema, DbTable, DbColumn, DbType, DbFilter, DbValue, Row};
use std::path::Path;

let client = DbClient::connect(Path::new("/run/ene/db.sock"))?;

client.declare_schema(DbSchema {
    prefix: "fs".to_string(),
    tables: vec![
        DbTable {
            name: "fs_visited".to_string(),
            columns: vec![
                DbColumn {
                    name: "id".to_string(),
                    ty: DbType::Integer,
                    primary_key: true,
                    auto_increment: true,
                    ..Default::default()
                },
                DbColumn {
                    name: "path".to_string(),
                    ty: DbType::Text,
                    ..Default::default()
                },
                DbColumn {
                    name: "visited_at".to_string(),
                    ty: DbType::Integer,
                    ..Default::default()
                },
            ],
        },
    ],
    indexes: vec![],
})?;

// 挿入
let mut row = Row::new();
row.insert("path".into(), DbValue::Text("/home/user/notes.md".into()));
row.insert("visited_at".into(), DbValue::Integer(1_700_000_000));
let rowid = client.insert("fs_visited", row)?;

// 検索
let rows = client.select(
    "fs_visited",
    None,  // SELECT *
    DbFilter::Eq { col: "path".into(), val: DbValue::Text("/home/user/notes.md".into()) },
    None,  // ORDER BY なし
    Some(10),
)?;

// 削除
let deleted = client.delete(
    "fs_visited",
    DbFilter::Eq { col: "id".into(), val: DbValue::Integer(rowid) },
)?;
println!("{deleted} 行を削除しました");
```

---

## `DbError`

```rust
pub enum DbError {
    /// 低レベルのトランスポート障害。
    Transport(io::Error),
    /// サーバーがアプリケーションレベルのエラーを返した。
    Server { code: DbErrorCode, message: String },
    /// サーバーがデコードできないレスポンスを送った。
    UnexpectedResponse(String),
    /// IPC 接続が予期せず閉じられた。
    ConnectionClosed,
}
```

### `DbErrorCode`

```rust
pub enum DbErrorCode {
    AccessDenied,       // このツールではテーブルのプレフィックスが許可されていない
    TableNotFound,
    ConstraintViolation,
    InvalidSchema,
    QueryError,
    Internal,
}
```

---

## スキーマ型

### `DbSchema`

```rust
pub struct DbSchema {
    /// ツール名のプレフィックスと一致する必要があります
    /// （例："fs" は "fs_*" テーブルすべてに対応）。
    pub prefix: String,
    pub tables: Vec<DbTable>,
    pub indexes: Vec<DbIndex>,
}
```

### `DbTable`

```rust
pub struct DbTable {
    /// スキーマのプレフィックスの後に `_` が続く名前で始まる必要があります。
    pub name: String,
    pub columns: Vec<DbColumn>,
}
```

### `DbColumn`

```rust
pub struct DbColumn {
    pub name: String,
    pub ty: DbType,
    pub primary_key: bool,
    pub auto_increment: bool,
    pub not_null: bool,
    pub unique: bool,
    /// SQLite の DEFAULT 式（存在する場合）。
    pub default: Option<String>,
}
```

### `DbIndex`

```rust
pub struct DbIndex {
    pub name: String,
    pub table: String,
    pub columns: Vec<String>,
    pub unique: bool,
}
```

---

## 値型

### `DbType`

```rust
pub enum DbType {
    Integer,
    Real,
    Text,
    Blob,
}
```

### `DbValue`

```rust
pub enum DbValue {
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
    Null,
}
```

### `Row`

```rust
pub type Row = BTreeMap<String, DbValue>;
```

`Row` はカラム名から値への順序付きマップです。`BTreeMap` を使用することで、挿入・アップサート操作でのカラム順序が決定論的になります。

---

## フィルター DSL

`DbFilter` は SQL 文字列を書かずに `WHERE` 句を構築するための再帰的な列挙型です。

```rust
pub enum DbFilter {
    /// すべての行にマッチする（WHERE 句なし）。
    All,

    // 論理演算
    And(Vec<DbFilter>),
    Or(Vec<DbFilter>),

    // 比較演算
    Eq  { col: String, val: DbValue },
    Ne  { col: String, val: DbValue },
    Gt  { col: String, val: DbValue },
    Lt  { col: String, val: DbValue },
    Ge  { col: String, val: DbValue },
    Le  { col: String, val: DbValue },

    // 文字列
    Like { col: String, pattern: String },

    // NULL チェック
    IsNull    { col: String },
    IsNotNull { col: String },
}
```

### フィルターの例

```rust
// WHERE path LIKE '/home/%' AND visited_at > 1700000000
let filter = DbFilter::And(vec![
    DbFilter::Like { col: "path".into(), pattern: "/home/%".into() },
    DbFilter::Gt   { col: "visited_at".into(), val: DbValue::Integer(1_700_000_000) },
]);

// WHERE id IS NOT NULL
let filter = DbFilter::IsNotNull { col: "id".into() };

// フィルターなし（全行 SELECT）
let filter = DbFilter::All;
```

---

## プレフィックスアクセス制御

`ene-core` の `DbIpcServer` はすべてのリクエストに対して以下のルールを強制します：

> ツールは `<tool_prefix>_` で始まる名前のテーブルのみアクセスできます。

ツールが自分のプレフィックス外のテーブルへの読み書きを試みた場合、サーバーは `DbError::Server { code: DbErrorCode::AccessDenied, … }` を返します。

この分離は意図的なものです：ツールは別々の信頼されていないプロセスであり、互いの状態を読んだり破損させたりできないようにする必要があります。

---

## 関連ページ

- [`ene-tool-host`](ene-tool-host.md) — ツールプロセスをスポーンし、DB IPC ソケットパスを提供
- [メモリシステムルール](../../AGENTS.md#73-memory-system-rules-strict) — コアクレートの Diesel / sqlite-vec の制約
- [ツールシステム概要](../tools/overview.md)
