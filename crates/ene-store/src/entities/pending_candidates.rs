//! Pending memory-candidate approval queue.
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
    pub source_turn: Option<String>,
    pub approval_parked: bool,
    pub existing_memory_id: Option<i64>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

/// Convert a stored row into the domain DTO, failing closed on an
/// unrecognized status label.
///
/// Returns `None` (logging a warning) when the stored status is not one of
/// `pending` / `approved` / `rejected`, so a corrupted row is excluded from
/// listings instead of being silently resurrected into the live pending queue
/// where it could be approved again.
#[must_use]
pub fn model_to_dto(m: Model) -> Option<PendingCandidate> {
    let Some(status) = PendingCandidateStatus::from_db_str(&m.status) else {
        tracing::warn!(
            component = "ene-store",
            candidate_id = m.id,
            status = %m.status,
            "Excluding pending candidate with unrecognized status label"
        );
        return None;
    };
    Some(PendingCandidate {
        id: m.id,
        character_id: m.character_id,
        user_id: m.user_id,
        title: m.title,
        content: m.content,
        kind: MemoryKind::from_db_str(&m.kind),
        confidence: m.confidence,
        reason_detail: m.reason_detail,
        // Display-only hint captured at insert time; it is not persisted, so a
        // row rehydrated from the DB resolves its conflict title by joining on
        // `existing_memory_id` at list time instead.
        existing_memory_title: None,
        existing_memory_id: m.existing_memory_id,
        source_quote: m.source_quote,
        source_turn: m.source_turn,
        approval_parked: m.approval_parked,
        status,
        created_at: m.created_at,
        resolved_at: m.resolved_at,
    })
}
