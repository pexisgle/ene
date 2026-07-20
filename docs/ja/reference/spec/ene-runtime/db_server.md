# ツールデータベースプロキシ仕様 (`DbIpcServer`)

`DbIpcServer` は、ホストのメインプロセスのランタイム内に存在し、サンドボックス化されたサードパーティ製ツールとホストの SQLite 接続プール (`MemoryStore`) 間の仲介役として機能します。

---

## 1. サーバーのライフサイクルと接続

#### `start`
*   **シグネチャ**: `pub fn start(store: Arc<MemoryStore>, socket_path: &Path) -> Result<Self, DbServerError>`
*   **プロセス**:
    1.  指定された UDS (Unix Domain Socket) パス上の既存のファイルをクリーンアップします。
    2.  `IpcListener` にバインドしてソケットをリッスンします。
    3.  バックグラウンドで接続の受け入れループを実行します。
    4.  UDS のファイルアクセス許可を設定し、同一マシンのユーザープロセスのみが接続できるようにセキュリティを保護します。

#### `shutdown`
*   **シグネチャ**: `pub async fn shutdown(&self)`
*   **説明**: ソケットリスナーをクローズし、現在接続されているすべてのクエリセッションを切断し、UDS のソケットファイルを削除します。

---

## 2. セキュリティとクエリセッション処理

接続が受け入れられると、サーバーはクライアントごとに独立したタスクを実行し、`DbSession` を確立します。

#### `run_session`
*   **シグネチャ**: `async fn run_session(store: Arc<MemoryStore>, stream: IpcStream) -> Result<(), DbServerError>`
*   **プロセス**:
    1.  **ハンドシェイクフェーズ**:
        クライアントから最初の `DbRequest::Handshake` フレームを読み取ります。提供された Blake3 `auth_token` と、ホストが該当するツールプロセスの起動時に生成したトークンを照合します。
    2.  認証に成功すると、応答として `DbResponse::HandshakeAck` を送り返します。認証に失敗した場合は `DbErrorCode::Unauthorized` を返してソケット接続を即座に切断します。
    3.  **メッセージループ**:
        ハンドシェイクに成功したセッションでは、ループに入り、長さプレフィックス付きの `DbRequest` を受信・処理し、対応する `DbResponse` を返します。

---

## 3. クエリ検証ルーチン

#### `validate_schema`
*   **シグネチャ**: `fn validate_schema(tool_name: &str, schema: &DbSchema) -> Result<(), DbServerError>`
*   **プロセス**:
    1.  スキーマで定義されているすべてのテーブル名を検査します。
    2.  テーブル名がツールの名前空間プレフィックス（例: `fs_`）で始まっていることを確認します。
    3.  プレフィックスが一致しない場合は `DbErrorCode::PermissionDenied` を返し、スキーマ定義の適用をブロックします。

#### `validate_query_tables`
*   **シグネチャ**: `fn validate_query_tables(tool_name: &str, table: &str) -> Result<(), DbServerError>`
*   **説明**: SQL クエリインジェクション攻撃を防御するため、テーブル名およびカラム名が正規表現 `^[A-Za-z_][A-Za-z0-9_]*$` に準拠しているか検証します。また、スキーマで割り当てられた名前空間のプレフィックスで始まっているか再確認します。

---

## 4. DDL（データ定義言語）とスキーマ移行

#### `apply_schema_migration`
*   **シグネチャ**: `async fn apply_schema_migration(store: &MemoryStore, schema: &DbSchema) -> Result<(), DbServerError>`
*   **プロセス**:
    1.  `validate_schema` を呼び出して安全性を検証します。
    2.  ツール用の独立したデータベーススキーマ情報を解析します。
    3.  `CREATE TABLE IF NOT EXISTS` ステートメントを構築してテーブルを作成します。
    4.  スキーマで定義されたすべてのインデックス（例: `CREATE INDEX IF NOT EXISTS ...`）を順次構築します。

---

## 5. DML（データ操作言語）ヘルパーメソッド

#### `execute_insert`
*   **シグネチャ**: `async fn execute_insert(store: &MemoryStore, tool_name: &str, table: &str, row: Row) -> Result<i64, DbServerError>`
*   **説明**: `validate_query_tables` を実行したのち、SQLite パラメータバインディングプレースホルダーを構築し、ツールデータベースに行データを安全に挿入して、自動生成されたレコード ID を返します。

#### `execute_select`
*   **シグネチャ**: `async fn execute_select(store: &MemoryStore, tool_name: &str, table: &str, columns: &[String], filter: DbFilter, order_by: Vec<DbOrderBy>, limit: Option<u64>) -> Result<Vec<Row>, DbServerError>`
*   **説明**: AST フィルタパラメータを検証して SQL 構文を生成し、データを検索してクエリ結果をツールに返します。

#### `execute_update`
*   **シグネチャ**: `async fn execute_update(store: &MemoryStore, tool_name: &str, table: &str, set: BTreeMap<String, DbValue>, filter: DbFilter) -> Result<u64, DbServerError>`
*   **説明**: 行の値と更新用の設定情報を適用し、更新されたレコード数を返します。

#### `execute_delete`
*   **シグネチャ**: `async fn execute_delete(store: &MemoryStore, tool_name: &str, table: &str, filter: DbFilter) -> Result<u64, DbServerError>`
*   **説明**: 指定された条件に一致するレコードをツール用のテーブルから安全に削除します。
