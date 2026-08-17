# レーンコマンド面(公開 API)

> 実現する要件: **P-101**(対話のコマンド面)、**P-503/P-504**(ターン/レーンの操作面)、
> **P-515**(クラッシュ後の結果照合面)、P-103(アボート)。
> 参照: pi Part 5(lane surface)の結果/エラーモデルを Ene のレーン語彙に適合させる。

[agent-loop.md](agent-loop.md) はループの**振る舞い**、
[operations.md](operations.md)/[durability.md](durability.md) は
**耐久状態**を定義する。この文書は、上位層(platform・tasks・companion)が
ループを駆動する**コマンドの面**を定義する。
[../platform/server-api.md](../platform/server-api.md) と
[../platform/clients.md](../platform/clients.md) はこの面以外から
ループを駆動してはならない。

## 1. 責務と責務外

- **責務**: コマンドの語彙と受理/拒否の契約、結果とエラーの分類、
  クラッシュ後の結果照合、HTTP/WS エンドポイントとの対応。
- **責務外**: コマンドが受理された後の状態遷移(operations.md)、
  回復(durability.md)、輸送と認証(server-api.md)。

## 2. コマンド一覧

全コマンドは soul の**レーン単位**で発行する。対話レーン(表層)は1本、
job レーン(裏層)は job ごとに1本([agent-loop.md §2](agent-loop.md#2-レーンp-504))。
ユーザー発話の `prompt` は対話レーンにだけ届く。裏層をクライアントから
直接 `prompt` してはならない。

| コマンド | 対象 | 効果 | 拒否条件 |
|---|---|---|---|
| `prompt(message)` | レーン | ターンを1つ起こす(受理トランザクション、[operations.md §1](operations.md#1-操作operationとは)) | `LaneBusy`, `InvalidMessage`, `Closed` |
| `steer(message)` | 実行中の操作 | 進行中ターンへの **inject**(次 claim に載せる。生成は割らない) | `NoActiveOperation`, `InvalidMessage` |
| `follow_up(message)` | 実行中の操作 | 次のターンで捌く発話をキュー(inbox の `wake`) | 同上 |
| `next_run(message)` | レーン | 現操作の**終端後に**受理される入力を予約。承認の遅延応答など、操作が終わってから拾うものだけ。ask-user 回答は使わない | `InvalidMessage` |
| `cancel_queued(entry_id)` | キュー項目 | 予約 entry を取消。3分岐 `cancelled / already_consumed / not_found`([storage-model.md §3.2](storage-model.md#32-書き込みの規律)) | —(`not_found` は成功扱い) |
| `abort()` | 実行中の操作 | control を `cancel_requested` に([durability.md §6](durability.md#6-アボート))。drain した項目のペイロードを返す | `NoActiveOperation` |
| `resume()` | レーン | 回復不能に保留中の操作(deferred・クラッシュ残り)を前進 | `NothingToResume` |
| `compact(instructions?)` | セッション | compaction 操作を起こす([operations.md §5](operations.md#5-compactionstate構造変更)) | `LaneBusy`, `NothingToCompact` |
| `record_usage(usage)` | レーン | 対話外 LLM 呼び出し(記憶抽出等)の台帳行を追記([storage-model.md §4](storage-model.md#4-usage-ledgerコスト台帳)) | — |
| `wait_for_idle()` | レーン | 受理済みで未決の仕事がすべて片付くまで待つ | —(close で拒否) |
| `run_when_idle(callback)` | レーン | アイドル確定後に低優先作業を1つ実行(compaction 先回り・内省) | — |
| `get_last_result()` | レーン | 直近の終端結果を読む(§4) | — |

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
- `abort()` の2回目以降は何も追記せず、同じ drained ペイロードを返す
  ([durability.md §6](durability.md#6-アボート))。[agent-loop.md §10](agent-loop.md#10-キャンセルと音声割り込みp-103)
  の `cancel(turn_id)`・音声割り込みはこの `abort()` の呼び出しである。

## 3. 結果の形

操作の結果は、終端トランザクション([operations.md §6](operations.md#6-終端トランザクション))が
確定した時点で解決する。

| kind | 結果のバリアント |
|---|---|
| `run` | `completed / interrupted / cancelled / failed(error_class)` + 最終応答(あれば) |
| `compaction` | `completed(要約 entry 参照) / cancelled / declined / failed` |
| `resume` | 再開した操作の種類と、その結果 |

- 最終応答フィールド(entry id + メッセージ)は「あるか両方無いか」の
  二者択一。全ツール結果が terminate の完了 run は最終応答を持たない。
- `failed` は必ず [agent-loop.md §8](agent-loop.md#8-ガードとエラー回復) の
  `error_class` を伴う。

## 4. クラッシュ後の結果照合(P-515)

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

| コマンド | エンドポイント |
|---|---|
| `prompt` | `POST /sessions/{id}/messages` |
| `steer` / `follow_up` | `POST /sessions/{id}/messages`(モード指定 `steer\|follow_up`) |
| `cancel_queued` | `DELETE /sessions/{id}/queued/{entry_id}` |
| `abort` | `POST /turns/{id}/cancel` |
| `resume` | 起動時にコアが自動実行。手動は `POST /sessions/{id}/resume` |
| `compact` | `POST /sessions/{id}/compact` |
| `get_last_result` | `GET /sessions/{id}` に含める |

- 送信系の `Idempotency-Key` は受理トランザクションの
  重複排除に使う(同一キーの受理は2度起きない)。
- エラーレスポンスの `error_class` は §2 のタグと一致
  (`lane_busy` / `no_active_operation` / …)。

## 6. 障害モード

| 障害 | 挙動 |
|---|---|
| 2クライアントが同時に `prompt` | 一方が受理、他方は `LaneBusy`(I-9)。`LaneBusy` には進行中の操作 id が載るので、クライアントは照合できる |
| close 中に受理されたコマンド | `Closed` を返す。耐久状態はコミット済み接頭辞のまま([durability.md §7](durability.md#7-close--制御されたクラッシュ)) |
| `wait_for_idle` 待ち中に fault | 全ウェイターが拒否で解決 |
| 失われた `cancel_queued` の再送 | `not_found` を成功として扱う(再試行安全) |
| `run_when_idle` のコールバックが同レーンの状態変更を呼ぶ | 自身の予約の後ろでデッドロックする。**禁止**(検証は型で) |

---

- 前: [invariants.md](invariants.md) / 次: [observability.md](observability.md)
