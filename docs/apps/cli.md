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
| `--verbose` | Subscribe at `detail` depth (inner / thinking) |

`core start` launches the `ene-core` binary, waits for `api.json`, and prints
the token **file path** (not the token). `core stop` signals the recorded PID.

`ene-ctl task list` / `task cancel` / `task answer <id> <text>` talk to jobs.
`task answer` is `POST /api/v1/jobs/{id}/answer` — it does not send a chat
follow-up.

There is no desktop-only API. Anything stage can do on the wire,
`ene-ctl` can do as well.

| Command | Meaning |
|---|---|
| `soul list` / `soul show <id>` | List souls or show one |
| `soul skills <id> [names…]` | Replace the soul's skill allowlist (`PATCH /souls/{id}/skills`). Omit names to allow every installed skill |
