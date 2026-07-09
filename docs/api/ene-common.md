# `ene-common` — API Reference

> **Crate:** `ene-common`
> **Role:** Shared utility types used across the Ene workspace.

---

## Overview

`ene-common` is the foundational utility crate that holds code shared between multiple workspace members. It is kept intentionally minimal to avoid creating heavy transitive dependencies.

Its sole export is `Truncate`, a unit struct exposing a set of static (`Self`-less) helper functions for safely shortening text — by character count, by line/byte budget, or from the head or tail of the content. `Truncate` is **not** a trait; there is nothing to implement and nothing to call on `&str` or `String` directly. All operations are invoked as `Truncate::method(...)`.

---

## Types

### `Truncate`

```rust
/// Helper struct for high-performance and detailed string truncation.
pub struct Truncate;
```

A zero-sized marker struct. It only exists to namespace the truncation helper functions below; it is never instantiated.

### `TruncateResult`

```rust
#[derive(Debug, Clone)]
pub struct TruncateResult {
    /// The truncated (or original) text content.
    pub content: String,
    /// Whether the content was actually truncated.
    pub truncated: bool,
}
```

Returned by [`Truncate::output`](#truncateoutput) and [`Truncate::tail`](#truncatetail). Carries the resulting text plus a flag indicating whether the input actually needed to be cut.

---

## Method Table

All methods are inherent `Self`-less functions on `Truncate` (i.e. called as `Truncate::simple(...)`, not via a trait).

| Method | Signature | Returns | Description |
|---|---|---|---|
| `simple` | `fn simple(text: &str, max_chars: usize) -> String` | `String` | Truncates to `max_chars` Unicode characters, appending `"..."` if cut. |
| `detailed` | `fn detailed(text: &str, max_chars: usize) -> String` | `String` | Truncates to `max_chars` Unicode characters, appending a note with the original character count if cut. |
| `chars` | `fn chars(text: &str, max_chars: usize) -> String` | `String` | Alias for [`detailed`](#truncatedetailed); used by tools. |
| `output` | `fn output(text: &str, max_lines: usize, max_bytes: usize) -> TruncateResult` | `TruncateResult` | Fits `text` within both a line and byte budget, keeping content from the **head**. |
| `tail` | `fn tail(text: &str, max_lines: usize, max_bytes: usize) -> TruncateResult` | `TruncateResult` | Fits `text` within both a line and byte budget, keeping content from the **tail**. |

None of these methods can panic on malformed UTF-8 input: all character-boundary math snaps to `char_indices()` boundaries, and line/byte splitting operates on `str::lines()`/byte slices that stay within the original string's valid boundaries.

---

## Method Reference

### `Truncate::simple`

```rust
pub fn simple(text: &str, max_chars: usize) -> String
```

Truncates `text` to at most `max_chars` Unicode scalar values. If the text is longer, the excess is cut and `"..."` is appended. If the text already fits, it is returned unchanged.

| Input | `max_chars` | Result |
|---|---|---|
| `"hello"` | `10` | `"hello"` |
| `"hello world"` | `5` | `"hello..."` |

### `Truncate::detailed`

```rust
pub fn detailed(text: &str, max_chars: usize) -> String
```

Like [`simple`](#truncatesimple), but on truncation appends a descriptive notice instead of just `"..."`, including the original character count:

```text
{head}

[... truncated, total {char_count} chars ...]
```

| Input | `max_chars` | Result (truncated case) |
|---|---|---|
| `"hello"` | `10` | `"hello"` (unchanged) |
| `"hello world"` | `5` | `"hello\n\n[... truncated, total 11 chars ...]"` |

### `Truncate::chars`

```rust
pub fn chars(text: &str, max_chars: usize) -> String
```

A direct alias for [`Truncate::detailed`](#truncatedetailed), kept as a separate name for call sites (mainly tool implementations) that prefer the shorter, more descriptive name.

### `Truncate::output`

```rust
pub fn output(text: &str, max_lines: usize, max_bytes: usize) -> TruncateResult
```

Fits `text` within **both** a maximum line count and a maximum byte count, keeping lines from the beginning (head) of the text. If `text` already satisfies both budgets, `TruncateResult { content: text.to_string(), truncated: false }` is returned unchanged.

Otherwise, lines are accumulated from the start until either budget would be exceeded, and the result is annotated:

```text
{kept lines}

...{removed} {"bytes" | "lines"} truncated...

Use offset/limit or grep to view specific sections.
```

The `truncated` field is `true` whenever content was cut. This is the typical choice for truncating command/tool stdout where the most relevant information is usually near the top.

### `Truncate::tail`

```rust
pub fn tail(text: &str, max_lines: usize, max_bytes: usize) -> TruncateResult
```

Identical budget logic to [`output`](#truncateoutput), but keeps lines from the **end** of the text instead of the beginning. The annotation is prepended rather than appended:

```text
...{removed} {"bytes" | "lines"} truncated...

{kept lines}
```

Useful for truncating logs or long-running command output where the most recent lines matter most.

---

## Errors

`ene-common` has no error types. All `Truncate` methods are total functions over `&str` inputs — they never panic, never fail, and always return a value (falling back to returning the original text unchanged when no truncation is needed).

---

## Usage

### Simple ellipsis truncation

```rust,no_run
use ene_common::Truncate;

let text = "This is a long piece of text that we want to shorten.";
let short = Truncate::simple(text, 10);
assert_eq!(short, "This is a ...");
```

### Detailed truncation with character count

```rust,no_run
use ene_common::Truncate;

let text = "This is a long piece of text that we want to shorten.";
let detailed = Truncate::detailed(text, 10);
assert!(detailed.contains("truncated, total"));
```

### Fitting tool output within a line/byte budget

```rust,no_run
use ene_common::Truncate;

let stdout = run_some_command();
let result = Truncate::output(&stdout, /* max_lines */ 200, /* max_bytes */ 16_000);

if result.truncated {
    eprintln!("warning: command output was truncated");
}
send_to_model(&result.content);

fn run_some_command() -> String {
    unimplemented!()
}
fn send_to_model(_: &str) {}
```

### Keeping the tail of a long log

```rust,no_run
use ene_common::Truncate;

let log = read_log_file();
let result = Truncate::tail(&log, /* max_lines */ 100, /* max_bytes */ 8_000);

println!("{}", result.content);

fn read_log_file() -> String {
    unimplemented!()
}
```

---

## Re-exports in Other Crates

`ene-common::Truncate` (and `TruncateResult`) is re-exported by several consumer crates so they do not need to depend on `ene-common` directly:

```rust,no_run
// crates/ene-core/src/lib.rs
#[doc(no_inline)]
pub use ene_common::Truncate;

// crates/ene-session/src/lib.rs
pub use ene_common::truncate::Truncate;

// crates/ene-tool-common/src/lib.rs
pub mod truncate {
    #[doc(no_inline)]
    pub use ene_common::truncate::{Truncate, TruncateResult};
}
```

---

## Adding New Utilities

When adding a new utility to `ene-common`:

1. Keep dependencies minimal — `ene-common` should remain a lightweight crate with no heavy transitive deps.
2. Add the new module to `lib.rs` with `pub mod`.
3. Re-export from consumer crates where appropriate.
4. Add documentation here and in `docs/ja/api/ene-common.md`.

---

## See Also

- [`ene-tool-common`](./ene-tool-common.md) — Re-exports `Truncate`/`TruncateResult` for use in tool implementations
- [`ene-session`](./ene-session.md) — Re-exports `Truncate`
- [`ene-core`](./ene-core.md) — Workspace entry point
