# 設定

設定の優先順位は defaults → JSON → `ENE_` 環境変数です。
ネストしたキーは `__` で区切ります（例: `ENE_CORE__SERVER__BIND`）。

コアはデータディレクトリの `settings.json` を、他と同じ `ene-config` の
figment パイプライン（defaults → JSON → `ENE_`）で読みます。ファイルが無いとき
は `{}` として扱い、壊れた JSON では起動に失敗します。`ene-core` で `ENE_*` を
手で重ねないでください。リポジトリの
`assets/settings.json` は開発用サンプルであり、実行時ファイルではありません。
`ene-ctl` と `ene-stage` は `--url` / `--token`（または `ENE_API_URL` /
`ENE_API_TOKEN`）で起動済みコアへ接続します。これらの環境変数が無いとき、
`ene-stage` は `ene-core` を起動します。

データディレクトリは `ENE_DATA_DIR` があればそれです。無ければデバッグビルドは
リポジトリの `assets/` を設定・DB・vault・workspace の根にし、OS のデータ
ディレクトリには書きません。リリースは OS のデータディレクトリだけを使い、
リポジトリの `assets/` は読みません。stage の適用とコアの PATCH は同じ
`settings.json` に書きます。`GET /api/v1/settings` の `effective` はライブ
メモリが正で、ディスクの `overlay` は AI / mind / plugins のライブ値を
上書きしません。`body` / `voice` の PATCH は再起動なしでライブの `Stage` と
`VoiceRuntime`（同時表示上限、barge-in）にも入ります。
`store.sessions.synchronous` は `sessions.db` を開くときに適用します。
API キーは vault のままです。

キーは所有側の `define_config!` に足します。スキーマは `assets/schema/` へ
再生成されます（gitignored — コミットしない）。

| セクション | 所有 | 代表キー |
|---|---|---|
| `core` | `ene-kernel` | `server.bind`, `server.token_file`, `backup.*`, `clients.*` |
| `harness` | `ene-kernel` | `loop.max_steps_per_turn`、`retry.*`、`context.*`、`delegation.*`、`tool_output.soft_limit_bytes`、`tool_output.hard_limit_bytes` |
| `mind` | `ene-companion` | `inner.*`, `affect.*`, `recall.*`, `memory_approval.*`, `proactive.*`（`observation_interval_seconds` が観測 tick 間隔。開いているセッションをすべて観測する。`proactive.world_state.title_mode` と `ocr_hint` を含む） |
| `characters` | `ene-companion` | `home_dir`, `import_v3` |
| `body` | `ene-body` | `render.*`, `autonomy.*` |
| `voice` | `ene-body` | `enabled`, `barge_in.*`, `input.routing` |
| `store` | `ene-session` | `sessions.db_path`, `sessions.idle_timeout_secs`, `sessions.synchronous` |
| `approval` | `ene-access-control` | `mode`, `popup.timeout_ms` |

`core.backup.skills_max_bytes` は手動バックアップへコピーする `skills/` ツリーの
上限です。既定値は 100 MiB です。

会話・分類・埋め込み・TTS・STT・承認・ジョブは `ai.tasks.<task>`（`plugin`、
`model`、`model_path`、`base_url`、`voice`、`max_tokens`、`supports_images`、
`context_window`）でバインドします。
チャットは未設定のまま起動するので、最初のメッセージの前に
`ai.tasks.chat.plugin` を `provider.*` に設定してください。
`supports_images` はオプトインで既定は false です。設定済みかつフラグが真の
バインディングだけが `ImageRef` を `LlmImage` に畳み、text-only や能力不明の
provider は `[image omitted]` のままにします。
`approval.mode = ai_auto` は `ai.tasks.approve`（無ければ chat）を使い、失敗時は
ポップアップに落ちます。裏層ジョブは `ai.tasks.job`（無ければ chat。どちらも
未設定なら echo）を、対話レーンとは別のレーンで使います。複数プロバイダの
フェイルオーバーはありません。空のタスク行は chat を継承します（TTS / STT /
埋め込みは空のまま無効）。API キーは vault 秘密です（起動時は
`ENE_AI__TASKS__<TASK>__API_KEY`。PATCH `/api/v1/settings` は JSON に
書きません）。プラグイン id は [プラグイン一覧](concepts/plugins-and-mcp.md) の
`provider.*` です（`GET /api/v1/settings` の `effective.providers`）。
デスクトップは別の許可リストを持ちません。

