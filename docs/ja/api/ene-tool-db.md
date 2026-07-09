# `ene-tool-db` — APIリファレンス

> **クレート:** `ene-tool-db`
> **役割:** ツールバイナリ向けの型付きCRUDデータベースクライアント — IPC経由で `ene-core` 内の `DbIpcServer` と通信する。

---

## 概要

ツールバイナリは `ene-memory` を直接リンクしては**いけません**。代わりに、`ene-tool-db` は `ene-core` 内で実行されている `DbIpcServer` プロセスと、長さプレフィックス付き JSON プロトコルで話す型付きの async クライアント（`DbClient`）を提供します。サーバーはプレフィックスベースのアクセス制御を強制するため、各ツールは自身が宣言したプレフィックス（例：`fs_`、`utility_`）で始まる名前のテーブルのみを読み書きできます。

すべての接続状態（基盤となるソケットに加え、[`reconnect`](#reconnect) に必要なソケットパスと認証トークン）は `&mut self` の背後にあります。各呼び出しは単一のストリーム上で行われる同期的なリクエスト/レスポンスのラウンドトリップであるため、すべての CRUD メソッドは `&mut DbClient` を取ります。また `reconnect` は、サーバー再起動後にそのストリームを置き換えるために排他アクセスを必要とします。

参照: [AGENTS.md §7.3 メモリシステムのルール](../../AGENTS.md) および [`ene-tool-host`](./ene-tool-host.md)。

---

## `DbClient`

データベースサーバーへの主要なハンドルです。

```rust
pub struct DbClient { /* 非公開 */ }
```

### コンストラクタ

| コンストラクタ | シグネチャ | 説明 |
|---|---|---|
| `connect` | `async fn connect(socket_path: &Path) -> Result<Self, DbError>` | 認証せずに `socket_path` の `DbIpcServer` に接続する。サーバーは、未認証クライアントから最初の非 `Handshake` リクエストを受け取った時点で接続を閉じるため、これはサーバーが認証トークンなしで設定されている場合のみ有用。トークンが利用可能な場合は `connect_with_token` を優先すべき。 |
| `connect_with_token` | `async fn connect_with_token(socket_path: &Path, token: &str) -> Result<Self, DbError>` | 接続し、直ちに `token` を伴う `Handshake` リクエストを送信する。サーバーがトークンを拒否した場合は `Err(DbError::Auth { .. })` を返す。ソケットパスとトークンはクライアント上にキャプチャされ、後で `reconnect` が透過的に再認証できるようにする。 |

ソケットパスは、プロセスが起動される際に `ene-tool-host` が設定する環境変数を通じてツールバイナリに渡されます。認証トークンは `ENE_DB_AUTH_TOKEN` を通じて渡されます。

### 接続管理

| メソッド | シグネチャ | 説明 |
|---|---|---|
| `socket_path` | `fn socket_path(&self) -> &Path` | このクライアントが接続されたソケットパスを返す。 |
| `reconnect` | `async fn reconnect(&mut self) -> Result<(), DbError>` | 接続時にキャプチャされたソケットパスと（存在する場合）認証トークンを使用して、サーバー再起動後に IPC 接続を再確立する。`DbError::ConnectionClosed` を受け取った後にこれを呼び出すことで、クライアントのアイデンティティを失わずに回復できる — 以前の設計には回復パスがなく、クライアントをゼロから再構築する必要があった。 |

### スキーマ宣言

```rust
impl DbClient {
    /// このツールのスキーマを宣言する。起動時に一度だけ呼び出す必要があり、
    /// 読み書きを行う前に実行しなければならない。
    ///
    /// サーバーは、まだ存在しないテーブルとインデックスを作成する。
    /// `(created_tables, created_indexes)` を返す。
    pub async fn declare_schema(
        &mut self,
        schema: DbSchema,
    ) -> Result<(Vec<String>, Vec<String>), DbError>;
}
```

### CRUDメソッド

すべての CRUD メソッドは `async` で `&mut self` を取ります（クライアントごとに同時に1つのリクエストのみが処理される — 接続は内部的に多重化されていません）。

| メソッド | シグネチャ | 説明 |
|---|---|---|
| `insert` | `async fn insert(&mut self, table: &str, row: Row) -> Result<i64, DbError>` | 行を挿入する。新しい `rowid` を返す。 |
| `upsert` | `async fn upsert(&mut self, table: &str, row: Row, conflict_columns: &[&str]) -> Result<i64, DbError>` | 挿入し、競合時は更新する（`ON CONFLICT (conflict_columns) DO UPDATE`）。`rowid` を返す。 |
| `select` | `async fn select(&mut self, table: &str, columns: &[&str], filter: DbFilter, order_by: Vec<DbOrderBy>, limit: Option<u64>) -> Result<Vec<Row>, DbError>` | 行を検索する。すべての列を選択する（`SELECT *`）には `columns` に `&[]` を渡し、明示的な並び順を指定しない場合は `order_by` に `vec![]` を渡す — 「フィルターなし」／「順序なし」を表す `Option` ラップされたセンチネル値は存在しないため、空のスライス／ベクタを使用する。 |
| `update` | `async fn update(&mut self, table: &str, set: BTreeMap<String, DbValue>, filter: DbFilter) -> Result<u64, DbError>` | 一致する行を更新する。影響を受けた行数を返す。 |
| `delete` | `async fn delete(&mut self, table: &str, filter: DbFilter) -> Result<u64, DbError>` | 一致する行を削除する。削除された行数を返す。 |
| `count` | `async fn count(&mut self, table: &str, filter: DbFilter) -> Result<i64, DbError>` | 一致する行数をカウントする。 |
| `last_insert_rowid` | `async fn last_insert_rowid(&mut self) -> Result<i64, DbError>` | *この接続で* 最後に挿入された行の `rowid` を返す。 |
| `ping` | `async fn ping(&mut self) -> Result<(), DbError>` | 接続のヘルスチェックを行う。 |
| `shutdown` | `async fn shutdown(&mut self) -> Result<(), DbError>` | DB サーバーの正常終了を要求し、その確認応答（`DbResponse::Ack`）を待つ — fire-and-forget の送信とは異なり、ここでの失敗（`DbError::ConnectionClosed` を含む）は呼び出し元から見えるようになる。 |

### 使用例

```rust,no_run
use ene_tool_db::{DbClient, DbSchema, DbTable, DbColumn, DbType, DbFilter, DbOrderBy, DbValue, Row};
use std::path::Path;

async fn run() -> Result<(), ene_tool_db::DbError> {
    let mut client = DbClient::connect_with_token(
        Path::new("/run/ene/db.sock"),
        &std::env::var("ENE_DB_AUTH_TOKEN").unwrap_or_default(),
    ).await?;

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

    // 検索（全カラム、最近訪問した順）
    let rows = client.select(
        "fs_visited",
        &[],  // SELECT * — None ではなく空スライス
        DbFilter::eq("path", "/home/user/notes.md"),
        vec![DbOrderBy::desc("visited_at")],
        Some(10),
    ).await?;

    // 削除
    let deleted = client.delete(
        "fs_visited",
        DbFilter::eq("id", rowid),
    ).await?;
    println!("{deleted} 行を削除しました");

    client.shutdown().await?;
    Ok(())
}
```

---

## `DbError`

```rust
pub enum DbError {
    Transport(#[from] std::io::Error),
    Server { code: DbErrorCode, message: String },
    UnexpectedResponse(String),
    ConnectionClosed,
    Auth { code: DbErrorCode, message: String },
}
```

| バリアント | 意味 |
|---|---|
| `Transport(std::io::Error)` | 低レベルのトランスポート失敗（書き込み／読み取り／シリアライズ／デシリアライズ）。`#[from] std::io::Error` を実装しているため、生の I/O に対する `?` が自動的に変換される。 |
| `Server { code, message }` | サーバーが（ハンドシェイク後の）通常のリクエストに対してアプリケーションレベルのエラーを返した — 例えば権限違反やスキーマ違反。 |
| `UnexpectedResponse(String)` | サーバーが送ったリクエストに対して構文的には正しいが意味的には誤ったレスポンスバリアントを返した（例：`Insert` リクエストに対して `Select` レスポンスが返ってきた）。データエラーではなく、クライアント/サーバー間のプロトコル不整合を示す。 |
| `ConnectionClosed` | IPC 接続が予期せず閉じられた（レスポンスの読み取り中に EOF）。回復するには [`reconnect`](#接続管理) を呼び出す。 |
| `Auth { code, message }` | サーバーが、初回の `connect_with_token` 時、または `reconnect` 時に提示された認証トークンを拒否した。呼び出し元が「トークンが古い／無効」と「クエリが拒否された」を区別できるよう、`Server` とは別のバリアントになっている。 |

### `DbErrorCode`

```rust
pub enum DbErrorCode {
    PermissionDenied,
    UnknownTable,
    UnknownColumn,
    TypeMismatch,
    InvalidFilter,
    Internal,
}
```

| バリアント | 意味 |
|---|---|
| `PermissionDenied` | ツールに要求されたリソースへのアクセス権限がない（例：宣言したプレフィックス外のテーブルへの読み書きを試みた、または不正な認証トークンを提示した）。 |
| `UnknownTable` | 指定されたテーブルが存在しない、または `declare_schema` で宣言されていない。 |
| `UnknownColumn` | 指定された列がテーブルに存在しない。 |
| `TypeMismatch` | 値の型が列の宣言された型と一致しない。 |
| `InvalidFilter` | フィルター式が無効（例：存在しない列を参照している）。 |
| `Internal` | サーバー内部エラーが発生した。 |

`DbErrorCode` は `Display` を実装し、`SCREAMING_SNAKE_CASE` としてレンダリングされます（例：`PermissionDenied` → `"PERMISSION_DENIED"`）。

---

## スキーマ型

### `DbSchema`

```rust
pub struct DbSchema {
    /// ツールの名前プレフィックスと一致する必要がある（例："fs_*" テーブルすべてに対して "fs"）。
    pub prefix: String,
    pub tables: Vec<DbTable>,
    pub indexes: Vec<DbIndex>,
}
```

### `DbTable`

```rust
pub struct DbTable {
    /// スキーマのプレフィックスに続けて `_` で始まる必要がある。
    pub name: String,
    pub columns: Vec<DbColumn>,
}
```

### `DbColumn`

```rust
pub struct DbColumn {
    pub name: String,
    /// 列の型。デフォルトは `DbType::Text`。
    pub ty: DbType,
    /// 列が NULL を許容するか。デフォルトは `false`。
    pub nullable: bool,
    /// この列が主キーかどうか。デフォルトは `false`。
    pub primary_key: bool,
    /// この列が自動増分するかどうか。デフォルトは `false`。
    pub auto_increment: bool,
    /// この列に UNIQUE 制約があるかどうか。デフォルトは `false`。
    pub unique: bool,
    /// デフォルト値（型付き）。デフォルトは `None`。
    pub default: Option<DbValue>,
}
```

`DbColumn` は `Default` を derive しているため、（上の例のように）`..Default::default()` を使った部分的な構築が慣用的です。

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

## 値の型

### `DbType`

```rust
pub enum DbType {
    Integer,
    Real,
    #[default]
    Text,
    Blob,
    Boolean,
}
```

| バリアント | 意味 |
|---|---|
| `Integer` | 64ビット符号付き整数。 |
| `Real` | 64ビット IEEE 浮動小数点数。 |
| `Text` | UTF-8 テキスト文字列。`DbColumn::ty` のデフォルト。 |
| `Blob` | バイナリ blob。 |
| `Boolean` | ブール値（`INTEGER` の `0`/`1` として保存される）。 |

### `DbValue`

```rust
pub enum DbValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Blob(Vec<u8>),
}
```

| メソッド | シグネチャ | 説明 |
|---|---|---|
| `as_i64` | `fn as_i64(&self) -> Option<i64>` | `Int` であれば値を返す。 |
| `as_str` | `fn as_str(&self) -> Option<&str>` | `Text` であれば値を返す。 |
| `as_bool` | `fn as_bool(&self) -> Option<bool>` | `Bool` であれば値を返す。 |
| `as_f64` | `fn as_f64(&self) -> Option<f64>` | `Float` であれば値を返す。 |
| `as_bytes` | `fn as_bytes(&self) -> Option<&[u8]>` | `Blob` であれば値を返す。 |

`DbValue` は `From<bool>`、`From<i32>`、`From<i64>`、`From<f64>`、`From<String>`、`From<&str>`、`From<Vec<u8>>`、`From<&[u8]>` を実装しているため、ほとんどの呼び出し元でバリアントを直接構築する必要はありません — 上記の例の `DbFilter::eq("path", "/home/user/notes.md")` 呼び出しを参照してください。`&str` は `DbFilter::eq` の `impl Into<DbValue>` バウンドを介して暗黙的に変換されます。

### `Row`

```rust
pub type Row = BTreeMap<String, DbValue>;
```

`Row` は列名から値への順序付きマップです。`BTreeMap` を使用することで、挿入・アップサート操作における列の順序が決定的になります。

---

## フィルターDSL

`DbFilter` は SQL 文字列を書かずに `WHERE` 句を構築するための再帰的な enum です。

```rust
pub enum DbFilter {
    Always,
    And(Vec<DbFilter>),
    Or(Vec<DbFilter>),
    Not(Box<DbFilter>),
    Eq  { column: String, value: DbValue },
    Ne  { column: String, value: DbValue },
    Lt  { column: String, value: DbValue },
    Le  { column: String, value: DbValue },
    Gt  { column: String, value: DbValue },
    Ge  { column: String, value: DbValue },
    In  { column: String, values: Vec<DbValue> },
    Like { column: String, pattern: String },
    IsNull    { column: String },
    IsNotNull { column: String },
}
```

### コンストラクタとコンビネータ

バリアントを手で構築するよりも、以下の関連関数を優先してください。これらは `impl Into<String>`／`impl Into<DbValue>` を取るため、通常の `&str` や数値リテラルを直接渡せます：

| メソッド | シグネチャ | 説明 |
|---|---|---|
| `eq` | `fn eq(column: impl Into<String>, value: impl Into<DbValue>) -> Self` | `column = value`。 |
| `ne` | `fn ne(column: impl Into<String>, value: impl Into<DbValue>) -> Self` | `column != value`。 |
| `lt` / `le` / `gt` / `ge` | `fn lt(column: impl Into<String>, value: impl Into<DbValue>) -> Self`（他も同様） | `<`、`<=`、`>`、`>=` 比較。 |
| `is_null` | `fn is_null(column: impl Into<String>) -> Self` | `column IS NULL`。 |
| `is_not_null` | `fn is_not_null(column: impl Into<String>) -> Self` | `column IS NOT NULL`。 |
| `and` | `fn and(self, other: DbFilter) -> Self` | 2つのフィルターを AND で結合する。どちらかの側にネストした `And` があれば（ネストさせるのではなく）フラット化する（`a.and(b).and(c)` は `And(vec![And(vec![a, b]), c])` ではなく、1つの `And(vec![a, b, c])` を生成する）。 |
| `or` | `fn or(self, other: DbFilter) -> Self` | 2つのフィルターを OR で結合する。`and` と同様にネストした `Or` をフラット化する。 |
| `columns_referenced` | `fn columns_referenced(&self) -> Vec<&str>` | フィルターツリー内のすべての参照列名を再帰的に収集する。既知の列セットに対してフィルターを送信前に検証するのに便利。 |

`In { column, values }` と `Like { column, pattern }` には専用のコンストラクタ関数がありません。構造体バリアントリテラルとして直接構築してください。

### フィルター例

```rust,no_run
use ene_tool_db::{DbFilter, DbValue};

// WHERE path LIKE '/home/%' AND visited_at > 1700000000
let filter = DbFilter::Like {
    column: "path".into(),
    pattern: "/home/%".into(),
}.and(DbFilter::gt("visited_at", 1_700_000_000_i64));

// WHERE id IS NOT NULL
let filter = DbFilter::is_not_null("id");

// WHERE status IN ('active', 'pending')
let filter = DbFilter::In {
    column: "status".into(),
    values: vec![DbValue::Text("active".into()), DbValue::Text("pending".into())],
};

// フィルターなし（すべての行を検索）
let filter = DbFilter::Always;
```

### `DbOrderBy`

```rust
pub struct DbOrderBy {
    pub column: String,
    pub direction: DbOrderDirection,  // Asc | Desc
}

impl DbOrderBy {
    pub fn asc(column: impl Into<String>) -> Self;
    pub fn desc(column: impl Into<String>) -> Self;
}
```

`DbClient::select` には `Vec<DbOrderBy>` を渡します。空の `vec![]` は明示的な並び順を指定しないことを意味します（それ以外の場合、行の順序は特に保証されません — 挿入順が保証される*わけではありません*）。

---

## プレフィックスアクセス制御

`ene-core` 内の `DbIpcServer` は、すべてのリクエストに対して次のルールを強制します：

> ツールは `<tool_prefix>_` で始まる名前のテーブルにのみアクセスできる。

ツールがプレフィックス外のテーブルを読み書きしようとすると、サーバーは `DbError::Server { code: DbErrorCode::PermissionDenied, .. }` を返します。

この分離は意図的なものです：ツールは互いに独立した信頼されないプロセスであり、互いの状態を読み取ったり破壊したりできてはなりません。

---

## 関連項目

- [`ene-tool-host`](./ene-tool-host.md) — ツールバイナリを起動し、`DbClient::connect_with_token` が使用するソケットパス／認証トークンを提供する
- [`ene-tool-proto`](./ene-tool-proto.md) — `DbClient` が使用する基盤の `IpcStream` トランスポート
- [`ene-tool-common`](./ene-tool-common.md) — `ene-tool-db` と並んでツールバイナリが利用できる共有ユーティリティ
