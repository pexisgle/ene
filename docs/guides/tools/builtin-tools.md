# Built-in tools

Bundled tools live under `plugins/tool/` and use the same IPC as a
third-party tool. `ene-core` always applies these profile rows:

| Plugin | Binary | Role |
|---|---|---|
| `utility` | `ene-tool-utility` | Hash, time, calc, random, text |
| `fs` | `ene-tool-fs` | Read / write / edit / list / search / patch / undo in the workspace. No shell. |
| `exec` | `ene-tool-exec` | Process execution by program name (separate from `fs`) |
| `web` | `ene-tool-web` | HTTPS fetch and public search (SSRF blocked) |
| `app` | `ene-tool-app` | Screenshot, windows, clipboard, pointer/keyboard |

`fs.write`, `fs.edit`, `exec`, and input-mutating `app.*` tools are not on the
surface schema. The registry filters by empty `side_effects`, not by a name
allow-list. Approval is deny-by-default until `ene-plane` has a matching
policy. Host observation (`app.active_window`, `app.screenshot`) skips the
approval popup when the user enabled the proactive source.

Mature MCP servers (git, browser, calendar, homeassistant, geo) are not
in-tree; connect them as handwritten `mcp.<id>` rows.
