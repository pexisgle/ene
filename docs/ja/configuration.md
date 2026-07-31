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

環境変数による上書きは**一時的**です。現在のプロセスの実行時にのみ適用され、`settings.json` に書き戻されることはありません。設定を保存しても JSON 層の値のみが永続化されるため、`ENE_*` 変数を削除すれば、次回の起動時に元の JSON／デフォルト値に戻ります (#326)。

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

各プロバイダエントリには、任意で `context_window`（整数、トークン単位）を設定でき、
バックエンドが申告するコンテキストウィンドウに上限を設けられます (#364)。
有効ウィンドウは `min(advertised, context_window)` となるため、オーバーライドは
モデルの公称上限を「縮小」できるだけであって、決して上回ることはできません。
省略すれば、完全にプロバイダの申告に従います。プラグインプロバイダは
`LlmProviderSpec.context_window` 経由でウィンドウを申告し、ローカルモデルは
`LocalModelDef.context_size` を報告します。ランタイムはこの有効ウィンドウから、
モデルの応答用の余地としてタスクの `max_tokens`（`tasks.<task>.max_tokens`）を
予約し、さらにトークン推定の誤差を吸収するための安全マージンを差し引いて、
残りをプロンプトの予算とします：

```
available = min(model_window, context_window)
          − response_reserve    // tasks.<task>.max_tokens
          − safety_margin       // 推定誤差。usage が計測되면ほぼ 0 (#365)
```

```json
{
  "ai": {
    "providers": {
      "openai": {
        "kind": "openai",
        "api_key": "sk-...",
        "base_url": "https://api.openai.com/v1",
        "context_window": 32000
      }
    }
  }
}
```

#### トークン使用量の集計 (#365)

すべての補完には、任意のトークン使用量レコード（`prompt_tokens`、
`completion_tokens`、`total_tokens`）が 3 つのプロバイダ層すべて
（`ene-ai` のプロセス内型、プラグイン IPC、ストリーミングチャンク）を通じて
付随します。設定方法はバックエンドによって異なります：

- **使用量を報告するプロバイダ**（OpenAI 互換、Anthropic）は API レスポンスから
  直接設定します。ストリーミングでは、使用量は**最終**チャンクでのみ到着し、
  中間チャンクでは空のままです。
- **ローカルモデル**（llama.cpp）はトークンを自分でカウントします。コンテキストに
  投入された正確なプロンプト長とサンプリングされたトークン数です。そのため、
  ワンショット補完とストリーミング補完の両方で実際の使用量を報告します。
- **何も報告しないプロバイダ**は、粗い文字ベースの推定（おおよそ 3 文字あたり
  1 トークン）にフォールバックします。これは英語を過大評価し、日本語を
  単純な 4 文字/トークンルールよりも過小評価します。

測定されたカウントには推定誤差がないため、使用量が利用可能になると、上記の
`safety_margin` をゼロに近づけることができます。推定値は、バックエンドが何も
報告しない間のみ保守的なマージンを維持します。この動作に設定はありません。
プロバイダごとに自動です。

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
      "interval_seconds": 600,
      "sources": {
        "window_title_level": "app_only"
      }
    },
    "memory": {
      "recall_min_score": 0.10,
      "recall_similarity_threshold": 0.35,
      "commitment_boost": 0.25,
      "commitment_title_similarity_threshold": 0.82
    }
  }
}
```

プロアクティブ活動観測は、自発発話の判断材料としてフォーカス中のアプリケーションを取得します。
`mind.proactive.sources.window_title_level` は、フォーカス中ウィンドウのタイトルをどこまで
読み取るかを制御します (#378)。ウィンドウタイトルにはプライベート情報が日常的に含まれます
（文書名・ファイル名（顧客名・案件名を含みうる）、ページ URL、チャット相手名、メール件名）。
既定は `app_only`（アプリ名のみ。従来の挙動）です。このテキストは自発発話の判定モデルへ送られ、
クラウドプロバイダーを設定している場合は**ローカルマシンの外へ送信されます**。レベルは次のとおりです：

| レベル | 取得内容 |
|---|---|
| `app_only` | アプリ名のみ（既定。タイトルは読み取りません） |
| `redacted_title` | アプリ名 + ウィンドウタイトル（ファイルパス・メールアドレス・URL・数字列を除去。`report.xlsx` のような単独の文書名は保持） |
| `full_title` | アプリ名 + 生のウィンドウタイトル |

`full_title` はローカルモデル使用時のみ選択してください。クラウドプロバイダー使用時は生のタイトルが外部へ送信されます。

記憶想起はハイブリッドスコア `(relevance × quality + commitment_boost) × penalty`
（`crates/ene-rag/src/scoring.rs` を参照）を使用します。新しくて関連性の強い記憶は
`1.0` に近いスコアになり、最近の/語彙一致のみの候補は `0.1〜0.5` 程度、無関係な
ノイズは `0.0` になります。`recall_min_score`（デフォルト `0.10`）は最終ランキングを
フィルタリングし、`recall_similarity_threshold`（デフォルト `0.35`）はベクトル収集を
制御し、`commitment_boost`（デフォルト `0.25`）はクエリとの関連性がゼロでもアクティブな
約束を表面化させます。

`mind.emotion.classifier_language`（デフォルト `"en"`）は、感情分類器と認知出力契約で
使用されるプロンプトライブラリの言語を選択します。ユーザー向け LLM 指示文字列は
`assets/lang/{lang}/prompts.json` から実行時にロードされます。そのパックが存在しない
場合、`ene_config::SUPPORTED_LANGUAGES`（`en`, `ja`）の言語についてはコンパイル時
埋め込みパックへ、それ以外は英語へフォールバックします。詳細は
[ターン・セッション](concepts/turn-and-session.md) §3 を参照してください。

コミットメント台帳は、着信したコミットメントをタイトル埋め込みの類似度でアクティブな
コミットメントと照合します (#387)。`commitment_title_similarity_threshold`（デフォルト
`0.82`）はコサイン類似度の閾値で、これを上回ると言い回しの異なる約束は二重登録ではなく
既存コミットメントの更新（supersede）として扱われます。埋め込みプロバイダーが未設定の
場合、台帳は正規化タイトルの完全一致にフォールバックし、この閾値は使用されません。

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
    "max_concurrent": 8,
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

HTTP の MCP エンドポイントは接続前に URL を検証します (既定では HTTPS のみ。
ループバックおよびクラウドメタデータ/リンクローカルアドレスは拒否)。`plugins` 内で
`"mcp_allow_insecure_urls": true` を設定すると、ローカル開発用にプレーン `http://` と
ループバック URL を許可できます。リンクローカルアドレスは拒否されたままです。
詳細は[プラグインと MCP](concepts/plugins-and-mcp.md) を参照。

