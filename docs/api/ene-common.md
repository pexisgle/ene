# `ene-common` — API Reference

> **Crate:** `ene-common`  
> **Role:** Shared utility types and traits used across the Ene workspace.

---

## Overview

`ene-common` is the foundational utility crate that holds code shared between multiple workspace members. It is kept intentionally minimal to avoid creating heavy transitive dependencies.

Currently, its primary export is the `Truncate` trait for safely limiting string length in various units.

---

## `Truncate` Trait

Provides three truncation strategies on string types, each returning a `TruncateResult` that carries both the (possibly truncated) string and a flag indicating whether truncation occurred.

The trait is implemented for both `str` and `String` via a blanket implementation.

```rust
pub trait Truncate {
    /// Truncate to at most `n` Unicode scalar values (characters).
    fn truncate_chars(&self, n: usize) -> TruncateResult;

    /// Truncate to at most `n` lines.
    fn truncate_lines(&self, n: usize) -> TruncateResult;

    /// Truncate to at most `n` bytes.
    fn truncate_bytes(&self, n: usize) -> TruncateResult;
}
```

### `TruncateResult`

```rust
pub struct TruncateResult {
    /// The (possibly truncated) string content.
    pub truncated: String,

    /// `true` if the input was longer than `n` and was actually cut.
    pub was_truncated: bool,
}
```

---

## Method Reference

### `truncate_chars`

```rust
fn truncate_chars(&self, n: usize) -> TruncateResult
```

Splits at the `n`-th Unicode scalar value boundary. Safe for multi-byte UTF-8 text — will not produce invalid UTF-8.

| Input | `n` | `truncated` | `was_truncated` |
|-------|-----|-------------|-----------------|
| `"Hello, world!"` | `5` | `"Hello"` | `true` |
| `"Hi"` | `10` | `"Hi"` | `false` |
| `"日本語テキスト"` | `3` | `"日本語"` | `true` |

### `truncate_lines`

```rust
fn truncate_lines(&self, n: usize) -> TruncateResult
```

Keeps at most `n` lines (split on `\n`). Trailing newlines in the last retained line are preserved.

| Input | `n` | `truncated` | `was_truncated` |
|-------|-----|-------------|-----------------|
| `"a\nb\nc\nd"` | `2` | `"a\nb"` | `true` |
| `"only one line"` | `5` | `"only one line"` | `false` |

### `truncate_bytes`

```rust
fn truncate_bytes(&self, n: usize) -> TruncateResult
```

Truncates to at most `n` bytes, snapping back to the nearest valid UTF-8 character boundary to avoid producing invalid output.

| Input | `n` | `truncated` | `was_truncated` |
|-------|-----|-------------|-----------------|
| `"Hello"` | `3` | `"Hel"` | `true` |
| `"Hi"` | `100` | `"Hi"` | `false` |

---

## Usage Examples

### Basic usage

```rust
use ene_common::truncate::Truncate;

let text = "This is a long piece of text that we want to truncate.";

let result = text.truncate_chars(10);
println!("{}", result.truncated);       // "This is a "
println!("{}", result.was_truncated);   // true

let short = "Hello".truncate_chars(100);
assert!(!short.was_truncated);
assert_eq!(short.truncated, "Hello");
```

### Truncating multi-line output

```rust
use ene_common::truncate::Truncate;

let output = "line 1\nline 2\nline 3\nline 4\nline 5";
let result = output.truncate_lines(3);

assert_eq!(result.truncated, "line 1\nline 2\nline 3");
assert!(result.was_truncated);
```

### Byte-safe truncation for buffer limits

```rust
use ene_common::truncate::Truncate;

// Useful when interfacing with APIs that have byte-length limits
let user_input = get_user_input();
let safe_input = user_input.truncate_bytes(4096);

if safe_input.was_truncated {
    eprintln!("Warning: input was truncated to 4096 bytes");
}
send_to_api(&safe_input.truncated);
```

---

## Re-exports in Other Crates

`ene-session` re-exports `Truncate` for convenience, so consumers of `ene-session` do not need to depend on `ene-common` directly:

```rust
// In ene-session:
pub use ene_common::truncate::Truncate;
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

- [`ene-session`](./ene-session.md) — Re-exports `Truncate`
- [`ene-core`](./ene-core.md) — Workspace entry point
