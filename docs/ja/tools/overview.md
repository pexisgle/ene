# ツールシステム概要

各ツール種別は独立したバイナリプロセスとして動作し、IPC (Unix では UDS、Windows では Named Pipe) で core と通信します。

## アーキテクチャ

```
ToolHostManager (バイナリ発見、起動、監視)
  ├── SupervisedIpcRegistry × N (IPC + 再起動)
  │   └── IpcToolRegistry (再接続)
  │       └── ene-tool-proto プロトコル (v1)
  ├── extra_registries × N (MCP 等)
  │   └── McpToolRegistry
  └── CompositeToolRegistry
       └── 先勝ち重複排除

ToolRag (レジストリとは別、EneActor が所有)
  ├── EmbeddingProvider (クエリ + HyDE + リランキング)
  ├── MemoryStore (tool_embedding_index)
  └── 重み付きマルチフィールドコサイン類似度

DbIpcServer × N (ツール別 DB、Unix のみ)
  ├── ene-tool-fs  → ene-db-fs.sock   (プレフィックス: fs_)
  ├── ene-tool-utility → ene-db-utility.sock (プレフィックス: utility_)
  └── …
```

## ツール命名規則

全ツールは名前空間付き名前を使用: `<namespace>.<action>`。

| 名前空間 | ツール | バイナリ |
|---------|-------|--------|
| `filesystem` | `read`, `write`, `edit`, `delete`, `glob`, `grep`, `patch` | `ene-tool-fs` |
| `shell` | `execute` | `ene-tool-fs` |
| `app` | `clipboard_read`, `clipboard_write`, `list_windows`, `focus_window`, `get_active_window`, `list_monitors`, `capture_window`, `type_text`, `press_key`, `key_combo`, `mouse_move`, `mouse_click`, `mouse_drag`, `mouse_scroll`, `screenshot` | `ene-tool-app` |
| `browser` | `navigate`, `click`, `type`, `wait`, `screenshot`, `get_content`, `scroll`, `close` | `ene-tool-browser` |
| `web` | `fetch`, `search` | `ene-tool-web` |
| `utility` | `question`, `todo_list`, `todo_add`, `todo_update`, `todo_complete`, `todo_delete`, `get_system_info`, `get_current_time`, `undo` | `ene-tool-utility` / `ene-tool-fs` |

## IPC プロトコル (`ene-tool-proto`)

```rust
pub enum IpcRequest {
    Handshake { version: u32 },
    Initialize { sandbox: SandboxConfigData, tool_config: Option<Value> },
    ListTools,
    ListActionSpecs,
    GetConfigSchema,
    CallTool { name: String, arguments: String },
    SetSessionId { session_id: String },
    ApprovePermission { request_id: String },
    AllowPattern { action: String, target_pattern: String },
    GetMyConfig,
    SetMyConfig(Value),
    Ping,
    Shutdown,
}

pub enum IpcResponse {
    HandshakeAck { version: u32 },
    Ack,
    Tools { tools: Vec<ToolSpec> },
    ActionSpecs { specs: Vec<ActionSpec> },
    ConfigSchema { schema: Option<Value> },
    CallResult { result: Result<String, ToolError> },
    MyConfig(Value),
    Pong,
    Error { message: String },
}
```

ワイヤ形式: 4 バイトリトルエンディアン長さプレフィックス + JSON ペイロード。
最大メッセージサイズ: 64 MB。プロトコルバージョン:
`IPC_PROTOCOL_VERSION = 1` (`crates/ene-tool-proto/src/ipc.rs` を参照)。

## ToolHostManager

`ene-tool-host` クレート。全ツールプロセスを統括します。

| メソッド | 説明 |
|---------|------|
| `start(config: &EneConfig)` | ソケットディレクトリ作成、有効なツールバイナリを起動、`ToolHostManager` を返す |
| `start_full(config: &EneConfig)` | `start()` + MCP サーバー接続 → `Arc<dyn ToolRegistry>` を返す（失敗時はフォールバック） |
| `add_registry(registry)` | 外部レジストリを登録 (例: MCP) |
| `into_registry()` | マネージャーを消費し `Arc<dyn ToolRegistry>` を返す |

