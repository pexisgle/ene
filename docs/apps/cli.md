# CLI user guide

`ene-ctl` is the terminal client for `ene-core`. It talks over the public
HTTP/WS API — the same contract as stage, Web, and the legacy desktop.

```sh
cargo run -p ene-ctl -- --help
cargo run -p ene-ctl -- core start
cargo run -p ene-ctl -- core status
```

| Flag / env | Meaning |
|---|---|
| `--url` / `ENE_API_URL` | Base URL of `ene-core` |
| `--token` / `ENE_API_TOKEN` | Bearer token (or contents of `api.token`) |
| `--client-id` | Client id for exclusive resources (default `cli`) |
| `--verbose` | Subscribe at `detail` depth (inner / thinking / tool args) |

`core start` launches the `ene-core` binary, waits for `api.json`, and prints
the token **file path** (not the token). `core stop` signals the recorded PID.

`ene-ctl task list` / `task cancel` / `task answer <id> <text>` talk to jobs.
`task answer` is `POST /api/v1/jobs/{id}/answer` — it does not send a chat
follow-up.

There is no desktop-only API. Anything stage can do on the wire,
`ene-ctl` can do as well.

Normal chat output stays at **surface** depth. Inner, thinking, and tool
arguments appear only with `--verbose` or `ene-ctl debug`.

| Command | Meaning |
|---|---|
| `soul list` / `soul show <id>` | List souls or show one |
| `soul skills <id> [names…]` | Replace the soul's skill allowlist (`PATCH /souls/{id}/skills`). Omit names to allow every installed skill |
| `chat <target> [text]` | One-shot turn when `text` is set. Omit `text` for a REPL (`.quit` / `.exit` / EOF). `target` is a session id, or a soul id that reuses an open conversation or creates one |
| `session list/show/create/fork/export/compact` | Session lifecycle |
| `session search <query>` / `split <id>` / `end <id>` | Search, split at the current turn, explicit end |
| `usage [--session <id>]` | LLM usage totals |
| `tool list` / `tool call <name> [json]` | List tools or execute one (`POST /tools/{name}/test`). `json` defaults to `{}` |
| `plugin list/restart/config` | Plugin fibers |
| `task list/cancel/answer` | Jobs (public delegations) |
| `memory list/edit/delete` | Memory rows |
| `schedule list` / `schedule add <soul> <name> <spec>` | List schedules, or create one. Quote `spec` (5 or 6 cron fields). `--timezone` (default `UTC`), `--action` (`remind` / `job` / `turn`), `--action-ref`, `--important` |
| `core start/status/stop` | Core process |
| `debug log <session>` | History at detail depth |
| `debug delegation <id>` | Job id or delegation session id, history at detail depth |
| `debug spans` | Diagnostic spans |
| `exclusive show/claim` | Exclusive resources |

```sh
ene-ctl chat <soul-id>
ene-ctl chat <session-id> "hello"
ene-ctl schedule add <soul-id> morning "0 9 * * *" --timezone Asia/Tokyo
ene-ctl tool call utility.time
ene-ctl debug delegation <job-id>
```
