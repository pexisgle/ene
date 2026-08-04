use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "workspace_document_files")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub root: String,
    pub path: String,
    pub size: i64,
    pub modified_at: DateTime<Utc>,
    pub content_hash: String,
    pub model_name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
