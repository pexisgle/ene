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

プロバイダのバックエンドはプラグインとして提供されます。OpenAI 互換
バックエンドは `openai` プロバイダプラグイン（`plugins/provider/openai`、
kind は `"openai"`）で、デフォルトの `plugins.list` に含まれ、
`OPENAI_API_KEY` と `OPENAI_BASE_URL` が子プロセスへ受け渡されます。旧 kind 値
`"openai_compatible"` はエイリアスとして引き続き受け付けられ、プロバイダごとの
`base_url` / `api_key` はリクエストごとにプラグインへ転送されるため、
既存の OpenAI 互換設定（OpenRouter、ローカルサーバなど）はそのまま動作します。
`openai` プラグインは埋め込みバックエンドも兼ねます。クラウド埋め込みを
使うには `tasks.embedding` を `"openai"` kind のプロバイダに向けてください。
プラグインシステムを無効化した場合（`plugins.enabled = false`）、クラウド
プロバイダは利用できません。

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

各 `ai.local_models.<name>` エントリのモデルパス/設定（`url`・`quantization`・
`model_path`・`gpu_layers`）は、ローカル GGUF プロバイダプラグイン
(`ene-plugin-llama-cpp`) が消費する `plugins.list.llama-cpp.profiles.<name>`
ブロブへミラーされます。`local_models` のキー自体はルーティングおよび
コンテキスト予算の情報としてここに残ります（特に `context_size` は解決時に
読まれ、ミラーされません）。ミラーは v2→v3 設定マイグレーションによる
一方向コピーです — プロファイルを編集しても `local_models` は書き換わりません。

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
      "fatigue_suppression_threshold": 0.7,
      "confirmation_enabled": false,
      "sources": {
        "window_title_level": "app_only"
      }
    },
    "memory_limits": {
      "commitment_active_match_limit": 4096
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

チャットモデルが出す表情マーカーは正典です。ターン中に表情提案があれば
感情→表情マッピングより優先し、マーカーが無いときだけ感情マッピングを
フォールバックとして使います。ヒステリシスは発生源によらず適用し、ターン中の
連続マーカーによるちらつきを防ぎます。発話タイミングに同期した表情変化は、
この解決経路とは別に扱います。

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

`mind.proactive.fatigue_suppression_threshold`（0.0〜1.0、既定 `0.7`）は、キャラクターの感情疲労度が閾値以上のとき自発発話の判定を抑制します — 疲れているキャラクターは自分から話しかけません。既定値はムードラベル `"tired"` の境界（`compute_mood_label`）と一致しており、ゲートとキャラクターの見た目の機嫌が食い違いません。`1.0` にするとゲートが無効になり、疲労度の判断はモデルに委ねられます。閾値の有無にかかわらず、全8次元とムードラベルは常に判定モデルへ渡されます。

`mind.proactive.confirmation_enabled`（既定 `false`）を有効にすると、本体の生成モデルが**同じ生成呼び出しの中で**判定を確認します（往復は増えません）。生成プロンプトは「話す価値がなければ応答の最初に `<|silent|>` だけを出力してよい」と指示し、可視テキストより先にそのトークンが届いた時点でランタイムはストリームを即キャンセルし、表示・発話は一切行われません。確認は精度しか上げません（判定モデルの「話す」誤りは拾えますが、「話さない」誤りは拾えません）。確認を有効にすると、判定閾値（`mind.proactive.decision.min_confidence`、ステージドロールアウト中は現在 0.55 固定）は**自動的に 0.15 引き下げられます** — 安価な判定モデルは再現率優先の前段として境界ケースを通し、本体モデルが精度を担う後段として却下します。判定モデルと本体モデルの一致率（生成に到達した判定のうち accepted / declined）は、構造化ログの `event="confirmation"` に記録されます。可視テキストが無い空応答は `confirmation=empty` として記録され、一致率からは除外されます。早期キャンセルが有効なのはトークンをストリーミングするプロバイダーだけです。非ストリーミングのローカルアダプターは最初のチャンクの前に完了応答全体をバッファするため、その経路での辞退は生成済みの応答を破棄する形になり、トークン節約にはなりません。

### クワイエットアワーと手動一時停止

`mind.proactive.quiet_hours` は、スケジュールに基づいて自発発話を抑制します。既定は無効です。`mind.proactive` の一部として記述します:

```json
"quiet_hours": {
  "enabled": true,
  "timezone": "Asia/Tokyo",
  "days": {
    "monday": true, "tuesday": true, "wednesday": true,
    "thursday": true, "friday": true, "saturday": false, "sunday": false
  },
  "start": { "hour": 22, "minute": 0 },
  "end": { "hour": 7, "minute": 0 },
  "suppress": { "notifications": true, "decisions": true, "tts": true },
  "policy": "discard"
}
```

- `timezone` は IANA タイムゾーン名です（`Asia/Tokyo`、`America/New_York` など）。空欄はシステムのローカルタイムゾーンを使用します。夏時間（DST）の切り替えは、UTC の瞬間をローカルの壁時計時刻に変換して判定するため、秋の繰り返し時刻は2回ともウィンドウ内として扱われ、春にスキップされる時刻は決してウィンドウ内になりません。
- `days` で曜日を選択します。`start` は含み、`end` は含みません。`end` が `start` より早い場合は日付をまたいで深夜に折り返します（開始日の夜と翌朝をカバーし、開始日の曜日が有効である必要があります）。`start` と `end` が同じ場合は空のウィンドウです。
- `suppress` で抑制する出力チャンネルを選びます: `decisions` は決定論的ゲートで判定/生成パイプライン全体を止め（LLM は呼ばれません）、`notifications` は自発発話ターンのステータス通知を抑え、`tts` は生成テキストを表示しつつ自発発話ターンの TTS 音声だけを止めます。
- `policy` は `decisions` の抑制で止められた発話の扱いです: `discard` はログのみ残して破棄、`queue` はウィンドウ終了後に1件ずつキャッチアップ発話として届け、`summary` は終了後に1件にまとめて届けます。キュー/要約が機能するのは `decisions` の抑制が有効な場合だけです。キャッチアップキューには上限があり（古いものから破棄）、セッション単位です。記録されるのは決定論的ゲート（待機・クールダウン・セッション上限・ソース・疲労）を通過した瞬間だけであり、ユーザーターンが始まるとキューは破棄されます（ユーザーが席に戻ったため）。キャッチアップ項目にはローカルの日付と時刻のみが含まれ、画面データは一切含まれません。
- バックグラウンドの観測（アクティビティ、画面要約）は、既存のプライバシー設定（`sources.*`）に従ってクワイエットアワー中も継続します。クワイエットアワーは発話出力のみを止めます。抑制は構造化ログの `event="quiet_hours_suppression"` に記録され、ポリシーと判定メタデータのみを含みます — 画面画像は一切保存されません。

`mind.proactive.paused`（既定 `false`）は手動一時停止で、クワイエットアワーや他のすべてのゲートより優先されます。一時停止中は自発発話が一切行われず、保留中のキャッチアップ配信も破棄され、デスクトップの設定画面に一時停止状態が明示されます。

自発発話の判定は保存された記憶も参照します。`mind.proactive.sources.memory`（既定 `true`）は、ユーザーの `Preference` / `UserProfile` 記憶（「作業中は話しかけないで」「夜は静かに」など）を判定コンテキストの `user_instructions` として注入します。これらは決定論的に注入され（新しい順、最大 `mind.proactive.max_memory_notes`、既定 12 件。この上限はステージドロールアウトの間は 12 件に固定されており、まだユーザー設定できません）、想起スコアの競争には一切乗らないため、抑制条件が低スコアで落ちることはありません。判定モデルには、該当する恒常ルールがあれば `should_speak=false` で従うよう指示されます。生成フェーズでも同じ設定が話題の想起を有効にします。判定の `topic_hint` を語彙のみの検索クエリ（埋め込みプロバイダー不要）として使い、話しかける話題に関して覚えていることに触れられます。`sources.memory` を `false` にすると、コスト/レイテンシを気にする構成向けに従来の「記憶なし」挙動へ戻ります。

`redacted_title` はタイトルをフィールド単位でフィルタします。空白に加えてウィンドウタイトルで
使われる区切り文字（`_ - | 、 ・ 【】 「」 ｜ ：` など）でも分割するため、日本語・中国語で
一般的な「空白を含まないタイトル」も、1 つの塊として素通りするのではなくフィールド単位で
フィルタされます。`.` `/` および半角 `:` では分割しません。これらはパス・URL・拡張子を
つなぐ文字であり、検出器が完全なトークンを必要とするためです。区切り文字に囲まれていない
フィールド（人名を含む地の文など）はそのまま残るため、`redacted_title` は露出を
「なくす」のではなく「減らす」設定です。タイトルを一切外部に出したくない場合は
`app_only` を使用してください。

記憶想起はハイブリッドスコア `(relevance × quality + commitment_boost) × penalty`
（`crates/ene-rag/src/scoring.rs` を参照）を使用します。新しくて関連性の強い記憶は
`1.0` に近いスコアになり、最近の/語彙一致のみの候補は `0.1〜0.5` 程度、無関係な
ノイズは `0.0` になります。`recall_min_score`（デフォルト `0.10`）は最終ランキングを
フィルタリングし、`recall_similarity_threshold`（デフォルト `0.35`）はベクトル収集を
制御し、`commitment_boost`（デフォルト `0.25`）はクエリとの関連性がゼロでもアクティブな
約束を表面化させます。`access_boost_half_life_days`（デフォルト `14.0`、
`ene_rag::ACCESS_BOOST_HALF_LIFE_DAYS` と同じ）は、品質係数における過去アクセス
ブーストの減衰半減期を制御します。内容の忘却 / 新近性用の
`default_forgetting_half_life_days` とは独立です。

`mind.language`（デフォルト: システムロケールから解決。プライマリ言語コードが `ja` の
場合のみ `"ja"`、それ以外は `"en"`）は、認知プロンプトと決定論的パターンのアプリ全体の
言語です。感情分類器・認知出力契約・圧縮要約・想起意図キーワード・記憶抽出パターンは、
それぞれのタスク別 override が設定されていない限りこれに従います。
`mind.language` を設定していない既存インストールでは、アップグレード後にプロンプトと
分類器の言語が変わる可能性があります。デフォルトは以前は英語に固定されていましたが、
現在はシステムロケールから導出されます（日本語システムでは `ja` になります）。
`mind.emotion.classifier_language` と `mind.context.compression_language` はタスク別の
override で、空（デフォルト）の場合は `mind.language` を継承します。記憶抽出は
`mind.language` に直接従い、分類器設定を参照しません。ユーザー向け LLM 指示文字列は
`assets/lang/{lang}/prompts.json` から、決定論的な
パターンは `assets/lang/{lang}/patterns.json` から実行時にロードされます。その
パックが存在しない場合、`ene_config::SUPPORTED_LANGUAGES`（`en`, `ja`）の言語に
ついてはコンパイル時埋め込みパックへ、それ以外は英語へフォールバックします。詳細は
[ターン・セッション](concepts/turn-and-session.md) §3 を参照してください。

コミットメント台帳は、着信したコミットメントをタイトル埋め込みの類似度でアクティブな
コミットメントと照合します。`commitment_title_similarity_threshold`（デフォルト
`0.82`）はコサイン類似度の閾値で、これを上回ると言い回しの異なる約束は二重登録ではなく
既存コミットメントの更新（supersede）として扱われます。埋め込みプロバイダーが未設定の
場合、台帳は正規化タイトルの完全一致にフォールバックし、この閾値は使用されません。
照合は適用バッチごとにアクティブな台帳行をメモリへ読み込みます。
`mind.memory_limits.commitment_active_match_limit`（デフォルト `4096`）がその件数の上限で、現実的な
同時アクティブ約束数をはるかに上回る値とし、台帳が肥大化した場合のメモリと埋め込み
計算を抑えます。返却件数がちょうど上限と一致すると台帳は切り捨てを警告します。
照合漏れが疑われる場合は `mind.memory_limits.commitment_active_match_limit`（または
`ENE_MIND__MEMORY_LIMITS__COMMITMENT_ACTIVE_MATCH_LIMIT`）を引き上げてください。
これはオペレーターが設定できる 2 つのメモリ項目のうちの 1 つです。`mind.memory.*` の
その他の挙動はコード既定値（`MindMemoryConfig`）のままです。もう 1 つは下記の
承認ワークフロー切り替えです。

### `mind.memory_approval.*` — 保存前候補承認

```json
{
  "mind": {
    "memory_approval": {
      "require_approval": false
    }
  }
}
```

`require_approval`（既定 `false`、環境変数:
`ENE_MIND__MEMORY_APPROVAL__REQUIRE_APPROVAL`）は、typed memory の書き込みを
自動保存から「保存前レビュー」ワークフローに切り替えます。`true` のとき、通常は
永続化される（または既存記憶を上書きする）抽出候補はすべて `pending_candidates`
キューに留め置かれ、出典ターン・出典引用・抽出理由・confidence・上書き対象を保持します。
キューは CLI（`/memory approval`）とデスクトップの Memory Journal に表示され、
各候補を確認・編集・編集して承認・承認・却下できます。承認された候補は typed memory
として永続化され、元の競合対象は `supersedes_id` として引き継がれ、古い記憶は
自動保存と同じ上書きセマンティクスで `Superseded` に移行します。却下された候補は
破棄されます。編集は書き込み前に検証され、解決は競合安全なので、不正な編集や
競合しても元の候補が失われることはありません。承認・編集操作は実行中のターン ID を
保持し、ランタイムのライフサイクルバス上で `CandidateChanged` 監査イベントとして
発行されます。

承認モードでは、未承認の候補は通常の想起から除外されます。プロンプトには現れず、
レビューキューでのみ表示されます。承認モードで保留された候補は、後でモードを
オフにしても想起に戻りません。通常の想起に参加できるのは弱い矛盾による保留のみです。
既定の自動保存モード（`false`）は変更されません。弱い矛盾による候補は従来どおり
確認待ちとなり、下記の
`recall_pending_candidate_limit` の範囲で想起に参加できます。コミットメント候補
（専用の台帳パス）と明示的なユーザー忘却・係争（dispute）判定は、どちらのモードでも
即時に適用されます。

メモリー調停器（arbiter）は、着信した候補が同じ種別の既存記憶と矛盾するかどうかを、
*タイトル埋め込みの類似度* で判定します (#351)。`contradiction_title_similarity_threshold`
（デフォルト `0.82`）はコサイン類似度の閾値で、これを上回ると同義のタイトル
（「職業」と「仕事」、「住んでいる場所」と「居住地」）は同じ主題として扱われ、
無関係な重複として永続化される代わりに矛盾検査の対象となります。埋め込みプロバイダーが
未設定の場合、調停器は正規化タイトルの完全一致にフォールバックし、この閾値は使用されません。

メモリー調停器の 4 つの判定閾値は `MindMemoryConfig` でコード既定（code default）であり、
設定からは変更できません (#352)。これらは合わせて、着信した候補をいつ永続化するか、
いつ既存の矛盾する記憶を *上書き（supersede）* するか、いつ既存記憶を
*係争中（disputed）* とマークするか、そしていつ判定をユーザー確認へ回すかを決めます：

| 設定 | 既定 | 意味 |
|---|---|---|
| `min_confidence_to_persist` | `0.65` | 永続化に必要な候補の最低 confidence。 |
| `supersede_confidence_delta` | `0.05` | 候補が既存記憶の confidence を上回り、上書きするために必要な差。 |
| `semantic_similarity_threshold` | `0.85` | 2 つの記憶を意味的な重複とみなすコサイン類似度の閾値（以上で重複）。 |
| `dispute_confidence_gap` | `0.15` | 矛盾する候補が既存記憶を上書きせず係争中とマークする、confidence の差の下限。 |

4 つとも確率・比率であり、読み込み時に `0.0..=1.0` へ clamp されます。
特に `semantic_similarity_threshold` は埋め込みモデルの類似度分布に強く依存するため、
埋め込みプロバイダーを切り替えた際は再調整してください。

ユーザー確認へ回された候補（`AskConfirmationLater`）は `pending_candidates` キューに保持されます。
デスクトップの設定画面のレビュー一覧に加えて、これらはハイブリッド想起にも参加します。
話題が浮上したときにキャラクターが自然に確認できるよう、表面化した候補はプロンプト内で `[unconfirmed]` とマークされます。
`recall_pending_candidate_limit`（デフォルト `3`）はターンごとに競争へ参加する数を上限し、
`0` にすると設定画面のレビュー一覧に影響を与えずに想起経路を無効化できます。
この上限は `MindMemoryConfig` でコード調整でき、設定としてはまだ公開されていません。
承認・却下された候補は履歴としてキューに残り、保持スイープ（
`mind.memory.pending_candidate_retention`、コード既定 14 日 / 200 件）で削除される
まで、CLI の `/memory approval history` とデスクトップの履歴ビューから解決日時と
ともに確認できます。

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
        "enabled": true,
        "transport": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "@modelcontextprotocol/server-filesystem", "/home/user/allowed"]
        }
      }
    ]
  }
}
```

#### `plugins.mcp_servers` — MCP サーバーエントリ

各エントリは 1 つの MCP サーバーを宣言し、`name`（ルーティングとツールの名前空間にそのまま使われる）、`enabled`、`transport`、任意の `env_passthrough` リストを持ちます：

```jsonc
"mcp_servers": [
  {
    "name": "github",
    "enabled": true,
    "transport": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"]
    },
    "env_passthrough": ["GITHUB_PERSONAL_ACCESS_TOKEN"]
  },
  {
    "name": "local-dev",
    "enabled": true,
    "transport": {
      "type": "http",
      "url": "https://example.com/mcp",
      "auth_header": "Bearer <token>"
    }
  }
]
```

- `enabled` と `transport` は必須です。`enabled: false` にするとサーバーはスキップされます。
- `transport.type` は `"stdio"`（`command` を `args` 付きで子プロセスとして起動）または `"http"`（`url` に接続し、`auth_header` を `Authorization` ヘッダーとして送信。不正なヘッダーは認証なしへのダウングレードではなく接続拒否になります）。
- stdio の子プロセスは**クリアされた環境**で実行されます：転送されるのは `PATH`・`HOME`・`TMPDIR`・`LANG`・`TZ`・`LD_LIBRARY_PATH` と Windows 必須変数のみです。それ以外 — 特に API キー — はホスト環境にエクスポートし、`env_passthrough` でホワイトリスト登録する必要があります。サーバーごとのインライン `env` マップはありません。
- `mcp_servers` は配列のため、エントリは `settings.json` で宣言します：`ENE_` 環境変数はスカラー値（例：`ENE_PLUGINS__MCP_ALLOW_INSECURE_URLS`）の上書きはできますが、配列要素を追加することはできません。

HTTP の MCP エンドポイントは接続前に URL を検証します (既定では HTTPS のみ。
ループバックおよびクラウドメタデータ/リンクローカルアドレスは拒否)。`plugins` 内で
`"mcp_allow_insecure_urls": true` を設定すると、ローカル開発用にプレーン `http://` と
ループバック URL を許可できます。リンクローカルアドレスは拒否されたままです。
詳細は[プラグインと MCP](concepts/plugins-and-mcp.md) を参照。
サービス別の設定例（Calendar、Mail/Chat、Notes、Map、RSS）は
[MCP サーバー設定ガイド](guide/tools/mcp-servers.md) を参照してください。

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

