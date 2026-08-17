# エージェントループ

> 実現する要件: **P-503**(turn/step)、**P-504**(対話1+job 並行)、
> **P-508**(秘匿サブエージェント)、**P-509**(guard)、**P-510**(エラー回復)、
> **P-511**(plan)、**P-512**(ask-user)、**P-515**(中断の検出と報告)、
> **P-522**(表層/裏層)、**P-523**(委譲への自動切り替え)、P-103(割り込み)、
> **P-1007**(waterfall/emit)。

## 1. 責務と責務外

- **責務**: ターン/ステップの状態機械、入力キュー(inbox)、キャンセルと
  音声割り込み、ガード、エラー回復、委譲への自動切り替え、サブエージェント委譲、
  plan/ask-user、再起動時の中断検出と報告。
- **責務外**: コンテキスト組立([context-assembly.md](context-assembly.md))、
  ログの語彙([session-log.md](session-log.md))、ツール実行そのもの
  ([../tools/registry.md](../tools/registry.md))。
- **段階**: v1.0 のクラッシュ耐性はこの文書の §12 に閉じる——中断を検出し、
  後始末し、報告するまで。未消化の inbox はログから復元して見せ、
  実行は再開しない(D-5、P-515)。任意のクラッシュ位置から
  **重複効果なしで再開する**ための耐久状態
  ([operations.md](operations.md) の `op.state` プログラムカウンタと
  [durability.md](durability.md) の効果のサンドウィッチ)は**後継設計**であり、
  v1.0 では実装しない(D-4、P-525)。

## 2. レーン(P-504)

