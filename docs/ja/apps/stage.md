# Stage ユーザーガイド

`ene-stage` が製品 GUI です。必要なら `ene-core` を起動し、`ene-vrm` で
**透過 wgpu オーバーレイ** にコンパニオンを描き、会話は **表層**
WebSocket、Home / Companion / Conversation / Voice / Memory / Work /
Connections / System とセッションログは **別窓の詳細** に出します。

`ene-desktop` は凍結した再設計前の GUI です。機能追加はせず、機能対比も
要求しません。stage が、製品として要る desktop の能力を代替できたと
判断したら desktop を削除します。役割は
[製品境界](../concepts/product-boundaries.md) にあります。

```sh
cargo build -p ene-core -p ene-stage
cargo run -p ene-stage
```

自動のローカルコア起動にはビルドが必要です。Windows ネイティブ開発では
安定版 MSVC Rust、Visual Studio C++ Build Tools、Windows SDK を入れたうえで
PowerShell から同じコマンドを使います。

| ウィンドウ | 深さ | 内容 |
|---|---|---|
| キャラクターオーバーレイ（wgpu） | `surface` | VRM、VRMA、スプリング、視線、ビセーム。Space で枠。クリック透過はシステム → オーバーレイのクリック透過（既定オン）。Esc で終了。VRM 体は同時に最大2体（`body.render.max_concurrent`、既定 2）。A/D はチャットが対象にするソウルを切り替え、両方はオーバーレイに残る。W/S でアクティブな体のモーション。F3 でスプリングボーンのコライダーをワイヤーフレーム表示する。入力欄にフォーカスがなければチャット/詳細からも同じショートカットが届く。 |
| チャット | `surface` | Prompt / Steer / Follow-up（ホバーで意味）、承認（許可 / 常に許可 / 拒否）、ask-user（`question.asked` → `POST /jobs/{id}/answer`）、マイク PCM 中継、詳細ボタン（トレイと同じ）。ステータスは独立した行で折り返し、狭い窓でも設定エラーが読める。 |
| キャプション | `surface` | 発話の字幕。音声タブの Caption position（`top` / `bottom` / `left` / `right`）で位置を決め、Pin caption でドラッグを止めます。プロバイダ / HTTP エラーはチャットのステータス行（折り返し）に留め、字幕には出さない。ターン終了でオーバーレイは閉じる。長い発話はオーバーレイ内で折り返す。 |
| スポットライト（Alt+Space） | ローカル | 詳細セクションへジャンプ、マイク、終了。コマンドを選ぶとパレットは閉じます。OS が Alt+Space を掴んでいるときは音声 → スポットライトを開く |
| 詳細（トレイまたはチャット → 詳細） | `detail` | 設定 IA、内面 / thinking / ツール / PAD のログ。検索はセクションを絞り、タブやホームのショートカット、スポットライトを押すとフィルタを消すので、今のタブに固定されません。ログが空のときは空状態と次の操作を出す。 |

Stage は WebView を使いません。オーバーレイは wgpu、操作窓は winit 上の egui
です。コアとは `ene-api` のみで話し（`client_id = stage`）、`ene-core` や
`ene-companion`、`ene-card` はリンクしません。

キャラクターは `.enechar` パッケージです。`GET /characters` はインストール在庫、
対話相手はソウル（`GET /souls` / `GET /stage` の occupants）です。`body_ref` は
ボディ UUID です。Stage は同梱 Alicia VRM を `char.alicia@1.0.0` と
`char.alicia-b@1.0.0` としてインポートし、2体を同時に描けるようにしてから
HTTP 経由で soul 化します。CCv3 / PNG / CHARX は変換入力だけで、
CCv3 エディタはありません。Companion の書き出しと Work のセッション書き出しは、
ドキュメントまたはダウンロードで、拡張子付きの名前（`.enechar` / `.json`）を
提案する保存ダイアログを開きます。