ローカル GGUF 会話は `provider.gguf`（`local: true`）です。重みと
`llama-server` は AI / Engines の `provider.assets` からインストールします。
任意で `model_path` / `server_path` がカタログを上書きします。クラウド会話は
インストール済みの LLM プラグイン（API キーは vault）です。

埋め込みは任意で、独自の `ai.tasks.embedding` ファイバーです。未設定、
ローカル GGUF（`provider.gguf`、おすすめ Jina）、または `seam.embed` の
クラウドプラグイン。分類・能動発話が未指定なら会話モデルの値を継承します。
TTS・STT が空なら無効のままです。

観測のプライバシーは `mind.proactive.world_state` です。`title_mode` は
`app_only`（既定）、`redacted_title`、`full_title`。`ocr_hint` はローカル
opt-in の枠で、バックエンドは同梱しません。製品 GUI（詳細 → Conversation）
がいまの送信範囲を出します。生のスクリーンショットは session / memory /
audit に残さず、luma digest とテキスト要約だけが永続境界を越えます。

プロバイダ LLM 呼び出し（`ai.tasks.chat` / `job` / `classifier` / `approve`）は
一時障害（`429` / `502` / `503` / timeout / overload）を `harness.retry` で
再試行し、最後の失敗がターンまたはヘルパーのエラーになります。実効コンテキスト
ウィンドウは `min(プロバイダ申告, ai.tasks.<task>.context_window)`、どちらも無ければ
8192 です。`harness.context.response_reserve_tokens`（`max_tokens` があればそれ）と
`safety_margin_ratio` を引き、対話モデルは system 以外の古いメッセージを落として
パックします。プラグイン hello はまだ窓を申告しないので、大きいモデルは
`context_window` で上限を付けます。

| キー | 役割 |
|---|---|
| `harness.retry.max_attempts` | 初回を含む総試行回数。既定 `3`。 |
| `harness.retry.backoff_ms` | 再試行可能な失敗のあとの待ち（ms）。既定 `[500, 2000, 8000]`。 |
| `harness.context.response_reserve_tokens` | `max_tokens` 未設定時の応答予約。既定 `4096`。 |
| `harness.context.safety_margin_ratio` | 窓のうち見積もり誤差用に残す比。既定 `0.1`。 |
| `harness.context.token_estimation` | `auto`（CJK 判定）、`chars4`、`cjk15`。 |
| `ai.tasks.<task>.context_window` | オペレータが付ける窓の上限。環境変数: `ENE_AI__TASKS__<TASK>__CONTEXT_WINDOW`。 |

プラグイン起動は `plugins.profile`（`desktop` / `minimal` / `headless`）です。
プラグインごとの有効マップ（`plugins.list`）はありません。

| キー | 役割 |
|---|---|
| `plugins.profile` | 起動ツリー。既定 `desktop`。環境変数: `ENE_PLUGINS__PROFILE`。 |
| `plugins.home_dir` | インストール検索パス。空なら `<data>/plugins`。環境変数: `ENE_PLUGINS__HOME_DIR`。 |
| `plugins.policy.approval_mode` | 起動時に `approval.mode` を初期化（`ask_all` / `policy` / `ai_auto` / `auto`）。実行時の正は `approval.mode`。 |
| `plugins.policy.allow_unverified` | digest 不一致でも起動するか。既定 `false`。 |
| `plugins.ipc.max_frame_bytes` | IPC フレーム上限。既定 `1048576`。環境変数: `ENE_PLUGINS__IPC__MAX_FRAME_BYTES`。 |
| `plugins.ipc.bulk_threshold_bytes` | これを超える本体は MessagePack フレームに載せない（`stream.open` / Unix の `SCM_RIGHTS`）。既定 `65536`。環境変数: `ENE_PLUGINS__IPC__BULK_THRESHOLD_BYTES`。 |

MCP サーバーは手書きの `mcp.json` であり、設定キーではありません。
[プラグインと MCP](concepts/plugins-and-mcp.md) を見てください。
