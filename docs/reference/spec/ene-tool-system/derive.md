# `ToolSpec` & `ToolAction` Derive Macro Specs (`ene-tool-derive`)

The `ene-tool-derive` crate contains procedural macros that compile structs into function schema maps (`ToolSpec`) and execution interfaces (`ToolAction`) for LLM interactions.

---

## 1. `#[derive(ToolSpec)]` Specifications

This macro parses structured attributes and doc comments, compiling them into a JSON Schema definition:

### Schema Compilation Steps
1.  **Extract Base Schema**: Utilizes `schemars` to read syn field types (e.g. `String`, `f32`) and builds raw JSON Schema properties.
2.  **Enforce Strict Properties**:
    To prevent the LLM from inventing parameters, the macro forces `additionalProperties: false` on the root of the schema.
3.  **Reflect Meta Attributes**: Parses `#[tool(...)]` and `#[arg(...)]` properties, generating RAG category weights and parameter prompts.
4.  **Emit Constants**:
    Emits the following string literal variables to prevent spelling typos across dispatch tables and tests:
    -   `pub const TOOL_NAME: &'static str = "namespace.name";`
    -   `pub const DISPLAY_NAME: &'static str = "...";`

---

## 2. Attributes Reference

### 1. Container Attribute `#[tool(...)]` (On Struct)
*   `namespace: String`: Name category prefix (e.g. `fs`, `web`).
*   `name: String`: Specific action name (e.g. `read`, `write`).
*   `summary: String`: One-liner describing tool usage.
*   `category: String`: Categorization header (e.g. `Filesystem`, `Browser`).
*   `side_effects: bool`: True if the action modifies states or writes files.
*   `sandbox_required: bool`: True if sandbox boundaries are required.
*   `keywords_primary / keywords_secondary`: Primary/secondary positive matches for RAG.
*   `keywords_negative`: Negative words to penalize or skip this tool.

### 2. Parameter Attribute `#[arg(...)]` / `#[tool(...)]` (On Fields)
*   `description: String`: Argument explanation (defaults to field's doc comments).
*   `enum_values: Vec<String>`: Confines parameters to a list of allowed values.
*   `min / max`: Bounds numerical parameters.
*   `skip`: **Excludes** the field from LLM-facing schemas and JSON serialization.

---

## 3. `#[derive(ToolAction)]` Specifications & Flow

The `ToolAction` macro implements `ene_tool_common::ToolAction` on the decorated struct:

### Execution Pipeline (`execute`)
```rust
async fn execute(&self, arguments_json: &str) -> Result<String, ToolError> {
    // 1. Deserialize parameters from the LLM-supplied JSON
    let mut args: Self = serde_json::from_str(arguments_json)?;

    // 2. Clone stateful skipped fields (such as sandboxes or database sockets)
    //    from self to the newly deserialized struct
    args.sandbox = self.sandbox.clone();

    // 3. Delegate to the user-defined async run method
    args.run().await
}
```
*   **Obligation**: The tool developer must define `async fn run(&self) -> Result<String, ToolError>` in a separate `impl` block.
