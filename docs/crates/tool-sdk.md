# Tool SDK Crates — API Reference

> **Crates**: `ene-tool-sdk` | `ene-plugin-db` | `ene-tool-macros` | `ene-tool-rag`

Helper library crates designed to support tool plugin creation, stateful storage access, proc-macro derives, and retrieval-augmented tool discovery.

---

## 1. `ene-tool-sdk` (Tool Plugin Authoring SDK)

Shared traits (`ToolAction`, `ActionSetProvider`), prelude types, and helpers (HTML-to-Markdown, truncation) used by plugins to implement tool actions.

---

## 2. `ene-plugin-db` (Stateful Plugin Database IPC Client)

Stateful tool plugins (`ene-plugin-fs`, `ene-plugin-utility`) use `ene-plugin-db` to communicate with the host's `DbServer` socket:
- `UndoManager`: Manages file modification undo stacks for `ene-plugin-fs`.
- `TodoStore`: Manages active todo item CRUD operations for `ene-plugin-utility`.

---

## 3. `ene-tool-macros` (Proc-Macros)

Provides procedural macros simplifying tool definition:
- `#[derive(ToolAction)]`: Generates `ToolSpec` metadata, JSON argument deserialization, and `execute()` glue code automatically.
- `#[derive(ToolSpec)]`: Generates `ToolSpec`/`ToolRagProfile` construction from `#[tool(...)]`/`#[arg(...)]` attributes.
- `#[tool_action(args = ...)]`: Attribute macro that fills in `name()`/`definition()`/`rag_profile()` forwarders on a hand-written `ToolAction` impl.

---

## 4. `ene-tool-rag` (Retrieval-Augmented Tool Search)

Provides multi-vector semantic and lexical tool discovery:
- Indexes tool descriptions and action parameters.
- Uses LLM reranking and weighted field similarity to inject only relevant tools into prompt packets under tight token budgets.

---

## Related Links
- [Plugins & MCP System](../concepts/plugins-and-mcp.md)
- [Plugin System Crates](plugin-system.md)
