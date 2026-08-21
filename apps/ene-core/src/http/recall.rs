use std::sync::{Arc, Weak};

use async_trait::async_trait;
use ene_companion::MemoryKind;
use ene_kernel::{SessionId, SoulId, TurnPrefetch};
use ene_plugin_ipc::{EmbedRequest, ProviderAuth};
use ene_session::{ProjectOptions, Role, derive_messages};
use ene_work::{JobStatus, catalog};

use crate::CoreDaemon;

/// Loads per-turn Context Sources into the kernel registry.
pub struct RecallPrefetch {
    core: Weak<CoreDaemon>,
}

impl RecallPrefetch {
    #[must_use]
    pub fn new(core: &Arc<CoreDaemon>) -> Self {
        Self {
            core: Arc::downgrade(core),
        }
    }
}

#[async_trait]
impl TurnPrefetch for RecallPrefetch {
    async fn lines(
        &self,
        soul: SoulId,
        session: SessionId,
        user_text: &str,
    ) -> Vec<(String, String)> {
        let Some(core) = self.core.upgrade() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Some(persona) = persona_line(&core, soul) {
            out.push(persona);
        }
        if let Some(line) = character_state_line(&core, soul) {
            out.push(line);
        }
        out.extend(profile_line(&core, soul));
        out.extend(commitments_line(&core, soul));
        out.extend(skills_line(&core, soul));
        out.extend(mcp_context_lines(&core.workspace_dir()));
        out.extend(inner_recent_line(&core, session));
        out.extend(delegation_line(&core, soul));
        if !user_text.trim().is_empty() {
            out.extend(recall_lines(&core, soul, user_text).await);
        }
        out
    }
}

fn persona_line(core: &CoreDaemon, soul: SoulId) -> Option<(String, String)> {
    let store = core.companions();
    let row = store.get_soul(soul).ok().flatten()?;
    let (id, version) = row.character_ref.split_once('@')?;
    let path = store.package_path(id, version).ok().flatten()?;
    let text = persona_from_package(std::path::Path::new(&path))?;
    Some(("identity_kernel".to_owned(), text))
}

fn persona_from_package(package_dir: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(package_dir.join("soul/persona.md")).ok()?;
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_owned())
    }
}

fn character_state_line(core: &CoreDaemon, soul: SoulId) -> Option<(String, String)> {
    let row = core.companion().soul(soul).ok()?;
    Some((
        "character_state".to_owned(),
        format!("Affect: {}", row.affect.summary_words()),
    ))
}

fn profile_line(core: &CoreDaemon, soul: SoulId) -> Option<(String, String)> {
    let notes = core.companions().standing_notes(soul, 8).ok()?;
    source_block("memory.user_profile", "User profile", &notes)
}

fn commitments_line(core: &CoreDaemon, soul: SoulId) -> Option<(String, String)> {
    let notes = core.companions().open_commitments(soul, 8).ok()?;
    source_block("memory.commitments", "Open commitments", &notes)
}

fn skills_line(core: &CoreDaemon, soul: SoulId) -> Option<(String, String)> {
    let enabled = core
        .companion()
        .soul(soul)
        .ok()
        .map(|row| row.skill_refs)
        .unwrap_or_default();
    let home = core.data_dir().join("skills");
    let entries = catalog(&home, &enabled).unwrap_or_default();
    if entries.is_empty() {
        return None;
    }
    let body = entries
        .iter()
        .map(|(name, description)| format!("- {name}: {description}"))
        .collect::<Vec<_>>()
        .join("\n");
    Some((
        "skills.active".to_owned(),
        format!("Available skills:\n{body}"),
    ))
}

fn inner_recent_line(core: &CoreDaemon, session: SessionId) -> Option<(String, String)> {
    let events = core.store().load_events(session, 0).ok()?;
    let history = derive_messages(&events, ProjectOptions::model_visible(8));
    let thoughts: Vec<String> = history
        .messages
        .iter()
        .filter(|message| message.role == Role::Inner)
        .map(ene_session::ProjectedMessage::text)
        .filter(|text| !text.trim().is_empty())
        .collect();
    source_block("inner_recent", "Recent inner thoughts", &thoughts)
}

