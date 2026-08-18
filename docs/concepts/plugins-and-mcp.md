# Plugins & MCP

Tools are **out-of-process binaries**. The host (`ene-fiber`) spawns them,
negotiates split `core` / `tool` subprotocols (`ene-plugin-ipc`), and
registers their specs in `ene-registry`. Harness functions that touch
companion state stay in-process and still go through the same registry
pipeline.

Built-in tools live under `plugins/harness/`: `fs`, `exec`, `web`, `utility`.
`exec` is not part of `fs`. See [Built-in tools](../guides/tools/builtin-tools.md)
and [Write a tool](../guides/tools/write-a-tool.md).

MCP servers are not vendored. v1.0 connects them as handwritten profile rows
on the same pipeline as in-tree tools. A settings UI for picking popular
servers is a successor milestone.

Provider plugins (LLM / TTS / STT) are not in this tree yet; conversation is
Echo-only until they are rewritten onto the new IPC. Sidecar helpers for a
future rewrite live in `templates/sidecar`.
