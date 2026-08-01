use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "audit_log")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub turn_id: String,
    pub tool_name: String,
    pub action: String,
    pub target: String,
    pub decision: String,
    pub success: i32,
    pub redacted_args: String,
    pub created_at: DateTime<Utc>,
    /// Session that triggered the call (`None` for rows written before
    /// the `session_id` column existed).
    pub session_id: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
