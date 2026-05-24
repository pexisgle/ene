# IPC ツールシステム

各ツール種別は独立したバイナリプロセスとして動作し、core とは IPC で通信する。
Unix では UDS、Windows では Named Pipe を使用する。

## アーキテクチャ

```
ToolHostManager (バイナリ発見・起動・監視)
  ├── SupervisedIpcRegistry × N (IPC + 再起動)
  │   └── IpcToolRegistry (再接続)
  │       └── ene-tool-proto protocol
  ├── extra_registries × N (MCP など)
  │   └── McpToolRegistry
  └── MemoryStore (Tool RAG)
```

## ToolHostManager

`ene-tool-host` クレートで実装。全ツールプロセスを統括する。

```rust
pub struct ToolHostManager {
    composite: Arc<CompositeToolRegistry>,
}
```

| メソッド | 説明 |
|----------|------|
| `start(settings)` | ソケットディレクトリ作成、有効ツールバイナリを起動 |
| `add_registry(registry)` | MCP 等の追加レジストリを登録 |
| `with_store(store)` | MemoryStore をアタッチ（Tool RAG 用） |
| `into_registry()` | `Arc<dyn ToolRegistry>` に変換 |

ToolRegistry trait 実装:
- `list_tools()`: IPC ツール + extra_registries をマージ
- `list_relevant_tools()`: ツール埋め込みテーブルからコサイン類似度検索
- `call_tool()`: 該当エントリにディスパッチ。ツールプロセスが死亡していれば再起動＋再接続を実行
- `ensure_index_built()`: ツール定義のバージョンハッシュを比較し、変更があったものだけを再埋め込み

### バイナリ発見

`find_tool_binary(name)` が以下のパスを探索する:
1. `builtin_tools_dir()`（デバッグ時は exe_dir、リリース時は exe_dir/tools）
2. `user_tools_dir()`（`app_data_dir()/tools/`）

### クラッシュ耐性

| レイヤー | 動作 |
|----------|------|
| ToolHostManager | プロセス死亡検知 → 指数バックオフ（最大5回、500ms〜30s）で再起動 |
| IpcToolRegistry | 接続断 → 指数バックオフ（最大5回）で再接続、再接続時に Initialize 再送 |

## IpcToolRegistry

`ipc_client.rs` で実装。各ツールバイナリと 1:1 の IPC 接続を管理。

```rust
pub struct IpcToolRegistry {
    socket_path: PathBuf,
    sandbox: SandboxConfigData,
    tool_config: Option<serde_json::Value>,
    stream: TokioMutex<Option<IpcStream>>,
    tools: Mutex<Vec<ToolDefinition>>,
}
```

| メソッド | 説明 |
|----------|------|
| `new(socket_path, sandbox, tool_config)` | 接続 → Initialize → Ack → ListTools の順で初期化 |
| `ensure_connected()` | 未接続時は再接続＋再初期化 |
| `send_with_reconnect(req)` | リクエスト送信、失敗時は再接続＋リトライ |

## CompositeToolRegistry

複数の ToolRegistry を統合するコンポジット実装。

```rust
pub struct CompositeToolRegistry {
    registries: Vec<Box<dyn ToolRegistry>>,
    tool_index: HashMap<String, usize>,
    store: Option<Arc<MemoryStore>>,
}
```

- ツール名の重複は最初の登録が優先される（first-wins）
- `ensure_tool_embeddings()`: 各ツールのバージョンハッシュを計算し、変更があったものだけ `store.upsert_tool_embedding()` で更新

## McpToolRegistry

`mcp_client.rs` で実装。MCP（Model Context Protocol）サーバーとの接続を管理。

| メソッド | 説明 |
|----------|------|
| `connect_stdio(name, command, args)` | 子プロセスを起動し、rmcp 経由で MCP 接続、ツール一覧を取得 |
| `list_tools()` | 全サーバーのツール定義をマージ |
| `call_tool(name, arguments)` | 該当サーバーを特定し MCP 経由で呼び出し |

## ToolRAG

`tool_embeddings` テーブルに保存されたツール埋め込みに基づき、ユーザー入力の埋め込みベクトルとのコサイン類似度で動的にツールを絞り込む。

```rust
fn list_relevant_tools(&self, query_embedding: Option<&[f32]>, limit: usize) -> Vec<ToolDefinition>
```

- `store.search_tools()` で類似度検索
- `tool_rag_always_include` に指定されたツールは常に追加
- メモリ未アタッチ時は全ツールを返却
