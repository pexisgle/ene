# `ene-tool-host` — APIリファレンス

> **クレート:** `ene-tool-host`
> **役割:** ツールプロセスのライフサイクル、IPCクライアント管理、MCPサーバー接続、およびTool RAG選択パイプライン。

---

## 概要

`ene-tool-host` は `ene-core` とスタンドアロンのツールバイナリとの間の橋渡しを行います。担う責務は以下の通りです:

1. ツール子プロセスの**生成と監視** — クラッシュ時の自動再接続・再起動を含む。
2. IPCハンドシェイクの**交渉**と、永続的なUnixソケット接続の維持。
3. `call_tool` リクエストを適切なレジストリに**ルーティング**し、結果をコアアクターに返す。
4. **MCPサーバーへの接続**（stdioトランスポート）と、それらのツールを同じ `ToolRegistry` インターフェースで公開する。
5. **Tool RAGパイプラインの実行** — 全ツールリストがLLMのコンテキスト予算を超える場合に、各ターンに関連する部分集合を選択する。

参照: 配線型（`ToolSpec`、`IpcRequest`/`IpcResponse`、`ToolError`）については [`ene-tool-proto`](./ene-tool-proto.md)、ツール側APIについては [`ene-tool-common`](./ene-tool-common.md) を参照してください。

---

## `ToolRegistry` トレイト

中心となる抽象化。`IpcToolRegistry`、`McpToolRegistry`、`CompositeToolRegistry`、`ToolHostManager` はすべてこれを実装しており、合成可能になっています。

```rust
#[async_trait]
pub trait ToolRegistry: Send + Sync {
    /// 利用可能な全ツールのリストを返す。
    fn list_tools(&self) -> Vec<ToolSpec>;
    /// LLMから渡されたJSON引数で、名前を指定してツールを実行する。
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolHostError>;

    /// 現在のセッションIDを設定する（undo追跡、セッションスコープの状態管理に使用）。
    async fn set_session_id(&self, _session_id: &str) {}
    /// 保留中の破壊的操作の許可リクエストをIDで承認する。
    async fn approve_permission(&self, _request_id: &str) {}
    /// セッション全体の許可パターン（アクション + 対象グロブ）を追加する。
    async fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}
    /// settings.json のツール設定セクション用のJSON Schemaを返す。
    async fn config_schema(&self) -> Option<serde_json::Value> { None }
}
```

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `list_tools` | `fn list_tools(&self) -> Vec<ToolSpec>` | 同期メソッド — キャッシュされたツールリストを返す。 |
| `call_tool` | `async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolHostError>` | JSONエンコードされた引数を名前指定でツールへディスパッチする。ツールのテキスト出力、または `ToolHostError` を返す。 |
| `set_session_id` | `async fn set_session_id(&self, session_id: &str)` | デフォルトはno-op。現在のセッションIDを接続中のツールプロセスに伝播する。 |
| `approve_permission` | `async fn approve_permission(&self, request_id: &str)` | デフォルトはno-op。保留中の許可リクエストを承認する。 |
| `allow_pattern` | `async fn allow_pattern(&self, action: &str, target_pattern: &str)` | デフォルトはno-op。アクションクラスのサンドボックス許可リストにグロブパターンを追加する。 |
| `config_schema` | `async fn config_schema(&self) -> Option<serde_json::Value>` | デフォルトは `None`。このレジストリの設定用JSON Schemaを返す。 |

