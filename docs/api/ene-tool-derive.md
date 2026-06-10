# `ene-tool-derive`

> Proc-macros for generating `ToolSpec` metadata and `ToolAction` implementations from annotated structs.

`ene-tool-derive` provides three macros that eliminate boilerplate when writing Ene tools:

| Macro | Kind | Purpose |
|---|---|---|
| `#[derive(ToolSpec)]` | Derive | Generate `impl ToolSpecArgs` (metadata only) |
| `#[derive(ToolAction)]` | Derive | Generate `impl ToolSpecArgs` **+** `impl ToolAction` with `execute()` |
| `#[tool_action(args = T)]` | Attribute | Fill in `name()` and `definition()` on a manual `impl ToolAction` |

In the vast majority of cases you should use `#[derive(ToolAction)]`, which subsumes `#[derive(ToolSpec)]` and generates the complete implementation.

---

## `#[derive(ToolSpec)]`

Generates:
- `impl ToolSpecArgs for MyArgs`
- `const TOOL_NAME: &'static str`
- `const DISPLAY_NAME: &'static str`
- `const SUMMARY: &'static str`
- `fn spec() -> ToolSpec`

The JSON Schema for `parameters` is derived from the struct fields using `schemars`. The macro automatically sets `additionalProperties: false` on the root schema object.

### Container attributes — `#[tool(...)]`

Place these on the **struct itself**:

| Attribute | Type | Required | Description |
|---|---|---|---|
| `namespace` | `&str` | Yes | Namespace prefix for the tool name (e.g. `"fs"`). |
| `name` | `&str` | Yes | Action name within the namespace (e.g. `"read_file"`). The full name becomes `namespace.name`. |
| `display_name` | `&str` | No | Human-readable name. Defaults to a title-cased version of `name`. |
| `summary` | `&str` | Yes | One-sentence description for the LLM tool list. |
| `description` | `&str` | No | Longer description for the tool detail view. Defaults to `summary`. |
| `category` | `&str` | No | Category string (e.g. `"Filesystem"`, `"Web"`). |
| `side_effects` | `&str` | No | One of `"None"`, `"ReadOnly"`, `"Writes"`, `"Network"`, `"Destructive"`. Defaults to `"None"`. |
| `version` | `&str` | No | Semver string. Defaults to `"0.1.0"`. |
| `keywords_primary` | `&str` | No | Comma-separated primary keywords. |
| `keywords_secondary` | `&str` | No | Comma-separated secondary keywords. |
| `keywords_domain` | `&str` | No | Comma-separated domain tags. |
| `keywords_negative` | `&str` | No | Comma-separated negative keywords. |
| `examples` | `&str` | No | JSON literal (array of example invocations). |
| `caveats` | `&str` | No | Pipe-separated (`\|`) list of caveat strings. |
| `preconditions` | `&str` | No | Pipe-separated list of precondition strings. |
| `related` | `&str` | No | Comma-separated list of related tool names. |

### Field attributes — `#[arg(...)]` / `#[tool(...)]`

Place these on individual **struct fields**:

