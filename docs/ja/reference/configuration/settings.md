# 設定

ene の設定は `assets/settings.json`（初回起動時は OS のユーザ設定ディレクトリ）に集約されています。`settings.schema.json` が自動生成され、エディタでのバリデーションが可能です。

読み込み: `ene_config::load_full_config()` / `ConfigStore` がデフォルト値、ファイル、環境変数を解決します。

**API v1 の所有:** 永続化トグルは `store`（公開スキーマでは `enabled` のみ）。想起 / 書き込み / 減衰 / MMR / 感情の内部 / Performance 方針ノブは **`mind.*` のコードデフォルト** — ユーザー向けは `mind.emotion` と `mind.proactive` のポリシーフィールドのみ。トップレベルの `memory.*` 方針セクションや `cognition.enabled` 二重パイプラインスイッチはありません — mind パスが唯一のストリーミングパスです。

## トップレベル構造 (`EneConfig`)

```rust
pub struct EneConfig {
    pub version: u32,           // 現在 2
    pub character: String,      // キャラクターフォルダ名またはカードパス
    pub user_name: String,      // デフォルト "User"
    pub extra: HashMap<String, serde_json::Value>, // セクションマップ (ai, store, tools, mind, desktop, …)
}
```

`runtime_rules`（オーバーレイ向けの振る舞い指示）は**公開設定スキーマには含まれません**。`ene-config` のコンパイル時定数（`DEFAULT_RUNTIME_RULES`）として全システムプロンプトに注入されます。

### キャラクター解決ルール

