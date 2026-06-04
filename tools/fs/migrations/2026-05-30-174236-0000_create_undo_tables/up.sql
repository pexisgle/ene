CREATE TABLE IF NOT EXISTS undo_entries (
    id         TEXT    PRIMARY KEY NOT NULL,
    session_id TEXT    NOT NULL,
    tool_name  TEXT    NOT NULL,
    timestamp  TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_undo_entries_session ON undo_entries(session_id);

CREATE TABLE IF NOT EXISTS undo_operations (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    entry_id         TEXT    NOT NULL REFERENCES undo_entries(id) ON DELETE CASCADE,
    op_type          TEXT    NOT NULL,
    path             TEXT    NOT NULL,
    original_content BLOB,
    sort_order       INTEGER NOT NULL
);
