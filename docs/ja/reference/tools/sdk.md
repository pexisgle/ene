# SDK: カスタムツールの作成

`ene-tool-proto` は ene に統合するカスタムツールバイナリを作成するための軽量 SDK です。

## クイックスタート

### 1. プロジェクトを作成

```toml
# Cargo.toml
[package]
name = "my-cool-tool"
version = "0.1.0"
edition = "2024"

[dependencies]
ene-tool-common = { git = "https://github.com/pexisgle/ene" }
ene-tool-proto = { git = "https://github.com/pexisgle/ene" }
ene-tool-derive = { git = "https://github.com/pexisgle/ene" }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
schemars = "1"
async-trait = "0.1"
```

### 2. `#[derive(ToolAction)]` でアクションを定義

各ツールは `#[derive(ToolAction)]` を付けた構造体です。derive マクロが `ToolSpec`、JSON Schema、`ToolAction` impl を生成します。ビジネスロジックは `async fn run(&self)` に記述します。

```rust
use ene_tool_common::prelude::*;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "greeter",
    name = "hello",
    summary = "Returns a greeting for the given name.",
    category = "Utility",
    keywords_primary = "greeting, hello",
    side_effects = "ReadOnly"
)]
pub struct HelloAction {
    /// Name to greet.
    name: String,
}

impl HelloAction {
    async fn run(&self) -> Result<String, ToolError> {
        Ok(format!("Hello, {}!", self.name))
    }
}
```

### 3. ToolProvider を実装