バージョン 1 の `settings.json` は読み込み時に自動でマイグレーションされます。上記の移設対象キーは `plugins.list.*` の移動先へ移され（旧 `ai.*` の場所からは削除され）、その後にファイルが読み込まれて、マイグレーション後のドキュメントが永続化されます。対象キーを持たないファイルは論理的に変更されません。また、ネストした `config`/`profiles` 階層より前のレガシーなフラットなエントリレベルキー（`plugins.list.<name>.<key>`）も、起動時に配信される設定ブロブへ折り込まれます（明示的な `config` キーが優先）。この折り込みはディスク上のファイルを書き換えないため、リロードをまたいで安定しています。

バージョン 2 のファイルは読み込み時にバージョン 3 へマイグレーションされます。各
`ai.local_models.<name>` エントリが `plugins.list.llama-cpp.profiles.<name>` へ
ミラーされます（非空の `url`・`quantization`・`model_path`・`gpu_layers` のみ。
既存の非空のプロファイル値は上書きされません。既存の空値は「なし」とみなされます）。
`ai.local_models` 自体は無傷のまま
残ります — `ene-ai` はランタイムがプラグインへ切り替わるまで、ローカルタスクの
ルーティングとコンテキスト予算をここから読み続けます。

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
      },
      "llama-cpp": {
        "enable": true,
        "profiles": {
          "gemma-4-e4b": {
            "url": "https://example.com/gemma-4-e4b.gguf",
            "quantization": "Q4_0",
            "model_path": "",
            "gpu_layers": "33"
          }
        }
      }
    }
  }
}
```

ローカル GGUF プロバイダプラグイン（`ene-plugin-llama-cpp`）はモデルごとに
1 つのプロファイルを消費します：`url`（GGUF ダウンロード URL）、
`quantization`（ラベル、例：`"F16"` / `"Q4_0"`）、`model_path`（非空の場合は
ダウンロードをスキップするローカルパス）、`gpu_layers`（`"auto"` または整数
文字列）。プロファイルの*選択*はプラグイン側の責務で、値は
`ConfigurablePlugin::set_profiles` で配信されます。v2→v3 マイグレーションは
既存の `ai.local_models` エントリをこれらのプロファイルへミラーします。
ローカルモデルのキーはルーティング情報として `ai.local_models` に残ります。

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

### `scheduler.*` — 永続スケジューラーのポリシー

単発・インターバル・cron・起動時スケジュールを実行する永続スケジューラー
（`ene_runtime::scheduler::SchedulerConfig`）を制御します。スケジューラーは
メモリストア（`store.enabled`）を必要とし、無効な場合はどのスケジュールも
実行されません。スケジュール定義と実行履歴はストアのデータベースに保存され、
再起動後も復元されます。CLI の操作は
[スケジュールガイド](guide/schedules.md) を参照してください。

```json
{
  "scheduler": {
    "enabled": true,
    "late_grace_secs": 60,
    "confirmation_timeout_secs": 300
  }
}
```

- `enabled`（デフォルト `true`）— マスタースイッチ。`false` の間はどの
  スケジュールも実行されません。`ENE_SCHEDULER__ENABLED`。
- `late_grace_secs`（デフォルト `60`）— 予定時刻からこの秒数を超えて処理された
  実行（システムのサスペンド、時刻のズレ、アプリの終了中など）は
  `skipped_late` として記録され、**実行されません**。次の実行時刻は現在時刻から
  再計算されます。`ENE_SCHEDULER__LATE_GRACE_SECS`。
- `confirmation_timeout_secs`（デフォルト `300`）— ユーザー確認を待つスケジュール
  実行が、`timed_out` として記録されるまでの待機時間。
  `ENE_SCHEDULER__CONFIRMATION_TIMEOUT_SECS`。

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

### キャラクターの解決と列挙

キャラクターは `assets/characters/` 配下のフォルダーに格納され、カードファイル名は `character.json` です：

```
assets/characters/<name>/
  character.json
  character_settings.json
  model.vrm
  motions/VRMA_01.vrma
