# IPC プロトコルおよびサンドボックス制約仕様 (`ene-plugin-proto`)

`ene-plugin-proto` クレートは、ホストメインプロセスランタイムと外部独立ツールプロセス間で交換されるバイナリシリアライズフレーム仕様、トランスポート処理、およびサンドボックスセキュリティの制限定義を提供します。

---

## 1. フレーム境界プロトコルおよびトランスポート (`transport.rs`)

すべてのソケットデータは、長さプレフィックスを付与した JSON 形式でシリアライズされ通信を行います。

#### `IpcStream::connect`
*   **シグネチャ**: `pub async fn connect(path: &Path) -> io::Result<Self>`
*   **説明**: 指定された IPC ソケットパス（Linux/macOS では Unix Domain Sockets、Windows では Named Pipes）への接続を確立します。

#### `poll_read` / `poll_write` / `poll_flush` / `poll_shutdown`
*   **説明**: `IpcStream` に対する非同期の基本的なポーリング型入出力処理を実装します。

#### `IpcListener::bind`
*   **シグネチャ**: `pub fn bind(path: &Path) -> io::Result<Self>`
*   **説明**: ソケット接続を待機するためのリスナーをバインドします。

#### `IpcListener::accept`
*   **シグネチャ**: `pub async fn accept(&mut self) -> io::Result<IpcStream>`
*   **説明**: ツールクライアントからの新規ソケット接続要求の到着を非同期待機します。

#### `cleanup_path`
*   **シグネチャ**: `pub fn cleanup_path(path: &Path)`
*   **説明**: 以前のクラッシュ等で残留した古いソケットファイルを破棄クレンジングします。

---

## 2. IPC メッセージの読み書き処理 (`ipc.rs`)

#### `read_ipc_request`
*   **シグネチャ**: `pub async fn read_ipc_request<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<Option<IpcRequest>, ToolError>`
*   **説明**: バイトストリームから 4-byte big-endian `u32` 長さヘッダーを読み取り、メッセージボディのバイトサイズが安全上限値（`64MB` の `MAX_MESSAGE_SIZE`）を超えていないか確認します。検証に合格した場合、後続の JSON メッセージを読み込んで `IpcRequest` にデシリアライズします。

#### `write_ipc_request`
*   **シグネチャ**: `pub async fn write_ipc_request<W: AsyncWriteExt + Unpin>(writer: &mut W, req: &IpcRequest) -> Result<(), ToolError>`
*   **説明**: `IpcRequest` を JSON 形式に変換し、バイトサイズヘッダーを付与してバイトストリームに書き出します。

#### `read_ipc_response` / `write_ipc_response`
*   **シグネチャ**: `pub async fn read_ipc_response<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<Option<IpcResponse>, ToolError>` (および書き込みメソッド)
*   **説明**: ツールから戻る `IpcResponse` データの読み込み、および書き出しを実行します。

#### `IpcConfig::new`
*   **Signature**: `pub fn new(initial_config: serde_json::Value) -> Self`
*   **Description**: 初期構成JSONデータをバインドした構成管理オブジェクトを作成します。

#### `IpcConfig::get` / `set`
*   **Signature**: `pub async fn get<T: serde::de::DeserializeOwned>(&self) -> Result<T, ToolError>` (およびセットメソッド)
*   **Description**: JSON 構成ブロックへのスレッド安全な読み書き手段を提供します。

---

## 3. ツールサーバーとディスパッチロジック (`server.rs`)

ツールプロセス側で動作するメッセージルーティング機構です。

#### `run_tool_server`
*   **シグネチャ**: `pub async fn run_tool_server(provider: Box<dyn ToolProvider>) -> Result<(), ToolError>`
*   **プロセス**:
    1.  環境変数からソケット接続先パスの情報を解決します。
    2.  ホストデータベースプロキシソケットへの接続を開きます。
    3.  メッセージ受信ループを開始し、UDS からのリクエストを `dispatch` に送ってパース実行し、完了応答フレームを書き戻します。

#### `dispatch`
*   **シグネチャ**: `async fn dispatch(provider: &dyn ToolProvider, req: &IpcRequest) -> IpcResponse`
*   **説明**: 受信した `IpcRequest` を評価し、プロバイダから定義スペック情報を取得して返却するか、または対象ツール関数の実行を呼び出し結果フレームへとマッピングします。

---

## 4. メタデータ仕様定義 (`types.rs`)

#### `ToolName::try_new`
*   **シグネチャ**: `pub fn try_new(name: impl Into<String>) -> Result<Self, String>`
*   **説明**: ツール識別子の妥当性チェックを行います。名前空間の命名スキームが `<namespace>.<action>` に合致していることを検証します。

#### `ToolName::namespace` / `action`
*   **Signature**: `pub fn namespace(&self) -> Option<&str>` (およびアクション抽出)
*   **Description**: ツール名から名前空間、またはアクション名の部分文字列を切り出して返します。

#### `ToolVersion::new`
*   **Signature**: `pub const fn new(major: u32, minor: u32, patch: u32) -> Self`
*   **Description**: バージョン定数オブジェクトを初期化します。

#### `KeywordSet::primary_only` / `with_secondary`
*   **Signature**: `pub fn primary_only(primary: impl IntoIterator<Item = impl Into<String>>) -> Self`
*   **Description**: ツール検索（Tool RAG）用の一致キーワードセット（プライマリ、セカンダリ）を設定します。

#### `ToolRagProfile::from_tool_spec`
*   **Signature**: `pub fn from_tool_spec(spec: &ToolSpec) -> Self`
*   **Description**: 指定スペック定義情報に基づいて、ベクトルインデックス化に必要な RAG 検索用テキストデータを生成します。

#### `ToolRagProfile::embedding_text`
*   **Signature**: `pub fn embedding_text(&self, field: EmbeddingField, parameters: Option<&serde_json::Value>, example_index: Option<usize>) -> String`
*   **Description**: ツールの説明文、スキーマ引数情報、および実行例（Examples）などをシリアライズし、インデックスベクトル生成用のプレーンテキストを構成します。

---

## 5. ホスト側プロバイダレジストリ (`host_registry.rs`)

#### `HostRegistry::new`
*   **Signature**: `pub fn new() -> Self`
*   **Description**: 空のプロバイダレジストリを作成します。

#### `HostRegistry::try_add_provider`
*   **Signature**: `pub fn try_add_provider(&mut self, provider: Box<dyn ToolProvider>) -> Result<(), ToolError>`
*   **Description**: 新たなツールプロバイダを登録します。すでに登録されているツール名と重複競合した場合は、`ToolError` を返します。

#### `HostRegistry::list_specs` / `list_rag_profiles`
*   **Signature**: `pub fn list_specs(&self) -> Vec<ToolSpec>`
*   **Description**: 登録されているすべてのツールの定義スペック一覧を返します。

#### `HostRegistry::call_tool`
*   **Signature**: `pub async fn call_tool(&self, name: &ToolName, arguments: &str) -> Result<String, ToolError>`
*   **Description**: ツール名に合致するプロバイダオブジェクトを特定し、パラメータ文字列を引き渡して非同期実行します。

---

## 6. サンドボックス制約データ (`SandboxConfigData`)

サードパーティ製ツールの実行権限やリソース限界値を制限します：

#### `SandboxConfigData::sanitize`
*   **シグネチャ**: `pub fn sanitize(&mut self)`
*   **説明**: セキュリティバイパスを防御するため、もし制限閾値が `0` などに改ざんされている場合は、ただちに事前定義されている安全なフォールバック制限（読み取り最大 50KB、書き込み最大 1MB など）にリセットしてクレンジングします。
