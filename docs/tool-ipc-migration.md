# ツールシステム IPC 分離 — 移行計画

## 概要

`ene-ai-core` の肥大化したツールシステム（`src/tools/`）を独立クレートに分離し、IPC（Unix Domain Socket）経由で実行するアーキテクチャへ移行する。

**目的:**
- コンパイル時間の短縮（ツール変更時の再コンパイル範囲を限定）
- バイナリサイズの削減（`ene-app` が重い依存を引き込まない）
- 開発・変更の容易さ（各ツールクレートが独立）
- サンドボックス・Undo の統合安全モデルの実現

---

## 1. ターゲットアーキテクチャ

### 1.1 クレート構成

```
crates/
├── ene-tool-proto/                # 共有プロトコル・型定義（軽量）
│   ├── Cargo.toml                 # deps: serde, serde_json, async-trait, tokio
│   └── src/
│       ├── lib.rs
│       ├── types.rs                # ToolDefinition, ToolCategory, ToolCallResult
│       ├── sandbox.rs              # SandboxConfigData（シリアライズ可能POD型）
│       └── ipc.rs                  # IpcRequest/IpcResponse, UDSフレーミング
│
├── ene-tools/
│   ├── fs/                         # ファイルシステム + Shell + Sandbox（統合安全モデル）
│   │   ├── Cargo.toml             # deps: tokio, serde, serde_json, regex, walkdir,
│   │   │                          #       glob, diff, strsim, dashmap, uuid, chrono,
│   │   │                          #       ene-tool-proto
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── provider.rs         # FsToolProvider（ToolProvider実装）
│   │       ├── sandbox.rs          # Sandbox（アクセス制御 + 操作追跡の統合）
│   │       ├── permission.rs       # PermissionGate, DestructiveAction
│   │       ├── undo.rs             # UndoManager, UndoEntry, UndoOperation
│   │       ├── filesystem.rs       # 統合ディスパッチャ
│   │       ├── read.rs
│   │       ├── write.rs
│   │       ├── edit/               # 9戦略モジュール群
│   │       │   ├── mod.rs
│   │       │   ├── simple.rs
│   │       │   ├── line_trimmed.rs
│   │       │   ├── block_anchor.rs
│   │       │   ├── whitespace_normalized.rs
│   │       │   ├── indentation_flexible.rs
│   │       │   ├── escape_normalized.rs
│   │       │   ├── trimmed_boundary.rs
│   │       │   ├── context_aware.rs
│   │       │   └── multi_occurrence.rs
│   │       ├── delete.rs
│   │       ├── patch/
│   │       │   ├── mod.rs
│   │       │   └── parser.rs
│   │       ├── search/
│   │       │   ├── mod.rs
│   │       │   ├── glob.rs
│   │       │   └── grep.rs
│   │       └── shell/
│   │           ├── mod.rs
│   │           └── platform.rs
│   │
│   ├── browser/                    # ブラウザ自動化
│   │   ├── Cargo.toml             # deps: chromiumoxide, scraper, htmd, base64,
│   │   │                          #       dashmap, tokio, uuid, serde, serde_json,
│   │   │                          #       ene-tool-proto
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── provider.rs         # BrowserToolProvider
│   │       ├── chrome.rs
│   │       ├── extract.rs
│   │       └── session.rs          # BrowserSessionStore
│   │
│   ├── app/                         # GUI自動化
│   │   ├── Cargo.toml             # deps: enigo, xcap, active-win-pos-rs, ashpd,
│   │   │                          #       pipewire, base64, image, tokio, serde_json,
│   │   │                          #       ene-tool-proto
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── provider.rs         # AppToolProvider
│   │       └── actions/
│   │           ├── mod.rs
│   │           └── portal.rs
│   │
│   ├── web/                         # Web取得 + 検索
│   │   ├── Cargo.toml             # deps: reqwest, htmd, scraper, regex,
│   │   │                          #       urlencoding, serde_json, tokio,
│   │   │                          #       ene-tool-proto
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── provider.rs         # WebToolProvider
│   │       ├── webfetch.rs
│   │       ├── websearch.rs
│   │       ├── converter.rs
│   │       └── backends.rs
│   │
│   ├── utility/                     # 質問、Todo
│   │   ├── Cargo.toml             # deps: dashmap, serde, serde_json, chrono,
│   │   │                          #       ene-tool-proto
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── provider.rs         # UtilityToolProvider
│   │       ├── question.rs
│   │       ├── todo.rs             # + TodoStore, TodoItem
│   │       └── truncation.rs
│   │
│   └── host/                        # ツールホストプロセス（IPCサーバー）
│       ├── Cargo.toml              # deps: ene-tool-proto, ene-tools/{fs,browser,app,web,utility}
│       └── src/
│           ├── main.rs              # UDSサーバー起動、ToolProvider集約
│           └── registry.rs          # HostRegistry（全Providerを集約）
│
├── ene-ai-core/                    # 軽量化コア
│   ├── Cargo.toml                  # 重い依存をtool-hostへ移行後は大幅に軽量化
│   └── src/
│       ├── lib.rs
│       ├── ipc_client.rs           # IpcToolRegistry（ToolRegistry実装）
│       ├── builtin_registry.rs     # 段階的移行用（最終削除）
│       ├── composite.rs            # CompositeToolRegistry
│       ├── tool_factory.rs         # ToolRegistryBuilder（with_builtin, with_ipc）
│       ├── client.rs
│       ├── config.rs               # AiSandboxSettings → SandboxConfigData 変換追加
│       ├── embedding/              # 残す
│       ├── memory/                 # 残す
│       ├── session.rs
│       ├── stream.rs
│       ├── prompt_builder.rs
│       ├── mcp_client.rs
│       ├── ...                     # その他残す
│       └── (tools/ ディレクトリは段階的削除)
│
├── ene-app/
└── ene-cli/
```

