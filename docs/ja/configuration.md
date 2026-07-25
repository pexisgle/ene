# 設定・コンフィグレーションリファレンス

Ene は `figment` をベースとした多層コンフィグレーションシステムを採用しています。設定は以下の優先順位に従ってロードされます：

$$\text{デフォルト値} \longrightarrow \text{JSON 設定ファイル} \longrightarrow \text{環境変数 (\texttt{ENE\_*})}$$

---

## 1. 環境変数の優先順位

環境変数は、デフォルト構造体や JSON 設定ファイルで指定された設定を上書きします。環境変数は `ENE_` プレフィックスを使用し、ネストされたセクションキーはダブルアンダースコア (`__`) で区切ります：

```bash
# 例: デフォルトの LLM チャットモデルを上書き
export ENE_AI__TASKS__CHAT__MODEL="gpt-4o"

# 例: SQLite データベースファイルパスを指定
export ENE_STORE__DB_PATH="/path/to/custom_memory.db"

# Example: プロアクティブ発話間隔（秒）を設定
export ENE_MIND__PROACTIVE__INTERVAL_SECONDS="300"
```

---

## 2. 設定セクション一覧

パブリックな設定セクションは、所有クレート内の `define_config!` マクロ宣言によって定義されます。

### `ai.*` — LLM、埋め込み、および音声パイプライン設定

プロバイダ定義、タスクルーティング、リトライ/フォールバックルール、および音声 (STT/TTS/VAD) 設定を含みます：

```json
{
  "ai": {
    "providers": {
      "openai": {
        "kind": "openai",
        "api_key": "sk-...",
        "base_url": "https://api.openai.com/v1"
      }
    },
    "tasks": {
      "chat": {
        "provider": "openai",
        "model": "gpt-4o-mini"
      },
      "embedding": {
        "provider": "openai",
        "model": "text-embedding-3-small"
      }
    },
    "stt": { "enabled": true },
    "tts": { "enabled": true },
    "vad": { "enabled": true }
  }
}
```

### `store.*` — データベースおよびベクトル永続化

SQLite データベースの永続化、整合性チェック、およびバックアップ保持制御 (#239)：

```json
{
  "store": {
    "enabled": true,
    "backup_on_migrate": true,
    "max_backups": 5,
    "integrity_check_on_open": false
  }
}
```

### `mind.*` — 認知エンジンおよび感情パラメータ

トークンコンテキスト予算、ハイブリッド記憶想起、感情減衰、キャラクターコンパイル、およびプロアクティブ発話ポリシー (#103) を設定します：

```json
{
  "mind": {
    "context": {
      "max_tokens": 4096,
      "recall_limit": 10
    },
    "emotion": {
      "enabled": true,
      "decay_half_life_minutes": 30.0
    },
    "proactive": {
      "enabled": true,
      "interval_seconds": 600
    }
  }
}
```

### `plugins.*` — IPC プラグインおよび MCP サーバー接続

プロセス外ツールプラグインおよび Model Context Protocol (MCP) サーバーを管理します：

```json
{
  "plugins": {
    "enabled": true,
    "list": {
      "app": { "enable": true },
      "browser": { "enable": true },
      "fs": { "enable": true },
      "utility": { "enable": true },
      "web": { "enable": true }
    },
    "mcp_servers": [
      {
        "name": "filesystem",
        "command": "npx",
        "args": ["-y", "@modelcontextprotocol/server-filesystem", "/home/user/allowed"]
      }
    ]
  }
}
```

### `desktop.*` — デスクトップ GUI およびグラフィックスパラメータ

表示言語、グラフィックス描画パラメータ、およびマイク入力デバイスを制御します：

```json
{
  "desktop": {
    "language": "ja",
    "mic_device": null,
    "graphics": {
      "vsync": true
    }
  }
}
```

---

## 3. キャラクターカード形式 (`character.json`)

Ene は JSON キャラクターカードを介して、キャラクターのパーソナリティとプロンプトテンプレートをロードします：

```json
{
  "name": "Alicia",
  "identity": "電脳世界に住む人工知能の少女。コンピュータの中でユーザーをサポートする。",
  "system_prompt": "あなたはアリシアです。元気にユーザーをサポートしてください。簡潔かつ的確に答えてください。",
  "greeting": "システム正常稼働中だよ。今日は何をするの？",
  "initial_affect": {
    "pleasure": 0.6,
    "arousal": 0.7,
    "dominance": 0.5
  }
}
```

---

## 4. スキーマ自動生成

設定スキーマは、各所有クレートの `define_config!` マクロによって宣言されます。スキーマは設定初期化時に自動的に再生成されます。

> [!CAUTION]
> `assets/schema/*` 下の無視されたスキーマファイルを直接手修正したりコミットしたりしないでください。
