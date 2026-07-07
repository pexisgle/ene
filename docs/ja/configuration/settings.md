# 設定

ene の設定は `assets/settings.json` に集約されています。`settings.schema.json` が自動生成され、エディタでのバリデーションが可能です。

読み込み: `ene_config::load_full_config()` がデフォルト値、ファイル、環境変数を解決します。

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

### `provider` — AI プロバイダ接続

```json
{
  "provider": {
    "name": "openai-compatible",
    "model": "gpt-4o-mini",
    "base_url": "https://api.openai.com/v1",
    "api_key": {
      "source": "inline",
      "inline": "",
      "env": "OPENAI_API_KEY"
    },
    "embedding": {
      "backend": "cloud",
      "query_prefix": null,
      "cloud": {
        "model": "text-embedding-3-small",
        "dimensions": 1536
      },
      "local": {
        "model": "jina-embeddings-v5-text-small",
        "quantization": "F16"
      }
    }
  }
}
```

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `name` | string | `"openai-compatible"` | プロバイダ識別子 |
| `model` | string | `"gpt-4o-mini"` | チャットモデル名 |
| `base_url` | string | `""` | API ベース URL |
| `api_key` | object | (下記参照) | API キー設定 |
| `embedding` | object | (下記参照) | 埋め込み設定 |

#### `provider.api_key` — API キー設定

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `source` | string | `"inline"` | キーソース: `"inline"` または `"env"` |
| `inline` | string | `""` | API キー (`source = "inline"` 時、注意して使用) |
| `env` | string | `"OPENAI_API_KEY"` | `source = "env"` 時の環境変数名 |

#### `provider.embedding` — 埋め込み設定

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `backend` | string | `"cloud"` | `"cloud"` はプロバイダの埋め込み API を使用、`"local"` はローカル GGUF モデルを使用 |
| `query_prefix` | string or null | `null` | 検索クエリに付加するプレフィックス |
| `cloud` | object | (下記参照) | クラウド埋め込みモデル設定 |
| `local` | object | (下記参照) | ローカル GGUF 埋め込み設定 |

##### `provider.embedding.cloud` — クラウド埋め込み設定

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `model` | string | `"text-embedding-3-small"` | クラウド埋め込みモデル名 |
| `dimensions` | int | `1536` | クラウド埋め込みベクトルの次元数 |