### 1.2 プロセスアーキテクチャ

```
┌─────────────────────────┐         ┌──────────────────────────────┐
│   ene-ai-core           │         │  ene-tool-host                │
│   (メインプロセス)        │  UDS    │  (別プロセス)                  │
│                          │◄───────►│                              │
│  • LLMストリーミング      │  JSON   │  ToolProvider集約:            │
│  • メモリ / RAG          │         │    FsToolProvider             │
│  • Tool選択(埋め込み)     │         │    BrowserToolProvider        │
│  • MCPクライアント        │         │    AppToolProvider            │
│  • BuiltinTools(段階的)  │         │    WebToolProvider            │
│  • IpcToolRegistry       │         │    UtilityToolProvider        │
│                          │         │                              │
│                          │         │  Sandbox（undo統合済み）       │
└─────────────────────────┘         └──────────────────────────────┘
```

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
/// SandboxConfig のシリアライズ可能データ型
/// バリデーションロジックは含まず、ene-tools/fs::Sandbox で構築
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}
```

### 2.5 エラー型

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

- `call_tool` の戻り値および `IpcResponse::CallResult` は `Result<String, ToolError>` とする。
- IPC 越しにもシリアライズ可能で、各ツールクレート・core・host 間で統一的に使用する。
- `From<std::io::Error>` を実装し、I/O エラーの変換を簡潔にする。

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

    /// 読み取りパス検証
    pub fn check_readable(&self, path: &Path) -> Result<PathBuf, SandboxError>;

    /// 書き込みパス検証（将来的にUI承認フックを追加予定）
    pub fn check_writable(&self, path: &Path) -> Result<PathBuf, SandboxError>;

    /// コマンド検証
    pub fn check_command(&self, cmd: &str) -> Result<(), SandboxError>;

    // --- 操作追跡（UndoManager統合） ---

    /// 既存ファイルの上書きを記録
    pub fn track_overwrite(&self, original_content: Vec<u8>, path: &Path) {
        self.undo.push_restore_file(&self.session_id(), path, original_content);
    }

    /// 新規ファイル作成を記録
    pub fn track_creation(&self, path: &Path) {
        self.undo.push_delete_created_file(&self.session_id(), path);
    }

    /// ファイル削除を記録
    pub fn track_deletion(&self, original_content: Vec<u8>, path: &Path) {
        self.undo.push_restore_file(&self.session_id(), path, original_content);
    }

    /// パッチ適用を記録（アトミック: 複数ファイル操作を1つのUndoエントリに）
    pub fn track_patch(&self, operations: Vec<UndoOperation>) {
        self.undo.push_entry(&self.session_id(), operations);
    }

    // --- Undo実行 ---

    /// 直前の操作を取り消し
    pub async fn undo_last(&self) -> Result<String, SandboxError>;
}
```

**呼び出し側の比較:**

```rust
// Before（分離）:
let path = sandbox.resolve_and_check(path, true)?;
undo_manager.push_restore_file(session_id, path, original);

// After（統合）:
let path = sandbox.check_writable(&path)?;
sandbox.track_overwrite(original, &path);
// ツール作業者は Sandbox だけを意識すればよい
```

### 3.2 SandboxConfigData と Sandbox の分離

