# ツールシステム IPC 分離 — 移行計画

## 概要

`ene-ai-core` の肥大化したツールシステム（`src/tools/`）を独立クレートに分離し、各ツールを個別プロセスとして IPC（Unix Domain Socket）経由で実行するアーキテクチャへ移行する。

**目的:**
- コンパイル時間の短縮（ツール変更時の再コンパイル範囲を限定）
- バイナリサイズの削減（`ene-app` が重い依存を引き込まない）
- 開発・変更の容易さ（各ツールクレートが独立）
- サンドボックス・Undo の統合安全モデルの実現
- ユーザーツール追加機能（`app_data_dir()/tools/` にバイナリを配置）

---

## 1. ターゲットアーキテクチャ

### 1.1 クレート構成

```
crates/
├── ene-tool-proto/                # 共有プロトコル・型定義（軽量SDK）
│   ├── Cargo.toml                 # deps: serde, serde_json, async-trait, tokio
│   └── src/
│       ├── lib.rs                 # ToolProvider trait, re-exports
│       ├── types.rs               # ToolDefinition, ToolCategory, ToolCallResult
│       ├── sandbox.rs             # SandboxConfigData（シリアライズ可能POD型）
│       ├── ipc.rs                 # IpcRequest/IpcResponse, UDSフレーミング
│       ├── error.rs               # ToolError
│       ├── server.rs              # run_tool_server() — バイナリエントリ用ヘルパー
│       └── registry.rs            # HostRegistry — ToolProvider集約
│
├── ene-tools/
│   ├── fs/                         # ファイルシステム + Shell + Sandbox（統合安全モデル）
│   │   ├── Cargo.toml             # [[bin]] ene-tools-fs
│   │   └── src/
│   │       ├── main.rs            # run_tool_server(Box::new(FsToolProvider::new()))
│   │       ├── lib.rs
│   │       ├── provider.rs        # FsToolProvider（ToolProvider実装）
│   │       ├── sandbox.rs         # Sandbox（アクセス制御 + 操作追跡の統合）
│   │       ├── permission.rs      # PermissionGate, DestructiveAction
│   │       ├── undo_manager.rs    # UndoManager, UndoEntry, UndoOperation
│   │       ├── ...                # read, write, edit, delete, patch, search, shell
│   │
│   ├── browser/                    # ブラウザ自動化
│   │   ├── Cargo.toml             # [[bin]] ene-tools-browser
│   │   └── src/main.rs + lib.rs
│   │
│   ├── app/                         # GUI自動化
│   │   ├── Cargo.toml             # [[bin]] ene-tools-app
│   │   └── src/main.rs + lib.rs
│   │
│   ├── web/                         # Web取得 + 検索
│   │   ├── Cargo.toml             # [[bin]] ene-tools-web
│   │   └── src/main.rs + lib.rs
│   │
│   └── utility/                     # 質問、Todo
│       ├── Cargo.toml             # [[bin]] ene-tools-utility
│       └── src/main.rs + lib.rs
│
├── ene-ai-core/                    # 軽量化コア
│   ├── Cargo.toml                  # ツール専用依存を削除済み
│   └── src/
│       ├── lib.rs
│       ├── tool_host_manager.rs    # ToolHostManager（バイナリ発見・spawn・IPC接続）
│       ├── ipc_client.rs           # IpcToolRegistry（ToolRegistry実装）
│       ├── composite.rs            # CompositeToolRegistry
│       ├── tool_factory.rs          # ToolRegistryBuilder（シンプル版）
│       ├── config.rs               # AiSettings（tools.enabled 追加）
│       ├── paths.rs                 # builtin_tools_dir(), user_tools_dir(), tool_socket_dir()
│       ├── ...                     # その他残す（embedding, memory, session, stream, etc.）
│
├── ene-app/
└── ene-cli/
```

**ユーザー追加ツールの配置先:**
```
~/. local/share/dev.pexisgle.Ene/tools/   (Linux/macOS)
%APPDATA%/dev.pexisgle.Ene/tools/          (Windows)
```

### 1.2 プロセスアーキテクチャ

