# 操作状態機械

> 実現する要件: **P-503**(turn/step の耐久性)、**P-504**(レーン)、
> **P-510**(エラー回復の状態面)、P-103(アボート)。
> 参照: pi の OperationState(「durable program counter」)を Ene のレーン/ターン語彙に適合させる。

[agent-loop.md](agent-loop.md) がループの**振る舞い**(claim・pre-step・
パイプライン駆動)を定義するのに対し、この文書はループの**耐久状態**を
定義する。クラッシュしても回復できるのは、この状態が
[storage-model.md](storage-model.md) の registers に total な形で
永続化されているからである。

## 1. 操作(operation)とは

**レーンが受理した仕事の1単位**。2種ある。

| kind | 内容 | 例 |
|---|---|---|
| `run` | 対話ターンまたは job ターンの実行。0以上のステップを回す | ユーザー発話への応答、job の1前進 |
| `compaction` | 履歴の圧縮(構造変更)。[context-assembly.md §7](context-assembly.md#7-compactionp-506) の実行体 | 閾値圧縮・溢れ回復圧縮 |

操作は2つの部分から成る:

- **`op.meta`**(書き込み一度): 受理データ。操作 id・レーン・意図・
  起点 seq・受理時刻。受理後に変わらない。
- **`op.state`**(遷移ごとに全上書き): 操作の**現在の全状態**。
  プログラムカウンタそのもの([storage-model.md §3.2](storage-model.md#32-書き込みの規律))。

レーンは**同時に最大1つの操作**を持つ(対話レーンの単一フライト、
[agent-loop.md §2](agent-loop.md#2-レーンp-504) の実体)。
job レーンもそれぞれ1つ。`lane.state.current_op_id` がその紐付け。

### 受理(acceptance)

`run` の受理は**1つのアトミックトランザクション**で次を書く:

```text
TX[
  entry: user/message(または trigger 相当) + turn/start,
  register: op.meta/O(受理データ),
  register: op.state/O = checkpoint{ continuation: need_assistant },
  register: lane.leaf = その entry,
  register: lane.state = { current_op_id: O }
]
```

- 受理前の検証はレーン塞がりの `LaneBusy` のみ。
  **pre-step は受理後**に走る(dsh と同じ。試行は耐久ターンとして残る)。
- 受理した瞬間から、操作はクラッシュしても回復可能になる。
- pre-step が拒否、または書き換え結果が空なら、ステップを回さず
  終端トランザクションで `turn/end{outcome: completed}` を書く
  ([agent-loop.md §4](agent-loop.md#4-ターンステップ状態機械) の
  「0ステップで閉じる」ケース)。操作は作られている。

## 2. `OpState` — プログラムカウンタ

`op.state/{op_id}` は1つの total な状態を直接保持する。
遷移は常に全体上書き、終了時は削除。**「終了」メンバーは共用体に存在しない**
——終わった操作は状態を持たず、結果は `lane.last_result` に移る(§6)。

```text
OpState = RunState | CompactionState

Control = running
        | cancel_requested { requested_at, drained: [entry_id] }

RunState {
  control: Control
  settings: {                     -- 受理時に原子に捕獲。後からの変更は次操作へ
    compaction: CompactionSettings
    tool_execution: sequential | parallel
    steering: queue_mode          -- キュー入力の捌き方(`one_at_a_time` | `batch`)
  }
  phase: RunPhase
  inbox: { wake: [entry_id], inject: [entry_id] }
  latest_assistant_seq: u64?      -- この操作で最新の確定応答
  turn_id: TurnId
}

RunPhase =
  | checkpoint { continuation, trigger_seq, threshold_checked_seq? }
  | assistant { generation: Generation }
  | tools { batch: ToolBatch }
  | deferred { waiting: ask_user | approval | sidecar_health,
               reserved_response_seq, question_ref? }
  | failure_drain { error, provenance }

Continuation = need_assistant { overflow_recovery_used: bool }
             | may_finish { include_final_assistant: bool }
```

### checkpoint(ステップの間)

ターン/ステップの**合流点**。キュー・遅延書き込み・終了をここで決める。

- 会話入力かツール結果を追記し、まだアシスタントが必要なら、
  `need_assistant` の checkpoint を書く(`trigger_seq` = 追記した entry)。
- `may_finish` の checkpoint は、境界の原因になった entry を
  `trigger_seq` に持つ(stop 確定の応答、全 terminate のツールバッチの
  最新結果)。これで閾値重複排除と回復検証が常に実在 entry を指す。
- **閾値圧縮は1境界につき最大1回**。`threshold_checked_seq` に
  確認済みの trigger を記録し、拒否・空準備・成功・クラッシュのいずれでも
  同一境界を再チェックしない。閾値圧縮に入る際は、まず現 checkpoint を
  `resume_after` にコピーしてから入る。

### assistant(生成)

```text
Generation =
  | ready { context: GenerationContext, next_attempt }
  | effect_pending { context, attempt,
                     reserved_response_seq, reserved_usage_id,
                     intended_output_limit, context_window }
  | retry_wait { context, next_attempt, not_before, error_message }

GenerationContext {
  step_id, trigger_seq,
  configuration: LaneConfiguration     -- ステップ開始時の inline スナップショット
  stream_options, retry_policy,
  overflow_recovery_used: bool         -- クラッシュ後の分類でも判るよう捕獲
}
```

- `context` は設定・ストリームオプション・再試行ポリシーを **inline に
  スナップショット**。回復は何も解決せずに「何が足りないか」を報告できる
  ([durability.md §4](durability.md#4-復元restore))。
- 試行ごとに pre-request フック([agent-loop.md §6](agent-loop.md#6-pre-step-と-waterfall-イベントp-1007) の
  `agent/request`)は `ready` から走る(再試行待機明けはまず `ready` に戻る)。
  フックの整形は捕獲済みベースオプションと合成され、
  `intended_output_limit` と `context_window` は送出**前**に
  `effect_pending` の意図へ永続化する。意図コミット前のクラッシュは
  フックを再実行し得る(リプレイ規約、[durability.md §2](durability.md#2-effect-sandwich))。

### tools(ツールバッチ)

```text
ToolBatch {
  assistant_seq: u64              -- 生成した応答
  configuration: LaneConfiguration -- 生成時スナップショット。有効ツール名はここから
  turn_id: TurnId                  -- 回復ツールイベントが使う
  calls: [ToolCall]
}

ToolCall =
  | planned { source_index, reserved_result_seq }
  | effect_pending { source_index, reserved_result_seq, replay: never | safe }
  | completed { source_index, reserved_result_seq, terminate: bool }
```

- `replay` はツール宣言の冪等性: `safe` のみクラッシュ後に再実行可。
  [tools/registry.md](../tools/registry.md) の `is_concurrency_safe` とは
  **別の軸**(並行可否 ≠ 再実行可否)。宣言は ToolDefinition に
  `replay_safety: never | safe` を追加して行う。
- バッチは生成ステップ id を保持し、回復時のツールイベントがそれを
  turn 相関として使う。
- 全呼び出しが `terminate` を付けると、run は次の要求なしに終わる。

### deferred(外部待ち)

ask-user・承認ポップアップ・サイドカーヘルス待ちなど、
**外部の回答でしか進めない**状態([agent-loop.md §9](agent-loop.md#9-人間協調面plan--ask-userp-511-p-512))。

- 予約された応答 seq と質問参照を持つ。
- 回答は inbox の `wake` として届き、**同一操作**を `checkpoint` へ戻す。
  `next_run`(現操作の終端後予約)は使わない。
- `cancel_requested` のときに回答が来ても、新しい効果は始めない
  ([durability.md §3](durability.md#3-回復ポリシー))。
- タイムアウトは通常の期限ポリシー([agent-loop.md §11](agent-loop.md#11-設定キーと既定値) の
  `harness.ask_user.timeout`)で `cancelled` セットルメント。

### failure_drain(失敗の排水)

回復不能エラーが発生したとき、確定済みの成果を整合させてから終わる状態。

- `provenance` は失敗の由来(どの応答 entry か、どの構造タスクか)。
- ここでは**新しい効果を始めない**。確定済み効果のセットルメント・
  受理済み遅延書き込み・キャンセルの完遂のみ許可。

## 3. 状態の有効性(ロード時検証)

復号した最新の lane/操作状態には、**実行のたびに**有効性検証を走らせる
(アイドルレーンも含む)。規則:

1. `lane.state.current_op_id` が指す `op.state` が存在しないのに
   レーンが「実行中」を主張する → 破損。ロード拒否。
2. `op.state` が `tools` フェーズで、バッチの `assistant_seq` が
   entries に存在しない → 破損。ロード拒否。
3. `effect_pending` の予約 seq が、既に entry として存在する →
   排他性違反([storage-model.md §3.3](storage-model.md#33-レジスタと-entry-の排他性))。ロード拒否。
4. `aborted` で確定した応答があるのに `control` が `running` →
   到達不能な状態 = 破損。ロード拒否
   ([invariants.md §1](invariants.md#1-不変条件カタログ) の I-19)。
5. `lane.last_result` は**開いている操作の次の動作を決めない**。
   次の動作は常に `op.state` が決める。

## 4. 原子遷移規則

すべての遷移は、**1つのアトミックトランザクション**で
「entry 追記(あれば)+ usage 追記(あれば)+ `op.state` 全上書き」を
コミットする。部分遷移は観測されない。

`run` の遷移表(「状態 → 効果 → 次の状態」):

| # | 現在状態 | 効果(不確実区間) | コミット内容 | 次の状態 |
|---|---|---|---|---|
| 1 | `checkpoint{need_assistant}` | なし | (ready の準備) | `assistant{ready}` |
| 2 | `assistant{ready}` | なし | 意図の予約 | `assistant{effect_pending}` |
| 3 | `assistant{effect_pending}` | **プロバイダ要求** | 応答 entry + usage + leaf 更新 | ツール呼び出しあり → `tools`、なし → `checkpoint` |
| 4 | `assistant{retry_wait}` | タイマー | なし(期限待ち) | `assistant{ready}` |
| 5 | `tools{planned あり}` | なし | `op.tool_args` 確定 | `tools{effect_pending}` |
| 6 | `tools{effect_pending}` | **ツール実行** | 結果 entry + leaf | `tools{completed}` か次呼び出し |
| 7 | `tools{全 completed}` | なし | checkpoint | `checkpoint` |
| 8 | `checkpoint{may_finish}` | なし | 終端 | (状態削除) |
| 9 | 任意 | ask-user/承認要求 | 質問イベント + 予約 | `deferred` |
| 10 | `deferred` | 回答の取り込み(inbox `wake`) | 回答 entry | `checkpoint{need_assistant}` |
| 11 | 任意(回復不能エラー) | なし | エラー記録 | `failure_drain` |
| 12 | `failure_drain` | 整合処理 | 確定済み効果のセットルメント | 終端 |

- 効果(#3, #6)だけが「コミットの外」で起きる不確実な部分。
  その前後をコミットで挟むのが **effect sandwich**
  ([durability.md §2](durability.md#2-effect-sandwich))。
- コンパクションが閾値/溢れで必要になった場合、`checkpoint` から
  `compaction` フェーズに入り、`resume_after` の checkpoint に戻る
  (RunPhase に含めず、独立操作として走らせる実装も可。いずれにせよ
  閾値重複排除は §2 の `threshold_checked_seq` で守る)。

## 5. CompactionState(構造変更)

```text
CompactionState {
  control: Control
  phase: preparing | summarizing { reserved_summary_seq } | committing
  preparation: {                  -- op.preparation に書き込み一度
    messages_to_summarize: [seq 範囲]
    retained_tail: [seq 範囲]     -- 原文保持する末尾
    tokens_before: u64
    previous_summary_seq: u64?
    settings: CompactionSettings
    prune_plan: [{ call_id, original_size, kept_chars }]
  }
  trigger: pressure | overflow
}
```

- **ロック括弧**: コンパクションは開始イベントでロックを取得し、
  **最後に**解除する。途中でクラッシュすると
  「開始あり・解除なし」の孤立ロックとして検出できる
  (「解除あり」が偽の完了を主張するのを防ぐ)。
  イベントは [session-log.md §3.3](session-log.md#33-メッセージ) の
  `compaction/applied` に加え、`compaction/start` と `compaction/end`
  を**語彙に追加**する(payload: `compaction_id`、`start` は `turn` 参照)。
- `preparation` は判断フックの前に `op.preparation` へ書き込み一度。
  クラッシュしても再計算不要。
- 要約生成は `summarizing`(effect sandwich の不確実区間)。
  確定は**1つの終端コミット**で、要約 entry + `compaction/applied` +
  shadowed 範囲の記録 + usage を書く。
- **prune の組み込み**: 直近ツール出力の刈り込みは**不確実区間を持たない**
  純ローカル計算なので、サンドウィッチは不要。刈り込み計画は
  `preparation.prune_plan` に書き込み一度で記録し、置換本体
  (`tool/pruned` entry 群)は**終端コミットに同梱**する
  ([context-assembly.md §7](context-assembly.md#7-compactionp-506))。
  計画にない出力は刈り込まない(再計算しない)。
- 元メッセージ行は削除しない(投影が置き換える、L-3)。
- 失敗分類([context-assembly.md §9](context-assembly.md#9-障害モード) を補完):
  `busy`(レーン使用中)/ `cancelled` / `changed`(準備中に履歴が変わった)/
  `summary`(要約生成失敗)/ `commit`(確定失敗)/ `declined`(畳むものが無い)。
  手動コンパクションはこれらをエラーコードとして返す。
  `declined` は失敗ではなく結果バリアント
  ([lane-api.md §3](lane-api.md#3-結果の形))。

## 6. 終端トランザクション

操作の終わりは、**1つのトランザクション**で次を行う:

```text
TX[
  delete op.meta/O,
  delete op.state/O,
  delete op.tool_args/O:*(プレフィックススキャンで全削除),
  delete op.preparation/O:*,
  delete 未消化の pending.entry(この操作が所有していた分),
  upsert lane.last_result = { O, outcome, final_seq },
  upsert lane.state = { current_op_id: null },
  entry: turn/end { outcome }
]
```

- **操作ごとに最大1回**しかコミットされない。
- 条件付きコミットまたは再ロードで自分のレジスタが消えているのを
  見つけた駆動は、**何も書かずに停止**し、`lane.last_result` から
  結果を解決する(外部終端、[durability.md §5](durability.md#5-外部終端))。
- 終了したセッションに残るのは、会話・台帳・少数の lane/fact
  レジスタのみ。死んだ状態は残らない
  ([storage-model.md §3.2](storage-model.md#32-書き込みの規律))。
- `outcome` は [session-log.md §3.2](session-log.md#32-ターンとステップ) の
  `turn/end.outcome` と同一語彙(`completed | interrupted | cancelled | failed`)。

## 7. inbox とキューの耐久化

inbox の**種別は3つだけ**([agent-loop.md §3](agent-loop.md#3-inbox-と-claim)):

| 種別 | 即時性 | 耐久キュー | レーンコマンド |
|---|---|---|---|
| `wake` | 即時(ターンを起こす/続ける) | `inbox.wake` | `prompt`(新操作)、`follow_up`(現操作の次ターン) |
| `inject` | 待機(次 claim に載せる。ターンを起こさない) | `inbox.inject` | `steer` |
| `interrupt` | 即時(実行中ステップを中断) | `control: cancel_requested` | `abort()`、音声バグイン |

- キュー項目は **1項目 = 1 entry id**。ペイロードは各 id の
  `pending.entry` レジスタから解決する
  ([storage-model.md §3.4](storage-model.md#34-pendingentry未配置コンテンツ))。
- `steer` は **inject** であり、音声 `interrupt` ではない。
  進行中ターンの次 claim にテキストを載せる。生成そのものは割らない。
- キューモード(`settings.steering`): `one_at_a_time`(1つ捌いたら生成)
  / `batch`(境界で全取り込み)。既定 `one_at_a_time`。
- アボートは `wake`/`inject` の id を `control.drained` に移す。
  レジスタは**削除しない**——アボート結果の報告と、クラッシュ後の
  再開が、そこからペイロードを解決するため。削除は終端トランザクションのみ。
- `cancelQueued` の3分岐(pending → `cancelled` / entry 存在 →
  `already_consumed` / それ以外 → `not_found`)は
  [storage-model.md §3.1](storage-model.md#31-名前空間全集) の通り。
- **委譲のメールボックス**は同じ機構の上に載る:
  子→親の wake/inject は親レーンの `pending.entry`、
  親→子の instruct/answer/`message` は子の `delegation.inbox` レジスタ。
  子の終端トランザクションが親レーンへの終端 wake を**同トランザクション**
  で書くため、報告が消える窓はない([delegation.md §8](delegation.md#8-耐久性))。

## 8. 設定キーと既定値

| キー | 既定 | 説明 |
|---|---|---|
| `harness.queue.steering_mode` | `one_at_a_time` | inject(`steer`)の捌き方 |
| `harness.operations.validation` | `strict` | ロード時検証。`strict`(拒否)/ `warn`(警告して継続) |

([agent-loop.md §11](agent-loop.md#11-設定キーと既定値) の既存キー
`harness.loop.*`・`harness.delegation.*` はそのまま有効。この文書は
それらの耐久機構を定義する。`harness.jobs.*` は使わない。)

---

- 前: [storage-model.md](storage-model.md) / 次: [durability.md](durability.md)
