# Built-in tools

Bundled tools live under `plugins/harness/` and use the same IPC as a
third-party tool.

| Plugin | Binary | Role |
|---|---|---|
| `fs` | `ene-harness-fs` | Read / write / edit inside the workspace. No shell. |
| `exec` | `ene-harness-exec` | Process execution (separate from `fs`, D-24) |
| `web` | `ene-harness-web` | Fetch via the host HTTP broker |
| `utility` | `ene-harness-utility` | Deterministic helpers (time, hash, encode) |

`fs.write` and `exec` are not on the surface schema. The registry filters by
empty `side_effects`, not by a name allow-list. Approval is deny-by-default
until `ene-plane` has a matching policy.

Mature MCP servers (git, browser, calendar, homeassistant, geo) are not
in-tree; connect them as handwritten profile rows.
