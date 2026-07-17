# Memory

ene can keep **long-term memory** across sessions: facts, episodes, preferences, commitments, and related types, stored in SQLite with optional vector search.

## Division of labor

| Layer | Responsibility |
|-------|----------------|
| **mind** | Plans recall, scores what matters, writes after a turn, applies forgetting policy |
| **store** | Persists text (and optional embeddings), runs filtered search |

Turn on persistence with `store.enabled` and set `store.db_path` if needed. Recall and decay knobs live under `mind.*`.

## Hybrid recall (idea)

Search can combine embeddings, lexical match, recency, and salience — mind orchestrates; store executes.

## Dig deeper

- [Long-term memory reference](../../reference/memory/memory.md)
- [ene-store API](../../reference/api/ene-store.md)
- [ene-mind API](../../reference/api/ene-mind.md)
