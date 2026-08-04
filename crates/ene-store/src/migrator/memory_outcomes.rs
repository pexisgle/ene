//! Durable per-decision interaction outcome evaluations.
//!
//! One row per persisted arbiter decision (when the self-reflection pipeline
//! is enabled), keyed to the evaluated typed memory. The reflection generator
//! aggregates rows newer than its own creation instant into `Reflection`
//! strategy memories, so the table is the single durable source for the
//! evaluation signal.

use sea_orm_migration::prelude::*;

/// Adds the `memory_outcomes` table.
pub struct MemoryOutcomesMigration;

impl MigrationName for MemoryOutcomesMigration {
    fn name(&self) -> &'static str {
        "m20260804_000009_memory_outcomes"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for MemoryOutcomesMigration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(MemoryOutcomes::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(MemoryOutcomes::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(MemoryOutcomes::MemoryId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MemoryOutcomes::MemoryTitle)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MemoryOutcomes::CharacterId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MemoryOutcomes::UserId)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(ColumnDef::new(MemoryOutcomes::Rating).float().not_null())
                    .col(ColumnDef::new(MemoryOutcomes::Source).string().not_null())
                    .col(ColumnDef::new(MemoryOutcomes::SourceRef).string())
                    .col(
                        ColumnDef::new(MemoryOutcomes::CreatedAt)
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
                    .name("idx_memory_outcomes_character_id_id")
                    .table(MemoryOutcomes::Table)
                    .col(MemoryOutcomes::CharacterId)
                    .col(MemoryOutcomes::Id)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(MemoryOutcomes::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum MemoryOutcomes {
    Table,
    Id,
    MemoryId,
    MemoryTitle,
    CharacterId,
    UserId,
    Rating,
    Source,
    SourceRef,
    CreatedAt,
}
