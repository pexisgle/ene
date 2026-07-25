# ツールシステム（IPC / ホスト）

各ツールは独立したプラグインバイナリプロセスとして動作し、ホストと IPC（Linux は Unix Domain Socket、Windows は Named Pipe）で通信します。全ツールプラグインは統合プラグインプロトコル v3（`ene-plugin-proto`）を使用します。

人間向けのアクション一覧は [ツールカタログ](../../guide/tools/overview.md) を参照してください。

## アーキテクチャ

```
PluginHostManager (バイナリ発見、起動、監視)
  ├── IpcPluginRegistry × N (IPC + 再起動)
  │   └── ene-plugin-proto プロトコル v3
  ├── McpToolRegistry × N (MCP サーバー)
  ├── ToolRegistry アダプタ (capabilities.tools → ToolRegistry)
  └── CompositeToolRegistry
       └── 重複はハードエラー (DuplicateToolName)

ToolRag (レジストリとは別、EneActor が所有)
  ├── EmbeddingProvider (クエリ + HyDE + リランキング)
  ├── MemoryStore (tool_embedding_index)
  └── 重み付きマルチフィールドコサイン類似度

DbIpcServer × N (ツール別 DB、Unix のみ)
  ├── ene-plugin-fs  → ene-db-fs.sock   (プレフィックス: fs_)
  ├── ene-plugin-utility → ene-db-utility.sock (プレフィックス: utility_)
  └── …
```

## 命名

全ツールは `<namespace>.<action>` 形式です。名前空間の表は [カタログ](../../guide/tools/overview.md) にあります。

## IPC プロトコル (`ene-plugin-proto`)

プラグイン IPC はプロトコル v3 を使用します。これはレガシーのツール IPC v2 を拡張し、ストリーミング LLM メッセージとリッチなハンドシェイクを追加したものです。ツールプラグインはプロトコルのツール関連サブセットを使用します。

ワイヤ形式: 4 バイト little-endian 長プレフィックス + JSON。最大メッセージサイズ 64 MB。プロトコルバージョン: `PLUGIN_IPC_PROTOCOL_VERSION = 3`（`crates/ene-plugin-proto/src/ipc.rs`）。

主要なツール関連メッセージ:

```rust
pub enum PluginIpcRequest {
    Handshake {
        version: u32,
        sandbox: SandboxConfigData,
        plugin_config: Option<Value>,
    },
    ListTools,
    ListRagProfiles,
    GetConfigSchema,
    CallTool { name: String, arguments: String, deferred: bool },
    SetCallContext { conversation_id: String, turn_id: String },
    ApprovePermission { request_id: String },
    AllowPattern { action: String, target_pattern: String },
    RevokePattern { action: String, target_pattern: String },
    Shutdown,
    Ping,
    PollDeferred { task_id: String },
    CancelDeferred { task_id: String },
    // ... ストリーミング LLM メッセージ (CreateChatStream, ChatCompletion, EmbedBatch)
}

pub enum PluginIpcResponse {
    HandshakeAck { version: u32, capabilities: PluginCapabilities },
    Ack,
    Tools { tools: Vec<ToolSpec> },
    RagProfiles { profiles: Vec<ToolRagProfile> },
    ConfigSchema { schema: Option<Value> },
    CallResult { result: Result<String, ToolError> },
    DeferredAccepted { task_id: String },
    DeferredStatus { task_id: String, status: DeferredStatus },
    Error { message: String },
    Pong,
    // ... ストリーミングレスポンス (StreamChunk, StreamEnd, StreamError)
}
```

## PluginHostManager

`ene-plugin-host` クレート。全プラグインプロセス（ツール、プロバイダ、MCP）を統括します。

| メソッド | 説明 |
|----------|------|
| `start(config: &EneConfig)` | ソケット dir 作成、有効なプラグインバイナリ起動、MCP サーバー接続、`PluginHostManager` を返す |
| `tool_registries()` | 全ツール対応プラグインと MCP サーバーを統合した `Arc<dyn ToolRegistry>` を返す |
| `shutdown()` | 全管理プロセスを正常終了 |

### バイナリ発見

プラグインバイナリは `builtin_plugins_dir()` と `user_plugins_dir()`（`ene-config` のパス参照）から探索されます。バイナリは `ene-plugin-{name}` の命名規則に従う必要があります。

### クラッシュ耐性

| 層 | 挙動 |
|----|------|
| プロセス監視 | 死亡検知 → 指数バックオフ再起動（最大 5: 500ms → 8s） |
| ハング検知 | 呼び出し失敗後に `Ping` probe も失敗したプラグインを unhealthy（生存だが無応答）と判定し再起動 |
| 接続 | 切断 → 指数バックオフ再接続（200ms 倍増、上限 10s、5 回）、Handshake 再送 |

### ヘルスチェックとサーキットブレーカー

定期的な liveness probe が固定間隔で全プラグインに ping します。死亡している、または probe 制限時間内に `Ping` へ応答できないプラグインは再起動され、その回復はヘルスイベントとして通知されます。

プラグインごとのサーキットブレーカーは、連続失敗の後にクールダウン窓の間呼び出しを一時停止し、その後 probe 呼び出しを許可します。呼び出しが成功するとブレーカーは閉じます。

ヘルスイベントは runtime の診断チャネルへ `DiagnosticEvent::ToolHealth` としてブリッジされ、安定した英語の `status` 契約を持ちます: `unhealthy`、`restarting`、`restarted`、`recovered`、`circuit_open`、`circuit_closed`、`disabled`。

## ToolAction / ToolProvider / ToolRegistry

英語版リファレンスと同契約です。トレイト定義の詳細は [英語版](../../../reference/tools/overview.md) またはクレートソースを参照。実装手順は [ツールを書く](../../guide/tools/write-a-tool.md)、ABI 通し解説は [SDK](sdk.md)。

## MCP

`McpToolRegistry` が Model Context Protocol サーバーに接続します（stdio / http）。MCP サーバーは `plugins.mcp_servers` で設定します（[設定](../configuration/settings.md#plugins--プラグインシステム)を参照）。

## カスタムツール登録

人間向け手順: [ツールを書く](../../guide/tools/write-a-tool.md)。ABI: [SDK ガイド](sdk.md)。

```json
{
  "plugins": {
    "list": {
      "my-tool": { "enable": true }
    }
  }
}
```
