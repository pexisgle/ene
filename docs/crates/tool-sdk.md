# Tool Authoring Crates

> **Crates**: `ene-plugin-db` | `ene-plugin-macros`

Helper library crates supporting tool-plugin authoring: stateful storage
access for tool binaries and proc-macro derives for tool (and, more broadly,
plugin) boilerplate.

The `ToolAction` trait, `ActionSetProvider`/`SingleActionProvider` adapters,
and the tool-authoring `prelude` live in **`ene-plugin`** itself (see
[Plugin System Crates](plugin-system.md)). They were merged out of the former
standalone `ene-tool-sdk` crate, which has been deleted. Retrieval-augmented
tool discovery lives in `ene-rag` (see [RAG Policy Layer](rag.md)).

---

## Architectural boundaries

- `ene-plugin` owns the tool-authoring surface (`ToolAction`, `ActionSetProvider`, `prelude::tool`). Its only workspace dependencies are `ene-plugin-proto` (wire protocol), `ene-infer` (local-inference discipline), and `ene-plugin-macros` (proc-macro derives); everything else is standard async/serialization ecosystem crates (`tokio`, `tokio-stream`, `tokio-util`, `async-trait`, `schemars`, `serde`/`serde_json`, `tracing`, `thiserror`, `parking_lot`, `base64`). It does not depend on `ene-runtime`, `ene-mind`, or `ene-store`.
- `ene-plugin-db` is the IPC *client* used by stateful tool binaries (e.g. filesystem undo ledger, todo store); it opens the host-service `db` passenger (owned by `ene-store`) rather than opening its own database connection — stateful tool binaries never become a second SQLite writer.
- `ene-plugin-macros` is proc-macro only: it generates `ToolSpec`/`ToolAction` boilerplate from `#[tool(...)]`/`#[arg(...)]` attributes at compile time and has no runtime logic of its own. Its generated code references `::ene_plugin::` paths.
- Retrieval-augmented tool selection is owned by `ene-rag` (the `tool` module), not by these SDK crates; see [RAG Policy Layer](rag.md).

## Design rationale

- **Why the tool SDK merged into `ene-plugin`**: the `ene-plugin` / `ene-tool-sdk` split was meant to separate "generic plugin facade" from "tool-specific sugar", but `ene-plugin` already exposed tool-specific types (`ToolPlugin`, `ToolProviderPlugin`) unconditionally, and after `html`/`truncate` were extracted to `ene-util` (#300) the remaining `ene-tool-sdk` added no dependency isolation. Keeping two crates only added cargo-feature management cost for zero benefit.
- **Why derive macros for tool definitions**: each tool action needs a JSON schema, argument deserialization, and `ToolAction`/`ToolSpec` glue that would otherwise be repeated by hand for every action; `#[derive(ToolAction)]` generates it from the struct's fields and `#[tool(...)]`/`#[arg(...)]` attributes, so authors write only the `run` method.
- **Why `ene-plugin-macros` is a non-optional dependency of `ene-plugin`**: the proc-macro crate is tiny, and `schemars`/`syn` are already in the graph via `ene-plugin-proto`, so feature-gating it behind a `tool` feature would buy no dependency isolation while complicating `use ene_plugin::prelude::*;` for tool plugins — zero practical benefit.
- **Why retrieval-augmented tool selection exists**: as the number of available tools grows, sending every tool's full schema on every turn would consume a large, and unbounded, share of the prompt token budget. `ene-rag`'s tool pipeline indexes tool descriptions/parameters and uses embedding similarity plus optional LLM reranking to inject only the tools relevant to the current turn.
- **Tool design philosophy — mega-tool vs individual-tool**: the tool plugin family currently uses two architectural patterns. (1) The **mega-tool approach** (fs, app, browser) ships a single binary per domain with many actions dispatched internally, minimizing process overhead and IPC round-trips, and letting actions share state within one process. (2) The **individual-tool approach** (web, utility) ships multiple smaller plugins, each with a focused responsibility, which improves semantic-matching precision in Tool RAG. A future unification to a single approach is possible but not yet decided; when designing a new tool, weigh startup overhead against retrieval precision for the specific use case.

## API reference

Struct and method signatures are not duplicated here — they drift. Generate rustdoc for the authoritative, current API:

```sh
cargo doc -p ene-plugin --open
cargo doc -p ene-plugin-db --open
cargo doc -p ene-plugin-macros --open
```

Start at `ene_plugin::prelude` for authoring, and the `ToolAction`/`ToolSpec` derive macros in `ene-plugin-macros`.

For step-by-step authoring guidance and the full API/ABI reference, see
[Write a Tool](../guide/tools/write-a-tool.md) and
[Tool SDK Reference](../reference/tools/sdk.md). A complete end-to-end
sample (DB IPC state, permission gate, IPC integration tests) ships as
`plugins/tool/counter`.

---

## Related
- [Plugins & MCP System](../concepts/plugins-and-mcp.md)
- [Plugin System Crates](plugin-system.md)
