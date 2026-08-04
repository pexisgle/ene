# Workspace RAG Guide

Workspace RAG indexes local documents and project folders so Ene can retrieve
relevant passages — with citations — as conversation context. It is the third
RAG consumer after memory recall and tool selection, and it shares the same
policy layer (`ene-rag`): CJK-aware hybrid scoring, chunking with heading and
line-range metadata, and a persisted file-hash index in `ene-store`.

## Privacy model

The feature is **disabled by default and scans nothing until you opt in**. The
privacy model has four layers:

1. **Explicit folder allowlist.** Only the folders listed under
   `rag.workspace.folders` are ever read. Paths are canonicalized before
   scanning; directory symlinks are never followed; file symlinks are indexed
   only when their canonical target stays inside the configured folder, and
   any file whose canonical path escapes it is skipped.
2. **Search-time enforcement.** Every search (prompt injection and
   `/workspace search`) receives the *current* configured folders as an
   allowlist. Rows from a folder you removed from the config are pruned on the
   next sync and are unreachable immediately.
3. **Automatic exclusions.** The default ignore rules cover `.env` /
   `.env.*`, model weights (`.gguf`, `.safetensors`, `.ckpt`, `.pth`, `.onnx`,
   `.bin`), database files (`*.db*`), and common dependency/build directories
   (`.git`, `node_modules`, `target`, `dist`, `.venv`, `assets/models`).
   Binary files (NUL-byte sniff), non-UTF-8 files, and files above
   `max_file_bytes` (default 1 MiB) are skipped. Files exceeding
   `max_chunks_per_file` are skipped entirely rather than silently truncated.
4. **Local persistence.** The index (file paths, hashes, chunk text,
   embeddings) lives in the character's `memory.db` — the same local database
   as memories. Nothing is uploaded.

## Configuration

See the [configuration reference](../configuration.md#ragworkspace--documentworkspace-rag-settings)
for the full key list. The minimum setup for a project folder:

```json
{
  "rag": {
    "workspace": {
      "enabled": true,
      "folders": ["/home/me/projects/my-app"]
    }
  }
}
```

or via environment variables:

```bash
export ENE_RAG__WORKSPACE__ENABLED=true
export ENE_RAG__WORKSPACE__FOLDERS=/home/me/projects/my-app
```

## Keeping the index fresh

Syncs are hash-based: each file is hashed (blake3) and only changed or new
files are re-embedded; renames with unchanged content are remapped in place
without re-embedding; deleted files are removed. There are three ways to sync:

- `sync_on_startup: true` runs a background sync when the runtime opens.
- `/workspace sync` starts a background sync; `/workspace status` shows live
  progress (phase, files scanned/indexed/skipped, chunks embedded);
  `/workspace cancel` stops it. Cancellation lands at the next file boundary —
  an in-flight embedding batch runs to completion.
- Prompt injection and `/workspace search` read the current index; run a sync
  after large edits to keep results current.

## Citations

Every retrieved chunk carries its source location: canonical file path,
nearest heading (Markdown ATX headings, falling back to the file name), and
the 1-based line range of the chunk. Prompt injection renders them as:

```text
## Workspace Documents
- /home/me/projects/my-app/docs/setup.md:10-24 [Installation]
run cargo build --release
```

`/workspace search <query>` prints the same citation format with the hybrid
score.

## Privacy checklist

- Only opt in for folders you intend to share with the model.
- Review `ignore_globs` before enabling; `.env`-style files are excluded by
  default, but secrets can hide in other extensions.
- Removing a folder from `folders` stops future searches immediately; the next
  sync prunes its rows from the index.
- To wipe the index entirely, disable the feature and remove the character's
  `memory.db` (this also removes memories), or run a sync after emptying
  `folders` — an empty allowlist prunes every row.

## What is not covered (yet)

- Desktop settings UI for the RAG section (JSON/env configuration only).
- Live file watching — syncs are manual or on startup.
- Non-UTF-8 and binary documents are skipped, not converted.
