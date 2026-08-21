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
speech.

## Managing schedules

`ene-ctl schedule list` prints the HTTP page. Create / patch / delete are
on `/api/v1/schedules`:

```sh
ene-ctl schedule list
```

`PATCH` toggles `enabled`. Firing is server-side; clients do not poll a
local timer.
