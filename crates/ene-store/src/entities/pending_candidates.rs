//! Pending memory-candidate approval queue (#174, #420).
//!
//! Domain DTOs (`PendingCandidate`, `PendingCandidateStatus`) live in
//! `ene-core`; this module owns only the `SeaORM` entity and the
//! model-to-DTO conversion.

use chrono::{DateTime, Utc};
use ene_core::{MemoryKind, PendingCandidate, PendingCandidateStatus};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "pending_candidates")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i64,
    pub character_id: String,
    pub user_id: String,
    pub title: String,
    pub content: String,
    pub kind: String,
    pub confidence: f32,
    pub reason_detail: String,
    pub source_quote: String,
    pub existing_memory_id: Option<i64>,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl From<Model> for PendingCandidate {
    fn from(m: Model) -> Self {
        Self {
            id: m.id,
            character_id: m.character_id,
            user_id: m.user_id,
            title: m.title,
            content: m.content,
            kind: MemoryKind::from_db_str(&m.kind),
            confidence: m.confidence,
            reason_detail: m.reason_detail,
            existing_memory_title: None,
            existing_memory_id: m.existing_memory_id,
            source_quote: m.source_quote,
            status: PendingCandidateStatus::parse(&m.status)
                .unwrap_or(PendingCandidateStatus::Pending),
            created_at: m.created_at,
        }
    }
}
