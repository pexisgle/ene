#![allow(missing_docs)]

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(Migration),
            Box::new(Migration2),
            Box::new(Migration3),
            Box::new(Migration4),
            Box::new(Migration5),
            Box::new(Migration6),
            Box::new(Migration7),
            Box::new(Migration8),
        ]
    }
}

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20250628_000000_initial_schema"
    }
}

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
                    .col(ColumnDef::new(ToolSchemas::CreatedAt).string().not_null())
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

// ── Migration 2: Typed Memory ─────────────────────────────────────────────────

pub struct Migration2;

impl MigrationName for Migration2 {
    fn name(&self) -> &str {
        "m20250629_000000_typed_memory_schema"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration2 {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 1. typed_memories
        manager
            .create_table(
                Table::create()
                    .table(TypedMemories::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(TypedMemories::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(TypedMemories::Scope)
                            .string()
                            .not_null()
                            .default("character"),
                    )
                    .col(
                        ColumnDef::new(TypedMemories::CharacterId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TypedMemories::UserId)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(ColumnDef::new(TypedMemories::Kind).string().not_null())
                    .col(ColumnDef::new(TypedMemories::Title).string().not_null())
                    .col(ColumnDef::new(TypedMemories::Content).string().not_null())
                    .col(ColumnDef::new(TypedMemories::Source).string().not_null())
                    .col(ColumnDef::new(TypedMemories::SourceRef).string().null())
                    .col(
                        ColumnDef::new(TypedMemories::Confidence)
                            .float()
                            .not_null()
                            .default(0.5),
                    )
                    .col(
                        ColumnDef::new(TypedMemories::Salience)
                            .float()
                            .not_null()
                            .default(0.5),
                    )
                    .col(
                        ColumnDef::new(TypedMemories::AffectiveValence)
                            .float()
                            .not_null()
                            .default(0.0),
                    )
                    .col(
                        ColumnDef::new(TypedMemories::AffectiveArousal)
                            .float()
                            .not_null()
                            .default(0.0),
                    )
                    .col(
                        ColumnDef::new(TypedMemories::RelationshipImpact)
                            .float()
                            .not_null()
                            .default(0.0),
                    )
                    .col(
                        ColumnDef::new(TypedMemories::AccessCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(TypedMemories::LastAccessedAt)
                            .string()
                            .null(),
                    )
                    .col(ColumnDef::new(TypedMemories::CreatedAt).string().not_null())
                    .col(ColumnDef::new(TypedMemories::UpdatedAt).string().not_null())
                    .col(ColumnDef::new(TypedMemories::ValidFrom).string().null())
                    .col(ColumnDef::new(TypedMemories::ValidUntil).string().null())
                    .col(
                        ColumnDef::new(TypedMemories::Status)
                            .string()
                            .not_null()
                            .default("active"),
                    )
                    .col(ColumnDef::new(TypedMemories::SupersedesId).integer().null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_typed_mem_character_status")
                    .table(TypedMemories::Table)
                    .col(TypedMemories::CharacterId)
                    .col(TypedMemories::Status)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_typed_mem_kind")
                    .table(TypedMemories::Table)
                    .col(TypedMemories::Kind)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_typed_mem_created")
                    .table(TypedMemories::Table)
                    .col((TypedMemories::CreatedAt, IndexOrder::Desc))
                    .to_owned(),
            )
            .await?;

        // 2. memory_embeddings
        manager
            .create_table(
                Table::create()
                    .table(MemoryEmbeddings::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(MemoryEmbeddings::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(MemoryEmbeddings::MemoryItemId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MemoryEmbeddings::ModelName)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MemoryEmbeddings::Field)
                            .string()
                            .not_null()
                            .default("content"),
                    )
                    .col(
                        ColumnDef::new(MemoryEmbeddings::Embedding)
                            .blob()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MemoryEmbeddings::CreatedAt)
                            .string()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_memory_embeddings_item")
                            .from(MemoryEmbeddings::Table, MemoryEmbeddings::MemoryItemId)
                            .to(TypedMemories::Table, TypedMemories::Id)
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
                    .name("uniq_memory_embedding")
                    .table(MemoryEmbeddings::Table)
                    .col(MemoryEmbeddings::MemoryItemId)
                    .col(MemoryEmbeddings::ModelName)
                    .col(MemoryEmbeddings::Field)
                    .to_owned(),
            )
            .await?;

        // 3. memory_links
        manager
            .create_table(
                Table::create()
                    .table(MemoryLinks::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(MemoryLinks::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(MemoryLinks::FromId).integer().not_null())
                    .col(ColumnDef::new(MemoryLinks::ToId).integer().not_null())
                    .col(ColumnDef::new(MemoryLinks::Relation).string().not_null())
                    .col(
                        ColumnDef::new(MemoryLinks::Weight)
                            .float()
                            .not_null()
                            .default(1.0),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_memory_links_from")
                            .from(MemoryLinks::Table, MemoryLinks::FromId)
                            .to(TypedMemories::Table, TypedMemories::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_memory_links_to")
                            .from(MemoryLinks::Table, MemoryLinks::ToId)
                            .to(TypedMemories::Table, TypedMemories::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_memory_links_from")
                    .table(MemoryLinks::Table)
                    .col(MemoryLinks::FromId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_memory_links_to")
                    .table(MemoryLinks::Table)
                    .col(MemoryLinks::ToId)
                    .to_owned(),
            )
            .await?;

        // 4. memory_spans
        manager
            .create_table(
                Table::create()
                    .table(MemorySpans::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(MemorySpans::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(MemorySpans::SessionId).string().not_null())
                    .col(ColumnDef::new(MemorySpans::TurnStart).integer().not_null())
                    .col(ColumnDef::new(MemorySpans::TurnEnd).integer().not_null())
                    .col(ColumnDef::new(MemorySpans::RawExcerpt).string().null())
                    .col(
                        ColumnDef::new(MemorySpans::CompressedSummary)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(MemorySpans::CompressionLevel)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_memory_spans_session")
                    .table(MemorySpans::Table)
                    .col(MemorySpans::SessionId)
                    .col(MemorySpans::TurnStart)
                    .to_owned(),
            )
            .await?;

        // 5. affect_states
        manager
            .create_table(
                Table::create()
                    .table(AffectStates::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AffectStates::CharacterId)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AffectStates::Valence)
                            .float()
                            .not_null()
                            .default(0.0),
                    )
                    .col(
                        ColumnDef::new(AffectStates::Arousal)
                            .float()
                            .not_null()
                            .default(0.0),
                    )
                    .col(
                        ColumnDef::new(AffectStates::Dominance)
                            .float()
                            .not_null()
                            .default(0.0),
                    )
                    .col(
                        ColumnDef::new(AffectStates::DiscreteEmotions)
                            .string()
                            .not_null()
                            .default("[]"),
                    )
                    .col(ColumnDef::new(AffectStates::UpdatedAt).string().not_null())
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AffectStates::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(MemorySpans::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(MemoryLinks::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(MemoryEmbeddings::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(TypedMemories::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum TypedMemories {
    Table,
    Id,
    Scope,
    CharacterId,
    UserId,
    Kind,
    Title,
    Content,
    Source,
    SourceRef,
    Confidence,
    Salience,
    AffectiveValence,
    AffectiveArousal,
    RelationshipImpact,
    AccessCount,
    LastAccessedAt,
    CreatedAt,
    UpdatedAt,
    ValidFrom,
    ValidUntil,
    Status,
    SupersedesId,
    Pinned,
    FadedAt,
}

#[derive(Iden)]
enum MemoryEmbeddings {
    #[iden = "memory_embeddings"]
    Table,
    Id,
    MemoryItemId,
    ModelName,
    Field,
    Embedding,
    CreatedAt,
}

#[derive(Iden)]
enum MemoryLinks {
    #[iden = "memory_links"]
    Table,
    Id,
    FromId,
    ToId,
    Relation,
    Weight,
}

#[derive(Iden)]
enum MemorySpans {
    #[iden = "memory_spans"]
    Table,
    Id,
    SessionId,
    TurnStart,
    TurnEnd,
    RawExcerpt,
    CompressedSummary,
    CompressionLevel,
}

#[derive(Iden)]
#[allow(dead_code)]
enum AffectStates {
    #[iden = "affect_states"]
    Table,
    CharacterId,
    UserId,
    Valence,
    Arousal,
    Dominance,
    Trust,
    Affinity,
    Irritation,
    Curiosity,
    Fatigue,
    MoodLabel,
    LastExpression,
    DiscreteEmotions,
    UpdatedAt,
}

// ── Migration 3: AffectState relationship fields ────────────────────────────

pub struct Migration3;

impl MigrationName for Migration3 {
    fn name(&self) -> &str {
        "m20250629_000001_affect_state_fields"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration3 {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_column("affect_states", "user_id").await? {
            let stmt = Table::alter()
                .table(AffectStates::Table)
                .add_column(
                    ColumnDef::new(AffectStates::UserId)
                        .string()
                        .not_null()
                        .default(""),
                )
                .to_owned();
            manager.alter_table(stmt).await?;
        }
        if !manager.has_column("affect_states", "trust").await? {
            let stmt = Table::alter()
                .table(AffectStates::Table)
                .add_column(
                    ColumnDef::new(AffectStates::Trust)
                        .float()
                        .not_null()
                        .default(0.0),
                )
                .to_owned();
            manager.alter_table(stmt).await?;
        }
        if !manager.has_column("affect_states", "affinity").await? {
            let stmt = Table::alter()
                .table(AffectStates::Table)
                .add_column(
                    ColumnDef::new(AffectStates::Affinity)
                        .float()
                        .not_null()
                        .default(0.0),
                )
                .to_owned();
            manager.alter_table(stmt).await?;
        }
        if !manager.has_column("affect_states", "irritation").await? {
            let stmt = Table::alter()
                .table(AffectStates::Table)
                .add_column(
                    ColumnDef::new(AffectStates::Irritation)
                        .float()
                        .not_null()
                        .default(0.0),
                )
                .to_owned();
            manager.alter_table(stmt).await?;
        }
        if !manager.has_column("affect_states", "curiosity").await? {
            let stmt = Table::alter()
                .table(AffectStates::Table)
                .add_column(
                    ColumnDef::new(AffectStates::Curiosity)
                        .float()
                        .not_null()
                        .default(0.0),
                )
                .to_owned();
            manager.alter_table(stmt).await?;
        }
        if !manager.has_column("affect_states", "fatigue").await? {
            let stmt = Table::alter()
                .table(AffectStates::Table)
                .add_column(
                    ColumnDef::new(AffectStates::Fatigue)
                        .float()
                        .not_null()
                        .default(0.0),
                )
                .to_owned();
            manager.alter_table(stmt).await?;
        }
        if !manager.has_column("affect_states", "mood_label").await? {
            let stmt = Table::alter()
                .table(AffectStates::Table)
                .add_column(
                    ColumnDef::new(AffectStates::MoodLabel)
                        .string()
                        .not_null()
                        .default(""),
                )
                .to_owned();
            manager.alter_table(stmt).await?;
        }
        if !manager
            .has_column("affect_states", "last_expression")
            .await?
        {
            let stmt = Table::alter()
                .table(AffectStates::Table)
                .add_column(
                    ColumnDef::new(AffectStates::LastExpression)
                        .string()
                        .not_null()
                        .default(""),
                )
                .to_owned();
            manager.alter_table(stmt).await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite doesn't support DROP COLUMN, so the down migration is a no-op.
        // The columns remain but are ignored by older code versions.
        let _ = manager;
        Ok(())
    }
}

// ── Migration 4: Companion Commitment Ledger ────────────────────────────────

pub struct Migration4;

impl MigrationName for Migration4 {
    fn name(&self) -> &str {
        "m20250703_000000_commitments"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration4 {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Commitments::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Commitments::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Commitments::CharacterId).string().not_null())
                    .col(
                        ColumnDef::new(Commitments::UserId)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(ColumnDef::new(Commitments::Title).string().not_null())
                    .col(ColumnDef::new(Commitments::Description).string().not_null())
                    .col(
                        ColumnDef::new(Commitments::Status)
                            .string()
                            .not_null()
                            .default("active"),
                    )
                    .col(ColumnDef::new(Commitments::DueAt).string().null())
                    .col(ColumnDef::new(Commitments::DueLabel).string().null())
                    .col(ColumnDef::new(Commitments::SourceMemoryId).integer().null())
                    .col(ColumnDef::new(Commitments::CreatedAt).string().not_null())
                    .col(ColumnDef::new(Commitments::UpdatedAt).string().not_null())
                    .col(ColumnDef::new(Commitments::CompletedAt).string().null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_commitments_source_memory")
                            .from(Commitments::Table, Commitments::SourceMemoryId)
                            .to(TypedMemories::Table, TypedMemories::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_commitments_character_status_due")
                    .table(Commitments::Table)
                    .col(Commitments::CharacterId)
                    .col(Commitments::Status)
                    .col(Commitments::DueAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .unique()
                    .name("uniq_commitments_source_memory")
                    .table(Commitments::Table)
                    .col(Commitments::SourceMemoryId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Commitments::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum Commitments {
    #[iden = "commitments"]
    Table,
    Id,
    CharacterId,
    UserId,
    Title,
    Description,
    Status,
    DueAt,
    DueLabel,
    SourceMemoryId,
    CreatedAt,
    UpdatedAt,
    CompletedAt,
}

// ── Migration 5: Typed memory pin flag (#76) ────────────────────────────────

pub struct Migration5;

impl MigrationName for Migration5 {
    fn name(&self) -> &str {
        "m20250705_000000_typed_memory_pinned"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration5 {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_column("typed_memories", "pinned").await? {
            let stmt = Table::alter()
                .table(TypedMemories::Table)
                .add_column(
                    ColumnDef::new(TypedMemories::Pinned)
                        .integer()
                        .not_null()
                        .default(0),
                )
                .to_owned();
            manager.alter_table(stmt).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let _ = manager;
        Ok(())
    }
}

// ── Migration 6: Typed memory faded_at timestamp (#76) ────────────────────────

pub struct Migration6;

impl MigrationName for Migration6 {
    fn name(&self) -> &str {
        "m20250705_000001_typed_memory_faded_at"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration6 {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_column("typed_memories", "faded_at").await? {
            let stmt = Table::alter()
                .table(TypedMemories::Table)
                .add_column(ColumnDef::new(TypedMemories::FadedAt).timestamp_with_time_zone())
                .to_owned();
            manager.alter_table(stmt).await?;

            // Backfill existing faded rows so archive decay has a stable anchor.
            let backfill = Query::update()
                .table(TypedMemories::Table)
                .value(TypedMemories::FadedAt, Expr::col(TypedMemories::UpdatedAt))
                .and_where(Expr::col(TypedMemories::Status).eq("faded"))
                .and_where(Expr::col(TypedMemories::FadedAt).is_null())
                .to_owned();
            manager.exec_stmt(backfill).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let _ = manager;
        Ok(())
    }
}

// ── Migration 7: Legacy migration metadata (#98) ─────────────────────────────

pub struct Migration7;

impl MigrationName for Migration7 {
    fn name(&self) -> &str {
        "m20250705_000002_memory_migration_meta"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration7 {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(MemoryMigrationMeta::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(MemoryMigrationMeta::CardName)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(MemoryMigrationMeta::MigratedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MemoryMigrationMeta::LegacySummariesCount)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MemoryMigrationMeta::LegacyKeyfactsCount)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MemoryMigrationMeta::LegacyLogsCount)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MemoryMigrationMeta::Strategy)
                            .string()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(MemoryMigrationMeta::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum MemoryMigrationMeta {
    #[iden = "memory_migration_meta"]
    Table,
    CardName,
    MigratedAt,
    LegacySummariesCount,
    LegacyKeyfactsCount,
    LegacyLogsCount,
    Strategy,
}

// ── Migration 8: Pending affect proposals (#88 async post-turn) ──────────────

pub struct Migration8;

impl MigrationName for Migration8 {
    fn name(&self) -> &str {
        "m20260709_000000_pending_affect_proposals"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration8 {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PendingAffectProposals::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PendingAffectProposals::CharacterId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PendingAffectProposals::UserId)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(PendingAffectProposals::SourceTurnId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PendingAffectProposals::ProposalJson)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PendingAffectProposals::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(PendingAffectProposals::CharacterId)
                            .col(PendingAffectProposals::UserId),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(PendingAffectProposals::Table)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum PendingAffectProposals {
    #[iden = "pending_affect_proposals"]
    Table,
    CharacterId,
    UserId,
    SourceTurnId,
    ProposalJson,
    CreatedAt,
}