fn delegation_line(core: &CoreDaemon, soul: SoulId) -> Option<(String, String)> {
    let jobs = core.work().list_jobs(soul).ok()?;
    let active: Vec<String> = jobs
        .iter()
        .filter(|job| {
            matches!(
                job.status,
                JobStatus::Created | JobStatus::Queued | JobStatus::Running
            )
        })
        .map(|job| format!("- {} [{}] {}", job.title, job.status.as_str(), job.goal))
        .collect();
    source_block("delegation.active", "Active delegations", &active)
}

fn mcp_context_lines(workspace: &std::path::Path) -> Vec<(String, String)> {
    let dir = workspace.join("mcp-context");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut chunks = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "md")
            && let Ok(text) = std::fs::read_to_string(&path)
            && !text.trim().is_empty()
        {
            chunks.push(text);
        }
    }
    source_block("mcp.resources", "MCP resources", &chunks)
        .into_iter()
        .collect()
}

async fn recall_lines(core: &CoreDaemon, soul: SoulId, user_text: &str) -> Vec<(String, String)> {
    let query_vec = embed_query(core, user_text).await;
    let hits = match core
        .companion()
        .recall_for_turn(soul, user_text, query_vec.as_deref())
    {
        Ok(hits) => hits,
        Err(err) => {
            tracing::debug!(error = %err, "recall skipped");
            return Vec::new();
        }
    };
    let semantic: Vec<String> = hits
        .iter()
        .filter(|hit| {
            !matches!(
                hit.kind,
                MemoryKind::UserProfile | MemoryKind::Preference | MemoryKind::Commitment
            )
        })
        .map(|hit| format!("- {}: {}", hit.title, hit.content))
        .collect();
    source_block("memory.semantic", "Recalled memories", &semantic)
        .into_iter()
        .collect()
}

fn source_block(key: &str, heading: &str, lines: &[String]) -> Option<(String, String)> {
    if lines.is_empty() {
        return None;
    }
    Some((key.to_owned(), format!("{heading}:\n{}", lines.join("\n"))))
}

async fn embed_query(core: &CoreDaemon, text: &str) -> Option<Vec<f32>> {
    let (binding, row_id) = {
        let guard = core.ai();
        let ai = guard.lock();
        if ai.tasks.embedding.is_unconfigured() {
            (
                ai.tasks.chat.clone(),
                crate::plugin_profile::task_row_id("chat"),
            )
        } else {
            (
                ai.tasks.embedding.clone(),
                crate::plugin_profile::task_row_id("embedding"),
            )
        }
    };
    if binding.is_unconfigured() {
        return None;
    }
    let result = core
        .supervisor()
        .embed(
            &row_id,
            EmbedRequest {
                texts: vec![text.to_owned()],
                model: binding.model,
                base_url: binding.base_url,
                auth: ProviderAuth {
                    api_key: core.secret_for("embedding"),
                },
            },
        )
        .await
        .ok()?;
    result.vectors.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::{persona_from_package, source_block};

    #[test]
    fn persona_from_package_reads_alicia_prompt() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("soul")).unwrap();
        std::fs::write(
            dir.path().join("soul/persona.md"),
            "You are Alicia, a desktop companion.\n",
        )
        .unwrap();
        let text = persona_from_package(dir.path()).expect("persona");
        assert!(text.contains("Alicia"));
        assert!(!text.contains("Ene"));
    }

    #[test]
    fn source_block_skips_empty_and_joins_lines() {
        assert!(source_block("memory.semantic", "Recalled memories", &[]).is_none());
        let (key, text) = source_block(
            "memory.semantic",
            "Recalled memories",
            &["- picnic: planned".to_owned()],
        )
        .expect("block");
        assert_eq!(key, "memory.semantic");
        assert!(text.contains("picnic"));
    }
}
