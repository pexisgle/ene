use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "workspace_document_chunks")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub file_id: i64,
    pub chunk_index: i32,
    pub heading: String,
    pub content: String,
    pub start_line: i32,
    pub end_line: i32,
    #[sea_orm(column_type = "Blob")]
    pub embedding: Vec<u8>,
    pub created_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
