# Tool Catalog

Tools run as separate binaries and talk to ene over IPC. Names are namespaced: `<namespace>.<action>`.

## Built-in namespaces

| Namespace | Actions (summary) | Binary | Guide |
|-----------|-------------------|--------|-------|
| `filesystem` | `read`, `write`, `edit`, `delete`, `glob`, `grep`, `patch` | `ene-plugin-fs` | [Filesystem](fs.md) |
| `shell` | `execute` | `ene-plugin-fs` | [Filesystem](fs.md) |
| `app` | clipboard, windows, keyboard, mouse, screenshot, … | `ene-plugin-app` | [GUI automation](app.md) |
| `browser` | `navigate`, `click`, `type`, `wait`, … | `ene-plugin-browser` | [Browser](browser.md) |
| `web` | `fetch`, `search` | `ene-plugin-web` | [Web](web.md) |
| `utility` | `question`, todos, time, system info, `undo` | `ene-plugin-utility` / `ene-plugin-fs` | [Utility](utility.md) |

## Safety

Path limits, blocked shell commands, and undo: [Security sandbox](sandbox.md).

## Adding your own

Practical steps: [Write a tool](write-a-tool.md).

## Reference (IPC, host, RAG, SDK)

- [Tool system (IPC / host)](../../reference/tools/overview.md)
- [Tool RAG](../../reference/tools/tool-rag.md)
- [SDK](../../reference/tools/sdk.md)
- [Derive macro](../../reference/tools/derive-macro.md)
