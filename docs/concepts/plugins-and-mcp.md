# Plugins & MCP

Ene's abilities are not compiled into the host. Tools (filesystem, web,
calendar, …) and AI providers (OpenAI, Anthropic, local GGUF models, TTS
voices, …) are **separate plugin binaries** that the host spawns and talks
to over IPC. External MCP servers plug in the same way.

## Why out-of-process

- **Isolation** — a plugin that crashes, hangs, or misbehaves cannot take
  the host down; the supervisor restarts it.
- **Sandboxing** — plugins declare their resource needs, and the host
  enforces admission control.
- **One runtime per native stack** — llama.cpp, whisper.cpp, and ONNX
  Runtime each live in their own binary so no user pays the build cost of
  runtimes they do not use.

## Two kinds of plugins

### Tool plugins (`plugins/tool/*`)

Provide named actions the character can call during a turn: `fs.read`,
`web.search`, `utility.timer_start`, … Each action declares its JSON
arguments, a summary/description for the model, keywords for retrieval,
side effects, and whether it runs in the background.

Built-in tool plugins:

| Plugin | Actions (namespace) |
|---|---|
| `app` | window/monitor listing, screenshots, typing, mouse/keyboard control |
| `browser` | Chrome automation: navigate, click, type, screenshot, extract |
| `calc` | expression evaluation, unit/currency/color conversion |
| `calendar` | calendar accounts, events, free-slot search (stateful, approval-gated) |
| `counter` | stateful counter example (used as the reference sample) |
| `fs` | read/write/edit/delete, glob/grep search, patch, shell, undo |
| `geo` | location, weather, timezone, sun position |
| `git` | status, diff, log, branch, remote, blame |
| `homeassistant` | Home Assistant state and control |
| `random` | random numbers, UUIDs, picks, colors |
| `utility` | notifications, todo list, time, system info, timers, questions |
| `web` | fetch URLs, web search (Brave/Exa/Tavily/DuckDuckGo/arXiv) |

### Provider plugins (`plugins/provider/*`)

Provide models and voice engines:

| Plugin | Kind | Provides |
|---|---|---|
| `openai` | `openai` | LLM chat (SSE streaming, vision, structured output), embeddings |
| `anthropic` | `anthropic` | LLM chat via the Messages API |
| `local-llm` | `local` | GGUF chat + embeddings through llama.cpp (`llm/chat@1`, `embed@1`, `gguf-runner@1`) |
| `llama-server` | `llama-server` | GGUF chat through a llama.cpp sidecar server |
| `onnx` | `silero` | VAD (Silero ONNX) |
| `whisper` | `whisper` | STT (whisper.cpp) |
| `kokoro` | `kokoro` | Local TTS (Kokoro ONNX) |
| `edge-tts` | `edge-tts` | Cloud TTS (Microsoft Edge) |
| `elevenlabs` | `elevenlabs` | Cloud TTS (ElevenLabs REST, broker-mediated) |
| `openai-tts` | `openai_tts` | Cloud TTS (OpenAI) |
| `voicevox` | `voicevox` | TTS via a VOICEVOX / Aivis Speech engine (external or managed sidecar mode) |

## Lifecycle of a plugin

```text
discover → spawn → handshake → register capabilities → health probe
              ▲                                      │
              └────────── restart (circuit breaker) ◀┘
```

1. **Discovery** — plugin binaries are found in the built-in and user
   plugin directories, filtered by `plugins.list.<name>` / `tools.list`.
2. **Spawn + handshake** — the host launches the binary and negotiates the
   IPC protocol version over stdio (length-prefixed frames, JSON handshake,
   MessagePack after v6; see
   [Plugin IPC protocol](../reference/plugin-ipc.md)).
3. **Capability registration** — the plugin advertises its tools and
   provider specs. Declared `requires`/`provides` capability strings are
   validated; a plugin whose hard requirements are unmet is disabled.
4. **Health & supervision** — the host probes provider reachability through
   the plugin itself (a minimal chat ping), watches liveness, and restarts
   with backoff while a circuit breaker trips after repeated failures.

## Capability mediation

Plugins can declare capabilities they provide to *other* plugins
(`provides = "gguf-runner@1"`) and capabilities they need
(`requires = "gguf-runner@1"`). The host mediates the calls: the caller's
declared `requires` authorize the request, the host resolves the provider
from the capability registry, and the call is forwarded over the
provider's IPC connection.

## Host services over IPC

The host exposes multiplexed **passenger services** on one shared socket:

- **`db`** — stateful plugins perform typed CRUD against the host's
  `memory.db` through `ene-plugin-db` (prefix-isolated tables per plugin,
  authenticated by token).
- **`capability`** — the capability-mediation channel described above.

## MCP servers

Model Context Protocol servers are external processes or HTTP endpoints
that expose tools. Configure them under `tools.mcp_servers`:

```json
{
  "tools": {
    "mcp_servers": [
      {
        "name": "my-server",
        "enabled": true,
        "transport": { "type": "stdio", "command": "npx", "args": ["-y", "some-mcp-server"] },
        "env_passthrough": ["MY_API_KEY"]
      }
    ]
  }
}
```

Transports: `stdio` (child process) and `http` (streamable HTTP). For
security, the child inherits **no** environment variables except the ones
listed in `env_passthrough`. See the
[MCP servers guide](../guides/tools/mcp-servers.md).

## Permissions and safety

- Tool actions declare their side effects; the host asks for approval
  before destructive operations (`PermissionRequired` event). You can
  allow once, allow for the session, or deny; standing grants are listed
  and revocable (`/permissions`, desktop permissions page).
- Tool arguments and results are redacted before they reach logs or event
  streams.
- The filesystem tool sandbox restricts paths; shell execution is a
  separate, permission-gated action.
- Plugin configuration values are redacted at the host boundary.

## Writing plugins

- [Write a tool guide](../guides/tools/write-a-tool.md) — step-by-step for
  a new tool plugin.
- [Tool SDK reference](../reference/tools/sdk.md) — `ToolAction` and
  provider traits.
- [Derive macros reference](../reference/tools/derive-macro.md) —
  attribute reference.
- [Plugin IPC protocol](../reference/plugin-ipc.md) — the wire format.

## Sandbox, broker, and approvals

Plugins never touch the OS directly. The host applies an OS sandbox
(Landlock + seccomp + rlimits on Linux, Job Object on Windows), mediates
every operation through the broker channel (`file`, `network`, `process`,
`credential`, `artifact`, `platform`), and gates requests with a two-layer
approval model (signed manifest → global/per-plugin policy). Downloads of
executable artifacts come only from a signed catalog with CAS verification.
See [Sandbox, broker & approvals](sandbox-and-approvals.md) for the full
model, including the settings UI, the audit log, and the SSRF guard.

## FAQ

**Can I write a plugin in another language?** The protocol is just framed
JSON/MessagePack over stdio, but the in-repo plugins are Rust binaries
built on `ene-plugin`. The template and SDK assume Rust.

**How do I disable a tool I don't want?** `tools.list.<name>.enable =
false` (or `plugins.list.<name>.enable = false` for provider plugins).

**What happens if a plugin crashes mid-turn?** The turn fails with an
error event; the supervisor restarts the plugin. Stateful data is
persisted in `memory.db` via the `db` passenger, not in the plugin process.
