# エージェントループ

> 実現する要件: **P-503**(turn/step)、**P-504**(対話1+job 並行)、
> **P-508**(秘匿サブエージェント)、**P-509**(guard)、**P-510**(エラー回復)、
> **P-511**(plan)、**P-512**(ask-user)、P-103(割り込み)。

## 1. 責務と責務外

- **責務**: ターン/ステップの状態機械、入力キュー(inbox)、キャンセルと
  音声割り込み、ガード、エラー回復、サブエージェント委譲、plan/ask-user。
- **責務外**: コンテキスト組立([context-assembly.md](context-assembly.md))、
  ログの語彙([session-log.md](session-log.md))、ツール実行そのもの
  ([../tools/registry.md](../tools/registry.md))。この文書の状態機械が
  クラッシュ後も再開できるための**耐久状態**は
  [operations.md](operations.md)(`op.state` プログラムカウンタ)と
  [durability.md](durability.md)(効果のサンドウィッチと回復)が定義する。
  この文書は正常系の振る舞いを定義する。

## 2. レーン(P-504)

soul は2種のレーンを持つ。

| レーン | 本数 | 並行性 | ターンの origin |
|---|---|---|---|
| 対話レーン | 1 | **単一フライト**。先行ターン終了まで次は受け付けない | `user`, `proactive`, `scheduled` |
| job レーン | 複数(上限 `harness.delegation.max_active`: 既定 4) | 並行 | `delegation`(委譲の子ターン。[delegation.md](delegation.md)) |

- 対話レーンが塞がっているときのユーザー発話は **inbox** に入る(§3)。
- job レーンのターンは対話レーンに影響しない(逆も同じ)。
  ただし排他資源(音声出力・承認応答)は platform 層が調停する。
