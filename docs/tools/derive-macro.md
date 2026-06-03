# `#[derive(ToolSpec)]` Reference

The `ene-tool-derive` crate provides a proc-macro that generates `ToolSpec` implementations from declarative attributes on tool argument structs.

## Basic Usage

```rust
use ene_tool_derive::ToolSpec;
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(ToolSpec, JsonSchema, Deserialize)]
#[tool(
    namespace = "calculator",
    name = "add",
    summary = "Add two numbers together.",
    category = "Utility"
)]
pub struct AddArgs {
    /// First operand.
    pub a: f64,
    /// Second operand.
    pub b: f64,
}
```

This generates:
- `AddArgs::TOOL_NAME` = `"calculator.add"` (const `&str`)
- `AddArgs::DISPLAY_NAME` = `"Add"` (const `&str`, auto Title-Case)
- `AddArgs::SUMMARY` = `"Add two numbers together."` (const `&str`)
- `AddArgs::spec()` → full `ToolSpec` with auto-generated JSON Schema

## Required Attributes

| Attribute | Type | Description |
|-----------|------|-------------|
| `name` | string | Short action name (e.g. `"read"`) or full name when no `namespace` |
| `summary` | string | One-line summary used as the primary embedding field |
| `category` | ident | `ToolCategory` variant: `Filesystem`, `Shell`, `Browser`, `App`, `WebSearch`, `WebFetch`, `Utility`, `Memory`, `Search`, `Meta` |

## Optional Attributes

| Attribute | Type | Default | Description |
|-----------|------|---------|-------------|
| `namespace` | string | — | Namespace prefix. Full name = `"{namespace}.{name}"` |
| `display_name` | string | Title-Case of `name` | Human-friendly display name |
| `description` | string | Same as `summary` | Full markdown description |
| `version` | string | `"1.0.0"` | Semantic version |
| `side_effects` | ident | `ReadOnly` | `ReadOnly`, `Destructive`, `Idempotent`, or qualified path |

## Keyword Attributes

Comma-separated strings, each split on `,` and trimmed:

| Attribute | Weight | Description |
|-----------|--------|-------------|
| `keywords_primary` | 1.0 | High-weight terms for Tool RAG |
| `keywords_secondary` | 0.6 | Mid-weight terms |
| `keywords_domain` | 0.3 | Domain tags (language, framework, platform) |
| `keywords_negative` | -0.5 | Negative terms — penalize when present in query |

## Metadata Attributes

Comma-separated strings:

| Attribute | Description |
|-----------|-------------|
| `caveats` | Caveats the LLM should be aware of |
| `preconditions` | Preconditions that must hold before invocation |
| `related` | Names of related/complementary tools |

## Examples Attribute

Semicolon-separated list, each entry: `description|input|optional_output`.

```rust
#[tool(
    examples = "Add 2 and 3|{ \"a\": 2, \"b\": 3 }|2 + 3 = 5; Add 0 and 0|{ \"a\": 0, \"b\": 0 }"
)]
```

- Input is parsed as JSON; falls back to string on parse failure
- Use `s:` prefix to force string treatment: `s:{"a":1}`
- Use `j:` prefix to force JSON parsing: `j:{"a":1}`

## Generated Code

For a struct `AddArgs` with `namespace = "calculator"`, `name = "add"`:

```rust
impl AddArgs {
    /// Canonical tool name. Use with `ToolName::new(...)` for dispatch.
    pub const TOOL_NAME: &'static str = "calculator.add";

    /// Short human-friendly display name.
    pub const DISPLAY_NAME: &'static str = "Add";

    /// One-line summary used as the primary embedding field.
    pub const SUMMARY: &'static str = "Add two numbers together.";

    /// Construct a `ToolSpec` for this args type.
    pub fn spec() -> ToolSpec {
        // Auto-generates JSON Schema via schemars
        // Populates all fields from #[tool(...)] attributes
    }
}
```

## JSON Schema Generation

The derive macro uses `schemars` to auto-generate JSON Schema from the struct's fields. To get the best schema:

- Use `f64` for numbers, `String` for strings, `bool` for booleans, `i64`/`u64` for integers
- Use `Option<T>` for optional parameters
- Add `///` doc comments on fields — they become `description` in the schema
- Use `#[schemars(...)]` attributes for additional constraints (min, max, enum, etc.)

```rust
#[derive(ToolSpec, JsonSchema, Deserialize)]
#[tool(namespace = "example", name = "demo", summary = "Demo tool.", category = "Utility")]
pub struct DemoArgs {
    /// Required string parameter.
    pub name: String,
    /// Optional integer with default.
    #[serde(default)]
    pub count: Option<u64>,
}
```

Generated schema:
```json
{
  "type": "object",
  "properties": {
    "name": { "type": "string", "description": "Required string parameter." },
    "count": { "type": ["integer", "null"], "description": "Optional integer with default.", "minimum": 0 }
  },
  "required": ["name"]
}
```

## Dispatch Pattern

Use `TOOL_NAME` constants for dispatch in `call_tool`:

```rust
async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
    match name {
        AddArgs::TOOL_NAME => {
            let args: AddArgs = serde_json::from_str(arguments)?;
            Ok(format!("{} + {} = {}", args.a, args.b, args.a + args.b))
        }
        SubtractArgs::TOOL_NAME => {
            let args: SubtractArgs = serde_json::from_str(arguments)?;
            Ok(format!("{} - {} = {}", args.a, args.b, args.a - args.b))
        }
        _ => Err(ToolError::NotFound { tool_name: name.to_string() }),
    }
}
```

## Dependencies

```toml
[dependencies]
ene-tool-proto = { git = "https://github.com/pexisgle/Ene" }
ene-tool-derive = { git = "https://github.com/pexisgle/Ene" }
schemars = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```
