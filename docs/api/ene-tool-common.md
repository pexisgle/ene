# `ene-tool-common` — API Reference

> **Crate:** `ene-tool-common`
> **Role:** `ToolAction`/`ToolSpecArgs` traits, shared helpers, and the standard prelude for Ene tool binaries.

---

## Overview

`ene-tool-common` is the primary dependency for any tool binary in the `tools/` workspace. It provides:

- The **`ToolSpecArgs`** trait (implemented by `#[derive(ToolSpec)]`/`#[derive(ToolAction)]`) and the **`ToolAction`** trait that every tool action implements to be dispatchable.
- A **`prelude`** module re-exporting everything a tool author needs in one `use` statement.
- HTML-to-Markdown conversion and content-extraction helpers for web-content tools.
- Re-exports of the `Truncate`/`TruncateResult` struct API from `ene-common`.

`ToolAction` is deliberately **not** a supertrait of `ToolSpecArgs` — see [`ToolAction` Trait](#toolaction-trait) for why — so a tool binary's dispatch table can hold a plain `Vec<Box<dyn ToolAction>>`.

See also: [`ene-tool-derive`](./ene-tool-derive.md) for the proc-macros that implement these traits automatically, and [`ene-tool-proto`](./ene-tool-proto.md) for the underlying wire types (`ToolSpec`, `ToolError`, `ToolProvider`, `run_tool_server`).

---

## `ToolSpecArgs` Trait

A static-dispatch interface for argument structs that describe a tool's metadata. Implemented automatically by `#[derive(ToolSpec)]` and `#[derive(ToolAction)]` — you should not implement it by hand.

```rust
pub trait ToolSpecArgs: DeserializeOwned + Send + Sync + 'static {
    /// Canonical tool name (e.g. `"app.press_key"`).
    const TOOL_NAME: &'static str;

    /// Returns the LLM-facing `ToolSpec` for this args type.
    fn spec() -> ToolSpec;
}
```

| Member | Description |
|---|---|
| `const TOOL_NAME: &'static str` | The only associated constant on the trait. |
| `fn spec() -> ToolSpec` | Builds the full `ToolSpec` for this args type. |

> **Note:** `DISPLAY_NAME` and `SUMMARY` are **not** part of this trait. The `#[derive(ToolSpec)]` macro does generate `pub const DISPLAY_NAME: &'static str` and `pub const SUMMARY: &'static str` on the struct — but as plain **inherent constants**, not trait members. Access them as `MyArgs::DISPLAY_NAME` / `MyArgs::SUMMARY`, not through a `ToolSpecArgs` bound.

You rarely need to call `TOOL_NAME`/`spec()` directly; they are consumed internally by the generated `ToolAction` implementation.

---

## `ToolAction` Trait

The core trait implemented by every executable tool action and used for dynamic dispatch (`Box<dyn ToolAction>`) inside a `ToolProvider`.

```rust
#[async_trait]
pub trait ToolAction: Send + Sync {
    /// Returns the canonical tool name. Implement as `MyArgs::TOOL_NAME`.
    fn name(&self) -> &'static str;

    /// Returns the metadata definition of this tool. Implement as
    /// `MyArgs::spec()`.
    fn definition(&self) -> ToolSpec;

    /// Executes the action with a JSON argument string.
    async fn execute(&self, arguments: &str) -> Result<String, ToolError>;
}
```

### Method Table

| Method | Signature | Description |
|---|---|---|
| `name` | `fn name(&self) -> &'static str` | Canonical tool name. Implementations forward to `Args::TOOL_NAME`. |
| `definition` | `fn definition(&self) -> ToolSpec` | LLM-facing metadata. Implementations forward to `Args::spec()`. |
| `execute` | `async fn execute(&self, arguments: &str) -> Result<String, ToolError>` | **Async.** Executes the action with a JSON-encoded argument string, sourced from the LLM's tool call. |

`ToolAction` intentionally does **not** extend `ToolSpecArgs`: keeping it a plain `Send + Sync` trait (rather than `ToolSpecArgs + Send + Sync`) keeps `dyn ToolAction` object-safe and lets a single `Vec<Box<dyn ToolAction>>` hold actions backed by different, unrelated `Args` types. The convention is that `name()` and `definition()` are one-line forwarders to the args struct's `TOOL_NAME` const and `spec()` method, which guarantees the spec name and the dispatch name are the same `&'static str` by construction.

The `#[derive(ToolAction)]` macro from `ene-tool-derive` generates the entire impl — including an `async` `execute` that deserializes JSON into `Self`, copies any `#[tool(skip)]` fields, and calls `self.run().await` — leaving only the `run` body to write. See [`ene-tool-derive`](./ene-tool-derive.md).

---

## `prelude` Module

Import everything you need in one line:

```rust,no_run
use ene_tool_common::prelude::*;
```

This re-exports:

| Item | Source |
|---|---|
| `async_trait` | `async-trait` crate (attribute macro) |
| `ToolAction` (trait, brought into scope unqualified via `as _`) | This crate |
| `ToolSpec`, `tool_action`, `ToolSpec` (derive macro), `ToolAction` (derive macro) | `ene-tool-derive` |
| `ToolError` | `ene-tool-proto` |
| `JsonSchema` | `schemars` derive macro |
| `Deserialize` | `serde` derive macro |

> **Note:** All re-exports from other workspace crates are annotated with `#[doc(no_inline)]` so that rustdoc links resolve to the original crate's documentation.

---

## `truncate` Module

Re-exports the `Truncate` struct API from `ene-common` for use in tool output formatting:

```rust
pub mod truncate {
    pub use ene_common::truncate::{Truncate, TruncateResult};
}
```

`Truncate` is a **unit struct with static methods** — it is not a trait, and there is nothing to call directly on `&str`/`String`. See [`ene-common`](./ene-common.md) for the full method reference (`Truncate::simple`, `Truncate::detailed`, `Truncate::chars`, `Truncate::output`, `Truncate::tail`) and [`TruncateResult`](./ene-common.md#truncateresult) (`content: String`, `truncated: bool`).

Typical usage in a tool:

```rust,no_run
use ene_tool_common::truncate::Truncate;

fn format_tool_output(large_text: &str) -> String {
    let output = Truncate::output(large_text, /* max_lines */ 200, /* max_bytes */ 8_000);
    if output.truncated {
        format!("{}\n\n[Output truncated]", output.content)
    } else {
        output.content
    }
}
```

---

## `html` Module

HTML-to-Markdown conversion and content extraction for tools that fetch web content.

> **Note:** This module is built on the [`scraper`](https://crates.io/crates/scraper) crate (via `htmd`) and performs static HTML parsing — it does not execute JavaScript.

### Method Table

| Function | Signature | Description |
|---|---|---|
| `html_to_markdown` | `fn html_to_markdown(html: &str) -> String` | Converts raw HTML to Markdown. Falls back to returning the original HTML as plain text if the underlying converter fails, rather than an empty string. |
| `extract_html` | `fn extract_html(html: &str, extract: &str, trim: bool) -> String` | Extracts a region of the document (`"body"`, `"main"`, or `"full"`/anything else) and returns it as raw HTML. When `trim` is `true`, strips non-semantic noise (`script`, `style`, `nav`, `header`, `footer`, `aside`, `iframe`, `svg`, etc.) before returning. |
| `extract_markdown` | `fn extract_markdown(html: &str, extract: &str, trim: bool) -> String` | Applies `extract_html` (unless `extract == "full"` and `trim == false`, in which case the full input is used as-is), converts the result to Markdown, and normalizes whitespace (collapses runs of spaces/tabs, collapses 3+ newlines to a blank line, trims). |

> **Note:** there is **no** `html_to_text` or `extract_title` function in this module. To get plain text, convert to Markdown with `html_to_markdown`/`extract_markdown` and strip formatting yourself if needed; to get a title, select it out of the raw HTML with your own selector logic (e.g. via `extract_html` + a downstream HTML parser).

### Example

```rust,no_run
use ene_tool_common::html;

async fn fetch_article(url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let raw_html = reqwest::get(url).await?.text().await?;
    let markdown = html::extract_markdown(&raw_html, "main", /* trim */ true);
    Ok(markdown)
}
```

---

## Errors

`ene-tool-common` does not define its own error type. All fallible operations that cross the tool boundary use [`ToolError`](./ene-tool-proto.md#toolerror) from `ene-tool-proto`; see that crate's docs for the full variant list. `ToolAction::execute` returns `Result<String, ToolError>`, and the `#[derive(ToolAction)]`-generated `execute` reports JSON deserialization failures as `ToolError::InvalidArguments`.

---

## Usage

### Implementing `ToolAction` manually

```rust,no_run
use ene_tool_common::prelude::*;

#[derive(Debug, Clone, Deserialize, JsonSchema, ToolSpec)]
#[tool(
    namespace = "utility",
    name = "echo",
    summary = "Echoes the given text back.",
    category = "Utility",
)]
pub struct EchoArgs {
    /// The text to echo back.
    pub text: String,
}

pub struct EchoAction;

#[async_trait]
impl ToolAction for EchoAction {
    fn name(&self) -> &'static str {
        EchoArgs::TOOL_NAME
    }

    fn definition(&self) -> ToolSpec {
        EchoArgs::spec()
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: EchoArgs = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments { message: e.to_string() })?;
        Ok(args.text)
    }
}
```

### Wiring a tool binary

After implementing one or more `ToolAction`s, aggregate them behind a `ToolProvider` and hand that provider to `run_tool_server` from `ene-tool-proto`. `run_tool_server` is **not generic** — it takes a boxed trait object, not `run_tool_server::<T>()`:

```rust,no_run
// tools/my_tool/src/main.rs
use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{SandboxConfigData, ToolError, ToolProvider, ToolSpec, run_tool_server};

mod actions;
use actions::MyAction;

struct MyToolProvider {
    actions: Vec<Box<dyn ToolAction>>,
}

#[async_trait]
impl ToolProvider for MyToolProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        self.actions.iter().map(|a| a.definition()).collect()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        for action in &self.actions {
            if action.name() == name {
                return action.execute(arguments).await;
            }
        }
        Err(ToolError::NotFound { tool_name: name.to_string() })
    }

    fn set_session_id(&self, _session_id: &str) {}
    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {}
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let provider = MyToolProvider { actions: vec![Box::new(MyAction)] };
    run_tool_server(Box::new(provider)).await?;
    Ok(())
}
```

`run_tool_server` handles the full IPC lifecycle: handshake, initialize, list-tools, and dispatch loop. See [`ene-tool-proto`](./ene-tool-proto.md) for details.

---

## Related Pages

- [`ene-tool-derive`](./ene-tool-derive.md) — Proc-macros: `#[derive(ToolAction)]`, `#[derive(ToolSpec)]`, `#[tool_action(args = T)]`
- [`ene-tool-proto`](./ene-tool-proto.md) — `ToolSpec`, `ToolError`, `ToolProvider`, `run_tool_server`, `IpcRequest`/`IpcResponse`
- [`ene-common`](./ene-common.md) — `Truncate`/`TruncateResult`
- [`ene-tool-host`](./ene-tool-host.md) — Host-side process management and `ToolRegistry`
- [Writing a Tool](../tools/sdk.md)
