# ツールシステム IPC 分離 — 現状まとめ

このドキュメントは **現在の実装状態**に合わせて、ツール IPC 分離の構成を整理したもの。

## 1. 現在のクレート構成

```
crates/
├── ene-tool-proto/      # IPC プロトコル / ToolProvider trait / server helper
├── ene-tool-host/       # ToolHostManager / IpcToolRegistry / MCP
├── ene-tools/*          # 各ツールバイナリ（fs/web/app/browser/utility）
├── ene-core/            # ストリーム・プロンプト・セッション/記憶
apps/
├── ene-desktop/
└── ene-cli/
```

## 2. IPC プロトコル（ene-tool-proto）

```rust
pub enum IpcRequest {
    Initialize { sandbox: SandboxConfigData, tool_config: Option<serde_json::Value> },
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

- Unix: UDS（4バイト長 + JSON）
- Windows: Named Pipe

## 3. ToolHostManager（ene-tool-host）

`ToolHostManager::start(settings)` が設定を読み取り、ツールバイナリを起動する。

### バイナリ探索順
1. `builtin_tools_dir()`  
   - デバッグ: exe と同じディレクトリ  
   - リリース: `exe_dir/tools/`
2. `user_tools_dir()`（`app_data_dir()/tools/`）

### クラッシュ耐性
- ツールプロセス死亡時は指数バックオフで再起動（最大5回）
- IPC 接続断は再接続（指数バックオフ）

## 4. 設定（tools セクション）

`tools` は **マップ形式**で設定する。

```json
{
  "tools": {
    "tool_calling_enabled": true,
    "max_tool_call_rounds": 10,
    "tools": {
      "fs": { "enable": true },
      "web": { "enable": true },
      "browser": { "enable": true },
      "utility": { "enable": true },
      "app": { "enable": true },
      "my-tool": { "enable": true, "config": { "foo": "bar" } }
    }
  }
}
```

## 5. カスタムツールの登録

```rust
#[tokio::main]
async fn main() {
    ene_tool_proto::run_tool_server(Box::new(MyToolProvider)).await.unwrap();
}
```

ビルドしたバイナリを `~/.local/share/dev.pexisgle.ene/tools/` に配置し、  
`tools.tools` にエントリを追加すれば自動起動される。

## 6. ステータス

IPC 分離は **完了済み**。  
本ドキュメントは現行実装の構成に合わせて更新している。
