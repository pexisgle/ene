//! Marks approval-mode-parked candidates on the pending-candidate queue.
//!
//! `approval_parked` distinguishes rows deferred by
//! `mind.memory_approval.require_approval` from weak-contradiction deferrals,
//! so unconfirmed recall can keep excluding never-approved candidates even
//! after the mode is toggled back off.

use sea_orm_migration::prelude::*;

/// Adds the `approval_parked` flag to `pending_candidates`.
pub struct PendingCandidatesApprovalParkedMigration;

impl MigrationName for PendingCandidatesApprovalParkedMigration {
    fn name(&self) -> &'static str {
        "m20260804_000006_pending_candidates_approval_parked"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for PendingCandidatesApprovalParkedMigration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(PendingCandidates::Table)
                    .add_column(
                        ColumnDef::new(PendingCandidates::ApprovalParked)
                            .boolean()
                            .not_null()
                            .default(false),
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
                    .drop_column(PendingCandidates::ApprovalParked)
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
    ApprovalParked,
}
