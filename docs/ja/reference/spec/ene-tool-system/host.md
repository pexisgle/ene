# ツールプロセスマネージャーおよび MCP クライアント仕様 (`ene-tool-host`)

`ene-tool-host` クレートは、外部ツール子プロセスの起動・監視、IPC 通信用のチャネル確立、名前衝突のチェック、クラッシュ回復のための再接続ループ、および外部 MCP (Model Context Protocol) サーバーの接続統合を管理します。

---

## 1. ツールホスト構成設定およびバージョン管理

#### `deserialize_config`
*   **シグネチャ**: `pub fn deserialize_config<T>(&self) -> Result<T, serde_json::Error> where T: DeserializeOwned`
*   **説明**: ツール個別のカスタム JSON 設定ブロックを、構造化オブジェクトへとデシリアライズします。

#### `compute_tool_version_hash`
*   **シグネチャ**: `pub fn compute_tool_version_hash(tool: &ene_tool_proto::ToolSpec) -> String`
*   **説明**: ツールで定義されている引数の構成、キーワード、バージョン値から Blake3 ハッシュ値を算出し、定義変更が生じていないかを照合判定します。

---

## 2. プロセス監視機能 (`ToolHostManager`)

`ToolHostManager` は、`config.json` の `tools.list.<name>` 定義に沿って外部ツールのライフサイクルを監視・管理します。

#### `start`
*   **シグネチャ**: `pub async fn start(config: &EneConfig, mut db_tokens: std::collections::HashMap<String, String>) -> Result<Self, ToolHostError>`
*   **説明**: 構成設定情報を元に、登録されたツール群の初期化・起動を開始します。

#### `start_full`
*   **シグネチャ**: `pub async fn start_full(config: &EneConfig, db_tokens: std::collections::HashMap<String, String>) -> Result<Arc<dyn ToolRegistry>, ToolHostError>`
*   **説明**: すべての定義された外部ツールをロード・起動し、一元管理するスレッド安全な `CompositeToolRegistry` 参照ハンドラを返します。

#### `try_add_registry`
*   **シグネチャ**: `pub fn try_add_registry(&mut self, registry: Arc<dyn ToolRegistry>) -> Result<(), ToolHostError>`
*   **説明**: 管理リストに新たなツールレジストリを追加します。他のツール名と重複（競合）している場合は、`ToolHostError::DuplicateToolName` 例外を発行して起動を中断します。

#### `into_registry`
*   **シグネチャ**: `pub fn into_registry(self) -> Arc<dyn ToolRegistry>`
*   **説明**: 構築された統合型の `CompositeToolRegistry` オブジェクトを返します。

#### `start_tool`
*   **シグネチャ**: `async fn start_tool(name: &str, sandbox: &ene_tool_proto::SandboxConfigData, tool_config: Option<serde_json::Value>, timeout_ms: u64, db_token: Option<String>) -> Result<Arc<dyn ToolRegistry>, ToolHostError>`
*   **プロセス**:
    1.  `find_tool_binary` を実行して、該当ツールのバイナリ実体ファイルの絶対パスを取得します。
    2.  ツール固有のテンポラリ UDS チャネルソケットファイルを配置し、リスナーをバインドします。
    3.  環境変数パラメータ（DB ハンドシェイク情報 `ENE_DB_AUTH_TOKEN` やソケットパスなど）を設定します。
    4.  子プロセス（コマンド実行）を Spawn し起動します。
    5.  ツールからの初期ハンドシェイク接続の到着を指定時間待ちます。
    6.  UDS セッションが有効化された `IpcToolRegistry` を構築して返します。

#### `find_tool_binary`
*   **シグネチャ**: `fn find_tool_binary(name: &str) -> Option<PathBuf>`
*   **説明**: 内蔵のプリセットツール実行ファイルディレクトリ、およびユーザー設定のツール配置フォルダ配下から、ツール名に対応する実行ファイルを探索します。

#### `is_alive`
*   **シグネチャ**: `fn is_alive(&mut self) -> bool`
*   **説明**: 子プロセスのプロセスハンドルが現在正常にアクティブであるかを死活監視します。