```
┌─────────────────────────┐
│   ene-ai-core            │
│   (メインプロセス)        │
│                          │
│  • LLMストリーミング      │
│  • メモリ / RAG          │
│  • Tool選択(埋め込み)     │
│  • MCPクライアント        │
│  • ToolHostManager       │
│                          │
│  ToolHostManager:        │
│    spawn+IPC → ene-tools-fs.sock
│    spawn+IPC → ene-tools-web.sock
│    spawn+IPC → ene-tools-browser.sock
│    spawn+IPC → ene-tools-app.sock
│    spawn+IPC → ene-tools-utility.sock
│    spawn+IPC → my-custom-tool.sock  (ユーザー追加)
└─────────────────────────┘
         UDS (JSON)        ┌──────────────────┐
◄────────────────────────►│  各ツールバイナリ  │
                          │  (個別プロセス)    │
                          └──────────────────┘
```

各ツールバイナリは `ene-tool-proto::run_tool_server()` をエントリポイントとし、
環境変数 `ENE_TOOL_SOCKET` で指定されたソケットパスでリッスンする。

---

## 2. `ene-tool-proto` 設計

### 2.1 型定義

```rust
// src/types.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub category: Option<ToolCategory>,
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolCategory {
    Filesystem, Shell, Browser, App, WebSearch, Utility,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub tool_call_id: String,
    pub content: String,
}
```

### 2.2 サンドボックスデータ型

```rust
// src/sandbox.rs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxConfigData {
    pub enabled: bool,
    pub allowed_directories: Vec<String>,
    pub writable_directories: Vec<String>,
    pub blocked_commands: Vec<String>,
    pub max_read_bytes: usize,
    pub max_write_bytes: usize,
    pub shell_timeout_ms: u64,
    pub max_shell_output_bytes: usize,
    pub max_shell_output_lines: usize,
}
```

### 2.3 IPC プロトコル

```rust
// src/ipc.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcRequest {
    Initialize { sandbox: SandboxConfigData },
    ListTools,
    CallTool { name: String, arguments: String },
    SetSessionId { session_id: String },
    Ping,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcResponse {
    Ack,
    Tools { tools: Vec<ToolDefinition> },
    CallResult { result: Result<String, ToolError> },
    Pong,
    Error { message: String },
}
```

**トランスポート**: Unix Domain Socket（4バイト長前置き + JSONペイロード）

### 2.4 ToolProvider trait

```rust
// src/lib.rs
#[async_trait]
pub trait ToolProvider: Send + Sync {
    fn list_tools(&self) -> Vec<ToolDefinition>;
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>;
    fn set_session_id(&self, session_id: &str);
    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {}
}
```

### 2.5 サーバーヘルパー

```rust
// src/server.rs
pub async fn run_tool_server(provider: Box<dyn ToolProvider>) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

// 各ツールバイナリのエントリポイント:
// #[tokio::main]
// async fn main() {
//     ene_tool_proto::run_tool_server(Box::new(MyProvider::new())).await.unwrap();
// }
```

### 2.6 エラー型

```rust
// src/error.rs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolError {
    NotFound { tool_name: String },
    InvalidArguments { message: String },
    ExecutionFailed { message: String },
    SandboxViolation { message: String },
    PermissionDenied { message: String },
    IoError { message: String },
    Timeout { message: String },
    Internal { message: String },
    IpcTransport { message: String },
}
```

---

## 3. Sandbox + Undo 統合設計

### 3.1 統合コンセプト

Undo は Sandbox の安全モデルの一部として統合する。「許可する」+「記録して元に戻せるようにする」= Sandbox。

```rust
// ene-tools/fs/src/sandbox.rs
pub struct Sandbox {
    config: SandboxConfig,
    undo: UndoManager,
    session_id: Arc<Mutex<String>>,
}

impl Sandbox {
    // --- アクセス制御 ---
    pub fn check_readable(&self, path: &Path) -> Result<PathBuf, ToolError>;
    pub fn check_writable(&self, path: &Path) -> Result<PathBuf, ToolError>;
    pub fn check_command(&self, cmd: &str) -> Result<(), ToolError>;

    // --- 操作追跡（UndoManager統合） ---
    pub fn track_overwrite(&self, original_content: Option<Vec<u8>>, path: &Path);
    pub fn track_creation(&self, path: &Path);
    pub fn track_deletion(&self, original_content: Option<Vec<u8>>, path: &Path);
    pub fn track_patch(&self, operations: Vec<UndoOperation>);

    // --- Undo実行 ---
    pub async fn undo_last(&self) -> Result<String, ToolError>;
}
```

