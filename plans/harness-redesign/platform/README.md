# platform/ — プラットフォーム

ホストとクライアントの器。3文書。

| 文書 | 責務 | 主な P 対応 |
|---|---|---|
| [process-model.md](process-model.md) | コアデーモン・起動シーケンス・アクター配置・プラグイン監督・設定システム・バックアップ・性能予算 | P-701, P-708, P-709, P-1005 |
| [server-api.md](server-api.md) | HTTP API・WS イベント・音声ストリーミング・SDK 生成・外部公開 | P-702, P-703 |
| [clients.md](clients.md) | desktop(stage)/CLI/Web の責務分担・表層UIと詳細画面・複数クライアントの排他資源調停 | P-704..P-707, P-712 |

## この層に効く主な決定

- **desktop はネイティブのみ**(D-21)。egui + wgpu で、WebView は使わない。
  常駐アプリの起動時間・メモリ・配布サイズが価値に直結するため。
- **詳細画面は desktop では別ウィンドウ**(D-11)。主画面は「そこに居る」場所で、
  内部情報の表示面ではない。Web も同じ深さの詳細画面を持つが、
  閲覧とデバッグに限り、変更は desktop のみ(D-31)。
- **起動時に前回の中断を検出する**(D-5)。片付けまでがコアの仕事で、
  未消化の inbox はログから見せ、報告は次のターンでコンパニオンが話す。
  実行は再開しない。

## 読み順

1. `process-model.md`(ホストの中で何が動くか)
2. `server-api.md`(外に何を公開するか)
3. `clients.md`(どう使わせるか)

## 他フォルダとの接点

- ツールとして登録される面 → [../tools/registry.md](../tools/registry.md)
  (登録はファイバーの巻き戻し可能な effect)
- プラグイン輸送 → [../plugins/ipc.md](../plugins/ipc.md)
- ホスト文脈の寿命 → [../plugins/composition.md](../plugins/composition.md)
- 表示の深さ → [../core/visibility.md](../core/visibility.md)
- 音声パイプラインの本体 → [../body/voice.md](../body/voice.md)
- 承認の配信先 → [../security/approval.md](../security/approval.md)
