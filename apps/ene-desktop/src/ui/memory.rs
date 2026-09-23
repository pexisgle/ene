//! Memory management view-model: a Host-paged projection, not a store.
//!
//! The GUI does not know the DB schema. Current recognition, scope,
//! importance, created-at, Experience Summary grounds, and revision history
//! come from [`ManagementViewRequest`] fields `memory`, `memory_after`,
//! `memory_revisions_of`, and `memory_revisions_after`. The Host applies the
//! page bound; this module never full-scans then truncates.
//!
//! Formation, correction, situation change, merge, and conversational forget
//! stay on Learning / Companion. This page has no write, no physical-delete
//! shortcut, and no Targeted Deletion path.

use ene_api::v1::management::{ManagementView, ManagementViewRequest};

/// Host section name for the read-only Memory view.
pub const HOST_MEMORY_SECTION: &str = "memory";

/// One current Memory row as the Host rendered it.
#[derive(Clone, PartialEq, Eq, serde::Serialize)]
pub struct MemoryRow {
    pub id: String,
    pub scope: String,
    pub importance: String,
    pub temporal: String,
    pub recall: String,
    pub revision: String,
    /// Host list publishes this as `updated=`. For an initial revision it is
    /// the creation time; later revisions keep the current-row timestamp here
    /// and the per-change created-at on [`MemoryRevisionRow::at`].
    pub created_at: String,
    pub content: String,
}

impl core::fmt::Debug for MemoryRow {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("MemoryRow")
            .field("id", &self.id)
            .field("scope", &self.scope)
            .field("importance", &self.importance)
            .field("temporal", &self.temporal)
            .field("recall", &self.recall)
            .field("revision", &self.revision)
            .field("created_at", &self.created_at)
            .field("content", &"[redacted]")
            .finish()
    }
}

/// One revision of a Memory, including Experience Summary grounds when the
/// Host included them on this page.
#[derive(Clone, PartialEq, Eq, serde::Serialize)]
pub struct MemoryRevisionRow {
    pub revision: u64,
    pub change: String,
    pub at: String,
    pub content: String,
    pub grounds_summary: Option<String>,
    pub grounds: Option<String>,
}

impl core::fmt::Debug for MemoryRevisionRow {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("MemoryRevisionRow")
            .field("revision", &self.revision)
            .field("change", &self.change)
            .field("at", &self.at)
            .field("content", &"[redacted]")
            .field("grounds_summary", &self.grounds_summary)
            .field("grounds", &self.grounds.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

/// One Host Memory page (list or revisions), plus the cursor to ask for the
/// next page. Accumulating pages happens only when the caller requests the
/// next Host page; nothing here walks the whole corpus.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryPage {
    rows: Vec<MemoryRow>,
    next_after: Option<String>,
    revisions_of: Option<String>,
    revisions: Vec<MemoryRevisionRow>,
    next_revision_after: Option<u64>,
    notice: Option<String>,
    last_request: Option<ManagementViewRequest>,
}

impl MemoryPage {
    /// Typed current-list read. `after` is the Host `next: <id>` cursor.
    #[must_use]
    pub fn list_request(after: Option<&str>) -> ManagementViewRequest {
        ManagementViewRequest {
            sections: vec![String::from(HOST_MEMORY_SECTION)],
            memory_after: after.map(str::to_owned),
            memory_revisions_of: None,
            memory_revisions_after: None,
        }
    }

    /// Typed revision-history read for one Memory. `after_revision` is the
    /// Host `next-revision: <n>` cursor. The list cursor is not mixed in.
    #[must_use]
    pub fn revisions_request(
        memory_id: &str,
        after_revision: Option<u64>,
    ) -> ManagementViewRequest {
        ManagementViewRequest {
            sections: vec![String::from(HOST_MEMORY_SECTION)],
            memory_after: None,
            memory_revisions_of: Some(memory_id.to_owned()),
            memory_revisions_after: after_revision,
        }
    }

