-- Reverse of 00000000000001_create_tool_embedding_index.
-- Recreates the legacy single-vector `tool_embeddings` table, copying the
-- 'description' field row for each tool (the closest analogue of the old
-- single embedding), then drops the new multi-vector table.

CREATE TABLE IF NOT EXISTS tool_embeddings (
    tool_name    TEXT PRIMARY KEY,
    version_hash TEXT    NOT NULL,
    embedding    BLOB    NOT NULL,
    created_at   TEXT    NOT NULL
);

INSERT OR IGNORE INTO tool_embeddings (tool_name, version_hash, embedding, created_at)
SELECT tool_name, version_hash, embedding, created_at
FROM tool_embedding_index
WHERE field = 'description';

DROP TABLE IF EXISTS tool_embedding_index;
