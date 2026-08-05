//! Memory Ledger presenter: table row model, filters, and kind distribution.

use chrono::{DateTime, Duration, Utc};
use ene_store::{MemoryItem, MemoryKind, MemoryStatus};

/// Created-date filter for the ledger table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CreatedWithinFilter {
    #[default]
    Any,
    Days7,
    Days30,
    Days90,
}

impl CreatedWithinFilter {
    /// Number of days the filter keeps, `None` for no date bound.
    pub const fn cutoff_days(self) -> Option<i64> {
        match self {
            Self::Any => None,
            Self::Days7 => Some(7),
            Self::Days30 => Some(30),
            Self::Days90 => Some(90),
        }
    }
}

/// A memory row rendered by the ledger table.
#[derive(Debug, Clone)]
pub struct MemoryLedgerRow {
    /// Database primary key.
    pub id: i64,
    /// Short title or label.
    pub title: String,
    /// Full memory content (draft source for the edit dialog).
    pub content: String,
    /// Content preview (truncated).
    pub content_preview: String,
    /// Memory kind (the category tag).
    pub kind: MemoryKind,
    /// Lifecycle status.
    pub status: MemoryStatus,
    /// Ownership scope string (`character` / `user` / `shared`).
    pub scope: String,
    /// When the memory was created.
    pub created_at: DateTime<Utc>,
    /// Salience / importance weight.
    pub salience: f32,
    /// Confidence score.
    pub confidence: f32,
    /// Pinned memories are exempt from natural decay.
    pub pinned: bool,
}

/// Pure presenter for the memory ledger page.
pub struct MemoryLedgerPresenter;

impl MemoryLedgerPresenter {
    /// Kinds offered by the ledger's kind filter (UI menu, not exhaustive).
    pub const ALL_KINDS: [MemoryKind; 10] = [
        MemoryKind::Episodic,
        MemoryKind::Semantic,
        MemoryKind::UserProfile,
        MemoryKind::Relationship,
        MemoryKind::Affective,
        MemoryKind::Commitment,
        MemoryKind::Preference,
        MemoryKind::Procedure,
        MemoryKind::WorldState,
        MemoryKind::Reflection,
    ];

    /// Build a ledger row from a typed memory item.
    pub fn row_from_item(item: &MemoryItem) -> MemoryLedgerRow {
        MemoryLedgerRow {
            id: item.id.unwrap_or_default(),
            title: item.title.clone(),
            content: item.content.clone(),
            content_preview: truncate_content(&item.content, 140),
            kind: item.kind,
            status: item.status,
            scope: item.scope.as_str().to_string(),
            created_at: item.created_at,
            salience: item.salience.get(),
            confidence: item.confidence.get(),
            pinned: item.pinned,
        }
    }

