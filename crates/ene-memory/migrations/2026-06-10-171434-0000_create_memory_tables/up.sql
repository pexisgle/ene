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
CREATE INDEX IF NOT EXISTS idx_keyfacts_key ON conversation_keyfacts(card_name, key);

CREATE TABLE IF NOT EXISTS conversation_logs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id  TEXT    NOT NULL,
    card_name   TEXT    NOT NULL,
    role        TEXT    NOT NULL,
    content     TEXT    NOT NULL,
    created_at  TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_log_session ON conversation_logs(session_id);

CREATE TABLE IF NOT EXISTS tool_embedding_index (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    tool_name    TEXT NOT NULL,
    field        TEXT NOT NULL CHECK (field IN ('summary','description','capability','example','negative')),
    field_key    TEXT NOT NULL DEFAULT '',
    version_hash TEXT NOT NULL,
    model_name   TEXT NOT NULL DEFAULT '',
    source_text  TEXT NOT NULL DEFAULT '',
    embedding    BLOB NOT NULL,
    created_at   TEXT NOT NULL,
    UNIQUE(tool_name, field, field_key, model_name)
);
CREATE INDEX IF NOT EXISTS idx_tei_lookup ON tool_embedding_index(tool_name, field);
CREATE INDEX IF NOT EXISTS idx_tei_version ON tool_embedding_index(tool_name, field, version_hash);

CREATE TABLE IF NOT EXISTS __tool_schemas (
    prefix      TEXT PRIMARY KEY NOT NULL,
    schema_json TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
