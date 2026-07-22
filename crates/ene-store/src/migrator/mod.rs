#![expect(missing_docs, reason = "sea-orm migration modules are schema-internal")]

use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(Migration),
            Box::new(PendingMemoryWritesMigration),
            Box::new(SourceRefIndexMigration),
        ]
    }
}

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &'static str {
        "m20260719_000000_initial_schema"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // conversation_logs
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

        // tool_embedding_index
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

        // __tool_schemas
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

        // typed_memories
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
                    .col(
                        ColumnDef::new(TypedMemories::Pinned)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(TypedMemories::FadedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(TypedMemories::CommitmentId).integer().null())
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

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_typed_memories_commitment_id")
                    .table(TypedMemories::Table)
                    .col(TypedMemories::CommitmentId)
                    .to_owned(),
            )
            .await?;

        // memory_embeddings
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

        // memory_links
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

        // memory_spans
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

        // affect_states
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
                        ColumnDef::new(AffectStates::UserId)
                            .string()
                            .not_null()
                            .default(""),
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
                        ColumnDef::new(AffectStates::Trust)
                            .float()
                            .not_null()
                            .default(0.0),
                    )
                    .col(
                        ColumnDef::new(AffectStates::Affinity)
                            .float()
                            .not_null()
                            .default(0.0),
                    )
                    .col(
                        ColumnDef::new(AffectStates::Irritation)
                            .float()
                            .not_null()
                            .default(0.0),
                    )
                    .col(
                        ColumnDef::new(AffectStates::Curiosity)
                            .float()
                            .not_null()
                            .default(0.0),
                    )
                    .col(
                        ColumnDef::new(AffectStates::Fatigue)
                            .float()
                            .not_null()
                            .default(0.0),
                    )
                    .col(
                        ColumnDef::new(AffectStates::MoodLabel)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(AffectStates::LastExpression)
                            .string()
                            .not_null()
                            .default(""),
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

        // commitments
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
                    .col(ColumnDef::new(Commitments::CreatedAt).string().not_null())
                    .col(ColumnDef::new(Commitments::UpdatedAt).string().not_null())
                    .col(ColumnDef::new(Commitments::CompletedAt).string().null())
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

        // audit_log
        manager
            .create_table(
                Table::create()
                    .table(AuditLog::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AuditLog::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AuditLog::TurnId)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(ColumnDef::new(AuditLog::ToolName).string().not_null())
                    .col(
                        ColumnDef::new(AuditLog::Action)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(AuditLog::Target)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(AuditLog::Decision)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(AuditLog::Success)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(AuditLog::RedactedArgs)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(ColumnDef::new(AuditLog::CreatedAt).string().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_audit_log_created")
                    .table(AuditLog::Table)
                    .col((AuditLog::CreatedAt, IndexOrder::Desc))
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_audit_log_tool")
                    .table(AuditLog::Table)
                    .col(AuditLog::ToolName)
                    .to_owned(),
            )
            .await?;

        // sessions
        manager
            .create_table(
                Table::create()
                    .table(Sessions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Sessions::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Sessions::SessionId).string().not_null())
                    .col(
                        ColumnDef::new(Sessions::CardName)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(Sessions::Title)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(ColumnDef::new(Sessions::CreatedAt).string().not_null())
                    .col(ColumnDef::new(Sessions::UpdatedAt).string().not_null())
                    .col(
                        ColumnDef::new(Sessions::Archived)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Sessions::TurnCount)
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
                    .unique()
                    .name("uniq_sessions_session_id")
                    .table(Sessions::Table)
                    .col(Sessions::SessionId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_sessions_archived_updated")
                    .table(Sessions::Table)
                    .col(Sessions::Archived)
                    .col((Sessions::UpdatedAt, IndexOrder::Desc))
                    .to_owned(),
            )
            .await?;

        // pending_affect_proposals
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
            .drop_table(Table::drop().table(Sessions::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(AuditLog::Table).to_owned())
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(PendingAffectProposals::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(Commitments::Table).to_owned())
            .await?;
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
        manager
            .drop_table(Table::drop().table(ToolSchemas::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ToolEmbeddingIndex::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ConversationLogs::Table).to_owned())
            .await?;
        Ok(())
    }
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
    CommitmentId,
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
    CreatedAt,
    UpdatedAt,
    CompletedAt,
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

#[derive(Iden)]
enum AuditLog {
    #[iden = "audit_log"]
    Table,
    Id,
    TurnId,
    ToolName,
    Action,
    Target,
    Decision,
    Success,
    RedactedArgs,
    CreatedAt,
}

#[derive(Iden)]
enum Sessions {
    #[iden = "sessions"]
    Table,
    Id,
    SessionId,
    CardName,
    Title,
    CreatedAt,
    UpdatedAt,
    Archived,
    TurnCount,
}

/// Adds the deferred memory-write retry queue (#240).
pub struct PendingMemoryWritesMigration;

impl MigrationName for PendingMemoryWritesMigration {
    fn name(&self) -> &'static str {
        "m20260722_000001_pending_memory_writes"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for PendingMemoryWritesMigration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PendingMemoryWrites::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PendingMemoryWrites::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(PendingMemoryWrites::CharacterId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PendingMemoryWrites::UserId)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(PendingMemoryWrites::PayloadJson)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PendingMemoryWrites::Attempts)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(PendingMemoryWrites::MaxAttempts)
                            .integer()
                            .not_null()
                            .default(5),
                    )
                    .col(ColumnDef::new(PendingMemoryWrites::LastError).string())
                    .col(
                        ColumnDef::new(PendingMemoryWrites::Status)
                            .string()
                            .not_null()
                            .default("pending"),
                    )
                    .col(
                        ColumnDef::new(PendingMemoryWrites::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PendingMemoryWrites::NextRetryAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PendingMemoryWrites::UpdatedAt)
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
                    .name("idx_pending_memory_writes_retry")
                    .table(PendingMemoryWrites::Table)
                    .col(PendingMemoryWrites::Status)
                    .col(PendingMemoryWrites::NextRetryAt)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PendingMemoryWrites::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum PendingMemoryWrites {
    #[iden = "pending_memory_writes"]
    Table,
    Id,
    CharacterId,
    UserId,
    PayloadJson,
    Attempts,
    MaxAttempts,
    LastError,
    Status,
    CreatedAt,
    NextRetryAt,
    UpdatedAt,
}

/// Adds composite index on `(character_id, source_ref)` for `typed_memories`.
pub struct SourceRefIndexMigration;

impl MigrationName for SourceRefIndexMigration {
    fn name(&self) -> &'static str {
        "m20260722_000002_source_ref_index"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for SourceRefIndexMigration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_typed_mem_character_source_ref")
                    .table(TypedMemories::Table)
                    .col(TypedMemories::CharacterId)
                    .col(TypedMemories::SourceRef)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_typed_mem_character_source_ref")
                    .table(TypedMemories::Table)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
