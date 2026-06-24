# `ene-tool-proto`

> Ene ツールシステムの IPC ワイヤープロトコル — `ToolSpec`、`IpcRequest`/`IpcResponse`、`ToolError`、およびトランスポートヘルパー。

`ene-tool-proto` は、`ene-tool-host`（ホスト側）とスタンドアロンのツールバイナリとの間でプロセス境界を越えるすべての型を定義します。IPC チャンネルの両サイドはこのクレートに依存しています。`ene-core` への依存がないため、完全なランタイムを引き込まずにツールバイナリからインポートできます。

関連ページ：ホスト側の接続管理については [`ene-tool-host`](ene-tool-host.md)、ツール側の `ToolAction` トレイトについては [`ene-tool-common`](ene-tool-common.md) を参照してください。

---

## プロトコルバージョン

```rust
pub const IPC_PROTOCOL_VERSION: u32 = 1;
```

双方が `Handshake` / `HandshakeAck` メッセージで自分のバージョンを送信します。バージョンが一致しない場合、接続は切断されます。この定数は、ワイヤーフォーマットが後方互換性のない形で変更された場合にのみバンプしてください（[AGENTS.md §4 R3](../../AGENTS.md) を参照）。

---

## `ToolSpec`

単一の呼び出し可能なツールを記述します。Tool RAG パイプラインで使用される主要なメタデータ型であり、ツールリストの一部として LLM に渡されます。

```rust
pub struct ToolSpec {
    pub name: ToolName,
    pub version: ToolVersion,
    pub display_name: String,
    pub summary: String,
    pub description: String,
    pub category: ToolCategory,
    pub keywords: KeywordSet,
    pub parameters: serde_json::Value,  // JSON Schema オブジェクト
    pub examples: serde_json::Value,
    pub caveats: Vec<String>,
    pub side_effects: SideEffects,
    pub preconditions: Vec<String>,
    pub related: Vec<ToolName>,
}
```

### 補助型

#### `KeywordSet`

```rust
pub struct KeywordSet {
    /// ツールの機能を直接表す高シグナルな用語。
    pub primary: Vec<String>,
    /// 補足的・文脈的な用語。
    pub secondary: Vec<String>,
    /// ドメインタグ（例："filesystem"、"web"、"shell"）。
    pub domain: Vec<String>,
    /// このツールを使うべきでない状況を示す用語。
    pub negative: Vec<String>,
}
```

`negative` セットは、クエリ語がネガティブキーワードと重なる場合にツールのランクを下げるために RAG パイプラインで使用されます。

#### `SideEffects`

```rust
pub enum SideEffects {
    None,
    ReadOnly,
    Writes,
    Network,
    Destructive,
}
```

#### `ToolName` / `ToolVersion` / `ToolCategory`

`String` に対するニュータイプラッパー。`ToolName` は `<namespace>.<action>` という規約に従います（例：`fs.read_file`）。

---

## `ToolError`

すべてのツールの失敗は `ToolError` のバリアントとして表現されます。`Serialize`/`Deserialize` に対応しており、`IpcResponse::CallResult` の内部で IPC 境界を越えます。

```rust
pub enum ToolError {
    // ── 汎用 ────────────────────────────────────────────────
    NotFound { tool_name: String },
    InvalidArguments { message: String },
    ExecutionFailed { message: String },
    Internal { message: String },
    Other { message: String },

    // ── サンドボックス / セキュリティ ─────────────────────────
    SandboxViolation { message: String },
    PermissionDenied { message: String },
    CommandBlocked { command: String, reason: String },

    // ── インタラクティブ（再試行前にホスト側のアクションが必要）──
    /// ツールが明示的なユーザー/ホストの許可を必要とする。
    PermissionRequired {
        request_id: String,
        action: String,
        target: String,
        description: String,
    },
    /// ツールが続行のためにユーザーの回答を必要とする。
    UserInputRequired {
        request_id: String,
        prompt: UserInputPrompt,
    },

    // ── トランスポート / IPC ────────────────────────────────
    IpcTransport { message: String },
    IpcClient { message: String },

    // ── タイムアウト ────────────────────────────────────────
    Timeout { message: String },
    ShellTimeout { command: String, timeout_ms: u64 },

    // ── I/O ────────────────────────────────────────────────
    IoError { message: String },
    FileNotFound { path: String },
    FileTooLarge { path: String, size: u64, limit: u64 },
    ShellOutputTooLarge { size: usize, limit: usize },

    // ── ドメイン固有 ────────────────────────────────────────
    BrowserError { message: String },
    AppError { message: String },
    WebSearchError { message: String },
}
```

### インタラクティブエラーフロー

ツールが `PermissionRequired` または `UserInputRequired` を返した場合、ホストは次の手順を取ります：

1. リクエストをユーザーに提示するか、ポリシーを適用する。
2. `ToolRegistry::approve_permission(request_id)` を呼び出すか、回答を収集する。
3. 同じ引数で `call_tool` を再呼び出しする。

---

## `IpcRequest`

**ホスト**（`ene-tool-host`）からツールバイナリへ送信されるメッセージです。