- **`SandboxConfigData`** (`ene-tool-proto`): `Serialize + Deserialize` 可能なデータ型のみ。core → host 間のIPC通信に使用
- **`Sandbox`** (`ene-tools/fs`): `SandboxConfigData::into_config()` から構築。バリデーション + Undo統合

---

## 4. 移行戦略

### 基本方針

1. **BuiltinToolRegistry** は `ene-ai-core` に残存させ、IPC版と並行稼働可能にする
2. 各ツールクレートを1つずつIPC版に移植し、動作確認後にbuiltin版を削除
3. 全ツールのIPC移植完了後、BuiltinToolRegistryシステム全体を削除

### 移行完了条件

各フェーズは以下を満たしてから次へ進む:

- [x] 新クレートのコンパイルが通る
- [x] `cargo test` が通る（IPC 統合テスト通過）
- [x] IPC経由でツールが正しく動作する（手動テスト: get_current_time, get_system_info, question, todo, webfetch, websearch, app, browser 確認済み）
- [ ] 既存のbuiltin版と同じ結果が返る（fs/shell/undo は stub、完全実装後に検証）

---

## 5. フェーズ別 TODO

### Phase 1: 基盤構築

> 目標: IPCプロトコル・クレート構造・ホストプロセスの基盤を構築し、エンドツーエンドで1つのツールが動くことを確認する

- [x] **1.1** `ene-tool-proto` クレート作成
  - [x] `Cargo.toml` 作成（deps: serde, serde_json, async-trait, tokio）
  - [x] `src/lib.rs` — `ToolProvider` trait 定義
  - [x] `src/types.rs` — `ToolDefinition`, `ToolCategory`, `ToolCallResult`
  - [x] `src/sandbox.rs` — `SandboxConfigData`
  - [x] `src/ipc.rs` — `IpcRequest`/`IpcResponse`, UDS フレーミング（4バイト長前置き + JSON）
  - [x] `src/error.rs` — 構造化 `ToolError` enum
  - [x] `cargo check` 通す

- [x] **1.2** ワークスペース `Cargo.toml` に新メンバー追加
  - [x] `crates/ene-tool-proto`
  - [x] `crates/ene-tools/host`
  - [x] `crates/ene-tools/utility`
  - [ ] `crates/ene-tools/fs` （Phase 2e で作成）
  - [ ] `crates/ene-tools/browser` （Phase 2d で作成）
  - [ ] `crates/ene-tools/app` （Phase 2c で作成）
  - [ ] `crates/ene-tools/web` （Phase 2a で作成）

- [x] **1.3** `ene-tools/host` — UDSサーバー・ディスパッチ実装
  - [x] `HostRegistry` — 複数 `ToolProvider` を集約
  - [x] `main.rs` — UDSリスン、リクエストディスパッチループ
  - [x] `IpcRequest::Initialize` で `SandboxConfigData` 受信
  - [x] `IpcRequest::Shutdown` でグレースフルシャットダウン

- [x] **1.4** `ene-ai-core` — `IpcToolRegistry` 実装
  - [x] `src/ipc_client.rs` — UDS接続クライアント、`ToolRegistry` trait 実装
  - [x] `src/tool_factory.rs` — `with_ipc(socket_path, sandbox_settings)` メソッド追加
  - [x] `config.rs` — `AiSandboxSettings` に `to_sandbox_config_data()` 追加
  - [ ] ホストプロセスの spawn&管理（起動・再起動・クラッシュ検知）→ Phase 4

- [x] **1.5** エンドツーエンドテスト
  - [x] `get_current_time` / `get_system_info` を IPC 経由で動作確認（モック + 実 host 両方）
  - [x] `ene-ai-core/tests/ipc_integration.rs` に自動テスト追加
  - [ ] builtin版とIPC版が並行稼働することを確認（Phase 2以降）
  - [ ] `ene-cli` で `--ipc` フラグ等で切り替え可能に（Phase 2以降）

### Phase 2: ツール移植（クレートごと）

> 目標: 各ツールクレートを1つずつIPC版に移植し、builtin版から削除

#### 2a: `ene-tools/web` （最もステートレスで独立）

- [x] **2a.1** クレート作成・コード移行
  - [x] `Cargo.toml` 作成
  - [x] `webfetch.rs` — `ene-ai-core/src/tools/web/webfetch/` から移行
  - [x] `websearch.rs` + `backends.rs` — `websearch/` から移行
  - [x] `converter.rs` — `webfetch/converter.rs` から移行
  - [x] `provider.rs` — `WebToolProvider`（ToolProvider実装）
  - [x] `lib.rs` — re-exports
