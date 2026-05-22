# IPC ツールシステム

各ツール種別は独立したバイナリプロセスとして動作し、`ene-ai-core` とは Unix Domain Socket 経由の IPC で通信する。

## アーキテクチャ

```
ToolHostManager (バイナリ発見・起動・監視)
  ├── ToolEntry × N (IPC)
  │   └── IpcToolRegistry (UDS 通信・再接続)
  │       └── ene-tool-proto protocol
  ├── extra_registries × N (MCP など)
  │   └── McpToolRegistry
  └── MemoryStore (Tool RAG)
```

## ToolHostManager

`tool_host_manager.rs` で実装。全ツールプロセスを統括する。

```rust
pub struct ToolHostManager {
    entries: Vec<ToolEntry>,
    extra_registries: Vec<Arc<dyn ToolRegistry>>,
    tool_index: HashMap<String, usize>,
    store: Option<Arc<MemoryStore>>,
}
```

| メソッド | 説明 |
|----------|------|
| `start(settings)` | ソケットディレクトリ作成、有効ツールバイナリを `start_tool()` で起動 |
| `add_registry(registry)` | MCP 等の追加レジストリを登録 |
| `with_store(store)` | MemoryStore をアタッチ（Tool RAG 用） |
| `into_registry()` | `Arc<dyn ToolRegistry>` に変換 |

ToolRegistry trait 実装:
- `list_tools()`: IPC ツール + extra_registries をマージ
- `list_relevant_tools()`: ツール埋め込みテーブルからコサイン類似度検索
- `call_tool()`: 該当エントリにディスパッチ。失敗時は `call_with_supervision()` でプロセス再起動＋リトライ
- `ensure_index_built()`: ツール定義のバージョンハッシュを比較し、変更があったものだけを再埋め込み

### バイナリ発見

`find_tool_binary(name)` が以下のパスを探索する:
1. `<exe_dir>/tools/`（ビルドイン）
2. `app_data_dir()/tools/`（ユーザープラグイン）

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
    stream: TokioMutex<Option<UnixStream>>,
    tools: Mutex<Vec<ToolDefinition>>,
}
```

| メソッド | 説明 |
|----------|------|
| `new(socket_path, sandbox)` | 接続 → Initialize → Ack → ListTools の順で初期化 |
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
