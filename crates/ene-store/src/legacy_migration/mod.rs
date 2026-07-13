//! One-shot migration orchestration from legacy memory tables.

mod detect;
pub(crate) mod keyfacts;
pub(crate) mod logs;
pub(crate) mod summaries;

pub use detect::{LegacyRowCounts, MigrationStatus};
pub use keyfacts::keyfact_kind_for_key;
pub use logs::{LegacyLogRow, logs_to_spans};

use crate::error::MemoryError;
use crate::store::MemoryStore;

/// Row types exposed for migration (internal to crate).
pub(crate) struct LegacySummaryRow {
    pub id: i64,
    pub summary: String,
    pub embedding: Vec<u8>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub(crate) struct LegacyKeyfactRow {
    pub id: i64,
    pub key: String,
    pub value: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub(crate) struct LegacyLogRowRaw {
    pub session_id: String,
    pub role: String,
    pub content: String,
}

/// Options controlling legacy migration execution.
#[derive(Debug, Clone)]
pub struct LegacyMigrationOptions {
    /// Character card name to migrate.
    pub card_name: String,
    /// User identifier stored on typed memories.
    pub user_id: String,
    /// Embedding model name for copied summary vectors.
    pub embedding_model: String,
    /// When true, report counts without writing.
    pub dry_run: bool,
}

/// Outcome of a legacy migration run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyMigrationReport {
    /// Summaries converted to Episodic memories.
    pub summaries_migrated: usize,
    /// Keyfacts converted to typed memories.
    pub keyfacts_migrated: usize,
    /// Log-derived memory spans inserted.
    pub spans_migrated: usize,
    /// Rows skipped because already migrated (matching `source_ref`).
    pub skipped_existing: usize,
}

/// Run one-shot legacy → typed migration for a character card.
pub async fn execute_legacy_migration(
    store: &MemoryStore,
    options: &LegacyMigrationOptions,
) -> Result<LegacyMigrationReport, MemoryError> {
    if store.is_legacy_migrated(&options.card_name).await? {
        return Err(MemoryError::LegacyAlreadyMigrated {
            card_name: options.card_name.clone(),
        });
    }

    let counts = store.count_legacy_rows(&options.card_name).await?;

    let summary_rows = store.list_legacy_summaries(&options.card_name).await?;
    let keyfact_rows = store.list_legacy_keyfacts(&options.card_name).await?;
    let log_rows = store.list_legacy_logs(&options.card_name).await?;
    let legacy_logs: Vec<LegacyLogRow> = log_rows
        .into_iter()
        .map(|r| LegacyLogRow {
            session_id: r.session_id,
            role: r.role,
            content: r.content,
        })
        .collect();
    let spans = logs::logs_to_spans(&legacy_logs);

    if options.dry_run {
        return Ok(LegacyMigrationReport {
            summaries_migrated: summary_rows.len(),
            keyfacts_migrated: keyfact_rows.iter().filter(|r| !r.value.is_empty()).count(),
            spans_migrated: spans.len(),
            skipped_existing: 0,
        });
    }

    store
        .apply_legacy_migration_writes(options, counts, summary_rows, keyfact_rows, spans)
        .await
}
