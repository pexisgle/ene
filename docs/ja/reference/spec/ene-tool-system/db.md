# ツールデータベースプロキシ仕様 (`ene-tool-db`)

`ene-tool-db` クレートは、ツールのデータベース CRUD 操作のためのプロキシクライアントと通信プロトコルを提供します。これにより、サンドボックス化された外部ツールプロセスが、ローカルソケット接続を介してホストメインプロセス側データベースとの間で安全にデータを読み書きできるようになります。

---

## 1. 接続およびセキュリティ検証ハンドシェイク

ツールは起動時に、環境変数として提供される `db_socket`（ソケットファイルパス）と `db_auth_token`（検証トークン）を読み取って接続を確立します：

### 1. IPC メッセージ仕様 (`DbRequest` / `DbResponse`)

#### `DbRequest` (ツール → DB サーバー)
*   **`Handshake`**: Blake3 認証トークンおよびツール名情報を送信し、セッション接続を検証します。
*   **`DeclareSchema`**: ツールが必要とするテーブル、カラム名、データ型、およびインデックスのメタデータ定義（`DbSchema`）を宣言・移行要求します。
*   **`Insert`**: レコード行データを挿入します。
*   **`Update`**: 指定した条件式（`DbFilter`）に合致するレコードを更新します。
*   **`Delete`**: 条件式に一致するレコードを削除します。
*   **`Select`**: フィルタ、ソート順、上限数、およびオフセットに基づいてレコード行をクエリします。

#### `DbResponse` (DB サーバー → ツール)
*   **`HandshakeAck`**: ハンドシェイクが確認され、接続が承認されたことを示します。
*   **`Success`**: 操作の成功を通知し、影響を受けた行数または生成されたレコード ID を返します。
*   **`Rows`**: 検索に成功した複数の行データ一覧を返します。
*   **`Error`**: 処理の失敗を通知し、詳細なエラーコード（`DbErrorCode`）と説明メッセージを返します。

---

## 2. テーブル名のプレフィックス分離 (セキュリティ境界)

ツールがコアデータベースシステムテーブル（`typed_memories` など）に不正にアクセスしたり、他の無関係なプラグインツールのテーブルを操作したりすることを防ぐために、プレフィックス分離を強制します：
*   **ルール**: ツールスキーマで宣言されるすべてのテーブル名は、ツールの名前空間プレフィックス（例: `todo_`）で開始されていなければなりません。
*   **検証**: DB プロキシサーバー（`DbIpcServer`）は、受信したすべての DML 要求（Insert, Select, Update, Delete）の対象テーブル名をスキャンし、ツールプレフィックスと不一致である場合は、SQL コマンドを一切実行せず即座に `DbErrorCode::PermissionDenied` を応答してクエリを強制却下します。

---

## 3. クライアント側接続およびクエリ操作メソッド (`client.rs`)

`DbClient` は、ツールプロセス側で動作するプロキシ API です。

#### `connect`
*   **シグネチャ**: `pub async fn connect(socket_path: &Path) -> Result<Self, DbError>`
*   **説明**: 環境変数 `ENE_TOOL_DB_AUTH_TOKEN` から自動ロードしたトークンを使用して、指定のソケットに対する UDS 接続を確立・初期化します。

#### `connect_with_token`
*   **シグネチャ**: `pub async fn connect_with_token(socket_path: &Path, token: &str) -> Result<Self, DbError>`
*   **説明**: 明示的な認証トークン文字列とソケットパスを指定して接続し、セキュリティハンドシェイクを実行します。

#### `socket_path`
*   **シグネチャ**: `pub fn socket_path(&self) -> &Path`
*   **説明**: 現在アクティブなデータベースソケットパスを取得します。

#### `reconnect`
*   **シグネチャ**: `pub async fn reconnect(&mut self) -> Result<(), DbError>`
*   **説明**: 現在のクローズされた接続をクリアし、新規にソケットを開き直してハンドシェイクプロセスを再実行します。

