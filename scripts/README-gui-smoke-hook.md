# GUI smoke hook (GitHub Actions)

Wakes the **ene GUI確認** Cursor routine via its webhook when an open PR is ready
for GUI smoke — even if the Grok Bot computer is stopped.

## Fire rules (`scripts/gui-smoke-hook.py`)

Same meaning as the old box `ene-gui-smoke-watcher`:

1. PR touches `apps/ene-stage/` or `apps/ene-desktop/`
2. All non-skipped checks are green
3. Either first green for that PR, or first green after label `gui-smoke-issues`

Retry: POST retries ~3 times on HTTP 5xx / URLError / timeout (1s, 2s, 4s); leftover failures are logged, that PR is left unfired, and the job stays green. HTTP 4xx still fails the job.

State:

- `fired_green` → Actions cache (`.gui-smoke-hook-state/state.json`)
- issues / blocked → PR label `gui-smoke-issues` (set by box `record_smoke.py`)

## Repo secrets (required)

Settings → Secrets and variables → Actions:

| Name | Value |
| --- | --- |
| `ENE_GUI_SMOKE_WEBHOOK_URL` | Cursor routine webhook URL |
| `ENE_GUI_SMOKE_WEBHOOK_KEY` | webhook bearer / X-Webhook-Key |

On the box these already live in `/home/box/ene-gui-smoke-watcher/config.env`
as `WEBHOOK_URL` / `WEBHOOK_KEY`.

## Triggers

- `workflow_run` of **CI** completed
- weekday schedule (UTC) as a thin backup
- `workflow_dispatch` for manual runs
