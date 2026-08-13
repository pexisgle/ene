//! Per-profile lazy model registry for the llama-server sidecar.
//!
//! Models live in the sidecar process; this module owns the mapping from
//! profile keys to on-disk GGUF weights, the embedding-dimension cache, and
//! the sidecar lifecycle. Downloads still happen lazily per profile on first
//! use (same behavior as the in-process plugin), so startup never fetches
//! weights that are not configured.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, PoisonError};

use ene_ai::resolve::ResolvedLocalModel;
use ene_plugin::PluginError;

use crate::client::LlamaServerClient;
use crate::config::{HostConfig, Profile, all_profiles, current_config, profile_for};
use crate::convert;
use crate::gguf::{ensure_gguf_available, ensure_mmproj_available};
use crate::server::ensure_sidecar;

/// Measured embedding dimensionality per profile key, filled by a probe on
/// first embed and cleared when configuration changes or the model unloads.
static EMBED_DIMS: LazyLock<Mutex<HashMap<String, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Invalidates cached dimensions and restarts the sidecar after a live host
/// configuration update. Synchronous because it is called from
/// `ConfigurablePlugin` handlers that cannot await.
pub(crate) fn config_changed() {
    EMBED_DIMS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
    crate::server::reset_sidecar();
}

/// Returns a chat client for `model`, downloading its weights (and the
/// optional mmproj) and ensuring the sidecar is up before the request.
pub(crate) async fn chat_provider(model: &str) -> Result<LlamaServerClient, PluginError> {
    let config = current_config()?;
    let profile = profile_for(model)?;
    let _weights = resolve_weights(model, &profile, &config).await?;
    let mmproj = resolve_mmproj(&config).await?;
    if let Some(path) = mmproj {
        tracing::info!(
            component = "LlamaServerPlugin",
            path = %path.display(),
            "mmproj ready for vision requests"
        );
    }
    let state = ensure_sidecar(&config, &all_profiles()).await?;
    let client = LlamaServerClient::new(&state)?;
    Ok(client)
}

/// Returns an embed client plus the model's measured dimensionality.
///
/// The dimension comes from a probe embedding (the sidecar has no
/// pre-load `n_embd` query), cached per profile so repeated batches do not
/// pay for it.
pub(crate) async fn embed_provider(model: &str) -> Result<(LlamaServerClient, usize), PluginError> {
    let config = current_config()?;
    let profile = profile_for(model)?;
    let _weights = resolve_weights(model, &profile, &config).await?;
    let state = ensure_sidecar(&config, &all_profiles()).await?;
    let client = LlamaServerClient::new(&state)?;
    let dims = embed_dims(&client, model).await?;
    Ok((client, dims))
}

/// Removes a profile's cached dimension and asks the sidecar to unload the
/// model (best-effort: an already-unloaded model is not an error).
pub(crate) async fn unload(model: &str) {
    EMBED_DIMS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(model);
    let Some(state) = crate::server::current_sidecar() else {
        return;
    };
    let Ok(client) = LlamaServerClient::new(&state) else {
        return;
    };
    if let Err(e) = client.unload(model).await {
        tracing::warn!(
            component = "LlamaServerPlugin",
            model,
            error = %e,
            "sidecar unload failed; model may stay resident"
        );
    }
}

/// Cached probe-based dimensionality for `model`.
async fn embed_dims(client: &LlamaServerClient, model: &str) -> Result<usize, PluginError> {
    if let Some(dims) = EMBED_DIMS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(model)
        .copied()
    {
        return Ok(dims);
    }
    let probe = vec!["ene".to_string()];
    let vectors = client.embed_batch(model, &probe).await?;
    let dims = vectors.first().map_or(0, Vec::len);
    if dims == 0 {
        return Err(PluginError::provider(format!(
            "llama-server returned an empty embedding for model {model:?}"
        )));
    }
    EMBED_DIMS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(model.to_string(), dims);
    Ok(dims)
}

/// Resolves the GGUF path for a profile: validates `model_path` when set,
/// otherwise downloads `url` (magic validation + blake3 cache naming live in
/// [`crate::gguf`]).
async fn resolve_weights(
    model: &str,
    profile: &Profile,
    config: &HostConfig,
) -> Result<PathBuf, PluginError> {
    let local = ResolvedLocalModel {
        name: model.to_string(),
        url: profile.url().unwrap_or_default().to_string(),
        artifact_id: profile.artifact_id().unwrap_or_default().to_string(),
        artifact_version: profile.artifact_version().unwrap_or_default().to_string(),
        model_path: profile.model_path().unwrap_or_default().to_string(),
        mmproj_url: config.mmproj_url.clone().unwrap_or_default(),
        mmproj_path: config.mmproj_path.clone().unwrap_or_default(),
        quantization: profile.quantization().to_string(),
        acceleration: config.acceleration()?,
        gpu_layers: profile.gpu_layers(),
        context_size: profile.context_size(),
        dimensions: profile.dimensions(),
    };
    ensure_gguf_available(&local)
        .await
        .map_err(|e| convert::map_llm_error(&e))
}

/// Resolves the optional mmproj path from the host config (may download).
async fn resolve_mmproj(config: &HostConfig) -> Result<Option<PathBuf>, PluginError> {
    if !config_has_mmproj(config) {
        return Ok(None);
    }
    let local = ResolvedLocalModel {
        name: String::new(),
        url: String::new(),
        artifact_id: String::new(),
        artifact_version: String::new(),
        model_path: String::new(),
        mmproj_url: config.mmproj_url.clone().unwrap_or_default(),
        mmproj_path: config.mmproj_path.clone().unwrap_or_default(),
        quantization: String::new(),
        acceleration: config.acceleration()?,
        gpu_layers: ene_ai::GpuLayers::Auto,
        context_size: 0,
        dimensions: None,
    };
    ensure_mmproj_available(&local)
        .await
        .map_err(|e| convert::map_llm_error(&e))
}

fn config_has_mmproj(config: &HostConfig) -> bool {
    !config
        .mmproj_url
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
        || !config
            .mmproj_path
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect for concise assertions"
)]
mod tests {
    use super::*;
    use serde::Deserialize as _;

    #[test]
    fn mmproj_detection_matches_config() {
        let none = HostConfig::default();
        assert!(!config_has_mmproj(&none));
        let url = HostConfig::deserialize(serde_json::json!({
            "mmproj_url": "https://cdn.example/mmproj.gguf"
        }))
        .expect("config");
        assert!(config_has_mmproj(&url));
        let path = HostConfig::deserialize(serde_json::json!({
            "mmproj_path": "/data/mmproj.gguf"
        }))
        .expect("config");
        assert!(config_has_mmproj(&path));
    }

    #[tokio::test]
    async fn dims_cache_round_trips() {
        EMBED_DIMS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert("probe".to_string(), 384);
        let dims = EMBED_DIMS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get("probe")
            .copied();
        assert_eq!(dims, Some(384));
        config_changed();
        assert!(
            EMBED_DIMS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty()
        );
    }
}
