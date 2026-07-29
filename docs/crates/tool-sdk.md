# Tool SDK Crates

> **Crates**: `ene-tool-sdk` | `ene-plugin-db` | `ene-tool-macros` | `ene-tool-rag`

Helper library crates supporting tool-plugin authoring, stateful storage access for tool binaries, proc-macro derives for tool boilerplate, and retrieval-augmented tool discovery.

---

## Architectural boundaries

- `ene-tool-sdk` provides the `ToolAction` trait, `ActionSetProvider`, and a `prelude`; it depends on `ene-plugin` (which in turn depends only on `ene-plugin-proto`) — it does not depend on `ene-runtime`, `ene-mind`, or `ene-store`.
- `ene-plugin-db` is the IPC *client* used by stateful tool binaries (e.g. filesystem undo ledger, todo store); it talks to the host's `db_server` (owned by `ene-store`) over a socket rather than opening its own database connection — stateful tool binaries never become a second SQLite writer.
- `ene-tool-macros` is proc-macro only: it generates `ToolSpec`/`ToolAction` boilerplate from `#[tool(...)]`/`#[arg(...)]` attributes at compile time and has no runtime logic of its own.
- `ene-tool-rag` depends on `ene-ai` (embeddings), `ene-store` (index storage), and `ene-plugin-proto` (tool types); it is consumed by `ene-runtime` to select which tools to inject into a prompt packet under token budget pressure.

## Design rationale

- **Why derive macros for tool definitions**: each tool action needs a JSON schema, argument deserialization, and `ToolAction`/`ToolSpec` glue that would otherwise be repeated by hand for every action; `#[derive(ToolAction)]` generates it from the struct's fields and `#[tool(...)]`/`#[arg(...)]` attributes, so authors write only the `run` method.
- **Why retrieval-augmented tool selection exists**: as the number of available tools grows, sending every tool's full schema on every turn would consume a large, and unbounded, share of the prompt token budget. `ene-tool-rag` indexes tool descriptions/parameters and uses embedding similarity plus optional LLM reranking to inject only the tools relevant to the current turn.

## API reference

Struct and method signatures are not duplicated here — they drift. Generate rustdoc for the authoritative, current API:

```sh
cargo doc -p ene-tool-sdk --open
cargo doc -p ene-plugin-db --open
cargo doc -p ene-tool-macros --open
cargo doc -p ene-tool-rag --open
```

Start at `ene_tool_sdk::prelude` for authoring, and the `ToolAction`/`ToolSpec` derive macros in `ene-tool-macros`.

---

## Related
- [Plugins & MCP System](../concepts/plugins-and-mcp.md)
- [Plugin System Crates](plugin-system.md)
