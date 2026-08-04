//! Adds approval-workflow provenance columns to `pending_candidates`.
//!
//! `source_turn` records the conversation turn that produced a candidate so
//! the approval UI can point back at the source; `resolved_at` records when a
//! candidate was approved or rejected so history views can show the decision
//! time. Both are nullable: rows written before this migration carry neither,
//! and a candidate that is still pending has no `resolved_at`.

use sea_orm_migration::prelude::*;

/// Adds `source_turn` and `resolved_at` to the pending-candidate queue.
pub struct PendingCandidatesSourceTurnMigration;

impl MigrationName for PendingCandidatesSourceTurnMigration {
    fn name(&self) -> &'static str {
        "m20260804_000005_pending_candidates_source_turn"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for PendingCandidatesSourceTurnMigration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(PendingCandidates::Table)
                    .add_column(
                        ColumnDef::new(PendingCandidates::SourceTurn)
                            .string()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(PendingCandidates::Table)
                    .add_column(
                        ColumnDef::new(PendingCandidates::ResolvedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(PendingCandidates::Table)
                    .drop_column(PendingCandidates::SourceTurn)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(PendingCandidates::Table)
                    .drop_column(PendingCandidates::ResolvedAt)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum PendingCandidates {
    #[iden = "pending_candidates"]
    Table,
    SourceTurn,
    ResolvedAt,
}
