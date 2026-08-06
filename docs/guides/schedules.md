# Schedules

Schedules are **persistent, timed actions**: the companion (or the system)
does something at a future time, on an interval, or on a cron expression —
even across restarts, because schedules live in the database.

## Schedule kinds

| Kind | Fires | Example |
|---|---|---|
| `one_shot` | Once, at `start_at` | "Remind me in 10 minutes" |
| `interval` | Every `interval` seconds, anchored on `start_at` | "Check every hour" |
| `cron` | Per a 5/6-field cron expression in `timezone` | "Every weekday 09:00 Asia/Tokyo" |
| `startup` | Once per process start | "On each app launch" |

## Managing schedules

### CLI REPL

```sh
/schedule list
/schedule add water --kind one_shot --at "2026-08-07T09:00:00+09:00" --prompt "Time to water the plants"
/schedule add check --kind interval --every 3600 --prompt "Check the inbox"
/schedule add morning --kind cron --cron "0 9 * * 1-5" --tz "Asia/Tokyo" --prompt "Good morning summary"
/schedule history <name>
/schedule pause <name>
/schedule resume <name>
/schedule delete <name>
```

Exactly the fields each kind requires must be present; invalid cron
expressions, invalid timezones, or past `start_at` values are rejected at
creation time. `--tz <IANA zone>` (default: the system local timezone,
falling back to `UTC` when it cannot be determined) selects the timezone for
cron evaluation; one-shot and interval schedules are absolute-time based, so
the timezone is stored for reference only. `--confirm` requires user
confirmation before the action starts (the confirmation prompt reuses the
standard permission dialog; an unanswered prompt times out after
`scheduler.confirmation_timeout_secs`). The full option set is
`/schedule add <name> --kind <one_shot|interval|cron|startup>`
`[--at <RFC3339> | --every <secs> | --cron <expr>]`
`[--tool <name> --args <json> | --prompt <text>] [--allow-tools]`
`[--tz <zone>] [--confirm] [--retries <n>] [--retry-delay <secs>]` — a
schedule can run a tool call (`--tool`) or a chat prompt (`--prompt`), and
`--allow-tools` lets the scheduled turn use tools without per-call prompts.

### Runtime API

`EneHandle::add_schedule` / `list_schedules` / `delete_schedule` /
`set_schedule_enabled` / `list_schedule_runs` — used by embedders and the
desktop app.

## What a schedule does when it fires

The schedule carries an **action** and a **prompt**. Firing a schedule
starts a turn with `TurnOrigin::Scheduled`, so the character can speak or
run tools on schedule. If the schedule requires confirmation, the turn
waits for approval (`ScheduleConfirmation`); otherwise it runs directly.
Every run is recorded (`schedule_runs`) and inspectable via
`/schedule history`.

## Quiet hours and interference

Proactive speech has its own gating (cooldown, quiet hours) in
`mind.proactive` — scheduled turns are independent of proactive gating.
See [Cognitive runtime → Proactive](../reference/architecture/cognitive-runtime.md#proactive).
