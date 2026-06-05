# Derive Macro Reference

The `ene-tool-derive` crate provides two proc macros for building tools with minimal boilerplate.

## `#[derive(ToolAction)]` (Recommended)

The single derive macro that generates everything: `ToolSpec` metadata, JSON Schema, and a full `ToolAction` impl. You write the business logic in `async fn run(&self)`.

### Basic Example

```rust
use ene_tool_common::prelude::*;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "greeter",
    name = "hello",
    summary = "Returns a greeting for the given name.",
    category = "Utility",
    keywords_primary = "greeting, hello",
    side_effects = "ReadOnly"
)]
pub struct HelloAction {
    /// Name to greet.
    name: String,
}

impl HelloAction {
    async fn run(&self) -> Result<String, ToolError> {
        Ok(format!("Hello, {}!", self.name))
    }
}
```

This generates:
- `HelloAction::TOOL_NAME` = `"greeter.hello"` (const `&str`)
- `HelloAction::DISPLAY_NAME` = `"Hello"` (const `&str`)
- `HelloAction::SUMMARY` = `"Returns a greeting for the given name."` (const `&str`)
- `HelloAction::spec()` → full `ToolSpec` with auto-generated JSON Schema
- `impl ToolAction for HelloAction` with `name()`, `definition()`, `execute()`
- `execute()` deserializes JSON into `Self`, then calls `self.run().await`

### Stateful Actions with `#[tool(skip)]`

Fields marked `#[tool(skip)]` are hidden from the JSON Schema, not deserialized from user input, and copied from `self` during `execute()`. Use this for injected dependencies (sandbox, database, HTTP client, session store).

```rust
use ene_tool_common::prelude::*;
use std::sync::Arc;

fn default_store() -> Arc<BrowserSessionStore> {
    Arc::new(BrowserSessionStore::new())
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "browser",
    name = "click",
    summary = "Clicks a page element matching the selector.",
    category = "Browser",
    keywords_primary = "click, element"
)]
pub struct ClickAction {
    /// CSS selector for the element to click.
    selector: String,

    #[tool(skip)]
    #[serde(skip, default = "default_store")]
    store: Arc<BrowserSessionStore>,
}

impl ClickAction {
    pub fn new(store: Arc<BrowserSessionStore>) -> Self {
        Self {
            selector: String::new(),
            store,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        // self.store is available here
        Ok(format!("Clicked {}", self.selector))
    }
}
```

**Rules for `#[tool(skip)]` fields:**
1. Add `#[serde(skip, default = "fn_name")]` alongside `#[tool(skip)]`
2. The default function must return the same type as the field
3. The field is cloned from the provider's instance into the deserialized args in `execute()`

### Provider Integration

```rust
struct MyProvider {
    store: Arc<BrowserSessionStore>,
}

impl ToolProvider for MyProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        vec![ClickAction::new(self.store.clone()).definition()]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            ClickAction::TOOL_NAME => {
                let action = ClickAction::new(self.store.clone());
                action.execute(arguments).await
            }
            _ => Err(ToolError::NotFound { tool_name: name.to_string() }),
        }
    }
}
```

## `#[derive(ToolSpec)]` (Lower-Level)

Generates only the `ToolSpec` metadata and JSON Schema — no `ToolAction` impl. Use this when you need the spec but want to write your own `execute()` manually.

```rust
#[derive(ToolSpec, JsonSchema, Deserialize)]
#[tool(namespace = "calculator", name = "add", summary = "Add two numbers.", category = "Utility")]
pub struct AddArgs {
    pub a: f64,
    pub b: f64,
}
```

## Required `#[tool(...)]` Attributes

| Attribute | Type | Description |
|-----------|------|-------------|
| `name` | string | Short action name (e.g. `"read"`) or full name when no `namespace` |
| `summary` | string | One-line summary used as the primary embedding field |
| `category` | ident | `ToolCategory` variant: `Filesystem`, `Shell`, `Browser`, `App`, `WebSearch`, `WebFetch`, `Utility`, `Memory`, `Search`, `Meta` |

## Optional `#[tool(...)]` Attributes

| Attribute | Type | Default | Description |
|-----------|------|---------|-------------|
| `namespace` | string | — | Namespace prefix. Full name = `"{namespace}.{name}"` |
| `display_name` | string | Title-Case of `name` | Human-friendly display name |
| `description` | string | Same as `summary` | Full markdown description |
| `version` | string | `"1.0.0"` | Semantic version |
| `side_effects` | ident | `ReadOnly` | `ReadOnly`, `Destructive`, `Idempotent`, or qualified path |

## Keyword Attributes

Comma-separated strings:

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

## Per-Field `#[arg(...)]` Attributes

| Attribute | Description |
|-----------|-------------|
| `internal` / `hidden` | Remove field from JSON Schema properties and required list |
| `skip` | Alias for `internal` (same as `#[tool(skip)]` on a field) |
| `enum_values = "a, b, c"` | Add `enum` constraint to schema |
| `default = "value"` | Add `default` to schema (numbers, bools, strings) |
| `minimum = 0` / `maximum = 100` | Numeric constraints |
| `min_length` / `max_length` | String length constraints |
| `min_items` / `max_items` | Array length constraints |
| `description = "..."` | Override the doc-comment description |

## JSON Schema Generation

The derive macros use `schemars` to auto-generate JSON Schema. Tips:

- Use `f64` for numbers, `String` for strings, `bool` for booleans, `i64`/`u64` for integers
- Use `Option<T>` for optional parameters
- Add `///` doc comments on fields — they become `description` in the schema
- `#[serde(rename = "...")]` changes the JSON property key
- `#[serde(alias = "...")]` values are appended to the description as "Aliases: ..."
- `additionalProperties: false` is always set on the root object

## Dependencies

```toml
[dependencies]
ene-tool-common = { path = "../common" }
ene-tool-proto = { path = "../../crates/ene-tool-proto" }
ene-tool-derive = { path = "../../crates/ene-tool-derive" }
schemars = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
async-trait = "0.1"
```