##### `provider.embedding.local` — ローカル埋め込み設定

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `model` | string | `"jina-embeddings-v5-text-small"` | ローカル GGUF 埋め込みモデル名 |
| `quantization` | string | `"F16"` | 量子化レベル (例: `"F16"`, `"Q4_K_M"`) |

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
    "recency_weight": 0.3
  }
}
```

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `enabled` | bool | `false` | 長期記憶を有効化 |
| `db_path` | string | `""` | SQLite データベースパス (空 = デフォルト位置) |
| `recall_limit` | int | `5` | 1クエリあたりの最大呼び出し要約数 |
| `similarity_threshold` | float | `0.5` | 呼び出しの最小コサイン類似度 |
| `time_decay_hours` | float | `24.0` | 直近性が減衰するまでの時間 (時間) |
| `similarity_weight` | float | `0.7` | 呼び出しランキングにおける類似度スコアの重み |
| `recency_weight` | float | `0.3` | 呼び出しランキングにおける直近性スコアの重み |

### `session` — セッション管理

```json
{
  "session": {
    "auto_split": true,
    "timeout_minutes": 30,
    "topic_similarity_threshold": 0.5,
    "min_turns_before_split": 3,
    "recall_limit": 3,
    "summarization": {
      "model": "",
      "base_url": ""
    }
  }
}
```

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `auto_split` | bool | `true` | 自動セッション分割を有効化 |
| `timeout_minutes` | int | `30` | 分割前のアイドルタイムアウト |
| `topic_similarity_threshold` | float | `0.5` | 話題変化検出のコサイン類似度しきい値 (0.0–1.0) |
| `min_turns_before_split` | int | `3` | 分割が発生する最小ターン数 |
| `recall_limit` | int | `3` | プロンプトに注入する要約の最大数 |
| `summarization` | object | (下記参照) | 要約モデルの設定 |

#### `session.summarization` — 要約モデル設定

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `model` | string | `""` | 要約に使用するモデル (空 = チャットモデルを使用) |
| `base_url` | string | `""` | 要約に使用するベース URL (空 = チャットベース URL を使用) |

### `tools` — ツール設定

```json
{
  "tools": {
    "enabled": true,
    "max_rounds": 10,
    "timeout_ms": 60000,
    "list": {
      "fs": { "enable": true },
      "web": { "enable": true },
      "browser": { "enable": true },
      "utility": { "enable": true },
      "app": { "enable": true }
    },
    "mcp_servers": [],
    "rag": {
      "enabled": true,
      "top_k": 12,
      "final_n": 6,
      "use_hyde": true,
      "use_rerank": true,
      "rerank_candidates": 24,
      "min_similarity": 0.25,
      "background_index_on_startup": false,
      "forced": [
        "utility.question",
        "utility.todo_add",
        "utility.get_current_time"
      ],
      "weights": {
        "summary": 1.0,
        "description": 0.6,
        "capability": 0.8,
        "example": 0.4,
        "negative": -0.5,
        "hyde": 0.7
      }
    }
  }
}
```

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `enabled` | bool | `true` | 全ツールの関数呼び出しを有効化 |
| `max_rounds` | int | `10` | ユーザーターンあたりの最大ツール呼び出し反復数 |
| `timeout_ms` | int | `60000` | 個別ツール呼び出しのタイムアウト (ミリ秒) |
| `list` | object | (下記参照) | ツール個別有効化マップとオプション設定 |
| `mcp_servers` | array | `[]` | MCP サーバーリスト |
| `rag` | object | (下記参照) | ツール RAG 設定 |

#### `tools.list` — ツール個別有効化マップ

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `<name>.enable` | bool | `true` | 特定のツールを有効/無効化 |
| `<name>.config` | object | `{}` | ツール固有の追加設定 (エントリにフラット化) |

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
| `name` | string | サーバー名 (表示とルーティングに使用) |
| `enabled` | bool | この MCP サーバーが有効かどうか |
| `transport` | object | トランスポート設定 (下記参照) |

**トランスポートタイプ:**

| タイプ | フィールド | 説明 |
|--------|-----------|------|
| `stdio` | `command`, `args` | stdio トランスポートで子プロセスを起動 |
| `http` | `url` | HTTP 経由で接続 |

#### `tools.rag` — ツール RAG パイプライン

ツール RAG は、トークン消費を削減するためにユーザー入力に関連するツールのみを動的に選択します。

```json
{
  "rag": {
    "enabled": true,
    "top_k": 12,
    "final_n": 6,
    "use_hyde": true,
    "use_rerank": true,
    "rerank_candidates": 24,
    "min_similarity": 0.25,
    "background_index_on_startup": false,
    "forced": ["utility.question", "utility.todo_add", "utility.get_current_time"],
    "weights": {
      "summary": 1.0,
      "description": 0.6,
      "capability": 0.8,
      "example": 0.4,
      "negative": -0.5,
      "hyde": 0.7
    }
  }
}
```

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `enabled` | bool | `true` | ツール RAG を有効化 |
| `top_k` | int | `12` | ベクトルインデックスから取得する候補数 |
| `final_n` | int | `6` | リランキング後に返される最終ツール数 |
| `use_hyde` | bool | `true` | 假説文書埋め込み (HyDE) でクエリを拡張 |
| `use_rerank` | bool | `true` | LLM ベースのリランキングを実行 |
| `rerank_candidates` | int | `24` | リランキングに渡す候補数 |
| `min_similarity` | float | `0.25` | ツールが考慮される最小類似度スコア |
| `background_index_on_startup` | bool | `false` | 起動時にバックグラウンドタスクでインデックスをウォーム |
| `forced` | string[] | `["utility.question", "utility.todo_add", "utility.get_current_time"]` | 関連性に関わらず常に含めるツール |
| `weights` | object | (下記参照) | マルチベクトル類似度計算のフィールド別重み |

##### `tools.rag.weights` — フィールド重み

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `summary` | float | `1.0` | ツールサマリ埋め込みの重み |
| `description` | float | `0.6` | ツール説明埋め込みの重み |
| `capability` | float | `0.8` | ツール能力埋め込みの重み |
| `example` | float | `0.4` | ツール例埋め込みの重み |
| `negative` | float | `-0.5` | ネガティブ/不要埋め込みの重み (一致をペナルティ) |
| `hyde` | float | `0.7` | HyDE (假説文書埋め込み) の重み |

### `web_config` — ウェブ検索プロバイダ

ウェブツールで使用されるウェブ検索プロバイダの API キーです。ツール固有の設定で、ランタイム時に注入されます。

```json
{
  "web_config": {
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

### `desktop` — GUI 設定

デスクトップアプリケーション固有の GUI 設定です。`ene-desktop` 実行時のみ利用可能です。

```json
{
  "desktop": {
    "graphics": {
      "mask_render_downsample": 1,
      "target_fps": 60,
      "shadow_quality": "medium",
      "antialiasing_mode": "msaa_4x",
      "debug_fps": 30
    }
  }
}
```

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `graphics.mask_render_downsample` | int | `1` | マスクレンダリングのダウンサンプル倍率 |
| `graphics.target_fps` | int | `60` | 目標フレームレート |
| `graphics.shadow_quality` | string | `"medium"` | シャドウ品質レベル |
| `graphics.antialiasing_mode` | string | `"fxaa"` | アンチエイリアシングモード |
| `graphics.debug_fps` | int | `30` | デバッグ描画更新レート（FPS、0は制限なし） |

### `cognition` — 認知ランタイム

Ene Cognitive Runtime の設定です。コンテキスト予算、記憶抽出・保持、感情処理、キャラクターコンパイルを制御します。

> **注意:** このセクションは [Ene Cognitive Runtime](../architecture/cognitive-runtime.md) 再設計の一部です。`cognition.enabled` が `true` の場合、認知ランタイムがレガシーストリーミングパイプラインを置き換えます（Phase 10 統合予定）。

```json
{
  "cognition": {
    "enabled": true,
    "context": {
      "max_prompt_tokens": 12000,
      "recent_turns": 8,
      "scene_summary_tokens": 800,
      "memory_budget_tokens": 1800,
      "semantic_budget_tokens": 1200,
      "style_example_budget_tokens": 600
    },
    "memory": {
      "write_every_turn": true,
      "hybrid_search": true,
      "decay_enabled": true,
      "default_forgetting_half_life_days": 30.0,
      "min_confidence_to_persist": 0.65,
      "extraction_timeout_secs": 30,
      "use_hyde": false,
      "recall_result_limit": 8,
      "recall_similarity_threshold": 0.35,
      "recall_min_score": 0.20,
      "rerank_enabled": false,
      "rerank_candidate_limit": 16,
      "rerank_timeout_secs": 10,
      "mmr_enabled": true,
      "mmr_lambda": 0.7,
      "mmr_duplicate_cluster_threshold": 0.75,
      "mmr_min_slots_semantic": 1,
      "mmr_min_slots_episodic": 1,
      "mmr_min_slots_user_profile": 1,
      "mmr_min_slots_commitment": 1,
      "mmr_source_diversity_bonus": 0.05,
      "require_migration": false
    },
    "emotion": {
      "enabled": true,
      "engine": "hybrid",
      "decay_half_life_minutes": 30.0,
      "expression_hysteresis_seconds": 4.0,
      "llm_can_propose_expression": true,
      "llm_expression_is_advisory": true,
      "classifier_timeout_secs": 15,
      "classifier_min_confidence": 0.5,
      "classifier_language": "ja"
    },
    "character": {
      "compile_ccv3_to_semantic_memory": true,
      "always_include_identity_kernel": true,
      "identity_kernel_max_tokens": 400,
      "style_retrieval": true
    }
  }
}
```

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `enabled` | bool | `true` | 認知ランタイムを有効にする。`false` の場合はレガシーストリーミングパイプラインにフォールバック。 |

#### `cognition.context` — コンテキスト予算

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `max_prompt_tokens` | int | `12000` | 全セクションの最大プロンプトトークン数 |
| `recent_turns` | int | `8` | プロンプトに含める直近の会話ターン数 |
| `scene_summary_tokens` | int | `800` | シーン・サマリーセクションのトークン予算 |
| `memory_budget_tokens` | int | `1800` | 想起記憶のトークン予算 |
| `semantic_budget_tokens` | int | `1200` | 意味記憶（lorebook）のトークン予算 |
| `style_example_budget_tokens` | int | `600` | CCv3 lorebook からのスタイル例のトークン予算 |
| `compression_enabled` | bool | `true` | 認知ランタイム有効時にセッション分割の代わりに rolling context compression を使う |
| `scene_turn_threshold` | int | `12` | シーンレベル圧縮を開始するターン数 |
| `chapter_span_threshold` | int | `5` | チャプター rollup 前のシーン span 数 |
| `arc_span_threshold` | int | `3` | アーク rollup 前のチャプター span 数 |
| `compression_timeout_secs` | int | `60` | 圧縮要約 LLM 呼び出しのタイムアウト（秒） |

#### `cognition.memory` — 記憶抽出・保持

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `write_every_turn` | bool | `true` | 毎ターン記憶を抽出し永続化する |
| `hybrid_search` | bool | `true` | ハイブリッド検索（ベクトル + 新しさ + 顕著性 + 信頼度）を使用 |
| `decay_enabled` | bool | `true` | post-turn の自然減衰（`Active → Faded → Archived`）を `ForgettingLifecycle` で有効化 |
| `default_forgetting_half_life_days` | float | `30.0` | ライフサイクル減衰スコアと recall 新しさスコアの半減期（日） |
| `min_confidence_to_persist` | float | `0.65` | 記憶永続化の最低信頼度しきい値（0.0〜1.0） |
| `extraction_timeout_secs` | int | `30` | LLM 記憶抽出呼び出し 1 回のタイムアウト（秒）。超過時は抽出失敗となり deterministic 候補にフォールバック |
| `use_hyde` | bool | `false` | cognitive recall plan に HyDE query expansion hint を記録する。実際の provider 呼び出しは後続の recall execution が担当 |
| `recall_result_limit` | int | `8` | `RecallPlan` が要求する型付き記憶結果の最大数 |
| `recall_similarity_threshold` | float | `0.35` | vector 由来 recall candidate の最低類似度 |
| `recall_min_score` | float | `0.20` | recalled memory result に必要な最低 hybrid score |
| `rerank_enabled` | bool | `false` | hybrid recall 候補に対する optional LLM rerank を有効化 |
| `rerank_candidate_limit` | int | `16` | reranker に渡す hybrid-search 上位候補の最大数 |
| `rerank_timeout_secs` | int | `10` | LLM memory rerank 呼び出し 1 回のタイムアウト（秒）。超過時または provider 失敗時は hybrid search 順序にフォールバック |
| `mmr_enabled` | bool | `true` | hybrid search 後の MMR 多様化を有効化（#78）。既定 true のため、recall 候補の順序は pure hybrid スコア順と異なる場合がある |
| `mmr_lambda` | float | `0.7` | MMR の relevance 対 diversity トレードオフ（`0.0`–`1.0`）。高いほど relevance 優先 |
| `mmr_duplicate_cluster_threshold` | float | `0.75` | 近傍重複 recall 候補をマージする lexical 類似度しきい値 |
| `mmr_min_slots_semantic` | int | `1` | semantic 記憶の最低 recalled 枠 |
| `mmr_min_slots_episodic` | int | `1` | episodic 記憶の最低 recalled 枠 |
| `mmr_min_slots_user_profile` | int | `1` | user profile 記憶の最低 recalled 枠 |
| `mmr_min_slots_commitment` | int | `1` | commitment 記憶の最低 recalled 枠 |
| `mmr_source_diversity_bonus` | float | `0.05` | 新しい recall source 種別を持つ候補に加算する MMR ボーナス |
| `require_migration` | bool | `false` | true のとき、レガシー summaries/keyfacts が残り migration 未完了なら typed recall をブロック（通常の `conversation_logs` だけではブロックしない）(#98) |

#### `cognition.emotion` — 感情エンジン

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `enabled` | bool | `true` | 感情処理を有効化 |
| `engine` | string | `"hybrid"` | エンジンモード: `"deterministic"`, `"llm"`, または `"hybrid"` |
| `decay_half_life_minutes` | float | `30.0` | 感情減衰の半減期（分） |
| `expression_hysteresis_seconds` | float | `4.0` | 表情変更の最小間隔（秒）（ちらつき防止） |
| `llm_can_propose_expression` | bool | `true` | LLM が表情トークンを提案することを許可 |
| `llm_expression_is_advisory` | bool | `true` | LLM の表情提案をコマンドではなくアドバイスとして扱う |
| `classifier_timeout_secs` | int | `15` | オプション LLM 感情分類器のタイムアウト（秒）（#88） |
| `classifier_min_confidence` | float | `0.5` | LLM 感情デルタ適用に必要な最小 confidence |
| `classifier_language` | string | `"en"` | 感情分類器と自然対話出力契約のプロンプト言語（`en` または `ja`） |

#### `cognition.character` — キャラクターコンパイル

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `compile_ccv3_to_semantic_memory` | bool | `true` | CCv3 lorebook エントリを意味記憶インデックスにコンパイル |
| `always_include_identity_kernel` | bool | `true` | 全プロンプトの先頭に必ず Identity Kernel を含める |
| `identity_kernel_max_tokens` | int | `400` | コンパイル済み Identity Kernel の概算トークン予算（#82）。詳細セクションから優先的に削り、コア見出し行は保持 |
| `style_retrieval` | bool | `true` | lorebook からキャラクタースタイル例の検索を有効化 |

## ツール固有の設定

ツール固有の設定は `tools.tools.<name>.config` 内に格納され、ツールごとに異なります。

### `tools.tools.fs.config` — サンドボックス設定

`fs` ツールはファイルシステムアクセスのサンドボックスコントロールを提供します:

```json
{
  "tools": {
    "tools": {
      "fs": {
        "enable": true,
        "config": {
          "enabled": true,
          "allowed_directories": ["/home/user/projects"],
          "writable_directories": ["/home/user/projects"],
          "blocked_commands": ["rm -rf /", "dd if=", "mkfs", "sudo"],
          "max_read_bytes": 51200,
          "max_write_bytes": 1048576,
          "shell_timeout_ms": 120000,
          "max_shell_output_bytes": 51200,
          "max_shell_output_lines": 2000
        }
      }
    }
  }
}
```

| フィールド | 型 | デフォルト | 説明 |
|-----------|------|---------|------|
| `enabled` | bool | `true` | サンドボックスを有効化 |
| `allowed_directories` | string[] | `["."]` | 読み取りアクセスが許可されたディレクトリ |
| `writable_directories` | string[] | `["."]` | 書き込みアクセスが許可されたディレクトリ |
| `blocked_commands` | string[] | (コード参照) | ブロックされたシェルコマンドの正規表現パターン |
| `max_read_bytes` | int | `51200` | 1回の読み取り操作の最大バイト数 |
| `max_write_bytes` | int | `1048576` | 1回の書き込み操作の最大バイト数 |
| `shell_timeout_ms` | int | `120000` | シェルコマンドのタイムアウト (ミリ秒) |
| `max_shell_output_bytes` | int | `51200` | シェル出力の最大バイト数 |
| `max_shell_output_lines` | int | `2000` | シェル出力の最大行数 |

## 読み込み順序

1. `EneConfig::default()` — コンパイル時デフォルト値
2. `assets/settings.json` — ユーザーオーバーライド
3. 環境変数 (`ENE_` プレフィックス、`__` 区切りでネスト指定)

読み込み後、`settings.schema.json` と `character_settings.schema.json` が `assets/schema/` に自動生成されます。

## JSON スキーマ

`settings.schema.json` は `cargo run -p ene-cli` (または任意のビルド) 時に自動生成され、`assets/schema/settings.schema.json` に書き出されます。このファイルは gitignored です — コミットや手動編集はしないでください。

スキーマはエディタバリデーション (VS Code の `"json.schemas"` 設定) やプログラマティックな設定構築に使用できます。

## 設定登録 API

設定システムは宣言的マクロとグローバルスキーマレジストリ上に構築されています。各設定セクションはマクロで定義され、`Serialize`、`Deserialize`、`JsonSchema`、`Default`、`HasConfigKey` の実装が自動生成され、`#[ctor]` によりプログラム起動時にスキーマが登録されます。

### `define_config!`

設定構造体を定義するためのメインマクロ。3つのフォームがあります。

#### トップレベル settings セクション

```rust
ene_config::define_config!(
    settings,          // ターゲット: ConfigTarget::Settings
    "provider",        // EneConfig.extra の JSON キー
    /// AI プロバイダ接続設定。
    pub struct ProviderConfig {
        /// プロバイダ名。
        pub name: String = "openai-compatible".to_string(),
        /// チャットモデル名。
        pub model: String = "gpt-4o-mini".to_string(),
    }
);
```

生成されるもの:
- `#[derive(Serialize, Deserialize, JsonSchema)]` + `#[serde(rename_all = "snake_case", default)]`
- インライン `= default_value` 構文による `impl Default` (省略時は `Default::default()`)
- `impl HasConfigKey`: `KEY = "provider"`, `TARGET = Settings`, `path() = ["provider"]`
- `__register_schema::<ProviderConfig>(Settings, None)` を呼ぶ `#[ctor]` 関数

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

#### ネストされたセクション (親構造体の子)

```rust
ene_config::define_config!(
    EmbeddingConfig,   // 親構造体 (HasConfigKey を実装していること)
    "local",           // provider.embedding.* の JSON キー
    pub struct LocalEmbeddingConfig {
        pub model: String = "jina-embeddings-v5-text-small".to_string(),
        pub quantization: String = "F16".to_string(),
    }
);
```

親から `TARGET` を継承する。`path()` は親のパス + 自身のキーを返す (例: `["provider", "local_embedding"]`)。`#[ctor]` 呼び出しに親キーが渡され、スキーマが正確にネストされる。

### `define_tool_config!`

ツール固有の設定スキーマ用 (`tools.tools.<name>.config` に注入):

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

同じ derive/デフォルト生成を行うが、`__register_tool_schema::<T>("fs")` を呼ぶ。スキーマは `parent_key = "tools_map"` で登録され、生成される JSON スキーマの `ToolConfig` 定義の `tools` プロパティにマージされる。

### `HasConfigKey` トレイト

```rust
pub trait HasConfigKey {
    const KEY: &'static str;       // JSON キー (例: "provider")
    const TARGET: ConfigTarget;    // Settings または Character
    fn path() -> &'static [&'static str]; // ルートからのフルパス (例: ["provider", "local_embedding"])
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
| `register_runtime_schema(key, schema_json)` | ランタイム (例: MCP ツールプロバイダ) | 動的スキーマ登録 |

`generate_schema_json()` 中で、レジストリがルート `EneConfig` スキーマにマージされる:
- **トップレベルセクション** (`parent_key = None`) はルートスキーマの `properties` に追加される。
- **ツール設定** (`parent_key = "tools_map"`) は `ToolConfig` の `tools` プロパティに `allOf: [ToolEntry, <ツールスキーマ>]` として注入される。
- 各エントリの **定義** (`$defs`) はルートスキーマの definitions にコピーされる。

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
| `with_config_mut(f)` | クロージ経由の変更アクセス (自動でダーティマーク) |
| `get_section::<T>()` / `set_section(&T)` | 型安全なセクション読み書き |
| `character_config()` / `set_character_config()` | キャラクター個別設定のアクセス |
| `load_character_config(name)` | ディスクからキャラクター設定を読み込み |
| `flush_if_dirty(name)` | 変更時のみディスクに保存 (`Ok(true)` を書き込み時返す) |
| `flush(name)` | ダーティ状態に関わらず強制保存 |
| `is_dirty()` | いずれかの設定に未保存の変更があるかチェック |

ゲームループ (例: Bevy) での典型的な使用法:

```rust
fn auto_save(store: Res<ConfigStore>, character: Res<CharacterName>) {
    let _ = store.flush_if_dirty(Some(&character.0));
}
```

### 新しい設定セクションの追加手順

1. 適切なクレートで `define_config!(settings, "my_key", ...)` を使って構造体を定義する。
2. **`cargo build`** を実行 — `#[ctor]` がスキーマを自動登録する。
3. **`cargo run -p ene-cli`** を1回実行し、`assets/schema/settings.schema.json` を再生成する。
4. `docs/configuration/settings.md` と `docs/ja/configuration/settings.md` に新しいセクションをドキュメントする。
5. `config.get_section::<MyConfig>()` または `store.get_section::<MyConfig>()` でアクセスする。

## デバッグオーバーレイ (セッション毎、永続化なし)

以下のオーバーレイは永続化される設定には含まれず、起動ごとにデフォルト (オフ) に戻ります。`UiState` (`apps/ene-desktop-v2/src/settings.rs` 内のランタイム状態) に保持され、Debug 設定ページまたは character ウィンドウのホットキーで切り替えます。

| オーバーレイ | デフォルト | ホットキー | 設定 UI | 効果 |
|------------|-----------|-----------|---------|------|
| **Raycast Colliders (Debug)** | `false` (オフ) | `F3` | Debug ページの「Raycast Colliders (Debug)」チェックボックス | PR5.2 のボーンコライダーごとにワイヤーフレームの球体 (アイドル時はシアン、カーソル下のコライダーは黄) とレイキャストヒット地点の 3 軸クロス (赤) を描画する。`ene_vrm::DebugRenderer` (line-list、3D 深度テスト有効) で構築。 |
| **Input Region (Debug)** | `false` (オフ) | `F9` | Debug ページの「Input Region (Debug)」チェックボックス | OS のディスプレイサーバー (Wayland/X11) に送信された実際の入力領域の矩形をオレンジのワイヤーフレームとして描画する (空/固定/ウィンドウ全域などの特殊モード時は赤/緑/黄の枠線を表示)。 |
| **Mask Overlay (Debug)** | `false` (オフ) | なし | Debug ページの「Mask Overlay (Debug)」チェックボックス (Linux のみ) | オフスクリーンマスクキャプチャのワイヤーフレーム矩形を紫色の線で描画する (Linux のみ)。 |