#### `send_request`
*   **シグネチャ**: `async fn send_request(&mut self, req: &DbRequest) -> Result<DbResponse, DbError>`
*   **説明**: `DbRequest` を JSON 形式でシリアライズし、長さプレフィックスヘッダーを付与してソケットに書き出し、応答が受信されるまで非同期でブロッキング待機します。

#### `check_error`
*   **シグネチャ**: `fn check_error(resp: DbResponse) -> Result<DbResponse, DbError>`
*   **説明**: DB サーバーから戻ってきたエラーフレーム情報を解析し、対応するクライアント例外へとマッピング変換します。

#### `declare_schema`
*   **シグネチャ**: `pub async fn declare_schema(&mut self, schema: DbSchema) -> Result<(Vec<String>, Vec<String>), DbError>`
*   **説明**: ツールが使用するスキーマ定義情報を送信します。移行（Migration）が成功して作成されたテーブル名リストとインデックス名リストを返します。

#### `insert`
*   **シグネチャ**: `pub async fn insert(&mut self, table: &str, row: Row) -> Result<i64, DbError>`
*   **説明**: ターゲットテーブルに1行のデータを挿入し、新規レコードの ID を返します。

#### `upsert`
*   **シグネチャ**: `pub async fn upsert(&mut self, table: &str, row: Row, conflict_columns: &[&str]) -> Result<i64, DbError>`
*   **説明**: 行を挿入します。ただし競合カラム（`conflict_columns`）の値がすでに存在する場合は、エラーとせず該当レコードの情報を更新します。

#### `select`
*   **シグネチャ**: `pub async fn select(&mut self, table: &str, columns: &[&str], filter: DbFilter, order_by: Vec<DbOrderBy>, limit: Option<u64>) -> Result<Vec<Row>, DbError>`
*   **説明**: 条件に合致するデータをクエリして取得します。

#### `update`
*   **シグネチャ**: `pub async fn update(&mut self, table: &str, set: BTreeMap<String, DbValue>, filter: DbFilter) -> Result<u64, DbError>`
*   **説明**: 条件に一致するレコードを更新し、更新された行数を返します。

#### `delete`
*   **シグネチャ**: `pub async fn delete(&mut self, table: &str, filter: DbFilter) -> Result<u64, DbError>`
*   **説明**: 条件に合致するレコードを削除し、削除された行数を返します。

#### `count`
*   **シグネチャ**: `pub async fn count(&mut self, table: &str, filter: DbFilter) -> Result<i64, DbError>`
*   **説明**: 指定フィルタに合致するレコードの行数を返します。

#### `last_insert_rowid`
*   **シグネチャ**: `pub async fn last_insert_rowid(&mut self) -> Result<i64, DbError>`
*   **説明**: この接続トランザクションにおいて最後に挿入されたレコード ID を取得します。

#### `ping`
*   **シグネチャ**: `pub async fn ping(&mut self) -> Result<(), DbError>`
*   **説明**: DB サーバーに対して Ping 要求を送信し、接続セッションの死活状態をチェックします。

#### `shutdown`
*   **シグネチャ**: `pub async fn shutdown(&mut self) -> Result<(), DbError>`
*   **説明**: 接続セッションを安全にクローズします。

---

## 4. AST ベースの SQL 自動生成

SQL インジェクション攻撃を防御するために、ツールクライアントは生の SQL テキストを直接送信することはできません。
*   **構造化**: 条件定義はカラム名、比較演算子（一致、類似等）、および値オブジェクトを AST 状に組み合わせた `DbFilter` 構造体として定義されます。
*   **パラメータバインド**: サーバー側は受け取ったオブジェクトを元にプリペアドステートメント（プレースホルダー）を構築し、値を安全にバインドします。また、テーブル名やカラム名自体も正規表現 `^[A-Za-z_][A-Za-z0-9_]*$` に準拠した安全な文字パターンのみを許可するように制約されています。
