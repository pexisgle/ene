# `ene-tool-host`

> ツールプロセスのライフサイクル管理、IPC クライアント接続、および Tool RAG パイプライン。

`ene-tool-host` は `ene-core` とスタンドアロンのツールバイナリをつなぐブリッジです。主な責務は以下のとおりです：

1. ツールの子プロセスを**スポーンして監視**する。
2. IPC ハンドシェイクを**ネゴシエート**し、持続的な接続を維持する。
3. `CallTool` リクエストを**ルーティング**し、結果をコアアクターへ返す。
4. 各ターンに適切なツールのサブセットを選択する **Tool RAG パイプライン**を実行する。

関連ページ：ワイヤー型については [`ene-tool-proto`](ene-tool-proto.md)、ツール側 API については [`ene-tool-common`](ene-tool-common.md) を参照してください。

---

## `ToolRegistry` トレイト

中心となる抽象インターフェースです。ホストマネージャーと各個別レジストリの両方がこのトレイトを実装しており、コンポジションが可能です。

```rust
#[async_trait::async_trait]
pub trait ToolRegistry: Send + Sync {
    fn list_tools(&self) -> Vec<ToolSpec>;
    async fn call_tool(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<String, ToolError>;

    // 任意のフック — デフォルト実装は no-op
    async fn set_session_id(&self, _session_id: &str) {}
    async fn approve_permission(&self, _request_id: &str) {}
    async fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}
    async fn config_schema(&self) -> Option<serde_json::Value> { None }
}
```

### メソッド一覧

