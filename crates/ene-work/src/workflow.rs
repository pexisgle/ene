use crate::error::WorkError;
use crate::host::DelegationHost;
use crate::types::{Artifact, ArtifactKind, CompanionReport};
use chrono::Utc;
use ene_session::{DelegationId, SoulId};
use std::fmt::Write;
use uuid::Uuid;

/// Section in a bookmark-style Markdown report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmarkSection {
    pub heading: String,
    pub body: String,
}

/// Write a bookmark Markdown file, register it, deliver, and complete the task.
pub fn deliver_bookmark_workflow(
    host: &DelegationHost,
    soul_id: SoulId,
    job_id: DelegationId,
    title: &str,
    sections: &[BookmarkSection],
) -> Result<(Artifact, CompanionReport), WorkError> {
    let job = host.status_snapshot(job_id)?;
    let safe_title = sanitize_filename(title);
    let path = std::path::Path::new(&job.workspace_dir).join(format!("{safe_title}.md"));
    let mut markdown = format!("# {title}\n\n");
    for section in sections {
        write!(markdown, "## {}\n\n{}\n\n", section.heading, section.body).ok();
    }
    std::fs::write(&path, markdown)?;
    let artifact = host.store().register_artifact(Artifact {
        id: Uuid::now_v7().to_string(),
        soul_id,
        job_id: Some(job_id),
        kind: ArtifactKind::Markdown,
        title: title.to_owned(),
        path: path.to_string_lossy().into_owned(),
        mime: Some("text/markdown".to_owned()),
        size_bytes: std::fs::metadata(&path)
            .ok()
            .and_then(|meta| i64::try_from(meta.len()).ok()),
        created_at: Utc::now().to_rfc3339(),
        delivered: false,
    })?;
    host.store().deliver(&artifact.id)?;
    let delivered = host.store().get_artifact(&artifact.id)?.unwrap_or(artifact);
    let report = host.complete(job_id, &format!("bookmark ready: {title}"))?;
    Ok((delivered, report))
}

fn sanitize_filename(title: &str) -> String {
    let mut out = String::new();
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if (ch.is_whitespace() || ch == '-' || ch == '_') && !out.ends_with('_') {
            out.push('_');
        }
    }
    if out.is_empty() {
        "bookmark".to_owned()
    } else {
        out
    }
}
