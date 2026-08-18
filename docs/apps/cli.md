# CLI user guide

`ene-ctl` is the terminal client for `ene-core`. It talks over the public
HTTP/WS API — the same contract as stage and Web.

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

There is no desktop-only API. Anything stage can do on the wire, `ene-ctl` can
do as well.
