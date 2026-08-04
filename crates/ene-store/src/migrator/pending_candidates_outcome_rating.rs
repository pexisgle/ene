//! Carries the deferred decision's outcome rating on the pending-candidate
//! queue.
//!
//! Approval mode rewrites persist/supersede decisions into
//! `AskConfirmationLater`; without the rating the later approval insert
//! bypasses the arbiter and the memory would never enter the self-reflection
//! loop. The column preserves the original evaluation until approval time.

use sea_orm_migration::prelude::*;

/// Adds the `outcome_rating` column to `pending_candidates`.
pub struct PendingCandidatesOutcomeRatingMigration;

impl MigrationName for PendingCandidatesOutcomeRatingMigration {
    fn name(&self) -> &'static str {
        "m20260805_000010_pending_candidates_outcome_rating"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for PendingCandidatesOutcomeRatingMigration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(PendingCandidates::Table)
                    .add_column(ColumnDef::new(PendingCandidates::OutcomeRating).float())
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
                    .drop_column(PendingCandidates::OutcomeRating)
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
    OutcomeRating,
}
