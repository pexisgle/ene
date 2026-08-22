# Built-in tools

Bundled tools live under `plugins/tool/` and use the same IPC as a
third-party tool. `ene-core` always applies these profile rows:

| Plugin | Binary | Role |
|---|---|---|
| `utility` | `ene-tool-utility` | Hash, time, system_info, calc (units and snapshot FX), random, text |
| `fs` | `ene-tool-fs` | Read / write / edit / list / glob / delete / search / patch / undo in the workspace. No shell. `fs.glob` is workspace-relative, sorted, and capped; it does not follow directory symlinks. `fs.delete` is a destructive side effect (approval required): files or empty directories only, never symlinks, junctions, or non-empty dirs. Search delegates to host `rg` and is literal unless `regex` is set. `fs.read` returns `text` and a blake3 `hash` of the raw file bytes. `fs.write`, `fs.edit`, and `fs.patch` accept optional `expected_hash`; a mismatch fails with a stale-precondition error and leaves the file unchanged. Writes use a temp file plus rename, and per-path operations serialize. Edits use exact substring match; multiple matches without `replace_all` are ambiguity errors. Line endings (CRLF/LF), UTF-8 BOM, and trailing newlines are preserved. `fs.undo` only reverts writes from the same job (`job_id` or `ENE_JOB_ID`); secret-looking paths and bodies over 1 MiB are not stored in the undo journal. Unified diffs match hunk context, not only line numbers. The `tool.fs` process is not given workspace filesystem rights; glob/delete/read/write run on the host FileBroker. |
| `exec` | `ene-tool-exec` | Process execution by program name or shell (`exec.run` / `exec.shell`). Output is capped while streaming (1 MiB stdout, 1 MiB stderr, 2 MiB combined) with truncation metadata. Timeouts signal the process tree (Unix process group; Windows job object + `taskkill /T`). Working directory is confined to `ENE_WORKSPACE`; inherited env is allowlisted and secret-shaped vars are dropped. |
| `web` | `ene-tool-web` | HTTPS fetch and public search. The host net broker performs every hop (SSRF, DNS pin, 1 MiB stream cap, text content types). The plugin process is network-isolated and cannot dial HTTP itself. Fetch returns markdown/text/html; search backends include DuckDuckGo (default), ArXiv; Tavily/Exa need vault credentials. |
| `fs` | `ene-tool-fs` | Read / write / edit / list / search / patch / undo in the workspace. No shell. Search is literal unless `regex` is set and delegates to host `rg`, with optional `include`, case-insensitive, context, count, capture-group, and line-number modes. `fs.read` returns `text` and a blake3 `hash` of the raw file bytes. `fs.write`, `fs.edit`, and `fs.patch` accept optional `expected_hash`; a mismatch fails with a stale-precondition error and leaves the file unchanged. Writes use a temp file plus rename, and per-path operations serialize. Edits try exact matching first, then CRLF-normalized indent-tolerant, line-trimmed, and block-anchor fallbacks; multiple matches without `replace_all` are ambiguity errors. Line endings (CRLF/LF), UTF-8 BOM, and trailing newlines are preserved. `fs.undo` only reverts writes from the same job (`job_id` or `ENE_JOB_ID`); secret-looking paths and bodies over 1 MiB are not stored in the undo journal. Unified diffs match hunk context, not only line numbers. |
| `exec` | `ene-tool-exec` | Process execution by program name or shell (`exec.run` / `exec.shell`). Output is capped while streaming (1 MiB stdout, 1 MiB stderr, 2 MiB combined) with truncation metadata. Timeouts signal the process tree (Unix process group; Windows job object + `taskkill /T`). Working directory is confined to `ENE_WORKSPACE`; inherited env is allowlisted and secret-shaped vars are dropped. |
| `web` | `ene-tool-web` | HTTPS fetch and public search. The host net broker performs every hop (SSRF, DNS pin, 1 MiB stream cap, text content types). The plugin process is network-isolated and cannot dial HTTP itself. Fetch returns markdown/text/html; search backends include DuckDuckGo (default), ArXiv; Tavily/Exa need vault credentials. |
| `app` | `ene-tool-app` | Screenshot (XDG portal on Wayland, CLI fallback, GDI on Windows), monitors, windows where the compositor allows, native clipboard, X11/Windows input only |

`fs.write`, `fs.edit`, `fs.delete`, `exec`, and input-mutating `app.*` tools are not on the
surface schema. The registry filters by empty `side_effects`, not by a name
allow-list. Approval is deny-by-default until `ene-plane` has a matching
policy. Host observation (`app.active_window`, `app.screenshot`) skips the
approval popup when the user enabled the proactive source. Observation decodes
`png_base64` and summarizes off the session log; `{available: false}` is not a
successful look. When the model calls `app.screenshot`, the PNG is stored as a
spill blob and the conversation log keeps an `ImageRef` so a chat binding
with `ai.tasks.chat.supports_images` receives `LlmImage` instead of a giant
base64 text block. Text-only or unknown bindings keep `[image omitted]`. Tool JSON larger than
`harness.tool_output.soft_limit_bytes` (default 64 KiB) is spilled the same way.

Mature MCP servers (git, browser, calendar, homeassistant, geo) are not
in-tree; connect them as handwritten `mcp.<id>` rows. Old-action mapping,
security gaps, and v1.0 vs post-v1.0 live in
[Product boundaries](../../concepts/product-boundaries.md).
