# Tool SDK Crates — API Reference

> **Crates**: `ene-tool-common` | `ene-tool-db` | `ene-tool-derive` | `ene-tool-rag`

Helper library crates designed to support tool plugin creation, stateful storage access, proc-macro derives, and retrieval-augmented tool discovery.

---

## 1. `ene-tool-common` (Common Action Traits & Providers)

Shared traits (`ActionSetProvider`), prelude types, and reflection helpers used by plugins to implement tool actions.

---

## 2. `ene-plugin-db` (Stateful Plugin Database IPC Client)

Stateful tool plugins (`ene-plugin-fs`, `ene-plugin-utility`) use `ene-plugin-db` to communicate with the host's `DbServer` socket:
- `UndoManager`: Manages file modification undo stacks for `ene-plugin-fs`.
- `TodoStore`: Manages active todo item CRUD operations for `ene-plugin-utility`.

---

## 3. `ene-tool-derive` (Proc-Macro Derives)

Provides procedural macros simplifying tool definition:
- `#[derive(ToolAction)]`: Generates `ToolSpec` metadata, JSON argument deserialization, and `execute()` glue code automatically.

---

## 4. `ene-tool-rag` (Retrieval-Augmented Tool Search)

Provides multi-vector semantic and lexical tool discovery:
- Indexes tool descriptions and action parameters.
- Uses LLM reranking and weighted field similarity to inject only relevant tools into prompt packets under tight token budgets.

---

## Related Links
- [Plugins & MCP System](../concepts/plugins-and-mcp.md)
- [Plugin System Crates](plugin-system.md)