> **注記:** `call_tool` は `Result<String, ToolHostError>` を返します — このクレート独自のエラー型であり、`ene_tool_proto::ToolError` **ではありません**。`ToolHostError` はプロトコルレベルの `ToolError` をラップしています（下記の[エラー](#エラー-toolhosterror--eneToolhosterror) を参照）。

---

## `ToolHostManager`

設定済みの全ツールプロセスを生成し、設定済みのMCPサーバーへ接続した上で、それらを1つの合成レジストリに組み立てるトップレベルのマネージャーです。

```rust
pub struct ToolHostManager { /* private */ }
```

### コンストラクタ

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `start` | `pub async fn start(config: &EneConfig, db_tokens: HashMap<String, String>) -> Result<Self, ToolHostError>` | `config.tool` を読み取り、`enabled` な各ツールプロセスを（監視付き・自動再接続レジストリでラップして）生成し、設定済みのMCPサーバーへ接続する。`db_tokens` はツール名 → ツールごとのデータベース認証トークンをマッピングし、各ツールが生成される際にそのサンドボックス設定へ消費・転送される。`add_registry` で拡張可能な未開始のマネージャーを返す。 |
| `start_full` | `pub async fn start_full(config: &EneConfig, db_tokens: HashMap<String, String>) -> Result<Arc<dyn ToolRegistry>, ToolHostError>` | 便利なラッパー: `start` を呼び出した後 `into_registry` を呼ぶ。ほとんどのアプリケーションではこちらを使用する。 |

### インスタンスメソッド

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `add_registry` | `pub fn add_registry(&mut self, registry: Arc<dyn ToolRegistry>)` | 変換前に追加のレジストリ（例: カスタムのインプロセスレジストリ）を追加する。内部で `CompositeToolRegistry::add_registry` に委譲する。 |
| `into_registry` | `pub fn into_registry(self) -> Arc<dyn ToolRegistry>` | マネージャーを消費し、`Arc<dyn ToolRegistry>` として返す（自身がトレイトを実装し、内部の合成レジストリに委譲する）。 |

### 例

```rust,no_run
use ene_tool_host::ToolHostManager;
use std::collections::HashMap;

# async fn run(config: &ene_config::EneConfig) -> Result<(), Box<dyn std::error::Error>> {
let db_tokens: HashMap<String, String> = HashMap::new();
let registry = ToolHostManager::start_full(config, db_tokens).await?;

let tools = registry.list_tools();
println!("Loaded {} tools", tools.len());

let result = registry.call_tool("fs_read_file", r#"{"path":"/tmp/foo.txt"}"#).await?;
println!("{result}");
# Ok(())
# }
```

内部的には、生成された各ツールプロセスは単純な `IpcToolRegistry` ではなく、プライベートな `SupervisedIpcRegistry`（[後述](#内部プロセス監視)）でラップされます — これが `ToolHostManager` にクラッシュ時再起動の挙動を与えています。`SupervisedIpcRegistry` は公開APIの一部**ではありません**。

---

## `IpcToolRegistry`

単一のツールバイナリ子プロセスへのUnixドメインソケット経由の永続的なIPC接続を、自動再接続機能とともに管理します。

```rust
pub struct IpcToolRegistry { /* private */ }
```

### 接続のライフサイクル

```
プロセス生成
     │
     ▼
Handshake  { version: IPC_PROTOCOL_VERSION }
     │
     ▼
Initialize { sandbox, tool_config }
     │
     ▼
ListTools  → Vec<ToolSpec> をキャッシュ
     │
     ▼
CallTool / SetSessionId / … の準備完了
```

### コンストラクタとメソッド

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `new` | `pub async fn new(socket_path: PathBuf, sandbox: SandboxConfigData, tool_config: Option<serde_json::Value>, timeout_ms: u64) -> Result<Self, ToolHostError>` | ソケットに接続し、handshake/initialize/list-toolsの一連の流れを実行し、結果の `ToolSpec` をキャッシュする。`timeout_ms`（`ToolConfig::timeout_ms` から取得）は以降のすべてのリクエストに適用され、ハングしたツール呼び出しがホストを無期限にブロックしないようにする。 |
| `refresh_tools` | `pub async fn refresh_tools(&self) -> Result<(), ToolHostError>` | `ListTools` を再送し、内部キャッシュを更新する。ホットリロード後に有用。 |
| `socket_path` | `pub fn socket_path(&self) -> &PathBuf` | このレジストリが接続しているIPCソケットパスを返す。 |
| `get_config_schema` | `pub async fn get_config_schema(&self) -> Option<serde_json::Value>` | `GetConfigSchema` を介してツールバイナリの設定スキーマを取得する。 |

**公開の `connect` メソッドは存在しません** — 再接続は、`ToolRegistry` トレイト実装経由で行われるすべての呼び出しの内部で、透過的かつプライベートに処理されます（`connect_with_retry`、`ensure_connected`、`send_with_reconnect`）。

### 自動再接続

セッション中に接続が切れた場合、`IpcToolRegistry` は指数バックオフで再接続します:

| 試行回数 | 次の試行までの遅延 |
|---|---|
| 1 | 200 ms |
| 2 | 400 ms |
| 3 | 800 ms |
| 4 | 1.6 s |
| 5 | 断念 — `ToolHostError` を返す |

---

## 内部プロセス監視

> `SupervisedIpcRegistry` は `tool_host_manager.rs` の**プライベート**な実装詳細です（`pub` 修飾子なし）。意図的に文書化された公開APIの一部**ではなく**、`ToolHostManager::start` から返される不透明な `Arc<dyn ToolRegistry>` としてのみアクセス可能です。

これは `IpcToolRegistry` をプロセスレベルの監視でラップします: 子プロセスがクラッシュした場合、指数バックオフ（`BASE_DELAY_MS = 500`、試行ごとに倍増、`MAX_DELAY_MS = 30_000` で上限）で再起動され、その後ラップされた `IpcToolRegistry` は `ToolHostManager` 内部の定数リトライポリシー（`CONNECT_RETRIES = 50` 回、`CONNECT_DELAY_MS = 50` ms間隔）を使って再接続されます。

---

## `CompositeToolRegistry`

複数の `ToolRegistry` 実装を1つに集約し、ツール名による**O(1)**のディスパッチを実現します。

```rust
pub struct CompositeToolRegistry {
    state: RwLock<CompositeState>,
}

struct CompositeState {
    registries: Vec<Arc<dyn ToolRegistry>>,
    /// tool_name -> registries内のインデックスへのマッピング。new/add_registry で構築される。
    tool_index: HashMap<String, usize>,
}
```

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `new` | `pub fn new(registries: Vec<Arc<dyn ToolRegistry>>) -> Self` | 与えられたレジストリから順に合成レジストリと `tool_index` を構築する。 |
| `add_registry` | `pub fn add_registry(&self, registry: Arc<dyn ToolRegistry>)` | レジストリを追加し、そのツールをインデックス化する。`&self` を取る（内部の `RwLock` を使用）ため、合成レジストリが共有された後でも呼び出せる。 |

`call_tool` は、すべてのサブレジストリの `list_tools()` を走査する代わりに `tool_index.get(name)` を参照して所有レジストリをO(1)で見つけ、直接そこへディスパッチします — 名前がインデックスに存在しない場合は `ToolHostError::Protocol(ToolError::NotFound { .. })` を返します。複数のレジストリで同じツール名が重複する場合は、**最初**の登録が優先されます（`HashMap::entry(..).or_insert(idx)`）。

---

## MCP統合

### `McpServerConnection`

```rust
/// MCPサーバーへの接続を表す。
pub struct McpServerConnection {
    pub name: String,
    pub client: Arc<rmcp::Peer<rmcp::RoleClient>>,
    pub tools: Vec<ToolSpec>,
}
```

### `McpToolRegistry`

1つ以上のMCP（Model Context Protocol）クライアント接続を単一の `ToolRegistry` としてラップするアダプタで、Eneが任意のMCP互換サーバーが公開するツールを呼び出せるようにします。

```rust
#[derive(Default)]
pub struct McpToolRegistry {
    servers: Arc<RwLock<Vec<McpServerConnection>>>,
}
```

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `new` | `pub fn new() -> Self` | 空のレジストリを作成する（`Self::default()` と同等）。 |
| `connect_stdio` | `pub async fn connect_stdio(&self, name: &str, command: &str, args: &[&str]) -> Result<(), ToolHostError>` | `command` を子プロセスとして生成し、MCPのstdioトランスポート経由で接続し、そのツールを一覧取得して `name` の下にキャッシュする。内部の `rmcp` クライアントが返す文字列ベースのエラーは `ToolHostError::ExecutionFailed { message }` にラップされる（[APIリファクタリング計画](../../architecture/api-refactor-plan.md) の項目3を参照 — 以前は素の `Result<(), String>` を返していた）。 |

`ToolConfig::mcp_servers` 経由で設定される。`McpTransport::Http` は設定/スキーマ上は受け入れられますが、`ToolHostManager::start` ではまだ実装されていません（警告をログに記録してスキップされます）。

```rust
pub struct McpServerConfig {
    pub name: String,
    pub enabled: bool,
    pub transport: McpTransport,
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpTransport {
    Stdio { command: String, args: Vec<String> },
    Http { url: String },
}
```

---

## Tool RAGパイプライン

利用可能なツールの数がLLMのコンテキスト予算を超える場合、`ToolRag` は検索拡張生成（RAG）による選択ステップを実行します: 埋め込み → オプションのHyDE拡張 → 重み付き多フィールド類似度 → オプションのクロスエンコーダーによるリランク → 上位N件。

### `ToolRag`

```rust
pub struct ToolRag {
    embedder: Arc<dyn EmbeddingProvider>,
    store: Option<Arc<MemoryStore>>,
    opts: ToolRagOptions,
    specs: RwLock<HashMap<ToolName, ToolSpec>>,
    last_specs_hash: AtomicU64,
    cached_field_rows: RwLock<Vec<CachedFieldRow>>,
}
```

| メソッド | シグネチャ | 説明 |
|--------|-----------|-------------|
| `new` | `pub fn new(embedder: Arc<dyn EmbeddingProvider>, store: Option<Arc<MemoryStore>>, opts: ToolRagOptions) -> Self` | すでに解決済みの `ToolRagOptions` がある場合の直接コンストラクタ。 |
| `from_config` | `pub fn from_config(embedder: Arc<dyn EmbeddingProvider>, store: Option<Arc<MemoryStore>>, config: ToolRagConfig) -> Result<Self, ToolHostError>` | 設定向けの `ToolRagConfig` から `ToolRagOptions` を構築する（`forced` 用に `Vec<String>` → `Vec<ToolName>` へ変換）し、パイプラインを構築する。 |
| `ensure_index` | `pub async fn ensure_index(&self, specs: &[ToolSpec]) -> Result<(), EmbeddingError>` | 仕様セットに対してBLAKE3ハッシュを計算する。前回の呼び出しから変化がなければ高速なno-opになる。変化があれば、変更されたツールを（再）埋め込みし、フィールドごとのベクトルを保存する。 |
| `select` | `pub async fn select(&self, query: &str) -> Vec<ToolSpec>` | 内部で `query` を埋め込み、`select_with_embedding` に委譲する。 |
| `select_with_embedding` | `pub async fn select_with_embedding(&self, query: &str, query_embedding: &[f32]) -> Vec<ToolSpec>` | （`query_embedding` を使った）重み付きフィールド単位の類似度スコアリングに加え、オプションのHyDEブレンディングとリランクを実行し、`opts.min_similarity` を上回る上位 `opts.final_n` 件のツールを返す。`opts.forced` のツールは常に含まれる。 |
| `start_background_indexer` | `pub fn start_background_indexer(self: &Arc<Self>, specs: Vec<ToolSpec>)` | `ensure_index` を呼び出してキャッシュをウォームアップするバックグラウンドタスクを生成する。即座に処理を返す。 |
| `stats` | `pub async fn stats(&self) -> ToolRagStats` | 直前の `select`/`select_with_embedding` 呼び出しのスナップショット: ヒット数、インデックスサイズ、最高類似度。 |
| `opts` | `pub fn opts(&self) -> &ToolRagOptions` | 解決済みのオプションを返す。 |
| `has_store` | `pub fn has_store(&self) -> bool` | 裏付けとなる `MemoryStore` がアタッチされているかどうか（RAGは再起動を超えて埋め込みを永続化するために必要）。 |

### `ToolRagOptions`

```rust
#[derive(Debug, Clone)]
pub struct ToolRagOptions {
    pub enabled: bool,
    /// ベクトル検索から取得する上位候補数。
    pub top_k: usize,
    /// リランク後に返す最終的なツール数。
    pub final_n: usize,
    pub use_hyde: bool,
    pub use_rerank: bool,
    /// リランク時に考慮する候補数。
    pub rerank_candidates: usize,
    /// ツールを含めるための最小類似度スコア。
    pub min_similarity: f32,
    /// 起動時にバックグラウンドでインデックスをウォームアップするか。
    pub background_index_on_startup: bool,
    /// RAGスコアリングに関わらず常に含めるツール。
    pub forced: Vec<ToolName>,
    /// スコアリングに使用するフィールド単位の埋め込み重み。
    pub weights: FieldWeights,
}
```

### `FieldWeights`

各埋め込みフィールドがツールの関連性スコアにどの程度強く寄与するかを制御します。負の重み（例: `negative`）は、完全な除外ではなくソフトなペナルティとして働きます。

```rust
#[derive(Debug, Clone)]
pub struct FieldWeights {
    pub summary: f32,
    pub description: f32,
    pub example: f32,
    /// negative/望ましくない埋め込みの重み（マッチをペナルティ化する）。
    pub negative: f32,
    /// HyDE（仮説的文書埋め込み）の重み。
    pub hyde: f32,
    /// 最終スコアに対してHyDE類似度が寄与する割合。
    /// 残りは直接的なフィールド単位のコサイン類似度から寄与する。
    /// `0.0` はHyDEブレンディングを無効化し、`1.0` はHyDE類似度のみを使用する。
    pub hyde_blend: f32,
}
```

`FieldWeightsConfig`（`config.rs` 内）は `FieldWeights` のシリアライズ可能/スキーマ対応のカウンターパートで、同じフィールドを持ちます — `impl From<FieldWeightsConfig> for FieldWeights` によって相互に変換されます。フィールドを追加する場合は両方を同期させてください。

### `ToolRagStats`

```rust
#[derive(Debug, Clone, Default)]
pub struct ToolRagStats {
    /// 直前の `select` 呼び出しで返されたツール数。
    pub hits: usize,
    /// インデックス内の個別ツール総数。
    pub total: usize,
    /// 直前の呼び出しにおける最高一致のコサイン類似度。
    pub top_similarity: f32,
}
```

### `compute_tool_version_hash`

```rust
pub fn compute_tool_version_hash(tool: &ToolSpec) -> String
```

ツールの仕様が意味的に変化した際に、キャッシュされたツール埋め込みを無効化するために使われる安定したBLAKE3ハッシュを計算します。このハッシュは `tool.name`、`tool.version`、**`tool.display_name`**、**`tool.summary`**、`tool.description`、`tool.parameters`、および `keywords` の4つの階層（`primary`、`secondary`、`domain`、`negative`）すべてをカバーします。あるツールについてこのハッシュが変化すると（`ene-memory` の `list_tool_embedding_hashes` で追跡される）、`ToolRag::ensure_index` はそのツールを再埋め込みします。

---

## 設定型

これらは `ene-config` の `define_config!` マクロを通じてロードされる、`assets/settings.json` の `[tools]` セクションに対応します。

### `ToolConfig`

```rust
pub struct ToolConfig {
    pub enabled: bool = true,
    /// ターンごとの連続ツール呼び出しラウンドの最大数。
    pub max_rounds: usize = 10,
    pub timeout_ms: u64 = 60_000,
    pub list: HashMap<String, ToolEntry>,
    pub mcp_servers: Vec<McpServerConfig>,
    pub rag: ToolRagConfig,
}
```

### `ToolEntry`

```rust
pub struct ToolEntry {
    pub enable: bool,
    /// ツール固有の設定（親のJSONオブジェクトにフラット化される）。
    #[serde(flatten)]
    pub config: serde_json::Value,
}

impl ToolEntry {
    /// `config` をツール固有の設定構造体へ型安全に逆シリアライズする。
    pub fn deserialize_config<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error>;
}
```

### `ToolRagConfig`

```rust
pub struct ToolRagConfig {
    pub enabled: bool,
    pub top_k: usize,
    pub final_n: usize,
    pub use_hyde: bool,
    pub use_rerank: bool,
    pub rerank_candidates: usize,
    pub min_similarity: f32,
    pub background_index_on_startup: bool,
    pub forced: Vec<String>,
    pub weights: FieldWeightsConfig,
}
```

---

## エラー: `ToolHostError` / `EneToolHostError`

```rust
#[derive(Debug, Error)]
pub enum EneToolHostError {
    /// 基盤となるツールプロトコル（IPC）由来のエラー。
    #[error(transparent)]
    Protocol(#[from] ene_tool_proto::ToolError),
    /// 設定エラー（例: 無効なRAG設定）。
    #[error("Configuration error: {0}")]
    Config(String),
    /// ツールの生成やソケット管理中のI/Oエラー。
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// 実行失敗（例: ツールバイナリが見つからない、MCPクライアントの起動失敗）。
    #[error("Execution failed: {message}")]
    ExecutionFailed { message: String },
}

impl EneToolHostError {
    /// 指定したメッセージで `Protocol(ToolError::IpcClient { .. })` エラーを作成する。
    pub fn ipc(message: impl Into<String>) -> Self;
}

/// クレートの公開API全体で使われるエイリアス。
pub type ToolHostError = EneToolHostError;
```

---

## 関連項目

- [`ene-tool-proto`](./ene-tool-proto.md) — IPC配線型（`ToolSpec`、`IpcRequest`/`IpcResponse`、`ToolError`）
- [`ene-tool-common`](./ene-tool-common.md) — ツール側の `ToolAction` トレイトとツールバイナリ向けヘルパー
- [`ene-tool-derive`](./ene-tool-derive.md) — ツール作者向けのプロシージャルマクロ（`#[derive(ToolSpec)]`）
- [`ene-memory`](./ene-memory.md) — `ToolRag` の永続的な埋め込みインデックス（`tool_embedding_index` テーブル）を支える
- [`ene-core`](./ene-core.md) — `start_full` が返す `Arc<dyn ToolRegistry>` を所有し、アクターループからツール呼び出しを駆動する
- [ツールシステム概要](../tools/overview.md)
