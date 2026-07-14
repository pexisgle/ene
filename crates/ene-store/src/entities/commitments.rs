use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "commitments")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub character_id: String,
    pub user_id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub due_at: Option<DateTime<Utc>>,
    pub due_label: Option<String>,
    pub source_memory_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::typed_memories::Entity",
        from = "Column::SourceMemoryId",
        to = "super::typed_memories::Column::Id",
        on_update = "NoAction",
        on_delete = "SetNull"
    )]
    TypedMemories,
}

impl Related<super::typed_memories::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::TypedMemories.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