- **フォルダ名を推奨**（例: `"Alicia"`）— デスクトップのキャラクター探索と一致し、設定を移植しやすい。
- 空文字 `""` → `assets_dir/characters/Alicia/character.json`（後方互換）。
- パス区切りを含まない文字列 → `assets_dir/characters/{name}/character.json`。
- `/` または `\` を含むパス → そのまま使用（絶対または相対のカードパス）。

## 完全な例

```json
{
  "version": 2,
  "character": "Alicia",
  "user_name": "User",
  "ai": {
    "providers": {
      "default": {
        "kind": "openai_compatible",
        "base_url": "",
        "api_key": { "source": "env", "env": "OPENAI_API_KEY", "inline": "" }
      }
    },
    "tasks": {
      "chat": { "provider": "default", "model": "gpt-4o-mini", "max_tokens": 8192 },
      "embedding": { "provider": "default", "model": "text-embedding-3-small", "dimensions": 1536 },
      "classifier": null,
      "proactive": null
    }
  },
  "store": { "enabled": false },
  "tools": {
    "enabled": true,
    "list": {
      "fs": { "enable": true, "allowed_directories": ["."], "writable_directories": ["."] },
      "web": { "enable": true, "tavily_api_key": "", "brave_api_key": "", "exa_api_key": "" },
      "browser": { "enable": true },
      "utility": { "enable": true },
      "app": { "enable": true }
    },
    "mcp_servers": []
  },
  "mind": {
    "emotion": { "enabled": true },
    "proactive": {
      "enabled": false,
      "interval_seconds": 60,
      "min_idle_seconds": 120,
      "cooldown_seconds": 300
    }
  },
  "desktop": {
    "language": "en",
    "graphics": { "quality": "medium" }
  }
}
```

## セクション

### `ai` — プロバイダレジストリとタスクルーティング

`ai` セクションはレガシーの `provider` ブロックに置き換わります。名前付きプロバイダを一度定義し、各認知ワークロード（`chat`、`embedding`、`classifier`、`proactive`）がプロバイダと任意のモデル override を指します。

```json
{
  "ai": {
    "providers": {
      "default": {
        "kind": "openai_compatible",
        "base_url": "",
        "api_key": { "source": "env", "env": "OPENAI_API_KEY", "inline": "" }
      }
    },
    "tasks": {
      "chat": { "provider": "default", "model": "gpt-4o-mini", "max_tokens": 8192 },
      "embedding": { "provider": "default", "model": "text-embedding-3-small", "dimensions": 1536 },
      "classifier": null,
      "proactive": null
    }
  }
}
```

| フィールド | 型 | 説明 |
|-----------|------|------|
| `providers` | object | プロバイダ名 → 定義のマップ |
| `tasks.chat` | object | メイン会話モデル（必須） |
| `tasks.embedding` | object | 埋め込みモデル（必須） |
| `tasks.classifier` | object または `null` | 感情分類器；`null` → `tasks.chat` にフォールバック |
| `tasks.proactive` | object または `null` | 能動発話の生成ルーティング；`null` → `tasks.chat` にフォールバック |

#### `ai.tasks` — タスク参照 (`TaskRef`)

| フィールド | 型 | 説明 |
|-----------|------|------|
| `provider` | string | `ai.providers` のキー |
| `model` | string | モデル名（`openai_compatible` の chat/embedding で必須） |
| `max_tokens` | int | チャット完了の最大トークン数（`0` = リクエストから省略）。OpenRouter はこの上限に対してクレジット担保を確保 |
| `dimensions` | int | 埋め込みベクトルの次元数（クラウド埋め込み） |
| `query_prefix` | string または null | 埋め込み検索クエリに前置する任意プレフィックス（例: `"Query: "`） |

#### `ai.providers` — プロバイダ種別

各プロバイダは `"kind"` タグ付きオブジェクトです:

##### `openai_compatible`

クラウド chat、embedding、classifier、クラウド能動判定を OpenAI 互換 HTTP API 経由で提供します。

```json
{
  "kind": "openai_compatible",
  "base_url": "https://api.openai.com/v1",
  "api_key": { "source": "env", "env": "OPENAI_API_KEY", "inline": "" }
}
```

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `base_url` | string | `""` | API ベース URL。空 → `OPENAI_BASE_URL` 環境変数 |
| `api_key` | object | (下記参照) | API キー設定 |

###### `api_key`

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `source` | string | `"env"` | `"inline"` または `"env"` |
| `inline` | string | `""` | API キー（`source = "inline"` 時、注意して使用） |
| `env` | string | `"OPENAI_API_KEY"` | `source = "env"` 時の環境変数名 |

##### `local_gguf`

プロセス内 llama-cpp-2 によるローカル GGUF。**埋め込み**（Hub モデル名）および/または **能動判定**（`model_path`）に使用します。

```json
{
  "kind": "local_gguf",
  "model": "jina-embeddings-v5-text-small",
  "quantization": "F16",
  "model_path": "",
  "acceleration": "auto",
  "gpu_layers": "auto",
  "context_size": 2048
}
```

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `model` | string | `"jina-embeddings-v5-text-small"` | 埋め込み用 Hub モデル名 |
| `quantization` | string | `"F16"` | 量子化レベル（例: `"F16"`, `"Q4_K_M"`） |
| `model_path` | string | `""` | 判定用 GGUF のファイルシステムパス。空 = 埋め込みのみ / Hub ダウンロード |
| `acceleration` | string | `"auto"` | `"auto"`, `"vulkan"`, `"cuda"`, `"cpu"` |
| `gpu_layers` | string | `"auto"` | `"auto"` または GPU layer offload 用の整数文字列 |
| `context_size` | int | `2048` | 判定モデルのコンテキストサイズ |

**ルーティング規則:**

- `tasks.chat` と `tasks.classifier` は `openai_compatible` プロバイダのみ使用可能。
- `tasks.embedding` はどちらの種別も使用可能。
- `tasks.classifier: null` → 分類器は `tasks.chat` のプロバイダとモデルを再利用。
- `tasks.proactive: null` → 能動発話の**生成**は `tasks.chat` を再利用。
- 能動**判定**: `tasks.proactive` が非空 `model_path` 付き `local_gguf` を指す場合はプロセス内 GGUF（ロード失敗時は OpenAI 互換の chat/proactive があればクラウドへフォールバック）。`tasks.proactive` が `openai_compatible` を指す場合はそのモデルでクラウド判定；それ以外は `tasks.chat`。[能動発話 ADR](../architecture/proactive-speech.md) を参照。

GGUF は同梱されず、パスはユーザー指定。外部 `llama-server` バイナリは不要。

#### マルチプロバイダの例

OpenRouter で chat + classifier、ローカル埋め込み:

```json
{
  "ai": {
    "providers": {
      "openrouter": {
        "kind": "openai_compatible",
        "base_url": "https://openrouter.ai/api/v1",
        "api_key": { "source": "env", "env": "OPENROUTER_API_KEY", "inline": "" }
      },
      "local_embed": {
        "kind": "local_gguf",
        "model": "jina-embeddings-v5-text-small",
        "quantization": "F16",
        "model_path": "",
        "acceleration": "auto",
        "gpu_layers": "auto",
        "context_size": 2048
      }
    },
    "tasks": {
      "chat": {
        "provider": "openrouter",
        "model": "xiaomi/mimo-v2.5",
        "max_tokens": 8192
      },
      "embedding": { "provider": "local_embed" },
      "classifier": {
        "provider": "openrouter",
        "model": "google/gemini-2.5-flash-lite"
      },
      "proactive": null
    }
  }
}
```

### `store` — SQLite-vec 永続化ストア

```json
{
  "store": {
    "enabled": false
  }
}
```

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `enabled` | bool | `false` | 永続化ストアを有効化 |

データベースパスは自動解決されます（`assets/characters/{name}/memory.db`）。公開スキーマではユーザー設定不可。

### `tools` — ツール設定

```json
{
  "tools": {
    "enabled": true,
    "list": {
      "fs": { "enable": true },
      "web": { "enable": true },
      "browser": { "enable": true },
      "utility": { "enable": true },
      "app": { "enable": true }
    },
    "mcp_servers": []
  }
}
```

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `enabled` | bool | `true` | 全ツールの関数呼び出しを有効化 |
| `list` | object | (組み込みツール) | ツール個別有効化マップとフラット化された任意設定 |
| `mcp_servers` | array | `[]` | MCP サーバーリスト |

`max_rounds` と `timeout_ms` は**コードデフォルト**で、薄い公開 UI スキーマには含まれません。Tool RAG は `tools.rag` で設定します（下記）。

#### `tools.list` — ツール個別エントリ

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `<name>.enable` | bool | `true` | 特定のツールを有効/無効化 |
| `<name>.*` | 各種 | — | ツール固有フィールドがエントリにフラット化（ネストした `config` オブジェクトなし） |

##### `tools.list.fs` — ファイルシステムサンドボックス

```json
{
  "fs": {
    "enable": true,
    "allowed_directories": ["."],
    "writable_directories": ["."],
    "blocked_commands": ["rm\\s+-rf\\s+/", "dd\\s+if=", "mkfs", "sudo\\s+", ":\\s*\\{\\s*\\|\\s*&\\s*;\\s*\\}"],
    "max_read_bytes": 51200,
    "max_write_bytes": 1048576,
    "shell_timeout_ms": 120000,
    "max_shell_output_bytes": 51200,
    "max_shell_output_lines": 2000
  }
}
```

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `allowed_directories` | string[] | `["."]` | 読み取りアクセスが許可されたディレクトリ |
| `writable_directories` | string[] | `["."]` | 書き込みアクセスが許可されたディレクトリ |
| `blocked_commands` | string[] | （危険コマンドの正規表現） | ブロックするシェルコマンドの正規表現 |
| `max_read_bytes` | int | `51200` | 1 回の読み取り上限バイト |
| `max_write_bytes` | int | `1048576` | 1 回の書き込み上限バイト |
| `shell_timeout_ms` | int | `120000` | シェルコマンドのタイムアウト |
| `max_shell_output_bytes` | int | `51200` | シェル出力の最大バイト数 |
| `max_shell_output_lines` | int | `2000` | シェル出力の最大行数 |

##### `tools.rag` — Tool RAG パイプライン

```json
{
  "rag": {
    "enabled": true,
    "use_hyde": false,
    "use_rerank": false,
    "top_k": 12,
    "final_n": 6,
    "background_index_on_startup": true
  }
}
```

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `enabled` | bool | `true` | Tool RAG を有効化（ツール有効時、これが true なら embedder 必須） |
| `use_hyde` | bool | `false` | 予約済み。LLM HyDE は無効（no-op） |
| `use_rerank` | bool | `false` | 候補の cosine 埋め込みリランク（LLM なし） |
| `top_k` | int | `12` | リランク前の候補数 |
| `final_n` | int | `6` | 最終返却ツール数 |
| `background_index_on_startup` | bool | `true` | 起動時にバックグラウンドでインデックスをウォームアップ（`false` でスキップ） |

##### `tools.list.web` — ウェブ検索 API キー

```json
{
  "web": {
    "enable": true,
    "tavily_api_key": "",
    "brave_api_key": "",
    "exa_api_key": ""
  }
}
```

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `tavily_api_key` | string | `""` | Tavily Search API キー |
| `brave_api_key` | string | `""` | Brave Search API キー |
| `exa_api_key` | string | `""` | Exa Search API キー |

#### `tools.mcp_servers` — Model Context Protocol サーバー

```json
{
  "mcp_servers": [
    {
      "name": "my-server",
      "enabled": true,
      "transport": {
        "type": "stdio",
        "command": "/usr/bin/my-mcp-server",
        "args": ["--verbose"]
      }
    },
    {
      "name": "http-server",
      "enabled": true,
      "transport": {
        "type": "http",
        "url": "http://localhost:3000/mcp"
      }
    }
  ]
}
```

| フィールド | 型 | 説明 |
|-----------|------|------|
| `name` | string | サーバー名（表示とルーティング） |
| `enabled` | bool | この MCP サーバーが有効かどうか |
| `transport` | object | トランスポート設定（下記参照） |

**トランスポートタイプ:**

| タイプ | フィールド | 説明 |
|--------|-----------|------|
| `stdio` | `command`, `args` | stdio トランスポートで子プロセスを起動 |
| `http` | `url` | HTTP 経由で接続 |

### `mind` — 認知ランタイム（公開サーフェス）

ユーザー向けポリシートグルのみ。コンテキスト予算、記憶抽出、キャラクターコンパイル、拡張能動/感情ノブはコードデフォルト（[Cognitive Runtime](../architecture/cognitive-runtime.md) 参照）。

```json
{
  "mind": {
    "emotion": { "enabled": true },
    "proactive": {
      "enabled": false,
      "interval_seconds": 60,
      "min_idle_seconds": 120,
      "cooldown_seconds": 300
    }
  }
}
```

#### `mind.emotion`

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `enabled` | bool | `true` | 感情処理を有効化 |
| `classifier_language` | string | `"en"` | 感情分類器のプロンプト言語（`"en"` または `"ja"`） |

感情分類器のモデルは `ai.tasks.classifier` でルーティング（`null` のとき `ai.tasks.chat` にフォールバック）。

#### `mind.proactive` — 能動発話

ユーザー入力なしの companion 発話ポリシー。デフォルトは **オフ**。モデルルーティングは `ai.tasks` 配下（[能動発話 ADR](../architecture/proactive-speech.md) 参照）。

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `enabled` | bool | `false` | 機能全体の on/off |
| `interval_seconds` | int | `60` | 判定 tick 間隔（最小 1） |
| `min_idle_seconds` | int | `120` | 最後のユーザー入力からの最低待機 |
| `cooldown_seconds` | int | `300` | 成功した能動発話（`TerminalReason::Done`）後の抑制時間 |

拡張能動設定（ソースフラグ、信頼度ゲート、タイムアウト、ツール許可）はコードデフォルト。

### `desktop` — GUI 設定

`ene-desktop` 実行時のみ利用可能な GUI 設定です。

```json
{
  "desktop": {
    "language": "ja",
    "graphics": { "quality": "medium" }
  }
}
```

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `language` | string | `"en"` | UI 言語: `"en"` または `"ja"` |
| `graphics.quality` | string | `"medium"` | グラフィックスプリセット: `"low"`, `"medium"`, `"high"` |

品質プリセットはランタイムで具体的なレンダラー設定（FPS、シャドウマップ、アンチエイリアシング、マスクダウンサンプル）にマップされます。個別の graphics フィールドは公開スキーマでは設定不可。

## 内部デフォルト（ユーザー向けではない）

以下はコードデフォルトで制御され、`settings.json` と生成スキーマから意図的に除外されています:

| 領域 | 例 |
|------|-----|
| `runtime_rules` | オーバーレイ向け振る舞い指示（`DEFAULT_RUNTIME_RULES`） |
| `session` | 自動分割、要約モデル override |
| `mind.context` | トークン予算、rolling compression しきい値 |
| `mind.memory` | 抽出、ハイブリッド recall、MMR（memory HyDE/LLM rerank は意図的に非搭載） |
| `mind.character` | CCv3 コンパイル、Identity Kernel 予算 |
| `mind.emotion` / `mind.proactive` | エンジンモード、分類器タイムアウト、ソースフラグ、信頼度ゲート |
| `tools` | `max_rounds`、`timeout_ms` |
| `store` | `db_path` |

## 読み込み順序

1. `EneConfig::default()` — コンパイル時デフォルト値
2. `assets/settings.json`（または OS ユーザ設定）— ユーザーオーバーライド
3. 環境変数（`ENE_` プレフィックス、`__` 区切りでネスト指定）

例: `ENE_AI__TASKS__CHAT__MODEL=gpt-4o` は `ai.tasks.chat.model` を上書きします。

読み込み後、`settings.schema.json` と `character_settings.schema.json` が `assets/schema/` に自動生成されます。

## JSON スキーマ

`settings.schema.json` は `cargo run -p ene-cli`（または任意のビルド）時に自動生成され、`assets/schema/settings.schema.json` に書き出されます。このファイルは gitignored です — コミットや手動編集はしないでください。

スキーマはエディタバリデーション（VS Code の `"json.schemas"` 設定）やプログラマティックな設定構築に使用できます。

## 設定登録 API

設定システムは宣言的マクロとグローバルスキーマレジストリ上に構築されています。各設定セクションはマクロで定義され、`Serialize`、`Deserialize`、`JsonSchema`、`Default`、`HasConfigKey` の実装が自動生成され、`#[ctor]` によりプログラム起動時にスキーマが登録されます。

