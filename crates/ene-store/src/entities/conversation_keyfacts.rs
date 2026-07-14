//! Legacy `conversation_keyfacts` entity.
//!
//! Retained for backward compatibility during migration from the old
//! summary/keyfact storage model to the typed memory system (#121).
//! New code paths in `ene-mind` write to `typed_memories` instead.
//! This table and `conversation_summaries` are read-only after migration
//! and will be removed in a future schema version once all users have
//! completed the transition.

use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "conversation_keyfacts")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub card_name: String,
    pub summary_id: Option<i64>,
    pub key: String,
    pub value: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