単一のステートレスなアクションの場合は、`ToolProvider` を手書きする代わりに [`ActionSetProvider`](#toolprovider-アダプタ) を単一要素の vec で使うことを推奨します:

```rust
use ene_tool_common::ActionSetProvider;

let provider = ActionSetProvider::new(vec![Box::new(HelloAction::default())]);
```

カスタムの `set_call_context`/`set_sandbox` の動作が必要な場合、または完全な制御が必要な場合は、代わりに `ToolProvider` を手書きします:

```rust
use ene_tool_proto::{ToolError, ToolProvider, ToolSpec};

struct MyToolProvider;

#[async_trait]
impl ToolProvider for MyToolProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        vec![HelloAction::default().definition()]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            HelloAction::TOOL_NAME => {
                let action = HelloAction::default();
                action.execute(arguments).await
            }
            _ => Err(ToolError::NotFound {
                tool_name: name.to_string(),
            }),
        }
    }

    fn set_session_id(&self, _session_id: &str) {}
}
```

### 4. サーバーを起動

```rust
#[tokio::main]
async fn main() {
    run_tool_server(Box::new(MyToolProvider)).await.unwrap();
}
```

### 5. ビルドして配置

```bash
cargo build --release
mkdir -p ~/.local/share/dev.pexisgle.ene/tools
cp target/release/my-cool-tool ~/.local/share/dev.pexisgle.ene/tools/
```

### 6. 設定で有効化

```json
{
  "tools": {
    "tools": {
      "my-cool-tool": { "enable": true }
    }
  }
}
```

## ステートフルアクション

サンドボックス、データベース、HTTP クライアント等の依存注入が必要なアクションには `#[tool(skip)]` を使用します:

```rust
use ene_tool_common::prelude::*;
use std::sync::Arc;

fn default_client() -> reqwest::Client {
    reqwest::Client::new()
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "myapi",
    name = "fetch",
    summary = "Fetch data from the API.",
    category = "WebFetch"
)]
pub struct FetchAction {
    /// The endpoint to call.
    endpoint: String,

    #[tool(skip)]
    #[serde(skip, default = "default_client")]
    client: reqwest::Client,
}

impl FetchAction {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            endpoint: String::new(),
            client,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let resp = self.client.get(&self.endpoint).send().await
            .map_err(|e| ToolError::ExecutionFailed { message: e.to_string() })?;
        Ok(resp.text().await.unwrap_or_default())
    }
}
```

プロバイダは実際の依存でアクションを構築し、`execute()` がそれらをコピーします:

```rust
async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
    match name {
        FetchAction::TOOL_NAME => {
            let action = FetchAction::new(self.client.clone());
            action.execute(arguments).await
        }
        _ => Err(ToolError::NotFound { tool_name: name.to_string() }),
    }
}
```

## 複数ツール

単一プロバイダから複数のツールを公開できます。[`ActionSetProvider`](#toolprovider-アダプタ) を使えば、ディスパッチ用の `match` を手書きする必要はありません:

```rust
use ene_tool_common::ActionSetProvider;

let provider = ActionSetProvider::new(vec![
    Box::new(AddAction::default()),
    Box::new(SubtractAction::default()),
]);
```

同等の手書き実装（単純な名前マッチを超えるディスパッチロジックが必要な場合に有用）:

```rust
#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(namespace = "calculator", name = "add", summary = "Add two numbers.", category = "Utility")]
pub struct AddAction { a: f64, b: f64 }

impl AddAction {
    async fn run(&self) -> Result<String, ToolError> {
        Ok(format!("{} + {} = {}", self.a, self.b, self.a + self.b))
    }
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(namespace = "calculator", name = "subtract", summary = "Subtract two numbers.", category = "Utility")]
pub struct SubtractAction { a: f64, b: f64 }

impl SubtractAction {
    async fn run(&self) -> Result<String, ToolError> {
        Ok(format!("{} - {} = {}", self.a, self.b, self.a - self.b))
    }
}

struct CalculatorProvider;

#[async_trait]
impl ToolProvider for CalculatorProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        vec![
            AddAction::default().definition(),
            SubtractAction::default().definition(),
        ]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            AddAction::TOOL_NAME => AddAction::default().execute(arguments).await,
            SubtractAction::TOOL_NAME => SubtractAction::default().execute(arguments).await,
            _ => Err(ToolError::NotFound { tool_name: name.to_string() }),
        }
    }

    fn set_session_id(&self, _session_id: &str) {}
}
```

## ToolProvider アダプタ

`ene-tool-common` は `ToolProvider` を実装するアダプタを提供しており、ほとんどのツールは `list_specs`/`call_tool` のディスパッチループを手書きする必要がありません:

| アダプタ | 使用場面 | セッション/サンドボックスフック |
|---|---|---|
| `ActionSetProvider::new(vec![...])` | バイナリあたり1つ以上のアクション（`ene-tool-fs`、`ene-tool-app`、`ene-tool-browser` で使われているメガツールパターン） | `.with_set_call_context_hook(...)`、`.with_sandbox_hook(...)` |

リクエストされたツール名と `ToolAction::name()` を照合して `call_tool` をディスパッチし、一致しない場合は `ToolError::NotFound` を返します — これは、このコードベース内のすべての手書きプロバイダーがこれまで再実装してきたのと同じ動作です。アクションが `set_call_context`/`set_sandbox` に反応する必要がある場合（例: 会話IDやDBソケットを共有状態に伝える）は、手動の `ToolProvider` 実装に切り替える代わりにフックを登録してください:

```rust
use ene_tool_common::ActionSetProvider;
use std::sync::Arc;

let state = Arc::new(MyState::default());
let session_state = state.clone();

let provider = ActionSetProvider::new(vec![Box::new(MyAction::new(state))])
    .with_set_call_context_hook(move |conv_id| session_state.set_session_id(conv_id));
```

完全な実例は `tools/utility/src/provider.rs` を参照してください（セッションIDとDBサンドボックスのソケット/トークンの両方がフック経由で伝えられています）。

## ToolProvider トレイトリファレンス

```rust
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// このプロバイダが公開するツール一覧を返す。
    /// メガツールはアクションごとに 1 スペック返す (例: `filesystem.read`, ...)。
    fn list_specs(&self) -> Vec<ToolSpec>;

    /// ホスト/RAG メタデータプロファイルを返す (#137)。デフォルト: 空。
    fn list_rag_profiles(&self) -> Vec<ToolRagProfile> { Vec::new() }

    /// ツール名と JSON 引数でツールを実行
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>;

    /// 呼び出しコンテキスト (会話 + ターン識別子) を設定
    fn set_call_context(&self, _ctx: &CallContext) {}

    /// サンドボックス設定を受信 (ファイルシステムツール用; デフォルト: no-op)
    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {}

    /// ペンディング中の破壊的操作の権限リクエストを承認
    fn approve_permission(&self, _request_id: &str) {}

    /// セッション全体の権限許可パターンを追加 (アクション + ターゲットグロブ)
    fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}

    /// ツール固有設定を受信 (Handshake 時に 1 回呼び出される)
    fn set_config(&self, _config: &serde_json::Value) {}

    /// このツールが受け付ける設定の JSON Schema を返す
    fn config_schema(&self) -> Option<serde_json::Value> { None }
}
```

## ToolSpec

LLM 向けの構造化されたツール仕様（v3 / #135 で slim 化）:

```rust
pub struct ToolSpec {
    pub name: ToolName,                     // 例: "filesystem.read"
    pub description: String,                // 完全なマークダウン説明
    pub parameters: serde_json::Value,      // JSON Schema (schemars による自動生成)
}
```

RAG メタデータ（キーワード、例、カテゴリなど）は [`ToolRagProfile`](#toolragprofile) (#137) にあり、`ToolSpec` には含まれません。

## ToolRagProfile

呼び出し可能なツールのホスト/RAG 専用メタデータ。LLM のツールリストには渡されません — `IpcResponse::RagProfiles`（IPC v4）経由で交換され、`ene-tool-rag` が消費します。

```rust
pub struct ToolRagProfile {
    pub name: ToolName,
    pub display_name: String,
    pub summary: String,
    pub description: String,
    pub category: ToolCategory,
    pub keywords: KeywordSet,
    pub examples: Vec<ToolExample>,
    pub caveats: Vec<String>,
    pub preconditions: Vec<String>,
    pub side_effects: SideEffects,
    pub related: Vec<ToolName>,
    pub version: ToolVersion,
}
```

`#[derive(ToolSpec)]` は同じ `#[tool(...)]` 属性から `spec() -> ToolSpec` と `rag_profile() -> ToolRagProfile` の両方を生成します。`ActionSetProvider` は `ToolProvider::list_rag_profiles` 経由でプロファイルを集約します。

## `#[tool(...)]` 属性

```rust
#[derive(ToolAction, JsonSchema, Deserialize)]
#[tool(
    // 必須
    namespace = "calculator",        // 名前空間プレフィックス
    name = "add",                    // アクション名 (完全名: "calculator.add")
    summary = "Add two numbers.",    // 1行の要約 (埋め込みフィールド)
    category = "Utility",            // ToolCategory バリアント

    // オプション
    display_name = "Add Numbers",    // デフォルト: 名前のタイトルケース
    description = "Longer markdown", // デフォルト: summary
    version = "1.0.0",               // デフォルト: 1.0.0
    side_effects = "ReadOnly",       // デフォルト: ReadOnly

    // キーワード (カンマ区切り)
    keywords_primary = "add, sum, plus",
    keywords_secondary = "math, number",
    keywords_domain = "arithmetic",
    keywords_negative = "subtract, remove",

    // メタデータ (カンマ区切り)
    caveats = "Division by zero returns an error.",
    preconditions = "Arguments must be valid numbers.",
    related = "calculator.subtract, calculator.multiply",

    // 例 (セミコロン区切り、各: 説明|入力|出力)
    examples = "Add 2 and 3|{ \"a\": 2, \"b\": 3 }|2 + 3 = 5"
)]
pub struct AddAction {
    /// First operand.
    a: f64,
    /// Second operand.
    b: f64,
}
```

詳細は [derive-macro.md](derive-macro.md) を参照してください。

## ToolCategory

| バリアント | 使用場面 |
|-----------|---------|
| `Filesystem` | ファイル読み取り、書き込み、編集操作 |
| `Shell` | シェルコマンド実行 |
| `Browser` | Web フェッチ、ブラウザ自動化 |
| `App` | GUI 自動化、デスクトップ操作 |
| `WebSearch` | 検索エンジンクエリ |
| `WebFetch` | URL フェッチ |
| `Utility` | ヘルパーツール (時刻、システム情報など) |
| `Memory` | 長期記憶操作 |
| `Search` | ローカル検索 / ユーザードキュメントの RAG |
| `Meta` | 自己分析、ツール選択 |

## ToolError

```rust
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
    PermissionRequired { request_id: String, action: String, target: String, description: String },
    UserInputRequired { request_id: String, prompt: UserInputPrompt },
    FileNotFound { path: String },
    FileTooLarge { path: String, size: u64, limit: u64 },
    CommandBlocked { command: String, reason: String },
    ShellTimeout { command: String, timeout_ms: u64 },
    ShellOutputTooLarge { size: u64, limit: u64 },
    BrowserError { message: String },
    AppError { message: String },
    WebSearchError { message: String },
    IpcClient { message: String },
    Other { message: String },
}
```

## IPC ライフサイクル

```
ツールバイナリ起動
  → ENE_TOOL_SOCKET で待機 (ToolHostManager が環境変数として提供)
  → IpcRequest::Handshake を受信 → HandshakeAck を応答
  → Handshake が sandbox + tool_config を運ぶ
  → CallTool リクエストを処理可能に
```

## プロトコルバリアント

`IPC_PROTOCOL_VERSION = 1` の IPC ワイヤープロトコルは、9 種類のリクエストバリアントと 7 種類のレスポンスバリアントを持ちます。`UserInput` は IPC バリアントでは**ありません** — `ToolError::UserInputRequired` を通じて表面化され、`ene-runtime` のストリーミングループで処理されます。

### リクエスト（ホスト → ツール）

| バリアント | ペイロード | セマンティクス |
|---|---|---|
| `Handshake` | `version: u32`, `sandbox: SandboxConfigData`, `tool_config: Option<Value>` | プロトコル交渉 + サンドボックス設定 + ツール設定プッシュ |
| `ListTools` | — | プロバイダから全 `ToolSpec` を取得 |
| `ListRagProfiles` | — | ホスト/RAG 用 `ToolRagProfile` メタデータを取得 (#137) |
| `GetConfigSchema` | — | ツール設定の JSON Schema をリクエスト（#150 例外） |
| `CallTool` | `name: String`, `arguments: String` | ツール名と JSON 引数でツールを実行 |
| `SetCallContext` | `conversation_id: String`, `turn_id: String` | 会話・ターン識別子をツールに伝達 |
| `ApprovePermission` | `request_id: String` | ペンディング中の破壊的操作の権限リクエストを承認 |
| `AllowPattern` | `action: String`, `target_pattern: String` | セッション全体の権限許可パターンを登録 |
| `Shutdown` | — | 正常終了リクエスト |

### レスポンス（ツール → ホスト）

| バリアント | ペイロード | トリガー |
|---|---|---|
| `HandshakeAck` | `version: u32` | `Handshake` |
| `Ack` | — | `SetCallContext`, `ApprovePermission`, `AllowPattern`, `Shutdown` |
| `Tools` | `tools: Vec<ToolSpec>` | `ListTools` |
| `RagProfiles` | `profiles: Vec<ToolRagProfile>` | `ListRagProfiles` |
| `ConfigSchema` | `schema: Option<Value>` | `GetConfigSchema` |
| `CallResult` | `result: Result<String, ToolError>` | `CallTool` |
| `Error` | `message: String` | IPC レベルで失敗した任意のリクエスト |

## ABI 互換性

ワイヤーABIは `ene-tool-proto` のIPC表面すべてを指します: `IpcRequest`/`IpcResponse`、`IPC_PROTOCOL_VERSION`、`ToolSpec`/`ToolRagProfile` のフィールド、`SandboxConfigData`、`ToolError`。`run_tool_server` は `IPC_PROTOCOL_VERSION` の不一致を**厳密に拒否**します — ダウングレードや交渉は行いません — そのため、バージョンアップの判断は重要です。

| 変更 | 互換性あり? | 必要な対応 |
|---|---|---|
| `IpcRequest`/`IpcResponse` に新しいenumバリアントを追加 | ✅ 追加的 | 不要 — 古いツールバイナリは新しいバリアントを単に送受信しません。新しいホストコードは、古いツールバイナリがそれを送らないケースも処理する必要があります。 |
| `ToolSpec`/`ToolRagProfile`/`SandboxConfigData` に新しい任意フィールドを追加（`#[serde(default)]` またはマクロ提供のデフォルト付き） | ✅ 追加的 | 不要。`SandboxConfigData` が既に使っている `define_tool_config!`/`schemars` のパターンに従えば、フィールドのない古いJSONもそのままデシリアライズできます。 |
| `ToolError` に新しいバリアントを追加 | ✅ 追加的 | 不要 — `ToolError` は（タグ付き列挙体ではあるが）単純な列挙型なので、古いコードがワイルドカードアームなしで網羅的に `match` していない限り、新しいバリアントは問題なくデシリアライズされます。新しい `match` にはワイルドカード（`_ => ...`）アームを追加することを推奨します。 |
| `ToolProvider` トレイトに新しいメソッドを追加 | ✅ 追加的（デフォルト実装がある場合） | `set_sandbox`、`approve_permission`、`set_config` などが既にそうしているように、デフォルト（no-op / 空）実装を与えてください — これにより、既存のすべてのプロバイダー（手書きでもアダプタベースでも）がコンパイルされ続けます。 |
| 既存の `IpcRequest`/`IpcResponse` バリアントを削除・改名、またはフィールドの型/意味を変更 | ❌ 破壊的 | `ene-tool-proto` で `IPC_PROTOCOL_VERSION` をバンプする（[AGENTS.md §6 R3](../../../../AGENTS.md) を参照）。同じ変更でホスト（`ene-tool-host`）とすべてのツールバイナリを更新してください。 |
| `ToolProvider` トレイトメソッドを削除、または既存のデフォルト付きメソッドを必須化 | ❌ 破壊的 | 上記と同様 — これは各ツールバイナリが実装すべき内容を変更します。`tools/*` 全体での連携した更新に加え、ワイヤー動作も変わる場合は `PROTOCOL_VERSION` のバンプが必要です。 |
| `Box::new(provider)` の呼び出し箇所を壊すような形で `run_tool_server` のシグネチャを変更 | ❌ 破壊的（ソースレベル） | それ自体は `PROTOCOL_VERSION` のバンプを必要としません（Rust APIの破壊であり、ワイヤーの破壊ではない）が、同じ変更内ですべての `tools/*/src/main.rs` の呼び出し箇所と `AGENTS.md` §6 R1 のレシピを更新してください。 |

要約すると: **追加的な変更は常に安全**です。既存のワイヤーフィールド/バリアントの意味を変更したり、ツールバイナリが既に送信している可能性のあるものを削除する変更には、`PROTOCOL_VERSION` のバンプとホスト・ツールバイナリ双方の連携した更新が必要です。

## ベストプラクティス

1. **`#[derive(ToolAction)]` を使用** — 一つの derive で spec、schema、ディスパッチを生成。ロジックは `async fn run(&self)` に記述。
2. **依存には `#[tool(skip)]` を使用** — サンドボックス、データベース、HTTP クライアント等は LLM から隠蔽され、プロバイダから注入されます。
3. **名前空間を使用** — 関連ツールを名前空間でグループ化 (例: `calculator.add`, `calculator.subtract`)。
4. **良い要約を書く** — 要約は Tool RAG のプライマリ埋め込みフィールドです。いつ、どのように使うかを明確に記述してください。
5. **キーワードを活用** — 同義語や関連用語を含めてください。プライマリキーワードは RAG スコアリングで最も高く重み付けされます。
6. **副作用を正しく設定** — LLM がツールの安全性を理解し、サンドボックスが正しい判断を行うのに役立ちます。
7. **エラーを適切に処理** — 明確なメッセージで `ToolError` バリアントを返してください。LLM はエラーメッセージに基づいて使用法を修正しようとすることがあります。