### `define_config!`

設定構造体を定義するためのメインマクロ。3つのフォームがあります。

#### トップレベル settings セクション

```rust
ene_config::define_config!(
    settings,          // ターゲット: ConfigTarget::Settings
    "ai",              // EneConfig.extra の JSON キー
    /// AI プロバイダレジストリとタスクルーティング。
    pub struct AiConfig {
        pub providers: BTreeMap<String, AiProviderDef> = default_providers(),
        pub tasks: AiTasksConfig,
    }
);
```

生成されるもの:
- `#[derive(Serialize, Deserialize, JsonSchema)]` + `#[serde(rename_all = "snake_case", default)]`
- インライン `= default_value` 構文による `impl Default`（省略時は `Default::default()`）
- `impl HasConfigKey`: `KEY = "ai"`, `TARGET = Settings`, `path() = ["ai"]`
- `__register_schema::<AiConfig>(Settings, None)` を呼ぶ `#[ctor]` 関数

#### トップレベル character セクション

```rust
ene_config::define_config!(
    character,         // ターゲット: ConfigTarget::Character
    "expressions",     // character_settings.json の JSON キー
    pub struct ExpressionsConfig {
        pub entries: Vec<ExpressionEntry> = vec![],
    }
);
```

上記と同様だが `TARGET = Character` となり、`character_settings.json` 向けのスキーマが登録される。

