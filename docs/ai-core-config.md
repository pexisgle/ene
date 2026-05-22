# 設定システム

全設定は 1 つの JSON ファイルに集約される。`ene-ai-core::paths::config_file_path()` から読み込む。

## AiSettings（ルート構造）

```rust
pub struct AiSettings {
    pub provider_name: String,              // デフォルト "openai-compatible"
    pub model: String,                      // デフォルト "gpt-4o-mini"
    pub base_url: String,
    pub api_key: String,
    pub character_card_path: String,
    pub user_name: String,                  // デフォルト "User"
    pub runtime_rules: String,
    pub tool_calling_enabled: bool,         // デフォルト true
    pub max_tool_call_rounds: usize,        // デフォルト 10
    pub mcp_servers: Vec<McpServerConfig>,
    pub memory: AiMemorySettings,
    pub sandbox: AiSandboxSettings,
    pub tools: AiToolSettings,
}
```

## AiMemorySettings

| フィールド | 型 | デフォルト | 説明 |
|-----------|-----|-----------|------|
| `enabled` | bool | - | 長期記憶の有効/無効 |
| `db_path` | String | - | 明示的なDBパス（未指定時はカードディレクトリに自動生成） |
| `embedding_provider_type` | enum | `Local` | `Api` / `Local` |
| `embedding_model` | String | - | API埋め込みモデル名 |
| `embedding_base_url` | String | - | チャットAPIと別の場合のみ指定 |
| `embedding_dimensions` | usize | - | 埋め込みベクトル次元数 |
| `gguf_quantization` | String | - | GGUF量子化設定 |
| `recall_limit` | usize | - | 要約検索件数上限 |
| `similarity_threshold` | f32 | - | 類似度フィルタ閾値 |
| `session_timeout_minutes` | u64 | 30 | セッションタイムアウト |
| `topic_change_threshold` | f32 | 0.5 | トピック変化検出閾値（コサイン類似度） |
| `min_turns_before_split` | usize | 3 | 分割実行最小ターン数 |
| `summary_recall_limit` | usize | 3 | プロンプト注入する要約数 |
| `tool_rag_enabled` | bool | true | Tool RAG 有効/無効 |
| `tool_rag_limit` | usize | 6 | ツール絞り込み件数 |
| `tool_rag_always_include` | Vec\<String\> | ["question","todo","get_current_time"] | 常時含めるツール |
| `summarization_model` | String | - | 要約用モデル（未指定時はチャットモデル） |
| `summarization_base_url` | String | - | 要約用API URL |

## AiSandboxSettings

| フィールド | デフォルト |
|-----------|-----------|
| `enabled` | true |
| `allowed_directories` | 未設定 |
| `writable_directories` | 未設定 |
| `blocked_commands` | rm -rf /, dd if=, mkfs, フォークボム |
| `max_read_bytes` | 50KB |
| `max_write_bytes` | 1MB |
| `shell_timeout_ms` | 120s |
| `max_shell_output_bytes` | 50KB |
| `max_shell_output_lines` | 2000 |

## AiToolSettings

```rust
pub struct AiToolSettings {
    pub enabled: Vec<String>,  // デフォルト ["fs", "web", "browser", "utility", "app"]
}
```

## McpServerConfig

```rust
pub struct McpServerConfig {
    pub name: String,
    pub enabled: bool,
    pub transport: McpTransport,  // Stdio { command, args } / Http { url }
}
```
