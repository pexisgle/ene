# Desktop ユーザーガイド

`ene-desktop` は凍結した再設計前の GUI で、PR #794 が復元したものです。
機能追加はしません。製品 GUI は `ene-stage` です。伸ばすのはそちらです。
stage が、製品として要る desktop の能力を代替できたと判断したらこの
クレートは削除します。それまでは必要なら `ene-core` を起動し、`ene-vrm` で
キャラクターオーバーレイを描き、チャットは **表層**、詳細は **別窓**です。

```sh
cargo build -p ene-daemon -p ene-desktop
cargo run -p ene-desktop
```

最初のビルドは、デスクトップがローカルの `ene-core` を自動起動するために
必要です。ネイティブ Windows でも stable MSVC Rust ツールチェーン、Visual
Studio C++ Build Tools、Windows SDK を入れた PowerShell から同じコマンドを使います。

| 窓 | 深さ | 内容 |
|---|---|---|
| キャラクター + チャット | `surface` | コンパニオンと発話。内面 / thinking / ツール引数は出さない |
| 詳細 (F4 / トレイ) | `detail` | セッションログ（内面を含む）、thinking、ツール、PAD、タスク |
| 設定 | ローカル + API | 適用すると JSON 層（API キー以外）を `settings.json` に書き、コア節は PATCH |

WebView は使いません。UI は egui、VRM は wgpu です。デーモンとは `ene-api`
だけ（`client_id = desktop`）で話します。`ene-daemon` / `ene-companion` や
旧 runtime / mind / store クレートはリンクしません。

ローカルの `desktop.*`（グラフィックス、テーマ、言語、マイク、キャプション、
beat sync、コア寿命）と、適用した他セクション（AI、mind、plugins）は共有の
`settings.json` に保存します。デバッグビルドではリポジトリの `assets/` が
データディレクトリです。リリースは OS のデータディレクトリだけを使い、
リポジトリの `assets/` は読みません。API キーは vault のままです。コアは
同じファイルを読み、PATCH します。起動済みコアへは `ENE_API_URL` /
`ENE_API_TOKEN` で接続します。

`ene-stage` が同一 API の製品 GUI です。こちらへ機能追加はしません。
stage が、製品として要る能力を代替できたと判断したら desktop は削除します。
判断表は [製品境界](../concepts/product-boundaries.md) です。

チャットは **未設定** のまま起動します。表層から AI ページを開きます。
選択肢はハードコードではなく、ホストが公開した **インストール済みプロバイダ
プラグイン** です。会話モデルは `seam.llm` を宣言したプラグインのコンボです。
ローカル GGUF は `provider.gguf`（このパソコン）です。

埋め込みは別の任意ピッカーです。`seam.embed` のプラグインか未設定。
ローカル GGUF 埋め込みは会話とは別の `llama-server` サイドカーです。
OpenAI 互換と Anthropic のモデルコンボは `POST /api/v1/providers/models`
（プロバイダ IPC `list_models`）から更新します。一覧が空か失敗したときは
用意してある一覧に戻します。ローカル GGUF の重みは汎用のプロバイダーアセット UI
（`POST /api/v1/providers/assets/*`）からインストールでき、カスタムパスも任意で
使えます。

分類・能動発話は **その他（上級者）** にあります。会話モデルと同じプラグイン
選びです。未指定なら会話モデルの値を継承します。TTS/STT は `seam.tts` /
`seam.stt` を宣言したプラグインだけが出ます。

音声デバイスの中継と承認ポップアップはデスクトップ側の仕事で、ポリシーと
ライブバスはデーモンが持ちます。
