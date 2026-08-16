//! `MemoryStore::search_messages` filters with a leading-wildcard
//! `LOWER(content) LIKE '%…%'`, which cannot use a B-tree index on
//! `content` (that needs FTS5, tracked separately). The only other index
//! on the table is `idx_log_session` (`session_id`), so without this
//! migration every search would perform an in-memory sort of all matching
//! rows to satisfy `ORDER BY created_at DESC` + `LIMIT`/`OFFSET`.
//!
//! This migration adds `idx_log_created_at` on `created_at DESC`. `SQLite` can
//! walk that index newest-first, evaluating the `LIKE` predicate as it goes
//! and stopping once `LIMIT`/`OFFSET` are satisfied — no full sort of the
//! table. The worst-case scan for a query with no matches is unchanged (the
//! leading-wildcard filter still touches every row), but the common paginated
//! case is served without materializing and sorting the entire result set.

use sea_orm_migration::prelude::*;

pub struct ConversationLogsSearchIndexMigration;

impl MigrationName for ConversationLogsSearchIndexMigration {
    fn name(&self) -> &'static str {
        "m20260731_000005_conversation_logs_search_index"
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
