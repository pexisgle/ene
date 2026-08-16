# 委譲(非同期サブエージェント)

> 実現する要件: **P-519**(非同期委譲と主従対話)、**P-521**(完了報告の対話化)、
> **P-508**(秘匿サブエージェント)、P-504(job レーンの実体)、P-605(job の実体)。
> 参照: 一般的なハーネスの同期 subagent ではなく、**会話を止めない非同期委譲**を
> Ene の既定とする。同期ツール結果を待つサブエージェントは持たない。

## 1. 位置づけ

このアプリの対話レーンは**会話のために空けておく**。実作業(調べ物、
文書作成、コード作業、長い計算)は、対話レーンの外で走る
**委譲(delegation)** に移す。

- **対話レーンは仲介層である**: 通常のハーネスに存在しない層として、
  対話レーンのエージェントは「裏で走る LLM 群(委譲・記憶抽出・
  承認判断など)」と「ユーザーの要望」のあいだを仲介する。
  子エージェントたちの仕事の束ね方・報告の伝え方・ユーザーへの
  質問のまとめ方を管理し、ユーザーは常に1体のコンパニオンと
  話している体験を保つ(1つの声、[invariants.md](invariants.md) I-31)。
- 親(対話レーンのエージェント)は委譲を**待たない**。
  `delegate.*` ツールは受理確認+ハンドルを即座に返し、ターンは続く。
  **ブロッキング await は存在しない**。会話が同期点になることを
  許さないためである。「待っていて」は会話で扱う
  (「できたら言うね」が正解)。
- 委譲の完了・質問・失敗は、親の対話レーンへの**メッセージ**として
  届き、親がユーザーへの伝え方を決める(P-521)。
- job(P-605)と秘匿サブエージェント(P-508)は**同じ機構の2モード**
  である。UX の区別は維持する(§2)。

## 2. モードとユーザー可視性

