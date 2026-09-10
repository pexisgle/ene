# Concurrency / Race Control / Stale-result Acceptance の具体設計 — Step 13 Concrete Design

本書は Step 13 の Concurrency artifact である。[対応関係・識別](correspondence-identity.md)（CI）の identity / revision / generation / correlation / boundary token の意味と、[Persistence / Recovery](persistence-recovery.md)（PR）の owner 別 durable state・D1/D2/D3/R/T/E 分類・atomicity / ordering・recovery semantics・compare 対象・durable-before-visible を前提とし、変更しない。上位設計との優先順位と矛盾時の扱いは [設計文書 README](../README.md#正本と優先順位) に従う。本書内の DR は [Dependency Rules](../architecture/dependency-rules.md) を指す。

実装コードはまだ変更しない。本書は mechanism 選択まで固定し、具体 SQL・crate 分割・IPC 形状・retry/timeout 値は固定しない。

## 1. 対象と非対象

### 1.1 今回具体化するもの

- 何が同時に起こり得るか、何と何が競合するか、競合時にどちらを受け入れてよいか（第3節）。
- どの比較を不可分に行う必要があるか、どこで serialization が必要でどこで不要か（第4・5節）。
- stale / delayed 結果の検出・隔離の具体方式（第5・6節）。
- domain invariant に適した mechanism の選択（第6節）。一つの mechanism の全体強制はしない。
- Task / Action / Permission-cost / Presence / Deletion / Restore / Filesystem+DB / Cancellation の競合制御（第7–12節）。
- lock の ownership・granularity・ordering・await 中の保持可否（第15節）。
- 次の interface 設計で必要になる concurrency property の引渡し（第16節）。
- Rust 実装方針の具体化範囲（第17節）。特定 library の導入は必要性が明確な場合だけ固定する。
- race walkthrough（第18節）。

### 1.2 今回決めないもの

- DB 製品固有の完全な `CREATE TABLE`、index、migration、vacuum。
- crate / module 分割、process / thread 配置、IPC / wire format の byte 仕様。
- retry / timeout / polling 間隔の値、scheduling algorithm、embedding / scoring、prompt 組立 algorithm。
- 各 Subsystem の完全な API 一覧。ただし「落としてはならない concurrency property」は第16節で固定する。
- 暗号方式、archive 形式、署名方式。

### 1.3 用語

- **premise（前提）**: 長時間の非同期処理を開始した時点で読んだ current state の写し。boundary token として commit 時に再比較する材料。authority ではない。
- **commit（受入確定）**: 非同期結果を durable state・現在 Task・提示・実行へ反映するための短い原子区間。長時間処理そのものを含めない。
- **serialization domain**: 同じ順序で逐次化しなければならない state 変更の単位。排他そのものが目的ではなく、compare-and-commit の不可分性を成立させる範囲。
- **compare-before-commit**: premise を読み、lock / transaction を保持せず非同期処理を行い、commit 直前に current と比較し、成立する用途だけ accept する形。CI 第6節の述語の concurrency 側の実現である。
- **stale**: premise が現在の owner / lifecycle の revision / generation・許可・保留・消去・復元条件と対応しなくなったこと。到着順・時刻・TTL だけでは判定しない。
- **delayed result の帰属**: 遅延到着物を元の試行・区間・世代へ記録すること。現在への自動採用と区別する。
- **reservation**: 費用・資源の並列消費で「まだ cap 以下」の同時判断による使い切りを防ぐための、commit 前の上限引き当て。費用計算法そのものではない。
- **publication guard**: filesystem 上で durable 化・rename 済みだが DB pointer commit が完了していない file を、周期的 orphan cleanup の削除候補から除外するための短い排他・登録。実装は lock・in-progress registry 等から選べるが、cleanup の削除判定と不可分に照合できることを要求する。

## 2. 原則（固定 premise の再掲＋ concurrency 側の帰結）

1. **arrival order だけで currentness を判断しない。** 到着順は受入根拠にならない（CI §6.1）。
2. **wall-clock / TTL だけで stale 判定しない。** 時刻は説明・表示・Schedule 計算のためであり、現在性は revision / generation・許可・保留・消去・復元条件との対応で判定する（CI §4.2）。
3. **global revision counter を導入しない。** 全 state 共通の単調 counter・snapshot id・transaction id を前提にしない（CI §4.3, PR §4.1）。
4. **unknown outcome を別状態へ推定しない。** Cancel 受付・通信成功・表示・保存成功・再接続・復元・移動で `Unknown` を未実行・成功・失敗へ書き換えない。新 evidence が事実 owner に確認されたときに限り更新する（CI §6.3）。
5. **retry は新しい Attempt。** 重複し得る再送を「同じ試行の継続」として除外しない。`prior_attempt` 対応を付けて新 `ActionAttemptId` とする（CI §6.3, PR §4 Group E）。
6. **delayed result は元の活動へ帰属。** 記録（元へ残す）と semantic 更新・次実行・提示（現在の受入）を分ける（CI §6.3）。
7. **reconnect / restart / restore が replay authority にならない。** 旧要求の実利用許可・旧実行をそのまま再開しない（PR §6.1）。
8. **Permission 判断記録は live authority ではない。** 保存 Allow・委任時 copy・事前判定・復元 Rule・文脈内許可文・cache 判定を現在許可として再利用しない。開始前に現在条件と再照合する（CI §3.2, PR §4 Group F）。
9. **resolved Provider route は authority ではない。** 解決済み割当経路を独立した利用可能 assignment の正本にしない（PR §4 Group I）。
10. **Client claim は presence authority ではない。** presence の authority は Host 管理の個体別帰属記録のみである（CI §3.5）。
11. **DB transaction を長時間保持しない。** external I/O・長時間 inference 中に DB transaction / mutex を保持しない（第15節）。
12. **単純化の禁止。** 「全部 actor」「全部 mutex」「全部 DB transaction」「一つの global coordinator」「universal event ordering」「Lamport / vector clock」「distributed consensus / lease」は必要性がなければ導入しない。Owner 管理 Host を canonical authority とする現在の topology を活用する。

## 3. 何が同時に起こり得るか（concurrency sources と競合対）

Ene は単一 Host 上の非同期 system であり、並行の源泉は Host 内部の task・全域 operation・外部由来の遅延である。分散合意は不要であり、必要なのは Host canonical への commit 順序と stale 隔離である。

### 3.1 並行の源泉

| 源泉 | 例 | 性質 |
|---|---|---|
| Owner 操作の並行 | 追加指示（steering）、Cancel、承認、Rule 変更、同意変更、停止・削除・復元操作 | 新 revision / 新 lifecycle を生む。進行中の前提を陳腐化させる側 |
| Task / delegation の並行 | 複数 Task、同一 Task への並列委任、Schedule 回の到来、委任 Agent の並列実行 | 同一 Task revision への複数 writer になり得る。完了・結果到着が交差する |
| 推論・Action の並行 | 複数 Provider request、fallback・再送、Tool / MCP 実行、Computer Use、長時間 inference | 外部作用は rollback 不能。timeout / response loss で outcome が `Unknown` に残る |
| Client 活動の並行 | move、disconnect、reconnect、Host restart restoration、Stop、simultaneous summon、stale Client message | 同一 Companion への二重 active 競合。旧 round・旧承認・旧一時 state の再流入 |
| 全域 operation の並行 | Targeted Deletion、Backup、Restore、retention / Reset、保持整理、cache/index rebuild、Learning 形成 | 区間・世代を生む。新規形成・再保存・旧 live 混入と競合する |
| 遅延・再送・復旧 | delayed inference / Tool 結果、遅延 usage 報告、Provider session 残存、Client 一時 copy の再送、crash 後の orphan | 到着順が最後でも現在ではない。元の活動へ帰属させる対象 |

### 3.2 競合対と受入方針（概要。詳細は第7–10節）

| 競合 | 受け入れてよい側 | 受け入れない側 |
|---|---|---|
| 会話推論結果 vs 新しい Owner 入力・steering | 新しい目的・revision を持つ側。旧結果は元 Task / 元 round へ記録し historical evidence として残せる | 古い前提の結果を新目的の達成・次 Action・提示へ自動採用すること |
| Task Agent 結果 vs steering / Cancel | steering 後の現在 revision・目的と対応する結果のみ。旧結果は元 revision への記録に留める | 旧目的の結果を新目的の達成に自動採用すること。Cancel 後の後続自動開始 |
| 並列 Action attempts | 前提照合・予約を満たした各試行の開始自体は並列可。確定度更新は per-attempt CAS で分離 | 不明試行の「同じ試行の継続」扱い。旧承認・解決済み経路だけでの開始 |
| Action timeout vs 遅延成功 | 両方を元の attempt へ事実として残す。確定度は新 evidence で `Unknown→Confirmed` の原子更新。現在 Task への採用は別途現在照合 | timeout による `Unknown` の成功・失敗・未実行への書き換え。遅延成功の無条件の現在採用 |
| 並列 Provider requests vs cost cap | 予約枠内のもの。予約＋commit の原子照合で上限を守る | 同一残額を独立に使い切る同時開始。処理中・不明のゼロ化 |
| summon A→B vs disconnect | generation CAS に勝った単一の帰属のみ。旧 in-flight は安全な区切りまで | 二重 active。新旧いずれでの新規開始（移行中）。旧作用の別 Client 自動継続 |
| reconnect stale Client vs restored presence | 現在 generation・現接続・許可・排他性が確認できた側のみ | 古い一時 state・旧承認・解決済み経路だけでの成立。確認不能の現在推定 |
| Learning 形成 vs Targeted Deletion | 消去条件・scope・保存禁止と対応した形成のみ。区間内再到着・再生成は消去対象 | 削除前情報を使う実行中処理による再保存。旧根拠だけの自動再形成。完了前の完了表示 |
| deletion 完了 vs 遅延結果 | provenance が新区間・新提供と確認できたもの。旧 provenance の遅延物は破棄または元記録に留め再保存しない | 完了後の旧 provenance 遅延物の再保存。cache / session 由来の意図しない再出現 |
| Restore switch vs 旧 live 結果 | 新 generation 前提と対応し、復元後保留・一括有効化・現在条件を満たしたもの | 旧 generation 前提の旧 live 結果の新正本への混入。復元成立だけでの自動処理開始 |
| crash at filesystem / DB boundary | durable-before-visible / durable-before-mark-success の順序を満たした側。公開中でない orphan だけを cleanup 対象にする | DB 指標のみ・file のみの中間を正本・成功として可視化すること。rename 済みだが pointer commit 前の公開中 file を cleanup が削除すること |

## 4. Unit of serialization（serialization domain）

### 4.1 方針

- serialization は「同じ compare を不可分にする」ためにだけ設ける。domain invariant が独立なら並行させる。
- 長時間処理（inference、外部作用、staging、派生再構築）は serialization domain の外に置く。commit の瞬間だけ短く逐次化する。
- cross-owner の更新を一つの巨大 transaction・一つの actor へまとめない。必要なのは短い cross-owner atomic read（同一 SQLite transaction の共有）であり、意味変更権の統合ではない（PR §7）。

### 4.2 Serialization domain 表

| ID | domain（逐次化単位） | 含める state 変更 | 並行してよいもの | 備考 |
|---|---|---|---|---|
| SD-Task | per `TaskId` | steering（新 revision＋新 context の forward）、委任作成の受付、完了・失敗・Cancel の確定、結果採用の判定 | 異なる `TaskId` 間の一切。同一 Task の読取専用の参照・進捗表示・由来説明 | PR AU2/AU3/AU4 の compare を不可分にする範囲。Task 達成（作業）と作用確定度（実行・拡張）と許可確定（権限・制約）を同一更新にしない |
| SD-Attempt | per `ActionAttemptId` | 確定度 `Unknown→Confirmed` の更新、停止要求・停止結果・hold の付記 | 異なる attempt 間の実行・報告。同一 attempt の読取 | append-only 試行＋ per-row CAS。Task 達成は別 transaction で読み取って更新（durable-before-adopt） |
| SD-Presence | per `CompanionId` の帰属 | `presence_attribution` の `旧→移行中→新` 遷移、generation bump、`active_client` 切替、hint・復旧先の更新（要時） | 異なる Companion の帰属。同一 Companion の表示・audio・観測等の非帰属処理 | PR AU7。live 到達性確認は DB 外であり atomic に含めない。移行中は新旧いずれも Client 依存の新規開始をしない |
| SD-Cap | per `CapId`（＋ provider / 全体 scope） | `usage_fact_*` の予約 insert＋ cap 照合、確定時の予約→実績 commit、release | 異なる cap の消費。同一 cap の読取・集計表示。予約後の inference 実行そのもの | PR AU6。同一 SQLite transaction 内で判定。処理中・不明をゼロにしない |
| SD-CharApply | per `CompanionId` の適用 pointer | `companion_applied_current` 更新＋履歴 append（`expected_character_revision` 照合付き） | 異なる Companion の適用。新 `character_revision` の insert 自体 | PR AU12。内容の正本（Character）と適用関係の正本（個体調整）を同一更新にしない |
| SD-Undelivered | per `undelivered_id`（＋登録時の親原子） | 報告状況 `Pending→Summarized→Presented/Unknown` の更新。会話由来は History append と同一原子、Task 由来は Task durable 後の別 transaction 原子登録 | 異なる undelivered 間の報告。同一物の読取・要約生成（commit 前） | PR AU1a/AU1b/AU8。送信だけで `Presented` にしない（durable-after-confirmed） |
| SD-Deletion | per `DeletionOperationId`＋sweep | operation＋`erasure_condition` の先行 durable、参加者集約・残存検証、検索 token の除去 / 復元不能化、全域完了の原子確定。token と完了 marker を同じ durable commit にできない間は `finalizing` を維持 | 参加者の局所処理・検証そのもの。対象外の通常活動。異なる operation 間 | PR AU9。durable-before-enforce。局所返却で hold を解除しない。token が復元可能な状態を全域完了にしない。全 domain の lock ではない |
| SD-Restore | singleton `restore_generation_state` の switch 瞬間のみ | staging 検証後の `restore_generation` bump＋正本 pointer switch の原子確定 | staging 作業、live mutation（switch 瞬間を除く）、異なる時点の backup 作成、derived 再構築 | PR AU11。switch 前は復元前正常が正本、switch 後は復元内容が正本。第三の混合を作らない。switch 全期間の read 停止はしない（第12節） |
| SD-RuleConsent | per `RuleId` / `AssignmentId` / `DevicePermission` / `SandboxException` | 本文・解釈・scope・同意 revision の更新、Undo 対応の付記 | 評価（読取＋判断）そのもの。異なる rule / assignment 間 | 評価時は保存 Allow を再利用しない。更新と評価を同一更新にしない |
| SD-CompanionLife | per `CompanionId` の lifecycle | Running / Stopped / Deleted（tombstone 最小）の遷移、新規禁止 hold の先行 | 異なる Companion の lifecycle。各 owner の局所削除・検証作業 | PR AU13。単一 global transaction にしない。各 owner の局所 durable の集約 |

明示的に serialization **しない**もの：inference 実行、外部作用の継続、Provider session、embedding / index rebuild、routing 文脈生成、要約生成、表示・集計、Observer Capture・候補検知、backup file copy（marking 前）、restore staging 検証。これらは premise 付きで開始し、commit 時に compare する（第5節）。

## 5. Compare-before-commit（前提と current の比較）

### 5.1 一般形

CI 第6節の述語を concurrency の commit 境界へ落としたものである。DB transaction を長時間保持しない。

```
1. 前提を読む（短い read。lock / tx を保持しないか、保持しても ms オーダーの read のみ）
   - (identity, expected revision / generation)
   - 依拠 Permission evaluation・rule / consent revision・device・cap・失効・停止・保留
   - Client 依存なら claimed generation＋現接続・可用性の観測前提
   - 消去条件 (operation, sweep, valid_interval)・復元 generation 前提
   - Task なら目的・steering 前提・委任 scope・Workspace 有効性、Action なら実対象・操作種別
2. 非同期処理を行う（lock / tx なし）
   - inference、外部作用、staging、派生生成、検証など。Cancel・steering・失効・消去・復元が途中で起こり得る
3. commit 直前に current と比較する（短い原子区間。PR §7 の AU に対応）
   - 同一 SQLite Immediate transaction 内で current を読み直し、premise と比較する
   - await・外部 I/O を transaction 内で行わない。比較と durable 更新だけを行う
4. 成立する用途だけ accept する
   - 一致：durable 更新＋可視化（durable-before-visible / durable-after-confirmed を守る）
   - 不一致：hold・deny・不足・再評価へ戻す。古い判定・解決済み経路だけで開始しない
   - 遅延物：元の試行・区間・世代へ記録し、現在の目的への自動採用にしない
```

「最新の値を読んだ」「cache hit」「到着順で最後」はいずれも単独では受入根拠にならない。revision と generation は混ぜない。boundary token の欠落は「制約なし」ではなく不受理の理由にする（CI §4.5）。

### 5.2 原子に比較しなければならないもの（PR §10 の concurrency 落とし込み）

| 利用・受入 | 原子に比較するもの | 不一致時の扱い |
|---|---|---|
| Task への委任・steering・Action 開始 | `(task_id, expected_task_revision)` × 現在 revision × 委任 scope・Workspace 有効性・steering 前提 | 開始しない。再評価へ戻す |
| Action の新規開始 | attempt の `(Task revision 前提, 委任 scope, 実対象・操作種別, 依拠 Permission evaluation)` × 現在の同型の組 × Client 依存なら `presence_generation` と現接続・可用性 × 消去・復元保留 | 開始しない。Owner 確認待ちは待機にする |
| 確定度の更新 | `(attempt_id, expected_certainty=Unknown)` × 新 evidence の事実 owner 確認 × 現在 Task revision（採用可否の分離用） | 新 evidence なしに書き換えない。現在 Task への採用は別受入とする |
| Client 依存活動の開始・継続 | `claimed_generation` × `presence_attribution.generation` × 現接続・可用性 × 現在許可・停止・保留 | 確認不能なら継続しない |
| 消去区間の受入・生成・再保存 | 到着・生成情報の `(source 関係, 取得・生成時点, sweep 以前・区間内の別)` × `(operation, sweep, valid_interval)` × 保持者の局所検証。完了後は旧 provenance linkage × 完了 operation の scope | 区間内再到着・再生成は消去対象。実行中処理による再保存をしない。完了後の旧 provenance 遅延物も再保存しない |
| 復元後の利用 | 利用の `(restore generation 前提, assignment/consent revision, Credential 照合, 依拠 Rule revision)` × 現在の `(restore_generation, 現 store, 現制約, 復元後保留)` | 旧 live・旧同意・旧 assignment だけで自動利用・自動処理を開始しない |
| 権限・Rule 解釈の採用 | 過去 Allow・復元 Rule・context 内許可文・cache 判定 × 現在の `(rule revision, 同意, device, cap, 失効・停止・帰属・消去・復元保留)` | 制御を変更しない |
| 費用・資源の継続判断 | 消費の `(用途・送信先対応, 報告/推定/不明/処理中の別)` × 現在 cap・資源・不明の扱い。並列は予約＋commit の原子照合 | 処理中・遅延・不明をゼロにしない。並列で同一残額を使い切れる扱いにしない |
| Character 適用 | `(character_id, expected_character_revision)` × 現在適用関係 × `OwnerSelectionRef` | 未確認部品を更新済みにしない |
| 全域完了の確定 | 各 participant の局所完了・検証・未完了・失敗 × 機械的残存検証 × 区間内再到着の取込み × 本文非再保存 × 検索 token の除去 / 復元不能化 | 未確認・検証失敗・pending / unreachable・token 残存を成功に読み替えない。token が復元可能なら `finalizing` の未完了として扱う |

### 5.3 一律 pipeline として強制しないこと

上記は「必要な比較」の表であり、実行順序の強制ではない。各 domain は次のいずれかで成立させてよい。

- 楽観 compare-and-swap（Task・presence・適用・確定度・cap 予約の既定）。
- owner-local mailbox での順序付け（同一 Task・同一 Companion 帰属・同一 operation の完了集約）。
- 短い DB transaction 内の atomic read＋insert（cap 予約、委任受付、attempt 受付）。

いずれでも「前提 read → 長時間処理 → 短い commit compare」の分離と、transaction 内での await 禁止を守ること。

## 6. Concurrency model（domain 別の mechanism 選択）

単一 Host・単一 SQLite file（PR §4・§11）・Tokio async を前提とし、domain invariant に適した方式を選択する。一つの mechanism を全体へ強制しない。

| domain | 選択する mechanism | 選ぶ理由・捨てる選択肢 |
|---|---|---|
| Task / steering / 委任受付 | owner-local serialization（per-Task mailbox または per-Task Mutex による短い順序付け）＋楽観 CAS（`expected_task_revision`）＋短い `Immediate` transaction（PR AU2/AU3/AU4） | Task 現在 pointer の forward 競合を順序付けるため。全体 actor・global lock は不要。異なる Task は完全並列 |
| Action 受付・確定度 | append-only insert＋ per-attempt CAS（`expected_certainty`）＋短い transaction（PR AU5）。実行そのものは非同期並列 | 試行は履歴であり現在値ではないため global 順序は不要。Agent 申告を evidence にしない制約は mechanism ではなく owner 確認で守る |
| Permission 評価 | 評価は並列（snapshot read）。commit（予約・開始）は短い transaction 内の現在条件との再照合。保存 Allow の再利用禁止は評価 logic で守る | 評価の並列性を lock で潰さない。判定 cache を live 化しない |
| 費用・cap | 予約（reservation）＋ commit / release の三状態＋短い transaction 内の原子照合（第9節）。予約後の inference は lock なし並列 | 楽観 read のみでは同一残額の使い切りを防げないため悲観的予約が必要。ただし予約保持中の inference を block しない |
| Presence 帰属 | per-Companion serialization（帰属 actor / 短い Mutex）＋ `expected_generation + expected_state` の CAS（PR AU7）＋ `旧→移行中→新` の durable 遷移。live 到達性は DB 外確認 | 同一 Companion の二重 active を防ぐには逐次化が必須。異なる Companion は並列。Client 側 lock だけに依存しない |
| Character 適用 | per-Companion CAS（`expected_character_revision + OwnerSelectionRef`）＋短い transaction（PR AU12） | 適用 pointer の単一性だけ守ればよく、revision insert 自体は並列可 |
| Undelivered | 登録は親原子（AU1a/AU1b）、確定は per-row CAS（提示確認後のみ）。要約生成は並列 | 送信≠報告の区別は commit 順序（durable-after-confirmed）で守る |
| Targeted Deletion | `operation + erasure_condition` の durable-before-enforce＋各受入箇所での lock-free 照合＋ per-operation の完了集約 serialization（第11節）。検索 token の最終消去が完了 marker と不可分でなければ `finalizing` を durable に維持する。全 domain の長大 lock はしない | 新規禁止と in-flight best-effort の分離のため。完了集約だけ逐次化すれば足りる |
| Backup / Restore | staging 並列＋ switch 瞬間のみ singleton 短期排他（第12節）。live read の全面停止はしない | 単一正本 switch の atomicity だけ守ればよく、全期間の global lock は過剰 |
| Filesystem＋DB | temp 書込→ fsync → publication guard / cleanup 除外の確立→ atomic rename → DB pointer commit → guard 解除→ orphan cleanup（第13節）。DB transaction のみで atomic にしない | DB と filesystem は別 durability domain のため順序＋ cleanup との同期で守る。周期 cleanup が publish 中 file を削除しないことが必要。OS API は過度に固定しない |
| Cancellation | signal（`Notify` / cancellation token 系）＋協調停止＋ commit 禁止の三層分離（第14節の取消分離）。future drop を停止完了とみなさない | 外部作用は rollback 不能のため、取消伝達と停止完了と結果隔離を分ける必要がある |

採用しないもの：distributed consensus、global scheduler lock、global transaction、universal actor、universal event ordering、Lamport / vector clock、distributed lease protocol。いずれも単一 Host canonical の topology では必要性がない。

## 7. Task / delegation / steering

### 7.1 前提と serialization

- Task 作成は `task + task_revision + 初期 context + workspace_assoc` の原子 durable（PR AU2）とし、commit 前は委任・実行から不可視にする（durable-before-visible）。
- steering は新 revision＋新 context の原子 forward（PR AU4）とし、旧 revision を残す。旧目的の結果を新目的に自動採用しない。
- 委任作成は `expected_task_revision` の atomic compare を満たして作成する（PR AU3）。
- 同一 `TaskId` の steering・委任受付・完了確定・結果採用は SD-Task で逐次化する。異なる `TaskId` は並列でよい。

### 7.2 競合制御

- **instruction / steering revision 競合。** 二つの steering が同時到着したら SD-Task の順序で直列化し、先勝ちの revision を現在にし、後着は新現在に対する再 steering として評価する。上書き・merge をしない。発話 record（History）と Task 反映内容と未反映・待機を区別し、反映不可は理由と選択肢を示す。
- **delegation relation 競合。** 委任は作成時点の `(task_id, expected_task_revision, scope 写し)` を保持する（`DelegationRef`）。steering 後に旧 revision 前提の委任作成が届いたら不受理・再評価へ戻す。実行中の委任が steering と交差したら、委任元は旧委任の停止・縮小を best-effort で行い、旧委任の結果を新目的に自動採用しない。
- **cancellation 競合。** Cancel は SD-Task で現在 Task に記録し、委任・attempt へ best-effort の停止を伝える。Cancel 記録と停止完了を分け、停止不能・既作用・不明・未保存を残して報告する。Cancel 後の遅延 Agent 結果は元 revision への記録に留め、現在 Task の達成にしない。
- **delayed Agent result。** 帰属は `attempt → task revision → 現在 Task` の順に辿る（CI §6.3）。到着時に `(attempt の Task revision 前提, 目的)` × 現在の `(Task revision, 目的・steering 前提)` を比較する。一致しなければ現在採用せず、元 Action / Task へ事実を残す。historical evidence としての保存（元 revision への記録）は許し、意味の異なる現在への採用はしない。
- **Task completion 競合。** 完了確定は SD-Task の短い transaction で「現在 revision が期待通りか＋未完了・判断待ち・不明の扱いが確定しているか」を確認して行う。二つの完了報告が交差したら先勝ちを現在にし、後着は元記録として残し二重完了にしない。
- **retry / restart。** retry・再委任は新 revision 前提・新 attempt で開始する。Host restart 後の Task は保存済み進捗・既知作用・不明・未完了を示し明示再開待ちにし、Agent を自動再起動しない。新委任は新 Task revision 前提で開始する。

### 7.3 pseudo-flow（steering vs 結果到着）

```
-- steering (SD-Task, 短い tx)
BEGIN IMMEDIATE;
  cur = SELECT revision FROM task WHERE task_id = ?;
  IF cur != expected_revision THEN ROLLBACK; RETURN StalePremise;
  INSERT task_revision(task_id, cur+1, 新目的・指示・委任前提);
  UPDATE task SET revision = cur+1 WHERE task_id = ?;
  INSERT task_context_entry(...新 revision 対応...);
COMMIT;

-- delayed Agent result 到着 (lock なしで帰属解決 → SD-Task の短い受入 tx)
premise = result.task_ref; -- (task_id, revision 前提)
BEGIN IMMEDIATE;
  cur = SELECT revision, purpose FROM task WHERE task_id = ?;
  IF premise.revision != cur.revision OR premise.purpose != cur.purpose
    THEN -- 現在不採用。元へ記録する別 tx で保存し RETURN AdoptedToOriginalOnly;
    ELSE -- 現在採用の可否をさらに許可・消去・復元・cap と照合し、成立する用途だけ accept;
COMMIT;
```

## 8. Action（authorization と outcome tracking の分離）

外部作用そのものは DB transaction で rollback できないことを前提にする。effect 開始前の atomic compare と、開始後の outcome tracking を分離する。

### 8.1 開始前の atomic compare（短い transaction、await なし）

`action_attempt` insert 時に次を同一 `Immediate` transaction 内で照合して原子にする（PR AU5＋AU3/AU6/AU7 の cross-owner read）。

- `(task_id, expected_task_revision)` × 現在 Task revision・委任有効性・Workspace 有効性・steering 前提。
- 解決後の実対象・操作種別（Read / Create / Edit / Delete / Execute を潰さない）。
- 依拠 Permission evaluation × 現在の `(rule revision, 同意, device, cap, 失効・停止・帰属・消去・復元保留)`。保存 Allow・解決済み経路だけでは開始しない。
- Client 依存なら `presence_generation` × 現接続・可用性。
- cap は予約 insert＋照合を同一 transaction で行う（第9節）。
- 消去条件・復元保留の照合。

不一致なら開始しない。Owner 確認待ちは実行せず待機にする。不一致の理由（stale / hold / deny / 不足・再評価）は呼び出しへ返し、黙って queue・replay しない。

### 8.2 開始後の outcome tracking（並列、per-attempt CAS）

- 段階（受付 / 開始 / 送信 / 把握）・確定度（`ConfirmedSuccess / ConfirmedFailure / Unknown`）・根拠対応・hold を `action_attempt` 行に保持する。
- 確定度更新は `(attempt_id, expected_certainty)` の CAS とし、新 evidence の事実 owner 確認を必須とする。`Unknown` は粘着させ、Cancel 受付・通信成功・表示・保存成功・再接続・復元・移動で書き換えない。
- 外部 `成功` 応答は対象・作用の対応と由来・scope を保った上で evidence として扱う。Agent 自己申告を証拠にしない。
- Task 達成・報告は別 transaction で attempt を読み取って更新する（durable-before-adopt）。実行・拡張が Task 達成を直接更新しないし、作業が確定度を独立更新しない。

### 8.3 競合制御

- **two concurrent attempts。** 各 attempt の開始 compare は短い transaction で直列化されるが、実行そのものは並列でよい。同一 Task・同一実対象への並列開始が明示的な並列委任なら許す。重複 risk がある retry 的な再実行は owner 判断＋ `prior_attempt` 対応付きの新 attempt とし、迂回（別経路・別表現での同等 Action）をしない。
- **timeout / response loss vs late success。** timeout は確定度を変更しない（`Unknown` のまま）。遅延成功は元 attempt へ `Unknown→Confirmed` の CAS で記録し、現在 Task への採用は第7節の受入で別途照合する。Unknown に対する再試行は新 Attempt かつ既存 Owner-decision contract（重複 risk・費用を示した判断、確認済み失敗と不明の別経路）に従う。
- **Cancel / revocation との交差。** 開始前なら開始禁止、実行中なら best-effort 停止、停止不能・既作用・不明を残して報告する。Cancel 受付 ≠ 停止完了。失効を別実行経路・Task Agent で迂回しない。
- **authorization と actual target resolution の分離。** 対象記述の文字列一致だけで対応にしない。解決後の実対象（`RealTargetRef`）を attempt に保持し、解決前提が変われば再評価する。

## 9. Permission / cost / usage（reservation / commit / release）

### 9.1 並列消費の race と対策

複数 request が同時に「まだ cap 以下」と判断して全て開始する race を、同一 transaction 内の予約＋照合で防ぐ（PR AU6）。評価（並列可）と commit（直列化）を分ける。

- 利用事実の原記録は各 owner（推論・作業・実行・拡張・保全・消去）に残し、権限・制約は可否だけを管理する。Task・推論側に独立許可・使用実績の正本を作らない。
- 処理中・未報告・不明をゼロにしない。cap・不明で安全継続不可なら data 保持のまま停止・判断待ちにする。

### 9.2 reservation / commit / release

```
state: Reserved(処理中・上限引き当て) → Committed(確定) / Released(解放)
       + Reported / Estimated / Unknown の区別を別 field に保持する
```

- **reservation（開始時、短い `Immediate` tx）。** `usage_fact_*` に `Reserved` 行を insert し、同一 tx 内で `cap_limit`＋関連 `usage_fact_*`（`Reserved + Committed + Unknown` の合計。`Released` を除く）を読み取って cap 照合する。上限超過・不明で継続不可なら ROLLBACK し、開始しない。予約量は推定上限（upper bound）とし、過小予約による超過をしない。
- **実行（lock なし）。** 予約後の inference・Tool 実行は並列に行う。予約保持を理由に他 request を block しない。失効・停止・保留が発生したら best-effort で停止する。
- **commit（確定時、短い tx）。** 実績（報告値 / 推定 / 不明の別）を `Reserved` 行に原子更新するか、差分を `Committed` として確定し、余剰を `Released` する。遅延 usage 報告は元の `usage_id`・attempt・task・assignment 対応へ帰属させ、現在 cap の再評価材料にする。未報告・処理中・不明をゼロ化・リセットしない（Companion 削除・Agent 終了・移動・cache clear・log 整理でも reset しない）。
- **release（Cancel・失敗・失効時、短い tx）。** 未使用予約を `Released` にし、上限を回復させる。既に生じた外部消費・不明消費は release しない（事実として残す）。
- **孤立予約の recovery。** Host crash / Agent 停止で所有 in-flight を失った `Reserved` は、当該利用 owner の再評価が元 reservation に対して IB K-G `CommitUsageCommand(actual = 不明)` を呼び、SD-Cap の短 transaction で `Reserved` を照合して `Committed / Unknown` へ確定する。再評価の重複は二重計上せず、確定済み報告を不明へ巻き戻さない。引当を release・ゼロ化せず不明として cap 集計に一度だけ含め、後着報告は元利用対応への更新として再評価する。不明・孤立理由は費用管理面へ示す（PR §6.2）。
- **revocation during use。** 失効は新規予約の deny と実行中の best-effort 停止にとどめ、既確定消費の事後的取消にしない。別実行経路・Task Agent による迂回をしない。

費用の具体計算法・Provider billing 仕様は本書の対象外であり、reservation 量の算定式・集計期間・推定方式は Design Freedom とする。固定するのは「同一残額の独立使い切りを許さない」ことと「処理中・不明をゼロにしない」ことである。

### 9.3 concurrent Task Agents との関係

- 並列委任された複数 Agent の消費は同一 `CapId` の予約 domain で合算する。Agent ごとに独立残額・独立 budget を作らない。
- Observer の消費は Observer 専用 assignment の対応で区別するが、同一全体 cap には合算する（CC-06）。Client ごとに重複実行しない制約と併せて守る。

## 10. Presence / Client（`PresenceGeneration` による競合制御）

### 10.1 serialization と compare

- 同一 `CompanionId` の帰属切替は SD-Presence で逐次化し、`expected_generation + expected_state` の CAS による `旧→移行中→新` の durable 遷移とする（PR AU7）。`presence_transition_log` append と要時の `relocation_hint` 更新を含む。
- hint・復旧先の更新と帰属成立を同一視しない。現在接続を古い保存値から再成立させない。
- 異なる Companion の帰属は並列でよい。同一 Companion でも表示・audio・観測等の非帰属処理は帰属 lock の外で並列に行う。

### 10.2 競合制御

- **同一 Companion への二重 active 禁止。** `presence_attribution` の `(companion_id, generation, active_client)` を Host canonical とし、同時に二つの `Present` を成立させない。simultaneous summon は CAS の先勝ちのみ成立させ、後着は不受理・再評価へ戻す。Client 側 lock だけに依存しない。
- **move。** 切替区間は `旧 / 移行中 / 新 / active なし / 停止中 / 復旧待ち` を区別し、移行中は新旧いずれでも Client 依存の新規開始をしない。旧 in-flight は安全な区切りまで継続し、旧作用の別 Client 自動継続をしない。
- **disconnect。** 一時的な到達不能は帰属 durable を即時破棄せず、到達性・排他性が確認不能な間は新規開始をしない。通常切断・process 終了が確定したら、`ene-presence` が利用可能な Host PC Client（IPC §10.1 の Host 確定 SameMachine、live 認証・device 許可・排他性を再照合）へ SD-Presence の CAS で `旧→移行中→新` と遷移し、候補なし・確認不能なら `NoActive` とする（`DisconnectFallback`）。Host 側 Client を自動起動せず、通常切断を `RecoveryWait` にしない。検知から切断確定への timeout 等は Freedom。Client 依存 Action の停止は best-effort とし、停止不能・既知作用・不明を残す。
- **reconnect。** `ClientPresenceClaim.claimed_generation` × 現在 `generation` × 現接続・可用性 × 現在許可・停止・保留を照合する。古い一時 state・旧承認・判定 copy・解決済み経路だけでは成立させない。確認不能を現在と推定しない。旧 round の入力・未提示出力を新 round へ付け替えない。
- 通常切断後は再接続だけで fallback / `NoActive` を元 Client へ戻さない。呼出し・事前指示・通常の自発判断を必要とする。`ReconnectRecovery` は Host restart の `RecoveryWait` と復旧先に対する確認だけに使う。
- **Host restart restoration。** `presence_attribution`＋hint・復旧先（非現在）＋現接続・許可・排他性の live 確認で再構成する。Running presence は現確認ができれば復元前 Client へ自動復元し、できなければ active なしにする。Stopped に移動・復旧しない。復旧は presence のみであり、Task・Action の再開権限にしない。
- **Stop。** Stop は帰属解除として保持し（hint と区別）、Stop 競合では Stop が勝つ。接続回復だけで Resume・再配置しない。
- **summon A→B vs disconnect。** 両方を SD-Presence の順序で直列化する。先に確定した遷移の generation が現在になり、後着は新現在に対する再要求として評価する。二重 presence・旧 in-flight の自動継続・未終了作用の別 Client 再実行のいずれもしない。
- **stale Client message。** round・試行・候補・routing 派生物に添えた `PresenceGeneration` で世代を識別し、旧世代の材料だけで新規利用を開始しない（CI §6.4）。

### 10.3 pseudo-flow（帰属切替）

```
BEGIN IMMEDIATE;
  cur = SELECT state, generation, active_client FROM presence_attribution WHERE companion_id = ?;
  IF cur.generation != expected_generation OR cur.state != expected_state
    THEN ROLLBACK; RETURN StalePremise;
  UPDATE presence_attribution
    SET state='InTransition', generation=cur.generation+1 WHERE companion_id = ?;
  INSERT presence_transition_log(...旧→移行中...);
COMMIT;
-- 移行処理（安全な区切り待ち。DB tx なし、lock なし）
...
BEGIN IMMEDIATE;
  -- 新帰属の現接続・許可・排他性の確認結果を受けて確定
  UPDATE presence_attribution SET state='Present', active_client=?, generation=? WHERE ...;
  INSERT presence_transition_log(...移行中→新...);
COMMIT;
```

## 11. Targeted Deletion との race

`targeted-deletion.md`・PR §8 を前提とし、semantic owner・boundary を変更しない。全 domain を一つの長大な transaction で lock しない。

### 11.1 先行 durable と各箇所の照合

- `deletion_operation + erasure_condition` の durable を参加開始より先行させる（durable-before-enforce）。各参加者の局所完了・検証は durable 化してから coordinator へ返し、返却で hold を解除しない。全参加の集約＋機械的残存検証＋区間内再到着の取込みを満たした後、検索 token を除去または復元不能化し、その成立を確認してから全域完了を原子に確定する（PR AU9）。token の最終消去と完了 marker を同じ durable commit にできない場合、その間は `finalizing` を未完了として durable に保つ。
- 各受入・保存先（会話・Task・Learning・Action・undelivered・cache / index・Client / 拡張一時 copy・返却可能結果の保持者を含む全 holder）は、保存・採用の commit 時に飛行中の `ErasureConditionRef(operation, sweep, valid_interval)` を lock-free に照合する。既知 source の追跡だけでは区間内再到着を扱えないため、`erasure_condition` を durable かつ index 付きで保持する。
- 実行中処理による再保存をしない。削除処理・残存検証・検索 token の最終消去が完了していない場合は完了と表示しない。

### 11.2 競合制御

- **新しい情報形成 vs 削除区間。** 区間内に再到着・再生成した対象情報も同じ消去対象とする。削除途中の到着・生成を新 Experience として救済しない。到着・生成情報の `(source 関係, 取得・生成時点)` × `(operation, sweep, valid_interval)` の照合で判定する。
- **delayed result vs 削除区間。** 遅延結果の受入に消去チェックを加える（用途別受入＋消去条件）。旧 provenance の遅延物は元記録への事実残しに留めるか破棄し、現在への採用・再保存をしない。`lateness != newness`、`latest != current` を守る。
- **Learning update vs 削除。** 形成 commit 時に現在の Memory revision・scope・制約に加えて消去条件を照合する。遅延形成が現在を無条件上書きしない。到着順を根拠の新旧にしない。旧根拠だけの自動再形成をしない。
- **cache / index rebuild vs 削除。** derived の invalidation key に消去条件を含める（PR §5）。hit を理由に制約確認を省かない。古い index から権限・状態を復活させない。
- **Task / inference の結果到着 vs 削除。** Task 由来・推論由来のいずれも現在 Task・現在認識への採用前に消去条件を照合する。削除前情報を利用する実行中処理は best-effort で停止・非対象 scope へ縮小し、停止不能・既外部・不明を残す。
- **verification / completion vs 遅延。** 局所完了 ≠ 全域完了。未確認・到達不能を成功と読まず、局所返却だけで hold を解除しない。機械的残存検証＋区間内再到着の取込み＋検索 token の除去 / 復元不能化を満たして初めて全域完了とする。
- **finalizing 中の crash。** 参加者・残存検証が完了していても検索 token が復元可能なら operation は未完了である。restart 後は通常処理へ token を再利用せず、最終消去と完了確定だけを続行し、token の復元不能化を確認するまで hold を解除しない。
- **完了後の遅延物。** 機械的条件の検索 token は全域完了の durable 確定前に wipe / 復元不能化し、完了記録・Audit へ対象本文を残さない。一方、完了後に届いた旧 provenance（削除区間開始前の attempt・Task revision・source 範囲に対応する）遅延物は、完了 operation の scope との provenance linkage で再保存を抑止する。本文保持なしに対応を維持できること（CA §8.2）。完了後の Owner 新規提供は provenance が新区間であるため新 Experience として区別する。文字列一致だけで新旧を判定しない。

### 11.3 `ErasureConditionRef` / operation identity / participant state の利用

- `ErasureConditionRef(operation, sweep, valid_interval)` を ingestion 時の照合用 index として各受入箇所へ配る。authority ではなく比較材料である。
- `DeletionOperationId + DeletionSweepGeneration` を消去区間の識別とし、消去区間を跨ぐ保持・処理・cache・Client 一時 copy・返却可能結果に添える（CI §6.4）。
- `ParticipantCompletionRef`（参加者ごとの処理・検証・未完了・失敗・未確認範囲）を durable に保持し、全域完了の集約材料にする。`HoldConditionRef` で新規禁止・再保存防止を表現する。`DeletionOperationRef` の完了は保全・消去が全域として確定するが、各 domain の意味変更は各 owner が行う（DR-09）。

## 12. Backup / Restore との race（旧 live 分離と正本 switch）

`backup-restore.md`・PR §9 を前提とする。古い Backup Restore に関する既存 decision を変更しない。

### 12.1 staging・live・switch の分離

- Restore は staging（別 file / 別 group）での照合・検証を先行させ、`restore_generation` の原子 switch で正本を切り替える。switch 前は復元前正常が正本、switch 後は復元内容が正本とし、第三の混合を作らない（PR AU11）。
- switch 前の crash は旧正本＋pending、switch 後の crash は新正本＋保留とする。失敗時は復元前正常を維持し、部分置換を新正本にしない。
- **Host の全 read を全期間停止しない。** live read は switch 瞬間まで旧正本を読み続け、staging read は隔離して行う。switch の瞬間だけ singleton の短い排他（SD-Restore）で pointer を切り替える。以後は新正本＋復元後保留で読む。

### 12.2 競合制御

- **live mutation vs restore staging。** staging 期間の live mutation は旧正本への通常 commit として継続する（新 activity の一律停止を必須にしない）。staging 内容へは混ぜない。
- **old live result vs 新正本。** 切替前に開始した推論・Tool 結果が切替後に届いても、利用の `(restore generation 前提, ...)` × 現在の `(restore_generation, ...)` の照合で抑止し、用途別受入で現在の対象・制限・意味へ照合する。旧 live・旧同意・旧 assignment だけで自動利用・自動処理を開始しない。旧 live 要求・結果・Client copy を復元正本へ混ぜない。Client copy で Host を上書きせず、未送信操作を自動 queue にしない。
- **restore generation 照合。** 復元を跨ぐ参照（Task・Rule・同意・assignment・作用・未伝達・全域未完了）には `RestoreGeneration` を添える（CI §6.4）。旧世代の参照だけで復元後に利用・実行・送信しない。Task の目的変更は `task_revision` の前進であり generation 変化で代替しない。
- **global-operation hold。** staging 開始時に必要な hold（新規 Action・送信・外部作用の保留）を permission + activity owner が適用する。復元成立と実行再有効化は別々に確認する。成立後も Task・Schedule・外部接続の自動処理は保留し、Owner が内容確認後まとめて有効化できる（Deny・同意・cap・認証不足・不明を無視しない）。復元済み assignment / consent だけで自動利用を開始しない。
- **未完了消去との交差。** 未完了 targeted deletion の消去条件・検証未完了・再保存防止は置換で黙って解除しない。復元 operation 状態と競合したら対象再利用を制限する側を優先する。消去区間に重なる Restore 由来の再到着は、各受入・保存先が進行中の消去条件を適用する。
- **Backup 作成との交差。** file durable を `backup_point` 成功 marking より先行させる（durable-before-mark-success）。対象時点・参照・履歴・未完了の対応が揃って初めて成功とし、各部 copy 成功だけを成功にしない。未完了消去・復旧と重なる場合は制約を無視した正常 copy を作らない。

## 13. Filesystem + DB の境界（DB transaction だけでは atomic にならない境界）

PR §4.1・§11 の technology mapping を前提とする。Host durable（D1/D2/D3）は単一 SQLite file、blob 本体は filesystem（例: `internal_copies/`）、portable backup は外部 copy（例: `*.ene-backup`）、秘密値は credential store である。

### 13.1 pattern（過度に OS API を固定しない）

内部保持 copy・中間 file の blob 本体、backup file、restore staging file について次を守る。

```
1. write temp（同 filesystem 上の一時名へ書込）
2. durable write（fsync / 同等の durable 化。具体 API は固定しない）
3. publication guard / in-progress 登録（この publish と orphan cleanup の削除判定を同期し、対象 path を cleanup 候補から除外）
4. atomic rename（同 filesystem 内での原子置換。読取側が中間を見ない）
5. DB pointer commit（短い transaction で参照・由来・用途・削除 marker を durable 化）
6. publication guard 解除（DB pointer の durable を確認した後）
7. orphan cleanup（起動時 scan＋周期的整理。live 参照も active publication も持たない temp・旧 file のみ除去）
```

順序の要点は **file durable → publish 保護 → DB pointer commit → 可視化**（durable-before-visible / durable-before-mark-success）である。DB commit を先行させて「DB 指標はあるが file がない」可視中間を作らない。同時に、周期的 cleanup が「まだ DB 指標がない」ことだけを根拠に、rename 後・pointer commit 前の file を削除してはならない。

publication guard の具体実装は固定しない。process 内の lock / in-progress registry、cleanup 側の exclusion token 等を選べる。ただし cleanup は削除直前に同じ同期境界の下で **(a) live DB / backup / restore 参照がないこと** と **(b) active publication がないこと** を再確認しなければならない。先に「参照なし」と読んだ結果だけを保持して、publish / DB commit と競合した後に unlink してはならない。crash により publication guard が失われ、かつ DB pointer が存在しない file は通常の orphan として cleanup / recovery 対象にできる。

### 13.2 failure の各位置で残ってよいもの

| failure 位置 | 残ってよいもの | 残してはいけないもの（可視化禁止） |
|---|---|---|
| temp 書込前・書込中 | 壊れた temp（cleanup 対象）。旧正本・旧 pointer は無傷 | 新 pointer・成功 marking・完了表示 |
| temp durable 後・rename 前 | 隔離された temp（active publication 中は cleanup しない。crash 後は cleanup 対象）。旧正本は正本のまま | 新内容の正本化・成功表示 |
| rename 後・DB commit 前 | 参照なしの file。正常な publish 中は publication guard により cleanup から保護し、crash 後に guard が失われた場合は orphan として除去または recovery で再連結できる。旧 pointer は旧 file を指したまま | publish 中 file の cleanup、参照なし file の正本扱い・成功表示 |
| DB commit 後・可視化前 | 新旧両 file＋新 pointer（durable 済み）。crash 後は新 pointer を正本として再構成する | cleanup による新 file 削除、旧 pointer への巻戻し・部分正本の混在 |
| backup file durable 後・`backup_point` 成功 marking 前 | 未成功の file（publish 中は保護し、crash 後は再試行・除去の対象）。最後の正常を破壊しない | 各部 copy 成功だけでの成功表示 |
| restore staging 検証前・switch 前 | 隔離 staging（旧正本は無傷）。`restore_operation` は pending | 部分置換の新正本扱い・自動処理開始 |
| switch 後 | 新正本＋保留。旧正本は Audit・rollback 用の隔離 copy としてのみ残せる（live 正本ではない） | 第三の混合・権限先行復活・旧 live 混入 |

- orphan cleanup は live 参照（DB pointer・`backup_point`・`restore_operation` の保管参照）または active publication を持つ file を削除しない。削除判定は publication と同期し、unlink 直前に参照と publication を再確認する。到達不能 Client の物理消去を確認済みにしない。
- startup cleanup では process crash により残った active publication は存在しないため、DB 等の durable 参照と recovery marker を照合した上で orphan を整理できる。周期 cleanup と通常 publish は上記 guard で競合を防ぐ。
- 全データ Reset の途中 crash でも旧処理・一時 copy から復活させない（filesystem marker と DB 削除後の継続を併用する。PR §4 Group J）。
- Credential 秘密値・外部 Workspace 実体・Provider / MCP 側状態は上表の file 扱いに含めない（E であり backup 除外・Restore 維持）。

## 14. Cancellation の分離（drop を停止完了とみなさない）

既存の Cancel 受付 ≠ 停止完了の contract を維持する。少なくとも次を分離する。

| 層 | 意味 | mechanism の目安 |
|---|---|---|
| request cancellation | Owner・管理操作・steering・失効・保留による「止めてほしい」の受付 | actor mailbox への Cancel コマンド、oneshot 応答での受付確認。LLM・Agent 正常終了待ちにしない |
| computation 停止 | Host 内の推論・集計・派生生成等の協調停止 | `Notify` / cancellation token 系による協調 cancel＋ `select!` での中断。 await 中の lock 保持なし |
| external effect 停止 | Tool / MCP / Provider / Computer Use 等の外部作用の best-effort 停止 | 実行・拡張による停止 signal・切断・取消要求。停止不能・既作用・不明を残して報告。別 Client・Host での自動再実行をしない |
| result arrival | 停止後に届く遅延結果の扱い | 元 attempt / 元 Task revision への記録に留め、現在への自動採用・後続自動開始をしない。`Unknown` は維持する |
| state commit 禁止 | 取消後に新規 commit をさせないこと | commit 時の premise 再比較（Task revision・委任有効性・許可・帰属・消去・復元保留）。取消後の commit は `StalePremise`・`HoldActive` として不受理にする |

- Cancel の伝達・新規禁止・状態参照は、外部 process の exit・全 Task の完了を待たない。
- Cancel・失効・steering・移動・削除・復元後の到着物は、元の Action / Task へ記録し、旧承認の解除・旧結果の新目的採用・後続の自動開始をしない。
- Host shutdown / restart は進行中・不明・未伝達・全域未完了を保全し、外部作用が Host と同時に消えると推定しない。

## 15. Locking と deadlock 回避

### 15.1 lock ownership・granularity

- **DB transaction（`Immediate`）。** commit compare＋durable 更新のための ms オーダーの排他。所有は呼出側の短い closure に限り、await・外部 I/O・inference を内部で行わない。複数 owner の atomic read に使うことは mechanism 共有であり ownership 統合ではない。
- **owner-local mutex / mailbox。** SD-Task・SD-Presence・SD-Deletion（完了集約）・SD-Restore（switch 瞬間）の順序付けのため。各 domain の coordinator が所有し、他 domain の mutex と二重保持しない。
- **publication guard。** filesystem publish と orphan cleanup の削除判定の間だけ使う狭い同期境界。domain commit の global lock に流用せず、external I/O・長時間処理を跨いで保持しない。file durable 後から pointer commit 後までの publish critical section と、cleanup の最終 recheck + unlink を相互排他にできればよい。
- **cancellation signal（`Notify` / token）。** 所有は実行単位。lock ではなく協調停止の伝達に使う。
- **filesystem lock（例: `ene-core.lock` の exclusive）。** process 多重起動防止等の粗い境界に限り、domain commit の逐次化に流用しない。
- granularity は第4節の domain 表に従う。global lock・全 domain の長大 lock・単一 global transaction は設けない。

### 15.2 acquisition ordering と await 中の保持可否

- 原則：**await 中（`.await` を跨ぐ間）に DB transaction / domain mutex を保持しない。** premise read と commit compare を分け、長時間処理を外に置く（第5節）。filesystem publication guard は第13節の短い publish / cleanup critical section に限り、非同期の長時間 I/O を跨がせない。
- 複数 domain の compare が一つの commit tx に必要な場合（例: Action 開始時の Task＋cap＋presence＋消去・復元保留）、**async mutex の多重保持ではなく単一の短い DB transaction 内の ordered read** で行う。transaction 内の read 順序を固定する（例: `restore_generation_state` → `deletion_operation / erasure_condition` → `task / delegation` → `presence_attribution` → `cap_limit / usage_fact` → `action_attempt`）。順序外の access・条件分岐による逆順取得をしない。
- mailbox（mpsc receiver 処理）の中で他 domain の mutex・DB transaction を await 付きで待たない。必要な compare は mailbox 外の短い tx へ委譲するか、mailbox 処理自体を短く保つ。
- broadcast は観測・表示のための配信に限り、authority・commit 順序・排他に使わない。`Lagged` は skip し、欠落を成功・現在と読まない。
- 上記により、複数 owner を跨ぐ処理でも global lock ではなく compare / commit 境界で成立させる。循環承認待ち・管理経路の循環依存を作らない（DR-08）。

## 16. Interface handoff（次の interface 設計で要求すべき concurrency property）

具体 API 全部を今回設計しない。落としてはならない情報を固定する。暗黙の「最新を使う」指定を設けず、token 欠落は不受理の理由にする。

| 境界 | 要求すべき concurrency property |
|---|---|
| Task 化・委任・steering・結果統合 | `(task_id, expected_task_revision)`、目的・steering 前提、委任 scope 写し、Workspace 有効性、Cancel 対象。発話 record と Task 反映内容と未反映・待機の区別 |
| Action 要求・試行・停止・結果 | `ActionAttemptRef`（試行・Task revision 前提・委任・Workspace・実対象・操作種別・依拠 Permission evaluation）、`PermissionEvaluationRef`、実対象解決の前提、費用・停止・保留・消去・復元条件の写し、把握された作用・確定度・根拠対応、retry 時の `prior_attempt` 対応 |
| Permission 判断の依頼・回答 | 判断対象（主体・委任・Task・Workspace・目的・実対象・操作・送信先・data・作用・費用 risk）、依拠 Owner 意図・Rule の対応（`(rule_id, rule_revision)`、`assignment_consent` revision、device、cap）、重要変化の有無。判断記録と生きた許可の区別 |
| Provider 実送信（初回・fallback・再送・補助・継続の各々） | 論理選択範囲、解決済み consumer / Capability assignment、実送信先・data・用途・取扱い・費用の同意対応、認証用途・制限・保留・利用量、元要求・範囲の対応、報告 / 推定 / 不明 / 処理中の別、予約 `usage_id` |
| Client 依存活動の開始・継続 | `PresenceAttribution`（個体・状態・active・generation）、`ClientPresenceClaim.claimed_generation`、`ConversationRound`、旧・新・移行中・active なし・停止・復旧待ちの区別、現接続・可用性 |
| Observer routing | `RoutingContextRef`（source・target・目的・制約・選択前提）、起源 Client・取得時点・候補対応、`PresenceGeneration`、消去・失効条件 |
| Learning・Summary・根拠 | `SummaryGroundsRef`、`(learning_id, learning_revision)`、source 範囲・取得時点、scope・制約、訂正と状況変化の別、消去条件 |
| 消去・保持・backup・復元 | `DeletionOperationRef`・`ErasureConditionRef`（目的・範囲・影響・除外・要確認、参加者・未完了・検証・hold、検索 token の最終消去 / `finalizing`）、backup 時点・参照・除外・保護・結果、`RestoreGeneration`・保留・一括有効化対応、Audit 順序・保持 |
| 利用量・費用 | 用途・送信先の対応、報告 / 推定 / 不明 / 処理中の別、cap・資源の現在条件、予約 `usage_id` |
| Character 適用 | `(character_id, expected_character_revision)`、`OwnerSelectionRef`、適用部品群 |

- **acceptance result の表現。** commit 結果は少なくとも `Accepted / StalePremise / HoldActive / Denied / NeedsReevaluation / AdoptedToOriginalOnly（遅延物の元記録化）` を区別できること。これは concurrency compare の結果であり、各 domain の lifecycle 状態（Task 状態・presence 状態・確定度・全域 operation 状態）を潰した共通 `Status` enum ではない。共通 lifecycle 状態 machine を新設しない（CI §2.1）。
- Serialization 境界では ID・revision・generation・correlation を明示 field として serialize し、自由記述の本文中の文字列を照合に使わない（CI §4.6）。

## 17. Concrete Rust implications（実装方針の具体化範囲）

特定 library の導入は必要性が明確な場合だけ固定する。以下は本書の concurrency property と PR の technology mapping から導く**実装候補**であり、旧実装の型名・actor・writer・shutdown 構造を互換性要件として引き継がない。新規実装は要件・設計を満たす最小の機構を選び、旧実装と同型であること自体を採用理由にしない。

### 17.1 `tokio::sync` primitive のカテゴリ

| 用途 | 使うもの | 使わない・避けること |
|---|---|---|
| serialization domain の順序付け | `mpsc`＋ owner-local task / actor、または短い Mutex。per-Task / per-Companion / per-Operation の狭い mailbox 化 | 万能 actor・global coordinator・全 domain 共通 mailbox。mailbox 内での長時間 await・他 domain lock 待ち |
| request / reply | `oneshot` 等の一回応答 primitive | 応答を authority・完了証拠にしないこと（受付と完了を分ける） |
| 観測・表示の配信 | `broadcast` 等の fan-out。`Lagged → skip` を許す用途に限定 | commit 順序・排他・許可・報告完了の根拠にしない |
| 協調 cancel | `Notify + AtomicBool` または必要性が明確なら cancellation token 系。`select!`＋ `timeout` による中断 | future drop を停止完了とみなすこと。Cancel を LLM・Agent 正常終了待ちにすること |
| 短い排他 | `tokio::sync::Mutex` は短い順序付けに限定し、`.await` を跨ぐ保持・外部 I/O 中の保持をしない。同期 DB connection を採る場合は狭い同期 ownership＋`spawn_blocking` 等で async executor から隔離する | `tokio::sync::Mutex` の長時間保持・DB `Connection` の async 共有・transaction の await 跨ぎ保持 |
| hold / cap 変更の通知 | `Notify`・必要なら `watch`。導入は hold 購読の必要性が明確になってから | 待機 polling（要件で禁止）。`watch` 値を authority にしないこと |

`watch`・`CancellationToken`・`JoinSet`・`LocalSet` の導入は本書で固定しない。必要性が明確になったときに、authority 化・lifecycle 統合をしない範囲で選ぶこと。

### 17.2 owner-local task / actor の目安

- per-Task 受付、per-Companion 帰属、per-Operation 完了集約、cap 予約の commit（短い tx）は actor / mailbox または短い Mutex で順序付ける。実装は各 serialization domain の invariant に必要な最小範囲に留め、旧 crate の actor / writer / gate を移植することを要求しない。
- inference 実行・外部作用・staging・派生再構築は owner-local serialization の外の async task と、同期 DB / CPU-bound 処理に必要な `spawn_blocking` 等へ分離し、serialization 側は premise・token・結果受入だけを扱う。
- shutdown は受付停止・協調停止・完了待ち・未完了 durable 化を分離する。`oneshot`＋`JoinHandle` は一つの実装候補にすぎず、旧 shutdown pattern の維持を要求しない。一時 buffer 消失を成功・完了の根拠にしない。

### 17.3 transaction closure と compare-and-swap style repository method の目安

PR の SQLite mapping を実装する際、同期 `rusqlite`＋`WAL`＋`Immediate` transaction を `spawn_blocking` 経由で使う構成は一つの候補であり、async transaction library の導入も含めて最終 mechanism は固定しない。どの実装でも短 transaction・await 非保持・§5 の compare-and-commit を守る。pseudo-code（コンパイル対象ではない）：

```rust
/// 短い Immediate transaction の closure。内部で await しない。
fn with_immediate_tx<T>(conn: &Connection, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let out = f(&tx)?;
    tx.commit()?;
    Ok(out)
}

/// Task steering の CAS 例。旧 revision を残す。
fn cas_task_steer(
    conn: &Connection,
    task: TaskId,
    expected: TaskRevision,
    next_purpose: SteeringPremiseRef,
) -> Result<CasOutcome> {
    with_immediate_tx(conn, |tx| {
        let cur: TaskRevision = query_task_revision(tx, &task)?;
        if cur != expected {
            return Ok(CasOutcome::StalePremise { expected, current: cur });
        }
        insert_task_revision(tx, &task, &cur.next(), &next_purpose)?;
        update_task_current(tx, &task, &cur.next())?;
        insert_task_context_entry(tx, &task, &cur.next())?;
        Ok(CasOutcome::Accepted)
    })
}

/// cap 予約の例。予約 insert＋照合を同一 tx で行う。
fn reserve_usage(
    conn: &Connection,
    cap: CapId,
    upper_bound: UsageAmount,
    attr: UsageAttributionRef,
) -> Result<ReserveOutcome> {
    with_immediate_tx(conn, |tx| {
        let id = insert_usage_reserved(tx, &cap, &upper_bound, &attr)?;
        let total = sum_usage(tx, &cap)?; // Reserved + Committed + Unknown。Released を除く
        let limit = load_cap_limit(tx, &cap)?;
        if total > limit || must_hold_for_unknown(tx, &cap)? {
            mark_released(tx, &id)?;
            return Ok(ReserveOutcome::DeniedOrHold);
        }
        Ok(ReserveOutcome::Reserved(id))
    })
}
```

- `CasOutcome::{Accepted, StalePremise, HoldActive, Denied, ...}` は concurrency compare の結果であり、domain lifecycle の `Status` ではない（第16節）。
- error 型は本書で固定しない。library 化する際は `thiserror` を用い、bare `String` / `Box<dyn Error>` を public error にしない（repo 規約の再掲）。
- 将来 `unsafe` が必要になれば `// SAFETY:` を付すが、本書の表現のために `unsafe` を要求しない。

## 18. Validation（race walkthrough）

固定チェックリストではなく、意味のある競合・障害を選んで walkthrough した。各行の owner・contract は既存のままである。

| race | 歩行と必要な結果 | 本書の成立箇所 |
|---|---|---|
| conversation result vs newer Owner input | 推論開始時の premise（Task revision・目的・steering 前提・History 範囲・Client round・帰属 generation・消去・復元条件）を保持し、結果到着時に SD-Task の短い受入 tx で現在と比較する。新入力・steering が先に commit されていたら現在不採用とし、元 Task / 元 round へ記録する。提示・次 Action は現在照合を満たしたものだけ行う | §5, §7 |
| Task Agent result vs steering / Cancel | 到着結果の `(attempt, Task revision 前提, 目的)` × 現在の `(Task revision, 目的)` を比較する。不一致なら `AdoptedToOriginalOnly` とし、旧目的の達成・後続自動開始・旧承認の解除をしない。Cancel 後は停止不能・既作用・不明を残して報告する | §5, §7, §14 |
| two concurrent Action attempts | 各開始を短い tx の premise 照合＋ cap 予約で直列化し、実行は並列にする。retry 的再実行は `prior_attempt` 付き新 attempt＋ Owner 判断を要求し、「同じ試行の継続」除外・別経路迂回をしない。確定度は per-attempt CAS で分離する | §8 |
| Action timeout vs late success | timeout で確定度を変えない（`Unknown` 維持）。遅延成功は元 attempt へ `Unknown→Confirmed` の CAS で記録し、現在 Task への採用は別途現在照合する。再試行は新 Attempt かつ Owner-decision contract に従う | §8, §14 |
| two Provider requests near cost cap | 両方を `reserve_usage` の同一 tx 照合で判定する。先勝ちの予約で上限に達したら後着は deny・hold し、data 保持のまま停止・判断待ちにする。処理中・不明をゼロにしない。予約後の inference は並列でよい | §9 |
| summon A→B vs disconnect | 両方を SD-Presence の順序で直列化し、`expected_generation + expected_state` の CAS で単一帰属だけ成立させる。移行中は新旧いずれも新規開始しない。旧 in-flight は安全な区切りまで、旧作用の別 Client 自動継続をしない | §10 |
| reconnect stale Client vs restored presence | `claimed_generation` × 現在 generation × 現接続・可用性 × 現在許可・停止・保留を照合する。古い一時 state・旧承認・解決済み経路だけでは成立させず、確認不能を現在と推定しない。旧 round 付け替え・旧承認復活をしない | §10 |
| Learning update vs Targeted Deletion | 形成 commit 時に現在認識・scope・保存条件に加えて飛行中の `ErasureConditionRef` を照合する。区間内再到着・再生成は消去対象とし、実行中処理による再保存・旧根拠だけの自動再形成をしない。完了前に完了表示しない | §11 |
| deletion completion vs delayed result / crash | 全参加集約＋残存検証＋区間内再到着の取込み後に検索 token を wipe / 復元不能化し、その成立確認後に全域完了を durable 確定する。最終消去前 crash は `finalizing` から再開し、完了・hold 解除へ進まない。完了後の旧 provenance 遅延物は provenance linkage で再保存を抑止し、新規提供は新 Experience として区別する | §11 |
| Restore switch vs old live result | staging 検証→ `restore_generation` switch の原子確定とし、switch 前は旧正本・switch 後は新正本＋保留とする。旧 generation 前提の旧 live 結果は新正本へ混入せず、用途別受入で現在照合する。成立後も自動処理は保留し一括有効化待ちとする | §12 |
| periodic cleanup vs file publication | writer を rename 後・DB pointer commit 前で停止し、その間に周期 cleanup を走らせても active publication により file は削除されない。pointer commit 後に guard を解除し、その後の cleanup は新 live 参照を再確認するため file を削除しない。writer が crash して guard と pointer の双方が残らない場合のみ orphan として整理できる | §13, §15 |
| Host crash at filesystem / DB boundary | file durable→ publish 保護→ DB pointer commit→ 可視化の順序を守り、中間 crash は orphan（cleanup / recovery 対象）または旧正本＋pending として再構成する。DB 指標のみ・file のみを成功・正本にしない。全データ Reset の途中 crash でも旧処理・一時 copy から復活させない | §13 |
| steering 連打 vs 委任作成（追加） | SD-Task の順序で revision forward を直列化し、旧 revision 前提の委任作成を不受理・再評価へ戻す。新委任は新 revision 前提で開始する | §7 |
| revocation during use（追加） | 新規予約を deny し、実行中を best-effort で停止する。既確定消費を事後取消にせず、別経路迂回をしない | §9, §14 |
| Character 更新 vs 適用競合（追加） | 新 revision insert と適用を分離し、適用は `expected_character_revision + OwnerSelectionRef` の CAS で確定する。未確認部品を更新済みにしない | §4, §5 |

## 19. 横断レビュー（自己レビュー）

本書完成後に requirements・Step 11・Step 12・CI・PR へ戻して自己レビューした。観点と結果は次のとおりである。

- **固定 premise 維持。** identity / revision / generation / directed correlation / boundary token の意味と分離、owner 別 durable・D1/D2/D3/R/T/E・atomicity / ordering・recovery・compare 対象・durable-before-visible の境界を維持した。arrival order・wall-clock / TTL・global revision counter・unknown 推定・retry 新 Attempt・遅延帰属・replay 禁止・判断記録 / 解決経路 / Client 主張の非 authority 化のいずれも崩していない。
- **単一 mechanism 強制の回避。** 楽観 CAS・owner-local serialization・短い DB transaction・予約・mailbox・cancel signal・filesystem publication guard を必要な domain にだけ選択し、「全部 actor / mutex / DB transaction / global coordinator」を設けていない。global version・universal ordering・consensus・lease を導入していない。
- **serialization の過不足。** 同一 Task・同一 Companion 帰属・同一 attempt 確定度・同一 cap・同一 operation 完了集約・switch 瞬間のみ逐次化し、異なる Task・Companion・attempt・cap・operation 間と長時間処理を並列に残した。filesystem publish と cleanup の race は path publication の狭い境界だけ同期する。全 domain の長大 lock・全 read 停止を避けた。
- **stale / delayed の隔離。** 全競合で「元への記録」と「現在への採用」を分け、旧目的・旧承認・旧 provenance の自動採用・自動再実行・自動復活をしないことを walkthrough で確認した。
- **外部作用の不可逆性。** 開始前 compare と開始後 tracking を分離し、DB rollback で外部作用を取り消せる想定を置いていない。Unknown 粘着・新 evidence 更新・新 Attempt retry を維持した。
- **修正。** レビューで見つけた不足（acceptance result と lifecycle `Status` の混同防止、完了後遅延物の provenance linkage による抑止、Targeted Deletion の検索 token 最終消去を全域完了前の必須条件化、filesystem publish 中 file と周期 cleanup の排他、transaction 内 read 順序、mailbox 内での他 domain 待ち禁止、旧実装 pattern を実装要件にしないこと）を本書へ反映した。Step 11・Step 12・CI・requirements の変更は不要であった。

## 20. 後続設計への引渡しと残す Design Freedom

### 20.1 固定前提として使えるもの

後続の interface / IPC / 実装設計は次を固定契約として利用できる。

- concurrency sources・競合対・受入方針（第3節）。
- serialization domain 表（第4節）。同一 Task・同一 Companion 帰属・同一 attempt・同一 cap・同一 operation・switch 瞬間の逐次化範囲と、並列してよい範囲。
- compare-before-commit の一般形と原子比較表（第5節）。DB transaction 長時間保持の禁止、token 欠落の不受理を含む。
- domain 別 mechanism 選択（第6節）。単一 mechanism 強制・global 系・consensus 系の不採用を含む。
- Task / Action / Permission-cost / Presence / Deletion / Restore の競合制御（第7–12節）。
- filesystem＋DB の順序、publication guard と cleanup の同期、failure 残存条件（第13節）。
- cancellation の五層分離（第14節）。
- lock ownership・granularity・ordering・await 中の保持禁止（第15節）。
- interface handoff の concurrency property と acceptance result の区別（第16節）。
- Rust 方針の具体化範囲（第17節）。primitive カテゴリ・actor 目安・transaction closure / CAS method の形。旧実装 pattern の互換維持は含まない。

### 20.2 意図的に残した Design Freedom

| 設計対象 | 固定済みの architecture property | 残す Design Freedom |
|---|---|---|
| 保存・transaction 実装 | PR §7 の atomicity / ordering / durable-before-visible、本書 §5 の atomic compare、§13 の file→DB 順序と publish / cleanup 排他を守る | 具体 SQL 方言・index・migration・vacuum・transaction library・filesystem layout・blob inline / file の選択・fsync API・publication guard の具体方式・orphan scan 周期 |
| 並行機構の実装 | §4 の serialization 範囲、§6 の mechanism 選択、§15 の ordering・await 保持禁止を守る。確認不能を許可・現在・完了へ変換しない | actor / mailbox の crate 配置・queue 種別・Mutex / RwLock の最終選択・`watch` / token 系の要否・IPC・process 配置・retry / timeout 値・exact progress 表現 |
| 費用・資源 | 同一残額の独立使い切り禁止、処理中・不明のゼロ化禁止を守る | 予約量算定式・集計期間・推定方式・上限値・表示粒度 |
| 派生・ routing・要約 | 第二正本化の禁止、invalidation key（revision / generation / 消去条件）の保持、消去参加を守る | embedding / scoring・cache 実装・routing 生成方式・model・形式・頻度・再構築時機 |
| 全域操作の運用 | 単一正本・非混合・権限先行復活の禁止・成功表示条件・再起動時保全を守る | 停止伝達・切断検知・帰属調停・round 区切り・対象探索・参照整合・照合配置・backup 整合時点の作り方・部分失敗からの復旧 |
| 到達性・時刻・ UI | 経過時間の解釈、missed 非補完、確認不能の不受理を守る | 到達性確認方式・減衰・時刻計算・Capture 時機・提示確認・要約粒度・UI layout |

archive / file format、encryption implementation、serialization、Rust type / trait、crate / module、IPC、locking、retry / timeout、specific library も固定しない。上表の対応関係から統一 Context layer、Policy Engine、Manager、Service、Coordinator の追加を導かない。既存の責務、semantic owner、Host / Client 配置と trust boundary の下で実現方法を選ぶ。

### Step 13 で次に具体化すべき領域

1. **interface boundary。** 第16節の concurrency property を各 Subsystem 間の request・response・notification の field へ落とす。本文埋込・モデル出力の authority 化をしない範囲で行う。
2. **IPC・Host↔Client 形式。** round・提示・未伝達・Client 主張・一時 copy の扱いを、Client 最小一時・非永続・削除参加の契約の範囲で具体化する。
3. **残りの Step 13 具体設計がある場合は、本書 §4・§5・§13・§16 の property を欠落させないこと。**

## 21. Requirement / Architecture Issue の有無

- **Requirement 変更。** なし。
- **Step 11 / Step 12 semantic contract の変更。** なし。
- **CI 原則変更。** なし。
- **PR 原則変更。** なし。
- **semantic owner / subsystem boundary 変更。** なし。
- **Security / Privacy / Permission semantics 変更。** なし。
- concurrency mechanism の選択自体は Issue ではない。第20.2節の Freedom の範囲で後続設計が行う。