コンパニオンは2層で走り、レーンはその駆動面である(P-522)。
位置づけは [../product/vision.md](../product/vision.md#51-コアデーモン内の2層)。

| 層 | レーン | 本数 | 並行性 | ターンの origin |
|---|---|---|---|---|
| 表層 soul | 対話レーン | 1 | **単一フライト**。先行ターン終了まで次は受け付けない | `user`, `proactive`, `scheduled` |
| 裏層ハーネス | job レーン | 複数(上限は `harness.delegation.max_active`) | 並行 | `delegation`(委譲の子ターン。[delegation.md](delegation.md)) |

- ユーザー入力(音声・テキスト)は**対話レーンにだけ**入る。裏層は層間
  メッセージ以外の入口を持たない。
- 対話レーンが塞がっているときのユーザー発話は **inbox** に入る(§3)。
- job レーンのターンは対話レーンに影響しない(逆も同じ)。
  ただし排他資源(音声出力・承認応答)は platform 層が調停する。
- 対話レーンのターンにはさらに `subagent` origin の**報告ターン**がある
  (委譲の完了・質問が inbox を経て届く、[delegation.md §6](delegation.md#6-報告ターンp-521))。
- 表層の公開ツール面は**副作用のないツール**に限る(D-2)。表層はその範囲で、
  計画立案を要さない簡単な作業を自分で片付けてよい(D-1)。
  副作用のあるツールは裏層にだけ公開する
  ([../tools/registry.md](../tools/registry.md) §2)。

### 2.1 エージェント実装の登録表(P-522)

カーネル(ログ・耐久性・Tool Calling / MCP / skill 形式・レジストリ)は共有する。
**違うのは専用実装**であり、公開ツールのフィルタだけではない。

| 項目 | 表層実装 | 裏層実装 |
|---|---|---|
| レーン | 対話レーン 1 本 | job レーン(spawn ごと 1 本、上限 `harness.delegation.max_active`) |
| 公開ツール | **副作用のないツールすべて** + `delegate.*`(§2.2) | 作業ツール一式、`delegation.send`、再帰 `delegate.*` |
| ステップ上限 | `harness.loop.max_steps_per_turn`(小さい。超過は §2.3 の切り替えを起こす) | `harness.delegation.step_budget`(大きい。委譲全体で数える) |
| 1ターンの性格 | 発話+内面。ツールは読み取りと委譲のみ | ツールステップが主。発話チャネルなし(I-31) |
| waterfall | `agent/pre-step`: 感情・quiet hours・能動発話ゲート | `agent/pre-step`: guard。`tools/pre-execute`: 承認・sandbox |
| Context Source | [context-assembly.md §2.1](context-assembly.md#21-層ごとの-source-集合) の表層集合 | 同節の裏層集合 |
| モデルタスク | `ai.tasks.chat` | `ai.tasks.job` |
| workspace | 持たない | spawn ごとに独立([../tasks/jobs-and-schedules.md](../tasks/jobs-and-schedules.md)) |

後継の実装差し替え(P-514)は、この表の列を別アーキテクチャに入れ替える余地である。
v1.0 はこの2実装で固定する。

### 2.2 表層のツール面(D-1 / D-2)

表層は「簡単な作業なら自分でやる」。ただし**簡単さでは境界を引かない**。

境界は**副作用の有無**である。ツール定義が宣言する `side_effects` が空の
ツールだけを表層に公開し、書き込み・実行・送信・外部状態の変更を伴うものは
公開しない。「簡単さ」はモデルの自己申告でしか判定できないため、
それを副作用の許可条件にすると事故が防げない。副作用の有無は
ツール定義から静的に決まるので、境界として信頼できる。

表層に出るのは、たとえば時刻・記憶想起・計算・ファイル読み取り・
Web 取得のような、失敗しても状態が変わらない呼び出しである。
これにより「今何時?」「さっきのファイル名は?」のために委譲の往復を
起こさずに済む。会話の即応性は存在感の中核であり、一律委譲はそれを壊す。

### 2.3 委譲への自動切り替え(P-523 / D-3)

表層が「簡単だと思ったが実は複雑だった」場合を、機構で回収する。
モデルが複雑さを正しく判断することに依存しない。

切り替えの起点は2つ。

| 起点 | 判定 |
|---|---|
| ステップ超過 | 表層ターンのステップ数が `harness.loop.max_steps_per_turn` に到達した |
| 副作用の要求 | モデルが、表層に公開されていない副作用ありツールを呼ぼうとした |

どちらの場合も、ターンを失敗させずに**委譲へ昇格**する。

1. その時点までのステップ結果を委譲ブリーフに畳む(何を調べ、何が分かったか)。
2. `delegate.start` 相当の委譲を起こす([delegation.md](delegation.md))。
3. 表層のターンは「今調べているところ」を伝えて閉じる。
   ユーザーを待たせたまま黙らない。
4. 完了は通常の報告ターンとして戻る(P-521)。

副作用の要求が起点の場合、そのツール呼び出し自体は表層では**実行しない**。
表層で実行してから昇格するのでは、境界を設けた意味がなくなる。

昇格は表層UIには出ない。ユーザーから見えるのは、コンパニオンが
「ちょっと調べてくるね」と言って、後で答えを持ってくることだけである。

## 3. inbox と claim

入力はすべて soul 単位の inbox に届き、ループが **claim** して初めて
ステップの入力になる(dsh の inbox に倣う)。表層の inbox にユーザー発話と
層間メッセージが混在する。裏層の inbox には層間メッセージだけが入る。

| 入力種別 | 即時性 | 説明 |
|---|---|---|
| `wake` | 即時 | ユーザー発話(text/voice 確定)、承認応答、ask-user 回答、**委譲の質問・完了・失敗報告**([delegation.md §5](delegation.md#5-子--親メールボックス))。ループを即起こす |
| `inject` | 待機 | 注入コンテキスト(例: スケジュール発火の前置き、**委譲の progress / 表層からの `delegate.message`**)。次の wake まで待機 |
| `interrupt` | 即時 | 音声割り込み信号(バグイン)。実行中ステップを中断させる |

claim の規則: 1回の claim で「次のステップ入力1件 + キューされた wake 1件」
を取得。`inject` 単独ではターンを起こさない。
**wake の優先順位は ユーザー由来 > 委譲由来**(会話が委譲報告で
遅延しない、[delegation.md §5](delegation.md#5-子--親メールボックス))。

レーンコマンドとの対応([lane-api.md](lane-api.md)):

| コマンド | inbox | 備考 |
|---|---|---|
| `prompt` | `wake`(新操作) | アイドル時のみ |
| `follow_up` | `wake`(現操作の次ターン) | 実行中の操作にのみ |
| `steer` | `inject` | 進行中ターンの次 claim に載せる。**生成は割らない**。音声 `interrupt` ではない |
| `abort()` / 音声バグイン | `interrupt` | 実行中ステップを中断([durability.md §6](durability.md#6-アボート)は後継。v1.0 は [§10](#10-キャンセルと音声割り込みp-103)) |
| ask-user 回答 | `wake` | **同一プロセス内**では同一ターンの保留中ツールを続ける。`next_run` は使わない。クラッシュ後は §12 |
| `next_run` | 操作終端後の予約 | 承認の遅延応答など、**今の操作の後**にだけ使う |

v1.0 のキュー永続は [session-log.md §3.7](session-log.md#37-inboxv10-のキュー永続d-5)
のログイベント。後継の `pending.entry` は [operations.md §7](operations.md#7-inbox-とキューの耐久化)。

## 4. ターン/ステップ状態機械

```text
turn/start
 ├─ inbox から wake(+待機 inject)を claim
 ├─ 境界処理: 入力昇格 → ツール確定 → コンテキスト変更取り込み
 ├─ pre-step(§6 の waterfall で拒否/書き換え可能)
 │    └─ 拒否、または書き換え結果が空 → ステップなしで turn/end(outcome: completed)
 ├─ step/start
 │    ├─ System Context + Session History を組立 → モデル要求
 │    ├─ ストリーミング: text delta / inner delta / tool_call
 │    ├─ ツール実行(§5 のパイプライン)
 │    └─ step/end(outcome: next | stop | error)
 ├─ next かつ (未消化の tool_result がある or inbox に wake) → 次ステップへ
 ├─ stop → turn/end(outcome: completed)
 ├─ error → §8 の回復 → turn/end(outcome: failed|completed)
 └─ interrupt → 中断確定 → turn/end(outcome: interrupted)
```

- ターンは**0ステップで閉じ得る**(pre-step 拒否)。試行自体はログに残る。
- ステップ上限は層で違う(§2.1)。**到達時の扱いも違う**:
  裏層は打ち切り(`outcome: failed`, error_class=`step_budget`)だが、
  **表層は打ち切らず、委譲へ昇格する**(§2.3)。表層の上限は
  「これ以上は複雑だ」の検出器であって、失敗の条件ではない。
- 現在位置(ステップ間・生成中・ツール実行中・外部待ち)は進行中の操作として
  記録され、再起動時の中断検出に使う(§12)。v1.0 ではここまでで、
  各位置からの**重複効果なしの再開**は後継設計に属する
  ([operations.md](operations.md)、[durability.md](durability.md)、P-525)。

## 5. ツール実行の駆動

- ツール呼び出しは [../tools/registry.md](../tools/registry.md) の
  パイプライン(pre-execute → execute → post-execute)に委譲する。
- **並行ツール呼び出し**: モデルが1ステップで複数ツールを呼んだ場合、
  `is_concurrency_safe(args) == true` の呼び出し同士は並行実行する
  (dsh に倣う)。safe でない呼び出しは順次実行。
- ツールが ask-user を要求した場合、ターンは**中断せず**保留状態になる
  (§9)。ログには `tool/call` のみ記録し、回答後に継続。
- ツールが plan 承認を要求した場合も同様に保留(§9)。

## 6. pre-step と waterfall イベント(P-1007)

イベントは2モード(dsh に倣う):

- **waterfall**: リスナーが処理を横取り・書き換えできる。必ず `next()` を
  呼んで委譲する。ループの拡張点。
- **emit**: 通知のみ。ログの永続イベントと、ライブバスの非永続イベントがある。

ループが公開する waterfall:

| イベント | 役割 |
|---|---|
| `agent/pre-step` | claim 済み入力の書き換え・拒否。感情エンジン・quiet hours・承認 plane がリスナー |
| `agent/request` | モデル要求の最終整形(システムメッセージ添付、メタデータ付与) |
| `llm/stream` | プロバイダストリームの横取り(ローカルキャッシュ・ログ録等) |
| `tools/pre-execute` / `tools/post-execute` | ツール前後のフック(承認・guard・spill) |

`agent/turn-stopping`(serial、`next()` なし): ターンを止める最終機会。
quiet hours 進入・疲労限界が使う。

## 7. 秘匿サブエージェント(P-508)

サブエージェントは **裏層ハーネス内部の機構**であり、ユーザーのイベント・UI から
完全に秘匿する。**同期ツールではなく非同期委譲**である——表層は待たず、
会話は止まらない(P-519)。

- 実体・モード(internal/public)・表層↔裏層メッセージ・完了報告ターン・
  予算とガードは [delegation.md](delegation.md) が定義する。
  この節はループとの接点のみを置く。
- ループ接点: 表層の `delegate.start` はツール呼び出しとして
  §5 のパイプラインを通過し、**即座に受理確認を返す**。
  裏層の完了・質問は §3 の inbox に `wake`/`inject` として届き、
  報告ターンは `origin: subagent` で §4 の状態機械を回る。
- 秘匿の実体(何がログに残り、何がユーザーに出ないか)は
  [visibility.md](visibility.md) が定義する。

## 8. ガードとエラー回復

### guard(P-509)

| ガード | 規則 |
|---|---|
| 繰り返し呼び出し検知 | 同一ツール+同一引数ハッシュの連続3回で、`tools/post-execute` が「同じ呼び出しを繰り返している」リマインダーを次ステップに注入 |
| 実行期限 | `tools/execute` waterfall がツールごとの `timeout_ms` を強制。超過は `cancelled`(error_class=`timeout`) |
| 出力肥大 | post-execute が spill 判定([context-assembly.md §5](context-assembly.md#5-spill)) |
| ステップ予算 | §4 の `max_steps_per_turn` |

### エラー分類(P-510)

| error_class | 意味 | 回復 |
|---|---|---|
| `provider_overload` | 429/5xx 一時的 | 指数バックオフで再試行 |
| `provider_auth` | 認証失敗 | 再試行せずターン失敗+設定確認を促す発話 |
| `context_overflow` | 窓超過 | compaction を即時実行して1回再試行 |
| `timeout` | ツール/要求の期限超過 | ツールは `cancelled` で継続。要求側はステップ打ち切り |
| `log_write` | ログ書き込み失敗 | ターン失敗(データ保全のため継続しない) |
| `plugin_dead` | プラグイン喪失 | [../platform/process-model.md](../platform/process-model.md) の監督に委譲。再起動待ち後に1回再試行 |

- 回復不能の失敗は、**対話的に**報告する(「うまくいかなかった」発話+
  `turn/end{outcome: failed}`)。沈黙のまま失敗しない。
- **ツール呼び出しの失敗と、プロセスの死は別物である。** 前者はこの表の
  分類に従い、失敗として報告すれば足りる。後者はターンが「途中で消えた」
  状態を残すので、起動時の中断検出(§12)が処理する。

## 9. 人間協調面(plan / ask-user)(P-511, P-512)

- **ask-user**: ツール実行中に質問が発生、またはモデルが `question.ask` を呼ぶ
  ([../tools/registry.md §0.1](../tools/registry.md#01-ハーネス機能ツールホスト内))。
  ターンは中断せず保留になり、
  ログに `question/asked` を書く。**対話レーンではキャラ発話として
  質問する**(ポップアップにしない)。同一プロセス内では、回答は inbox の
  `wake` として届き、保留中のツール呼び出しを続ける。`next_run` は使わない。
  タイムアウト(既定 24 時間、`harness.ask_user.timeout`)で `cancelled`。
  プロセスが死んだあとの復元は §12(見せるが実行は再開しない)。
  後継の `op.state deferred` は [operations.md](operations.md)。
- **plan / 承認**: 変更を伴う作業(fs 書き込み・exec・送信系)の前に、
  モードが `plan_required` のツールはまず計画を提示して承認を待つ。
  **ポップアップは承認 plane に限る**
  ([../security/approval.md](../security/approval.md))。
  拒否はツール結果 `denied` としてモデルに返る(対話は継続)。

## 10. キャンセルと音声割り込み(P-103)

- **キャンセル**: `cancel(turn_id)`。モデル要求は即破棄、実行中ツールには
  協調的キャンセル信号(`signal` を伝播)。確定済みのツール結果はログに残る。
- **音声割り込み(バグイン)**: 発話の TTS 再生中に VAD がユーザー発話を
  検知 → TTS 再生を即停止 → inbox に `interrupt` → 実行中ステップを
  キャンセル → ターンを `interrupted` で確定(中断位置までの発話は履歴に残る)
  → 新たな発話を `user/message` として新ターン開始。
  詳細な音声側は [../body/voice.md](../body/voice.md)。

## 11. 設定キー

具体的な数値は実装しながら決める(D-29)。ここでは何を設定できるかだけを定める。

| キー | 説明 |
|---|---|
| `harness.delegation.max_active` | job レーン(委譲)並行上限(soul ごと)。[delegation.md](delegation.md) と同一キー |
| `harness.loop.max_steps_per_turn` | 表層対話レーンのステップ上限。到達は失敗ではなく**委譲への昇格**を起こす(§2.3) |
| `harness.delegation.step_budget` | 裏層のステップ予算(委譲全体)。到達は打ち切り |
| `harness.loop.repeat_call_threshold` | 同一呼び出しの繰り返し検知閾値 |
| `harness.ask_user.timeout` | ask-user タイムアウト |
| `harness.delegation.report_gates` | 報告ターンに quiet hours/疲労ゲートを適用するか |
| `harness.retry.backoff_ms` | `provider_overload` 再試行のバックオフ列 |

## 12. 中断の検出と報告(P-515 / D-5)

v1.0 のクラッシュ耐性はここに閉じる。**重複効果なしの再開は保証しない。**
保証するのは「途中だったことをなかったことにしない」ことである。

起動時に、前回の実行で閉じていないターンとジョブを走査する。
`turn/start` があって対応する `turn/end` がない、あるいはジョブが
実行中のまま残っている——これが中断の定義である。
同時に、未 claim の inbox を [session-log.md §3.7](session-log.md#37-inboxv10-のキュー永続d-5)
から復元する(ユーザー発話、委譲の報告・質問、ask-user の保留)。

検出したものに対して、順に行う。

1. **後始末**: 進行中だったツール呼び出しに対応する子プロセス・一時ファイル・
   予約された workspace を解放する。プロセスは既に死んでいるので、
   残っているのは孤児資源だけである。
2. **確定**: 中断されたターンを `turn/end{outcome: interrupted}` で閉じ、
   ジョブを中断状態で確定する。開いたままにしない。
   未 claim の inbox は `inbox/cancelled{reason: abandoned_interrupt}` で閉じる。
   それらを次のターンの入力として claim してはならない。
3. **報告**: 次にユーザーが現れたとき、コンパニオンが中断を伝える。
   「さっきの調べ物、途中で止まっちゃった」に相当する発話であり、
   システム通知やエラーダイアログではない(D-13)。
   復元した未消化項目(言いかけの発話、届いていなかった完了報告、
   未回答の質問)は、この報告の材料として見せる。

実行の続きはしない。中断されたジョブをもう一度走らせるかはユーザーが決める。
自動で再開すると、既に一度実行された副作用がもう一度起きうるためで、
それを安全にするのが後継設計の effect sandwich である(P-525)。

## 13. 障害モード

| 障害 | 挙動 |
|---|---|
| pre-step で全リスナーが拒否 | ステップなしでターン終了。ログに試行は残る。対話的には何も発話しない(拒否の理由がある場合は注入で説明) |
| ask-user 中にアプリ終了 | `question/asked` はログに残る。起動時は §12 で中断として確定し、質問は「途中だった」と見せる。保留中ツールは再開しない。タイムアウトは操作が死ぬので数えない |
| interrupt の連打 | 割り込み確定中は次の interrupt を無視(短いデバウンス) |
| サブエージェントが予算超過 | 子ターンを打ち切り、`failed{budget}` の報告が親の inbox に届く。親は継続可能([delegation.md §9](delegation.md#9-予算とガード)) |
| 報告 wake が溜まったままユーザーが離席 | 同一プロセス内ではユーザー wake が来ない限り報告ターンは起きない。ログに残るのでプロセス生存中は失われない。プロセスが死んだあとは §12(見せるが報告ターンは起こさない) |
| 対話レーンと job レーンの承認競合 | 承認 plane がキューイング([../security/approval.md](../security/approval.md)) |
| 表層で副作用ツールの昇格が連続する | 昇格はターンごとに1回。同一ターンで再度起きたら、そのターンは委譲済みとして閉じる(昇格ループを作らない) |
| プロセスが強制終了された | §12 の検出と報告。再開はしない |

---

- 前: [context-assembly.md](context-assembly.md) / 次: [delegation.md](delegation.md)
