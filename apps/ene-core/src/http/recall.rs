use std::sync::Arc;

use async_trait::async_trait;
use ene_kernel::{SessionId, SoulId, TaskBinding, TurnPrefetch};
use ene_plugin_ipc::{EmbedRequest, ProviderAuth};

use crate::CoreDaemon;

/// Logs recalled memories as `context/system_message` before generation.
pub struct RecallPrefetch {
    core: Arc<CoreDaemon>,
}

impl RecallPrefetch {
    #[must_use]
    pub fn new(core: Arc<CoreDaemon>) -> Self {
        Self { core }
    }

    fn embed_binding(&self) -> TaskBinding {
        let guard = self.core.ai();
        let ai = guard.lock();
        if ai.tasks.embedding.uses_echo() {
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
        if user_text.trim().is_empty() {
            return Vec::new();
        }
        let query_vec = embed_query(self, user_text).await;
        let hits = match self
            .core
            .companion()
            .recall_ranked(soul, user_text, query_vec.as_deref())
        {
            Ok(hits) => hits,
            Err(err) => {
                tracing::debug!(error = %err, "recall skipped");
                return Vec::new();
            }
        };
        if hits.is_empty() {
            return Vec::new();
        }
        let body = hits
            .iter()
            .map(|hit| format!("- {}: {}", hit.title, hit.content))
            .collect::<Vec<_>>()
            .join("\n");
        vec![(
            "companion.recall".to_owned(),
            format!("Recalled memories:\n{body}"),
        )]
    }
}

async fn embed_query(prefetch: &RecallPrefetch, text: &str) -> Option<Vec<f32>> {
    let binding = prefetch.embed_binding();
    if binding.uses_echo() {
        return None;
    }
    let result = prefetch
        .core
        .supervisor()
        .embed(
            &binding.plugin,
            EmbedRequest {
                texts: vec![text.to_owned()],
                model: binding.model,
                base_url: binding.base_url,
                auth: ProviderAuth {
                    api_key: prefetch.core.secret_for("embedding"),
                },
            },
        )
        .await
        .ok()?;
    result.vectors.into_iter().next()
}
