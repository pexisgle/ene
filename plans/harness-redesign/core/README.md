# core/ — ハーネスカーネル

エージェントハーネスの中核。11文書が3つの層を成す。
ここは**インフラ**(両層が共有するカーネル)であり、コンパニオンの
**裏層ハーネス**(作業エージェント)そのものではない。2層の位置づけと
専用実装の切り方は
[../product/vision.md](../product/vision.md#51-コアデーモン内の2層)。
表層実装と裏層実装の登録表は [agent-loop.md](agent-loop.md) §2.1 と
[context-assembly.md](context-assembly.md) §2.1。

**語彙・振る舞いの層**(何が記録され、どう回るか):

| 文書 | 責務 | 主な P 対応 |
|---|---|---|
| [session-log.md](session-log.md) | イベント語彙・SQLite 永続化・投影・fork・不変条件 | P-501, P-502, P-108..P-110 |
| [context-assembly.md](context-assembly.md) | Context Source・epoch・途中システムメッセージ・compaction(+prune)・spill | P-505..P-507, P-614 |
| [agent-loop.md](agent-loop.md) | turn/step 状態機械・レーン・割り込み・guard・plan/ask-user | P-503, P-504, P-509..P-512 |
| [delegation.md](delegation.md) | 表層↔裏層の非同期委譲・層間メッセージ・報告ターン | P-519, P-521, P-508, P-522 |
| [visibility.md](visibility.md) | 出力秘匿モデル(チャネル分類・thinking・UI 投影規則) | P-520, P-613 |

**耐久性の層**(クラッシュしても壊れず、再開できるか):

| 文書 | 責務 | 主な P 対応 |
|---|---|---|
| [storage-model.md](storage-model.md) | 3ストア(entries/registers/usage)・トランザクション・レジスタ名前空間・使用量台帳・スキーマ進化 | P-501, P-516, P-518 |
| [operations.md](operations.md) | 操作状態機械(program counter)・control・遷移表・終端トランザクション | P-503, P-504 |
| [durability.md](durability.md) | effect sandwich・クラッシュ位置・回復ポリシー・アボート・close | P-515, P-510 |
| [invariants.md](invariants.md) | 不変条件カタログ・競合カタログ・テスト階層・受入条件 | P-502 の検証 |

**駆動と診断面**(外からどう動かし、どう診るか):

| 文書 | 責務 | 主な P 対応 |
|---|---|---|
| [lane-api.md](lane-api.md) | レーンコマンド面(prompt/steer/abort/resume…)・結果とエラー・クラッシュ後の照合 | P-101, P-503, P-515 |
| [observability.md](observability.md) | ローカルスパン計測・プライバシー規則・診断ファイル | P-517 |

## 読み順

1. `session-log.md`(土台: ログが先、他はすべて投影)
2. `context-assembly.md`(モデルに何を見せるか)
3. `agent-loop.md`(どう回すか)
4. `delegation.md`(実作業はどこで回るか)
5. `visibility.md`(何がユーザーに見えるか)
6. `storage-model.md`(何をどう永続化するか)
7. `operations.md`(ループの耐久状態)
8. `durability.md`(壊れても戻れるか)
9. `invariants.md`(検証)
10. `lane-api.md`(外からの動かし方)
11. `observability.md`(診方)

語彙層の5文書が「正常系」、耐久性層の4文書が「異常系も含めた正しさ」、
駆動/診断面の2文書が「境界」を定義する。実装者はこれらすべてを
読んで初めてループを正しく実装できる。

## このアプリの委譲と秘匿の立場

- ユーザー入力は**表層 soul**だけが受ける。実作業は**裏層ハーネス**への
  非同期委譲に移す。対話レーンは会話のために空ける(delegation.md)。
  同期ツール結果を待つサブエージェントは持たない。
- 表層の公開ツール面は、層間メッセージとコンパニオン局所能力に限る。
  副作用のある作業ツールは裏層にだけ公開する(P-522)。
- LLM の生出力(thinking・ツール引数・生出し・子セッション)は
  **ユーザーに見せないのが既定**。見えるのは発話・内面・要約・成果物
  (visibility.md)。
- job は委譲の public モード、秘匿サブエージェントは internal モード。
  機構は1つ、UX は2つ。

## 他フォルダとの接点

- 2層ランタイム(表層 soul / 裏層ハーネス) → [../product/vision.md](../product/vision.md#51-コアデーモン内の2層)
- ツール実行パイプライン → [../tools/registry.md](../tools/registry.md)
- job のユーザー向け定義 → [../tasks/jobs-and-schedules.md](../tasks/jobs-and-schedules.md)
- プラグイン輸送 → [../plugins/ipc.md](../plugins/ipc.md)
- 割り込みの音声側 → [../body/voice.md](../body/voice.md)
- 承認(plane・internal 委譲の匿名化) → [../security/approval.md](../security/approval.md)
- 内面イベントの形式詳細 → [../companion/inner-channel.md](../companion/inner-channel.md)
- 使用量・診断の公開 API → [../platform/server-api.md](../platform/server-api.md)
- レーンコマンド面の HTTP 射影 → [../platform/server-api.md](../platform/server-api.md)
- 診断 CLI(`ene debug`) → [../platform/clients.md](../platform/clients.md)
