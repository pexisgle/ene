//! Single resolution point from an [`AiTaskKind`] to a configured provider row.

use crate::{CoreDaemon, plugin_profile::task_row_id};
use ene_kernel::{AiTaskKind, TaskBinding, TextDeltaSink};
use ene_plugin_ipc::{
    EmbedRequest, LlmGenerateRequest, LlmGeneration, ProviderAuth, TtsAudio, TtsRequest,
};

/// One resolved provider call target.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedTask {
    pub binding: TaskBinding,
    pub row_id: String,
    pub api_key: String,
}

impl ResolvedTask {
    fn auth(&self) -> ProviderAuth {
        ProviderAuth {
            api_key: self.api_key.clone(),
        }
    }
}

/// Resolve a lane to its effective binding, row id and API key.
pub(crate) fn resolve_task(core: &CoreDaemon, kind: AiTaskKind) -> Option<ResolvedTask> {
    let (binding, origin) = core.ai().lock().tasks.effective_binding(kind);
    if binding.is_unconfigured() {
        return None;
    }
    Some(ResolvedTask {
        binding,
        row_id: task_row_id(origin.name()),
        api_key: core.secret_for_kind(kind),
    })
}

/// Generate on the resolved lane via the retrying LLM path.
pub(crate) async fn generate_llm(
    core: &CoreDaemon,
    task: &ResolvedTask,
    mut request: LlmGenerateRequest,
) -> Result<LlmGeneration, String> {
    request.model = task.binding.model.clone();
    request.base_url = task.binding.base_url.clone();
    request.auth = task.auth();
    super::http::llm::generate_llm(core, &task.row_id, request).await
}

/// Streaming variant of [`generate_llm`].
pub(crate) async fn generate_llm_streaming(
    core: &CoreDaemon,
    task: &ResolvedTask,
    mut request: LlmGenerateRequest,
    sink: &mut dyn TextDeltaSink,
) -> Result<LlmGeneration, String> {
    request.model = task.binding.model.clone();
    request.base_url = task.binding.base_url.clone();
    request.auth = task.auth();
    super::http::llm::generate_llm_streaming(core, &task.row_id, request, sink).await
}
pub(crate) async fn embed_texts(
    core: &CoreDaemon,
    texts: Vec<String>,
) -> Result<Vec<Vec<f32>>, String> {
    let task = resolve_task(core, AiTaskKind::Embedding)
        .ok_or_else(|| "embedding is not configured".to_owned())?;
    let result = core
        .supervisor()
        .embed(
            &task.row_id,
            EmbedRequest {
                texts,
                model: task.binding.model.clone(),
                base_url: task.binding.base_url.clone(),
                auth: task.auth(),
            },
        )
        .await
        .map_err(|err| err.to_string())?;
    Ok(result.vectors)
}
/// `None` when the TTS lane is unconfigured or the provider call fails.
pub(crate) async fn synthesize_tts(core: &CoreDaemon, text: String) -> Option<TtsAudio> {
    let task = resolve_task(core, AiTaskKind::Tts)?;
    let request = TtsRequest {
        text,
        voice: task.binding.voice.clone(),
        model: task.binding.model.clone(),
        base_url: task.binding.base_url.clone(),
        auth: task.auth(),
    };
    match core
        .supervisor()
        .synthesize_tts(&task.row_id, request)
        .await
    {
        Ok(audio) => Some(audio),
        Err(err) => {
            tracing::warn!(error = %err, "tts provider failed");
            None
        }
    }
}
