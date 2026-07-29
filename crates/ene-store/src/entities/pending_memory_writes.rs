//! Pending deferred memory-write queue (#240).

use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Lifecycle status of a queued memory write (#240).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingMemoryWriteStatus {
    /// Waiting for (re)try.
    Pending,
    /// Exhausted retries; needs user attention.
    Permanent,
}

impl PendingMemoryWriteStatus {
    /// Stable string label for persistence / display.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Permanent => "permanent",
        }
    }

    /// Parse a stored status label.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "pending" => Some(Self::Pending),
            "permanent" => Some(Self::Permanent),
            _ => None,
        }
    }
}

/// Domain row for a deferred memory-write retry (#240).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingMemoryWrite {
    /// Primary key.
    pub id: i64,
    /// Character scope.
    pub character_id: String,
    /// User scope.
    pub user_id: String,
    /// JSON-encoded [`OwnedPostTurnInput`]-compatible payload.
    pub payload_json: String,
    /// Attempts already made (including the original failure).
    pub attempts: i32,
    /// Maximum attempts before becoming permanent.
    pub max_attempts: i32,
    /// Last error message.
    pub last_error: Option<String>,
    /// Queue status.
    pub status: PendingMemoryWriteStatus,
    /// When the row was first created.
    pub created_at: DateTime<Utc>,
    /// Earliest time a retry should run.
    pub next_retry_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "pending_memory_writes")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i64,
    pub character_id: String,
    pub user_id: String,
    pub payload_json: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub last_error: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub next_retry_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl From<Model> for PendingMemoryWrite {
    fn from(m: Model) -> Self {
        Self {
            id: m.id,
            character_id: m.character_id,
            user_id: m.user_id,
            payload_json: m.payload_json,
            attempts: m.attempts,
            max_attempts: m.max_attempts,
            last_error: m.last_error,
            status: PendingMemoryWriteStatus::parse(&m.status)
                .unwrap_or(PendingMemoryWriteStatus::Pending),
            created_at: m.created_at,
            next_retry_at: m.next_retry_at,
            updated_at: m.updated_at,
        }
    }
}