- [x] **2a.2** `ene-tools/host` に `WebToolProvider` を登録
- [x] **2a.3** 動作確認（ListTools で webfetch/websearch を確認、統合テスト通過）
- [x] **2a.4** `ene-ai-core/src/tools/core/mod.rs` の `EneToolRegistry` から web 関連を削除
- [x] **2a.5** `ene-ai-core/Cargo.toml` から `reqwest` は core 内の web ツール以外では未使用。Phase 3 で `src/tools/web/` ディレクトリ完全削除時に削除可能

#### 2b: `ene-tools/utility`

- [x] **2b.1** クレート作成・コード移行
  - [x] `Cargo.toml` 作成
  - [x] `provider.rs` — `UtilityToolProvider`（get_current_time / get_system_info / question / todo）
  - [x] `question.rs` — 質問ツール
  - [x] `todo.rs` — TodoStore + Todo ツール
  - [x] `truncation.rs` — Truncate ユーティリティ（内部使用、ツールとして未登録）
- [x] **2b.2** host 登録・動作確認（全 utility ツール、IPC 統合テスト通過）
- [x] **2b.3** builtin から utility ツール削除
  - [x] `BuiltinToolRegistry` を空に（get_current_time / get_system_info を IPC 版へ移行）
  - [x] `EneToolRegistry` から question / todo / get_current_time / get_system_info を削除

#### 2c: `ene-tools/app`

- [x] **2c.1** クレート作成・コード移行
  - [x] `actions.rs`, `portal.rs` — GUI自動化
  - [x] `provider.rs` — `AppToolProvider`
- [x] **2c.2** host 登録・動作確認
- [x] **2c.3** builtin削除（EneToolRegistry から app を削除）

#### 2d: `ene-tools/browser`

- [x] **2d.1** クレート作成・コード移行
  - [x] `chrome.rs`, `extract.rs`, `session.rs` — ブラウザ自動化
  - [x] `provider.rs` — `BrowserToolProvider`
  - [x] `BrowserSessionStore` は host 側に配置（IPC 越しでセッション管理）
- [x] **2d.2** host 登録・動作確認
- [x] **2d.3** builtin削除（EneToolRegistry から browser を削除）

#### 2e: `ene-tools/fs` （最も複雑、Sandbox + Undo統合）

- [x] **2e.0** クレート作成・基盤構築
  - [x] `Cargo.toml` 作成
  - [x] `provider.rs` — `FsToolProvider` stub（IPC 登録用）
  - [x] host 登録・builtin 削除（EneToolRegistry から fs/shell/undo を削除）
- [ ] **2e.1** Sandbox + Undo 統合実装
  - [ ] `sandbox.rs` — `SandboxConfig` + `UndoManager` 統合型
  - [ ] `permission.rs` — `PermissionGate`, `DestructiveAction`
  - [ ] `SandboxConfigData::into_config()` 変換
- [ ] **2e.2** 各ツール移行
  - [ ] `read.rs` — `Sandbox::check_readable()` 使用
  - [ ] `write.rs` — `Sandbox::check_writable()` + `track_overwrite()` / `track_creation()` 使用
  - [ ] `edit/` — 9戦略 + `Sandbox::check_writable()` + `track_overwrite()`
  - [ ] `delete.rs` — `Sandbox::check_writable()` + `track_deletion()`
  - [ ] `patch/` — `Sandbox::track_patch()` 使用
  - [ ] `search/` — glob + grep、`Sandbox::check_readable()` 使用
  - [ ] `filesystem.rs` — 統合ディスパッチャ
  - [ ] `shell/` — `Sandbox::check_command()` + `check_writable()` 使用
- [ ] **2e.3** `provider.rs` — `FsToolProvider` 完全実装（`Sandbox` 内包）
- [ ] **2e.4** host 登録・動作確認（全 fs/shell/undo ツール）

### Phase 3: クリーンアップ

> 目標: builtinシステムを完全に削除し、IPC版のみにする

- [ ] **3.1** `ene-ai-core/src/tools/` ディレクトリ全体を削除
- [ ] **3.2** `BuiltinToolRegistry` 削除
- [ ] **3.3** `ToolRegistryBuilder::with_builtin()` 削除
- [ ] **3.4** `SandboxConfig`, `PermissionGate` を `ene-ai-core` から削除（`ene-tools/fs` のみに存在）
- [ ] **3.5** `ene-ai-core/Cargo.toml` から削除可能な依存関係を削除
  - [ ] `chromiumoxide`, `scraper`, `htmd` (browser)
  - [ ] `xcap`, `enigo`, `active-win-pos-rs`, `ashpd`, `pipewire` (app)
  - [ ] `walkdir`, `glob`, `diff`, `strsim` (fs/edit)
  - [ ] `dashmap` (undo_manager, browser_session, todo_store) — core で不要なら
  - [ ] 他、ツール専用依存を順次確認
