# Tool Catalog

Tools run as separate binaries and talk to ene over IPC. Names are namespaced: `<namespace>.<action>`.

## Built-in namespaces

| Namespace | Actions (summary) | Binary | Guide |
|-----------|-------------------|--------|-------|
| `filesystem` | `read`, `write`, `edit`, `delete`, `glob`, `grep`, `patch` | `ene-tool-fs` | [Filesystem](fs.md) |
| `shell` | `execute` | `ene-tool-fs` | [Filesystem](fs.md) |
| `app` | clipboard, windows, keyboard, mouse, screenshot, … | `ene-tool-app` | [GUI automation](app.md) |
| `browser` | `navigate`, `click`, `type`, `wait`, … | `ene-tool-browser` | [Browser](browser.md) |
| `web` | `fetch`, `search` | `ene-tool-web` | [Web](web.md) |
| `utility` | `question`, todos, time, system info, `undo` | `ene-tool-utility` / `ene-tool-fs` | [Utility](utility.md) |

## Safety

Path limits, blocked shell commands, and undo: [Security sandbox](sandbox.md).

## Adding your own

Practical steps: [Write a tool](write-a-tool.md).

## Reference (IPC, host, RAG, SDK)

- [Tool system (IPC / host)](../../reference/tools/overview.md)
- [Tool RAG](../../reference/tools/tool-rag.md)
- [SDK](../../reference/tools/sdk.md)
- [Derive macro](../../reference/tools/derive-macro.md)
