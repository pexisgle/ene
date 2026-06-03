-- Multi-vector tool embedding index.
-- One row per (tool_name, field) where field is one of:
--   'summary'    — name + summary (highest signal, used for fast retrieval)
--   'description' — full description + keywords (used for ranking refinement)
--   'negative'   — negative-keyword phrase (used for disambiguation)
-- Mirrors `ene_tool_proto::EmbeddingField`.

CREATE TABLE IF NOT EXISTS tool_embedding_index (
    tool_name    TEXT    NOT NULL,
    field        TEXT    NOT NULL CHECK (field IN ('summary', 'description', 'negative')),
    version_hash TEXT    NOT NULL,
    embedding    BLOB    NOT NULL,
    created_at   TEXT    NOT NULL,
    PRIMARY KEY (tool_name, field)
);

CREATE INDEX IF NOT EXISTS idx_tool_embedding_field ON tool_embedding_index(field);
CREATE INDEX IF NOT EXISTS idx_tool_embedding_version ON tool_embedding_index(tool_name, version_hash);

-- Migrate legacy single-vector rows from `tool_embeddings` into the new
-- multi-vector index, mapping each existing row to the 'description' field
-- (its semantics most closely match the old single embedding).
INSERT OR IGNORE INTO tool_embedding_index (tool_name, field, version_hash, embedding, created_at)
SELECT tool_name, 'description', version_hash, embedding, created_at
FROM tool_embeddings;

DROP TABLE IF EXISTS tool_embeddings;
