# Session Split and Compression

**Hard-split is not the product path.** Context boundaries use rolling compression in the mind runtime (`mind.context.compression_*`). Session-ID minting via split scoring is legacy / explicit-only.

## Current Policy

- **Compression (product path, `mind.context.compression_enabled=true`)**
  - Automatic hard-split is bypassed when compression is authoritative.
  - Old turns are compressed into `memory_spans`.
  - Session ID remains stable for continuity.
- **Hard-split (deprecated, `session.auto_split` default `false`)**
  - Not recommended for companion UX.
  - When explicitly enabled and compression is off, composite scoring may trigger a split and mint a new `session_id`.
  - Manual `/session split` may still exist for ops / debugging; prefer manual compression behavior when cognition+compression are on.

## Why Compression Is Preferred

- Preserves relationship continuity by keeping one ongoing session identity.
- Avoids hard boundaries in companion interactions.
- Keeps prompt size bounded with rolling summaries.

## Legacy Split Reasons

- Timeout
- Topic change
- Manual

## Operational Notes

- Only one pending split/compression task is processed at a time.
- Manual split in cognitive+compression mode routes to manual compression behavior.
- When legacy hard-split is used, a new `session_id` is issued after apply.

## Related Docs

- `docs/architecture/api-v2.md`
- `docs/architecture/cognitive-runtime.md`
- `docs/core/session.md`
- `docs/configuration/settings.md` (`session.auto_split`, `mind.context.compression_*`)