ローカルの `desktop.*`（テーマ、言語、マイク、キャプション、ビート同期、画質、
コア寿命、オーバーレイ配置）はクライアント側です。テーマ（`light` / `dark` /
`system`）は wgpu のクリア色と egui の文字色の両方に効くので、light でも
コントラストが保たれます。日本語 UI は OS の CJK フォント（Windows は游ゴシック
/ メイリオ、Linux は Noto や Droid）で描画し、バイナリ横に
`assets/fonts/NotoSansJP-Regular.ttf` がある場合はそれを使います。表示言語を
変えると、開いている操作窓のタイトルも再起動なしで更新されます。
コアの PATCH キーは
`core` / `harness` / `approval` / `theme` / `ai` / `mind` / `plugins` です。
プラグインの有効化は `plugins.profile`（`desktop` / `minimal` / `headless`）で、
`plugins.list` の個別マップはありません。API キーはボールトに置きます。
Connections の MCP は名前・ローカルコマンドまたは HTTP・引数/URL のフォームで
編集し、プラグイン設定は `GET /api/v1/plugins/{id}/config` で host API と同じ
schema / validation を使います（秘密は名前だけ）。System もプラグインプロファイルと承認モードを同じように適用します。
JSON の取り込み / 書き出しは詳細用に折りたたみます。既に動いているコアへは
`ENE_API_URL` / `ENE_API_TOKEN` で接続します。

コア寿命の既定は `desktop.core_lifetime = app`（stage 終了時に子コアも停止）
です。常駐させたい場合は `detached` にします。

チャットは **未設定** から始まります。詳細 → Conversation で、インストール済み
カタログ（`GET /settings` → `effective.providers`）から名前付きプロバイダを選び、
続けてモデルを選びます。Home の **チャットの準備ができています。** は、その束縛に
モデルがあり、かつ `effective.providers[].needs_key` が真ならボールトに API
キーがある（`effective.ai_chat_key_set`）ときだけ出ます。HTTP 401 などの
プロバイダ失敗は設定 / ステータスのエラーであり、アシスタントの返信ではありません。
適用ボタンはスクロールとフィルタ付きのモデル一覧の上にあるので、一覧取得で
保存が画面外へ押し出されません。観測のプライバシー（タイトルモード、OCR ヒント、
いまの送信範囲）も同じ Conversation タブにあり、`mind.proactive.world_state` を
PATCH します。エンジンや GGUF は Connections のプロバイダ資産です。
TTS/STT は `ai.tasks.tts` / `ai.tasks.stt` です。

VAD/ASR/TTS はコアが持ちます。Stage は `GET /sessions/{id}/listen/stream`
でマイク PCM を `pcm_s16le` のバイナリフレームとして中継し（チャンクごとの
JSON POST はしない）、`audio.chunk` を再生します。マイク取得中に listen
ソケットが切れたときは sender を捨て、短い backoff のあと再接続します。
送信 `Closed` は新しい stream を開き、`Full` のときだけそのフレームを捨てます。ローカル TTS 再生中は
マイク RMS 閾値を 2 倍（`BARGE_IN_ENERGY_FACTOR`）に上げ、スピーカ漏れで
割り込みが誤発火しないようにします。大きいユーザー発話はコア VAD へ届きます。
割り込みの判定はコア（`voice.state` と `abort: true` の `audio.chunk`）が持ちます。
Stage はその abort チャンクで再生シンクを止め、viseme をリセットします。正は
クライアントの RMS ではありません。Stage は既定で speaker / notify の排他を
取得します。

音声デバイス中継・承認ポップアップ・トレイ・OS 通知（`notify.hint`）は
stage 側の仕事で、ポリシーとライブバスはコアが所有します。

## 2体のコンパニオン

コア起動はソウルを2つ seed します。Stage は同梱 Alicia VRM
（`assets/characters/Alicia/AliciaSolid.vrm`）を別パッケージ ID で2回入れ、
オーバーレイに2体を描きます。ソウルごとにセッションは分かれ、履歴は漏れません。
A/D はもう一方のソウルへチャットを付け替え、メッシュはアンロードしません。

### 自動と手動

CI と Cloud Agent はソフトウェア Vulkan（lavapipe）です:
`DISPLAY=:1 WGPU_BACKEND=vulkan`。自動で見るのは次です。

- `ene-vrm` が同梱 Alicia VRM をパースし、wgpu アダプタがあれば GPU ロードする
- HTTP: 2ソウルのセッション隔離、Alicia インポートで `avatar_path` が付く
- オーバーレイ配置が2スロットを離す。`ene-stage` は minimal GLB fixture を書く

手動: `ene-stage` を起動し、オーバーレイに VRM が2体いることと、A/D で
それぞれと話せることを確認します。その GUI 手順は CI には含まれません。
