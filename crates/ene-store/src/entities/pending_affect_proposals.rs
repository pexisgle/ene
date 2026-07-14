use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "pending_affect_proposals")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub character_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: String,
    pub source_turn_id: i64,
    pub proposal_json: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
