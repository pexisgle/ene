use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "__tool_schemas")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub prefix: String,
    pub schema_json: String,
    pub fingerprint: String,
    pub created_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
