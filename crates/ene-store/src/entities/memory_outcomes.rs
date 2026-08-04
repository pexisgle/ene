use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "memory_outcomes")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub memory_id: i64,
    pub memory_title: String,
    pub character_id: String,
    pub user_id: String,
    pub rating: f32,
    pub source: String,
    pub source_ref: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
