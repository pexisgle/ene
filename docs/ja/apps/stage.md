# Stage ユーザーガイド

`ene-stage` は新 harness コア向けの製品 GUI です。必要なら `ene-core` を起動し、
`ene-vrm` で **透過 wgpu オーバーレイ** にコンパニオンを描き、会話は **表層**
WebSocket、Home / Companion / Conversation / Voice / Memory / Work /
Connections / System とセッションログは **別窓の詳細** に出します。

```sh
cargo build -p ene-core -p ene-stage
cargo run -p ene-stage
```

自動のローカルコア起動にはビルドが必要です。Windows ネイティブ開発では
安定版 MSVC Rust、Visual Studio C++ Build Tools、Windows SDK を入れたうえで
PowerShell から同じコマンドを使います。

| ウィンドウ | 深さ | 内容 |
|---|---|---|
| キャラクターオーバーレイ（wgpu） | `surface` | VRM、VRMA、スプリング、視線、ビセーム。Space で枠。クリック透過はシステム → オーバーレイのクリック透過（既定オン）。Esc で終了。A/D で体、W/S でモーション。入力欄にフォーカスがなければチャット/詳細からも同じショートカットが届く。 |
| チャット（F2） | `surface` | Prompt / Steer / Follow-up（ホバーで意味）、承認（許可 / 常に許可 / 拒否）、ask-user、マイク PCM 中継、詳細ボタン（トレイと同じ） |
| キャプション | `surface` | ストリームされた発話 |
| スポットライト（Alt+Space） | ローカル | 詳細セクションへジャンプ、マイク、終了。OS が Alt+Space を掴んでいるときは音声 → スポットライトを開く |
| 詳細（トレイまたはチャット → 詳細。F1 はコンパニオン、F4 はログ） | `detail` | 設定 IA、内面 / thinking / ツール / PAD のログ |

Stage は WebView を使いません。オーバーレイは wgpu、操作窓は winit 上の egui
です。コアとは `ene-api` のみで話し（`client_id = stage`）、`ene-core` や
`ene-companion`、`ene-card` はリンクしません。

キャラクターは `.enechar` パッケージです。`GET /characters` はインストール在庫、
対話相手はソウル（`GET /souls` / `GET /stage` の occupants）です。`body_ref` は
ボディ UUID です。Stage は同梱 Alicia VRM を `char.alicia@1.0.0` として
インポートし、HTTP 経由で soul 化します。CCv3 / PNG / CHARX は変換入力だけで、
CCv3 エディタはありません。

ローカルの `desktop.*`（テーマ、言語、マイク、キャプション、ビート同期、画質、
コア寿命、オーバーレイ配置）はクライアント側です。コアの PATCH キーは
`core` / `harness` / `approval` / `theme` / `ai` / `mind` / `plugins` です。
プラグインの有効化は `plugins.profile`（`desktop` / `minimal` / `headless`）で、
`plugins.list` の個別マップはありません。API キーはボールトに置きます。
既に動いているコアへは `ENE_API_URL` / `ENE_API_TOKEN` で接続します。

コア寿命の既定は `desktop.core_lifetime = app`（stage 終了時に子コアも停止）
です。常駐させたい場合は `detached` にします。

チャットは **未設定** から始まります。詳細 → Conversation で `ai.tasks.chat`
を束縛してください。エンジンや GGUF は Connections のプロバイダ資産です。
TTS/STT は `ai.tasks.tts` / `ai.tasks.stt` です。

VAD/ASR/TTS はコアが持ちます。Stage は `POST /sessions/{id}/listen` でマイク
PCM を中継し、`audio.chunk` を再生します。割り込みの正はコアの `voice.state`
であり、クライアントの RMS ではありません。Stage は既定で speaker / notify
の排他を取得します。

音声デバイス中継・承認ポップアップ・トレイ・OS 通知（`notify.hint`）は
stage 側の仕事で、ポリシーとライブバスはコアが所有します。
