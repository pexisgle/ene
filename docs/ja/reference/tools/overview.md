# ツールシステム（IPC / ホスト）

各ツールは独立したバイナリプロセスとして動作し、ホストと IPC（Linux は Unix Domain Socket、Windows は Named Pipe）で通信します。

人間向けのアクション一覧は [ツールカタログ](../../guide/tools/overview.md) を参照してください。

## アーキテクチャ

```
ToolHostManager (バイナリ発見、起動、監視)
  ├── SupervisedIpcRegistry × N (IPC + 再起動)
  │   └── IpcToolRegistry (再接続)
  │       └── ene-tool-proto プロトコル
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

## 命名

全ツールは `<namespace>.<action>` 形式です。名前空間の表は [カタログ](../../guide/tools/overview.md) にあります。

## IPC プロトコル (`ene-tool-proto`)

```rust
pub enum IpcRequest {
    Handshake {
        version: u32,
        sandbox: SandboxConfigData,
        tool_config: Option<Value>,
    },
    ListTools,
    ListRagProfiles,
    GetConfigSchema,
    CallTool { name: String, arguments: String },
    SetCallContext { conversation_id: String, turn_id: String },
    ApprovePermission { request_id: String },
    AllowPattern { action: String, target_pattern: String },
    RevokePattern { action: String, target_pattern: String },
    Shutdown,
    Ping,
}

pub enum IpcResponse {
    HandshakeAck { version: u32 },
    Ack,
    Tools { tools: Vec<ToolSpec> },
    RagProfiles { profiles: Vec<ToolRagProfile> },
    ConfigSchema { schema: Option<Value> },
    CallResult { result: Result<String, ToolError> },
    Error { message: String },
    Pong,
}
```

ワイヤ形式: 4 バイト little-endian 長プレフィックス + JSON。最大メッセージサイズ 64 MB。プロトコルバージョン: `IPC_PROTOCOL_VERSION = 2`（`crates/ene-tool-proto/src/ipc.rs`）。v2 でホストのヘルスチェックに使う `Ping`/`Pong` liveness probe を追加（#238）。

## ToolHostManager

`ene-tool-host` クレート。全ツールプロセスを統括します。

| メソッド | 説明 |
|----------|------|
| `start(config: &EneConfig)` | ソケット dir 作成、有効なツールバイナリ起動、`ToolHostManager` を返す |
| `start_full(config: &EneConfig)` | `start()` + MCP 接続 → `Arc<dyn ToolRegistry>`（失敗時フォールバックあり） |
| `add_registry(registry)` | 外部レジストリ（例: MCP）を登録 |
| `into_registry()` | マネージャを消費し `Arc<dyn ToolRegistry>` を返す |

> **注:** 以前の草案にあった `with_store(store)` は `ToolHostManager` に存在しません。Tool RAG の配線は `EneActor::reconfigure` 経由の `init_tool_rag(config, embedder, session)` です（`crates/ene-runtime/src/handle.rs`）。

### バイナリ発見

`find_tool_binary(name)` の探索順:

1. `builtin_tools_dir()` — debug: 実行ファイルと同じ dir、release: `exe_dir/tools/`
2. `user_tools_dir()` — `app_data_dir()/tools/`

### クラッシュ耐性

| 層 | 挙動 |
|----|------|
| `SupervisedIpcRegistry`（プロセス） | 死亡検知 → 指数バックオフ再起動（最大 5: 500ms → 8s） |
| `SupervisedIpcRegistry`（ハング） | 呼び出し失敗後に `Ping` probe も失敗したツールを unhealthy（生存だが無応答）と判定し再起動（#238） |
| `IpcToolRegistry`（接続） | 切断 → 指数バックオフ再接続（200ms 倍増、上限 10s、5 回）、Handshake 再送 |
| `ToolHostManager::connect_with_retry`（初期） | 定数 50ms、50 回リトライ |

### ヘルスチェックとサーキットブレーカー (#238)

定期的な liveness probe が固定間隔（`tools.health_interval_ms`、既定 30 秒）で
全ツールに ping します。死亡している、または probe 制限時間内に `Ping` へ応答
できないツールは再起動され、その回復はヘルスイベントとして通知されます。

ツールごとのサーキットブレーカーは、連続失敗（`tools.circuit_failure_threshold`、
既定 5 回）の後にクールダウン窓（`tools.circuit_cooldown_ms`、既定 30 秒）の間
呼び出しを一時停止し、その後 probe 呼び出しを許可します。呼び出しが成功すると
ブレーカーは閉じます。

ヘルスイベントは runtime の診断チャネルへ `DiagnosticEvent::ToolHealth` として
ブリッジされ、安定した英語の `status` 契約を持ちます: `unhealthy`、`restarting`、
`restarted`、`recovered`、`circuit_open`、`circuit_closed`、`disabled`。

## ToolAction / ToolProvider / ToolRegistry

英語版リファレンスと同契約です。トレイト定義の詳細は [英語版](../../../reference/tools/overview.md) またはクレートソースを参照。実装手順は [ツールを書く](../../guide/tools/write-a-tool.md)、ABI 通し解説は [SDK](sdk.md)。

## MCP

`McpToolRegistry` が Model Context Protocol サーバーに接続します（stdio）。

## カスタムツール登録

人間向け手順: [ツールを書く](../../guide/tools/write-a-tool.md)。ABI: [SDK ガイド](sdk.md)。

```json
{
  "tools": {
    "tools": {
      "my-tool": { "enable": true, "config": { "foo": "bar" } }
    }
  }
}
```
