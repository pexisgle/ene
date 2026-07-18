# Session Split and Compression

**Hard-split is not the product path.** Context boundaries use rolling compression only (`mind.context.compression_*`). `ene-runtime` does not spawn hard-split / session-ID minting tasks.

## Current Policy

- **Compression (required product path)**
  - Old turns are compressed into `memory_spans`.
  - Session ID remains stable for continuity.
  - Manual `/session split` triggers **context compression** (same session id).
- **Hard-split**
  - Not used by the host. Scoring / `execute_split` may remain in `ene-mind` for library experiments but are not wired from `ene-runtime`.

## Why Compression Is Preferred

- Preserves relationship continuity by keeping one ongoing session identity.
- Avoids hard boundaries in companion interactions.
- Keeps prompt size bounded with rolling summaries.

## Operational Notes

- Only one pending compression task is processed at a time.
- Manual split in the host routes to manual compression behavior.

## Related Docs

- `docs/reference/architecture/api-v1.md`
- `docs/reference/architecture/cognitive-runtime.md`
- `docs/reference/runtime/session.md`
- `docs/reference/configuration/settings.md` (`mind.context.compression_*`)
