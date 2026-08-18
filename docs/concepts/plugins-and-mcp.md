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
subprotocol. Bind them with `ai.tasks.<task>.plugin`:

| Plugin | Modalities |
|---|---|
| `echo` | Host offline model (no network) |
| `provider.openai_compat` | LLM, embeddings, TTS, STT (`/v1` chat+audio). Set `server_path` to spawn llama-server on loopback (P-1006). |
| `provider.anthropic` | LLM (Messages API) |
| `provider.elevenlabs` | TTS |
| `provider.voicevox` | TTS. User-run engine, or `server_path` sidecar |
| `provider.edge_tts` | TTS (Edge Neural Voice) |

API keys are stored in the vault, not in `settings.json`. Native in-process
engines (llama.cpp, whisper.cpp, Kokoro ONNX) are not in this tree; local
GGUF chat uses `provider.openai_compat` with `ai.tasks.*.server_path` pointing
at `llama-server` (and `model_path` at the GGUF). The plugin spawns that
binary on loopback and talks `/v1`. Sidecar helpers also live in
`templates/sidecar`.

MCP `resources/list` snapshots land in `<workspace>/mcp-context/` and are
injected as a context source. MCP `prompts/list` become `SKILL.md` files under
the data-dir skills home.
