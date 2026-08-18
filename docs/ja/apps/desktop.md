# Stage ユーザーガイド

`ene-stage` はネイティブクライアントです。必要なら `ene-core` を起動し、
`ene-vrm` でコンパニオンを描き、**詳細画面は別窓**です。

```sh
cargo run -p ene-stage
```

| 窓 | 深さ | 内容 |
|---|---|---|
| 主画面 (stage) | `surface` | コンパニオンと発話。内面 / thinking / ツール引数は出さない |
| 詳細 | `detail` | セッションログ（内面を含む）、thinking、ツール、PAD、タスク |

WebView は使いません。UI は egui、VRM は wgpu です。

会話プロバイダプラグインが無い間、応答は Echo のみです。音声デバイスの中継と
承認ポップアップは stage 側の仕事で、ポリシーとライブバスはデーモンが持ちます。
