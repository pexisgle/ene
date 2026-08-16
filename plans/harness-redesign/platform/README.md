# platform/ — プラットフォーム

ホストとクライアントの器。3文書。

| 文書 | 責務 | 主な P 対応 |
|---|---|---|
| [process-model.md](process-model.md) | コアデーモン・起動シーケンス・アクター配置・プラグイン監督・設定システム・バックアップ・性能予算 | P-701, P-708, P-709, P-1005 |
| [server-api.md](server-api.md) | HTTP API・WS イベント・音声ストリーミング・SDK 生成・外部公開 | P-702, P-703 |
| [clients.md](clients.md) | desktop(stage)/CLI/Web の責務分担・複数クライアントの排他資源調停 | P-704..P-707 |

## 読み順

1. `process-model.md`(ホストの中で何が動くか)
2. `server-api.md`(外に何を公開するか)
3. `clients.md`(どう使わせるか)

## 他フォルダとの接点

- プラグインの輸送・監督対象 → [../plugins/ipc.md](../plugins/ipc.md)
- 音声パイプラインの本体 → [../body/voice.md](../body/voice.md)
- 承認の配信先 → [../security/approval.md](../security/approval.md)

