use crate::error::WorkError;
use crate::host::DelegationHost;
use crate::skill::match_skills;
use crate::types::{Artifact, ArtifactKind, CompanionReport, JobStatus};
use chrono::Utc;
use ene_session::{DelegationId, SoulId};
use ene_tool_registry::{Layer, ToolRegistry};
use serde_json::{Value, json};
use std::fmt::Write;
use std::path::Path;
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
    host.require_mutating_allowed(job_id)?;
    let job = host.status_snapshot(job_id)?;
    let safe_title = crate::host::sanitize_filename(title);
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
    let report = host.complete(job_id, &format!("bookmark ready: {title}"))?;
    let delivered = host.store().get_artifact(&artifact.id)?.unwrap_or(artifact);
    Ok((delivered, report))
}

/// Inputs for researching a theme and delivering a bookmark on an existing job.
pub struct BookmarkFill<'a> {
    pub host: &'a DelegationHost,
    pub soul_id: SoulId,
    pub job_id: DelegationId,
    pub theme: &'a str,
    pub skills_home: &'a Path,
    pub enabled: &'a [String],
    pub registry: Option<&'a ToolRegistry>,
}

/// Search (when `web.search` is registered), apply matching skills, write Markdown, complete.
pub async fn fill_bookmark_job(
    fill: BookmarkFill<'_>,
) -> Result<(Artifact, CompanionReport), WorkError> {
    fill.host.require_mutating_allowed(fill.job_id)?;
    fill.host
        .store()
        .set_status(fill.job_id, JobStatus::Running, None)?;
    let skills = match_skills(fill.skills_home, fill.enabled, fill.theme)?;
    let mut sections = research_sections(fill.registry, fill.theme).await;
    if sections.is_empty() {
        sections.push(BookmarkSection {
            heading: "Findings".to_owned(),
            body: format!("No web hits for {}.", fill.theme),
        });
    }
    if !skills.is_empty() {
        let mut body = String::new();
        for meta in &skills {
            write!(body, "### {}\n\n{}\n", meta.name, meta.body).ok();
        }
        sections.push(BookmarkSection {
            heading: "Skill notes".to_owned(),
            body,
        });
    }
    let title = bookmark_title(fill.theme, &skills);
    deliver_bookmark_workflow(fill.host, fill.soul_id, fill.job_id, &title, &sections)
}

async fn research_sections(registry: Option<&ToolRegistry>, theme: &str) -> Vec<BookmarkSection> {
    let Some(registry) = registry else {
        return Vec::new();
    };
    match registry
        .execute("web.search", json!({ "query": theme }), Layer::Job)
        .await
    {
        Ok(value) => sections_from_search(&value),
        Err(ene_tool_registry::PipelineError::Unknown(_)) => Vec::new(),
        Err(err) => vec![BookmarkSection {
            heading: "Findings".to_owned(),
            body: format!("web.search unavailable: {err}"),
        }],
    }
}

fn sections_from_search(value: &Value) -> Vec<BookmarkSection> {
    let mut findings = Vec::new();
    let mut sources = Vec::new();
    if let Some(rows) = value.get("results").and_then(Value::as_array) {
        for row in rows {
            let title = row.get("title").and_then(Value::as_str).unwrap_or("");
            let snippet = row.get("snippet").and_then(Value::as_str).unwrap_or("");
            let url = row.get("url").and_then(Value::as_str).unwrap_or("");
            if !title.is_empty() || !snippet.is_empty() {
                findings.push(format!("- {title}: {snippet}"));
            }
            if !url.is_empty() {
                sources.push(format!("- {url}"));
            }
        }
    }
    let mut sections = Vec::new();
    if !findings.is_empty() {
        sections.push(BookmarkSection {
            heading: "Findings".to_owned(),
            body: findings.join("\n"),
        });
    }
    if !sources.is_empty() {
        sections.push(BookmarkSection {
            heading: "Sources".to_owned(),
            body: sources.join("\n"),
        });
    }
    sections
}

fn bookmark_title(theme: &str, skills: &[crate::skill::SkillMeta]) -> String {
    let trimmed = theme.trim();
    if trimmed.is_empty() {
        return skills
            .first()
            .map_or_else(|| "Bookmark".to_owned(), |meta| meta.name.clone());
    }
    if trimmed.chars().count() <= 60 {
        return trimmed.to_owned();
    }
    let mut out: String = trimmed.chars().take(57).collect();
    out.push_str("...");
    out
}
