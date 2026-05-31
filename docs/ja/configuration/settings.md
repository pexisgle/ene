# 設定

ene の設定は `assets/settings.json` に集約されています。`settings.schema.json` が自動生成され、エディタでのバリデーションが可能です。

読み込み: `ene_config::load_full_settings()` がデフォルト値、ファイル、環境変数を解決します。

## トップレベル構造 (`EneConfig`)

```rust
pub struct EneConfig {
    pub version: u32,           // 現在 1
    pub character: String,      // キャラクターカードのパスまたは名前
    pub user_name: String,      // デフォルト "User"
    pub runtime_rules: String,  // デフォルトのシステム指示
    pub extra: HashMap<String, serde_json::Value>, // セクションマップ
}
```

### キャラクター解決ルール
- 空文字 → `assets_dir/characters/Alicia/character.json`
- パス区切りを含まない文字列 → `assets_dir/characters/{name}/character.json`

## セクション

### `provider` — LLM 接続

```json
{
  "provider": {
    "provider_name": "openai-compatible",
    "model": "gpt-4o-mini",
    "base_url": "https://api.openai.com/v1",
    "api_key": ""
  }
}
```

| フィールド | 型 | 説明 |
|-----------|------|------|
| `provider_name` | string | プロバイダ識別子 (default: `"openai-compatible"`) |
| `model` | string | モデル名 (default: `"gpt-4o-mini"`) |
| `base_url` | string | API エンドポイント (本番では必須) |
| `api_key` | string | API キー (debug 時は `API_TOKEN` 環境変数にフォールバック) |

### `embedding` — ベクトル埋め込み

```json
{
  "embedding": {
    "provider_type": "local",
    "model": "jina-embeddings-v5-text-small",
    "base_url": "",
    "dimensions": null,
    "gguf_quantization": "F16"
  }
}
```

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `provider_type` | `"api"` または `"local"` | `"local"` | バックエンド種別 |
| `model` | string | `"jina-embeddings-v5-text-small"` | モデル名 |
| `base_url` | string | `""` | API URL (API モードのみ) |
| `dimensions` | int or null | `null` | 出力次元数 |
| `gguf_quantization` | string | `"F16"` | GGUF 量子化レベル |

### `memory` — 長期記憶

```json
{
  "memory": {
    "enabled": false,
    "db_path": "",
    "recall_limit": 5,
    "similarity_threshold": 0.5,
    "time_decay_hours": 24.0,
    "similarity_weight": 0.7,
    "recency_weight": 0.3,
    "tool_rag_enabled": true,
    "tool_rag_limit": 6,
    "tool_rag_always_include": ["question", "todo", "get_current_time"],
    "summarization_model": "",
    "summarization_base_url": ""
  }
}
```

| 主要フィールド | 説明 |
|-------------|------|
| `enabled` | 長期記憶を有効化 |
| `db_path` | SQLite データベースパス (空 = デフォルト位置) |
| `recall_limit` | 1クエリあたりの最大呼び出し要約数 |
| `similarity_threshold` | 呼び出しの最小コサイン類似度 |
| `tool_rag_enabled` | 埋め込みベースのツールフィルタリングを有効化 |
| `tool_rag_limit` | RAG フィルタリングで返される最大ツール数 |
| `tool_rag_always_include` | 類似度に関わらず常に含めるツール |

### `session` — セッション管理

```json
{
  "session": {
    "auto_session_split": true,
    "session_timeout_minutes": 30,
    "topic_change_threshold": 0.5,
    "min_turns_before_split": 3,
    "summary_recall_limit": 3
  }
}
```

| フィールド | 説明 |
|-----------|------|
| `auto_session_split` | 自動セッション分割を有効化 |
| `session_timeout_minutes` | 分割前のアイドルタイムアウト |
| `topic_change_threshold` | 話題変化検出のコサイン類似度しきい値 |
| `min_turns_before_split` | 分割が発生する最小ターン数 |
| `summary_recall_limit` | プロンプトに注入する要約の最大数 (デフォルト: 3) |

### `sandbox` — ツールセキュリティ

```json
{
  "sandbox": {
    "enabled": true,
    "allowed_directories": ["/home/user/projects"],
    "writable_directories": ["/home/user/projects"],
    "blocked_commands": ["rm -rf /", "dd if=", "mkfs", "sudo"],
    "max_read_bytes": 51200,
    "max_write_bytes": 1048576,
    "shell_timeout_ms": 120000,
    "max_shell_output_bytes": 51200,
    "max_shell_output_lines": 2000,
    "undo_db_path": null
  }
}
```

### `tools` — ツール設定

```json
{
  "tools": {
    "tool_calling_enabled": true,
    "max_tool_call_rounds": 10,
    "tool_call_timeout_ms": 60000,
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

| フィールド | 説明 |
|-----------|------|
| `tool_calling_enabled` | 全ツールの関数呼び出しを有効化 |
| `max_tool_call_rounds` | ユーザーターンあたりの最大ツール呼び出し反復数 |
| `tool_call_timeout_ms` | 個別ツール呼び出しのタイムアウト (ミリ秒、デフォルト: 60000) |
| `tools.<name>.enable` | 特定のツールを有効/無効化 |
| `tools.<name>.config` | ツール固有の追加設定 |

### `mcp_servers` — Model Context Protocol

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

### `desktop` — GUI 設定

```json
{
  "desktop": {
    "graphics": {
      "mask_render_downsample": 1,
      "target_fps": 60,
      "shadow_quality": "medium",
      "antialiasing_mode": "msaa_4x"
    }
  }
}
```

## 読み込み順序

1. `EneConfig::default()`
2. `assets/settings.json`
3. 環境変数 (`ENE_` プレフィックス、`__` 区切りでネスト指定)

読み込み後、`settings.schema.json` と `character_settings.schema.json` が自動生成されます。
�
