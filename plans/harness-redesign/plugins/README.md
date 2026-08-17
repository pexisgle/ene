# plugins/ — プラグイン実行系

アウトプロセスのプラグインを動かすための3文書。

ハーネス機能ツール(記憶想起・委譲など)はホスト内で動くので、この層を
通らない([../tools/registry.md §0](../tools/registry.md#0-2軸の分類d-10))。
ビルトインツールは**サードパーティと同じ API**に載る。別扱いにすると
実装が二重になるうえ、「`fs` と同等のツールを後から誰でも作れる」という
保証が失われる。

| 文書 | 責務 | 主な P 対応 |
|---|---|---|
| [ipc.md](ipc.md) | 副プロトコル分割・輸送・ハンドシェイク・メッセージ・バルク転送・キャンセル伝播 | P-1001, P-1004, P-1009 |
| [manifest-and-profile.md](manifest-and-profile.md) | manifest・能力宣言・プロファイル/バンドル/パッチ・コミュニティプラグイン形式 | P-1002, P-909, P-1008, P-616 |
| [broker.md](broker.md) | 資源仲介 RPC・FD 委譲・資格情報注入・サイドカー | P-902, P-1006 |

## この層に効く主な決定

- **IPC は書き直す**(D-22)。現行 v8 の単一巨大 enum を core / tool /
  provider / capability の独立バージョンに割り、境界を型付けし、
  音声などのバルクをフレーム外へ出す。理由は
  [ipc.md §0](ipc.md#0-現行v8を書き直す理由d-22)。
- **配布はコミュニティ形成を前提にする**(D-26)。ただし v1.0 は
  ハッシュ検証まで。署名とマーケットは将来で、そこへ大規模改修なしに
  到達できる形だけを今固める。
- **MCP の接続は v1.0 ではプロファイル行への手書き**(D-23)。
  設定画面の導線(P-616)は後継。

## 読み順

1. `ipc.md`(線)
2. `manifest-and-profile.md`(誰が乗るか)
3. `broker.md`(資源の仲介)

## 他フォルダとの接点

- ツールとして登録される面 → [../tools/registry.md](../tools/registry.md)
- MCP に委ねる領域 → [../tools/capabilities.md](../tools/capabilities.md)
- 承認判断 → [../security/approval.md](../security/approval.md)
- サンドボックス強制 → [../security/sandbox.md](../security/sandbox.md)
- プロセス監督 → [../platform/process-model.md](../platform/process-model.md)
