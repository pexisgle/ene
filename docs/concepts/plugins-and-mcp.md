# Plugins & MCP

Tools are **out-of-process binaries**. The host (`ene-fiber`) spawns them,
negotiates split `core` / `tool` / `provider` subprotocols (`ene-plugin-ipc`),
and registers tools in `ene-registry`. Harness functions that touch companion
state stay in-process and still go through the same registry pipeline.

Built-in tools live under `plugins/harness/`: `fs`, `exec`, `web`, `utility`,
`app`. `exec` is not part of `fs`. See [Built-in tools](../guides/tools/builtin-tools.md)
and [Write a tool](../guides/tools/write-a-tool.md).

MCP servers are not vendored. Each handwritten `mcp.json` row becomes a
`mcp.<id>` fiber running `ene-harness-mcp` (stdio or Streamable HTTP) on the
same pipeline as in-tree tools. The Connectors page edits that document. A
marketplace picker for popular servers is a successor milestone.

Provider plugins live under `plugins/provider/` and speak the `provider`
subprotocol. The host catalog (`ene_fiber::PROVIDER_PLUGINS`) is the single
list: desktop pickers, Engines, and `ai.tasks.*` all read it (via
`effective.providers`). Adding a provider is adding a plugin binary plus a
catalog row with its seams, `local`, and `needs_key` — not a second allowlist
in the UI.

Bind a catalog id with `ai.tasks.<task>.plugin`. Each configured task gets its
own fiber (`row_id = ai.tasks.<task>`) so chat and embedding can share a
plugin binary with different GGUFs.

| Plugin | Modalities |
|---|---|
| `provider.gguf` | Local GGUF LLM and embeddings (`plugins/provider/gguf`). Set `model_path`; `llama-server` is resolved from `PATH` or the bundle when `server_path` is empty. |
| `provider.openai_compat` | Cloud LLM, embeddings, TTS, STT (`/v1` chat+audio). Optional `base_url` for OpenRouter and other hosts. |
| `provider.anthropic` | LLM (Messages API) |
| `provider.elevenlabs` | TTS |
| `provider.voicevox` | TTS. User-run engine, or `server_path` sidecar |
| `provider.edge_tts` | TTS (Edge Neural Voice) |

Native in-process engines (llama.cpp, whisper.cpp, Kokoro ONNX) are not in this
tree. Local GGUF chat and embeddings use `provider.gguf` (`ene-provider-gguf`)
with `model_path`; each task's sidecar starts `llama-server` on loopback and
talks `/v1`. Sidecar helpers also live in `templates/sidecar`.

MCP `resources/list` snapshots land in `<workspace>/mcp-context/` and are
injected as a context source. MCP `prompts/list` become `SKILL.md` files under
the data-dir skills home.

## Launch profiles

`plugins.profile` chooses the harness tree. `apply_profile` reconciles fibers;
unrelated rows stay up.

| Profile | Harness plugins | MCP |
|---|---|---|
| `desktop` (default) | `tool.utility`, `tool.fs`, `tool.exec`, `tool.web`, `tool.app` | handwritten `mcp.json` rows |
| `minimal` | `tool.utility` | none |
| `headless` | `tool.utility`, `tool.fs`, `tool.exec`, `tool.web` | handwritten `mcp.json` rows |

Providers come from the host catalog and are spawned when bound in
`ai.tasks.*`, not from the profile name. Change the profile from the Plugins
page or `PATCH /api/v1/settings` with `{"plugins":{"profile":"minimal"}}`.

Remote inventory (OpenAI-compatible `/models`, Anthropic `v1/models`) is a
provider RPC (`list_models`). Core exposes it as `POST /api/v1/providers/models`
(plugin, task, draft base URL, typed key; empty key uses the vault). Desktop
does not call vendor HTTP. Local GGUF files stay on the host catalog and file
picker; plugins never download weights. `provider.gguf` lists sidecar
`/v1/models` only when llama-server is already up. TTS plugins that have not
implemented the RPC keep free-text model fields.
