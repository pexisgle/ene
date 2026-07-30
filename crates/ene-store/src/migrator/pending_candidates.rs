//! Persistent pending memory-candidate approval queue (#420).
//!
//! Replaces the previous in-memory `Vec<PendingCandidate>` so candidates
//! survive restarts and are shared across `MemoryStore` instances on the
//! same database. Follows the `pending_memory_writes` "pending queue"
//! conventions: `user_id` defaults to `''`, and `created_at` is a
//! `timestamp_with_time_zone` so the retention sweep can compare it in SQL
//! exactly like `pending_memory_writes.next_retry_at`. (The issue sketched
//! `created_at TEXT`; the timestamptz column is chosen instead because it is
//! the repo's proven pattern for datetime columns filtered in SQL.) Both the
//! table and index are created `IF NOT EXISTS` so the migration is additive
//! and idempotent.
//!
//! The composite index is `(character_id, status, created_at)`: it covers the
//! retention count query's `WHERE character_id = ? AND status = 'pending'
//! ORDER BY created_at` without a filesort, and still serves the age `DELETE`
//! (which keys on the `character_id` prefix).

use sea_orm_migration::prelude::*;

/// Adds the persistent pending memory-candidate approval queue (#420).
pub struct PendingCandidatesMigration;

impl MigrationName for PendingCandidatesMigration {
    fn name(&self) -> &'static str {
        "m20260730_000004_pending_candidates"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for PendingCandidatesMigration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PendingCandidates::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PendingCandidates::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(PendingCandidates::CharacterId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PendingCandidates::UserId)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(ColumnDef::new(PendingCandidates::Title).string().not_null())
                    .col(
                        ColumnDef::new(PendingCandidates::Content)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(PendingCandidates::Kind).string().not_null())
                    .col(
                        ColumnDef::new(PendingCandidates::Confidence)
                            .float()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PendingCandidates::ReasonDetail)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PendingCandidates::SourceQuote)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(PendingCandidates::ExistingMemoryId).integer())
                    .col(
                        ColumnDef::new(PendingCandidates::Status)
                            .string()
                            .not_null()
                            .default("pending"),
                    )
                    .col(
                        ColumnDef::new(PendingCandidates::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_pending_candidates_character_status_created")
                    .table(PendingCandidates::Table)
                    .col(PendingCandidates::CharacterId)
                    .col(PendingCandidates::Status)
                    .col(PendingCandidates::CreatedAt)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PendingCandidates::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum PendingCandidates {
    #[iden = "pending_candidates"]
    Table,
    Id,
    CharacterId,
    UserId,
    Title,
    Content,
    Kind,
    Confidence,
    ReasonDetail,
    SourceQuote,
    ExistingMemoryId,
    Status,
    CreatedAt,
}
