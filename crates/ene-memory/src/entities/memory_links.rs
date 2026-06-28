use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "memory_links")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i64,
    pub from_id: i64,
    pub to_id: i64,
    pub relation: String,
    pub weight: f32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::typed_memories::Entity",
        from = "Column::FromId",
        to = "super::typed_memories::Column::Id"
    )]
    FromTypedMemory,
    #[sea_orm(
        belongs_to = "super::typed_memories::Entity",
        from = "Column::ToId",
        to = "super::typed_memories::Column::Id"
    )]
    ToTypedMemory,
}

impl ActiveModelBehavior for ActiveModel {}
