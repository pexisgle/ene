# `ene-rag` interface

## Role

RAG **policy** layer: pure scoring, decay, tool selection, and workspace
document chunking. No I/O and no database access.

## Public modules

| Module | Contents |
|---|---|
| `decay` | `half_life_decay`, `decay_score`, `recency_score`, `emotional_impact`, lifecycle thresholds (`FADE_THRESHOLD`, `ARCHIVE_THRESHOLD`) and weights |
| `scoring` | `score_candidate`, `score_and_rank`, `relevance_score`, `lexical_overlap_score`, `emotional_match_score`, `relationship_score`, `contradiction_penalty`, `stale_penalty`, `access_boost_score`, `within_time_range`, `document_lexical_similarity` |
| `tool` *(feature `tool`)* | `ToolRag`, `ToolRagConfig`, `ToolRagOptions`, `ToolRagStats`, `hybrid_embed`, `hyde_document`, `rerank_tool_specs`, `FieldWeights` |
| `workspace` | `chunk_document`, `ChunkOptions`, `ChunkedDocument`, `DocumentChunk`, `score_chunk`, `glob_matches`, `WorkspaceRagConfig` |

## Dependencies

- Depends on: `ene-core`, `ene-config`; with the `tool` feature also
  `ene-ai`, `ene-plugin-proto`, and friends.
- Used by: `ene-store` (scoring core), `ene-mind` (recall scoring),
  `ene-runtime` (tool selection, workspace indexing).

## Refactoring notes

- This crate exists so the **same scoring policy cannot diverge** between
  the memory and tool sides. Move scoring here, not into callers.
- The `tool` feature is the dependency gate: persistence and cognitive
  callers use the default (pure) feature set; only `ene-runtime` enables
  `tool`. Adding a dependency to the default set leaks the embedding stack
  into `ene-store`.
- Decay thresholds and weights are magic constants with documented meaning;
  changing them changes recall behaviour globally — keep the tests that pin
  them.
- `glob_matches` (ignore rules) semantics are pinned by tests
  (basename match, trailing `/**` matches the directory itself).
