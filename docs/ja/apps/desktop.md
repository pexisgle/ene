# Desktop ユーザーガイド

`ene-desktop` が製品 GUI です。必要なら `ene-core` を起動し、`ene-vrm` で
キャラクターオーバーレイを描き、チャットは **表層**、詳細は **別窓**です。

```sh
cargo run -p ene-desktop
```

| 窓 | 深さ | 内容 |
|---|---|---|
| キャラクター + チャット | `surface` | コンパニオンと発話。内面 / thinking / ツール引数は出さない |
| 詳細 (F4 / トレイ) | `detail` | セッションログ（内面を含む）、thinking、ツール、PAD、タスク |
| 設定 | ローカル + API | `desktop.*` はデスクトッププロセス。他セクションは `/api/v1/settings` へ PATCH |

WebView は使いません。UI は egui、VRM は wgpu です。デーモンとは `ene-api`
だけ（`client_id = desktop`）で話します。`ene-daemon` / `ene-companion` や
旧 runtime / mind / store クレートはリンクしません。

ローカルの `desktop.*`（グラフィックス、テーマ、言語、マイク、キャプション、
beat sync、コア寿命）はデスクトップが保存します。デーモン設定はデータ
ディレクトリの `settings.json` です。起動済みコアへは `ENE_API_URL` /
`ENE_API_TOKEN` で接続します。

`ene-stage` は同一 API の任意のデバッグクライアントとして残しています。

既定のチャットは Echo（`ai.tasks.chat.plugin = echo`）です。ライブモデルは
AI ページでプロバイダプラグインを結びます。音声デバイスの中継と
承認ポップアップはデスクトップ側の仕事で、ポリシーとライブバスはデーモンが持ちます。
