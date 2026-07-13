# Session Split and Compression

Session splitting is now an explicit hard-boundary path. The preferred context-management path is rolling compression in the mind runtime.

## Current Policy

- **Mind compression mode (`compression_enabled=true`)**
  - Automatic split is bypassed.
  - Old turns are compressed into `memory_spans`.
  - Session ID remains stable for continuity.
- **Hard-split mode (`compression_enabled=false`)**
  - Composite split scoring can trigger automatic split.
  - Manual `/session split` remains available.

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
- When legacy split is used, a new `session_id` is issued after apply.

## Related Docs

- `docs/architecture/cognitive-runtime.md`
- `docs/core/session.md`
