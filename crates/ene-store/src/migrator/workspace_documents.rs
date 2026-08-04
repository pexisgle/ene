//! Creates the workspace document index tables.
//!
//! The `vec0` ANN shadow table is deliberately *not* created here: its column
//! definition bakes in the embedding dimension, which is a runtime parameter
//! (same arrangement as the memory/tool vec0 tables, which are created lazily
//! by `ensure_vec0_index` on fresh databases).

use sea_orm_migration::prelude::*;

/// Base tables for the document/workspace RAG index.
pub struct WorkspaceDocumentsMigration;

impl MigrationName for WorkspaceDocumentsMigration {
    fn name(&self) -> &'static str {
        "m20260804_000008_workspace_documents"
    }
}

#[derive(DeriveIden)]
enum WorkspaceDocumentFiles {
    Table,
    Id,
    Root,
    Path,
    Size,
    ModifiedAt,
    ContentHash,
    ModelName,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum WorkspaceDocumentChunks {
    Table,
    Id,
    FileId,
    ChunkIndex,
    Heading,
    Content,
    StartLine,
    EndLine,
    Embedding,
    CreatedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for WorkspaceDocumentsMigration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(WorkspaceDocumentFiles::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(WorkspaceDocumentFiles::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceDocumentFiles::Root)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceDocumentFiles::Path)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceDocumentFiles::Size)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceDocumentFiles::ModifiedAt)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceDocumentFiles::ContentHash)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceDocumentFiles::ModelName)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceDocumentFiles::CreatedAt)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceDocumentFiles::UpdatedAt)
                            .string()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .unique()
                    .name("uniq_ws_file_path")
                    .table(WorkspaceDocumentFiles::Table)
                    .col(WorkspaceDocumentFiles::Path)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_ws_file_root")
                    .table(WorkspaceDocumentFiles::Table)
                    .col(WorkspaceDocumentFiles::Root)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(WorkspaceDocumentChunks::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(WorkspaceDocumentChunks::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceDocumentChunks::FileId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceDocumentChunks::ChunkIndex)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceDocumentChunks::Heading)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(WorkspaceDocumentChunks::Content)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceDocumentChunks::StartLine)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceDocumentChunks::EndLine)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceDocumentChunks::Embedding)
                            .blob()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceDocumentChunks::CreatedAt)
                            .string()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_ws_chunk_file")
                            .from(
                                WorkspaceDocumentChunks::Table,
                                WorkspaceDocumentChunks::FileId,
                            )
                            .to(WorkspaceDocumentFiles::Table, WorkspaceDocumentFiles::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .unique()
                    .name("uniq_ws_chunk_file_index")
                    .table(WorkspaceDocumentChunks::Table)
                    .col(WorkspaceDocumentChunks::FileId)
                    .col(WorkspaceDocumentChunks::ChunkIndex)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(WorkspaceDocumentChunks::Table)
                    .if_exists()
                    .cascade()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(WorkspaceDocumentFiles::Table)
                    .if_exists()
                    .cascade()
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
