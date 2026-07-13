# `ene-tool` — API Reference

> **Crate:** `ene-tool`
> **Role:** Facade for the tool ABI surface (API v2). Preferred import path for new tool binaries.
> **Does not depend on:** `ene-runtime`, `ene-mind`, or `ene-store`.

---

## Overview

`ene-tool` re-exports the wire, host-adapter, and derive layers used by tool binaries:

| Layer | Types | Source crate |
|---|---|---|
| Wire (IPC / sandbox) | `ToolProvider`, `IpcRequest` / `IpcResponse`, `ToolSpec`, `run_tool_server` | `ene-tool-proto` |
| Host adapters | `ActionSetProvider`, `SingleActionProvider`, `ToolAction` | `ene-tool-common` |
| Derive | `#[derive(ToolSpec)]`, `#[derive(ToolAction)]` | `ene-tool-derive` |

Host aggregation (`ToolRegistry`, MCP, composite tools, Tool RAG) stays in [`ene-tool-host`](ene-tool-host.md). A full physical merge of the leaf crates into this facade is a follow-up; until then, the leaf crates remain in the workspace and this facade is the supported import path.

## Two-layer contract

- **Wire:** tool binaries implement `ToolProvider` and speak handshake / list / call / permission-user-input continuation / shutdown.
- **Host:** `ene-tool-host::ToolRegistry` aggregates IPC + MCP.
- **Name collision** is a hard error at registry build / add time.
- **`ToolSpec`** is LLM-facing only: `name`, `description`, `parameters`.

```rust,ignore
use ene_tool::prelude::*;
```

## See Also

- [`ene-tool-proto`](ene-tool-proto.md) — IPC wire protocol details
- [`ene-tool-common`](ene-tool-common.md) — `ToolAction` helpers
- [`ene-tool-derive`](ene-tool-derive.md) — derive macros
- [`ene-tool-host`](ene-tool-host.md) — process manager and Tool RAG
- [Tool System Overview](../tools/overview.md)
- [API v2](../architecture/api-v2.md)
