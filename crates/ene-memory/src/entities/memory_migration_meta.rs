use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "memory_migration_meta")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub card_name: String,
    pub migrated_at: DateTime<Utc>,
    pub legacy_summaries_count: i32,
    pub legacy_keyfacts_count: i32,
    pub legacy_logs_count: i32,
    pub strategy: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
