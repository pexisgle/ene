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
same pipeline as in-tree tools. Process acceptance is a stdio server that
invokes real `git` (`mcp:git.status` / `mcp:git.log`). The Connectors page
edits that document. A marketplace picker for popular servers is a successor
milestone.

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
| `provider.gguf` | Local GGUF LLM and embeddings (`plugins/provider/gguf`). GGUF weights use the plugin catalog; `llama-server` is installed from the host GitHub catalog (`provider.assets`, Engines page). Optional `server_path` / `model_path` override installs. |
| `provider.openai_compat` | Cloud LLM, embeddings, TTS, STT (`/v1` chat+audio). Optional `base_url` for OpenRouter and other hosts. |
| `provider.anthropic` | LLM (Messages API) |
| `provider.elevenlabs` | TTS |
| `provider.voicevox` | TTS. Host-managed VOICEVOX Engine (VVPP CPU) via `provider.assets`, or user-run engine / `server_path` |
| `provider.edge_tts` | TTS (Edge Neural Voice) |

Native in-process engines (llama.cpp, whisper.cpp, Kokoro ONNX) are not in this
tree. Local GGUF chat and embeddings use `provider.gguf` (`ene-provider-gguf`).
The plugin owns the static weight catalog; the host fetches `llama-server`
releases from GitHub, stores verified artifacts under
`data_dir/plugins/provider.gguf/assets/`, and spawns `llama-server` on loopback
via `ene-fiber`. Sidecar helpers also live in `templates/sidecar`.

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
does not call vendor HTTP. Local GGUF weights and sidecars use the generic
`provider.assets` flow (`POST /api/v1/providers/assets/*`). `provider.gguf`
lists sidecar `/v1/models` only when llama-server is already up. TTS plugins
that have not implemented the RPC keep free-text model fields.

## `provider.assets`

Provider plugins may expose an `assets` face (`PROVIDER_ASSETS_VERSION = 1`):

| Method | Role |
|---|---|
| `assets.list` | Catalog rows plus install state (`installed`, `active`, `local_path`, versions) |
| `assets.install` | Start an async install job (`job_id`) |
| `assets.install_status` | Progress (`phase`, `received`, `total`, `error`) |
| `assets.set_active` | Switch the active version (sidecars) |

Kinds are extensible strings (`sidecar`, `weight`, …). Desktop renders any
plugin that negotiates `assets`; `ene-core` proxies the same contract over HTTP
(`POST /api/v1/providers/assets/*`, including `refresh_catalog`).

**Host-managed catalogs.** Sidecar engines for `provider.gguf` (`llama-server`)
and `provider.voicevox` (`voicevox-engine`) are listed and installed by the host
(`ene-fiber` + `ene-provider-assets`), not from static plugin source. At startup
(and on manual refresh) the host fetches GitHub Releases for
`ggml-org/llama.cpp` and `VOICEVOX/voicevox_engine`, caches JSON under
`data_dir/catalog-cache/`, and merges install state from each plugin's
`manifest.json`. Install keys are `{release_tag}/{variant_id}` (for example
`b4282/avx2`, `0.25.2/cpu`, `0.25.2/directml`). The Engines UI picks release and
backend variant before download. VOICEVOX CUDA/NVIDIA packages split across
`.vvppp` / `.7z.001` multipart archives are not installed by the host yet.

**Plugin-owned catalogs.** GGUF weight files remain static Hugging Face URLs
inside `provider.gguf`; the plugin probe still serves `assets.list` for weights
while the host overrides sidecar rows.

**Downloads.** The host validates URLs against fixed GitHub (and Hugging Face for
weights) prefixes, streams artifacts to disk, verifies SHA-256 when GitHub
provides a digest, and records local digests after first install. VVPP CPU
packages extract as a full zip tree; llama-server zips extract the full archive
(Windows bundles ship `ggml.dll` and related libraries beside the executable).
CUDA builds also pull matching `cudart-*` companion zips into the same directory.

**Sidecar injection.** After install, `ene-fiber` injects `sidecar_base_url` for
`provider.gguf` and `cas_path` for `provider.voicevox` when those fields are not
already set in settings.
