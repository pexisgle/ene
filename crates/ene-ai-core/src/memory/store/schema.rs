/// sqlite-vec 拡張をグローバルに自動登録
/// 初回のみ呼び出す（複数回呼び出しても安全）
pub fn init_sqlite_vec() {
    use rusqlite::ffi::sqlite3_auto_extension;
    use sqlite_vec::sqlite3_vec_init;
    unsafe {
        sqlite3_auto_extension(Some(std::mem::transmute(sqlite3_vec_init as *const ())));
    }
}

/// スキーマ初期化
pub fn initialize_schema(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS conversation_summaries (
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
        ",
    )
}
