# Schedules

Schedules are persistent timed actions in `ene-work`. They survive process
restart because they live in the work database, not in a client.

## Spec and action

`spec` is a cron expression (5 or 6 fields) evaluated in `timezone`.
`action` is one of:

| Action | Meaning |
|---|---|
| `remind` | Speak a reminder on the surface |
| `turn` | Start a dialogue turn |
| `job` | Kick a back-harness job (`action_ref`); the daemon job lane then runs tools |

`important` schedules still fire when quiet hours would otherwise hold
speech. Quiet hours for schedules use the same window as
`mind.proactive.quiet_hours` (start/end hour and timezone). They do not
reuse the proactive speech gate: a due `remind` is deferred until the
window ends, except `important`.

## Daemon driver

`ene-core` polls due rows about once a second.

- On boot, `catch_up_missed` runs first: overdue `remind` fires once;
  overdue `job` / `turn` are not started (D-5) and `next_fire` is
  advanced.
- After that, `fire_due` runs. `remind` is a `CompanionReport` through
  the job speech gate (`it's time: …`) and lands in the open session.
  `job` starts a public delegation (`action_ref` or the schedule name).
  `turn` starts a dialogue turn with `TurnOrigin::Scheduled` when the
  soul has an open session.

## Managing schedules

`ene-ctl schedule list` prints the HTTP page. `ene-ctl schedule add` creates
a row after checking that `spec` is a 5- or 6-field cron expression and that
`timezone` is an IANA name (or `UTC`). Quote `spec` when it contains spaces.
Create / patch / delete are also on `/api/v1/schedules`:

```sh
ene-ctl schedule list
ene-ctl schedule add <soul-id> morning "0 9 * * *" --timezone UTC
ene-ctl schedule add <soul-id> standup "0 30 9 * * 1-5" --timezone Asia/Tokyo --action remind
```

`PATCH` toggles `enabled`. Firing is server-side; clients do not poll a
local timer.

A commitment due (`expires_at`) does not auto-create a schedule. Patch
`schedule_id` on a commitment to name an existing row for the same soul.
Completing, deleting, or expiring that commitment disables the named
schedule. Create a `remind` / `job` / `turn` row when you want a timed
Work action.
