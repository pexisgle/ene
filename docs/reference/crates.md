# Crate reference

This page is the map of every crate, app, and plugin binary in the workspace,
with the dependency rules that keep the architecture intact. For how the
pieces work together, see [Architecture](../concepts/architecture.md).
Signatures live in rustdoc (`cargo doc -p <crate> --open`), not here.

## Applications

| Package | Path | Role |
|---|---|---|
| `ene-daemon` | `apps/ene-core` (binary `ene-core`) | Core daemon: data-dir lock, HTTP/WS API, session + kernel + companion + work + plane + fiber |
| `ene-ctl` | `apps/ene-ctl` | CLI client for the same HTTP/WS API |
| `ene-stage` | `apps/ene-stage` | Product GUI: wgpu overlay, chat, 8-section detail, tray; surface + detail sockets |

## Library crates

| Crate | Role | Key dependencies (internal) |
|---|---|---|
| `ene-session` | Append-only conversation log, usage ledger, history projection | config |
| `ene-kernel` | Dialogue lane: prompt / steer / follow_up / abort / compact, visibility, observability | config, session |
| `ene-companion` | Soul, affect, memory, inner channel, proactive speech, character packages | card, config, plane, registry, session |
| `ene-body` | Performance queue, emotion-to-expression mapping, duplex voice | config, session |
| `ene-work` | Delegation, jobs (job-lane runner), schedules, skills, MCP bindings | companion, kernel, plane, registry, session |
| `ene-plane` | Approval plane, hash-chained audit log, credential vault | config |
| `ene-fiber` | Plugin fiber composition: reversible effects, profile reconcile, sandbox spawn | plugin-ipc, registry, sandbox |
| `ene-registry` | Unified tool registry: side_effects filter, deny-by-default pipeline | plugin-ipc, plane |
| `ene-plugin-ipc` | Split plugin IPC: length-prefixed MessagePack frames for core and tool subprotocols | (nothing internal) |
| `ene-api` | HTTP/WS types, OpenAPI document, typed Rust client | (nothing internal) |
| `ene-card` | Character Card V3 / PNG / CHARX import | config |
| `ene-config` | Settings load/save/schema, paths, `define_config!` | (nothing internal) |
| `ene-sandbox` | OS sandbox primitives (Landlock + seccomp + rlimits on Linux) | (nothing internal) |
| `ene-vrm` | VRM 1.0 loader + wgpu renderer | (nothing internal) |

## Dependency rules (enforced by review)

```text
ene-session     ↛ kernel, companion, work, daemon
ene-kernel      ↛ companion, work, fiber, daemon
ene-companion   ↛ daemon, fiber
ene-plugin-ipc  ↛ business logic
ene-card        → ene-config only (never the reverse)
ene-vrm         ↛ kernel, companion, work, session
ene-api         ↛ daemon types
```

Clients talk to the daemon only through `ene-api`. Do not link `ene-daemon`
from `ene-stage` production code (`ene-ctl` tests may spawn the daemon).

## Plugin binaries

### Harness tools (`plugins/harness/*`)

`fs`, `exec`, `web`, `utility` — see [Built-in tools](../guides/tools/builtin-tools.md).
`exec` is a separate plugin from `fs` (D-24).

A Python dummy (`plugins/tool/dummy-py`) exists only as an IPC fixture and is
excluded from the Cargo workspace.