    /// Apply the ledger's text search and kind / status / created filters.
    ///
    /// Search matches the lowercased title and full content; the created
    /// filter is anchored at `now`.
    pub fn filter_rows(
        rows: &[MemoryLedgerRow],
        query: &str,
        kind: Option<MemoryKind>,
        status: Option<MemoryStatus>,
        created_within: CreatedWithinFilter,
        now: DateTime<Utc>,
    ) -> Vec<MemoryLedgerRow> {
        let query = query.trim().to_lowercase();
        let cutoff = created_within
            .cutoff_days()
            .map(|days| now - Duration::days(days));
        rows.iter()
            .filter(|row| {
                if kind.is_some_and(|kind| row.kind != kind) {
                    return false;
                }
                if status.is_some_and(|status| row.status != status) {
                    return false;
                }
                if cutoff.is_some_and(|cutoff| row.created_at < cutoff) {
                    return false;
                }
                if !query.is_empty() {
                    let haystack = format!("{} {}", row.title, row.content).to_lowercase();
                    if !haystack.contains(&query) {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect()
    }

    /// Count ledger rows per kind, most frequent first (ties by kind name).
    pub fn kind_distribution(rows: &[MemoryLedgerRow]) -> Vec<(MemoryKind, usize)> {
        let mut counts: Vec<(MemoryKind, usize)> = Vec::new();
        for row in rows {
            match counts.iter_mut().find(|(kind, _)| *kind == row.kind) {
                Some(entry) => entry.1 += 1,
                None => counts.push((row.kind, 1)),
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.as_str().cmp(b.0.as_str())));
        counts
    }

    /// Fluent i18n key for a kind's display label.
    pub const fn kind_label_key(kind: MemoryKind) -> &'static str {
        match kind {
            MemoryKind::Episodic => "memory-kind-episodic",
            MemoryKind::Semantic => "memory-kind-semantic",
            MemoryKind::UserProfile => "memory-kind-user-profile",
            MemoryKind::Relationship => "memory-kind-relationship",
            MemoryKind::Affective => "memory-kind-affective",
            MemoryKind::Commitment => "memory-kind-commitment",
            MemoryKind::Preference => "memory-kind-preference",
            MemoryKind::Procedure => "memory-kind-procedure",
            MemoryKind::WorldState => "memory-kind-world-state",
            MemoryKind::Reflection => "memory-kind-reflection",
            _ => "memory-kind-unknown",
        }
    }

    /// Fluent i18n key for a status's display label.
    pub const fn status_label_key(status: MemoryStatus) -> &'static str {
        match status {
            MemoryStatus::Active => "memory-ledger-status-active",
            MemoryStatus::Faded => "memory-ledger-status-faded",
            MemoryStatus::Archived => "memory-ledger-status-archived",
            MemoryStatus::Disputed => "memory-ledger-status-disputed",
            MemoryStatus::Superseded => "memory-ledger-status-superseded",
            MemoryStatus::UserDeleted => "memory-ledger-status-user-deleted",
            _ => "memory-ledger-status-unknown",
        }
    }
}

/// Truncate a long text to a display preview with an ellipsis.
fn truncate_content(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }
    let truncated: String = content.chars().take(max_chars).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use ene_store::{
        AffectAnnotation, MemoryConfidence, MemorySalience, MemoryScope, MemorySource,
    };

    fn row(
        id: i64,
        title: &str,
        kind: MemoryKind,
        status: MemoryStatus,
        created: DateTime<Utc>,
    ) -> MemoryLedgerRow {
        MemoryLedgerRow {
            id,
            title: title.into(),
            content: format!("full content of {title}"),
            content_preview: format!("preview of {title}"),
            kind,
            status,
            scope: "character".into(),
            created_at: created,
            salience: 0.5,
            confidence: 0.8,
            pinned: false,
        }
    }

    fn item(kind: MemoryKind) -> MemoryItem {
        MemoryItem {
            id: Some(1),
            scope: MemoryScope::Character,
            character_id: "Ene".into(),
            user_id: String::new(),
            kind,
            title: "title".into(),
            content: "content".into(),
            source: MemorySource::Conversation,
            source_ref: None,
            confidence: MemoryConfidence::new(0.8),
            salience: MemorySalience::new(0.5),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            access_count: 0,
            last_accessed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            faded_at: None,
            commitment_id: None,
        }
    }

    #[test]
    fn filter_rows_applies_search_kind_status_and_date() {
        let now = Utc.with_ymd_and_hms(2026, 8, 5, 12, 0, 0).unwrap();
        let rows = vec![
            row(
                1,
                "likes coffee",
                MemoryKind::Preference,
                MemoryStatus::Active,
                now - Duration::days(2),
            ),
            row(
                2,
                "old note",
                MemoryKind::Episodic,
                MemoryStatus::UserDeleted,
                now - Duration::days(60),
            ),
            row(
                3,
                "tea habit",
                MemoryKind::Preference,
                MemoryStatus::Faded,
                now - Duration::days(20),
            ),
        ];

        let by_kind = MemoryLedgerPresenter::filter_rows(
            &rows,
            "",
            Some(MemoryKind::Preference),
            None,
            CreatedWithinFilter::Any,
            now,
        );
        assert_eq!(by_kind.len(), 2);

        let by_status = MemoryLedgerPresenter::filter_rows(
            &rows,
            "",
            None,
            Some(MemoryStatus::UserDeleted),
            CreatedWithinFilter::Any,
            now,
        );
        assert_eq!(by_status.len(), 1);
        assert_eq!(by_status[0].id, 2);

        let by_date = MemoryLedgerPresenter::filter_rows(
            &rows,
            "",
            None,
            None,
            CreatedWithinFilter::Days30,
            now,
        );
        assert_eq!(by_date.len(), 2, "60-day-old row falls outside 30 days");

        let by_text = MemoryLedgerPresenter::filter_rows(
            &rows,
            "tea",
            None,
            None,
            CreatedWithinFilter::Any,
            now,
        );
        assert_eq!(by_text.len(), 1);
        assert_eq!(by_text[0].id, 3);
    }

    #[test]
    fn kind_distribution_counts_only_present_kinds() {
        let now = Utc::now();
        let rows = vec![
            row(1, "a", MemoryKind::Preference, MemoryStatus::Active, now),
            row(2, "b", MemoryKind::Preference, MemoryStatus::Active, now),
            row(3, "c", MemoryKind::Episodic, MemoryStatus::Active, now),
        ];
        let counts = MemoryLedgerPresenter::kind_distribution(&rows);
        assert_eq!(
            counts,
            vec![(MemoryKind::Preference, 2), (MemoryKind::Episodic, 1)]
        );
    }

    #[test]
    fn row_from_item_carries_salience_and_status() {
        let memory = item(MemoryKind::Preference);
        let row = MemoryLedgerPresenter::row_from_item(&memory);
        assert_eq!(row.kind, MemoryKind::Preference);
        assert_eq!(row.status, MemoryStatus::Active);
        assert!((row.salience - 0.5).abs() < f32::EPSILON);
        assert_eq!(row.content_preview, "content");
    }

    #[test]
    fn long_content_is_truncated_with_ellipsis() {
        let long = "x".repeat(500);
        let preview = truncate_content(&long, 100);
        assert_eq!(preview.chars().count(), 101);
        assert!(preview.ends_with('…'));
    }
}
