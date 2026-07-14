use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "tool_embedding_index")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub tool_name: String,
    pub field: String,
    pub field_key: String,
    pub version_hash: String,
    pub model_name: String,
    pub source_text: String,
    #[sea_orm(column_type = "Blob")]
    pub embedding: Vec<u8>,
    pub created_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