`max_concurrent` は、**プラグイン接続ごと**の同時進行中 IPC リクエスト数の上限です。ツール呼び出しだけでなく、ping・`list_tools`・`chat_completion` など*すべて*のリクエスト種別を対象とします。上限を超えたリクエストは、プラグインへ無制限に送出されるのではなく（自身のタイムアウトを上限として）キューイングされます。チャット*ストリーム* (`CreateChatStream`) は例外で、この上限をバイパスし、カウントされません。

### `tools.*` — ツール実行ランタイムの挙動

`plugins.*`（プラグインの プロセス／IPC 層を管理）とは別に、`tools.*` は
`ene-runtime` と `ene-rag` が所有するツール呼び出しランタイムの設定を扱います。`tools.rag` は Tool RAG 選択パイプライン (`ene_rag::ToolRagConfig`) を設定し、以下のフィールド (`ene_runtime::ToolRuntimeConfig`) はアクターが同時に保持するバックグラウンドタスク数の上限と、遅延ツールのポーリング予算を制御します。上限に達すると、無制限にキューイングされるのではなく、admission（受け入れ）が拒否されます（フェイルファスト）。`CallTool`/`CancelDeferredTool` および `SearchTools` の呼び出し元には具体的な "busy" エラーが返りますが、ポストターンの分類器・メモリライター・遅延ツールのポーラーには返信チャンネルがないため、そこでの拒否は `TaskRejected` 診断イベントとしてのみ観測できます：

```json
{
  "tools": {
    "call_tool_cap": 64,
    "deferred_tool_cap": 32,
    "classifier_cap": 16,
    "memory_writer_cap": 16,
    "search_cap": 16,
    "deferred_max_polls": 600,
    "rag": {
      "enabled": true,
      "top_k": 12,
      "min_similarity": 0.20,
      "use_failure_feedback": true,
      "failure_penalty": 0.5,
      "weights": {
        "summary": 1.0,
        "description": 0.6,
        "capability": 0.8,
        "example": 0.4,
        "negative": -0.5,
        "negative_threshold": 0.70
      }
    }
  }
}
```

Tool RAG は各ツールを、そのフィールドごとの埋め込み類似度の重み付き平均
（`[-1, 1]`）でスコアリングします。`min_similarity`（デフォルト `0.20`）はその
平均に対する包含下限です。`weights.negative_threshold`（デフォルト `0.70`）は
ツールのネガティブ例埋め込みがこの値以上に一致した場合にそのツールを完全に
除外するゲートです — 自身のネガティブ例と強く一致するツールはペナルティ
ではなく除外されます。

`use_failure_feedback`（デフォルト `true`）が有効な場合、アクティブなキャラクターに
対して最近失敗したツールの重みが下がります：そのスコアはランキング前に
`failure_penalty`（デフォルト `0.5`、失敗したツールのスコアは半分になります）倍され、
ペナルティによって
`min_similarity` を下回ったツールは除外されます。最近の失敗は
`ene_core::ToolFailureSignalPort`（`ene-store` が実装）経由で読み取られるため、
パイプラインは永続化への依存を持ちません — 詳細は
[記憶システム §5](concepts/memory-system.md#5-ツール由来記憶のガードレール) を参照してください。

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

設定スキーマは、各所有クレートの `define_config!` マクロによって宣言されます。スキーマは設定読み込みのたびにではなく、アプリケーション起動時（CLI の `init`、デスクトップの初回起動、ランタイムの open 経路）にプロセスあたり一度だけ書き込まれます。各スキーマファイルはアトミックに（一時ファイル + `fsync` + リネームで）書き込まれるため、クラッシュしても途中で切れたスキーマファイルが残ることはありません。

`settings.json` を保存する際、Ene は相対的な `$schema` ポインタ（`./schema/settings.schema.json`）をファイル先頭に自動で書き込みます。これにより、キーを手書きしなくてもエディタが補完とバリデーションを提供します。既存の `$schema` 値はそのまま保持され、ポインタは存在しない場合にのみ補完されます。同様に、ユーザーが手動で並べ替えたトップレベルのセクション順も保存をまたいで維持され、新しく追加されたセクションは末尾に追加されます。

> [!CAUTION]
> `assets/schema/*` 下の無視されたスキーマファイルを直接手修正したりコミットしたりしないでください。
