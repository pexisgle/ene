//! Legacy row detection types.

use chrono::{DateTime, Utc};

/// Counts of legacy rows for a character card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LegacyRowCounts {
    /// Rows in `conversation_summaries`.
    pub summaries: i64,
    /// Rows in `conversation_keyfacts`.
    pub keyfacts: i64,
    /// Rows in `conversation_logs`.
    pub logs: i64,
}

impl LegacyRowCounts {
    /// Returns true when any legacy table has rows for the card.
    #[must_use]
    pub fn has_legacy_data(self) -> bool {
        self.summaries > 0 || self.keyfacts > 0 || self.logs > 0
    }

    /// Returns true when unmigrated legacy summaries or keyfacts remain.
    ///
    /// Ongoing `conversation_logs` writes are excluded — they are expected on the
    /// cognitive path and must not block `require_migration` strict mode.
    #[must_use]
    pub fn requires_migration_gate(self) -> bool {
        self.summaries > 0 || self.keyfacts > 0
    }
}

/// Recorded migration outcome for a character card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationStatus {
    /// Character card name.
    pub card_name: String,
    /// When migration completed.
    pub migrated_at: DateTime<Utc>,
    /// Summaries migrated.
    pub legacy_summaries_count: i32,
    /// Keyfacts migrated.
    pub legacy_keyfacts_count: i32,
    /// Logs migrated.
    pub legacy_logs_count: i32,
    /// Strategy label (e.g. `"one_shot"`).
    pub strategy: String,
}
