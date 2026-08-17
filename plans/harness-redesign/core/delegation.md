# 委譲(非同期サブエージェント)

> 実現する要件: **P-519**(非同期委譲と層間対話)、**P-521**(完了報告の対話化)、
> **P-508**(秘匿サブエージェント)、**P-522**(表層/裏層)、P-504(job レーンの実体)、P-605(job の実体)。
> 参照: 一般的なハーネスの同期 subagent ではなく、**会話を止めない非同期委譲**を
> Ene の既定とする。同期ツール結果を待つサブエージェントは持たない。
> 層間の通信骨格は [Codex](https://github.com/openai/codex) multi-agent v2
> (`NEW_TASK` / `MESSAGE` / `FINAL_ANSWER`)に倣う。

## 1. 位置づけ

コンパニオンは表層 soul と裏層ハーネスの2層で1体である
([../product/vision.md](../product/vision.md#51-コアデーモン内の2層))。
この文書は、その**層間の実行面**を定義する。

このアプリの対話レーン(表層)は**会話のために空けておく**。複雑な作業
(まとまった調べ物、文書作成、コード作業、長い計算)は、対話レーンの外で走る
**委譲(delegation)** に移す。委譲の実行体が裏層ハーネスである。

ただし**すべてを委譲するわけではない**。表層は副作用のない読み取り専用
ツールを持ち、計画立案を要さない簡単な作業はその場で片付ける(D-1/D-2、
[agent-loop.md §2.2](agent-loop.md#22-表層のツール面d-1--d-2))。
「今何時?」のために委譲の往復を起こすのは、遅延と機構の重さに見合わない。
見誤りは自動昇格(§4.1)で回収する。

- **表層は仲介層である**: 通常のハーネスに存在しない層として、
  表層のエージェントは「裏で走る LLM 群(委譲・記憶抽出・
  承認判断など)」と「ユーザーの要望」のあいだを仲介する。
  裏層の仕事の束ね方・報告の伝え方・ユーザーへの
  質問のまとめ方を管理し、ユーザーは常に1体のコンパニオンと
  話している体験を保つ(1つの声、[invariants.md](invariants.md) I-31)。
- 表層は委譲を**待たない**。
  `delegate.*` ツールは受理確認+ハンドルを即座に返し、ターンは続く。
  **ブロッキング await は存在しない**。会話が同期点になることを
  許さないためである。「待っていて」は会話で扱う
  (「できたら言うね」が正解)。
- 委譲の完了・質問・失敗は、表層の対話レーンへの**層間メッセージ**として
  届き、表層がユーザーへの伝え方を決める(P-521)。
- job(P-605)と秘匿サブエージェント(P-508)は**同じ機構の2モード**
  である。UX の区別は維持する(§2)。
- 表層と裏層は互いに進捗を確認できる(§4 の `status`、§5 の `message`)。
  裏層は表層の対話履歴を既定では見ない(§7)。知るのは inbox に届いた
  層間メッセージと、表層が選んだ excerpt だけである。

## 2. モードとユーザー可視性

| モード | ユーザーから | UI | ログの扱い |
|---|---|---|---|
| `internal` | **見えない**(秘匿サブエージェント) | なし。承認ポップアップは匿名化([../security/approval.md §4](../security/approval.md#4-リスク分類とポップアップp-905)) | 親セッションには `delegation/*` 要約イベントのみ。子セッションは診断専用 |
| `public` | **job(ユーザー向けの呼称は「タスク」)** として見える | コンパニオンの発話として進捗が届く(D-13、[../tasks/jobs-and-schedules.md](../tasks/jobs-and-schedules.md)) | 同上+`job/progress` ライブイベント |

モードは委譲の開始時に決まり、途中で変えない。
内部機構の非表示は [visibility.md](visibility.md) の規則に従う。
以降の「親」は表層 soul、「子」は裏層の1委譲(1 job レーン)を指す。

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

## 4. 層間エンベロープと表層 → 裏層

層間メッセージは Codex multi-agent v2 の3種を骨格にする。
ツール名は既存の `delegate.*` / `delegation.send` に載せる。

| 種別 | Codex v2 | 方向 | inbox | 載せるツール |
|---|---|---|---|---|
| `task` | `NEW_TASK`(spawn) | 表層 → 裏層 | wake | `delegate.start`(モデルが呼ぶ)、または §4.1 の自動昇格 |
| `task` | `NEW_TASK`(followup) | 表層 → 裏層 | wake | `delegate.instruct` |
| `message` | `MESSAGE` | 表層 → 裏層 | **inject**(ターンを起こさない) | `delegate.message` |
| `message` | `MESSAGE` | 裏層 → 表層 | inject | `delegation.send{progress\|artifact_ready}` |
| `question` | — | 裏層 → 表層 | wake | `delegation.send{question}` |
| `answer` | — | 表層 → 裏層 | wake | `delegate.answer` |
| `final` | `FINAL_ANSWER` | 裏層 → 表層 | wake(終端) | `delegation.send{complete\|failed}` |
| `cancel` | — | 表層 → 裏層 | wake | `delegate.cancel` |

表層モデルには `delegate.*` ツール族が公開される。すべて**非同期**。
副作用のある作業ツール(fs / exec / web / browser / 送信系)は表層には出さない
([../tools/registry.md](../tools/registry.md) §3.1)。

| ツール | 効果 | 返値 |
|---|---|---|
| `delegate.start(goal, mode, excerpt?, tools?, budget?)` | 委譲を作成し job レーンへ(`task`) | `delegation_id`+受理確認 |
| `delegate.instruct(id, message)` | 追加指示。子の inbox に `wake` で届く(`task`) | 受理確認 |
| `delegate.message(id, message)` | 補足・状況共有。子の inbox に **inject** で届く。子の実行中ターンを起こさず、次の claim で見える | 受理確認 |
| `delegate.answer(id, question_id, answer)` | 子の質問への回答(§5) | 受理確認 |
| `delegate.status(id)` | 状態・最新進捗・未回答質問の**読み取り**。子にメッセージは送らない | 状態スナップショット |
| `delegate.cancel(id)` | 協調的キャンセル。確定済み成果は保持 | 受理確認 |

- `excerpt` は親が**自分で選んだ**関連文脈(会話の抜粋・記憶の要点)。
  子は親の対話履歴を既定では見ない(§7)。
- `status` はレジスタ読み取りなので何度呼んでも安く、
  「あれどうなってる?」に表層が即答できる。裏層の progress `message` と
  あわせて、両層が互いの進捗を確認する手段になる。
- `instruct`(wake)と `message`(inject)を分ける。Codex v2 の
  `followup_task` と `send_message` に対応する。急がない共有で
  裏層の作業ターンを割らない。
- キャンセルは**再試行安全**: 完了済みの委譲への cancel は
  `already_completed` を返し、取消済みへの再送は `cancelled` を返す
  ([lane-api.md](lane-api.md) の `cancelQueued` と同じ思想)。
  失われた cancel を再送しても状態は壊れない。

### 4.1 自動昇格(P-523 / D-3)

委譲は、モデルが `delegate.start` を呼ぶ経路のほかに、**ハーネスが機構として
起こす**経路を持つ。表層は簡単な作業を自分で片付けてよい(D-1)ので、
「簡単だと思ったが実は複雑だった」場合の回収が必要になる。

起点と手順は [agent-loop.md §2.3](agent-loop.md#23-委譲への自動切り替えp-523--d-3)。
この文書の側で押さえるのは3点。

- 昇格で作られる委譲は、モデルが呼んだものと**同じ実体**である。
  別のライフサイクルや別の報告経路を作らない。
- 委譲ブリーフは、モデルが書いた目標文ではなく、**それまでのステップの畳み込み**
  から作る(何を調べ、何が分かり、次に何が必要か)。表層が既に得た結果を
  裏層が最初からやり直さないため。
- 昇格は `public` モードで起こす。ユーザーが依頼した作業の続きであり、
  秘匿する理由がない。

## 5. 子 → 親(メールボックス)

裏層は `delegation.send`(ハーネス機能ツール、
[../tools/registry.md §0.1](../tools/registry.md#01-ハーネス機能ツールホスト内))で
表層へメッセージを送る。種別と配送:

| 種別 | エンベロープ | 親 inbox での扱い | 用途 |
|---|---|---|---|
| `progress` | `message` | **inject**(待機。ターンを起こさない) | 進捗・中間所感。`delegation.active` Source を更新し、親は次ターン境界で自然に知る |
| `question` | `question` | **wake** | 親の判断が欲しい質問。`delegation.pending_question` に予約され、回答か取下げまで子は待つか自立判断する(§9)。複数同時可 |
| `artifact_ready` | `message` | inject | 成果物の中間交付(親が先に確認できる) |
| `complete` | `final` | **wake**(終端) | 完了報告。結果要約+成果物参照 |
| `failed` | `final` | **wake**(終端) | 失敗報告。error_class+どこまで出来たか |

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

**v1.0 の保証**: 委譲は中断を検出できる形で記録される。プロセスが死んだら、
再起動時に未完了の委譲が検出され、後始末され、コンパニオンがユーザーに
中断を報告する([agent-loop.md §12](agent-loop.md#12-中断の検出と報告p-515--d-5))。
中断された委譲は**自動再開しない**。

メールボックスは、子→親の wake/inject が親レーンに予約された形で永続する。
子の終端(turn/end + `delegation/end` + 親レーンへの終端 wake + 進捗の掃除)は
**1トランザクション**でコミットする。報告が途中で消える窓を作らないため。
これは v1.0 でも守る——効果の重複を防ぐ話ではなく、報告の欠落を防ぐ話であり、
単一トランザクションで足りる。

**後継設計**(P-525、D-4): 子の実行本体を
[operations.md](operations.md)/[durability.md](durability.md) の操作状態機械に
載せると、任意のクラッシュ位置から重複効果なしで再開できるようになる。
v1.0 ではここまで踏み込まない。

## 9. 予算とガード

| ガード | 規則 |
|---|---|
| 並行上限 | soul ごとの job レーン上限(`harness.delegation.max_active`)。超過時 `delegate.start` は `rejected{slots_full}` |
| progress のレート制限 | 最小間隔を置き、超過分は最新1件にまとめる |
| 質問の同時数 | 複数可(上限 `harness.delegation.max_open_questions`)。上限超過時の質問は拒否(子はその場で自立判断)。親への配信は**結合**して届く(§6) |
| 質問のタイムアウト | 未回答の質問ごとに個別に数え、超過すると子は**仮定を置いて進み**、その仮定を progress で報告する。停止して腐らない |
| ステップ/実時間予算 | §7。超過は `failed{budget}` |
| 昇格ループ | 自動昇格(§4.1)は表層ターンごとに1回。同一ターンで再度起きたら、そのターンは委譲済みとして閉じる |

## 10. 設定キー

具体的な数値は実装しながら決める(D-29)。

| キー | 説明 |
|---|---|
| `harness.delegation.max_active` | soul ごとの並行委譲上限 |
| `harness.delegation.step_budget` | 子ターンの合計ステップ上限 |
| `harness.delegation.wall_timeout` | 委譲の実時間予算 |
| `harness.delegation.progress_min_interval` | progress 最小間隔 |
| `harness.delegation.max_open_questions` | 委譲ごとの未回答質問の上限 |
| `harness.delegation.max_depth` | 委譲の深さ上限(資源ガード、§7) |
| `harness.delegation.question_timeout` | 子の質問の親回答待ち上限 |
| `harness.delegation.report_gates` | 報告ターンに quiet hours/疲労ゲートを適用するか |

## 11. 障害モード

| 障害 | 挙動 |
|---|---|
| 子の質問中に親がユーザーへ ask-user | 親は対話レーンでキャラとして質問し、回答を `delegate.answer` に転送できる。2つの待ちは対話で直列化する。ポップアップは承認 plane に限る([agent-loop.md §9](agent-loop.md#9-人間協調面plan--ask-userp-511-p-512)) |
| 報告 wake の滞留中にユーザーが長会話 | ユーザー wake が優先され続ける。報告は claim 可能になり次第1ターンに結合して届く。`pending.entry` なので失われない |
| 子が予算超過で死んだ | `failed{budget}` の終端報告が親に届く。中間成果物は保持 |
| 親のキャンセルと子の完了が同時 | 先にコミットされた側が成立。cancel が先 → `cancelled`、完了が先 → cancel は `already_completed`(競合カタログ、[invariants.md §2](invariants.md#2-競合カタログ)) |
| タイムアウトで仮定を置いた後に親の回答が遅着 | 仮定を維持し、回答は棄棄してログに残す(子が方針を二転させない)。遅着の事実は progress で補足できる |
| 子の承認ポップアップ中に close | 承認は拒否して閉じる([durability.md §7](durability.md#7-close--制御されたクラッシュ))。子は `denied` として継続判断する |
| internal 委譲の存在をユーザーに示す経路 | 存在しない。UI 投影・ライブイベント・承認表示のすべてで匿名化されていることを CI で検証(visibility.md) |

---

- 前: [agent-loop.md](agent-loop.md) / 次: [visibility.md](visibility.md)