- 対話レーンのターンにはさらに `subagent` origin の**報告ターン**がある
  (委譲の完了・質問が inbox を経て届く、[delegation.md §6](delegation.md#6-報告ターンp-521))。

## 3. inbox と claim

入力はすべて soul 単位の inbox に届き、ループが **claim** して初めて
ステップの入力になる(dsh の inbox に倣う)。

| 入力種別 | 即時性 | 説明 |
|---|---|---|
| `wake` | 即時 | ユーザー発話(text/voice 確定)、承認応答、ask-user 回答、**委譲の質問・完了・失敗報告**([delegation.md §5](delegation.md#5-子--親メールボックス))。ループを即起こす |
| `inject` | 待機 | 注入コンテキスト(例: スケジュール発火の前置き、**委譲の progress**)。次の wake まで待機 |
| `interrupt` | 即時 | 音声割り込み信号(バグイン)。実行中ステップを中断させる |

claim の規則: 1回の claim で「次のステップ入力1件 + キューされた wake 1件」
を取得。`inject` 単独ではターンを起こさない。
**wake の優先順位は ユーザー由来 > 委譲由来**(会話が委譲報告で
遅延しない、[delegation.md §5](delegation.md#5-子--親メールボックス))。

inbox とキュー項目の永続化(1項目 = 予約 entry id、steer/followup の
2キュー、キューモード)は [operations.md §7](operations.md#7-inbox-とキューの耐久化) が定義する。

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
- ステップ上限 `harness.loop.max_steps_per_turn`: 既定 32。到達で打ち切り
  (`outcome: failed`, error_class=`step_budget`)。
- 状態機械の各位置(ステップ間・生成中・ツール実行中・外部待ち)は、
  [operations.md](operations.md) の `op.state` レジスタに total な形で
  耐久化される。どの位置でクラッシュしてもそこから再開できる
  ([durability.md](durability.md)、P-515)。

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

サブエージェントは **soul 内部の機構**であり、ユーザーのイベント・UI から
完全に秘匿する。**同期ツールではなく非同期委譲**である——親は待たず、
会話は止まらない(P-519)。

- 実体・モード(internal/public)・親↔子メッセージ・完了報告ターン・
  予算とガードは [delegation.md](delegation.md) が定義する。
  この節はループとの接点のみを置く。
- ループ接点: 親の `delegate.start` はツール呼び出しとして
  §5 のパイプラインを通過し、**即座に受理確認を返す**。
  子の完了・質問は §3 の inbox に `wake`/`inject` として届き、
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
| `provider_overload` | 429/5xx 一時的 | 指数バックオフで3回再試行(0.5/2/8 秒) |
| `provider_auth` | 認証失敗 | 再試行せずターン失敗+設定確認を促す発話 |
| `context_overflow` | 窓超過 | compaction を即時実行して1回再試行 |
| `timeout` | ツール/要求の期限超過 | ツールは `cancelled` で継続。要求側はステップ打ち切り |
| `log_write` | ログ書き込み失敗 | ターン失敗(データ保全のため継続しない) |
| `plugin_dead` | プラグイン喪失 | [../platform/process-model.md](../platform/process-model.md) の監督に委譲。再起動待ち後に1回再試行 |

- 回復不能の失敗は、**対話的に**報告する(「うまくいかなかった」発話+
  `turn/end{outcome: failed}`)。沈黙のまま失敗しない。
- **プロセス死はエラー分類の外**: 上記のいずれにも当てはまらない失敗
  (プロセス全体の死)は、起動時の回復が処理する
  ([durability.md §3](durability.md#3-回復ポリシー))。

## 9. 人間協調面(plan / ask-user)(P-511, P-512)

- **ask-user**: ツール実行中に質問が発生。ターンは `awaiting_user` になり、
  質問を `question/asked` イベントで公開(承認 plane と同じポップアップ経路)。
  回答は inbox の `wake` として届き、保留中のツール呼び出しを再開。
  タイムアウト(既定 24 時間、`harness.ask_user.timeout`)で `cancelled`。
- **plan**: 変更を伴う作業(fs 書き込み・exec・送信系)の前に、
  モードが `plan_required` のツールはまず計画を提示して承認を待つ。
  承認 plane のポップアップと同じ UI 面を使う。承認は
  [../security/approval.md](../security/approval.md) のポリシーに従う。
  拒否はツール結果 `denied` としてモデルに返る(対話は継続)。

## 10. キャンセルと音声割り込み(P-103)

- **キャンセル**: `cancel(turn_id)`。モデル要求は即破棄、実行中ツールには
  協調的キャンセル信号(`signal` を伝播)。確定済みのツール結果はログに残る。
- **音声割り込み(バグイン)**: 発話の TTS 再生中に VAD がユーザー発話を
  検知 → TTS 再生を即停止 → inbox に `interrupt` → 実行中ステップを
  キャンセル → ターンを `interrupted` で確定(中断位置までの発話は履歴に残る)
  → 新たな発話を `user/message` として新ターン開始。
  詳細な音声側は [../body/voice.md](../body/voice.md)。

## 11. 設定キーと既定値

| キー | 既定 | 説明 |
|---|---|---|
| `harness.delegation.max_active` | `4` | job レーン(委譲)並行上限(soul ごと)。[delegation.md](delegation.md) と同一キー |
| `harness.loop.max_steps_per_turn` | `32` | ステップ上限 |
| `harness.loop.repeat_call_threshold` | `3` | 繰り返し検知閾値 |
| `harness.ask_user.timeout` | `24h` | ask-user タイムアウト |
| `harness.delegation.report_gates` | `true` | 報告ターンに quiet hours/疲労ゲートを適用([delegation.md §10](delegation.md#10-設定キーと既定値)) |
| `harness.retry.backoff_ms` | `[500, 2000, 8000]` | provider_overload 再試行 |

## 12. 障害モード

| 障害 | 挙動 |
|---|---|
| pre-step で全リスナーが拒否 | ステップなしでターン終了。ログに試行は残る。対話的には何も発話しない(拒否の理由がある場合は注入で説明) |
| ask-user 中にアプリ終了 | 保留状態はログから復元。次回起動で再開(タイムアウトは継続して数える) |
| interrupt の連打 | 割り込み確定中は次の interrupt を無視(デバウンス 300ms) |
| サブエージェントが予算超過 | 子ターンを打ち切り、`failed{budget}` の報告が親の inbox に届く。親は継続可能([delegation.md §9](delegation.md#9-予算とガード)) |
| 報告 wake が溜まったままユーザーが離席 | ユーザー wake が来ない限り報告ターンは起きない。`pending.entry` で耐久化されており、復帰後に結合して届く |
| 対話レーンと job レーンの承認競合 | 承認 plane がキューイング([../security/approval.md](../security/approval.md)) |

---

- 前: [context-assembly.md](context-assembly.md) / 次: [delegation.md](delegation.md)
