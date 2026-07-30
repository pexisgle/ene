//! Search-ordering index for `conversation_logs` (#422).
//!
//! `MemoryStore::search_messages` filters with a leading-wildcard
//! `LOWER(content) LIKE '%…%'`, which cannot use a B-tree index on
//! `content` (that needs FTS5, tracked separately in #424). Before this
//! migration the only index on the table was `idx_log_session`
//! (`session_id`), so every search also performed an in-memory sort of all
//! matching rows to satisfy `ORDER BY created_at DESC` + `LIMIT`/`OFFSET`.
//!
//! This migration adds `idx_log_created_at` on `created_at DESC`. `SQLite` can
//! walk that index newest-first, evaluating the `LIKE` predicate as it goes
//! and stopping once `LIMIT`/`OFFSET` are satisfied — no full sort of the
//! table. The worst-case scan for a query with no matches is unchanged (the
//! leading-wildcard filter still touches every row), but the common paginated
//! case no longer materializes and sorts the entire result set.

use sea_orm_migration::prelude::*;

/// Adds `idx_log_created_at` so session search can serve its
/// `ORDER BY created_at DESC` + `LIMIT`/`OFFSET` from an index.
pub struct ConversationLogsSearchIndexMigration;

impl MigrationName for ConversationLogsSearchIndexMigration {
    fn name(&self) -> &'static str {
        "m20260731_000004_conversation_logs_search_index"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for ConversationLogsSearchIndexMigration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS idx_log_created_at \
                 ON conversation_logs(created_at DESC)",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS idx_log_created_at")
            .await?;
        Ok(())
    }
}
