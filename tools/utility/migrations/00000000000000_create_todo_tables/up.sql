CREATE TABLE IF NOT EXISTS todo_items (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT    NOT NULL,
    parent_id  INTEGER REFERENCES todo_items(id) ON DELETE CASCADE,
    content    TEXT    NOT NULL,
    status     TEXT    NOT NULL CHECK (status  IN ('pending','in_progress','completed','cancelled')),
    priority   TEXT    NOT NULL CHECK (priority IN ('high','medium','low')),
    created_at TEXT    NOT NULL,
    updated_at TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_todo_items_session ON todo_items(session_id);
CREATE INDEX IF NOT EXISTS idx_todo_items_parent  ON todo_items(parent_id);
CREATE INDEX IF NOT EXISTS idx_todo_items_status  ON todo_items(session_id, status);
