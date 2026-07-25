# `ene-plugin` — API Reference

> **Crate:** `ene-plugin`
> **Role:** Plugin authoring facade (API v1). Preferred import path for new tool/plugin binaries.
> **Does not depend on:** `ene-runtime`, `ene-mind`, or `ene-store`.

---

## Overview

`ene-plugin` re-exports the wire, host-adapter, and derive layers used by tool binaries:

| Layer | Types | Source crate |
|---|---|---|
| Wire (IPC / sandbox) | `ToolProvider`, `IpcRequest` / `IpcResponse`, `ToolSpec` | `ene-plugin-proto` |
| Server / registry | `run_tool_server`, `HostRegistry` | `ene-plugin` (this crate) |
| Host adapters | `ActionSetProvider`, `SingleActionProvider`, `ToolAction` | `ene-tool-common` |
| Derive | `#[derive(ToolSpec)]`, `#[derive(ToolAction)]` | `ene-tool-derive` |

Host aggregation (`ToolRegistry`, MCP, composite tools, Tool RAG) stays in [`ene-plugin-host`](ene-plugin-host.md). A full physical merge of the leaf crates into this facade is a follow-up; until then, the leaf crates remain in the workspace and this facade is the supported import path.

## Two-layer contract

- **Wire:** tool binaries implement `ToolProvider` and speak handshake / list / call / permission-user-input continuation / shutdown.
- **Host:** `ene-plugin-host::ToolRegistry` aggregates IPC + MCP.
- **Name collision:** is a hard error at every registry layer (`HostRegistry` returns `ToolError::DuplicateName`; `CompositeToolRegistry` returns `ToolHostError::DuplicateToolName`).
- **`ToolSpec`** is LLM-facing: `name`, `description`, `parameters`.
- **`ToolRagProfile`** is host/RAG-only (#137): keywords, examples, category, etc. — never passed to the LLM tool list.

```rust,ignore
use ene_tool_common::prelude::*;
```

## See Also

- [`ene-plugin-proto`](ene-plugin-proto.md) — IPC wire protocol details
- [`ene-tool-common`](ene-tool-common.md) — `ToolAction` helpers
- [`ene-tool-derive`](ene-tool-derive.md) — derive macros
- [`ene-plugin-host`](ene-plugin-host.md) — process manager and Tool RAG
- [Tool System Overview](../tools/overview.md)
- [API v1](../architecture/api-v1.md)
