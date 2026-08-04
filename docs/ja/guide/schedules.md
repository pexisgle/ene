# スケジュール：永続的な時刻指定アクション

Ene の永続スケジューラーは、指定した時刻・固定間隔・cron 式・またはアプリ
起動時にアクションを実行します。スケジュール定義と実行履歴（失敗ステータス、
再試行回数を含む）はメモリストアのデータベースに保存され、アプリを再起動して
も復元されます。

> スケジューラーはメモリストア（`store.enabled = true`）を必要とします。無効な
> 場合はスケジュールは実行されず、`/schedule` はストアが必要であると報告します。

## CLI でのスケジュール管理

スケジュールの管理はすべて REPL の `/schedule` コマンドで行います：

```
/schedule list
/schedule history <name>
/schedule delete <name>
/schedule pause <name>
/schedule resume <name>
/schedule add <name> --kind <one_shot|interval|cron|startup> [options]
```

### スケジュールの追加

すべてのスケジュールには一意の `name`（`history` / `delete` / `pause` /
`resume` で使用）、`--kind`、およびアクションが 1 つ必要です：

- `--tool <name> --args <json>` — ツールを直接実行します（例：
  `--tool system.search_tools --args '{"query":"files"}'`）。
- `--prompt <text> [--allow-tools]` — 指定したプロンプトでコンパニオンのターンを
  実行します。`--allow-tools` を付けるとそのターンはツールを呼び出せます。

種類ごとのオプション：

| 種類 | オプション | 動作 |
|---|---|---|
| `one_shot` | `--at <RFC3339 タイムスタンプ>` | 指定した瞬間に一度だけ実行され、完了します。 |
| `interval` | `--at <RFC3339 タイムスタンプ>` `--every <秒>` | `--at` を基準に固定レートで実行：`start + k × every`。 |
| `cron` | `--cron <式>` `--tz <IANA ゾーン>` | スケジュールのタイムゾーンで cron 式に従って実行。5 フィールド（`分 時 日 月 曜日`）と 6 フィールド（秒付き）の両方に対応。 |
| `startup` | — | アプリ起動のたびに一度実行されます。 |

共通オプション：

- `--tz <IANA ゾーン>`（デフォルト `UTC`）— cron 評価用のタイムゾーン。単発・
  インターバルは絶対時刻ベースのため、タイムゾーンは参照用に保存されるだけです。
  例：`--tz Asia/Tokyo`。
- `--confirm` — アクション開始前にユーザー確認を要求します。確認プロンプトは
  標準のパーミッションダイアログを再利用します。未回答のまま
  `scheduler.confirmation_timeout_secs`（デフォルト 5 分）が経過すると、
  実行は `timed_out` として記録されます。
- `--retries <n>`（デフォルト `0`）— 失敗した実行の追加試行回数。再試行は
  `--retry-delay <秒>`（デフォルト `60`）の間隔で行われます。
- `--allow-tools` — `--prompt` と併用し、スケジュールされたターンがツールを
  呼び出せるようにします。

例：

```
/schedule add morning-reminder --kind cron --cron "0 9 * * *" --tz Asia/Tokyo \
  --prompt "Remind the user to drink water"
/schedule add backup --kind interval --every 3600 \
  --tool fs.copy --args '{"src":"/data/a","dst":"/data/b"}'
/schedule add one-off --kind one_shot --at 2026-08-05T15:00:00+09:00 \
  --prompt "Say hello to the user"
```

## 実行の記録方法

実行が要求されるたびに、実行履歴（`/schedule history <name>`）に行が追加され、
次のいずれかのステータスになります：

| ステータス | 意味 |
|---|---|
| `running` | 実行要求が処理され、実行中。 |
| `awaiting_approval` | 確認の決定を待機中。 |
| `success` | 正常に完了。 |
| `failed` | エラーで終了（設定されていれば再試行が適用されます）。 |
| `skipped_busy` | 実行時刻に会話中でした。その実行はスキップされ、キューには入りません。 |
| `skipped_late` | 遅延実行の猶予期間を超えて処理されました（下記参照）。 |
| `denied` | ユーザーが確認プロンプトを拒否しました。 |
| `timed_out` | 確認プロンプトに時間内に回答がありませんでした。 |
| `interrupted` | 実行中にアプリが再起動しました。 |

スケジュール実行が**会話を妨げることはありません**：実行時刻にチャット中の
場合、その実行は `skipped_busy` として記録され、スケジュールは次の実行時刻に
進みます。通常の会話と同じシングルフライトゲートを使用するため、Busy 状態は
一貫して保たれます。

## サスペンド・時刻変更・遅延実行

システムがスリープから復帰したとき、時計がずれたとき、またはアプリが終了して
いたとき、`scheduler.late_grace_secs`（デフォルト 60 秒）を超えて遅れた実行は
`skipped_late` として記録され、**実行されません** — 長時間のサスペンド後に
溜まったジョブが一斉に実行されることはありません。次の実行時刻は現在時刻から
再計算されます。猶予期間内の実行は通常どおり実行されます。

## タイムゾーンと夏時間（DST）

cron スケジュールは設定された IANA タイムゾーンで評価されます。夏時間はその
ゾーンに従って処理されます：存在しない実行時刻（春先の時計送り）は次の有効な
実行時刻にスキップされ、2 回発生する実行時刻（秋の時計戻し）は両方の瞬間に
実行されます。

## 発話のスケジュール

`--prompt` のスケジュールは通常のコンパニオンターン（origin `scheduled`）として
実行され、プロアクティブ発話と同様に CLI に表示されます。ツールアクションは
`ToolCallStart` / `ToolCallResult` / `Terminal` イベントを配信し、パーミッション
プロンプトも対話型ツール呼び出しと同じダイアログで解決されます。
