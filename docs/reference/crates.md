# Crate reference

This page is the map of every crate, app, and plugin binary in the workspace,
with the dependency rules that keep the architecture intact. For how the
pieces work together, see [Architecture](../concepts/architecture.md).
Signatures live in rustdoc (`cargo doc -p <crate> --open`), not here.

## Applications

| Package | Path | Role |
|---|---|---|
| `ene-daemon` | `apps/ene-core` (binary `ene-core`) | Core daemon: data-dir lock, HTTP/WS API, session + kernel + companion + work + plane + fiber |
| `ene-stage` | `apps/ene-stage` | Product GUI: wgpu overlay, chat, 9-section detail, tray; surface + detail sockets (`client_id = stage`) |
| `ene-ctl` | `apps/ene-ctl` | CLI client for the same HTTP/WS API |
| `ene-desktop` | `apps/ene-desktop` | Frozen pre-redesign GUI restored in #794. No new features; delete when stage is judged to replace it |

## Library crates

| Crate | Role | Key dependencies (internal) |
|---|---|---|
| `ene-session` | Append-only conversation log, usage ledger, history projection | config |
| `ene-kernel` | Dialogue lane: prompt / steer / follow_up / abort / compact, visibility, observability | config, session |
| `ene-companion` | Soul, affect, memory, inner channel, proactive speech, character packages | card, config, plane, registry, session |
| `ene-body` | Performance queue, emotion-to-expression mapping, duplex voice | config, session |
| `ene-work` | Delegation, jobs (job-lane runner), schedules, skills (catalog/active context, bookmark workflow), MCP bindings | companion, kernel, plane, registry, session |
| `ene-plane` | Approval plane, hash-chained audit log, credential vault | config |
| `ene-fiber` | Plugin fiber composition: reversible effects, profile reconcile, sandbox spawn | plugin-ipc, registry, sandbox, kernel |
| `ene-registry` | Unified tool registry: side_effects filter, deny-by-default pipeline, lexical tool discovery index | plugin-ipc, plane |
| `ene-plugin-ipc` | Split plugin IPC: length-prefixed MessagePack frames for core and tool subprotocols | (nothing internal) |
| `ene-provider-assets` | Shared provider asset catalog, manifests, and verified downloads | config, plugin-ipc |
| `ene-api` | HTTP/WS types, OpenAPI document, typed Rust client | (nothing internal) |
| `ene-card` | Character Card V3 / PNG / CHARX import | config |
| `ene-config` | Settings load/save/schema, paths, `define_config!` | (nothing internal) |
| `ene-sandbox` | OS sandbox primitives (Landlock + seccomp + rlimits on Linux) | (nothing internal) |
| `ene-vrm` | VRM 1.0 loader + wgpu renderer | (nothing internal) |

## Dependency rules (enforced by review)

```text
ene-session     ↛ kernel, companion, work, daemon
ene-kernel      ↛ companion, work, fiber, daemon
ene-fiber       → kernel (shared `LoopHooks` only; kernel still ↛ fiber)
ene-companion   ↛ daemon, fiber
ene-plugin-ipc  ↛ business logic
ene-card        → ene-config only (never the reverse)
ene-vrm         ↛ kernel, companion, work, session
ene-api         ↛ daemon types
```

Clients talk to the daemon only through `ene-api`. Do not link `ene-daemon`
from `ene-desktop` or `ene-stage` production code (`ene-ctl` tests may spawn
the daemon).

Which client is the product GUI, and how old tools map onto the current
tree, is recorded in [Product boundaries](../concepts/product-boundaries.md).

## Plugin binaries

### Tool plugins (`plugins/tool/*`)

`fs`, `exec`, `web`, `utility`, `app`, `mcp` — see [Built-in tools](../guides/tools/builtin-tools.md)
and [MCP servers](../guides/tools/mcp-servers.md).
`exec` is a separate plugin from `fs` (D-24).

A Python dummy (`plugins/tool/dummy-py`) exists only as an IPC fixture and is
excluded from the Cargo workspace.

### Provider plugins (`plugins/provider/*`)

The workspace currently ships `openai-compat`, `anthropic`, `gguf`, `elevenlabs`,
`voicevox`, and `edge-tts`. They run out of process through the same plugin IPC
and expose their capabilities through seams such as LLM, embedding, TTS, or STT.