```rust
pub enum IpcRequest {
    /// プロトコルバージョンを交渉します。必ず最初のメッセージとして送ります。
    Handshake { version: u32 },
    /// サンドボックスポリシーとツールごとの設定を提供します。
    Initialize {
        sandbox: SandboxConfigData,
        tool_config: Option<serde_json::Value>,
    },
    /// ツールの完全なメタデータリストをリクエストします。
    ListTools,
    /// アクション単位のメタデータをリクエストします（メガツール埋め込み用）。
    ListActionSpecs,
    /// ツールの設定 JSON スキーマをリクエストします。
    GetConfigSchema,
    /// 名前と JSON 引数でツールを起動します。
    CallTool { name: String, arguments: String },
    /// アクティブなセッション ID を伝播します。
    SetSessionId { session_id: String },
    /// 保留中のパーミッションリクエストを承認します。
    ApprovePermission { request_id: String },
    /// サンドボックスの許可リストにパターンを追加します。
    AllowPattern { action: String, target_pattern: String },
    /// ツールの設定を取得します。
    GetMyConfig,
    /// ツールの設定を置き換えます。
    SetMyConfig(serde_json::Value),
    /// ヘルスチェック ping。
    Ping,
    /// グレースフルシャットダウン。
    Shutdown,
}
```

---

## `IpcResponse`

ツールバイナリからホストへ返送されるメッセージです。

```rust
pub enum IpcResponse {
    /// Handshake を受理し、交渉後のバージョンを返します。
    HandshakeAck { version: u32 },
    /// 汎用の確認応答（Initialize、`SetSessionId` などへの応答）。
    Ack,
    /// ListTools への応答。
    Tools { tools: Vec<ToolSpec> },
    /// ListActionSpecs への応答。メガツールの場合はアクションごとに 1 エントリ。
    ActionSpecs { specs: Vec<ActionSpec> },
    /// GetConfigSchema への応答。
    ConfigSchema { schema: Option<serde_json::Value> },
    /// CallTool への応答。
    CallResult { result: Result<String, ToolError> },
    /// GetMyConfig への応答。
    MyConfig(serde_json::Value),
    /// Ping への Pong 応答。
    Pong,
    /// 特定の呼び出し外で発生したツール側の回復不能エラー。
    Error { message: String },
}
```

### メッセージシーケンス図

```
ホスト                        ツール
 │                             │
 │── Handshake ───────────────▶│
 │◀── HandshakeAck ────────────│
 │── Initialize ──────────────▶│
 │◀── Ack ─────────────────────│
 │── ListTools ───────────────▶│
 │◀── Tools([...]) ────────────│
 │                             │
 │── CallTool(name, args) ────▶│
 │◀── CallResult(Ok(str)) ─────│   正常系
 │                             │
 │── CallTool(name, args) ────▶│
 │◀── CallResult(Err(PermissionRequired{...}))
 │    [ホストが承認]            │
 │── ApprovePermission(id) ───▶│
 │◀── Ack ─────────────────────│
 │── CallTool(name, args) ────▶│   再試行
 │◀── CallResult(Ok(str)) ─────│
```

---

## インタラクティブツール型

ユーザーから構造化された回答を収集するために `ToolError::UserInputRequired` で使用されます。

### `UserInputPrompt`

```rust
pub struct UserInputPrompt {
    pub items: Vec<QuestionItem>,
}
```

### `QuestionItem`

```rust
pub struct QuestionItem {
    pub question: String,
    /// 空でない場合、ユーザーはこのリストから選択する必要があります
    /// （allow_free_text が true の場合を除く）。
    pub options: Vec<String>,
    /// options が指定されていても自由テキストの回答を許可する。
    pub allow_free_text: bool,
}
```

### `MultiAnswer`

```rust
pub enum MultiAnswer {
    /// ユーザーが提供された選択肢のひとつを選んだ。
    Selected { option: String },
    /// ユーザーが自由テキストで回答した。
    Answer { text: String },
    /// ユーザーがこの質問をスキップした。
    Skip,
}
```

---

## トランスポート

### `IpcStream`

クロスプラットフォームのフレーム化バイトストリームです：

- **Unix** — Unix ドメインソケット（`AF_UNIX`）
- **Windows** — 名前付きパイプ（`\\.\pipe\…`）

### ワイヤーヘルパー

```rust
/// 長さプレフィックス付きの JSON エンコードされた IpcRequest をストリームに書き込む。
pub async fn write_ipc_request(
    stream: &mut IpcStream,
    req: &IpcRequest,
) -> Result<(), io::Error>;

/// ストリームから次の IpcResponse を読み込んでデコードする。
/// クリーンな EOF の場合は None を返す。
pub async fn read_ipc_response(
    stream: &mut IpcStream,
) -> Result<Option<IpcResponse>, io::Error>;
```

フレーミング形式：`[u32 リトルエンディアン長][JSON ペイロード]`。
最大メッセージサイズは 64 MB（`ene_tool_proto::ipc` の `MAX_MESSAGE_SIZE`）。

### `SandboxConfigData`

`Initialize` 時に送信されるサンドボックスポリシーのシリアライズ可能な表現です。具体的なフィールドは内部的なものであり変更される可能性があるため、ツールバイナリはこれを不透明な型として扱う必要があります。

---

## 関連ページ

- [`ene-tool-host`](ene-tool-host.md) — ホスト側のライフサイクルとレジストリ
- [`ene-tool-common`](ene-tool-common.md) — ツール側の `ToolAction` トレイト
- [`ene-tool-derive`](ene-tool-derive.md) — `ToolSpec` 生成プロシージャルマクロ
