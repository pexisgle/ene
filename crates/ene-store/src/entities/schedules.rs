use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "schedules")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub enabled: bool,
    pub timezone: String,
    pub cron_expr: Option<String>,
    pub interval_secs: Option<i64>,
    pub start_at: Option<DateTime<Utc>>,
    pub action: String,
    pub confirmation: String,
    pub max_retries: i64,
    pub retry_delay_secs: i64,
    pub next_run_at: Option<DateTime<Utc>>,
    pub pending_retry_of_run_id: Option<i64>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_status: Option<String>,
    pub run_count: i64,
    pub fail_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