#### `restart`
*   **シグネチャ**: `fn restart(&mut self) -> Result<(), ToolHostError>`
*   **説明**: ゾンビプロセス化した既存の子プロセスを強制終了（Kill）し、再度新規に子プロセスを立ち上げて再接続処理を実行します。

#### `delay_for_restart`
*   **シグネチャ**: `fn delay_for_restart(restart_count: usize) -> Duration`
*   **説明**: ツールのクラッシュに伴う自動再起動のバックオフ時間を算出します。指数バックオフを適用し、最大30秒のクールダウン間隔にクランプします。

---

## 3. IPC 通信レジストリ処理 (`ipc_registry.rs`)

外部プロセスとのメッセージングと接続確認を処理します。

#### `new` (for IpcToolRegistry)
*   **シグネチャ**: `pub async fn new(socket_path: PathBuf, sandbox: SandboxConfigData, tool_config: Option<serde_json::Value>, timeout_ms: u64) -> Result<Self, ToolHostError>`
*   **説明**: 対象の UDS ソケットと接続し、非同期メッセージ通信セッションを確立します。

#### `connect_with_retry`
*   **シグネチャ**: `pub(crate) async fn connect_with_retry(socket_path: &Path, sandbox: &ene_tool_proto::SandboxConfigData, tool_config: Option<serde_json::Value>, max_retries: u32, delay_ms: u64, timeout_ms: u64) -> Result<IpcToolRegistry, ToolError>` (および同名関数)
*   **説明**: ネットワークエラー等に対して指数バックオフ待機をかけながら、指定ソケットへの接続試行を繰り返します。

#### `do_request`
*   **シグネチャ**: `async fn do_request(&self, req: IpcRequest) -> Result<IpcResponse, ToolHostError>`
*   **説明**: `IpcRequest` をバイトシリアライズして UDS に送信し、結果フレームが返ってくるのを待ちます。

#### `do_refresh_tools_with_stream`
*   **シグネチャ**: `async fn do_refresh_tools_with_stream(&self, stream: &mut IpcStream) -> Result<(), ToolHostError>`
*   **説明**: ストリームに対して `ListTools` コマンドを送信し、外部プロセス側が提供可能な全ツールスペック情報を取得します。

#### `do_refresh_tools` / `refresh_tools`
*   **シグネチャ**: `pub async fn refresh_tools(&self) -> Result<(), ToolHostError>`
*   **説明**: キャッシュされているスキーマ情報を手動でリフレッシュして再ロードします。

#### `ensure_connected`
*   **シグネチャ**: `async fn ensure_connected(&self) -> Result<(), ToolHostError>`
*   **説明**: ソケットが切断状態になっているかをチェックし、切断されている場合は自動で回復接続プロセスをトリガーします。

---

## 4. Model Context Protocol (`McpToolRegistry`)

外部の標準 MCP サービスへのネイティブ接続を提供します。

#### `connect_stdio`
*   **シグネチャ**: `pub async fn connect_stdio(&self, name: &str, command: &str, args: &[&str]) -> Result<(), ToolHostError>`
*   **説明**: 外部の MCP サーバーコマンドプロセスを起動し、JSON-RPC 仕様に基づいて stdin/stdout 経由で機能定義の読み込みおよびツール呼び出しの転送を実行します。

---

## 5. 統合レジストリ処理 (`CompositeToolRegistry`)

#### `try_new` / `new`
*   **シグネチャ**: `pub fn try_new(registries: Vec<Arc<dyn ToolRegistry>>) -> Result<Self, ToolHostError>`
*   **説明**: 複数のレジストリソース（内蔵、IPC 子プロセス、MCP 接続）を一つのファサードに結合します。ツール名の重複衝突チェックを実行します。

#### `try_add_registry`
*   **シグネチャ**: `pub fn try_add_registry(&self, registry: Arc<dyn ToolRegistry>) -> Result<(), ToolHostError>`
*   **説明**: アプリケーションの実行中に動的にレジストリを追加します。

#### `call_tool`
*   **シグネチャ**: `async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolHostError>`
*   **説明**: ツール名（例: `fs.read`）のプレフィックスから対象のツールを所有するレジストリセグメントを特定し、引数を転送して非同期実行します。
