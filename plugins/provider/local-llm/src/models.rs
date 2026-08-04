//! Per-profile lazy model registry.
//!
//! Each profile key loads at most one chat model and one embedding model,
//! kept for the process lifetime (VRAM residency). The initial llama.cpp
//! build is a synchronous FFI-heavy call (`EngineHandle::try_spawn` builds
//! the first model on the calling thread), so loads run in `spawn_blocking`
//! and never block the plugin's async runtime.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use ene_ai::resolve::ResolvedLocalModel;
use ene_ai_local::gguf::{ensure_gguf_available, ensure_mmproj_available};
use ene_ai_local::{
    EneEmbeddingError, GgufEmbeddingProvider, LocalGgufLoadParams, LocalLlamaCppProvider,
};
use ene_plugin::PluginError;

use crate::config::{HostConfig, Profile, current_config, profile_for};
use crate::convert;

/// Chat request timeout in seconds, mapped onto `EngineConfig::job_timeout`.
/// Generous because a local chat generation can legitimately run for minutes
/// on CPU; the in-process decision path uses a smaller budget because it
/// serves short classification turns.
const CHAT_REQUEST_TIMEOUT_SECS: u64 = 300;

static CHAT_MODELS: LazyLock<Mutex<HashMap<String, Arc<LocalLlamaCppProvider>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static EMBED_MODELS: LazyLock<Mutex<HashMap<String, Arc<GgufEmbeddingProvider>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Returns the chat model for `model`, loading it on first use.
pub(crate) async fn chat_provider(model: &str) -> Result<Arc<LocalLlamaCppProvider>, PluginError> {
    if let Some(provider) = CHAT_MODELS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(model)
    {
        return Ok(Arc::clone(provider));
    }

    let config = current_config()?;
    let profile = profile_for(model)?;
    let weights = resolve_weights(model, &profile, &config).await?;
    let mmproj = resolve_mmproj(&config).await?;
    let params = LocalGgufLoadParams {
        model_path: weights.to_string_lossy().into_owned(),
        mmproj_path: mmproj.map(|path| path.to_string_lossy().into_owned()),
        acceleration: config.acceleration()?,
        gpu_layers: profile.gpu_layers().to_string(),
        context_size: profile.context_size(),
        request_timeout_seconds: CHAT_REQUEST_TIMEOUT_SECS,
    };
    let loaded = load_blocking(
        move || LocalLlamaCppProvider::load(&params),
        |e| convert::map_llm_error(&e),
    )
    .await?;
    Ok(insert_if_absent(model, loaded))
}

/// Returns the embedding model for `model`, loading it on first use.
pub(crate) async fn embed_provider(model: &str) -> Result<Arc<GgufEmbeddingProvider>, PluginError> {
    if let Some(provider) = EMBED_MODELS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(model)
    {
        return Ok(Arc::clone(provider));
    }

    let config = current_config()?;
    let profile = profile_for(model)?;
    let weights = resolve_weights(model, &profile, &config).await?;
    let path = weights.to_string_lossy().into_owned();
    let name = model.to_string();
    let quantization = profile.quantization().to_string();
    let acceleration = config.acceleration()?;
    let loaded = load_blocking(
        move || {
            GgufEmbeddingProvider::load_with_acceleration(&name, &path, &quantization, acceleration)
        },
        |e| map_embed_load_error(&e),
    )
    .await?;
    Ok(insert_embed_if_absent(model, loaded))
}

/// Resolves the GGUF path for a profile: validates `model_path` when set,
/// otherwise downloads `url` (magic validation + blake3 cache naming live in
/// `ene-ai-local`'s gguf module).
async fn resolve_weights(
    model: &str,
    profile: &Profile,
    config: &HostConfig,
) -> Result<PathBuf, PluginError> {
    let local = ResolvedLocalModel {
        name: model.to_string(),
        url: profile.url().unwrap_or_default().to_string(),
        model_path: profile.model_path().unwrap_or_default().to_string(),
        mmproj_url: config.mmproj_url.clone().unwrap_or_default(),
        mmproj_path: config.mmproj_path.clone().unwrap_or_default(),
        quantization: profile.quantization().to_string(),
        acceleration: config.acceleration()?,
        gpu_layers: profile.gpu_layers().to_string(),
        context_size: profile.context_size(),
        dimensions: profile.dimensions(),
    };
    ensure_gguf_available(&local)
        .await
        .map_err(|e| convert::map_llm_error(&e))
}

/// Resolves the optional mmproj path from the host config (may download).
async fn resolve_mmproj(config: &HostConfig) -> Result<Option<PathBuf>, PluginError> {
    if config
        .mmproj_url
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
        && config
            .mmproj_path
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
    {
        return Ok(None);
    }
    let local = ResolvedLocalModel {
        name: String::new(),
        url: String::new(),
        model_path: String::new(),
        mmproj_url: config.mmproj_url.clone().unwrap_or_default(),
        mmproj_path: config.mmproj_path.clone().unwrap_or_default(),
        quantization: String::new(),
        acceleration: config.acceleration()?,
        gpu_layers: String::new(),
        context_size: 0,
        dimensions: None,
    };
    ensure_mmproj_available(&local)
        .await
        .map_err(|e| convert::map_llm_error(&e))
}

/// Runs a synchronous model build on the blocking pool, mapping the model's
/// own error type to [`PluginError`].
async fn load_blocking<T, E>(
    build: impl FnOnce() -> Result<T, E> + Send + 'static,
    map_error: impl FnOnce(E) -> PluginError,
) -> Result<T, PluginError>
where
    T: Send + 'static,
    E: Send + 'static,
{
    tokio::task::spawn_blocking(build)
        .await
        .map_err(|e| PluginError::provider(format!("model load task failed: {e}")))?
        .map_err(map_error)
}

/// Inserts `provider` under `model` unless another load won the race; the
/// loser's engine is dropped, so a model that already serves live streams is
/// never replaced.
fn insert_if_absent(model: &str, provider: LocalLlamaCppProvider) -> Arc<LocalLlamaCppProvider> {
    let mut guard = CHAT_MODELS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(existing) = guard.get(model) {
        return Arc::clone(existing);
    }
    let provider = Arc::new(provider);
    guard.insert(model.to_string(), Arc::clone(&provider));
    provider
}

fn insert_embed_if_absent(
    model: &str,
    provider: GgufEmbeddingProvider,
) -> Arc<GgufEmbeddingProvider> {
    let mut guard = EMBED_MODELS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(existing) = guard.get(model) {
        return Arc::clone(existing);
    }
    let provider = Arc::new(provider);
    guard.insert(model.to_string(), Arc::clone(&provider));
    provider
}

/// Maps embedding load errors without panicking.
fn map_embed_load_error(err: &EneEmbeddingError) -> PluginError {
    PluginError::provider(err.to_string())
}