#### ネストされたセクション（親構造体の子）

```rust
ene_config::define_config!(
    AiConfig,          // 親構造体（HasConfigKey を実装していること）
    "api_key",         // ai.providers.*.api_key の JSON キー
    pub struct ApiKeyConfig {
        pub source: String = "env".to_string(),
        pub env: String = "OPENAI_API_KEY".to_string(),
    }
);
```

親から `TARGET` を継承する。`path()` は親のパス + 自身のキーを返す（例: `["ai", "api_key"]`）。`#[ctor]` 呼び出しに親キーが渡され、スキーマが正確にネストされる。

### `define_tool_config!`

ツール固有の設定スキーマ用（`tools.list.<name>` にフラット化）:

```rust
ene_config::define_tool_config!(
    "fs",              // ツール名
    /// fs ツールのサンドボックス設定。
    pub struct SandboxConfigData {
        pub enabled: bool = true,
        pub allowed_directories: Vec<String> = vec![".".to_string()],
    }
);
```

同じ derive/デフォルト生成を行うが、`__register_tool_schema::<T>("fs")` を呼ぶ。スキーマは `parent_key = "tools_map"` で登録され、生成される JSON スキーマの `ToolConfig` 定義の `list` プロパティにマージされる。

### `HasConfigKey` トレイト

