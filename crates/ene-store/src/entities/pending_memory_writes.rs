//! Pending deferred memory-write queue.
//!
//! Domain DTOs (`PendingMemoryWrite`, `PendingMemoryWriteStatus`) live in
//! `ene-core`; this module owns only the `SeaORM` entity and the
//! model-to-DTO conversion.

use chrono::{DateTime, Utc};
use ene_core::{PendingMemoryWrite, PendingMemoryWriteStatus};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "pending_memory_writes")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i64,
    pub character_id: String,
    pub user_id: String,
    pub payload_json: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub last_error: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub next_retry_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl From<Model> for PendingMemoryWrite {
    fn from(m: Model) -> Self {
        Self {
            id: m.id,
            character_id: m.character_id,
            user_id: m.user_id,
            payload_json: m.payload_json,
            attempts: m.attempts,
            max_attempts: m.max_attempts,
            last_error: m.last_error,
            status: PendingMemoryWriteStatus::parse(&m.status)
                .unwrap_or(PendingMemoryWriteStatus::Pending),
            created_at: m.created_at,
            next_retry_at: m.next_retry_at,
            updated_at: m.updated_at,
        }
    }
}