```

- `character` 設定は**ベア名**（`"Alicia"` など）を指定すると `assets/characters/Alicia/character.json` に解決され、**カードパス**（例: `assets/cards/ene.json`、相対・絶対どちらも可）を指定するとそのままカードとして読み込みます。
- 列挙（`ene characters list`、デスクトップのキャラクター選択）も同じ規則を使用します: `character.json` を含むフォルダーのみがキャラクターとして扱われます。旧来の誤記 `charactor.json` は受け付けません。
- **未設定と不在の区別。** 空の `character` 値は「キャラクター未選択」のエラーとなり、ハードコードされた既定キャラクターへ黙ってフォールバックしません。空でない名前でカードファイルが存在しない場合は「ファイル不在」として報告されます。
- **パス検証。** `..` によるパストラバーサルは拒否されます（キャラクター名は第三者製のカード配布物から来るため）。

---

## 4. スキーマ自動生成

設定スキーマは、各所有クレートの `define_config!` マクロによって宣言されます。スキーマは設定読み込みのたびにではなく、アプリケーション起動時（CLI の `init`、デスクトップの初回起動、ランタイムの open 経路）にプロセスあたり一度だけ書き込まれます。各スキーマファイルはアトミックに（一時ファイル + `fsync` + リネームで）書き込まれるため、クラッシュしても途中で切れたスキーマファイルが残ることはありません。

`settings.json` を保存する際、Ene は相対的な `$schema` ポインタ（`./schema/settings.schema.json`）をファイル先頭に自動で書き込みます。これにより、キーを手書きしなくてもエディタが補完とバリデーションを提供します。既存の `$schema` 値はそのまま保持され、ポインタは存在しない場合にのみ補完されます。同様に、ユーザーが手動で並べ替えたトップレベルのセクション順も保存をまたいで維持され、新しく追加されたセクションは末尾に追加されます。

> [!CAUTION]
> `assets/schema/*` 下の無視されたスキーマファイルを直接手修正したりコミットしたりしないでください。
