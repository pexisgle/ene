#![allow(missing_docs)]

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(Migration)]
    }
}

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 1. conversation_summaries
        manager
            .create_table(
                Table::create()
                    .table(ConversationSummaries::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ConversationSummaries::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConversationSummaries::SessionId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationSummaries::CardName)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationSummaries::Summary)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationSummaries::Embedding)
                            .blob()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationSummaries::CreatedAt)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationSummaries::EndedAt)
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
                    .name("idx_summary_card")
                    .table(ConversationSummaries::Table)
                    .col(ConversationSummaries::CardName)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_summary_created")
                    .table(ConversationSummaries::Table)
                    .col((ConversationSummaries::CreatedAt, IndexOrder::Desc))
                    .to_owned(),
            )
            .await?;

        // 2. conversation_keyfacts
        manager
            .create_table(
                Table::create()
                    .table(ConversationKeyFacts::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ConversationKeyFacts::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConversationKeyFacts::CardName)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationKeyFacts::SummaryId)
                            .integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConversationKeyFacts::Key)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationKeyFacts::Value)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationKeyFacts::CreatedAt)
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
                    .name("idx_keyfacts_card")
                    .table(ConversationKeyFacts::Table)
                    .col(ConversationKeyFacts::CardName)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_keyfacts_key")
                    .table(ConversationKeyFacts::Table)
                    .col(ConversationKeyFacts::CardName)
                    .col(ConversationKeyFacts::Key)
                    .to_owned(),
            )
            .await?;

        // 3. conversation_logs
        manager
            .create_table(
                Table::create()
                    .table(ConversationLogs::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ConversationLogs::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConversationLogs::SessionId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationLogs::CardName)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ConversationLogs::Role).string().not_null())
                    .col(
                        ColumnDef::new(ConversationLogs::Content)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationLogs::CreatedAt)
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
                    .name("idx_log_session")
                    .table(ConversationLogs::Table)
                    .col(ConversationLogs::SessionId)
                    .to_owned(),
            )
            .await?;

        // 4. tool_embedding_index
        manager
            .create_table(
                Table::create()
                    .table(ToolEmbeddingIndex::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ToolEmbeddingIndex::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ToolEmbeddingIndex::ToolName)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ToolEmbeddingIndex::Field)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ToolEmbeddingIndex::FieldKey)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(ToolEmbeddingIndex::VersionHash)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ToolEmbeddingIndex::ModelName)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(ToolEmbeddingIndex::SourceText)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(ToolEmbeddingIndex::Embedding)
                            .blob()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ToolEmbeddingIndex::CreatedAt)
                            .string()
                            .not_null(),
                    )
                    .check(Expr::col(ToolEmbeddingIndex::Field).is_in([
                        "summary",
                        "description",
                        "capability",
                        "example",
                        "negative",
                    ]))
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .unique()
                    .name("uniq_tool_embedding")
                    .table(ToolEmbeddingIndex::Table)
                    .col(ToolEmbeddingIndex::ToolName)
                    .col(ToolEmbeddingIndex::Field)
                    .col(ToolEmbeddingIndex::FieldKey)
                    .col(ToolEmbeddingIndex::ModelName)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_tei_lookup")
                    .table(ToolEmbeddingIndex::Table)
                    .col(ToolEmbeddingIndex::ToolName)
                    .col(ToolEmbeddingIndex::Field)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_tei_version")
                    .table(ToolEmbeddingIndex::Table)
                    .col(ToolEmbeddingIndex::ToolName)
                    .col(ToolEmbeddingIndex::Field)
                    .col(ToolEmbeddingIndex::VersionHash)
                    .to_owned(),
            )
            .await?;

        // 5. __tool_schemas
        manager
            .create_table(
                Table::create()
                    .table(ToolSchemas::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ToolSchemas::Prefix)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ToolSchemas::SchemaJson).string().not_null())
                    .col(ColumnDef::new(ToolSchemas::Fingerprint).string().not_null())
                    .col(
                        ColumnDef::new(ToolSchemas::CreatedAt)
                            .string()
                            .not_null()
                            .default(Expr::cust("(strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))")),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ToolSchemas::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ToolEmbeddingIndex::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ConversationLogs::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ConversationKeyFacts::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ConversationSummaries::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum ConversationSummaries {
    Table,
    Id,
    SessionId,
    CardName,
    Summary,
    Embedding,
    CreatedAt,
    EndedAt,
}

#[derive(Iden)]
enum ConversationKeyFacts {
    #[iden = "conversation_keyfacts"]
    Table,
    Id,
    CardName,
    SummaryId,
    Key,
    Value,
    CreatedAt,
}

#[derive(DeriveIden)]
enum ConversationLogs {
    Table,
    Id,
    SessionId,
    CardName,
    Role,
    Content,
    CreatedAt,
}

#[derive(DeriveIden)]
enum ToolEmbeddingIndex {
    Table,
    Id,
    ToolName,
    Field,
    FieldKey,
    VersionHash,
    ModelName,
    SourceText,
    Embedding,
    CreatedAt,
}

#[derive(Iden)]
enum ToolSchemas {
    #[iden = "__tool_schemas"]
    Table,
    Prefix,
    SchemaJson,
    Fingerprint,
    CreatedAt,
}
