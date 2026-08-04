# Schedules: Persistent Timed Actions

Ene's persistent scheduler runs actions at a specific time, on a fixed
interval, on a cron schedule, or once per app start — across restarts.
Schedules are stored in the memory store's database together with their run
history, so definitions, failure status, and retry counts survive an app
restart.

> The scheduler requires the memory store (`store.enabled = true`). Without
> it, no schedule fires and `/schedule` reports that the store is required.

## Managing schedules in the CLI

All schedule management happens through the `/schedule` REPL command:

```
/schedule list
/schedule history <name>
/schedule delete <name>
/schedule pause <name>
/schedule resume <name>
/schedule add <name> --kind <one_shot|interval|cron|startup> [options]
```

### Adding a schedule

Every schedule has a unique `name` (used by `history` / `delete` / `pause` /
`resume`), a `--kind`, and exactly one action:

- `--tool <name> --args <json>` — run a tool directly (e.g.
  `--tool system.search_tools --args '{"query":"files"}'`).
- `--prompt <text> [--allow-tools]` — run a companion turn with the given
  prompt; `--allow-tools` lets that turn call tools.

Kind-specific options:

| Kind | Options | Behavior |
|---|---|---|
| `one_shot` | `--at <RFC3339 timestamp>` | Fires once at the given instant, then completes. |
| `interval` | `--at <RFC3339 timestamp>` `--every <seconds>` | Fires at a fixed rate anchored on `--at`: `start + k × every`. |
| `cron` | `--cron <expression>` `--tz <IANA zone>` | Fires per the cron expression in the schedule's timezone. 5-field (`minute hour dom month dow`) and 6-field (with seconds) expressions are accepted. |
| `startup` | — | Fires once per app start. |

Common options:

- `--tz <IANA zone>` (default `UTC`) — timezone for cron evaluation. One-shot
  and interval schedules are absolute-time based; the timezone is stored for
  reference only. Example: `--tz Asia/Tokyo`.
- `--confirm` — require user confirmation before the action starts. The
  confirmation prompt reuses the standard permission dialog; an unanswered
  prompt times out after `scheduler.confirmation_timeout_secs` (default 5
  minutes) and the run is recorded `timed_out`.
- `--retries <n>` (default `0`) — how many extra attempts a failed run gets.
  Retries wait `--retry-delay <seconds>` (default `60`) between attempts.
- `--allow-tools` — with `--prompt`, allow the scheduled turn to call tools.

Examples:

```
/schedule add morning-reminder --kind cron --cron "0 9 * * *" --tz Asia/Tokyo \
  --prompt "Remind the user to drink water"
/schedule add backup --kind interval --every 3600 \
  --tool fs.copy --args '{"src":"/data/a","dst":"/data/b"}'
/schedule add one-off --kind one_shot --at 2026-08-05T15:00:00+09:00 \
  --prompt "Say hello to the user"
```

## How runs are recorded

Every claimed fire appends a row to the schedule's run history
(`/schedule history <name>`), with one of these statuses:

| Status | Meaning |
|---|---|
| `running` | Claimed and executing. |
| `awaiting_approval` | Waiting for a confirmation decision. |
| `success` | Finished successfully. |
| `failed` | Finished with an error (retries apply when configured). |
| `skipped_busy` | The actor was mid-conversation at fire time; the occurrence was skipped, never queued. |
| `skipped_late` | The fire arrived beyond the late-execution grace window (see below). |
| `denied` | The user denied the confirmation prompt. |
| `timed_out` | The confirmation prompt was never answered in time. |
| `interrupted` | The app restarted while the run was in flight. |

Scheduled runs **never interrupt a conversation**: if a fire comes due while
you are chatting, the occurrence is recorded `skipped_busy` and the schedule
continues with its next occurrence. The single-flight gate that prevents
overlapping turns is the same one used for normal conversation, so the Busy
state stays consistent.

## Suspend, clock changes, and late execution

When the system wakes from sleep, the clock jumps, or the app was closed,
fires that are overdue by more than `scheduler.late_grace_secs` (default 60
seconds) are recorded `skipped_late` and are **not** executed — there is no
burst of missed jobs after a long suspend. The next occurrence is computed
from the current time. Fires within the grace window still execute normally.

## Timezones and DST

Cron schedules are evaluated in their configured IANA timezone. Daylight
saving time is handled per that zone: a fire time that does not exist (spring
forward) is skipped to the next valid occurrence, and a fire time that occurs
twice (fall back) fires on both instants.

## Scheduling speech

Schedules with `--prompt` run as normal companion turns (origin
`scheduled`), rendered by the CLI like proactive speech. Tool actions stream
`ToolCallStart` / `ToolCallResult` / `Terminal` events and resolve
permission prompts through the same dialog as interactive tool calls.
