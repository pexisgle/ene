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

ローカルモデルの `context_size` の既定値は 16,384 トークンです (#366)。これは
システム自身の既定プロンプト予算（`mind.context.max_prompt_tokens` = 12,000）に
モデルの応答を加えた分を収容できるよう較正されています。従来の既定値 2,048 は
小さな決定タスク向けのもので、ローカルモデルがメイン会話を担うとプロンプトの
大部分のセクションが黙って欠落していました。llama.cpp の KV キャッシュを現実的な
範囲に収めるため、32K ではなく 16K を採用しています（Gemma 3 4B 相当のモデルで
重みに加えて約 2.3 GB、32K では約 4.6 GB）。決定タスク専用モデルは `context_size`
を明示的に下げられます。

起動時、ランタイムは各生成タスク（`chat`、および設定されている場合は `proactive`）
のウィンドウが必要量（プロンプト予算 + 応答予約 `tasks.<task>.max_tokens`）を
満たしているかを検証し、設定されたウィンドウが小さすぎる場合は警告をログに出します。
さもなくば、何の表示もなく毎ターンプロンプトのセクションが欠落するためです。
明示的な `context_window` オーバーライドのないクラウドタスクは、プロバイダが
実際のウィンドウを報告する実行時に検証されます。

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

コンテキストウィンドウのパッキング、ハイブリッド記憶想起、感情減衰、キャラクターコンパイル、およびプロアクティブ発話ポリシー (#103) を設定します：

```json
{
  "mind": {
    "context": {
      "max_prompt_tokens": 4096
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
      "commitment_title_similarity_threshold": 0.82,
      "contradiction_title_similarity_threshold": 0.82,
      "min_confidence_to_persist": 0.65,
      "supersede_confidence_delta": 0.05,
      "semantic_similarity_threshold": 0.85,
      "dispute_confidence_gap": 0.15
    }
  }
}
```

プロンプトのパッキングは、セクション別のトークン予算配分を廃止しました (#370)。
代わりに、モデルの実効コンテキストウィンドウ (#364) を優先度順に充填します。
必須セクション（アイデンティティカーネル、出力契約、ユーザー入力）は常に保持
され、プロンプトがウィンドウを超過した場合は最も優先度の低いドロップ可能
セクションから順に落としていきます。`mind.context.max_prompt_tokens` は任意の
オペレーター上限で、`min(advertised, max_prompt_tokens)` としてウィンドウを
縮小します。これを省略する（デフォルト）と、プロンプトはモデルが広告する
コンテキストサイズに自動的に追随します。

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

メモリー調停器（arbiter）は、着信した候補が同じ種別の既存記憶と矛盾するかどうかを、
*タイトル埋め込みの類似度* で判定します (#351)。`contradiction_title_similarity_threshold`
（デフォルト `0.82`）はコサイン類似度の閾値で、これを上回ると同義のタイトル
（「職業」と「仕事」、「住んでいる場所」と「居住地」）は同じ主題として扱われ、
無関係な重複として永続化される代わりに矛盾検査の対象となります。埋め込みプロバイダーが
未設定の場合、調停器は正規化タイトルの完全一致にフォールバックし、この閾値は使用されません。

メモリー調停器の 4 つの判定閾値は、すべて `mind.memory.*` で設定できます (#352)。
これらは合わせて、着信した候補をいつ永続化するか、いつ既存の矛盾する記憶を
*上書き（supersede）* するか、いつ既存記憶を *係争中（disputed）* とマークするか、
そしていつ判定をユーザー確認へ回すかを決めます：

| 設定 | 既定 | 意味 |
|---|---|---|
| `min_confidence_to_persist` | `0.65` | 永続化に必要な候補の最低 confidence。 |
| `supersede_confidence_delta` | `0.05` | 候補が既存記憶の confidence を上回り、上書きするために必要な差。 |
| `semantic_similarity_threshold` | `0.85` | 2 つの記憶を意味的な重複とみなすコサイン類似度の閾値（以上で重複）。 |
| `dispute_confidence_gap` | `0.15` | 矛盾する候補が既存記憶を上書きせず係争中とマークする、confidence の差の下限。 |

4 つとも確率・比率であり、読み込み時に `0.0..=1.0` へ clamp されます。
特に `semantic_similarity_threshold` は埋め込みモデルの類似度分布に強く依存するため、
埋め込みプロバイダーを切り替えた際は再調整してください。

### `plugins.*` — IPC プラグインおよび MCP サーバー接続

プロセス外ツールプラグインおよび Model Context Protocol (MCP) サーバーを管理します：

```json
{
  "plugins": {
    "enabled": true,
    "list": {
      "app": { "enable": true },
      "browser": { "enable": true },
      "fs": { "enable": true, "db_quota_mb": 256 },
      "utility": { "enable": true },
      "web": { "enable": true }
    },
    "max_concurrent": 8,
    "parallel_tool_calls_max": 4,
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

`parallel_tool_calls_max` は、1 つの LLM 応答に含まれる**副作用のない**ツール呼び出しを同時にいくつ実行するかの上限です。モデルが 1 ターンで複数のツール呼び出しを出力したとき、`ToolSpec` で `side_effects: ReadOnly` を宣言しているもの（かつバックグラウンド非対応のもの）を、この上限まで並列にディスパッチします。並列化は応答内の*すべて*の呼び出しが副作用のない場合にのみ適用されます。混合ラウンドでは、読み取り専用の呼び出しが同じ応答内の先行する書き込みを追い越してはならない（read-after-write）ため、厳密に元の順序で逐次実行されます。それ以外 — 副作用のあるツール、副作用を宣言していないツール、`system.search_tools` — は従来どおり逐次実行されます。結果は元の `tool_calls` の順序へ並べ戻されるため、権限/ユーザー入力のプロンプト、undo スタック、`ToolCallStart`/`ToolCallResult` イベント、`ToolResultSummary` の順序はすべて保たれます。`0` を設定すると並列化が完全に無効になり、以前の完全逐次動作に戻ります。分類はフェイルクローズドです。`ReadOnly` の副作用を宣言していないツールは決して並列化されません。

`plugins.list.<name>.db_quota_mb` は、プラグインのテーブルが**共有 `memory.db`** 内で占有できる上限をメビバイト単位で設定します (#424)。ステートフルなプラグインはすべて 1 つの共有データベースへ書き込むため、上限がなければ 1 つの暴走（または悪意ある）プラグインがディスクを使い切ったり、`memory.db` を肥大化させて記憶システムのクエリ・バックアップ・整合性検査を劣化させたりするおそれがあります。ホストは各プラグインの使用量（宣言済みテーブル全体の全セルのバイト長合計）を測定し、上限に達するか超えるようなストレージを増やす書き込み（`Insert`/`Upsert`、`Batch` 内のものを含む）を拒否し、`QUOTA_EXCEEDED` エラーを返します。読み取りと削除は一切制限されないため、上限に達したプラグインでも常に空きを確保できます。既定値は `256` で、組み込みプラグインが近づけないほど十分に大きい一方、暴走プラグインが実際の被害を出す前に抑制できます。無制限のストレージが正当に必要なプラグインには、このフィールドを `null` に設定して強制を無効化できます。

#### `plugins.list.<name>.config` — プラグイン所有の設定 (#313)

各プラグインエントリは、ホストからは**不透明**な設定ブロブを保持できます：

```json
{
  "plugins": {
    "list": {
      "anthropic": {
        "enable": true,
        "config": {
          "api_key": { "source": "env", "env": "ANTHROPIC_API_KEY" }
        }
      },
      "llama-cpp": {
        "enable": true,
        "config": {
          "mmproj_url": "https://example.com/mmproj.gguf",
          "acceleration": "vulkan"
        }
      }
    }
  }
}
```

ホストはこのブロブを**そのまま**保存・配信します。ブロブ内のキーを解釈・書き換え・破棄することは決してありません（未知のキーもロード→セーブの往復で保持されます）。ブロブはハンドシェイク時に一度だけプラグインへ送信されます（`ConfigurablePlugin::set_config`）。プロバイダートレイト（LLM/embed/TTS/STT）を実装するプラグインも、ツールプラグインと同じ方法で受け取ります。単一キーの環境変数オーバーライドは `ENE_PLUGINS__LIST__<NAME>__CONFIG__<KEY>`（例：`ENE_PLUGINS__LIST__ANTHROPIC__CONFIG__API_KEY`）です。従来 `ai.*` にあったプロバイダー固有設定はここへ移動しました。たとえば `plugins.list.llama-cpp.config.{mmproj_url,mmproj_path,acceleration}`（旧 `ai.local_models.<name>.{mmproj_url,mmproj_path,acceleration}`）、`plugins.list.onnx.config.ort_dylib_path`（旧 `ai.ort_dylib_path`）、`plugins.list.kokoro.profiles.kokoro.voices_path`（旧 `ai.tts.voices_path`）。

#### `plugins.list.<name>.profiles.<profile>` — プロファイル別設定 (#313)

1 つのプラグインがモデル/音声/プロファイルごとに異なる設定を必要とすることがあります。`profiles` マップは、ホストからは不透明なプロファイル別ブロブを保持し、ハンドシェイク時にプラグインへ配信されます（`ConfigurablePlugin::set_profiles`）。プロファイルの*選択*はプラグイン側の責務です：

```json
{
  "plugins": {
    "list": {
      "kokoro": {
        "enable": true,
        "profiles": {
          "kokoro": { "voices_path": "/data/voices.bin" }
        }
      }
    }
  }
}
```

#### シークレットのマーキング

プラグインの `config_schema()` は、フィールドに `x-ene-secret: true` を付与できます。ホストはこれ（および既知の名前によるフォールバック：`api_key`・`token`・`password`・`authorization` など）を使って、設定 UI でフィールドをマスクする予定であり、ホストのログ出力からは値を削除（redact）します。インラインの API キーがログストリームに現れることはありません。`settings.json` の外部（キーリング/シークレットサービス）へのシークレット保存は別途追跡されており、それまではプラグインのシークレットは `plugins.list.<name>.config` 内に置かれ、スキーマでマークされ、ホスト境界で redact されます。

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
