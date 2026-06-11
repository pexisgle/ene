use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "conversation_logs")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub session_id: String,
    pub card_name: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
