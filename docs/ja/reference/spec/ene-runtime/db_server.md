# `DbIpcServer` / ツールDBプロキシセキュリティ仕様

`DbIpcServer` は、各外部ツールプロセスに対してSQLiteデータベースへの安全なアクセスを提供するIPCソケットサーバーです。ツールバイナリが生の `memory.db` ファイルへのハンドルを直接保持したり、任意のSQL文を実行したりすることを防ぎ、セキュリティ境界を確立します。

---

## 1. サーバーデータ構造

### `DbServerError` (公開 / エラー列挙型)
DB IPCサーバーの実行およびリクエストのパース・検証時に発生しうるエラー。
*   `Io(std::io::Error)`: ソケット接続や読み書きのエラー。
*   `Json(serde_json::Error)`: JSON Lines メッセージのデシリアライズ失敗。
*   `Db(sea_orm::DbErr)`: SeaORM 経由でSQLiteを実行した際のエラー。
*   `PermissionDenied(String)`: 認証失敗、プレフィックス違反、またはシステムテーブルへのアクセス試行。
*   `UnknownTable(String)`: `DeclareSchema` で宣言されていないテーブルへのアクセス。
*   `UnknownColumn { table, column }`: 宣言されていないカラムへのアクセス。
*   `Internal(String)`: その他の内部サーバーエラー。

### `DbIpcServer` (公開 / サーバーインスタンス)
ツールごとに1台生成されるサーバー実体。
```rust
pub struct DbIpcServer {
    db: DatabaseConnection,
    socket_path: PathBuf,
    tool_name: String,
    prefix: String,
    auth_token: String,
}
```
*   `new(db: DatabaseConnection, socket_path: PathBuf, tool_name: String, prefix: String, auth_token: String) -> Self`: コンストラクタ。
*   `async fn run(self) -> Result<(), DbServerError>`:
    1.  Unix環境では、前回の実行で残った古いソケットファイルを `remove_file` でクリーンアップ。
    2.  `IpcListener::bind` により指定のソケットパス/パイプにバインド。
    3.  Unix環境では、ソケットファイルのパーミッションを `0o600`（所有ユーザーのみ接続可能）へ `chmod` してローカル特権昇格を防ぐ。
    4.  無限 accept ループを実行。一時的なリソース枯渇（EMFILE等）に対しては、ソケットタスクを終了せず、500msのバックオフを挟んでリトライ。
    5.  接続ごとに `tokio::spawn` して `handle_connection` タスクを起動。

---

## 2. 接続・認証・ライフサイクル制御

### `handle_connection`
```rust
async fn handle_connection(
    stream: IpcStream,
    db: DatabaseConnection,
    tool_name: String,
    prefix: String,
    auth_token: String,
) -> Result<(), DbServerError>
```
1.  **接続ローカル状態管理**:
    -   `last_rowid`: 直前の `Insert` で発行された `rowid` を一時保存する `Arc<Mutex<Option<i64>>>`。SeaORM のコネクションプールを使うため、接続間でSQLiteの `last_insert_rowid()` を直接呼び出すとスレッド競合が発生して異なるツールのrowidが返る恐れがあります。これを防ぐため、本ハンドラ単位（＝同一ソケット接続内）でメモリ上に保存します。
    -   `declared_tables`/`declared_columns`: ツールが宣言したスキーマ構造。
2.  **メッセージ長ヘッダーパース**:
    -   すべてのメッセージは 4バイトの小端エンディアン整数でメッセージ長を前置します。
    -   最大メッセージ長を `64MB` に制限し、悪意ある巨大データによるメモリ枯渇攻撃を防止します。
3.  **ハンドシェイク & 認証**:
    -   接続後、最初のメッセージは `DbRequest::Handshake { token }` である必要があります。
    -   受け取った `token` が起動時に生成された `auth_token` と完全一致しない場合、即座に接続を切断します。

---

## 3. セキュリティ防御機構 (Security Model)

### 1. 識別子の厳格検証 (`validate_identifier`)
テーブル名、カラム名、インデックス名など、SQLに動的埋め込みが必要となるすべての識別子（これらはプレースホルダによるパラメータ化ができないため）に対し、以下のバリデーションを実行します。
*   空文字、または64文字を超えるものは拒否。
*   先頭文字が `[A-Za-z_]`、それ以降が `[A-Za-z0-9_]` 以外の文字を含む場合は `PermissionDenied`。SQLインジェクションを完全に防御します。

### 2. プレフィックス制限 (Namespace Isolation)
ツールがアクセスできるテーブル名は、起動時に指定された `prefix`（例: `fs_`、`web_`）から始まるものに限定されます。
*   `DeclareSchema` でテーブルを作成する際、および `Select`/`Insert`/`Update` 等のクエリ発行時、テーブル名のプレフィックスが検証されます。
*   `sqlite_*` などのメタテーブルや、他のツールのテーブル、スキーマ管理用内部テーブル `__tool_schemas` へのアクセスはすべて拒否されます。

### 3. DDLの遮断
ツールは生の `CREATE TABLE` などのSQLを送信できません。
*   スキーマ定義は `DbRequest::DeclareSchema` APIに制限され、内部でバリデーションされた情報に基づき安全に `CREATE TABLE` 文が生成・発行されます。
*   テーブルの `DROP` や `ALTER`、インデックスの削除などはAPIとして提供されておらず、ツール側からデータベース構造を破壊することはできません。
