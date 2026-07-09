# API Refactor Plan

- **Status:** In progress
- **Date:** 2026-07-09
- **Last updated:** 2026-07-09 (P0–P3 initial pass landed; see [Completed](#completed-2026-07-09))

## Context

The 2026-07 API documentation audit refreshed every library crate page and added missing pages for `ene-cognition` and `ene-vrm`. A first implementation pass (2026-07-09) addressed the highest-leverage items below. This document tracks **remaining** work only.

## Goals

1. Shrink accidental public surface without breaking intentional host APIs (`EneHandle`, tool IPC ABI).
2. Keep crate boundaries aligned with [Cognitive Runtime ADR](cognitive-runtime.md) and [AGENTS.md](../../AGENTS.md) §4.1.
3. Make async vs sync and error naming predictable across crates.
4. Stage session-split → compression and event-model changes so apps can migrate safely.
5. Stabilize the tool wire protocol and narrow the VRM entry points used by desktop.

## Non-Goals

- Merging crates or collapsing the tool-binary sandbox model.
- Rewriting application UX (`ene-cli` / `ene-desktop`) except where they must follow API changes.
- Changing sea-orm ownership of SQLite (remains exclusive to `ene-memory`).

---

## Completed (2026-07-09)

| Area | Delivered |
|---|---|
| **Public surface** | `ene-core::schema_link` for cognition ctor linking (#95); contributor-only notes on `streaming` / `message_builder` |
| **Async / errors** | Error & async conventions in [`docs/api/index.md`](../api/index.md); `run_tool_server` returns `Result<(), ToolError>` |
| **Boundaries** | ADR guardrail module docs on `ene-cognition`, `ene-memory`, `ene-tool-proto` |
| **Tool ABI** | `ActionSetProvider` / `SingleActionProvider` in `ene-tool-common`; ABI table in [`docs/tools/sdk.md`](../tools/sdk.md); `AGENTS.md` R1 wiring fix; `tools/utility` migrated |
| **Events / session** | [`docs/core/streaming-events.md`](../core/streaming-events.md) (legacy vs cognitive); compression-preference doc comments on split APIs |
| **VRM** | `ene_vrm::prelude`; `#[doc(hidden)]` on internal loaders/helpers; Supported vs Internal section in [`docs/api/ene-vrm.md`](../api/ene-vrm.md) |
| **API docs** | Full EN+JA refresh for all 14 library crates + `ene-cognition` / `ene-vrm` pages |

---

## Remaining work

### 1. Shrink the public surface (follow-up)

- Trim accidental `ene-core` root re-exports; prefer importing domain types from owning crates in apps.
- Visibility pass: demote unused `pub` items to `pub(crate)` in `ene-cognition`, `ene-memory`, `ene-session`, `ene-tool-host`.
- Run `cargo doc --no-deps` review per crate.

**Affected crates:** `ene-core`, `ene-cognition`, `ene-memory`, `ene-session`, `ene-tool-host`

---

### 2. Enforce crate boundaries (follow-up)

- Migrate remaining legacy prompt assembly call sites to `CognitionEngine` when cognition is enabled.
- Keep dependency graph free of new ADR violations (`cargo tree -p ene-cognition -p ene-tool-proto`).

**Affected crates:** `ene-core`, `ene-cognition`, `ene-session`

---

### 3. Unify API shape (follow-up)

- Type `McpToolRegistry::connect_stdio` errors (still `Result<(), String>` today).
- Add async + typed-error checks to the PR verification checklist in `AGENTS.md` §8.

**Affected crates:** `ene-tool-host`, `ene-provider`

---

### 4. Events and session API migration (Phases B–C)

- **Phase B:** Emit compression-oriented events (or enrich `SessionSplit` / status) when compression runs.
- **Phase C:** Make engine-managed `Expression` the primary path when the emotion engine is enabled; treat inline `<|emo:name|>` tokens as legacy/advisory.
- Verify `ene-cli` / `ene-desktop` handle `EneEvent::Terminal` and `Expression` everywhere.
- Optional: feature-flag or config to disable split scoring when compression is authoritative.

**Affected crates:** `ene-core`, `ene-session`, `ene-cognition`, `ene-cli`, `ene-desktop`

---

### 5. Tool ABI stabilization (follow-up)

- Migrate remaining tool binaries (`ene-tool-fs`, `ene-tool-web`, `ene-tool-app`, `ene-tool-browser`) to `ActionSetProvider`.
- Add proto unit tests for handshake / version rejection.

**Affected crates:** `tools/*`, `ene-tool-common`, `ene-tool-proto`

---

## Definition of done (per item)

- Code change + tests/clippy green under `direnv exec .`
- `docs/api/` and `docs/ja/api/` (and any tutorial pages) updated in the same change
- No new circular dependencies; `ene-memory` remains sole sea-orm owner
- Tool wire changes either additive or accompanied by `PROTOCOL_VERSION` bump

## Related documentation

- [API Index](../api/index.md)
- [Streaming Events](../core/streaming-events.md)
- [Cognitive Runtime ADR](cognitive-runtime.md)
- [Architecture Overview](overview.md)
- [Tool SDK](../tools/sdk.md)
- [AGENTS.md](../../AGENTS.md)
