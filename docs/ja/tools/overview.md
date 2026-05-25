# ツールシステム概要

各ツール種別は独立したバイナリプロセスとして動作し、IPC (Unix では UDS、Windows では Named Pipe) で core と通信します。

## アーキテクチャ

```
ToolHostManager (バイナリ発見、起動、監視)
  ├── SupervisedIpcRegistry × N (IPC + 再起動)
  │   └── IpcToolRegistry (再接続)
  │       └── ene-tool-proto プロトコル
  ├── extra_registries × N (MCP 等)
  │   └── McpToolRegistry
  └── MemoryStore (Tool RAG)
```

## IPC プロトコル (`ene-tool-proto`)

```rust
pub enum IpcRequest {
    Initialize { sandbox: SandboxConfigData, tool_config: Option<Value> },
    ListTools,
    CallTool { name: String, arguments: String },
    SetSessionId { session_id: String },
    Ping,
    Shutdown,
}

pub enum IpcResponse {
    Ack,
    Tools { tools: Vec<ToolDefinition> },
    CallResult { result: Result<String, ToolError> },
    Pong,
    Error { message: String },
}
```

ワイヤ形式: 4 バイトビッグエンディアン長さプレフィックス + JSON ペイロード。

## ToolHostManager

`ene-tool-host` クレート。全ツールプロセスを統括します。

| メソッド | 説明 |
|---------|------|
| `start(settings)` | ソケットディレクトリ作成、有効なツールバイナリを起動 |
| `add_registry(registry)` | 外部レジストリを登録 (例: MCP) |
| `with_store(store)` | Tool RAG 用に MemoryStore をアタッチ |
| `into_registry()` | `Arc<dyn ToolRegistry>` に変換 |

### バイナリ発見

`find_tool_binary(name)` の検索順序:
1. `builtin_tools_dir()` — debug: exe と同じディレクトリ、release: `exe_dir/tools/`
2. `user_tools_dir()` — `app_data_dir()/tools/`

### クラッシュ耐性

| レイヤー | 動作 |
|---------|------|
| ToolHostManager | プロセス死 → 指数バックオフ再起動 (最大 5 回、500ms → 30s) |
| IpcToolRegistry | 接続断 → 指数バックオフ再接続 (最大 5 回)、Initialize 再送 |

## ToolRegistry トレイト

```rust
#[async_trait]
pub trait ToolRegistry: Send + Sync {
    fn list_tools(&self) -> Vec<ToolDefinition>;
    fn list_relevant_tools(&self, query_emb: Option<&[f32]>, limit: usize) -> Vec<ToolDefinition>;
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>;
    async fn set_session_id(&self, session_id: &str) {}
    async fn config_schema(&self) -> Option<serde_json::Value> { None }
    async fn ensure_index_built(&self, embedder: &dyn EmbeddingProvider, store: Option<&MemoryStore>) -> Result<(), ToolError> { Ok(()) }
}
```

## CompositeToolRegistry

複数の `ToolRegistry` インスタンスを集約:

- **先勝ち** — 重複ツール名は最初の登録が優先
- **Tool RAG** — `ensure_tool_embeddings()` がバージョンハッシュを計算し、変更があったツールのみ `store.upsert_tool_embedding()` で再埋め込み
- **`list_relevant_tools()`** — 保存されたツール埋め込みのコサイン類似度フィルタリング、`tool_rag_always_include` のツールは常に含める

## MCP サポート

`McpToolRegistry` が Model Context Protocol サーバーに接続:

| メソッド | 説明 |
|---------|------|
| `connect_stdio(name, cmd, args)` | 子プロセス起動、rmcp 経由で接続 |
| `list_tools()` | 全サーバーのツール定義をマージ |
| `call_tool(name, args)` | 該当サーバーにディスパッチ |

## カスタムツール登録

1. `ene-tool-proto` の `ToolProvider` トレイトを実装
2. バイナリの `main()` で `run_tool_server()` を呼び出し
3. バイナリを `~/.local/share/dev.pexisgle.ene/tools/` に配置
4. `settings.json` の `tools.tools` にエントリを追加

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
