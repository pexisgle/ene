# スケジュール

スケジュールは**永続的な時間指定アクション**です。コンパニオン（または
システム）が将来の時刻・間隔・cron 式で何かを実行します。スケジュールは
データベースに保存されるため、再起動しても維持されます。

## スケジュールの種類

| 種類 | 発火条件 | 例 |
|---|---|---|
| `one_shot` | `start_at` に 1 回 | 「10 分後にリマインド」 |
| `interval` | `start_at` を起点に `interval` 秒ごと | 「毎時間チェック」 |
| `cron` | `timezone` の 5/6 フィールド cron 式 | 「平日 09:00 Asia/Tokyo」 |
| `startup` | プロセス起動ごとに 1 回 | 「アプリ起動ごとに」 |

## スケジュールの管理

### CLI REPL

```sh
/schedule list
/schedule add water --kind one_shot --at "2026-08-07T09:00:00+09:00" --prompt "植物に水をやる時間です"
/schedule add check --kind interval --every 3600 --prompt "受信トレイを確認して"
/schedule add morning --kind cron --cron "0 9 * * 1-5" --tz "Asia/Tokyo" --prompt "おはようの要約を"
/schedule history <name>
/schedule pause <name>
/schedule resume <name>
/schedule delete <name>
```

各種類が要求するフィールドが正確に必要です。不正な cron 式・不正な
タイムゾーン・過去の `start_at` は作成時に拒否されます。`--tz <IANA ゾーン>`
（デフォルト: システムのローカルタイムゾーン。判定できない場合は `UTC` に
フォールバック）が cron 評価用のタイムゾーンです。単発・インターバルは絶対
時刻ベースのため、タイムゾーンは参照用に保存されるだけです。`--confirm` は
アクション開始前にユーザー確認を要求します（確認プロンプトは標準の権限
ダイアログを再利用し、未回答のまま `scheduler.confirmation_timeout_secs`
が経過するとタイムアウトします）。完全なオプションは
`/schedule add <name> --kind <one_shot|interval|cron|startup>`
`[--at <RFC3339> | --every <秒> | --cron <式>]`
`[--tool <name> --args <json> | --prompt <テキスト>] [--allow-tools]`
`[--tz <ゾーン>] [--confirm] [--retries <回>] [--retry-delay <秒>]` です。
スケジュールはツール呼び出し（`--tool`）かチャットプロンプト（`--prompt`）
を実行でき、`--allow-tools` でスケジュールターンが毎回の確認なしにツールを
使えるようになります。

### ランタイム API

`EneHandle::add_schedule` / `list_schedules` / `delete_schedule` /
`set_schedule_enabled` / `list_schedule_runs` — 組み込みアプリや
デスクトップアプリが使います。

## 発火時の動作

スケジュールは**アクション**と**プロンプト**を持ちます。発火すると
`TurnOrigin::Scheduled` のターンが始まり、キャラクターが発話したりツールを
実行したりできます。確認が必要なスケジュールは承認を待ってから実行します
（`ScheduleConfirmation`）。すべての実行は記録され（`schedule_runs`）、
`/schedule history` で確認できます。

## 静音時間との関係

プロアクティブ発話には独自のゲート（クールダウン・静音時間）が
`mind.proactive` にあります。スケジュールターンはプロアクティブゲートとは
独立です。[認知ランタイム → プロアクティブ発話](../reference/architecture/cognitive-runtime.md#プロアクティブ発話)参照。
