# plugins/ — プラグイン実行系

全アウトプロセスのプラグインを動かすための3文書。

| 文書 | 責務 | 主な P 対応 |
|---|---|---|
| [ipc.md](ipc.md) | 輸送・フレーム・ネゴシエーション・メッセージ種別・ストリーミング・キャンセル伝播 | P-1001, P-1004 |
| [manifest-and-profile.md](manifest-and-profile.md) | manifest・能力宣言・プロファイル/バンドル/パッチ・コミュニティプラグイン形式 | P-1002, P-909, P-1008 |
| [broker.md](broker.md) | 資源仲介 RPC・FD 委譲・資格情報注入・サイドカー | P-902, P-1006 |

## 読み順

1. `ipc.md`(線)
2. `manifest-and-profile.md`(誰が乗るか)
3. `broker.md`(資源の仲介)

## 他フォルダとの接点

- ツールとして登録される面 → [../tools/registry.md](../tools/registry.md)
- 承認判断 → [../security/approval.md](../security/approval.md)
- サンドボックス強制 → [../security/sandbox.md](../security/sandbox.md)
- プロセス監督 → [../platform/process-model.md](../platform/process-model.md)