> **注意:** 旧版ドキュメントにあった `with_store(store)` メソッドは
> 現在の `ToolHostManager` には存在しません。Tool RAG のワイヤリングは
> `EneActor::reconfigure` 内の `init_tool_rag(config, embedder, session)`
> が担当します (`crates/ene-runtime/src/handle.rs` を参照)。

### バイナリ発見

`find_tool_binary(name)` の検索順序:
1. `builtin_tools_dir()` — debug: exe と同じディレクトリ、release: `exe_dir/tools/`
2. `user_tools_dir()` — `app_data_dir()/tools/`

### クラッシュ耐性

| レイヤー | 動作 |
|---------|------|
| `SupervisedIpcRegistry` (プロセス) | プロセス死 → 指数バックオフ再起動 (最大 5 回: 500ms → 8s) |
| `IpcToolRegistry` (接続) | 接続断 → 指数バックオフ再接続 (ベース 200ms 倍増、上限 10s、5 リトライ)、Handshake + Initialize 再送 |
| `ToolHostManager::connect_with_retry` (初回) | 一定 50ms 間隔、50 リトライ (`CONNECT_RETRIES = 50`、`CONNECT_DELAY_MS = 50`) |

## ToolAction トレイト

`ene-tool-common` は、全ビルトインツールバイナリで使用されるアクションモジュールパターンの `ToolAction` トレイトを定義します:

```rust
#[async_trait]
pub trait ToolAction: Send + Sync {
    fn definition(&self) -> ToolSpec;
    fn tool_name(&self) -> &'static str;
    async fn execute(&self, arguments: &str) -> Result<String, ToolError>;
}
```

各ツールバイナリは共有状態を所有し、`tool_name()` で個別の `ToolAction` 実装にディスパッチするプロバイダ構造体を持ちます。

## ToolProvider トレイト

ツールバイナリ側で実装。構造化された `ToolSpec` 型を返します:

```rust
#[async_trait]
pub trait ToolProvider: Send + Sync {
    fn list_specs(&self) -> Vec<ToolSpec>;
    fn list_action_specs(&self) -> Vec<ActionSpec> { vec![] }
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>;
    fn set_session_id(&self, session_id: &str);
    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {}
    fn approve_permission(&self, _request_id: &str) {}
    fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}
    fn set_config(&self, _config: &serde_json::Value) {}
    fn config_schema(&self) -> Option<serde_json::Value> { None }
}
```

## ToolRegistry トレイト

ホスト側のツールアクセスインターフェース:

```rust
#[async_trait]
pub trait ToolRegistry: Send + Sync {
    fn list_tools(&self) -> Vec<ToolSpec>;
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>;
    async fn set_session_id(&self, _session_id: &str) {}
    async fn approve_permission(&self, _request_id: &str) {}
    async fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}
    async fn config_schema(&self) -> Option<serde_json::Value> { None }
}
```

Tool RAG はレジストリではなく、`EneActor` が所有する `ToolRag` 構造体が個別に処理します。

## CompositeToolRegistry

複数の `ToolRegistry` インスタンスを集約:

- **先勝ち** — 重複ツール名は最初の登録が優先
- `call_tool`, `set_session_id`, `approve_permission`, `allow_pattern` を正しいサブレジストリにディスパッチ

## MCP サポート

`McpToolRegistry` が Model Context Protocol サーバーに接続:

| メソッド | 説明 |
|---------|------|
| `connect_stdio(name, cmd, args)` | 子プロセス起動、rmcp 経由で接続 |
| `list_tools()` | 全サーバーのツール定義をマージ |
| `call_tool(name, args)` | 該当サーバーにディスパッチ |

## カスタムツール登録

1. `ene-tool-proto` の `ToolProvider` トレイトを実装
2. 引数構造体に `#[derive(ToolSpec)]` を使用して仕様を自動生成
3. バイナリの `main()` で `run_tool_server()` を呼び出し
4. バイナリを `~/.local/share/dev.pexisgle.ene/tools/` に配置
5. `settings.json` の `tools.tools` にエントリを追加

```json
{
  "tools": {
    "tools": {
      "my-tool": { "enable": true, "config": { "foo": "bar" } }
    }
  }
}
```

詳細は [SDK ガイド](sdk.md) を参照してください。