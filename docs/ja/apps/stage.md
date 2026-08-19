# Stage ユーザーガイド

`ene-stage` が製品 GUI です。必要なら `ene-core` を起動し、`ene-vrm` で
キャラクターオーバーレイを描画し、会話は **表層** に、設定・記憶・キャラ・
ジョブ・内部ログは **別窓の詳細画面** に出します。

```sh
cargo build -p ene-daemon -p ene-stage
cargo run -p ene-stage
```

自動のローカルコア起動にはビルドが必要です。Windows ネイティブ開発では
安定版 MSVC Rust、Visual Studio C++ Build Tools、Windows SDK を入れたうえで
PowerShell から同じコマンドを使います。

| ウィンドウ | 深さ | 内容 |
|---|---|---|
| キャラクターオーバーレイ + チャット | `surface` | コンパニオンと発話。内面 / thinking / ツール引数は出さない |
| 詳細（F1 / トレイ） | `detail` | 設定・記憶・キャラ・ジョブ/プラグイン、セッションログ（内面含む）、thinking、ツール、PAD |
| スポットライト（Alt+Space） | ローカル | 詳細タブを開く・マイク切替・終了などのクイック操作 |

Stage は WebView を使いません。UI は egui、VRM は wgpu です。デーモンとは
`ene-api` のみで話し（`client_id = stage`）、`ene-daemon` や
`ene-companion` などデーモン側クレートはリンクしません。

ローカルの `desktop.*`（テーマ、言語、マイク、キャプション、ビート同期、
コア寿命、オーバーレイ配置）とコア側セクション（AI、プラグインなど）は
共有の `settings.json` に保存されます。デバッグビルドはリポジトリの
`assets/` をデータディレクトリにし、リリースビルドは OS のデータ
ディレクトリを使いリポジトリの `assets/` は読みません。API キーはボールトに
置きます。既に動いているコアへは `ENE_API_URL` / `ENE_API_TOKEN` で接続します。

コア寿命の既定は `desktop.core_lifetime = app`（stage 終了時に子コアも停止）
です。常駐させたい場合は `detached` にします。

チャットは **未設定** から始まります。詳細の **Settings** タブで、導入済み
プロバイダプラグイン（`seam.llm`。ローカル GGUF なら `provider.gguf`）を
束縛してください。GGUF やサイドカーエンジンの導入は **Plugins** タブの
プロバイダ資産 UI です。TTS/STT は `seam.tts` / `seam.stt` を宣言する
プラグインから選びます。

音声デバイス中継・承認ポップアップ・トレイ・OS 通知は stage 側の仕事で、
ポリシーとライブバスはデーモンが所有します。