| Attribute | Description |
|---|---|
| `#[arg(description = "…")]` | Sets the JSON Schema `description` for this field. Equivalent to a `///` doc comment. |
| `#[arg(hidden)]` or `#[arg(internal)]` | Excludes the field from the JSON Schema (and thus from the LLM's view). |
| `#[arg(enum_values = "a, b, c")]` | Sets the JSON Schema `enum` constraint. |
| `#[arg(default = "value")]` | Sets the JSON Schema `default`. |
| `#[arg(minimum = N)]` / `#[arg(maximum = N)]` | Numeric range constraints. |
| `#[arg(min_length = N)]` / `#[arg(max_length = N)]` | String length constraints. |
| `#[arg(min_items = N)]` / `#[arg(max_items = N)]` | Array length constraints. |
| `#[tool(skip)]` | Field is excluded from JSON Schema **and** from deserialization. Its value is copied from `self` into the deserialized args before `run()` is called. Used for injected context (e.g. `Arc<SharedContext>`). |

> [!IMPORTANT]
> Fields marked `#[tool(skip)]` must also be annotated with `#[serde(skip, default)]` so that `serde` does not attempt to deserialize them from the LLM arguments JSON.

---

## `#[derive(ToolAction)]`

Combines `#[derive(ToolSpec)]` with a full `impl ToolAction`. You provide the execution logic by writing an **`async fn run(&self)`** method in a plain `impl` block.

The generated `execute()` method:
1. Deserializes the JSON `arguments` string into `Self`.
2. Copies all `#[tool(skip)]` fields from the receiver into the deserialized instance.
3. Calls `self.run().await` on the populated instance.
4. Returns the `Result<String, ToolError>`.

### Required derives

Your struct must also derive:

```rust
#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
```

- `Deserialize` — for step 1 above.
- `JsonSchema` — for generating the `parameters` schema.

### Full example

```rust
use std::sync::Arc;
use ene_tool_common::prelude::*;

/// Shared state injected by the tool server.
pub struct SharedContext {
    pub base_dir: std::path::PathBuf,
}

#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "my_ns",
    name = "do_thing",
    display_name = "Do Thing",
    summary = "Performs the thing on a given path.",
    description = "Reads the file at `path` and returns its content. \
                   Works on text files only.",
    category = "Utility",
    side_effects = "ReadOnly",
    keywords_primary = "thing, do, operate",
    keywords_domain = "filesystem",
    caveats = "Binary files are not supported | Max 1 MB",
)]
pub struct DoThingAction {
    /// The path to the file to read. Must be absolute.
    pub path: String,

    /// Maximum bytes to return. Defaults to 65536.
    #[arg(default = "65536", minimum = 1, maximum = 1048576)]
    pub max_bytes: Option<u64>,

    // Injected by the tool server — invisible to the LLM.
    #[tool(skip)]
    #[serde(skip, default)]
    pub context: Arc<Option<SharedContext>>,
}

impl DoThingAction {
    async fn run(&self) -> Result<String, ToolError> {
        let limit = self.max_bytes.unwrap_or(65_536);
        let content = tokio::fs::read_to_string(&self.path)
            .await
            .map_err(|e| ToolError::IoError { message: e.to_string() })?;

        let output = content.truncate_chars(limit as usize);
        Ok(output.content.to_string())
    }
}
```

### Wiring the binary

```rust
// tools/my_tool/src/main.rs
use ene_tool_proto::run_tool_server;
mod actions;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run_tool_server::<actions::DoThingAction>().await
}
```

---

## `#[tool_action(args = T)]`

An attribute macro for cases where you cannot use `#[derive(ToolAction)]` — for example, when `execute()` needs to be `async` at the trait level without the derive macro's wrapper, or when you are implementing the trait for a type you do not own.

```rust
#[tool_action(args = DoThingAction)]
impl ToolAction for DoThingWrapper {
    fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        // The macro fills in `name()` and `definition()` automatically.
        // You only write `execute()`.
        todo!()
    }
}
```

The macro reads `DoThingAction::TOOL_NAME` and `DoThingAction::spec()` (from `ToolSpecArgs`) and generates matching `name()` and `definition()` bodies.

---

## Generated schema notes

- The root JSON Schema is always `{ "type": "object", "additionalProperties": false, … }`.
- Rust doc comments (`///`) on fields are used as the JSON Schema `description` if no `#[arg(description = "…")]` is present.
- `Option<T>` fields generate a schema where the property is not in `required`.
- `Vec<T>` fields generate `{ "type": "array", "items": … }`.

---

## Related Pages

- [`ene-tool-common`](ene-tool-common.md) — `ToolAction` and `ToolSpecArgs` trait definitions
- [`ene-tool-proto`](ene-tool-proto.md) — `ToolSpec` structure
- [Writing a Tool](../tools/sdk.md) — End-to-end tutorial