### 3.2 SandboxConfigData と Sandbox の分離

- **`SandboxConfigData`** (`ene-tool-proto`): `Serialize + Deserialize` 可能なデータ型のみ。core → ツールバイトナリ間のIPC通信に使用
- **`Sandbox`** (`ene-tools/fs`): `SandboxConfigData::into_config()` から構築。バリデーション + Undo統合

---

## 4. ToolHostManager 設計

`ene-ai-core` がツールバイナリの発見・起動・IPC接続を管理する。

```rust
// ene-ai-core/src/tool_host_manager.rs
pub struct ToolHostManager {
    processes: Vec<ToolProcess>,
    registries: Vec<Box<dyn ToolRegistry>>,
}

impl ToolHostManager {
    /// 設定に基づいてツールホストプロセスを起動し、IPC接続を確立する
    pub async fn start(settings: &AiSettings) -> Result<Self, String>;

    /// MCP ツールレジストリを追加する
    pub fn add_registry(&mut self, registry: Box<dyn ToolRegistry>);

    /// 管理下のプロセスとレジストリを統合して ToolRegistry を構築する
    pub fn into_registry(self) -> Arc<dyn ToolRegistry>;
}
```

### バイナリ発見順序

1. **ビルトインツール**: `builtin_tools_dir()` — 実行ファイルと同じディレクトリの `tools/` サブディレクトリ
   - 例: `/usr/bin/ene-app` → `/usr/bin/tools/ene-tools-fs`
2. **ユーザーツール**: `user_tools_dir()` — `app_data_dir()/tools/`
   - 例: `~/.local/share/dev.pexisgle.Ene/tools/my-custom-tool`
3. `settings.json` の `tools.enabled` に指定された名前にマッチする `ene-tools-{name}` バイナリのみを起動

### ユーザーがカスタムツールを作る方法

```rust
// Cargo.toml
// [dependencies]
// ene-tool-proto = { path = "..." }

// src/main.rs
#[tokio::main]
async fn main() {
    ene_tool_proto::run_tool_server(Box::new(MyToolProvider)).await;
}
```

ビルドしたバイナリを `~/.local/share/dev.pexisgle.Ene/tools/` に配置し、
`settings.json` の `tools.enabled` に名前を追加するだけで自動認識される。

---

## 5. 移行完了状態

### Phase 1: 基盤構築 ✅

- [x] `ene-tool-proto` クレート作成（型、IPC、trait、エラー）
- [x] ワークスペース `Cargo.toml` にメンバー追加
- [x] `IpcToolRegistry` 実装
- [x] エンドツーエンドテスト通過

### Phase 2: ツール移植 ✅

- [x] **2a** `ene-tools/web`
- [x] **2b** `ene-tools/utility`
- [x] **2c** `ene-tools/app`
- [x] **2d** `ene-tools/browser`
- [x] **2e** `ene-tools/fs`（Sandbox統合、FsToolProvider完全実装）

### Phase 3: クリーンアップ ✅

- [x] `ene-tools-host` 削除（集約ホストから個別バイナリ型に移行）
- [x] `BuiltinToolRegistry` 削除
- [x] `EneToolRegistry` 削除
- [x] `ene-ai-core/src/tools/` の legacy 実装（app, browser, filesystem, read, write, edit, delete, patch, search, shell, web）を削除
- [x] `ToolRegistryBuilder::with_builtin()`, `with_sandbox()`, `with_ipc()` 削除
- [x] `ene-ai-core/Cargo.toml` からツール専用依存を削除（chromiumoxide, scraper, htmd, xcap, enigo, active-win-pos-rs, ashpd, pipewire, walkdir, glob, diff, strsim, urlencoding, image, base64, arboard）
- [x] 各 `ene-tools/*` クレートに `[[bin]]` と `src/main.rs`（ene-tool-proto::run_tool_server）を追加
- [x] `ene-tool-proto` に `server.rs`（run_tool_server）と `registry.rs`（HostRegistry）を追加
- [x] `ToolProvider` trait に `set_sandbox()` を追加（デフォルト空実装）
- [x] 各ツール Provider に `set_sandbox()` を実装（FsToolProvider のみ中身あり）
- [x] `ene-ai-core/src/tool_host_manager.rs` 追加（ToolHostManager）
- [x] `ene-ai-core/src/paths.rs` に `builtin_tools_dir()`, `user_tools_dir()`, `tool_socket_dir()` 追加
- [x] `ene-ai-core/src/config.rs` に `AiToolSettings` 追加（enabled フィールド）
- [x] `ene-cli`/`ene-app` を `ToolHostManager` 使用に更新

