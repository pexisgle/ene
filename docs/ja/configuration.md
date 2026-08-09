# 設定

Ene はグローバル設定ファイル 1 つと、キャラクターごとの設定ファイルを
読みます。すべての値にデフォルトがあるため、空の設定でも動作します。

## 設定ファイル

| ファイル | 目的 |
|---|---|
| `settings.json` | グローバル設定（AI プロバイダー・mind・store・ツール・プラグイン・デスクトップ）。アセットディレクトリに置かれます。 |
| `assets/characters/<name>/character.json` | キャラクターカード本体（[キャラクターカード](concepts/character-cards.md)参照）。 |
| `assets/characters/<name>/character_settings.json` | キャラクターごとの表示設定（位置・スケール・デフォルトモーション/表情・カード言語）。 |
| `assets/characters/<name>/character.<lang>.json` | 任意のローカライズ差分（[ローカライズ](concepts/character-cards.md#ローカライズ)参照）。 |
| `assets/lang/<lang>/prompts.json`, `patterns.json` | 実行時プロンプトパック・忘却パターンパック（埋め込みコピーにフォールバック）。 |

### アセットディレクトリの場所

- デバッグビルド: リポジトリの `assets/`（変更が即反映されます）。
- リリースビルド: OS のアプリケーションデータディレクトリ
  （Linux は `~/.local/share/ene`、Windows は `%APPDATA%\ene`）。

デスクトップアプリと CLI はどちらも `--config <path>` で別の
`settings.json` を指定できます。CLI は `--character <name>` と
`--lang <en|ja>` も受け付けます。

## 優先順位

設定は次の順でマージされます（後が優先）:

1. 組み込みデフォルト
2. ディスク上の `settings.json`
3. `ENE_*` 環境変数（ネストは `__` で区切り）

例:

```sh
ENE_AI__TASKS__CHAT__MODEL="openai/gpt-5.6-luna"
ENE_MIND__EMOTION__ENABLED="false"
ENE_TOOLS__LIST__WEB__ENABLE="false"
```

環境変数は `settings.json` にある任意のキーを上書きできます（値が JSON と
して解釈できる場合はセクション全体も指定可能）。

## スキーマと検証

各設定セクションは、所有クレート（`ene-ai`・`ene-mind`・`ene-store`・
`ene-plugin-host`・`apps/ene-desktop` など）の `define_config!` 呼び出しで
定義されます。起動時に Ene は JSON Schema を `assets/schema/`
（`settings.schema.json`・`character_settings.schema.json` など）へ再生成する
ため、エディタで補完・検証が効きます。これらのスキーマファイルは生成物で、
Git 管理されません。

`settings.json` は `version` フィールドを持ち、古いファイルは読み込み時に
自動で前方マイグレーションされます（`ene-config` のマイグレーション）。
後方互換のマイグレーションはなく、ファイルはその場で更新されます。

## トップレベルキー

| キー | 型 | デフォルト | 意味 |
|---|---|---|---|
| `$schema` | string | — | エディタ用スキーマポインタ（保存時に自動入力）。 |
| `version` | number | 1 | 設定スキーマバージョン。自動マイグレーション。 |
| `character` | string | `"Alicia"` | 読み込むキャラクターカード名（またはパス）。 |
| `user_name` | string | `"User"` | プロンプトに使う表示名（`{{user}}`）。 |
| `runtime_rules` | string | 組み込み | すべてのシステムプロンプトに注入される行動規則。 |
| `user_persona` | object | — | 構造化ユーザーペルソナ。`{{user_persona}}` を展開。 |
| `ai` | object | 下記 | プロバイダー・タスク・リトライ・フォールバック・TTS/STT/VAD。 |
| `mind` | object | 下記 | 感情・プロアクティブ・メモリ上限・トピック境界・セッション。 |
| `store` | object | `{ "enabled": true }` | メモリストアの有効化。 |
| `tools` | object | 下記 | ツールの有効化・MCP サーバー・ツール RAG。 |
| `plugins` | object | 下記 | プロバイダー/ツールプラグインの一覧と個別設定。 |
| `desktop` | object | 下記 | デスクトップ専用設定（グラフィック・言語・字幕など）。 |

未知のトップレベルキーは保存時も保持されます（ラウンドトリップ安全）。

## `ai.*` — AI プロバイダーとタスク

```json
{
  "ai": {
    "providers": {
      "openrouter": {
        "kind": "openai_compatible",
        "base_url": "https://openrouter.ai/api/v1",
        "api_key": { "source": "env", "env": "OPENROUTER_API_KEY", "inline": "" },
        "context_window": null
      }
    },
    "tasks": {
      "chat":       { "provider": "openrouter", "model": "openai/gpt-5.6-luna", "max_tokens": 8192, "supports_vision": true },
      "classifier": { "provider": "openrouter", "model": "openai/gpt-5.6-luna" },
      "embedding":  { "provider": "local",      "model": "jina-v5-small" },
      "proactive":  { "provider": "local",      "model": "gemma-4-e4b" }
    },
    "retry":     { "max_attempts": 3, "base_delay_ms": 500, "max_delay_ms": 30000, "timeout_ms": 120000 },
    "fallback":  { "enabled": false, "health_check_timeout_ms": 5000, "cache_ttl_ms": 60000, "max_history": 32 },
    "tts":       { "provider": "kokoro", "model": "kokoro-v1_0.onnx", "voice": "af_heart", "speed": 1.0, "language": "ja" },
    "stt":       { "provider": "whisper", "model": "", "language": "" },
    "vad":       { "provider": "none" }
  }
}
```

- **`ai.providers`** — 名前付きプロバイダー定義。`kind` はプロバイダー種別
  （`openai`・`openai_compatible`・`anthropic`・`local` など）。`api_key` は
  `source: "env"`（指定した環境変数から読む）・`source: "inline"`・
  `source: "file"` に対応。プロバイダー *kind* 名は組み込みセットに対して
  検証され、タイポ候補が提案されます。Broker 移行済みの `openai` プラグインでは
  キーはプラグインプロセスに渡らず、ホストがここで解決して各 API リクエストへ
  注入します（[サンドボックス・Broker・承認](concepts/sandbox-and-approvals.md)参照）。
- **`ai.tasks`** — パイプラインの各タスクにどのプロバイダー+モデルを使うか:
  `chat`（会話）、`classifier`（LLM 感情分類）、`embedding`（メモリ/ツール
  ベクトル）、`proactive`（プロアクティブ発話の判断）。`dimensions` は
  埋め込み次元を上書き、`query_prefix` は埋め込みクエリの接頭辞。
- **`ai.retry`** — 一時的プロバイダーエラーのリトライポリシー。
- **`ai.fallback`** — ヘルスチェック失敗時に代替プロバイダーへフェイルオーバー。
- **`ai.tts` / `ai.stt` / `ai.vad`** — 音声パイプラインのプロバイダー選択
  （[音声とアバター](concepts/voice-and-avatar.md)参照）。`model`/`voice` は
  プロバイダー固有です。
- **`ai.local_models`** — `local` プロバイダーが使うローカル GGUF モデル定義
  （URL・コンテキストサイズ・GPU レイヤー・量子化・次元）。

## `mind.*` — 認知エンジン

| セクション | 主なキー | 意味 |
|---|---|---|
| `mind.language` | `"ja"` | プロンプト・分類器のデフォルト言語。 |
| `mind.emotion` | `enabled`, `classifier_language` | PAD 感情エンジンと LLM 分類器。 |
| `mind.proactive` | `enabled`, `cooldown_seconds`, `interval_seconds`, `min_idle_seconds`, `sources`, `quiet_hours`, `paused` | プロアクティブ発話のゲート（[プロアクティブ](reference/architecture/cognitive-runtime.md#プロアクティブ発話)参照）。 |
| `mind.memory_limits` | `commitment_active_match_limit` | 想起の上限。 |
| `mind.memory_approval` | `require_approval` | true の場合、抽出メモリは活性化前にレビューキューで待機。 |
| `mind.topic_boundary` | `enabled`, `boundary_threshold`, 重み | セッション分割のヒューリスティック。 |
| `mind.session` | `session_timeout_minutes` | セッションを終了するアイドルタイムアウト。 |

## `store.*` — 永続化

```json
{ "store": { "enabled": true } }
```

メモリストアは、会話履歴・型付きメモリ・埋め込み・約束台帳・スケジュール・
監査ログをひとつの SQLite データベース（アセットディレクトリ内の
`memory.db`）に保持します。ストアを無効にするとメモリ機能が無効になります
（永続化なしでもチャットは動きます）。

## `tools.*` — ツールと MCP

```json
{
  "tools": {
    "enabled": true,
    "list": {
      "fs": { "enable": true },
      "web": { "enable": true },
      "homeassistant": { "enable": true, "base_url": "http://homeassistant.local:8123", "token": "" }
    },
    "mcp_servers": [],
    "rag": { "enabled": true }
  }
}
```

- `tools.list.<name>.enable` は組み込みツールプラグインのオン/オフです。新しい
  プラグイン設定は `plugins.list.<name>` に置き、ホスト管理の秘密情報は
  `credentials` マップを使います。
- `tools.mcp_servers` は外部 MCP サーバーを接続します
  （[MCP サーバーガイド](guides/tools/mcp-servers.md)参照）。
- `tools.rag` は埋め込みベースのツール選択（`ene-rag` の `tool`
  フィーチャー）を設定します。

## `plugins.*` — プラグイン一覧

```json
{
  "plugins": {
    "list": {
      "llama-cpp": {
        "config": { "mmproj_url": "...", "acceleration": "auto" },
        "profiles": {
          "gemma-4-e2b": { "url": "...", "gpu_layers": "auto", "context_size": 16384 }
        }
      }
    }
  }
}
```

各キーはプラグインバイナリ名（`plugins.list.<name>`）で、各プラグインは
独自の設定スキーマ（`config`）と任意の名前付き `profiles`（モデルプリセット）
を宣言します。ホストは起動時に該当セクションを IPC 経由でプラグインへ
渡します。ホスト管理の資格情報はエントリの `credentials` マップに置き、
ネットワークブローカーが注入します。`config` には含まれません。例えば web 検索のキーは
`plugins.list.web.credentials.exa_api_key` と
`plugins.list.web.credentials.tavily_api_key` を使います。
[プラグインと MCP](concepts/plugins-and-mcp.md) 参照。

## `desktop.*` — デスクトップアプリ

| キー | デフォルト | 意味 |
|---|---|---|
| `desktop.graphics.quality` | `"medium"` | 描画品質プリセット。 |
| `desktop.language` | `"ja"` | UI 言語（デスクトップ i18n は `en-US` / `ja`）。 |
| `desktop.theme` | `"system"` | アプリ全体の配色テーマ: `system`・`light`・`dark`。 |
| `desktop.mic_device` | `null` | 音声入力用マイクデバイス ID。 |
| `desktop.spotlight_enabled` | `true` | グローバルスポットライトオーバーレイ。 |
| `desktop.caption_enabled` | `true` | キャラクター発話の字幕オーバーレイ。 |
| `desktop.caption_position` / `caption_pinned` | `null` | 字幕の配置。 |
| `desktop.beat_sync` | `{ "enabled": false, "device": null }` | 音楽ビート同期（アバターモーション）。 |

`desktop.theme` の既定値は `system` です。Linux のシステムテーマは XDG
settings portal 経由で `org.freedesktop.appearance` の `color-scheme` を初期取得し、
変更通知を購読します。Windows では winit の初期ウィンドウテーマと
`ThemeChanged` 通知を使用します。OS が配色を指定しない場合や取得に失敗した
場合はダークになります。明示した `light`・`dark` は OS 通知より優先し、対応する
ネイティブウィンドウ装飾にも反映されます。環境変数では
`ENE_DESKTOP__THEME=system|light|dark` を指定します。

## キャラクターごとの設定（`character_settings.json`）

```json
{
  "character_position": [0, 0, 0],
  "model_scale": 1.0,
  "look_at_strength": 0.6,
  "default_motion": "",
  "default_expression": "neutral",
  "language": ""
}
```

- `character_position` / `model_scale` — デスクトップシーンでの VRM モデルの
  配置。
- `look_at_strength` — アバターがカーソルを追従する強さ（0–1）。
- `default_motion` / `default_expression` — カードのモーションカタログ/
  表情の名前。
- `language` — カードの言語オーバーライド（空ならアプリ言語を継承）。

## 実行時の設定編集

- **CLI REPL:** `/config set <ドット区切りキー> <値>`
  （例: `/config set ai.tasks.chat.model openai/gpt-5.6-luna`）。JSON として
  解釈できる値は JSON で、それ以外は文字列で保存されます。
- **デスクトップ:** 設定ウィンドウの各ページ（AI・キャラクター・メモリ・
  権限・コネクタ・音声など）が同じセクションを編集します。
- CLI・デスクトップのフラグ（`--config`・`--character`・`--lang`）は
  プロセス起動時にファイルを上書きします。

## シークレット

API キーやトークンはログやイベントストリームに書き込まれません。ツール
引数と自由文イベントは、出力・永続化の前にリダクションを通過し、プラグイン
設定値はホスト境界でリダクションされます。API キーは `source: "env"` を使い、
`settings.json` 自体にシークレットを置かないことを推奨します。
