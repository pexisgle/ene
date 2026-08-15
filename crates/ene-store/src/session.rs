//! Session metadata domain model.
//!
//! Plain serde structs decoupled from the `SeaORM` entity. The `sessions`
//! table tracks per-session bookkeeping (card, title, archive flag, turn
//! count) so sessions can be listed, searched, archived, exported, and
//! imported without scanning the full `conversation_logs` table.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::entities;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: i64,
    /// Logical session key (unique across the store).
    pub session_id: String,
    /// Character card the session belongs to.
    pub card_name: String,
    /// Human-readable session title (may be empty until set).
    pub title: String,
    pub created_at: DateTime<Utc>,
    /// When the session was last touched (newest-first ordering key).
    pub updated_at: DateTime<Utc>,
    /// Whether the session is archived (hidden from default listings).
    pub archived: bool,
    /// Number of conversation turns recorded for the session.
    pub turn_count: i64,
}

impl From<entities::session::Model> for SessionMeta {
    fn from(m: entities::session::Model) -> Self {
        Self {
            id: m.id,
            session_id: m.session_id,
            card_name: m.card_name,
            title: m.title,
            created_at: m.created_at,
            updated_at: m.updated_at,
            archived: m.archived != 0,
            turn_count: m.turn_count,
        }
    }
}

/// Input for creating (or upserting) a session metadata row.
///
/// Timestamps and defaults (`archived = false`, `turn_count = 0`) are filled
/// in by the store on insert.
#[derive(Debug, Clone)]
pub struct NewSessionMeta {
    pub session_id: String,
    /// Character card the session belongs to.
    pub card_name: String,
    /// Human-readable session title (may be empty).
    pub title: String,
}
