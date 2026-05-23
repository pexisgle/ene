# 設定システム

全設定は 1 つの JSON ファイルに集約され、`settings.schema.json` でスキーマが定義されている。
`ene-ai-core::paths::config_file_path()` → `assets/settings.json` から読み込む。

CLI（`ene-cli`）と GUI（`ene-desktop`）でファイル構造は異なるため、各々の読み込み方法も異なる。

---

## 1. AiSettings（コア設定）

`ene-ai-core/src/config.rs` に定義。すべてのフィールドに `#[serde(default)]` が付いており、
JSON にないフィールドは Rust 側のデフォルト値が使われる。

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

### 1.1 AiMemorySettings

| フィールド | 型 | デフォルト | 説明 |
|-----------|-----|-----------|------|
| `enabled` | bool | `false` | 長期記憶の有効/無効 |
| `db_path` | String | `""` | 明示的なDBパス（未指定時はカードディレクトリに `memory.db` を自動生成） |
| `embedding_provider_type` | enum | `"local"` | `"api"` / `"local"` |
| `embedding_model` | String | `"jina-embeddings-v5-text-small"` | 埋め込みモデル名 |
| `embedding_base_url` | String | `""` | チャットAPIと別の場合のみ指定 |
| `embedding_dimensions` | Option\<usize\> | `null` | 埋め込みベクトル次元数 |
| `gguf_quantization` | String | `"F16"` | GGUF量子化設定 |
| `recall_limit` | usize | `5` | 記憶検索件数上限 |
| `similarity_threshold` | f32 | `0.5` | 類似度フィルタ閾値（コサイン類似度） |
| `time_decay_hours` | f64 | `24.0` | 時間減衰係数（時間） |
| `similarity_weight` | f64 | `0.7` | 類似度スコアの重み |
| `recency_weight` | f64 | `0.3` | 新しさスコアの重み |
| `auto_session_split` | bool | `true` | セッション自動分割の有効/無効 |
| `session_timeout_minutes` | u64 | `30` | 無操作タイムアウト（分） |
| `topic_change_threshold` | f32 | `0.5` | トピック変化検出閾値 |
| `min_turns_before_split` | usize | `3` | 分割実行最小ターン数 |
| `summary_recall_limit` | usize | `3` | プロンプト注入する過去要約の最大数 |
| `tool_rag_enabled` | bool | `true` | Tool RAG 有効/無効 |
| `tool_rag_limit` | usize | `6` | ツール絞り込み件数 |
| `tool_rag_always_include` | Vec\<String\> | `["question","todo","get_current_time"]` | 常時含めるツール |
| `summarization_model` | String | `""` | 要約用モデル（空ならチャットモデルを使用） |
| `summarization_base_url` | String | `""` | 要約用API URL（空ならチャット用 base_url を使用） |

### 1.2 AiSandboxSettings

| フィールド | 型 | デフォルト | 説明 |
|-----------|-----|-----------|------|
| `enabled` | bool | `true` | サンドボックスの有効/無効 |
| `allowed_directories` | Vec\<String\> | `["."]` | 読み取り許可ディレクトリ |
| `writable_directories` | Vec\<String\> | `["."]` | 書き込み許可ディレクトリ |
| `blocked_commands` | Vec\<String\> | 5 パターン | ブロックコマンド正規表現 |
| `max_read_bytes` | usize | `51200` (50KB) | 1回の読み取り上限 |
| `max_write_bytes` | usize | `1048576` (1MB) | 1回の書き込み上限 |
| `shell_timeout_ms` | u64 | `120000` (120s) | シェルコマンドタイムアウト |
| `max_shell_output_bytes` | usize | `51200` (50KB) | シェル出力バイト上限 |
| `max_shell_output_lines` | usize | `2000` | シェル出力行数上限 |

`to_sandbox_config_data(undo_db_path)` メソッドで `SandboxConfigData` に変換され、
IPC 経由でツールバイナリ（`ene-tools-fs`）に配送される。

### 1.3 AiToolSettings

```rust
/// ツール名 → ToolEntry のマップ
pub struct AiToolSettings {
    pub tools: HashMap<String, ToolEntry>,
}

pub struct ToolEntry {
    pub enable: bool,
    #[serde(flatten)]
    pub config: serde_json::Value,  // ツール固有の追加設定
}
```

`enable: true` のツールのみが起動される。`config` フィールドはツール固有の設定
（例: web ツールの `search_backends.tavily.api_key`）を保持する。

**デフォルトで有効なツール**: `fs`, `web`, `browser`, `utility`, `app`

### 1.4 McpServerConfig

```rust
pub struct McpServerConfig {
    pub name: String,
    pub enabled: bool,
    pub transport: McpTransport,  // Stdio { command, args } / Http { url }
}
```

### 1.5 APIキー解決

`resolve_api_key()` は以下の優先順位で解決する:

1. `settings.json` の `api_key` フィールド（空でない場合）
2. デバッグビルド時のみ、環境変数 `API_TOKEN`
3. 上記いずれも空 → 空文字列

.env ファイルは cargo の dotenv 系依存ではなく、`direnv` / `.envrc` 経由で読み込まれる。

---

