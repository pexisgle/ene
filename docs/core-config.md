# 設定システム

ene の設定は `assets/settings.json` に集約され、`settings.schema.json` が自動生成される。  
読み込みは `ene_config::load_full_settings()`（もしくは `load_settings()`）で行う。

## 1. 基本構造（`EneSettings`）

`EneSettings` は最小限の共通フィールドと、拡張セクションを持つ。

```rust
pub struct EneSettings {
    pub version: u32,          // 現在 1
    pub character: String,     // キャラカードのパス or キャラ名
    pub user_name: String,     // デフォルト "User"
    pub runtime_rules: String, // 既定のシステム指示
    pub extra: HashMap<String, serde_json::Value>, // 各セクション
}
```

### character の解決
`load_full_settings()` は `character` を以下のルールで解決する。
- 空文字 → `assets_dir/characters/Alicia/character.json`
- パス区切りを含まない文字列 → `assets_dir/characters/{name}/character.json`

## 2. セクション構成（settings.json のキー）

| セクション | 型 | 内容 |
|---|---|---|
| `provider` | `ProviderSettings` | LLM 接続（base_url / api_key / model） |
| `embedding` | `EmbeddingConfig` | 埋め込み設定 |
| `memory` | `MemoryConfig` | 長期記憶 + Tool RAG |
| `session` | `SessionConfig` | 自動分割の閾値 |
| `sandbox` | `SandboxConfigData` | ツール用サンドボックス |
| `tools` | `ToolSettings` | ツール呼び出しと有効化 |
| `mcp_servers` | `Vec<McpServerConfig>` | MCP 接続設定 |
| `desktop` | `DesktopSection` | GUI のグラフィック設定 |

### 2.1 provider（LLM 接続）
```rust
pub struct ProviderSettings {
    pub provider_name: String = "openai-compatible",
    pub model: String = "gpt-4o-mini",
    pub base_url: String = "",
    pub api_key: String = "",
}
```

- `resolve_base_url()`：空なら `MissingBaseUrl` エラー
- `resolve_api_key()`：`settings.json` → (debugのみ) `API_TOKEN` の順

### 2.2 embedding
```rust
pub struct EmbeddingConfig {
    pub provider_type: EmbeddingProviderType = Local,
    pub model: String = "jina-embeddings-v5-text-small",
    pub base_url: String = "",
    pub dimensions: Option<usize> = None,
    pub gguf_quantization: String = "F16",
}
```

### 2.3 memory
```rust
pub struct MemoryConfig {
    pub enabled: bool = false,
    pub db_path: String = "",
    pub recall_limit: usize = 5,
    pub similarity_threshold: f32 = 0.5,
    pub time_decay_hours: f64 = 24.0,
    pub similarity_weight: f64 = 0.7,
    pub recency_weight: f64 = 0.3,
    pub tool_rag_enabled: bool = true,
    pub tool_rag_limit: usize = 6,
    pub tool_rag_always_include: Vec<String> = ["question", "todo", "get_current_time"],
    pub summarization_model: String = "",
    pub summarization_base_url: String = "",
}
```

### 2.4 session
```rust
pub struct SessionConfig {
    pub auto_session_split: bool = true,
    pub session_timeout_minutes: u64 = 30,
    pub topic_change_threshold: f32 = 0.5,
    pub min_turns_before_split: usize = 3,
    pub summary_recall_limit: usize = 3,
}
```

### 2.5 sandbox（ツール用）
`SandboxConfigData` は `ene-tool-proto` で定義され、IPC 経由で各ツールに配布される。

```rust
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
    pub undo_db_path: Option<String>,
}
```

### 2.6 tools
```rust
pub struct ToolSettings {
    pub tool_calling_enabled: bool = true,
    pub max_tool_call_rounds: usize = 10,
    pub tools: HashMap<String, ToolEntry>,
}

pub struct ToolEntry {
    pub enable: bool = true,
    #[serde(flatten)]
    pub config: serde_json::Value, // ツール固有の追加設定
}
```

`tools` は **ツール名 -> ToolEntry のマップ**。  
新しいツールを有効化する場合は `tools.tools` にエントリを追加する。

### 2.7 mcp_servers
```rust
pub struct McpServerConfig {
    pub name: String,
    pub enabled: bool,
    pub transport: McpTransport,
}

pub enum McpTransport {
    Stdio { command: String, args: Vec<String> },
    Http { url: String },
}
```

### 2.8 desktop（GUI）
`apps/ene-desktop/src/app_config.rs` の `DesktopSection`。

```rust
pub struct DesktopSection {
    pub graphics: GraphicsSection,
}
```

`graphics` は `mask_render_downsample` / `target_fps` / `shadow_quality` / `antialiasing_mode` を持つ。

## 3. 読み込みフロー

`ene_config::load_full_settings()` の順序:

1. `EneSettings::default()`
2. `assets/settings.json`
3. 環境変数（`ENE_` プレフィックス、`__` 区切りでネスト指定）

読み込み後、`settings.schema.json` と `character_settings.schema.json` が自動生成される。

## 4. 設定ファイルの例

```json
{
  "$schema": "./settings.schema.json",
  "version": 1,
  "character": "Alicia",
  "user_name": "pexisgle",
  "provider": {
    "provider_name": "openai-compatible",
    "model": "google/gemma-4-31b-it",
    "base_url": "https://openrouter.ai/api/v1",
    "api_key": ""
  },
  "embedding": {
    "provider_type": "local",
    "model": "jina-embeddings-v5-text-small",
    "gguf_quantization": "F16"
  },
  "memory": { "enabled": true },
  "tools": {
    "tool_calling_enabled": true,
    "max_tool_call_rounds": 10,
    "tools": {
      "fs": { "enable": true },
      "web": { "enable": true },
      "browser": { "enable": true },
      "utility": { "enable": true },
      "app": { "enable": true }
    }
  }
}
```
