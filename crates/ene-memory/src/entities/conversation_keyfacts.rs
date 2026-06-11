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
    pub created_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
