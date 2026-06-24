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
    pub async fn connect(socket_path: &Path) -> Result<Self, DbError>;

    /// 明示的な認証トークン付きで接続する
    /// （デフォルトの `ENE_DB_AUTH_TOKEN` 環境変数を上書き）。
    pub async fn connect_with_token(
        socket_path: &Path,
        token: &str,
    ) -> Result<Self, DbError>;
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
    pub async fn declare_schema(
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

すべての CRUD メソッドは `async` です。

### 使用例

```rust
use ene_tool_db::{DbClient, DbSchema, DbTable, DbColumn, DbType, DbFilter, DbValue, Row};
use std::path::Path;

let client = DbClient::connect(Path::new("/run/ene/db.sock")).await?;

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
}).await?;

// 挿入
let mut row = Row::new();
row.insert("path".into(), DbValue::Text("/home/user/notes.md".into()));
row.insert("visited_at".into(), DbValue::Int(1_700_000_000));
let rowid = client.insert("fs_visited", row).await?;

// 検索
let rows = client.select(
    "fs_visited",
    None,  // SELECT *
    DbFilter::Eq {
        column: "path".into(),
        value: DbValue::Text("/home/user/notes.md".into()),
    },
    None,  // ORDER BY なし
    Some(10),
).await?;

// 削除
let deleted = client.delete(
    "fs_visited",
    DbFilter::Eq {
        column: "id".into(),
        value: DbValue::Int(rowid),
    },
).await?;
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
    /// ツールが要求されたリソースへのアクセス権を持たない
    /// （例：宣言されたプレフィックス外のテーブルへアクセスしようとした）。
    PermissionDenied,
    /// 指定されたテーブルが存在しないか、宣言されていない。
    UnknownTable,
    /// 指定されたカラムがテーブルに存在しない。
    UnknownColumn,
    /// 値の型がカラムの宣言された型と一致しない。
    TypeMismatch,
    /// フィルター式が無効。
    InvalidFilter,
    /// サーバー内部エラーが発生した。
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
    /// カラム型。デフォルトは `DbType::Text`。
    pub ty: DbType,
    /// NULL を許容するかどうか。デフォルトは `false`。
    pub nullable: bool,
    /// 主キーかどうか。デフォルトは `false`。
    pub primary_key: bool,
    /// 自動増分するかどうか。デフォルトは `false`。
    pub auto_increment: bool,
    /// UNIQUE 制約を持つかどうか。デフォルトは `false`。
    pub unique: bool,
    /// デフォルト値（型付き）。デフォルトは `None`。
    pub default: Option<DbValue>,
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
    /// 64-bit 符号付き整数。
    Integer,
    /// 64-bit IEEE 浮動小数点。
    Real,
    /// UTF-8 テキスト文字列。（デフォルト）
    Text,
    /// バイナリブロブ。
    Blob,
    /// 真偽値（INTEGER の 0/1 として保存）。
    Boolean,
}
```

### `DbValue`

```rust
pub enum DbValue {
    /// SQL NULL。
    Null,
    /// 真偽値。
    Bool(bool),
    /// 64-bit 符号付き整数。
    Int(i64),
    /// 64-bit 浮動小数点。
    Float(f64),
    /// UTF-8 テキスト文字列。
    Text(String),
    /// バイナリブロブ。
    Blob(Vec<u8>),
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
    /// すべての行にマッチする。
    Always,

    // 論理演算
    And(Vec<DbFilter>),
    Or(Vec<DbFilter>),
    Not(Box<DbFilter>),

    // 比較演算
    Eq  { column: String, value: DbValue },
    Ne  { column: String, value: DbValue },
    Gt  { column: String, value: DbValue },
    Lt  { column: String, value: DbValue },
    Ge  { column: String, value: DbValue },
    Le  { column: String, value: DbValue },

    // 文字列
    Like { column: String, pattern: String },

    // NULL チェック
    IsNull    { column: String },
    IsNotNull { column: String },
}
```

### フィルターの例

```rust
// WHERE path LIKE '/home/%' AND visited_at > 1700000000
let filter = DbFilter::And(vec![
    DbFilter::Like {
        column: "path".into(),
        pattern: "/home/%".into(),
    },
    DbFilter::Gt {
        column: "visited_at".into(),
        value: DbValue::Int(1_700_000_000),
    },
]);

// WHERE id IS NOT NULL
let filter = DbFilter::IsNotNull {
    column: "id".into(),
};

// フィルターなし（全行 SELECT）
let filter = DbFilter::Always;
```

---

## プレフィックスアクセス制御

`ene-core` の `DbIpcServer` はすべてのリクエストに対して以下のルールを強制します：

> ツールは `<tool_prefix>_` で始まる名前のテーブルのみアクセスできます。

ツールが自分のプレフィックス外のテーブルへの読み書きを試みた場合、サーバーは `DbError::Server { code: DbErrorCode::PermissionDenied, … }` を返します。

この分離は意図的なものです：ツールは別々の信頼されていないプロセスであり、互いの状態を読んだり破損させたりできないようにする必要があります。

---

## 関連ページ

- [`ene-tool-host`](ene-tool-host.md) — ツールプロセスをスポーンし、DB IPC ソケットパスを提供
- [メモリシステムルール](../../AGENTS.md#73-memory-system-rules-strict) — コアクレートの sea-orm / sqlite-vec の制約
- [ツールシステム概要](../tools/overview.md)
