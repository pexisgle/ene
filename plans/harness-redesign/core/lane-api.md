# レーンコマンド面(公開 API)

> 実現する要件: **P-101**(対話のコマンド面)、**P-503/P-504**(ターン/レーンの操作面)、
> **P-515**(中断の検出と報告)、P-103(アボート)、**P-506**(compaction の起動)。
> 参照: pi Part 5(lane surface)の結果/エラーモデルを Ene のレーン語彙に適合させる。
>
> **段階**: この文書は v1.0 と後継が同居する。[operations.md](operations.md) /
> [durability.md](durability.md) と同じ切り方。v1.0 は §2 の v1.0 行と §4.1。
> `resume()` / `get_last_result()` と §4.2 の照合は後継(P-525、D-4)。

[agent-loop.md](agent-loop.md) はループの**振る舞い**、
[operations.md](operations.md)/[durability.md](durability.md) は
**後継の耐久状態**を定義する。この文書は、上位層(platform・tasks・companion)が
ループを駆動する**コマンドの面**を定義する。
[../platform/server-api.md](../platform/server-api.md) と
[../platform/clients.md](../platform/clients.md) はこの面以外から
ループを駆動してはならない。

v1.0 のクラッシュ耐性は [agent-loop.md §12](agent-loop.md#12-中断の検出と報告p-515--d-5)
に閉じる。コマンド面は中断を「なかったこと」にしないための入口であり、
`op.state` や `lane.last_result` による再開・照合は持たない。

## 1. 責務と責務外

- **責務(v1.0)**: コマンドの語彙と受理/拒否の契約、結果とエラーの分類、
  HTTP/WS エンドポイントとの対応。
- **責務(後継)**: クラッシュ後の結果照合(`op.state` / `lane.last_result`)、
  保留中操作の `resume()`。
- **責務外**: コマンドが受理された後の状態遷移(operations.md)、
  回復(durability.md)、輸送と認証(server-api.md)。
  v1.0 の中断検出そのものは agent-loop.md §12。

## 2. コマンド一覧

全コマンドは soul の**レーン単位**で発行する。対話レーン(表層)は1本、
job レーン(裏層)は job ごとに1本([agent-loop.md §2](agent-loop.md#2-レーンp-504))。
ユーザー発話の `prompt` は対話レーンにだけ届く。裏層をクライアントから
直接 `prompt` してはならない。

| コマンド | 段階 | 対象 | 効果 | 拒否条件 |
|---|---|---|---|---|
| `prompt(message)` | v1.0 | レーン | ターンを1つ起こす | `LaneBusy`, `InvalidMessage`, `Closed` |
| `steer(message)` | v1.0 | 実行中の操作 | 進行中ターンへの **inject**(次 claim に載せる。生成は割らない) | `NoActiveOperation`, `InvalidMessage` |
| `follow_up(message)` | v1.0 | 実行中の操作 | 次のターンで捌く発話をキュー(inbox の `wake`) | 同上 |
| `next_run(message)` | v1.0 | レーン | 現操作の**終端後に**受理される入力を予約。承認の遅延応答など、操作が終わってから拾うものだけ。ask-user 回答は使わない | `InvalidMessage` |
| `cancel_queued(entry_id)` | v1.0 | キュー項目 | 予約 entry を取消。3分岐 `cancelled / already_consumed / not_found`([storage-model.md §3.2](storage-model.md#32-書き込みの規律)は後継のレジスタ表現。v1.0 はログ上のキュー項目で同じ3分岐) | —(`not_found` は成功扱い) |
| `abort()` | v1.0 | 実行中の操作 | 進行中ターンを取り消す。[agent-loop.md §10](agent-loop.md#10-キャンセルと音声割り込みp-103)。後継では control を `cancel_requested` にする([durability.md §6](durability.md#6-アボート)) | `NoActiveOperation` |
| `compact(instructions?)` | v1.0 | セッション | compaction を起こす([context-assembly.md §7](context-assembly.md#7-compactionp-506))。後継の耐久状態は [operations.md §5](operations.md#5-compactionstate構造変更) | `LaneBusy`, `NothingToCompact` |
| `record_usage(usage)` | v1.0 | レーン | 対話外 LLM 呼び出し(記憶抽出等)の台帳行を追記([storage-model.md §4](storage-model.md#4-usage-ledgerコスト台帳)) | — |
| `wait_for_idle()` | v1.0 | レーン | 受理済みで未決の仕事がすべて片付くまで待つ | —(close で拒否) |
| `run_when_idle(callback)` | v1.0 | レーン | アイドル確定後に低優先作業を1つ実行(compaction 先回り・内省) | — |
| `resume()` | 後継 | レーン | 回復不能に保留中の操作(deferred・クラッシュ残り)を前進 | `NothingToResume` |
| `get_last_result()` | 後継 | レーン | 直近の終端結果を読む(§4.2) | — |

受理されたコマンドは、結果が `failed`/`interrupted` であっても
**成功(Ok)として返る**。拒否(Err)は「コマンドが受理されなかった」
ことのみを意味する。ストレージ故障・close 中の競合は promise 拒否
(`Fault`/`Closed`)であり、タグ付きエラーとは別の階層。

- `steer`/`follow_up` は**実行中の操作にのみ**発行できる。アイドル時は
  `prompt` を使う(曖昧な中間状態を作らない)。
- `steer` は inbox の `inject` であり、音声 `interrupt` ではない
  ([agent-loop.md §3](agent-loop.md#3-inbox-と-claim))。
- `next_run` は「今の操作の後に」を意味する経路。承認の遅延応答が使う。
  ask-user 回答は同一操作への `wake`([agent-loop.md §9](agent-loop.md#9-人間協調面plan--ask-userp-511-p-512))。
- `abort()` の2回目以降は何も追記せず、同じ結果を返す。
  [agent-loop.md §10](agent-loop.md#10-キャンセルと音声割り込みp-103)
  の `cancel(turn_id)`・音声割り込みはこの `abort()` の呼び出しである。
  後継では drained ペイロードを返す([durability.md §6](durability.md#6-アボート))。
- `resume()` は v1.0 では存在しない。中断された仕事を再実行するかは
  ユーザーが決める([agent-loop.md §12](agent-loop.md#12-中断の検出と報告p-515--d-5))。

## 3. 結果の形

v1.0 では、操作の結果はターン/ジョブの終端イベント
(`turn/end` 等)がログに載った時点で解決する。
終端トランザクション([operations.md §6](operations.md#6-終端トランザクション))は後継。

| kind | 段階 | 結果のバリアント |
|---|---|---|
| `run` | v1.0 | `completed / interrupted / cancelled / failed(error_class)` + 最終応答(あれば) |
| `compaction` | v1.0 | `completed(要約 entry 参照) / cancelled / declined / failed` |
| `resume` | 後継 | 再開した操作の種類と、その結果 |

- 最終応答フィールド(entry id + メッセージ)は「あるか両方無いか」の
  二者択一。全ツール結果が terminate の完了 run は最終応答を持たない。
- `failed` は必ず [agent-loop.md §8](agent-loop.md#8-ガードとエラー回復) の
  `error_class` を伴う。

## 4. 中断と結果照合

### 4.1 v1.0(P-515)

プロセスが死ぬと、呼び出し側は進行中ターンの結果を受け取れない。
v1.0 が保証するのは次だけである(D-5)。

1. 起動時に、閉じていないターン/ジョブを検出する。
2. 未 claim の inbox をログから復元する。
3. 孤児資源を後始末し、中断としてログを閉じる
   (未 claim inbox は `abandoned_interrupt`。claim して実行しない)。
4. 次にユーザーが現れたとき、コンパニオンが中断を報告する。
   復元した未消化項目はその報告の材料になる。

手順の本体は [agent-loop.md §12](agent-loop.md#12-中断の検出と報告p-515--d-5)。
`resume()` は呼ばない。`op.state` も `lane.last_result` も読まない
(どちらも後継のレジスタであり、v1.0 では存在しない)。

### 4.2 後継(P-525)

操作を受理した直後にプロセスが死ぬと、呼び出し側は promise の解決を
受け取れない。照合経路は**2つだけ**:

1. `op.state` がまだ存在する → 操作は未終端。`resume()` で前進できる。
2. `op.state` が消えている → 終端済み。`lane.last_result` から結果を読む。

`lane.last_result` は回復の判断には**決して使わない**
([storage-model.md §3.2](storage-model.md#32-書き込みの規律))。
これは「promise を失った呼び出し側」専用の帳尻合わせである。
外部から終端された操作([durability.md §5](durability.md#5-外部終端))の
結果を知る手段もこれである。

## 5. HTTP/WS との対応

[../platform/server-api.md](../platform/server-api.md) のエンドポイントは
この面の射影である。

| コマンド | 段階 | エンドポイント |
|---|---|---|
| `prompt` | v1.0 | `POST /sessions/{id}/messages` |
| `steer` / `follow_up` | v1.0 | `POST /sessions/{id}/messages`(モード指定 `steer\|follow_up`) |
| `cancel_queued` | v1.0 | `DELETE /sessions/{id}/queued/{entry_id}` |
| `abort` | v1.0 | `POST /turns/{id}/cancel` |
| `compact` | v1.0 | `POST /sessions/{id}/compact` |
| `resume` | 後継 | 起動時にコアが自動実行。手動は `POST /sessions/{id}/resume` |
| `get_last_result` | 後継 | `GET /sessions/{id}` に含める。v1.0 の同 GET はセッション要約のみで、このフィールドは持たない |

- 送信系の `Idempotency-Key` は受理の重複排除に使う(同一キーの受理は2度起きない)。
- エラーレスポンスの `error_class` は §2 のタグと一致
  (`lane_busy` / `no_active_operation` / …)。

## 6. 障害モード

| 障害 | 段階 | 挙動 |
|---|---|---|
| 2クライアントが同時に `prompt` | v1.0 | 一方が受理、他方は `LaneBusy`(I-9)。`LaneBusy` には進行中の操作 id が載るので、クライアントは照合できる |
| close 中に受理されたコマンド | v1.0 | `Closed` を返す。v1.0 の耐久状態はログの接頭辞のまま。後継の close は [durability.md §7](durability.md#7-close--制御されたクラッシュ) |
| `wait_for_idle` 待ち中に fault | v1.0 | 全ウェイターが拒否で解決 |
| 失われた `cancel_queued` の再送 | v1.0 | `not_found` を成功として扱う(再試行安全) |
| `run_when_idle` のコールバックが同レーンの状態変更を呼ぶ | v1.0 | 自身の予約の後ろでデッドロックする。**禁止**(検証は型で) |
| クラッシュ直後の `get_last_result` / `resume` | 後継 | §4.2。v1.0 ではこれらのコマンド自体が無い |

---

- 前: [invariants.md](invariants.md) / 次: [observability.md](observability.md)
