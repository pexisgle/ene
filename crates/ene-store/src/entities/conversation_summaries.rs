//! Legacy `conversation_summaries` entity.
//!
//! Retained for backward compatibility during migration from the old
//! summary/keyfact storage model to the typed memory system (#121).
//! New code paths in `ene-mind` write to `typed_memories` instead.
//! This table and `conversation_keyfacts` are read-only after migration
//! and will be removed in a future schema version once all users have
//! completed the transition.

use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "conversation_summaries")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub session_id: String,
    pub card_name: String,
    pub summary: String,
    #[sea_orm(column_type = "Blob")]
    pub embedding: Vec<u8>,
    pub created_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