| メソッド | 説明 |
|---|---|
| `list_tools()` | このレジストリが公開するすべての [`ToolSpec`](ene-tool-proto.md#toolspec) のリストを返します。 |
| `call_tool(name, arguments)` | JSON エンコードされた引数文字列を指定されたツールにディスパッチします。ツールのテキスト出力、または [`ToolError`](ene-tool-proto.md#toolerror) を返します。 |
| `set_session_id(session_id)` | 現在のセッション ID をすべての接続済みツールプロセスに伝播し、状態のスコープを設定します。 |
| `approve_permission(request_id)` | 保留中のパーミッションリクエストを承認します（ツールが `ToolError::PermissionRequired` を返した場合に使用）。 |
| `allow_pattern(action, target_pattern)` | 指定アクションクラスに対して、サンドボックス許可リストへグロブ/正規表現パターンを追加します。 |
| `config_schema()` | このレジストリの設定 JSON スキーマを返します（存在する場合）。 |

---

## `ToolHostManager`

設定されたすべてのツールプロセスと MCP サーバーからコンポジットレジストリを構成するトップレベルのマネージャーです。

```rust
pub struct ToolHostManager { /* private */ }
```

### コンストラクタ

| メソッド | 説明 |
|---|---|
| `ToolHostManager::start(config: &EneConfig) -> Result<Self, ToolError>` | `config.tool` を読み込み、`enabled` になっているツールプロセスのみをスポーンします。`add_registry` で拡張可能な未起動のマネージャーを返します。 |
| `ToolHostManager::start_full(config: &EneConfig) -> Result<Arc<dyn ToolRegistry>, ToolError>` | 便利ラッパー：`start` を呼び出した後に `into_registry` を実行します。ほとんどのアプリケーションでこちらを使用してください。 |

### インスタンスメソッド

| メソッド | 説明 |
|---|---|
| `add_registry(&mut self, registry: Arc<dyn ToolRegistry>)` | 変換前に追加のレジストリ（例：カスタムのインプロセスレジストリや MCP サーバー）を追加します。 |
| `into_registry(self) -> Arc<dyn ToolRegistry>` | マネージャーを消費し、登録されたすべてのレジストリをラップした `CompositeToolRegistry` を返します。 |

### 使用例

```rust
use ene_tool_host::ToolHostManager;

let registry = ToolHostManager::start_full(&config).await?;
let tools = registry.list_tools();
println!("ツールを {} 個読み込みました", tools.len());

let result = registry.call_tool("fs.read_file", r#"{"path":"/tmp/foo.txt"}"#)?;
println!("{result}");
```

---

## `IpcToolRegistry`

単一のツールバイナリサブプロセスへの持続的な IPC 接続を管理します。

```rust
pub struct IpcToolRegistry { /* private */ }
```

### 接続ライフサイクル

```
プロセスをスポーン
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
CallTool / SetSessionId / … を受け付ける状態へ
```

接続が切断された場合、`IpcToolRegistry` は指数バックオフで**自動再接続**を試みます（`RECONNECT_BASE_DELAY_MS = 200`、`RECONNECT_MAX_DELAY_MS = 10_000`、`RECONNECT_MAX_RETRIES = 5`）：

| 試行回数 | 次回試行までの待機 |
|---|---|
| 1 回目 | 200 ms |
| 2 回目 | 400 ms |
| 3 回目 | 800 ms |
| 4 回目 | 1.6 s |
| 5 回目 | (あきらめて `ToolError::IpcClient` を返す) |

### 主要メソッド

| メソッド | 説明 |
|---|---|
| `refresh_tools(&self) -> Result<(), ToolError>` | `ListTools` を再送信し、内部の `ToolSpec` キャッシュを更新します。ホットリロード後に便利です。 |
| `socket_path(&self) -> &PathBuf` | このレジストリが接続している IPC ソケットパスを返します。 |
| `get_config_schema(&self) -> Option<serde_json::Value>` | `GetConfigSchema` を介してツールバイナリの設定スキーマを取得します。 |

---

## `SupervisedIpcRegistry`

**プロセスレベルの監視**機能で `IpcToolRegistry` をラップします。子プロセスがクラッシュした場合、`SupervisedIpcRegistry` が自動的に再起動します。

再起動間の待機は指数バックオフです（`BASE_DELAY_MS = 500`、`2^attempt` 倍、`MAX_DELAY_MS = 30_000` でクランプ）：

| 再起動 # | 次回再試行までの待機 |
|---|---|
| 1 回目 | 500 ms |
| 2 回目 | 1 s |
| 3 回目 | 2 s |
| 4 回目 | 4 s |
| 5 回目 | 8 s |
| 5 回超 | あきらめて `ToolError::ExecutionFailed` を返す |

再起動後の再接続 (`IpcToolRegistry` 内部) は**一定 50 ms 間隔、最大 50 回リトライ**を使用します（`tool_host_manager.rs` の `CONNECT_DELAY_MS = 50`、`CONNECT_RETRIES = 50`）。

`ToolHostManager` がスポーンするすべてのツールプロセスには、これが自動的に使用されます。

---

## `CompositeToolRegistry`

複数の `ToolRegistry` 実装を一つに結合します。

```rust
pub struct CompositeToolRegistry {
    registries: Vec<Arc<dyn ToolRegistry>>,
}
```

- `list_tools()` — すべての内部レジストリのツールリストを連結します。
- `call_tool(name, arguments)` — `list_tools()` に指定名のツールを含む最初のレジストリにディスパッチします。
- その他のメソッドはすべての内部レジストリに転送されます。

---

## `McpToolRegistry`

MCP（Model Context Protocol）クライアントを `ToolRegistry` としてラップするアダプターです。MCP 互換サーバーが公開するツールを Ene から呼び出せるようになります。

```rust
pub struct McpToolRegistry { /* private */ }
```

`ToolConfig::mcp_servers` で設定します。MCP トランスポート（stdio、SSE など）は内部で処理されます。

---

## Tool RAG パイプライン

利用可能なツール数が LLM のコンテキスト予算を超える場合、`ene-tool-host` は検索拡張生成（RAG）による選択ステップを実行し、現在のクエリに最も関連性の高いツールを選びます。

### `ToolRag`

```rust
pub struct ToolRag { /* private */ }
```

| メソッド | 説明 |
|---|---|
| `ensure_index(tools: &[ToolSpec]) -> Result<(), ...>` | 各ツールの BLAKE3 コンテンツハッシュを計算し、変更されたものを（再）インデックス化します。 |
| `select(query: &str) -> Vec<ToolSpec>` | `query` を埋め込みベクトルに変換し、`similarity_threshold` でフィルタリングした上位 K 件のツールを返します。 |

### `ToolRagOptions`

```rust
pub struct ToolRagOptions {
    /// ツールを含めるための最小コサイン類似度。
    pub similarity_threshold: f32,
    /// 返すツールの最大数。
    pub top_k: usize,
    /// HyDE（仮説ドキュメント埋め込み）を使用して検索精度を向上させる。
    pub use_hyde: bool,
    /// 初期検索後にクロスエンコーダーで結果を再ランク付けする。
    pub use_rerank: bool,
}
```

### `ToolRagStats`

観測可能性のために、選択されたツールと一緒に返されます：

```rust
pub struct ToolRagStats {
    /// 返されたツールの数。
    pub hits: usize,
    /// インデックス内の総ツール数。
    pub total: usize,
    /// 最良マッチのコサイン類似度。
    pub top_similarity: f32,
}
```

### バージョンハッシュ

```rust
pub fn compute_tool_version_hash(tool: &ToolSpec) -> String
```

ツールの `name`、`version`、`description`、`parameters`、`keywords` にわたる BLAKE3 ハッシュを計算します。このハッシュはベクターインデックスに保存され、変更された場合にツールの埋め込みが再計算されます。

---

## 設定型

これらの型は `assets/settings.json` の `[tool]` セクションを反映しており、`ene-config` を介して読み込まれます。

### `ToolConfig`

```rust
pub struct ToolConfig {
    /// ツールシステム全体の有効/無効フラグ。
    pub enabled: bool,
    /// 1 ターンあたりの最大 LLM ↔ ツール往復回数。
    pub max_rounds: u32,
    /// 呼び出しごとのタイムアウト（ミリ秒）。
    pub timeout_ms: u64,
    /// ツールごとの有効フラグと設定オーバーライド。
    pub list: HashMap<String, ToolEntry>,
    /// MCP サーバーの定義。
    pub mcp_servers: Vec<McpServerConfig>,
    /// Tool RAG の設定。
    pub rag: ToolRagConfig,
}
```

### `ToolEntry`

```rust
pub struct ToolEntry {
    /// この特定のツールが有効かどうか。
    pub enable: bool,
    /// ツールバイナリにその設定として渡される任意の JSON。
    pub config: serde_json::Value,
}
```

### `ToolRagConfig`

```rust
pub struct ToolRagConfig {
    pub enabled: bool,
    pub similarity_threshold: f32,
    pub top_k: usize,
    pub use_hyde: bool,
    pub use_rerank: bool,
}
```

### `FieldWeightsConfig` / `FieldWeights`

`ToolSpec` の複合埋め込みを計算する際のフィールドごとの重み付けを制御します。

```rust
pub struct FieldWeightsConfig {
    pub summary: f32,
    pub description: f32,
    pub keywords_primary: f32,
    pub keywords_secondary: f32,
    pub keywords_domain: f32,
    pub examples: f32,
}
```

---

## 関連ページ

- [`ene-tool-proto`](ene-tool-proto.md) — IPC ワイヤー型（`ToolSpec`、`IpcRequest`、`ToolError`）
- [`ene-tool-common`](ene-tool-common.md) — ツール側の `ToolAction` トレイト
- [`ene-tool-derive`](ene-tool-derive.md) — ツール作成者向けプロシージャルマクロ
- [ツールシステム概要](../tools/overview.md)