| モード | ユーザーから | UI | ログの扱い |
|---|---|---|---|
| `internal` | **見えない**(秘匿サブエージェント) | なし。承認ポップアップは匿名化([../security/approval.md §4](../security/approval.md#4-リスク分類とポップアップp-905)) | 親セッションには `delegation/*` 要約イベントのみ。子セッションは診断専用 |
| `public` | **job(「おつかい」)** として見える | job カード・進捗表示([../tasks/jobs-and-schedules.md](../tasks/jobs-and-schedules.md)) | 同上+`job/progress` ライブイベント |

モードは委譲の開始時に決まり、途中で変えない。
内部機構の非表示は [visibility.md](visibility.md) の規則に従う。

## 3. ライフサイクル

```text
created → running → completed
              ├─→ failed(error_class 付き)
              └─→ cancelled
```

- 委譲は **job レーン**で走る([agent-loop.md §2](agent-loop.md#2-レーンp-504))。
  1委譲 = 1 job レーン = 同時に1操作(既存のレーン規則そのまま)。
- 実体は「目標+ツール集合+ターン列」。子セッション
  ([session-log.md](session-log.md) の sessions 表の `delegation_id` 参照)を
  持ち、子のターンは `origin: delegation` で記録される。
- 進捗・成果物・キャンセルの UX は public モードのみ
  ([../tasks/jobs-and-schedules.md](../tasks/jobs-and-schedules.md))。

## 4. 親 → 子(親モデルのツール)

親モデルには `delegate.*` ツール族が公開される。すべて**非同期**。

| ツール | 効果 | 返値 |
|---|---|---|
| `delegate.start(goal, mode, excerpt?, tools?, budget?)` | 委譲を作成し job レーンへ | `delegation_id`+受理確認 |
| `delegate.instruct(id, message)` | 追加指示。子の inbox に `wake` で届く | 受理確認 |
| `delegate.answer(id, question_id, answer)` | 子の質問への回答(§5) | 受理確認 |
| `delegate.status(id)` | 状態・最新進捗・未回答質問の**読み取り**。子にメッセージは送らない | 状態スナップショット |
| `delegate.cancel(id)` | 協調的キャンセル。確定済み成果は保持 | 受理確認 |

- `excerpt` は親が**自分で選んだ**関連文脈(会話の抜粋・記憶の要点)。
  子は親の対話履歴を既定では見ない(§7)。
- `status` はレジスタ読み取りなので何度呼んでも安く、
  「あれどうなってる?」に親が即答できる。
- キャンセルは**再試行安全**: 完了済みの委譲への cancel は
  `already_completed` を返し、取消済みへの再送は `cancelled` を返す
  ([lane-api.md](lane-api.md) の `cancelQueued` と同じ思想)。
  失われた cancel を再送しても状態は壊れない。

## 5. 子 → 親(メールボックス)

子は `delegation.send` 内部ツールで親へメッセージを送る。
種別と配送:

| 種別 | 親 inbox での扱い | 用途 |
|---|---|---|
| `progress` | **inject**(待機。ターンを起こさない) | 進捗・中間所感。`delegation.status` Source を更新し、親は次ターン境界で自然に知る |
| `question` | **wake** | 親の判断が欲しい質問。`delegation.pending_question` に予約され、回答か取下げまで子は待つか自立判断する(§9)。複数同時可 |
| `artifact_ready` | inject | 成果物の中間交付(親が先に確認できる) |
| `complete` | **wake**(終端) | 完了報告。結果要約+成果物参照 |
| `failed` | **wake**(終端) | 失敗報告。error_class+どこまで出来たか |

- 親への配送実体は、親の対話レーンの inbox への
  `pending.entry` 書き込み([operations.md §7](operations.md#7-inbox-とキューの耐久化))。
- **孫委譲の質問/報告は親へ直送される**(仲介層に集約)。
  中継ツリーは作らない——ユーザーとの窓口は常に1つ。
- **ユーザー発話が常に優先**: claim の順序は
  ユーザー由来 wake > 委譲由来 wake > inject
  ([agent-loop.md §3](agent-loop.md#3-inbox-と-claim))。会話が委譲報告で
  遅延しない。

## 6. 報告ターン(P-521)

終端 wake(`complete`/`failed`)は、親の対話レーンで
`origin: subagent` のターンになる。

- **実行中ターンを割らない**。親ターンが進行中なら wake はキューに
  留まり、次のターン境界で claim される(L-4 は維持)。
- **結合**: claim 時点で複数の終端 wake や質問 wake が溜まっていれば、
  1つの報告ターンにまとめて届ける(報告の散弾化を防ぐ)。
  複数の質問は1ターンの中でまとめてユーザーに尋ねられる。
- 報告ターンを受け取った親は、出力契約に基づき**伝え方を自分で選ぶ**:
  - ユーザーに発話で報告する(通常)
  - 内面だけに留める(ユーザーが忙しい・重要度が低い)
  - 静黙する(quiet hours・疲労ゲート。[../companion/proactive.md §2](../companion/proactive.md#2-ゲートの詳細))
- 報告の内容(子の生ログ・ツール生出し)は親にも**渡らない**。
  渡るのは終端メッセージの要約と成果物参照のみ
  ([visibility.md §2](visibility.md#2-チャネル分類全体表))。
- 沈黙の場合も、報告の受理自体はログに残る
  (`delegation/end`+ターン記録)。なかったことにはならない。

## 7. 子のコンテキストと実行

- **子コンテキストの構成**: 委譲ブリーフ(goal+excerpt)+
  soul の identity+関連 skill+自身の作業ディレクトリの
  workspace コンテキスト。**親の対話履歴は既定で含めない**。
  含める内容は親が excerpt で明示的に選ぶ(コンテキストの意図的管理)。
- **モデル**: soul の job 用タスク設定(`ai.tasks.job`)。
  会話より安価/ローカルなモデルを選べる
  (全 LLM 呼び出しのクラウド/ローカル選択、骨格決定14)。
- **ツール集合**: 親の部分集合。子は `delegate.*` を持つ
  (**委譲の再帰は許可**——孫委譲)。承認 plane は親と同じものを使うが、
  ポップアップの表示はモードで変わる(§2)。
- **再帰の管理は予算で、禁止ではない**: 孫以降の委譲も同じレーンプール・
  ステップ予算・実時間予算の制約下にある(§9)。深さの上限
  (`harness.delegation.max_depth`: 既定 3)は**資源管理のガード**であり、
  機構の制限ではない。深さ上限到達時の `delegate.start` は
  `rejected{depth_exceeded}` を返し、子は自力で進めざるを得ない。
- **秘匿の連鎖**: 孫委譲の秘匿性は連鎖する——internal の子の孫も
  internal(存在が親より上のユーザー可視チャネルに出ない)。
  public の子が internal の孫を持つことは可能(逆は不可:
  internal の子の孫は internal のみ。public 化は親委譲の
  存在秘匿を破るため)。
- **予算**: ステップ予算(`harness.delegation.step_budget`)と
  実時間予算(`harness.delegation.wall_timeout`)。超過は
  `failed{error_class: budget}` で終端し、報告する。

## 8. 耐久性

- 子の実行本体は job レーンの操作であり、
  [operations.md](operations.md)/[durability.md](durability.md) の
  規則がそのまま適用する。
- **メールボックスの耐久化**:
  - 子→親の wake/inject は、親レーンの `pending.entry`
    (1項目=予約 entry id)。クラッシュしても失われない。
  - 親→子の instruct/answer は、子レーンの
    `delegation.inbox` レジスタに届き、子の次 claim で消費される。
- **終端の原子性**: 子の終端トランザクションは
  「子の turn/end + 子セッションの `delegation/end` +
  親レーンへの終端 wake(pending.entry)+ 進捗レジスタの掃除」を
  **1トランザクション**でコミットする。報告が途中で消える窓はない。
- 親が委譲を開始した直後にクラッシュした場合も、
  受理トランザクション(LaneBusy 検査+`op.meta`/`op.state`)は
  既存の受理規則そのもの([operations.md §1](operations.md#1-操作operationとは))。

## 9. 予算とガード

| ガード | 規則 |
|---|---|
| 並行上限 | soul ごとの job レーン上限(`harness.delegation.max_active`: 既定 4)。超過時 `delegate.start` は `rejected{slots_full}` |
| progress のレート制限 | 最小間隔 `harness.delegation.progress_min_interval`(既定 30 秒)。超過分は最新1件にまとめる |
| 質問の同時数 | 複数可(上限 `harness.delegation.max_open_questions`: 既定 4)。上限超過時の質問は拒否(子はその場で自立判断)。親への配信は**結合**して届く(§6) |
| 質問のタイムアウト | `harness.delegation.question_timeout`(既定 24 時間)。未回答の質問ごとに個別に数え、超過すると子は**仮定を置いて進み**、その仮定を progress で報告する。停止して腐らない |
| ステップ/実時間予算 | §7。超過は `failed{budget}` |

## 10. 設定キーと既定値

| キー | 既定 | 説明 |
|---|---|---|
| `harness.delegation.max_active` | `4` | soul ごとの並行委譲上限 |
| `harness.delegation.step_budget` | `128` | 子ターンの合計ステップ上限 |
| `harness.delegation.wall_timeout` | `2h` | 委譲の実時間予算 |
| `harness.delegation.progress_min_interval` | `30s` | progress 最小間隔 |
| `harness.delegation.max_open_questions` | `4` | 委譲ごとの未回答質問の上限 |
| `harness.delegation.max_depth` | `3` | 委譲の深さ上限(資源ガード、§7) |
| `harness.delegation.question_timeout` | `24h` | 子の質問の親回答待ち上限 |
| `harness.delegation.report_gates` | `true` | 報告ターンに quiet hours/疲労ゲートを適用 |

## 11. 障害モード

| 障害 | 挙動 |
|---|---|
| 子の質問中に親がユーザーへ ask-user | 親は ask-user の回答を `delegate.answer` に転送できる。2つの待ちが1つのポップアップ経路に直列化する([agent-loop.md §9](agent-loop.md#9-人間協調面plan--ask-userp-511-p-512)) |
| 報告 wake の滞留中にユーザーが長会話 | ユーザー wake が優先され続ける。報告は claim 可能になり次第1ターンに結合して届く。`pending.entry` なので失われない |
| 子が予算超過で死んだ | `failed{budget}` の終端報告が親に届く。中間成果物は保持 |
| 親のキャンセルと子の完了が同時 | 先にコミットされた側が成立。cancel が先 → `cancelled`、完了が先 → cancel は `already_completed`(競合カタログ、[invariants.md §2](invariants.md#2-競合カタログ)) |
| タイムアウトで仮定を置いた後に親の回答が遅着 | 仮定を維持し、回答は棄棄してログに残す(子が方針を二転させない)。遅着の事実は progress で補足できる |
| 子の承認ポップアップ中に close | 承認は拒否して閉じる([durability.md §7](durability.md#7-close--制御されたクラッシュ))。子は `denied` として継続判断する |
| internal 委譲の存在をユーザーに示す経路 | 存在しない。UI 投影・ライブイベント・承認表示のすべてで匿名化されていることを CI で検証(visibility.md) |

---

- 前: [agent-loop.md](agent-loop.md) / 次: [visibility.md](visibility.md)
