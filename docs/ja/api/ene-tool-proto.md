# `ene-tool-proto` — APIリファレンス

> **クレート:** `ene-tool-proto`
> **役割:** Eneツールシステムの IPC ワイヤープロトコル — `ToolProvider`、`ToolSpec`、`IpcRequest`/`IpcResponse`、`ToolError`、およびトランスポートヘルパー。

---

## 概要

`ene-tool-proto` は、ホストランタイム（`ene-runtime` / `ene-tool-host`）とスタンドアロンのツールバイナリとの間でプロセス境界を越えるすべての型とヘルパーを定義します。IPC チャンネルの両サイドはこのクレートに依存しています。`ene-runtime` への依存がないため、ツールバイナリは完全なランタイムを引き込まずにこのクレートをリンクできます。

このクレートには3つの責務があります。

1. **`ToolProvider` トレイト** — 各ツールバイナリが自身のツールを記述・実行するために実装するインターフェース。
2. **ワイヤープロトコル** — `IpcRequest` / `IpcResponse`。Unix ドメインソケット（Unix）または名前付きパイプ（Windows）上でフレーム化された長さプレフィックス付き JSON として送受信され、`ToolProvider` を稼働中の IPC サーバーに変換する [`run_tool_server`](#run_tool_server) ヘルパーが付属します。
3. **共有メタデータ型** — `ToolSpec`、`ActionSpec`、`ToolName`、`ToolVersion`、`ToolCategory`、`KeywordSet`、`SideEffects`、`ToolError`。Tool RAG パイプラインで使用され、ツールリストの一部として LLM に渡されます。

関連ページ：ホスト側の接続管理については [`ene-tool-host`](./ene-tool-host.md)、ツール側の `ToolAction`/`ToolSpecArgs` トレイトについては [`ene-tool-common`](./ene-tool-common.md)、`ToolSpec` を生成するプロシージャルマクロについては [`ene-tool-derive`](./ene-tool-derive.md) を参照してください。

---

## プロトコルバージョン

```rust
pub const IPC_PROTOCOL_VERSION: u32 = 2;
```

双方が `Handshake` / `HandshakeAck` メッセージで自分のバージョンを送信します。サーバー（`run_tool_server`）はバージョンの不一致を**厳密に拒否**します — ダウングレードや交渉は行わず、`IpcResponse::Error` で接続を終了します。この定数は、ワイヤーフォーマットが後方互換性のない形で変更された場合にのみバンプしてください（[AGENTS.md §6 R3](../../AGENTS.md) を参照）。バージョン **2** は LLM 向けにスリム化した `ToolSpec`（`name` / `description` / `parameters` のみ）を反映します。

---

## `ToolProvider` トレイト

各ツールバイナリが実装するインターフェースです。ホスト側の `IpcToolRegistry`（`ene-tool-host` 内）は、IPC のみを通じて `ToolProvider` と通信します — このトレイトはツールプロセスの*内部*で実行される内容の契約です。

```rust
#[async_trait]
pub trait ToolProvider: Send + Sync {
    fn list_specs(&self) -> Vec<ToolSpec>;

    fn list_action_specs(&self) -> Vec<ActionSpec> {
        Vec::new()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>;

    fn set_session_id(&self, session_id: &str);

    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {}

    fn approve_permission(&self, _request_id: &str) {}

    fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}

    fn set_config(&self, _config: &serde_json::Value) {}

    fn get_config(&self) -> serde_json::Value {
        serde_json::Value::Null
    }

    fn config_schema(&self) -> Option<serde_json::Value> {
        None
    }
}
```

### メソッドテーブル

| メソッド | 必須か | デフォルト | 説明 |
|---|---|---|---|
| `list_specs(&self) -> Vec<ToolSpec>` | **必須** | — | このプロバイダーが提供する全ツールの完全なメタデータ。メガツールはアクションごとに1つ、N個のスペックを返す（例：`filesystem.read`、`filesystem.write` など）。 |
| `list_action_specs(&self) -> Vec<ActionSpec>` | 任意 | `Vec::new()` | Tool RAG 埋め込みに使用されるアクション単位のメタデータ。メガツールの場合のみ意味を持ち、個別ツールでは空のままにする。 |
| `call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>` | **必須** | — | JSON エンコードされた `arguments` を用いて名前でツールを実行する。`async`。 |
| `set_session_id(&self, session_id: &str)` | **必須** | — | 現在のセッション ID を設定する（アンドゥ追跡、セッション単位の状態などに使用）。 |
| `set_sandbox(&self, sandbox: &SandboxConfigData)` | 任意 | no-op | サンドボックス設定を受け取る（ファイルシステム／シェルツールで使用）。 |
| `approve_permission(&self, request_id: &str)` | 任意 | no-op | ID で保留中の破壊的操作の許可リクエストを承認する。 |
| `allow_pattern(&self, action: &str, target_pattern: &str)` | 任意 | no-op | セッション全体の許可パターン（アクション + ターゲットのグロブ）を追加する。 |
| `set_config(&self, config: &serde_json::Value)` | 任意 | no-op | ツール固有の設定を受け取る（`Initialize` または `SetMyConfig` 時に呼ばれる）。 |
| `get_config(&self) -> serde_json::Value` | 任意 | `Value::Null` | ツールの現在の設定を返す。 |
| `config_schema(&self) -> Option<serde_json::Value>` | 任意 | `None` | このツールが受け付ける設定の JSON スキーマを返す。 |

`HostRegistry`（後述）も `ToolProvider` を実装しているため、複数のプロバイダーのまとまりを単一のプロバイダーとして扱うことができます。

---

## `run_tool_server`

```rust
pub async fn run_tool_server(provider: Box<dyn ToolProvider>) -> Result<(), ToolError>;
```

`ToolProvider` を IPC サーバーとして起動します。この関数は**ジェネリックではありません** — `run_tool_server::<T>()` ではなく、ボックス化されたトレイトオブジェクトを受け取ります。呼び出し元が失敗に対して `match` できるように、ボックス化されたエラートレイトオブジェクトではなく `ToolError` を返します。ソケットI/Oエラーは `ToolError` の `From<std::io::Error>` 実装を介して変換されます。

動作：

1. `ENE_TOOL_SOCKET` 環境変数からソケット／パイプパスを読み取り、未設定の場合は `/tmp/ene-tool.sock`（Unix）または `\\.\pipe\ene-tool`（Windows）を既定値とする。
2. 古いソケットファイルを削除し、新しいリスナーをバインドする。Unix では、ソケットを `0600` に `chmod` する。
3. 接続をループで受け入れ、接続ごとにタスクを起動して `IpcRequest` を読み取り、プロバイダーに対してディスパッチし、`IpcResponse` を書き戻す。
4. `IpcRequest::Shutdown` を受信するとクリーンにシャットダウンする（応答を返してからループを抜け、ソケットファイルを削除する）。

---

## `HostRegistry`

```rust
#[derive(Default)]
pub struct HostRegistry { /* private fields */ }

impl HostRegistry {
    pub fn new() -> Self;
    pub fn add_provider(&mut self, provider: Box<dyn ToolProvider>);
    pub fn list_specs(&self) -> Vec<ToolSpec>;
    pub async fn call_tool(&self, name: &ToolName, arguments: &str) -> Result<String, ToolError>;
    pub fn set_session_id(&self, session_id: &str);
    pub fn set_sandbox(&self, sandbox: &SandboxConfigData);
}

impl ToolProvider for HostRegistry { /* ... */ }
```

複数の `ToolProvider` を集約し、ツール名で呼び出しをディスパッチする複合レジストリです。複数のプロバイダーをひとつのカスタムツールバイナリにまとめたい場合に便利です — スタンドアロンのツールバイナリでは通常、単一のプロバイダーで十分です。

### メソッドテーブル

| メソッド | 説明 |
|---|---|
| `new()` | 空のレジストリを作成する。 |
| `add_provider(provider)` | プロバイダーを登録する。ツール名が競合した場合は先に登録されたプロバイダーが優先される。プロバイダーが公開する `ToolSpec::name` すべてをインデックス化する。 |
| `list_specs()` | 登録されたすべてのプロバイダーからのツールスペックを連結して返す。 |
| `call_tool(name, arguments)` | `name` を登録したプロバイダーにディスパッチする。どのプロバイダーもその名前を所有していない場合は `ToolError::NotFound` を返す。 |
| `set_session_id(session_id)` | セッション ID を登録済みの全プロバイダーにブロードキャストする。 |
| `set_sandbox(sandbox)` | サンドボックス設定を登録済みの全プロバイダーにブロードキャストする。 |

`HostRegistry` 自体も `ToolProvider` を実装しています。そのトレイトレベルの `call_tool(&self, name: &str, ...)` メソッドは、[`ToolName::try_new`](#toolname) で `name` を解析し、IPC 経由で渡された文字列が不正な場合は（パニックではなく）`ToolError::InvalidName` を返します。

---

## 型

### `ToolSpec`

単一の呼び出し可能なツールに関する、LLM 向けの構造化された記述です。Tool RAG パイプラインで使用される主要なメタデータ型であり、ツールリストの一部として LLM に渡されます。

```rust
pub struct ToolSpec {
    pub name: ToolName,
    pub version: ToolVersion,
    pub display_name: String,
    pub summary: String,
    pub description: String,
    pub category: ToolCategory,
    pub keywords: KeywordSet,
    pub parameters: serde_json::Value,  // schemars によって自動導出される JSON Schema
    pub examples: Vec<ToolExample>,
    pub caveats: Vec<String>,
    pub side_effects: SideEffects,
    pub preconditions: Vec<String>,
    pub related: Vec<ToolName>,
}
```

| メソッド | シグネチャ | 説明 |
|---|---|---|
| `embedding_text` | `fn embedding_text(&self, field: EmbeddingField) -> String` | RAG 埋め込み用のテキスト表現を構築する。各バリアントに含まれる内容は [`EmbeddingField`](#embeddingfield) を参照。 |

### `ActionSpec`

メガツール内の1つの機能（例：`filesystem` ツールにおける `filesystem.read` と `filesystem.write` はそれぞれ独立した `ActionSpec`）。Tool RAG では常に個別に埋め込まれ、`IpcResponse::ActionSpecs` で公開されます。

```rust
pub struct ActionSpec {
    pub name: String,
    pub display_name: String,
    pub summary: String,
    pub description: String,
    pub keywords: KeywordSet,
    pub examples: Vec<ToolExample>,
    pub caveats: Vec<String>,
    pub side_effects: SideEffects,
    pub preconditions: Vec<String>,
}
```

| メソッド | シグネチャ | 説明 |
|---|---|---|
| `minimal` | `fn minimal(name: impl Into<String>, summary: impl Into<String>) -> Self` | 最小限の `ActionSpec` を構築する — `name`、`display_name`（`name` のコピー）、`summary` を設定し、それ以外のフィールドはすべてデフォルト値のまま（`ReadOnly` の副作用、空の Vec、`KeywordSet::default()`）とする。完全な記述子を必要としないシンプルな非メガツールで使用される。 |

### `ToolName`

検証済みの名前空間付きツール識別子。`String` に対するニュータイプラッパーです。

```rust
pub struct ToolName(/* private */ String);
```

形式：メガツールの場合は `"<namespace>.<action>"`（例：`"filesystem.read"`）、個別ツールの場合は単純な `"<name>"`（例：`"utility.get_current_time"`）。有効な文字は ASCII 英数字、`_`、`.`（先頭・末尾のドットは不可、空文字も不可）です。

| メソッド | シグネチャ | 説明 |
|---|---|---|
| `is_valid` | `fn is_valid(name: &str) -> bool` | `name` が空でなく、有効な文字のみを含む場合に `true` を返す。 |
| `new` | `fn new(name: impl Into<String>) -> Self` | `ToolName` を構築する。不正な入力に対して**パニックする** — 信頼できるコンパイル時検証済みのリテラル（例：`#[tool]` 属性名）のみに使用すること。 |
| `try_new` | `fn try_new(name: impl Into<String>) -> Result<Self, String>` | 失敗を許容するコンストラクタ。**信頼できないすべての入力に使用する** — IPC のツール名、MCP サーバーのツール名、設定／環境変数の値、DB の行など。 |
| `namespace` | `fn namespace(&self) -> Option<&str>` | 名前空間部分（`"filesystem.read"` → `Some("filesystem")`）。名前空間を持たないツールでは `None`。 |
| `action` | `fn action(&self) -> &str` | アクション部分（`"filesystem.read"` → `"read"`）。 |
| `as_str` | `fn as_str(&self) -> &str` | 内部文字列を借用する。 |
| `into_string` | `fn into_string(self) -> String` | `self` を消費し、内部の `String` を返す。 |

`Display`、`From<&str>`（`new` と同様に不正入力でパニック）、`From<String>`（同様）も実装しています。

### `ToolVersion`

```rust
pub struct ToolVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}
```

セマンティックバージョン。semver 上意味のある変更（メジャーバージョンの増加、または spec の `version` フィールドの変更）が発生した場合のみ、埋め込みキャッシュを無効化するために使用されます。

| メソッド | シグネチャ | 説明 |
|---|---|---|
| `new` | `const fn new(major: u32, minor: u32, patch: u32) -> Self` | バージョンを構築する。 |

`Default`（`1.0.0`）と `Display`（`"{major}.{minor}.{patch}"`）を実装しています。

### `ToolCategory`

```rust
pub enum ToolCategory {
    Filesystem,
    Shell,
    Browser,
    App,
    WebSearch,
    WebFetch,
    Utility,
    Memory,
    Search,
    Meta,
}
```

分類および RAG フィルタリングに使用されます。メガツール（`Filesystem`、`Shell`、`Browser`、`App`）は、`IpcResponse::ActionSpecs` を介して IPC レベルで追加のアクション単位スペックを持ちます。

| メソッド | シグネチャ | 説明 |
|---|---|---|
| `label` | `fn label(&self) -> &'static str` | このカテゴリの埋め込みテキストで使用される、人間が読める形式のラベル（例：`Filesystem` → `"filesystem_tools"`）。 |

### `KeywordSet`

```rust
pub struct KeywordSet {
    pub primary: Vec<String>,
    pub secondary: Vec<String>,
    pub domain: Vec<String>,
    pub negative: Vec<String>,
}
```

Tool RAG スコアリングで使用される構造化されたキーワードバッグです。各層は異なる重みを持ちます（`ene-tool-host::rag` の `FieldWeights`）：`primary` ≈ `1.0`、`secondary` ≈ `0.6`、`domain` ≈ `0.3`、`negative` ≈ `-0.5`（クエリ語が重なった場合の緩やかなペナルティ）。

| メソッド | シグネチャ | 説明 |
|---|---|---|
| `primary_only` | `fn primary_only(primary: impl IntoIterator<Item = impl Into<String>>) -> Self` | `primary` キーワードのみを設定した `KeywordSet` を構築する。 |
| `with_secondary` | `fn with_secondary(primary: ..., secondary: ...) -> Self` | `primary` + `secondary` キーワードを設定した `KeywordSet` を構築する。 |
| `is_empty` | `fn is_empty(&self) -> bool` | 4つの Vec がすべて空の場合に `true`。 |

### `SideEffects`

```rust
pub enum SideEffects {
    ReadOnly,
    FileSystem { mutates: bool },
    Network { external: bool },
    System { privileged: bool },
    Browser { mutates_dom: bool },
    Destructive,
    Idempotent,
}
```

ツールが持つ副作用の種類。安全性分析とサンドボックスフィルタリングに使用されます。`Default` は `ReadOnly` です。`#[serde(tag = "kind", rename_all = "snake_case")]` でシリアライズされます。

### `ToolExample`

```rust
pub struct ToolExample {
    pub description: String,
    pub input: serde_json::Value,
    pub output: Option<String>,
}
```

ツールの使用例のひとつで、LLM に表示され、例ベースの RAG 埋め込みに使用されます。`output` が存在する場合、その例は高信頼度として扱われ、RAG インデックスでより高く重み付けされます。

### `EmbeddingField`

```rust
pub enum EmbeddingField {
    Summary,
    Description,
    Negative,
}
```

[`ToolSpec::embedding_text`](#toolspec) が生成する `ToolSpec` のテキストコンテンツのうち、どの部分集合を使うかを制御します。

| バリアント | 含まれるテキスト |
|---|---|
| `Summary` | `"{name}: {summary}"` — 最もシグナルが強く、高速検索に使用。 |
| `Description` | `"{name}\n{description}"` に加え、空でない場合はフォーマット済みのキーワードブロック（`Primary: ... \| Secondary: ... \| Domain: ... \| Negative: ...`）— ランキングの精緻化に使用。 |
| `Negative` | `"{name} NOT: {ネガティブキーワードを", "で連結}"`。ネガティブキーワードがない場合は `""` — 曖昧性の解消に使用。 |

| メソッド | シグネチャ | 説明 |
|---|---|---|
| `as_str` | `fn as_str(&self) -> &'static str` | インデックスに保存される文字列ラベル（`"summary"`、`"description"`、`"negative"`）。 |

### `SandboxConfigData`

```rust
pub struct SandboxConfigData {
    pub enabled: bool,                        // デフォルト: true
    pub allowed_directories: Vec<String>,     // デフォルト: ["."]
    pub writable_directories: Vec<String>,    // デフォルト: ["."]
    pub blocked_commands: Vec<String>,        // デフォルト: [rm -rf /, dd if=, mkfs, sudo, フォークボム]
    pub max_read_bytes: usize,                // デフォルト: 50 * 1024
    pub max_write_bytes: usize,                // デフォルト: 1024 * 1024
    pub shell_timeout_ms: u64,                // デフォルト: 120_000
    pub max_shell_output_bytes: usize,        // デフォルト: 50 * 1024
    pub max_shell_output_lines: usize,        // デフォルト: 2000
    pub db_socket: Option<String>,            // デフォルト: None
    pub db_auth_token: Option<String>,        // デフォルト: None
}
```

`IpcRequest::Handshake` 時に送信され（v3 で旧 `Initialize` から吸収）、`ene_config::define_tool_config!` によって生成される、サンドボックスポリシーのシリアライズ可能な POD 表現です。フィールドの補足：

- `db_socket` — ツールごとの DB IPC ソケット（Unix ドメインソケット）へのパス。ツールバイナリはここに接続して、型付き CRUD 用のコア DB サーバーに到達します（[`ene-tool-db`](./ene-tool-db.md) を参照）。
- `db_auth_token` — ツールバイナリが最初の `ene_tool_db::DbRequest::Handshake` で提示しなければならない事前共有トークン。`None` の場合、そのツールの DB アクセスは完全に無効化されます。

それ以外については、ツールバイナリはこの構造体の具体的なデフォルト値をホストの実装詳細として扱い、上記のフィールド形状のみに依存すべきです。

### `ToolConfigAccessor`

```rust
pub struct ToolConfigAccessor { /* private */ }

impl ToolConfigAccessor {
    pub fn new(initial_config: serde_json::Value) -> Self;
    pub async fn get<T: serde::de::DeserializeOwned>(&self) -> Result<T, ToolError>;
    pub async fn set<T: serde::Serialize>(&self, config: &T) -> Result<(), ToolError>;
}
```

ツールのライブ JSON 設定を保持する、共有・`RwLock` 保護されたホルダーです。`ToolProvider::set_config`/`get_config` の実装の構成要素として便利です。

| メソッド | 説明 |
|---|---|
| `new(initial_config)` | 与えられた JSON 値を `Arc<RwLock<...>>` でラップする。 |
| `get::<T>()` | 保存された JSON を `T` に逆シリアライズする。保存された値が `T` の形状に一致しない場合、（サイレントなデフォルト値ではなく）`ToolError::InvalidArguments` を返す。 |
| `set(config)` | `config` を JSON にシリアライズして保存し、シリアライズに失敗した場合は `ToolError::InvalidArguments` を返す。 |

---

## `ToolError`

すべてのツールの失敗は `ToolError`（`EneToolProtoError` の型エイリアス）のバリアントとして表現されます。`Serialize`/`Deserialize` に対応しており、`IpcResponse::CallResult` の内部で IPC 境界を越えます。

```rust
pub enum ToolError {
    // ── 汎用 ────────────────────────────────────────────────
    NotFound { tool_name: String },
    InvalidName { reason: String },
    InvalidArguments { message: String },
    ExecutionFailed { message: String },
    Internal { message: String },
    Other { message: String },

    // ── サンドボックス / セキュリティ ─────────────────────────
    SandboxViolation { message: String },
    PermissionDenied { message: String },
    CommandBlocked { command: String, reason: String },

    // ── インタラクティブ（再試行前にホスト側のアクションが必要）──
    PermissionRequired {
        request_id: String,
        action: String,
        target: String,
        description: String,
    },
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
    ShellOutputTooLarge { size: u64, limit: u64 },
}
```

> **注意：** `BrowserError`、`AppError`、`WebSearchError` というバリアントは存在しません。ドメイン固有のツール失敗（ブラウザ、アプリ自動化、Web検索）は、専用のドメインごとのバリアントではなく、上記の汎用バリアント — 通常は `ExecutionFailed` または `Other` — を通じて報告されます。
>
> `InvalidName` は、呼び出し元が指定したツール名が `ToolName::try_new` の検証に失敗した場合に、`HostRegistry::call_tool` などの IPC エントリーポイントによって、不正な入力に対してパニックする代わりに返されます。

`ToolError` は `std::error::Error` と `From<std::io::Error>`（`IoError` にマッピング）を実装しています。

### インタラクティブエラーフロー

ツールが `PermissionRequired` または `UserInputRequired` を返した場合、ホストは次の手順を取ります：

1. リクエストをユーザーに提示するか、ポリシーを適用する。
2. `HostRegistry`／レジストリレベルの `approve_permission(request_id)` を呼び出すか、ユーザーの回答を収集する。
3. 同じ引数で `call_tool` を再呼び出しする。

---

## インタラクティブツール型

ユーザーから構造化された回答を収集するために `ToolError::UserInputRequired` の内部で使用されます。

### `UserInputPrompt`

```rust
pub struct UserInputPrompt {
    pub items: Vec<QuestionItem>,
}
```

`Display` を実装しており、各アイテムを `"{index}. {question} (options: ...) [free text]"` としてレンダリングします。

### `QuestionItem`

```rust
pub struct QuestionItem {
    pub question: String,
    pub options: Vec<String>,
    pub allow_free_text: bool,
}
```

`options` が空でない場合、`allow_free_text` が `true` でない限り、ユーザーはそのリストから選択する必要があります。

### `MultiAnswer`

```rust
pub enum MultiAnswer {
    Selected { option: String },
    Answer { text: String },
    Skip,
}
```

`UserInputPrompt::items` と同じ順序で、`QuestionItem` ごとに1エントリを持つ `Vec<MultiAnswer>` として返されます。

---

## `IpcRequest`

**コア**（`ene-runtime` / `ene-tool-host`）からツールバイナリへ送信されるメッセージです。

```rust
pub enum IpcRequest {
    Handshake { version: u32 },
    Initialize {
        sandbox: SandboxConfigData,
        tool_config: Option<serde_json::Value>,
    },
    ListTools,
    ListActionSpecs,
    GetConfigSchema,
    CallTool { name: String, arguments: String },
    SetSessionId { session_id: String },
    ApprovePermission { request_id: String },
    AllowPattern { action: String, target_pattern: String },
    GetMyConfig,
    SetMyConfig(serde_json::Value),
    Ping,
    Shutdown,
}
```

| バリアント | 用途 |
|---|---|
| `Handshake { version }` | プロトコルバージョンを交渉する。必ず最初のメッセージであること。 |
| `Initialize { sandbox, tool_config }` | サンドボックスポリシーとツールごとの設定を提供する。 |
| `ListTools` | ツールの完全なメタデータリストをリクエストする。 |
| `ListActionSpecs` | アクション単位のメタデータをリクエストする（メガツール埋め込み用）。 |
| `GetConfigSchema` | ツールの設定 JSON スキーマをリクエストする。 |
| `CallTool { name, arguments }` | 名前と JSON 引数でツールを起動する。 |
| `SetSessionId { session_id }` | アクティブなセッション ID を伝播する。 |
| `ApprovePermission { request_id }` | 保留中のパーミッションリクエストを承認する。 |
| `AllowPattern { action, target_pattern }` | サンドボックスの許可リストにパターンを追加する。 |
| `GetMyConfig` | ツールの設定を取得する。 |
| `SetMyConfig(value)` | ツールの設定を置き換える。 |
| `Ping` | ヘルスチェック ping。 |
| `Shutdown` | グレースフルシャットダウン。 |

---

## `IpcResponse`

ツールバイナリから**コア**（`ene-runtime` / `ene-tool-host`）へ返送されるメッセージです。

```rust
pub enum IpcResponse {
    HandshakeAck { version: u32 },
    Ack,
    Tools { tools: Vec<ToolSpec> },
    ActionSpecs { specs: Vec<ActionSpec> },
    ConfigSchema { schema: Option<serde_json::Value> },
    CallResult { result: Result<String, ToolError> },
    MyConfig(serde_json::Value),
    Pong,
    Error { message: String },
}
```

| バリアント | 用途 |
|---|---|
| `HandshakeAck { version }` | Handshake を受理し、交渉後のバージョンを返す。 |
| `Ack` | 汎用の確認応答（`Initialize`、`SetSessionId` などへの応答）。 |
| `Tools { tools }` | `ListTools` への応答。 |
| `ActionSpecs { specs }` | `ListActionSpecs` への応答。メガツールの場合はアクションごとに1エントリ。 |
| `ConfigSchema { schema }` | `GetConfigSchema` への応答。 |
| `CallResult { result }` | `CallTool` への応答。 |
| `MyConfig(value)` | `GetMyConfig` への応答。 |
| `Pong` | `Ping` への応答。 |
| `Error { message }` | 特定の呼び出し外で発生したツール側の回復不能エラー（例：ハンドシェイクのバージョン不一致）。 |

### メッセージシーケンス図

```text
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

## トランスポート

### `IpcStream`

`AsyncRead` + `AsyncWrite` を実装するクロスプラットフォームのフレーム化バイトストリームです：

- **Unix** — `tokio::net::UnixStream`（Unix ドメインソケット、`AF_UNIX`）をラップする。
- **Windows** — `NamedPipeServer`（サーバー側）または `NamedPipeClient`（クライアント側）、つまり `\\.\pipe\...` をラップする。

| メソッド | シグネチャ | 説明 |
|---|---|---|
| `connect` | `async fn connect(path: &Path) -> io::Result<Self>` | 待ち受け中の IPC エンドポイントに接続する（プラットフォームに応じて適切な方式）。 |

### `IpcListener`

クロスプラットフォームの IPC リスナーです。

| メソッド | シグネチャ | 説明 |
|---|---|---|
| `bind` | `fn bind(path: &Path) -> io::Result<Self>` | IPC エンドポイントにバインドする。Unix では `UnixListener` をラップする。Windows では最初の名前付きパイプインスタンスを作成する。 |
| `accept` | `async fn accept(&mut self) -> io::Result<IpcStream>` | 新しい接続を受け入れる。Windows では、各 accept 後に次のパイプインスタンスを透過的に再作成する。 |

### `cleanup_path`

```rust
pub fn cleanup_path(path: &Path);
```

Unix ではソケットファイルを削除する。Windows では no-op（名前付きパイプはファイルシステムオブジェクトではない）。

### 汎用ワイヤーヘルパー

以下の4つの関数は `AsyncReadExt`/`AsyncWriteExt` に対してジェネリックであり、`IpcStream` に紐付いていません — このクレート自身のテストで使われている `tokio::io::duplex` ストリームや、その他の任意の非同期バイトストリームに対しても直接動作します。

```rust
/// 4バイトの長さプレフィックス付き JSON として IpcRequest を読み取る。
/// UnexpectedEof（接続が閉じられた）の場合は Ok(None) を返す。
pub async fn read_ipc_request<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<Option<IpcRequest>, ToolError>;

/// 4バイトの長さプレフィックス付き JSON として IpcRequest を書き込む。
pub async fn write_ipc_request<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    req: &IpcRequest,
) -> Result<(), ToolError>;

/// 4バイトの長さプレフィックス付き JSON として IpcResponse を読み取る。
/// UnexpectedEof（接続が閉じられた）の場合は Ok(None) を返す。
pub async fn read_ipc_response<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<Option<IpcResponse>, ToolError>;

/// 4バイトの長さプレフィックス付き JSON として IpcResponse を書き込む。
pub async fn write_ipc_response<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    resp: &IpcResponse,
) -> Result<(), ToolError>;
```

フレーミング形式：`[u32 リトルエンディアン長][JSON ペイロード]`。最大メッセージサイズは 64 MB（`ene_tool_proto::ipc` に非公開の `MAX_MESSAGE_SIZE`）。これを超えるリクエスト／レスポンスは `ToolError::IpcTransport` で拒否されます。

---

## エラー

このクレートのすべての失敗しうる操作は [`ToolError`](#toolerror)（エイリアス `EneToolProtoError`）を通じて報告されます。独立した「トランスポートエラー」型は存在しません — ワイヤーヘルパーでの I/O 失敗は `From<std::io::Error>` を介して `ToolError::IoError` に変換され、不正な JSON は `ToolError::InvalidArguments` として報告されます。

---

## 使用例

### `ToolProvider` の実装

```rust,no_run
use async_trait::async_trait;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolName, ToolProvider, ToolSpec,
    ToolVersion, run_tool_server,
};

struct MyTool;

#[async_trait]
impl ToolProvider for MyTool {
    fn list_specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: ToolName::new("hello"),
            version: ToolVersion::new(1, 0, 0),
            display_name: "Hello".into(),
            summary: "Greets the user".into(),
            description: "Greets the user with a personalised message.".into(),
            category: ToolCategory::Utility,
            keywords: KeywordSet::primary_only(["greet", "hello", "greeting"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Name to greet"}
                },
                "required": ["name"]
            }),
            examples: vec![],
            caveats: vec![],
            side_effects: SideEffects::ReadOnly,
            preconditions: vec![],
            related: vec![],
        }]
    }

    async fn call_tool(&self, _name: &str, args: &str) -> Result<String, ToolError> {
        let v: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| ToolError::InvalidArguments { message: e.to_string() })?;
        Ok(format!("Hello, {}!", v["name"].as_str().unwrap_or("world")))
    }

    fn set_session_id(&self, _sid: &str) {}
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // run_tool_server::<MyTool>() ではない — この関数はボックス化されたトレイトオブジェクトを受け取る。
    run_tool_server(Box::new(MyTool)).await?;
    Ok(())
}
```

### `HostRegistry` で複数プロバイダーをまとめる

```rust,no_run
use ene_tool_proto::{HostRegistry, ToolProvider, run_tool_server};