    /// Replace or append from one Host answer. `append` is only for a caller
    /// that already consumed this page's `next` cursor; it is not a scan.
    pub fn apply_host_view(
        &mut self,
        view: &ManagementView,
        request: ManagementViewRequest,
        append: bool,
    ) {
        let body = view
            .sections
            .iter()
            .find(|section| section.kind == HOST_MEMORY_SECTION)
            .map(|section| section.body.as_str())
            .unwrap_or("");
        if request.memory_revisions_of.is_some() {
            self.revisions_of = request.memory_revisions_of.clone();
            apply_revision_body(self, body, append);
        } else if append {
            apply_list_body(self, body, true);
        } else {
            self.revisions.clear();
            self.revisions_of = None;
            self.next_revision_after = None;
            apply_list_body(self, body, false);
        }
        self.last_request = Some(request);
    }

    #[must_use]
    pub fn rows(&self) -> &[MemoryRow] {
        &self.rows
    }

    #[must_use]
    pub fn next_after(&self) -> Option<&str> {
        self.next_after.as_deref()
    }

    #[must_use]
    pub fn revisions_of(&self) -> Option<&str> {
        self.revisions_of.as_deref()
    }

    #[must_use]
    pub fn revisions(&self) -> &[MemoryRevisionRow] {
        &self.revisions
    }

    #[must_use]
    pub fn next_revision_after(&self) -> Option<u64> {
        self.next_revision_after
    }

    #[must_use]
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    #[must_use]
    pub fn last_request(&self) -> Option<&ManagementViewRequest> {
        self.last_request.as_ref()
    }

    /// Drops every Host-projected Memory copy this page holds. Old cursors
    /// and revision views are invalid after this.
    pub fn wipe(&mut self) {
        *self = Self::default();
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.revisions.is_empty()
    }

