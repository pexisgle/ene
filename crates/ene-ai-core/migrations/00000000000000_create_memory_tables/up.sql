CREATE TABLE IF NOT EXISTS conversation_summaries (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id  TEXT    NOT NULL,
    card_name   TEXT    NOT NULL,
    summary     TEXT    NOT NULL,
    embedding   BLOB    NOT NULL,
    created_at  TEXT    NOT NULL,
    ended_at    TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_summary_card ON conversation_summaries(card_name);
CREATE INDEX IF NOT EXISTS idx_summary_created ON conversation_summaries(created_at DESC);

CREATE TABLE IF NOT EXISTS conversation_keyfacts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    card_name   TEXT    NOT NULL,
    summary_id  INTEGER,
    key         TEXT    NOT NULL,
    value       TEXT    NOT NULL,
    created_at  TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_keyfacts_card ON conversation_keyfacts(card_name);
CREATE INDEX IF NOT EXISTS idx_keyfacts_key   ON conversation_keyfacts(card_name, key);

CREATE TABLE IF NOT EXISTS conversation_logs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id  TEXT    NOT NULL,
    card_name   TEXT    NOT NULL,
    role        TEXT    NOT NULL,
    content     TEXT    NOT NULL,
    created_at  TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_log_session ON conversation_logs(session_id);

CREATE TABLE IF NOT EXISTS tool_embeddings (
    tool_name    TEXT PRIMARY KEY,
    version_hash TEXT    NOT NULL,
    embedding    BLOB    NOT NULL,
    created_at   TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tool_embedding_version ON tool_embeddings(tool_name, version_hash);
