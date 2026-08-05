# Workspace RAG

The character can answer questions about **your project files**. Workspace
RAG indexes a directory's documents into chunks with embeddings, then
injects relevant chunks into the prompt (the `WorkspaceContext` section)
when a turn needs them.

## Configuration

```json
{
  "rag": {
    "workspace": {
      "enabled": true,
      "root": "/home/me/projects/myapp",
      "include_extensions": ["md", "rs", "toml", "py", "ts", "js"],
      "ignore_globs": [".git/**", "node_modules/**", "target/**"],
      "max_file_bytes": 1048576,
      "chunk_chars": 1200,
      "chunk_overlap_chars": 200,
      "max_chunks_per_file": 256,
      "top_k": 8
    }
  }
}
```

Defaults: common text/code extensions included; `.git`, `node_modules`,
`target`, model weights, and database files ignored; 1 MiB per-file cap;
1200-char chunks with 200-char overlap.

## Indexing

```sh
# In the REPL:
/workspace sync
/workspace status
/workspace search "authentication flow"
/workspace cancel
```

The indexer walks the configured root, applies ignore globs, chunks each
document, and stores chunks + embeddings in the memory database
(`workspace_document_files` / `workspace_document_chunks` tables). Index
state is persisted; a re-sync only re-chunks changed files.

## Retrieval

At turn time, relevant chunks are scored (embedding similarity + lexical
match), deduplicated, and placed into the prompt with source citations.
`/workspace search` shows the same retrieval without a turn, so you can
debug why a chunk did or did not surface.

## Notes

- Workspace indexing requires an embedding provider (`ai.tasks.embedding`).
- Files are read by the host process; the index root is a user-chosen
  directory, not a sandboxed plugin path.