```rust
pub trait HasConfigKey {
    const KEY: &'static str;       // JSON キー（例: "ai"）
    const TARGET: ConfigTarget;    // Settings または Character
    fn path() -> &'static [&'static str]; // ルートからのフルパス（例: ["ai", "tasks"]）
}
```

`define_config!` により自動実装される。以下で使用される:
- `EneConfig::get_section::<T>()` / `set_section()` — 型安全なサブセクションアクセス
- `ConfigStore::get_section::<T>()` / `set_section()` — ストア経由の同じ操作
- `get_global_section::<T>()` — グローバルシングルトンから直接読み取り
- `update_section::<T>()` — load → patch → save を1呼び出しで実行

### `ConfigTarget`

```rust
pub enum ConfigTarget {
    Settings,   // settings.json を対象
    Character,  // character_settings.json を対象
}
```

どの JSON ファイルとスキーマを対象とするかを決定する。

### スキーマレジストリ

グローバルな `OnceLock<Mutex<HashMap<String, SchemaEntry>>>` が起動時に全設定スキーマを収集する:

```rust
pub struct SchemaEntry {
    pub schema: schemars::Schema,
    pub target: ConfigTarget,
    pub parent_key: Option<String>,  // None = トップレベル、Some("tools_map") = ツール設定
}
```