- [ ] **3.6** `ToolRegistry` trait の `ensure_index_built()` を trait から削除し、`CompositeToolRegistry` のプライベートメソッドに降格
  - 理由: Embedding は core 側の責務。IPC先の `IpcToolRegistry` は embedding を知る必要がない

### Phase 4: 最適化・安定化

- [ ] **4.1** ホストプロセスのクラッシュ耐性
  - [ ] 自動再起動（指数バックオフ）
  - [ ] `IpcToolRegistry` の自動再接続
- [ ] **4.2** セキュリティ強化
  - [ ] UDS パーミッション設定（0600）
  - [ ] ホストプロセスの権限分離（将来的に別ユーザーで実行可能に）
- [ ] **4.3** パフォーマンス
  - [ ] IPC のベンチマーク測定
  - [ ] 必要に応じてバイナリプロトコル（MessagePack等）への移行検討
- [ ] **4.4** `docs/ai-core-flow.md` 更新（新しいアーキテクチャ反映）

---

## 6. 依存関係マッピング

### 6.1 各ツールクレートの外部依存

| クレート | 外部依存（eno-tool-proto以外） |
|----------|-------------------------------|
| `ene-tools/fs` | tokio, serde, serde_json, regex, walkdir, glob, diff, strsim, dashmap, uuid, chrono |
| `ene-tools/browser` | chromiumoxide, scraper, htmd, base64, dashmap, tokio, uuid, serde, serde_json, regex |
| `ene-tools/app` | enigo, xcap, active-win-pos-rs, ashpd, pipewire, base64, image, tokio, serde_json |
| `ene-tools/web` | reqwest, htmd, scraper, regex, urlencoding, serde_json, tokio |
| `ene-tools/utility` | dashmap, serde, serde_json, chrono |
| `ene-tools/host` | tokio, ene-tool-proto, ene-tools/{fs,browser,app,web,utility} |

### 6.2 core から削除される依存（Phase 3 完了後）

| 依存 | 現在の用途 | 移行先 |
|------|-----------|--------|
| `chromiumoxide` | browser ツール | `ene-tools/browser` |
| `scraper` | browser DOM解析 | `ene-tools/browser` |
| `htmd` | HTML→Markdown 変換 | `ene-tools/browser`, `ene-tools/web` |
| `xcap` | スクリーンショット | `ene-tools/app` |
| `enigo` | キーボード/マウス | `ene-tools/app` |
| `active-win-pos-rs` | ウィンドウ情報 | `ene-tools/app` |
| `ashpd` | Linux Portal | `ene-tools/app` |
| `pipewire` | Wayland キャプチャ | `ene-tools/app` |
| `walkdir` | ディレクトリ走査 | `ene-tools/fs` |
| `glob` | glob パターン | `ene-tools/fs` |
| `diff` | diff 生成 | `ene-tools/fs` |
| `strsim` | Levenshtein 距離 | `ene-tools/fs` |
| `dashmap` | 並行ハッシュマップ | `ene-tools/fs`, `ene-tools/browser`, `ene-tools/utility` |
| `urlencoding` | URL エンコード | `ene-tools/web` |

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
| その他 core 専用依存 | |

---

## 7. リスクと対策

| リスク | 対策 |
|--------|------|
| IPC 通信のレイテンシ | ツール呼び出しは元々非同期で数秒〜数十分かかる操作。UDS のミリ秒レイテンシは無視可能 |
| ホストプロセスクラッシュ | 自動再起動（指数バックオフ）。クラッシュ時は進行中のツール呼び出しがエラーとして返るのみ |
| セッションステートの同期 | `SetSessionId` IPC通知で同期。UDSは順序保証あり |
| 大きなファイル内容のIPC転送 | 4バイト長前置き + JSON で最大数百MB対応可能。将来的にバイナリプロトコルに切替可能 |
| ブラウザセッションのIPC越し管理 | `BrowserSessionStore` は host 側に配置。セッションIDで識別 |
| 段階的移行中の二重メンテナンス | builtin版とIPC版が並存する間は、変更を両方に適用。各フェーズで速やかにbuiltin削除 |