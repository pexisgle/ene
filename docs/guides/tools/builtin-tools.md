# Built-in tools

Bundled tools live under `plugins/tool/` and use the same IPC as a
third-party tool. `ene-core` always applies these profile rows:

| Plugin | Binary | Role |
|---|---|---|
| `utility` | `ene-tool-utility` | Hash, time, system_info, calc (units and snapshot FX), random, text |
| `fs` | `ene-tool-fs` | Read / write / edit / list / search / patch / undo in the workspace. No shell. Search is literal unless `regex` is set. `fs.read` returns `text` and a blake3 `hash` of the raw file bytes. `fs.write`, `fs.edit`, and `fs.patch` accept optional `expected_hash`; a mismatch fails with a stale-precondition error and leaves the file unchanged. Writes use a temp file plus rename, and per-path operations serialize. Edits use exact substring match; multiple matches without `replace_all` are ambiguity errors. Line endings (CRLF/LF), UTF-8 BOM, and trailing newlines are preserved. `fs.undo` only reverts writes from the same job (`job_id` or `ENE_JOB_ID`); secret-looking paths and bodies over 1 MiB are not stored in the undo journal. Unified diffs match hunk context, not only line numbers. |
| `exec` | `ene-tool-exec` | Process execution by program name (separate from `fs`). Timeouts send SIGTERM, then SIGKILL, and return captured output when the process exits. |
| `web` | `ene-tool-web` | HTTPS fetch (size-capped, SSRF blocked) and public search (DuckDuckGo answers, HTML fallback when the instant API is empty) |
| `app` | `ene-tool-app` | Screenshot (grim, ImageMagick, gnome-screenshot, spectacle, scrot), windows (wmctrl / hyprctl / sway), clipboard, pointer/keyboard |

`fs.write`, `fs.edit`, `exec`, and input-mutating `app.*` tools are not on the
surface schema. The registry filters by empty `side_effects`, not by a name
allow-list. Approval is deny-by-default until `ene-plane` has a matching
policy. Host observation (`app.active_window`, `app.screenshot`) skips the
approval popup when the user enabled the proactive source. Observation decodes
`png_base64` and summarizes off the session log; `{available: false}` is not a
successful look.

Mature MCP servers (git, browser, calendar, homeassistant, geo) are not
in-tree; connect them as handwritten `mcp.<id>` rows.
