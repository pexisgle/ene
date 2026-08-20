use std::sync::{Arc, Weak};

use async_trait::async_trait;
use ene_kernel::{SessionId, SoulId, TaskBinding, TurnPrefetch};
use ene_plugin_ipc::{EmbedRequest, ProviderAuth};

use crate::CoreDaemon;

/// Logs recalled memories as `context/system_message` before generation.
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

    fn embed_binding(core: &CoreDaemon) -> TaskBinding {
        let guard = core.ai();
        let ai = guard.lock();
        if ai.tasks.embedding.is_unconfigured() {
            ai.tasks.chat.clone()
        } else {
            ai.tasks.embedding.clone()
        }
    }
}

#[async_trait]
impl TurnPrefetch for RecallPrefetch {
    async fn lines(
        &self,
        soul: SoulId,
        _session: SessionId,
        user_text: &str,
    ) -> Vec<(String, String)> {
        let Some(core) = self.core.upgrade() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Some(persona) = persona_line(&core, soul) {
            out.push(persona);
        }
        out.extend(mcp_context_lines(&core.workspace_dir()));
        if user_text.trim().is_empty() {
            return out;
        }
        let query_vec = embed_query(&core, user_text).await;
        let hits = match core
            .companion()
            .recall_ranked(soul, user_text, query_vec.as_deref())
        {
            Ok(hits) => hits,
            Err(err) => {
                tracing::debug!(error = %err, "recall skipped");
                return out;
            }
        };
        if hits.is_empty() {
            return out;
        }
        let body = hits
            .iter()
            .map(|hit| format!("- {}: {}", hit.title, hit.content))
            .collect::<Vec<_>>()
            .join("\n");
        out.push((
            "companion.recall".to_owned(),
            format!("Recalled memories:\n{body}"),
        ));
        out
    }
}

fn persona_line(core: &CoreDaemon, soul: SoulId) -> Option<(String, String)> {
    let store = core.companions();
    let row = store.get_soul(soul).ok().flatten()?;
    let (id, version) = row.character_ref.split_once('@')?;
    let path = store.package_path(id, version).ok().flatten()?;
    let text = persona_from_package(std::path::Path::new(&path))?;
    Some(("companion.persona".to_owned(), text))
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
    if chunks.is_empty() {
        return Vec::new();
    }
    vec![(
        "mcp.resources".to_owned(),
        format!("MCP resources:\n{}", chunks.join("\n\n")),
    )]
}

async fn embed_query(core: &CoreDaemon, text: &str) -> Option<Vec<f32>> {
    let binding = RecallPrefetch::embed_binding(core);
    if binding.is_unconfigured() {
        return None;
    }
    let result = core
        .supervisor()
        .embed(
            &binding.plugin,
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
    use super::persona_from_package;

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
}