    /// Slint projection of the current Host page(s) already fetched.
    #[must_use]
    pub fn panel(&self) -> String {
        let mut lines = Vec::new();
        if self.rows.is_empty()
            && self.revisions.is_empty()
            && let Some(notice) = &self.notice
        {
            return notice.clone();
        }
        for row in &self.rows {
            lines.push(format!(
                "memory {} scope={} importance={} temporal={} recall={} revision={} created-at={}",
                row.id,
                row.scope,
                row.importance,
                row.temporal,
                row.recall,
                row.revision,
                row.created_at
            ));
            lines.push(format!("content: {}", row.content));
        }
        if let Some(next) = &self.next_after {
            lines.push(format!("next: {next}"));
        }
        if let Some(memory_id) = &self.revisions_of {
            lines.push(format!("revisions of {memory_id}"));
            for revision in &self.revisions {
                lines.push(format!(
                    "  rev{} {} at={} content: {}",
                    revision.revision, revision.change, revision.at, revision.content
                ));
                match (
                    revision.grounds_summary.as_deref(),
                    revision.grounds.as_deref(),
                ) {
                    (Some(summary), Some(grounds)) => {
                        lines.push(format!("  grounds summary {summary}: {grounds}"));
                    }
                    (_, Some(grounds)) => lines.push(format!("  grounds: {grounds}")),
                    _ => {}
                }
            }
            if let Some(next) = self.next_revision_after {
                lines.push(format!("next-revision: {next}"));
            }
        }
        lines.join("\n")
    }
}

fn apply_list_body(page: &mut MemoryPage, body: &str, append: bool) {
    if !append {
        page.rows.clear();
        page.next_after = None;
        page.notice = None;
    }
    if let Some(notice) = status_notice(body) {
        page.next_after = None;
        if page.rows.is_empty() {
            page.notice = Some(notice);
        }
        return;
    }
    let (rows, next) = parse_list_body(body);
    page.rows.extend(rows);
    page.next_after = next;
}

fn apply_revision_body(page: &mut MemoryPage, body: &str, append: bool) {
    if !append {
        page.revisions.clear();
        page.next_revision_after = None;
        page.notice = None;
    }
    if let Some(notice) = status_notice(body) {
        page.next_revision_after = None;
        if page.revisions.is_empty() {
            page.notice = Some(notice);
        }
        return;
    }
    let (revisions, next) = parse_revision_body(body);
    page.revisions.extend(revisions);
    page.next_revision_after = next;
}

fn status_notice(body: &str) -> Option<String> {
    match body {
        "" | "(none)" | "unavailable" | "invalid cursor" | "no older memories"
        | "no more revisions" | "unknown memory" => Some(if body.is_empty() {
            String::from("unavailable")
        } else {
            body.to_owned()
        }),
        _ => None,
    }
}

fn parse_list_body(body: &str) -> (Vec<MemoryRow>, Option<String>) {
    let mut rows = Vec::new();
    let mut next = None;
    let mut current: Option<MemoryRow> = None;
    for line in body.lines() {
        if let Some(cursor) = line.strip_prefix("next: ") {
            next = Some(cursor.to_owned());
            continue;
        }
        if let Some(header) = line.strip_prefix("memory ") {
            if let Some(row) = current.take() {
                rows.push(row);
            }
            current = parse_memory_header(header);
            continue;
        }
        if let Some(content) = line.strip_prefix("content: ")
            && let Some(row) = current.as_mut()
        {
            row.content = content.to_owned();
        }
    }
    if let Some(row) = current {
        rows.push(row);
    }
    (rows, next)
}

fn parse_memory_header(header: &str) -> Option<MemoryRow> {
    let mut parts = header.split_whitespace();
    let id = parts.next()?.to_owned();
    if id.is_empty() {
        return None;
    }
    let mut row = MemoryRow {
        id,
        scope: String::new(),
        importance: String::new(),
        temporal: String::new(),
        recall: String::new(),
        revision: String::new(),
        created_at: String::new(),
        content: String::new(),
    };
    for part in parts {
        if let Some(value) = part.strip_prefix("scope=") {
            row.scope = value.to_owned();
        } else if let Some(value) = part.strip_prefix("importance=") {
            row.importance = value.to_owned();
        } else if let Some(value) = part.strip_prefix("temporal=") {
            row.temporal = value.to_owned();
        } else if let Some(value) = part.strip_prefix("recall=") {
            row.recall = value.to_owned();
        } else if let Some(value) = part.strip_prefix("revision=") {
            row.revision = value.to_owned();
        } else if let Some(value) = part.strip_prefix("updated=") {
            row.created_at = value.to_owned();
        }
    }
    Some(row)
}

fn parse_revision_body(body: &str) -> (Vec<MemoryRevisionRow>, Option<u64>) {
    let mut revisions: Vec<MemoryRevisionRow> = Vec::new();
    let mut next = None;
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(raw) = trimmed.strip_prefix("next-revision: ") {
            next = raw.parse().ok();
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("grounds summary ") {
            if let Some(revision) = revisions.last_mut() {
                let (summary, grounds) = rest.split_once(": ").unwrap_or((rest, ""));
                revision.grounds_summary = Some(summary.to_owned());
                revision.grounds = Some(grounds.to_owned());
            }
            continue;
        }
        if trimmed == "grounds: unavailable"
            && let Some(revision) = revisions.last_mut()
        {
            revision.grounds = Some(String::from("unavailable"));
            continue;
        }
        if let Some(revision) = parse_revision_line(trimmed) {
            revisions.push(revision);
        }
    }
    (revisions, next)
}

fn parse_revision_line(line: &str) -> Option<MemoryRevisionRow> {
    let rest = line.strip_prefix("rev")?;
    let (number, rest) = rest.split_once(' ')?;
    let revision = number.parse().ok()?;
    let (change, rest) = rest.split_once(" at=")?;
    let (at, content) = rest.split_once(" content: ")?;
    Some(MemoryRevisionRow {
        revision,
        change: change.to_owned(),
        at: at.to_owned(),
        content: content.to_owned(),
        grounds_summary: None,
        grounds: None,
    })
}
