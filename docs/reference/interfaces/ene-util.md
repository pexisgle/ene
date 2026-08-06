# `ene-util` interface

## Role

Pure utility functions with feature-gated heavy dependencies. The crate
discipline: **no I/O, no business logic, no state** — anything else belongs
in a domain crate.

## Public modules

| Module | Gate | Contents |
|---|---|---|
| `truncate` | `truncate` (default) | `Truncate`, `TruncateResult` — smart string truncation by chars/lines/tail |
| `html` | `html` | `html_to_markdown`, `extract_html`, `extract_markdown` |

## Dependencies

- Depends on: nothing internal (heavy deps are feature-gated: htmd,
  scraper, ego-tree, regex).
- Used by: `ene-mind`, `ene-runtime`, `ene-cli`, `ene-desktop`.

## Refactoring notes

- The purity rule is enforced by review; a helper that grows I/O or domain
  knowledge must move out (e.g. into the owning domain crate).
- Feature gates keep the default build light; adding a heavy dependency
  behind a new feature is the pattern, not a new unconditional dep.
