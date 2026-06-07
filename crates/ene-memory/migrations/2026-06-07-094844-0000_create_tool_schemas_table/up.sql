CREATE TABLE IF NOT EXISTS __tool_schemas (
    prefix TEXT PRIMARY KEY NOT NULL,
    schema_json TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
