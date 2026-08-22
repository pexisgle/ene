# スケジュール

スケジュールは `ene-work` の永続的な時間指定アクションです。クライアント
ではなくワーク DB に置くので、プロセス再起動後も残ります。

## spec と action

`spec` は cron 式（5 または 6 フィールド）で、`timezone` で評価します。
`action` は次のいずれかです:

| アクション | 意味 |
|---|---|
| `remind` | 表層でリマインドを話す |
| `turn` | 対話ターンを開始する |
| `job` | 裏層ジョブを起動する（`action_ref`）。デーモンのジョブレーンがツールを回します |

`important` なスケジュールは、静穏時間が発話を抑えていても発火します。
スケジュールの静穏時間は `mind.proactive.quiet_hours` と同じ開始・終了時
とタイムゾーンを使います。能動発話のゲートは使いません。期限の `remind`
は窓が終わるまで繰り下げ、`important` だけ貫通します。

## デーモンの駆動

`ene-core` は約 1 秒ごとに期限行を見ます。

- 起動時は先に `catch_up_missed`: 過ぎた `remind` は一度だけ発火。過ぎた
  `job` / `turn` は走らせず（D-5）、`next_fire` だけ進めます。
- その後は `fire_due`。`remind` はジョブの発話ゲート経由の
  `CompanionReport`（`it's time: …`）で、開いているセッションに入ります。
  `job` は公開デリゲーションを開始（`action_ref` または名前）。
  `turn` は開いているセッションがあるとき `TurnOrigin::Scheduled` の対話
  ターンを開始します。

## スケジュールの管理

`ene-ctl schedule list` が HTTP の一覧を出します。`ene-ctl schedule add` は
`spec` が 5 または 6 フィールドの cron であることと、`timezone` が IANA 名
（または `UTC`）であることを確認してから行を作ります。空白を含む `spec` は
引用してください。作成 / 更新 / 削除は `/api/v1/schedules` でもできます:

```sh
ene-ctl schedule list
ene-ctl schedule add <soul-id> morning "0 9 * * *" --timezone UTC
ene-ctl schedule add <soul-id> standup "0 30 9 * * 1-5" --timezone Asia/Tokyo --action remind
```

`PATCH` は `enabled` を切り替えます。発火はサーバ側です。クライアントが
ローカルタイマーをポーリングすることはありません。

約束の期限（`expires_at`）はスケジュールを自動では作りません。同じ soul
の既存行を約束の `schedule_id` に PATCH できます。その約束の完了・削除・
期限切れは、指名したスケジュールを無効にします。時間指定の Work
アクションが要るときは `remind` / `job` / `turn` 行を作ってください。
