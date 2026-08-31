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
| キャラクターオーバーレイ（wgpu） | `surface` | VRM、VRMA、スプリング、視線、ビセーム。Space で枠。キャラのシルエット上で左ドラッグすると移動でき、位置は体ごと（soul 単位）に保存され、再起動後も復元します。背景のクリックは下のウィンドウへ抜けます。クリック透過はシステム → オーバーレイのクリック透過（既定オン）。オンでもカーソルが体の上ならドラッグできます（Windows/X11 はシルエット部分だけ入力を通す）。Wayland はクリック透過中に入力イベント自体が届かないため、まずオフにしてからドラッグします（Space で枠表示でも可）。Esc で終了。位置は `desktop.character_positions` に保存。Detail → Companion で表示する VRM ボディと順番を選び、`desktop.displayed_soul_ids` に保存します。Stage クライアントが同時に表示できるのは最大2体です。A/D は表示一覧とは別にチャット対象のソウルを切り替え、W/S でアクティブな体のモーションを変えます。F3 でスプリングボーンのコライダーをワイヤーフレーム表示する。入力欄にフォーカスがなければチャット/詳細からも同じショートカットが届く。 |
| チャット | `surface` | Prompt / Steer / Follow-up（ホバーで意味）、新しいチャット、承認（許可 / 常に許可 / 拒否）、ask-user（`question.asked` でフォームを開き、`POST /jobs/{id}/answer` や `question.resolved` で閉じる）、マイク PCM 中継、詳細ボタン（トレイと同じ）。新しいチャットは現在のレーンを終え、コアでの新規作成が成功した後にだけ Stage の購読先と保留状態を新しいセッションへ切り替えます。旧会話はログに残ります。ステータスは独立した行で折り返し、狭い窓でも設定エラーが読める。 |
| キャプション | `surface` | 発話の字幕。音声タブの Caption position（`top` / `bottom` / `left` / `right`）で位置を決め、Pin caption でドラッグを止めます。プロバイダ / HTTP エラーはチャットのステータス行（折り返し）に留め、字幕には出さない。ターン終了でオーバーレイは閉じる。長い発話はオーバーレイ内で折り返す。 |
| スポットライト（Alt+Space） | ローカル | 検索できるコマンドパレット: 入力で絞り込み、↑/↓ で移動、Enter で実行、Esc で閉じる（項目のクリックでも実行）。詳細タブ・マイク・チャット・終了を含みます。Alt+Space の登録に失敗した場合は音声タブに警告と強調された「スポットライトを開く」ボタンが出ます。 |
| 詳細（トレイまたはチャット → 詳細） | `detail` | 設定 IA、内面 / thinking / ツール / PAD のログ。検索はセクションを絞り、タブやホームのショートカット、スポットライトを押すとフィルタを消すので、今のタブに固定されません。ログが空のときは空状態と次の操作を出す。 |

アバターの直接操作は、詳細 → システムで設定します。シルエット上で動かさずに押して離すとクリック、短い間隔の2回ならダブルクリック、一定時間押し続けると長押しになります。移動がドラッグ閾値を超えた場合は従来どおり位置移動です。認識した soul だけに短い拡大・表情フィードバックを返し、背景クリックは従来どおり透過します。タッチも同じ判定を使い、ペンはプラットフォームがマウスまたはタッチへ変換した入力に従います。コンパニオンへのイベント送信と会話対象の切り替えは明示的にオンにした場合だけで、イベント送信にはレート制限があります。Wayland では透過中にコンポジターが入力を届けない場合があるため、タッチやドラッグの前にクリック透過をオフにしてください。

Stage は WebView を使いません。オーバーレイは wgpu 上に Slint UI を GPU
合成し、操作窓（チャット / 詳細 / キャプション / スポットライト）は winit 上の
Slint です。コアとは `ene-api` のみで話し（`client_id = stage`）、`ene-core` や
`ene-companion`、`ene-card` はリンクしません。

オーバーレイのイベントループは `ControlFlow::WaitUntil`（約 16ms）と、
VRM の動き・Slint dirty・リサイズ・コライダーデバッグがあるときだけの
`request_redraw` です。ビセームも視線も dirty UI もない静止ポーズは回し続けません。
操作窓が開いている間は chrome も描画します。idle CPU / フレーム時間の
performance gate は実 GPU のみです。Cloud Agent の lavapipe 数値は
ソフトウェア参考で、その gate を落とす理由にはしません。