登録関数:

| 関数 | 呼び出し元 | 目的 |
|------|-----------|------|
| `__register_schema::<T>(target, parent_key)` | `define_config!` の `#[ctor]` | settings/character セクションスキーマの登録 |
| `__register_tool_schema::<T>(tool_name)` | `define_tool_config!` の `#[ctor]` | ツール固有設定スキーマの登録 |
| `register_runtime_schema(key, schema_json)` | ランタイム（例: MCP ツールプロバイダ） | 動的スキーマ登録 |

`generate_schema_json()` 中で、レジストリがルート `EneConfig` スキーマにマージされる:
- **トップレベルセクション**（`parent_key = None`）はルートスキーマの `properties` に追加される。
- **ツール設定**（`parent_key = "tools_map"`）は `ToolConfig` の `list` プロパティに `allOf: [ToolEntry, <ツールスキーマ>]` として注入される。
- 各エントリの **定義**（`$defs`）はルートスキーマの definitions にコピーされる。

### `ConfigStore`

自動保存のためのダーティトラッキング付き中央永続化レイヤー:

```rust
pub struct ConfigStore {
    config: RwLock<EneConfig>,
    character_config: RwLock<CharacterConfig>,
    global_dirty: AtomicBool,
    character_dirty: AtomicBool,
}
```

