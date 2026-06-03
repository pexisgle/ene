-- Reverse: collapse back to the simple (tool_name, field) key, losing
-- field_key/model_name distinction.

CREATE TABLE IF NOT EXISTS tool_embedding_index_v1 (
    tool_name    TEXT    NOT NULL,
    field        TEXT    NOT NULL CHECK (field IN ('summary', 'description', 'negative')),
    version_hash TEXT    NOT NULL,
    embedding    BLOB    NOT NULL,
    created_at   TEXT    NOT NULL,
    PRIMARY KEY (tool_name, field)
);

-- For duplicate (tool_name, field) rows that differ only in field_key/model_name,
-- keep only the first one (arbitrary via MIN(id)).
INSERT INTO tool_embedding_index_v1 (tool_name, field, version_hash, embedding, created_at)
SELECT tool_name, field, version_hash, embedding, created_at
FROM tool_embedding_index
WHERE id IN (
    SELECT MIN(id) FROM tool_embedding_index GROUP BY tool_name, field
);

DROP TABLE tool_embedding_index;
ALTER TABLE tool_embedding_index_v1 RENAME TO tool_embedding_index;

CREATE INDEX IF NOT EXISTS idx_tool_embedding_field   ON tool_embedding_index(field);
CREATE INDEX IF NOT EXISTS idx_tool_embedding_version ON tool_embedding_index(tool_name, version_hash);
