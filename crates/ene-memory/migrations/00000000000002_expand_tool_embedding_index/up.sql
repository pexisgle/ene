-- Expand tool_embedding_index to support multi-key per-field embeddings
-- and model_name tracking for the ToolRag pipeline.
--
-- New columns:
--   field_key   — disambiguates multiple examples/capabilities per tool (default empty)
--   model_name   — tracks which embedding model produced the vector (default empty)
-- Expanded CHECK constraint to accept 'capability' and 'example' fields.

CREATE TABLE IF NOT EXISTS tool_embedding_index_v2 (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    tool_name    TEXT NOT NULL,
    field        TEXT NOT NULL CHECK (field IN ('summary','description','capability','example','negative')),
    field_key    TEXT NOT NULL DEFAULT '',
    version_hash TEXT NOT NULL,
    model_name   TEXT NOT NULL DEFAULT '',
    embedding    BLOB NOT NULL,
    created_at   TEXT NOT NULL,
    UNIQUE(tool_name, field, field_key, model_name)
);

INSERT INTO tool_embedding_index_v2 (tool_name, field, field_key, version_hash, model_name, embedding, created_at)
SELECT tool_name, field, '', version_hash, '', embedding, created_at
FROM tool_embedding_index;

DROP TABLE tool_embedding_index;
ALTER TABLE tool_embedding_index_v2 RENAME TO tool_embedding_index;

CREATE INDEX IF NOT EXISTS idx_tei_lookup  ON tool_embedding_index(tool_name, field);
CREATE INDEX IF NOT EXISTS idx_tei_version ON tool_embedding_index(tool_name, field, version_hash);