Wayland のクリック透過は interaction geometry から作る粗い
`wl_surface` input region です。X11 は WM が受け付けるときだけ粗い SHAPE
Bounding/Input を使い、失敗時は窓全体のヒットテストと cursor poll に戻します。
Windows は Passive を `set_cursor_hittest(false)` に写し、既存の DX12/DComp
経路は変えません。Wayland で確認するのは Weston 13 で、他コンポジタは
未保証です。X11 のピクセル単位領域は目標にしません。

キャラクターは `.enechar` パッケージです。`GET /characters` はインストール在庫、
対話相手はソウル（`GET /souls` / `GET /stage` の occupants）です。`body_ref` は
ボディ UUID です。Stage は初回設定で同梱 Alicia VRM を
`char.alicia@1.0.0` としてインポートします。Companion 画面には同梱の2体目を
`char.alicia-b@1.0.0` として追加する操作があり、どちらも HTTP 経由で soul 化します。
CCv3 / PNG / CHARX は変換入力だけで、
CCv3 エディタはありません。Companion の書き出しと Work のセッション書き出しは、
ドキュメントまたはダウンロードで、拡張子付きの名前（`.enechar` / `.json`）を
提案する保存ダイアログを開きます。

ローカルの `desktop.*`（テーマ、言語、マイク、キャプション、ビート同期、画質、
コア寿命、`displayed_soul_ids` による表示体/順番、`character_positions` による体ごとの
オーバーレイ配置）はクライアント側です。
テーマ（`light` / `dark` /
`system`）は wgpu のクリア色と Slint の文字色の両方に効くので、light でも
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

TTS 発話には自動表情が乗ります。発話ごとの最初の `audio.chunk` には、
コンパニオンの現在ムードが `expression` ラベルとして、対応する
`soul_id` とともに付きます。stage は該当アバターへその表情を適用し、
4 秒保持したあと最後の 0.3 秒でフェードアウトさせ、abort チャンクでは
解除します。モデルが出す明示的な `body.expression` コマンドは、この経路を
置き換えず今までどおり優先されます。

音声デバイス中継・承認ポップアップ・トレイ・OS 通知（`notify.hint`）は
stage 側の仕事で、ポリシーとライブバスはコアが所有します。

## コンパニオン表示の管理

空の profile では、同梱 Alicia アバターを1体だけ表示して始めます。
2体目は Detail → Companion の「コンパニオンを追加」をユーザーが選んだときだけ
ステージに追加されます。同じ画面で各在場の名前、サムネイルまたはテキスト専用表示、
チャット対象、soul、body、セッションの対応を確認できます。

表示選択はクライアント側のオーバーレイ設定で、パッケージのインストールや
チャット対象とは別です。「表示に追加 / 表示」はボディを読み込み、「一時的に隠す」
は Stage の再起動まで隠します。「表示から外す」は永続する表示一覧から外しますが、
パッケージ・ソウル・セッションは残ります。上下ボタンで表示順を変更できます。
オーバーレイから外す操作はアンインストールではありません。

Stage クライアントの同時表示上限は2体です。上限に達したときは、別の体を隠すか
表示から外す必要があることを Companion 画面に表示します。テキスト専用の在場や
VRM の読み込み失敗は、空の枠として黙って扱わず表示不可の理由を示します。
ソウルごとにセッションは分かれ、履歴は漏れません。A/D は表示状態を変えずに
チャット対象だけを切り替えます。

### 自動と手動

CI と Cloud Agent はソフトウェア Vulkan（lavapipe）です:
`DISPLAY=:1 WGPU_BACKEND=vulkan`。自動で見るのは次です。

- `ene-vrm` が同梱 Alicia VRM をパースし、wgpu アダプタがあれば GPU ロードする
- HTTP: ソウルのセッション隔離、Alicia インポートで `avatar_path` が付く
- オーバーレイ配置が2スロットを離す。`ene-stage` は minimal GLB fixture を書く

手動: `ene-stage` を起動し、オーバーレイに VRM が2体いることと、A/D で
それぞれと話せることを確認します。詳細 → システムでアバターの直接反応をオンにし、
片方を動かさずにクリック・長押ししてからドラッグします。押した体だけが反応し、
背景クリックが透過することを確認します。任意のコンパニオン送信はチャットプロバイダを
設定したあとにだけオンにします。その GUI 手順は CI には含まれません。

手動: `ene-stage` を起動し、まず VRM が1体であることを確認します。Detail →
Companion で同梱コンパニオンの追加操作を使って2体目を追加し、1体→2体への変更、一時的に隠す→表示、
表示から外す、2体表示時の上下による順序変更、再起動後の永続化を確認します。
A/D は表示状態とは独立してチャット対象だけを変えることも確認します。この GUI 手順は
CI には含まれません。