### Phase 4: 最適化・安定化（未着手）

- [ ] **4.1** ホストプロセスのクラッシュ耐性
  - [ ] 自動再起動（指数バックオフ）
  - [ ] `IpcToolRegistry` の自動再接続
- [ ] **4.2** セキュリティ強化
  - [ ] UDS パーミッション設定（0600）
  - [ ] ホストプロセスの権限分離
- [ ] **4.3** パフォーマンス
  - [ ] IPC のベンチマーク測定
  - [ ] 必要に応じてバイナリプロトコル（MessagePack等）への移行検討
- [ ] **4.4** `docs/ai-core-flow.md` 更新（新しいアーキテクチャ反映）

---

## 6. 依存関係マッピング

### 6.1 各ツールクレートの外部依存

| クレート | 外部依存（ene-tool-proto以外） |
|----------|-------------------------------|
| `ene-tools/fs` | tokio, serde, serde_json, regex, walkdir, glob, diff, strsim, dashmap, uuid, chrono |
| `ene-tools/browser` | chromiumoxide, scraper, htmd, base64, dashmap, tokio, uuid, serde, serde_json, regex |
| `ene-tools/app` | enigo, xcap, active-win-pos-rs, ashpd, pipewire, base64, image, tokio, serde_json |
| `ene-tools/web` | reqwest, htmd, scraper, regex, urlencoding, serde_json, tokio |
| `ene-tools/utility` | dashmap, serde, serde_json, chrono |

### 6.2 core から削除された依存 ✅

| 依存 | 移行先 |
|------|--------|
| `chromiumoxide` | `ene-tools/browser` |
| `scraper` | `ene-tools/browser` |
| `htmd` | `ene-tools/browser`, `ene-tools/web` |
| `xcap` | `ene-tools/app` |
| `enigo` | `ene-tools/app` |
| `active-win-pos-rs` | `ene-tools/app` |
| `ashpd` | `ene-tools/app` |
| `pipewire` | `ene-tools/app` |
| `walkdir` | `ene-tools/fs` |
| `glob` | `ene-tools/fs` |
| `diff` | `ene-tools/fs` |
| `strsim` | `ene-tools/fs` |
| `urlencoding` | `ene-tools/web` |
| `image` | `ene-tools/app` |
| `base64` | `ene-tools/browser`, `ene-tools/app` |
| `arboard` | `ene-tools/app` |

### 6.3 core に残る依存

| 依存 | 用途 |
|------|------|
| `async-openai` | LLM API クライアント |
| `candle-core`, `candle-nn` | ローカル GGUF 埋め込み |
| `tokenizers` | トークナイゼーション |
| `hf-hub` | モデルダウンロード |
| `diesel`, `rusqlite`, `sqlite-vec` | メモリDB |
| `reqwest` | API呼び出し用 |
| `rmcp` | MCP クライアント |
| `dashmap` | UndoManager |
| `uuid` | PermissionRequest, 会話ID |
| `regex` | SandboxConfig |
| `ene-tool-proto` | IPC プロトコル |
| その他 core 専用依存 | |

---

## 7. リスクと対策

| リスク | 対策 |
|--------|------|
| IPC 通信のレイテンシ | ツール呼び出しは元々非同期で数秒〜数十分かかる操作。UDS のミリ秒レイテンシは無視可能 |
| ツールプロセスクラッシュ | 自動再起動（指数バックオフ）（Phase 4）。クラッシュ時は進行中のツール呼び出しがエラーとして返るのみ |
| セッションステートの同期 | `SetSessionId` IPC通知で同期。UDSは順序保証あり |
| 大きなファイル内容のIPC転送 | 4バイト長前置き + JSON で最大数百MB対応可能。将来的にバイナリプロトコルに切替可能 |
| ブラウザセッションのIPC越し管理 | `BrowserSessionStore` はツールバイナリ側に配置。セッションIDで識別 |
| ユーザー追加ツールのセキュリティ | `settings.json` の `tools.enabled` に明示的に指定されたもののみ起動。未知のバイナリは自動実行しない |