主要メソッド:

| メソッド | 説明 |
|---------|------|
| `ConfigStore::load()` | figment パイプラインでディスクから読み込み |
| `config()` / `set_config()` | グローバル設定の取得/置換 |
| `with_config_mut(f)` | クロージ経由の変更アクセス（自動でダーティマーク） |
| `get_section::<T>()` / `set_section(&T)` | 型安全なセクション読み書き |
| `character_config()` / `set_character_config()` | キャラクター個別設定のアクセス |
| `load_character_config(name)` | ディスクからキャラクター設定を読み込み |
| `flush_if_dirty(name)` | 変更時のみディスクに保存（`Ok(true)` を書き込み時返す） |
| `flush(name)` | ダーティ状態に関わらず強制保存 |
| `is_dirty()` | いずれかの設定に未保存の変更があるかチェック |

ゲームループ（例: Bevy）での典型的な使用法:

```rust
fn auto_save(store: Res<ConfigStore>, character: Res<CharacterName>) {
    let _ = store.flush_if_dirty(Some(&character.0));
}
```

### 新しい設定セクションの追加手順

1. 適切なクレートで `define_config!(settings, "my_key", ...)` を使って構造体を定義する。
2. **`cargo build`** を実行 — `#[ctor]` がスキーマを自動登録する。
3. **`cargo run -p ene-cli`** を1回実行し、`assets/schema/settings.schema.json` を再生成する。
4. `docs/reference/configuration/settings.md` と `docs/ja/reference/configuration/settings.md` に新しいセクションをドキュメントする。
5. `config.get_section::<MyConfig>()` または `store.get_section::<MyConfig>()` でアクセスする。

## デバッグオーバーレイ（セッション毎、永続化なし）

以下のオーバーレイは永続化される設定には含まれず、起動ごとにデフォルト（オフ）に戻ります。`UiState`（`apps/ene-desktop-v2/src/settings.rs` 内のランタイム状態）に保持され、Debug 設定ページまたは character ウィンドウのホットキーで切り替えます。

| オーバーレイ | デフォルト | ホットキー | 設定 UI | 効果 |
|------------|-----------|-----------|---------|------|
| **Raycast Colliders (Debug)** | `false` (オフ) | `F3` | Debug ページの「Raycast Colliders (Debug)」チェックボックス | PR5.2 のボーンコライダーごとにワイヤーフレームの球体（アイドル時はシアン、カーソル下のコライダーは黄）とレイキャストヒット地点の 3 軸クロス（赤）を描画する。`ene_vrm::DebugRenderer`（line-list、3D 深度テスト有効）で構築。 |
| **Input Region (Debug)** | `false` (オフ) | `F9` | Debug ページの「Input Region (Debug)」チェックボックス | OS のディスプレイサーバー（Wayland/X11）に送信された実際の入力領域の矩形をオレンジのワイヤーフレームとして描画する（空/固定/ウィンドウ全域などの特殊モード時は赤/緑/黄の枠線を表示）。 |
| **Mask Overlay (Debug)** | `false` (オフ) | なし | Debug ページの「Mask Overlay (Debug)」チェックボックス（Linux のみ） | オフスクリーンマスクキャプチャのワイヤーフレーム矩形を紫色の線で描画する（Linux のみ）。 |
