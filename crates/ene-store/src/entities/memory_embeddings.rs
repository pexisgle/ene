use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "memory_embeddings")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub memory_item_id: i64,
    pub model_name: String,
    pub field: String,
    #[sea_orm(column_type = "Blob")]
    pub embedding: Vec<u8>,
    pub created_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::typed_memories::Entity",
        from = "Column::MemoryItemId",
        to = "super::typed_memories::Column::Id"
    )]
    TypedMemories,
}

impl Related<super::typed_memories::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::TypedMemories.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