fn build_registry(a: Box<dyn ToolProvider>, b: Box<dyn ToolProvider>) -> HostRegistry {
    let mut registry = HostRegistry::new();
    registry.add_provider(a);
    registry.add_provider(b);
    registry
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let a: Box<dyn ToolProvider> = unimplemented!();
    let b: Box<dyn ToolProvider> = unimplemented!();
    let registry = build_registry(a, b);
    run_tool_server(Box::new(registry)).await?;
    Ok(())
}
```

### 信頼できないツール名の検証

```rust,no_run
use ene_tool_proto::{ToolError, ToolName};

fn handle_ipc_call_tool(raw_name: &str) -> Result<ToolName, ToolError> {
    // 信頼できない入力（ワイヤーから届いたもの）— new ではなく try_new を使う。
    ToolName::try_new(raw_name).map_err(|reason| ToolError::InvalidName { reason })
}
```

### メガツールのアクション用に `ActionSpec` を構築する

```rust,no_run
use ene_tool_proto::ActionSpec;

let read_action = ActionSpec::minimal("read", "Read a file from disk");
assert_eq!(read_action.name, "read");
```

### RAG 埋め込みテキストの算出

```rust,no_run
use ene_tool_proto::{EmbeddingField, ToolSpec};

fn summary_embedding(spec: &ToolSpec) -> String {
    spec.embedding_text(EmbeddingField::Summary)
}
```

---

## 関連ページ

- [`ene-tool-host`](./ene-tool-host.md) — ホスト側のライフサイクルとレジストリ
- [`ene-tool-common`](./ene-tool-common.md) — ツール側の `ToolAction`/`ToolSpecArgs` トレイト
- [`ene-tool-derive`](./ene-tool-derive.md) — `ToolSpec` 生成プロシージャルマクロ
- [`ene-tool-db`](./ene-tool-db.md) — `SandboxConfigData::db_socket` 上で動作する、ツールごとのデータベース IPC
