# 製品境界

このページは、**どのクライアントが製品 GUI か**、旧 harness / 旧 desktop
の機能がどこへ行ったか、未達が v1.0 か post-v1.0 かを分類する判断表です。

正は `main` のコードと、[アーキテクチャ](architecture.md) および
[`plans/harness-redesign/`](../../../plans/harness-redesign/README.md)
の所有境界です。旧ツリーに存在したことだけでは復元理由になりません。
v1.0 の追跡は [#717](https://github.com/pexisgle/ene/issues/717)
のままです。このページがその定義を広げてはいけません。

## 判断原則

- **コードとクレート境界が勝つ。** 復元された UI 文言より先に見る。
- **製品 GUI は `ene-stage`。** 伸ばす対象は stage。`ene-desktop` との
  feature parity は要求しない。
- **`ene-desktop` は凍結したレガシー。**
  [PR #794](https://github.com/pexisgle/ene/pull/794) が、stage を伸ばすあいだ
  旧 UX を失わないために復元した。機能追加はしない。第二の製品にしない。
  stage が、今も製品として要る desktop の能力を代替できたと判断したら
  **`ene-desktop` を削除する。** その判断までは参照用にツリーへ残す。
- desktop の能力を stage へ移植するのは、現行 API と境界が今も求めるときに
  限る。desktop にあること自体がその判断ではない。
- **MCP に委ねた領域は MCP のまま。** git / browser / calendar /
  Home Assistant / geo 向けにホスト固有の OAuth や API クライアントを
  再導入しない（[D-23](../../../plans/harness-redesign/tools/capabilities.md)）。
- **復元済みは completed ではない。** desktop にしか無いページや
  no-op スタブは製品完成ではない。欠けは stage（またはコア）で埋める。
- **still missing** は open issue か、明示的な非目標を付ける。

表の Status:

| Status | 意味 |
|---|---|
| Current | 実装済みで、現行 API / プラグインパイプラインに載っている |
| Unconnected | クライアントに残っているが、現行コアに対する完成ではない |
| MCP | 意図的に委譲。v1.0 は手書きの `mcp.json` 行 |
| Dropped | 内製ツール / ホストコネクタとしては戻さない |
| Missing | 必要。リンク先の issue で追跡 |
| Post-v1.0 | 後継。[#717](https://github.com/pexisgle/ene/issues/717) のクローズ条件ではない |

## 1. クライアント責務

すべてのクライアントは `ene-api` 上の対等なピアです。desktop 専用 HTTP
はありません。排他資源（マイク、スピーカー、承認応答、OS 通知）は
`ene-core` が調停します。

| クライアント | 製品 status | 所有 | 所有しない | 検証 |
|---|---|---|---|---|
| `ene-stage` | **製品 GUI**（`client_id = stage`） | ローカル `ene-core` の起動/停止、wgpu オーバーレイ、表層チャット、9 セクションの詳細 IA（Home / Companion / Conversation / Voice / Memory / Work / Connections / System / Log）、トレイ・字幕・スポットライト・ホットキー、音声中継、承認ポップアップ、`notify.hint` | カーネル、コンパニオン永続化、承認ポリシー、ボールト、プラグイン監督。CCv3 アプリ内エディタ、旧 desktop の観測パイプライン | 製品経路: オーバーレイ、Conversation の束縛、設定適用、承認、排他資源。Linux と native Windows CI は `-p ene-stage` |
| `ene-desktop` | **凍結した旧 GUI**（`client_id = desktop`） | 再設計前の 18 ページ設定/管理 IA、CCv3 エディタ、#794 で復元したオーバーレイ/トレイ | 機能追加。出荷クライアントであり続けること。v1.0 E2E の必須クライアントであること | 削除までの旧 UX カタログ。機能は足さない。Linux CI はまだ `-p ene-desktop`。native Windows clippy の対象は今のところ stage。stage が代替できたと判断したらクレートを削除する |
| `ene-ctl` | CLI | テキスト対話、セッション/ツール/プラグイン/ジョブ/記憶/スケジュール/コア制御、詳細深さの `ene debug` | オーバーレイ、OS 通知、マイク/スピーカー | 配線上 stage ができること。`cargo test` の default member |
| Web（`apps/ene-core/web`） | LAN / トンネル | 表層チャットと読み取り専用の詳細（内面、thinking、ツール、PAD、記憶、ジョブ） | 設定変更、記憶削除、キャラ管理、VRM | テキスト経路と D-31。詳細 UX はまだログ寄り（[#717](https://github.com/pexisgle/ene/issues/717)） |
| モバイル | Post-v1.0（M1） | — | — | ツリーに無い |

`ene-stage` と `ene-desktop` は同じ API を話し、どちらも egui + wgpu で、
WebView は使いません。[PR #794](https://github.com/pexisgle/ene/pull/794)
は stage を伸ばすあいだ旧 UX を失わないために desktop を復元しましたが、
**製品の席は `ene-stage`** です。計画文書の `desktop(stage)` はその役割の
名前です。desktop は凍結のみ。stage が、製品として要る desktop の能力を
代替できたと判断したら `apps/ene-desktop` を削除します。排他資源の第二
クライアントは CLI か Web で足り、そのために desktop を残しません。

## 2. ツール移行

旧ビルトインは harness 書き換え前の `plugins/tool/` にありました。
現行は `fs` / `exec` / `web` / `utility` / `app` で、サードパーティと
同じ IPC です。成熟した外部サービスは MCP であり、ホストコネクタでは
ありません。

### `fs`（内製）

| 旧 action | 現行 | Status | 注記 |
|---|---|---|---|
| `read` | `fs.read` | Current | ワークスペース閉じ込め、親の canonicalize |
| `write` | `fs.write` | Current / Missing | 表層非公開（`side_effects: ["fs.write"]`）。atomic replace、行末保持、job 単位 undo は [#797](https://github.com/pexisgle/ene/issues/797) |
| `edit` | `fs.edit` | Current / Missing | 許容フォールバック照合を復活。precondition hash、曖昧さエラー、CRLF/BOM は [#797](https://github.com/pexisgle/ene/issues/797) |
| `patch` | `fs.patch` | Current / Missing | hunk 文脈照合はある。atomic/precondition は edit と同じ [#797](https://github.com/pexisgle/ene/issues/797) |
| `search`（grep / regex） | `fs.search` | Current | ホスト `rg` に委譲。既定はリテラル、`regex` で正規表現、旧 grep オプションあり |
| `search` glob / パス列挙 | — | Missing | [`fs.glob` と FileBroker list](https://github.com/pexisgle/ene/issues/813) |
| `delete` | — | Missing | 承認付き delete をホスト FileBroker で [#813](https://github.com/pexisgle/ene/issues/813) |
| `undo` | `fs.undo` | Current / Missing | 同一 job のみ（`job_id` / `ENE_JOB_ID`）。ジャーナルの原子性と秘密除外は [#797](https://github.com/pexisgle/ene/issues/797) |
| escape 正規化と境界限定の編集戦略 | — | Dropped | indent・行 trim・block anchor フォールバックは維持。複数候補はエラー |
| regex playground | — | Dropped | [#813](https://github.com/pexisgle/ene/issues/813) の範囲外 |
| プラグインからの直接 FS（workspace env 超え） | — | Missing | 閉じ込めの単一境界はホスト FileBroker [#813](https://github.com/pexisgle/ene/issues/813) |

### `exec`（`fs.shell` から分離、D-24）

| 旧 action | 現行 | Status | 注記 |
|---|---|---|---|
| `fs.shell` | `exec.run` | Current / Missing | 別プラグイン・別承認軸。出力上限、process tree 終了、cwd/env は [#798](https://github.com/pexisgle/ene/issues/798) |
| コマンド文字列の blocklist | — | Dropped | 安全境界にしない |
| `exec.pty` | — | Post-v1.0 | capabilities.md にあるが現行 specs には無い。持続 PTY は1つが上限 |

### `web`（内製）

| 旧 action | 現行 | Status | 注記 |
|---|---|---|---|
| `webfetch` | `web.fetch` | Current | `format` は `markdown`（既定）/ `text` / `html`。HTML は見出し・段落・link を保つ。binary は `binary_content`。byte と変換後文字数に上限 |
| HTML → 読みやすい Markdown | `web.fetch` `format=markdown` | Current | script/style/nav を除き、title と元 URL を残す |
| `websearch` | `web.search` | Current | `backend` は `duckduckgo`（既定・無credential）、`arxiv`（domain、同じ結果形）、`tavily`/`exa`（vault まで `credential_missing`）。`web.search_backends` が一覧 |
| 有料検索 backend | `web.search_backends` | Current | 宣言する。vault 資格情報なしでは選ばない |
| ブラウザ自動化 | — | MCP / ビルトインとしては Dropped | Playwright 系 MCP。`tool.web` ではない |

### `app`（内製）

| 旧 action | 現行 | Status | 注記 |
|---|---|---|---|
| `screenshot` / `capture_window` | `app.screenshot` | Current | Wayland は portal 優先、CLI フォールバック、Windows は GDI。capture JSON に size/scale/permission。モデル呼び出しは `ImageRef` + spill blob（inline base64 ではない）としてログし、`ai.tasks.<task>.supports_images` のときだけ `LlmImage` に畳む。[App プラットフォーム表](../guides/tools/app-platform.md) |
| `list_windows` | `app.window_list` | Current | wmctrl / hyprctl / sway。GNOME/KDE Wayland は `app.capabilities` で unsupported |
| `get_active_window` | `app.active_window` | Current | 能動観測ソース（画面観測が有効なとき） |
| `list_monitors` | `app.list_monitors` | Current | compositor がレイアウトを出すとき capture の scale/size と揃える |
| `clipboard_read` / `write` | `app.clipboard_get` / `app.clipboard_set` | Current | native（`arboard`）優先。CLI フォールバックは payload で明示 |
| `mouse_click` / `type_text` / `press_key` / `key_combo` | `app.click` / `app.type` / `app.key` | Current | X11/Windows のみ公開。`side_effects: ["input"]`。表層スキーマには出ない |
| `mouse_move` / `drag` / `scroll` / `focus_window` | — | Dropped / プラットフォーム限定 | GNOME/KDE Wayland 入力は公開しない |
| portal セッション寿命 | `app.screenshot` の `code` | Current | `waiting` / `denied` / `cancelled` / `unsupported` / `unavailable` |

### `utility` / `calc` / `random`（再分類、D-25）

| 旧 action | 現行 | Status | 注記 |
|---|---|---|---|
| `calc.evaluate` | `utility.calc`（`expr`） | Current / Missing | `+ - * / ^` と括弧。`sin` / `max` / `pi` / 変数は [#814](https://github.com/pexisgle/ene/issues/814) |
| `calc.unit` | `utility.calc`（`value`+`from`+`to`） | Current / Missing | 長さ・質量・時間・データ・温度。体積 / 速度 / 面積は [#814](https://github.com/pexisgle/ene/issues/814) |
| `calc.color` | — | Missing | sRGB hex/rgb/hsl/alpha [#814](https://github.com/pexisgle/ene/issues/814) |
| `calc.currency` | `utility.calc` の FX スナップショット | Current / Missing | `as_of` / `source` はある。`stale` と live feed は別 |
| `random.number` / `pick` / `uuid` | `utility.random` | Current / Missing | float、pick、UUID v7。整数範囲の unbiased sampling は [#814](https://github.com/pexisgle/ene/issues/814) |
| `random.color` | — | Missing | [#814](https://github.com/pexisgle/ene/issues/814) |
| `utility.time` / `system_info` / `hash` / `text` | 同名 | Current | ハッシュ、encode/decode、正規表現 |
| `utility.question` | ハーネスの `ask-user` | ツールとしては Dropped | コアのレーン。プラグインではない |
| `utility.notify` | クライアントの `notify.hint` | ツールとしては Dropped | desktop/stage の OS 通知。CLI は出さない |
| `utility.timer` | スケジュール | ツールとしては Dropped | `ene-work`。quiet hours / `important` |
| `utility.todo` | ジョブ | ツールとしては Dropped | public 委譲 / タスク一覧 |
| `counter.*` | — | Dropped | 状態付きサンプル。需要なし |

### MCP へ委譲（D-23）— ビルトインとしては戻さない

v1.0 の接続は手書き `mcp.json` 行で、同じレジストリパイプラインに載ります。
stage は同じ行の上に公式サーバーの厳選カタログを重ねました
（[#812](https://github.com/pexisgle/ene/issues/812)、P-616）。

| 旧プラグイン / action | 委ね先 | Status |
|---|---|---|
| `git.status` / `log` / `diff` / `branch` / `blame` / `remote` | git MCP | MCP（プロセス受入は実 git の stdio fixture） |
| `browser.navigate` / `click` / `type_text` / `get_content` / `screenshot` / `scroll` / `wait` / `close` | Playwright 系 MCP | MCP |
| `calendar.list_*` / `create_event` / `update_event` / `cancel_event` / `find_free_slots` / アカウント | calendar MCP | MCP — ホスト OAuth クライアントは持たない |
| `homeassistant.state` / `turn` / `climate` | Home Assistant MCP | MCP |
| `geo.weather` / `location` / `timezone` / `sun` | geo/weather MCP | MCP |

## 3. 旧 desktop 機能表

[PR #794](https://github.com/pexisgle/ene/pull/794) で復元した **旧 GUI**
の一覧です。製品作業は、現行 API が今も求めるものだけ `ene-stage`
（またはコア）へ移植します。desktop が既に `ene-api` を話していても、
クレートを残す理由にはしません。

### 設定・管理ページ

| 旧ページ | desktop 上 | stage の行き先 | Status |
|---|---|---|---|
| Overview | health / needs-config | Home | Home が薄いなら stage で伸ばす |
| General（グラフィックス、アクセシビリティ、言語、テーマ、字幕、ホットキー） | ローカル `desktop.*` | System + Voice（字幕） | stage にテーマ/言語/字幕/オーバーレイはある。アクセシビリティやホットキーの深さは移植するまで desktop |
| Character | occupants / bodies は HTTP、配置はローカル | Companion | 両方に Current |
| Character editor（CCv3） | ローカル `character.json` を `ene-card` で I/O | なし | 移植しない。v1.0 はパッケージインポート（`P-803`）。desktop を残す理由にしない |
| AI / Voice / Engines | `GET/PATCH /settings`、`providers`、`provider.assets` | Conversation、Voice、Connections | stage に Current。desktop にだけ残る操作があれば stage へ |
| Features（能動発話のトグル） | `mind.proactive.*` の PATCH | Conversation | 観測プライバシー（`title_mode`、`ocr_hint`、送信範囲）は stage で Current。他の能動トグルはまだ増やせる |
| Memory 設定 + Memories 台帳 | Memory HTTP | Memory | 一覧/編集/削除は stage で Current。補助 LLM の scope は [#717](https://github.com/pexisgle/ene/issues/717) |
| Sessions | Session HTTP | Log | stage で Current |
| Permissions / Approvals | Plane HTTP | System | stage で Current |
| Connectors（MCP フォーム + カタログ） | `GET/PUT /mcp`、`GET /mcp/catalog`、`POST /mcp/probe` | Connections | 手書きフォームに加え、公式サーバーのカタログ選択・有効化前の probe によるツールプレビュー・状態/エラー表示が stage で Current（[#812](https://github.com/pexisgle/ene/issues/812)） |
| Schedules | Schedule HTTP | Work | stage で Current |
| Plugins / Advanced / Diagnostics | プラグインプロファイルと schema 葉 | System | stage で Current。plugin config schema は [#819](https://github.com/pexisgle/ene/issues/819) |

### オーバーレイとプラットフォーム

| 機能 | 場所 | Status |
|---|---|---|
| wgpu VRM オーバーレイ、視線、スプリング、ビセーム | stage + desktop | stage が製品として Current |
| クリック透過、入力領域、Wayland layer-shell / mask | stage + desktop | stage で Current。desktop 側の追加 gizmo は移植するまで旧版 |
| トレイ、字幕、スポットライト、ホットキー | stage + desktop | stage で Current |
| 音声 PCM 中継、speaker/notify の排他 | stage + desktop | stage で Current |
| beat sync、画質 | 両方（クライアントローカル） | stage に画質/配置はある。beat-sync の深さは移植するまで desktop |

### 観測（旧 `proactive_observe`）

旧 desktop は ROI crop、luma fingerprint、OCR、画面要約を持っていました。
復元された desktop のコントロールは **no-op スタブ** です。このパイプラインを
desktop 内に作り直さず、`ene-work` / `ene-companion` に置き、プライバシー
設定は **stage** に出します。

| 部品 | 所有者 | Status |
|---|---|---|
| スクリーンショット能力 | `app` ツール / クライアント | CLI 経路は Current。portal は [#800](https://github.com/pexisgle/ene/issues/800) |
| ROI、luma fingerprint、changed-cell gate、caret 抑制 | `ene-work` の観測パイプライン | Current |
| タイトルの redaction（AppOnly / RedactedTitle / FullTitle） | `ene-companion` の設定 + `ene-work` の送信ラベル | Current（`mind.proactive.world_state.title_mode`） |
| 能動発話 / 世界状態 | `ene-companion` + コア tick | Current: 開いているセッションすべて。間隔は `mind.proactive.observation_interval_seconds`。無変化フレームは前の要約を再利用 |
| session / memory / audit への raw pixel | 禁止 | Current。digest とテキスト要約のみ |
| desktop の `ProactiveObserveControl` | クライアントのスタブ | Unconnected |

## 4. セキュリティ差分

| 層 | 現行 | ギャップ |
|---|---|---|
| OS サンドボックス（`ene-sandbox`） | Linux は Landlock + seccomp + rlimits | Windows AppContainer は設計目標であり、現行経路としては主張しない |
| ホスト FileBroker（`ene-plugin-host`） | read/write の `confine_path`。レジストリも `fs.*` 引数を書き換え | プラグインは `ENE_WORKSPACE` を受け取りファイルに触れる。list/glob/delete と TOCTOU は [#813](https://github.com/pexisgle/ene/issues/813) |
| ホスト net broker | 私的/loopback/link-local 拒否、DNS 固定、redirect なし、1 MiB | `web` プラグインは reqwest で迂回し最大 4 redirect [#799](https://github.com/pexisgle/ene/issues/799) |
| 資格情報 | ボールト（`vault.bin` + `vault.key`）。プラグイン環境へ生キーを出さない | 検索 backend（[#818](https://github.com/pexisgle/ene/issues/818)）と plugin config（[#819](https://github.com/pexisgle/ene/issues/819)）も vault 参照。MCP 向けホスト OAuth は持たない |
| 承認（`ene-access-control`） | deny-by-default、hash chain、ポップアップ、「次から確認しない」 | 本番の AI 自動承認モデルは未設定（[#717](https://github.com/pexisgle/ene/issues/717)） |
| `exec` | 直接の子へ SIGTERM のあと SIGKILL | process tree の所有、出力 byte 上限、cwd/env allowlist は [#798](https://github.com/pexisgle/ene/issues/798) |
| raw pixel | 観測要約はセッションログの外 | Current: session / memory / audit は digest と要約であり PNG ではない |

## 5. v1.0 と post-v1.0

[`product/done.md`](../../../plans/harness-redesign/product/done.md) と
[`product/features.md`](../../../plans/harness-redesign/product/features.md)
に合わせます。子 issue の close だけでは
[#717](https://github.com/pexisgle/ene/issues/717) を閉じません。

### v1.0（#717 の中で完了するか、明示的に対象外へ移す）

- 製品 GUI は `ene-stage`。伸ばす。CLI と Web はピア。`ene-desktop` は旧 GUI であり、v1.0 E2E の主クライアントではない。
- 同梱 `fs` / `exec` / `web` / `utility` / `app` を共有レジストリへ。
- 手書き MCP（実 stdio サーバーを1つ、例: git）。
- §4 で `done.md` が既に主張している境界に加え、高優先のツール強化:
  [#797](https://github.com/pexisgle/ene/issues/797)、
  [#798](https://github.com/pexisgle/ene/issues/798)、
  [#799](https://github.com/pexisgle/ene/issues/799)、
  [#813](https://github.com/pexisgle/ene/issues/813)。
- モデルを埋めず raw pixel を残さない観測:
  現行（`ene-work` のゲートと stage のプライバシー操作）。
- `done.md` の未チェック（実プロバイダ会話、本番 ASR/TTS、ジョブランナーの
  発話、GUI E2E）は #717 のまま。

### Post-v1.0（#717 を止めない）

| 項目 | Issue / ID | 後回しにする理由 |
|---|---|---|
| MCP カタログ、導入プレビュー、health、認証 UX | [#812](https://github.com/pexisgle/ene/issues/812)、P-616、M8 | post-v1.0 完了: 静的カタログ、有効化前の probe プレビュー（ツールごとの副作用つき）、認証必須状態を含むファイバーエラー表示、vault 経由の手動 Bearer トークン注入 |
| ツール discovery index | [#817](https://github.com/pexisgle/ene/issues/817)（epic [#796](https://github.com/pexisgle/ene/issues/796)） | scoring は `ene-tool-registry`。`done.md` の箱ではない |
| background tool の start/cancel/completion | [#816](https://github.com/pexisgle/ene/issues/816)（epic #796） | 永続は `ene-work` のジョブ。第二の task store は作らない |
| plugin config schema / dynamic options | [#819](https://github.com/pexisgle/ene/issues/819)（epic #796） | 未リリースなので旧 config shim は不要 |
| 読みやすい Markdown と検索 backend | [#818](https://github.com/pexisgle/ene/issues/818) | 出荷: fetch の markdown/text/html、DDG+ArXiv、有料 backend は未設定として宣言 |
| portal 優先の capture/clipboard | [#800](https://github.com/pexisgle/ene/issues/800) | 現行 v1.0 の能力は CLI 経路 |
| utility の数式/単位/色/乱数の不足 | [#814](https://github.com/pexisgle/ene/issues/814) | 低優先。eval や任意コード実行は入れない |
| `exec.pty`、デスクトップペット、カメラ、Live2D、モバイル | features.md の後継 ID | 形式が支える。v1.0 ではない |
| MCP 領域向けホスト OAuth / サービス API クライアント | — | 非目標（D-23） |
| ツリー内の git/browser/calendar/HA/geo/counter | — | Dropped |
| stage 上での desktop 機能対比 | — | 非目標。選んだ UX を stage へ移植し、desktop は凍結 |
| `ene-desktop` を出荷し続ける | — | 非目標。stage が製品として要る能力を代替できたと判断したら削除 |

### Epic #796

[#796](https://github.com/pexisgle/ene/issues/796) は tracker だけです。
実装するのは [#817](https://github.com/pexisgle/ene/issues/817)、
[#816](https://github.com/pexisgle/ene/issues/816)、
[#819](https://github.com/pexisgle/ene/issues/819) です。第二の job store、
wire ABI への scoring 漏洩、plugin schema 応答への秘密混入はしません。

## 次に読むもの

- [Stage ユーザーガイド](../apps/stage.md)
- [Desktop ユーザーガイド](../apps/desktop.md)（旧版）
- [CLI ユーザーガイド](../apps/cli.md)
- [同梱ツール](../guides/tools/builtin-tools.md)
- [サンドボックスと承認](sandbox-and-approvals.md)
- [プラグインと MCP](plugins-and-mcp.md)
