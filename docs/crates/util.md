# ene-util

> **Crates**: `ene-util`

## Role

`ene-util` is the home for small, **pure** utility functions whose dependency
trees are independent of each other. Each helper lives behind a Cargo feature
so consumers only pay for what they use.

Currently provided:

- `truncate` (default) — Smart string truncation helpers (`Truncate`).
- `html` — HTML-to-Markdown conversion and content extraction
  (pulls in `htmd`, `scraper`, `ego-tree`, `regex`).

## Boundaries

- **Pure functions only**: no I/O, no business logic, no mutable global state.
- **Feature-gated heavy deps**: the `html` feature's scraper/htmd stack is
  isolated so that truncate-only consumers (`ene-mind`, `ene-cli`,
  `ene-desktop`) never compile them.
- If a helper needs database access, network calls, or domain knowledge, it
  belongs in the appropriate domain crate — not here.

This discipline prevents `ene-util` from becoming the "junk drawer" crate that
the former `ene-common` became before its contents were redistributed.

## Exploration

```bash
cargo doc -p ene-util --open
```