## 2. AppSettings（GUIラッパー）

`ene-desktop` では `AiSettings` を内包する `AppSettings` 構造体でファイルを読み書きする。

```rust
struct AppSettings {
    version: u32,                          // 現在 1
    character: String,                     // 選択中のキャラクター名
    app: AppSection,                       // グラフィック設定
    #[serde(flatten)]
    ai: AiSettings,                        // すべての AI 設定（フラットに展開）
}
```

### app.graphics セクション

| フィールド | 型 | デフォルト | 選択肢 |
|-----------|-----|-----------|--------|
| `mask_render_downsample` | u32 | `8` | `4`, `6`, `8` |
| `target_fps` | u32 | `60` | `0`, `15`, `30`, `60`, `120` |
| `shadow_quality` | string | `"Medium"` | `"Low"`, `"Medium"`, `"High"` |
| `antialiasing_mode` | string | `"Fxaa"` | `"Off"`, `"Fxaa"`, `"Smaa"`, `"Taa"` |

---

## 3. 設定ファイルの実例

```json
{
  "$schema": "./settings.schema.json",
  "version": 1,
  "character": "Alicia",
  "app": {
    "graphics": {
      "mask_render_downsample": 8,
      "target_fps": 60,
      "shadow_quality": "Medium",
      "antialiasing_mode": "Fxaa"
    }
  },
  "base_url": "https://openrouter.ai/api/v1",
  "api_key": "",
  "user_name": "pexisgle",
  "model": "google/gemma-4-31b-it",
  "memory": {
    "enabled": true,
    "topic_change_threshold": 0.4,
    "gguf_quantization": "F16"
  }
}
```

---

## 4. 読み込みフロー

### 4.1 CLI（ene-cli）

`ene-cli/src/config.rs`:

1. `AiSettings::default()` で初期化
2. `config_file_path()` → `assets/settings.json` を読む
3. JSON → `serde_json::Value` → `AiSettings` にデシリアライズ（部分適用）
4. JSON の `character` フィールドから `assets/characters/{name}/character.json` を解決して `character_card_path` に設定
5. カードが未設定ならデフォルト `characters/Alicia/character.json`
6. `memory.enabled` が true なら `init_memory()` でメモリ初期化

### 4.2 GUI（ene-desktop）

`apps/ene-desktop/src/app_config.rs`:

1. `CharacterSettings::discover()` で `assets/characters/` 以下の全キャラクターをスキャン
2. デフォルト設定で初期化後、`load_from_file()` を呼ぶ
3. `config_file_path()` → JSON → `AppSettings` にデシリアライズ
4. `apply_to()` で `CharacterSettings` に反映（character 選択、graphics、AiSettings）
5. さらに `load_per_character_settings()` でキャラクター固有設定を読み込み
6. GUI 上で変更があった場合は `save()` → `settings.json` に書き戻し

### 4.3 設定ファイルの優先順位

```
settings.json の値（ファイルにあれば上書き）
  ↓
Rust デフォルト値（#[serde(default)] / Default impl）
  ↓
※ api_key のみデバッグビルド時に環境変数 API_TOKEN をフォールバック
```

---

## 5. パス解決

| メソッド | 戻り値 | 説明 |
|----------|--------|------|
| `resolve_base_url()` | `Result<String>` | 空なら `AiCoreError::MissingBaseUrl` |
| `resolve_api_key()` | `String` | settings → env `API_TOKEN`(debug only) の順 |
| `resolve_memory_db_path()` | `PathBuf` | `db_path` 指定時はそれ、未指定は `card_dir/memory.db` |
| `resolve_embedding_base_url()` | `Result<String>` | 未指定ならチャット用 base_url |
| `resolve_summarization_model()` | `String` | 未指定ならチャット用 model |
| `resolve_summarization_base_url()` | `Result<String>` | 未指定ならチャット用 base_url |
| `resolve_undo_db_path()` | `PathBuf` | `memory.db` と同じディレクトリの `undo.db` |

---

## 6. Per-Character Settings（キャラクター固有設定）

`assets/characters/{name}/character_settings.json` に保存される。

```rust
struct CharacterPerSettings {
    character_position: [f32; 3],        // [x, y, z]
    selected_motion_path: String,
    model_scale: f32,                     // デフォルト 1.0
    look_at_strength: f32,                // デフォルト 0.6
    default_motion: String,
    expressions: Option<serde_json::Value>,
}
```

キャラクター選択時に `load_per_character_settings()` で読み込まれ、
キャラクター切替時に `save_per_character_settings()` で書き出される。

---

## 7. スキーマバリデーション

`assets/settings.schema.json` が JSON Schema (draft-07) 形式で用意されており、
`$schema` 参照により IDE で補完・バリデーションが効く。

---

## 8. 設定ファイルの保存タイミング（GUI）

| トリガー | 保存内容 |
|----------|---------|
| キャラクター切替 | 現在キャラの `character_settings.json` 保存＋新キャラ読み込み |
| グラフィック設定変更 | `settings.json` 保存（即時） |
| キャラ位置/スケール変更 | `character_settings.json` 保存（即時） |
| AI設定変更 | `settings.json` 保存（即時） |
```
