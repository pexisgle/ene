//! GGUF download, cache, and path resolution (#171).

mod download;

use crate::config::{AiConfig, LOCAL_PROVIDER};
use crate::error::LlmProviderError;
use crate::resolve::ResolvedLocalModel;
use download::download_gguf;
use std::collections::BTreeSet;
use std::path::PathBuf;
use tokio::task::JoinSet;

pub use download::{filename_from_url, gguf_cache_dir};

/// Ensure a local model's GGUF weights are present on disk (may download).
pub async fn ensure_gguf_available(local: &ResolvedLocalModel) -> Result<PathBuf, LlmProviderError> {
    let path = resolve_target_path(local)?;
    if path.is_file() {
        return Ok(path);
    }
    if !local.model_path.trim().is_empty() {
        return Err(LlmProviderError::Provider(format!(
            "local model_path does not exist: {}",
            local.model_path
        )));
    }
    download_gguf(&local.url, &path).await?;
    Ok(path)
}

/// Synchronous path resolution (expects weights already cached or `model_path` set).
pub fn resolve_local_gguf_path(local: &ResolvedLocalModel) -> Result<PathBuf, LlmProviderError> {
    let path = resolve_target_path(local)?;
    if path.is_file() {
        return Ok(path);
    }
    Err(LlmProviderError::Provider(format!(
        "GGUF not found at {}; run prefetch or set model_path",
        path.display()
    )))
}

fn resolve_target_path(local: &ResolvedLocalModel) -> Result<PathBuf, LlmProviderError> {
    if !local.model_path.trim().is_empty() {
        let path = PathBuf::from(local.model_path.trim());
        return Ok(path);
    }
    if local.url.trim().is_empty() {
        return Err(LlmProviderError::Provider(format!(
            "local model {:?} has no url or model_path",
            local.name
        )));
    }
    let filename = filename_from_url(&local.url)?;
    Ok(gguf_cache_dir().join(filename))
}

fn collect_prefetch_targets(
    config: &AiConfig,
    prefetch_embedding: bool,
    prefetch_decision: bool,
) -> Vec<ResolvedLocalModel> {
    let mut keys = BTreeSet::new();
    let mut out = Vec::new();

    let mut push_task = |task: &crate::config::TaskRef| {
        if task.provider != LOCAL_PROVIDER {
            return;
        }
        let Some(model) = task.model.as_deref().filter(|m| !m.trim().is_empty()) else {
            return;
        };
        if !keys.insert(model.to_string()) {
            return;
        }
        if let Ok(local) = config.resolve_local_model_for_task(task) {
            out.push(local);
        }
    };

    if prefetch_embedding {
        push_task(&config.tasks.embedding);
    }
    if prefetch_decision
        && let Some(proactive) = config.tasks.proactive.as_ref()
    {
        push_task(proactive);
    }
    out
}

/// Prefetch all configured GGUF weights in parallel.
pub async fn prefetch_configured_gguf(
    config: &AiConfig,
    prefetch_embedding: bool,
    prefetch_decision: bool,
) -> Result<(), LlmProviderError> {
    let targets = collect_prefetch_targets(config, prefetch_embedding, prefetch_decision);
    if targets.is_empty() {
        return Ok(());
    }

    let mut set = JoinSet::new();
    for local in targets {
        set.spawn(async move { ensure_gguf_available(&local).await.map(|_| ()) });
    }

    while let Some(result) = set.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(e) => {
                return Err(LlmProviderError::Provider(format!(
                    "GGUF prefetch task join error: {e}"
                )));
            }
        }
    }
    Ok(())
}

/// Prefetch the embedding task's GGUF (if `provider: "local"`).
pub async fn prefetch_embedding_gguf(config: &AiConfig) -> Result<(), LlmProviderError> {
    prefetch_configured_gguf(config, true, false).await
}

/// Prefetch the proactive decision task's GGUF (if `provider: "local"`).
pub async fn prefetch_decision_gguf(config: &AiConfig) -> Result<(), LlmProviderError> {
    prefetch_configured_gguf(config, false, true).await
}

/// Legacy alias: resolve embedding GGUF path from URL (sync, no download).
pub fn resolve_embedding_gguf_path(url: &str) -> Result<PathBuf, LlmProviderError> {
    let filename = filename_from_url(url)?;
    let path = gguf_cache_dir().join(filename);
    if path.is_file() {
        return Ok(path);
    }
    Err(LlmProviderError::Provider(format!(
        "embedding GGUF not cached at {}; prefetch first",
        path.display()
    )))
}

/// Legacy alias: resolve decision GGUF path (async, may download).
pub async fn resolve_decision_gguf_path(
    model_path: &str,
    decision_model_url: &str,
) -> Result<Option<PathBuf>, LlmProviderError> {
    if !model_path.trim().is_empty() {
        let path = PathBuf::from(model_path.trim());
        return Ok(if path.is_file() { Some(path) } else { None });
    }
    if decision_model_url.trim().is_empty() {
        return Ok(None);
    }
    let local = ResolvedLocalModel {
        name: String::new(),
        url: decision_model_url.to_string(),
        model_path: String::new(),
        quantization: String::new(),
        acceleration: crate::config::ProactiveAcceleration::Auto,
        gpu_layers: "auto".to_string(),
        context_size: 2048,
    };
    Ok(Some(ensure_gguf_available(&local).await?))
}
