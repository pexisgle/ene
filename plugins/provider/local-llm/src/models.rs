//! Per-profile lazy model registry.
//!
//! Each profile key loads at most one chat model and one embedding model,
//! kept for the process lifetime (VRAM residency). The initial llama.cpp
//! build is a synchronous FFI-heavy call (`EngineHandle::try_spawn` builds
//! the first model on the calling thread), so loads run in `spawn_blocking`
//! and never block the plugin's async runtime.

use std::collections::HashMap;
use std::future::Future;
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
/// Per-model load gates: one mutex per profile key.
type LoadGates = LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>;
/// One gate per profile key: the task loading a model holds it for the whole
/// load, so concurrent callers wait instead of starting a second llama.cpp
/// load of the same weights (double RAM/VRAM until one finishes).
static CHAT_LOAD_GATES: LoadGates = LazyLock::new(|| Mutex::new(HashMap::new()));
static EMBED_LOAD_GATES: LoadGates = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Returns the chat model for `model`, loading it on first use.
pub(crate) async fn chat_provider(model: &str) -> Result<Arc<LocalLlamaCppProvider>, PluginError> {
    let model_key = model.to_string();
    load_once(model, &CHAT_LOAD_GATES, cached_chat_model, move || {
        load_chat_model(model_key)
    })
    .await
}

/// Returns the embedding model for `model`, loading it on first use.
pub(crate) async fn embed_provider(model: &str) -> Result<Arc<GgufEmbeddingProvider>, PluginError> {
    let model_key = model.to_string();
    load_once(model, &EMBED_LOAD_GATES, cached_embed_model, move || {
        load_embed_model(model_key)
    })
    .await
}

/// Runs `load` at most once concurrently per model key.
///
/// The caller that wins the gate spawns the load in its own task and hands
/// the gate to it, so the gate is released only when the load completes —
/// even if the request handler that started it is aborted (e.g. a host that
/// timed out). Concurrent callers wait on the gate and then reuse the cached
/// result; a failed load releases the gate so the next caller retries.
async fn load_once<T, F, Fut>(
    model: &str,
    gates: &LoadGates,
    cached: impl Fn(&str) -> Option<Arc<T>>,
    load: F,
) -> Result<Arc<T>, PluginError>
where
    T: Send + Sync + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<Arc<T>, PluginError>> + Send + 'static,
{
    if let Some(provider) = cached(model) {
        return Ok(provider);
    }
    let gate = load_gate(gates, model);
    let guard = gate.lock_owned().await;
    if let Some(provider) = cached(model) {
        return Ok(provider);
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _owner = guard;
        drop(tx.send(load().await));
    });
    rx.await
        .map_err(|_| PluginError::provider("model load task ended without a result"))?
}

fn load_gate(
    gates: &Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    model: &str,
) -> Arc<tokio::sync::Mutex<()>> {
    gates
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(model.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

fn cached_chat_model(model: &str) -> Option<Arc<LocalLlamaCppProvider>> {
    CHAT_MODELS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(model)
        .cloned()
}

fn cached_embed_model(model: &str) -> Option<Arc<GgufEmbeddingProvider>> {
    EMBED_MODELS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(model)
        .cloned()
}

/// Loads the chat model for `model` and inserts it into the cache. The
/// caller must hold the model's load gate.
async fn load_chat_model(model: String) -> Result<Arc<LocalLlamaCppProvider>, PluginError> {
    let config = current_config()?;
    let profile = profile_for(&model)?;
    let weights = resolve_weights(&model, &profile, &config).await?;
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
    Ok(insert_if_absent(&model, loaded))
}

/// Loads the embedding model for `model` and inserts it into the cache. The
/// caller must hold the model's load gate.
async fn load_embed_model(model: String) -> Result<Arc<GgufEmbeddingProvider>, PluginError> {
    let config = current_config()?;
    let profile = profile_for(&model)?;
    let weights = resolve_weights(&model, &profile, &config).await?;
    let path = weights.to_string_lossy().into_owned();
    let name = model.clone();
    let quantization = profile.quantization().to_string();
    let acceleration = config.acceleration()?;
    let loaded = load_blocking(
        move || {
            GgufEmbeddingProvider::load_with_acceleration(&name, &path, &quantization, acceleration)
        },
        |e| map_embed_load_error(&e),
    )
    .await?;
    Ok(insert_embed_if_absent(&model, loaded))
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

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect for concise failure messages"
)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_GATES: LoadGates = LazyLock::new(|| Mutex::new(HashMap::new()));
    static TEST_CACHE: LazyLock<Mutex<HashMap<String, Arc<String>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    fn cached(model: &str) -> Option<Arc<String>> {
        TEST_CACHE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(model)
            .cloned()
    }

    /// Concurrent callers for the same model key share one in-flight load:
    /// the load closure runs once and every caller receives the same `Arc`.
    #[tokio::test]
    async fn concurrent_loads_share_one_in_flight_load() {
        let loads = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let loads = Arc::clone(&loads);
            handles.push(tokio::spawn(async move {
                load_once("model", &TEST_GATES, cached, move || {
                    let loads = Arc::clone(&loads);
                    async move {
                        loads.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        let provider = Arc::new("loaded".to_string());
                        TEST_CACHE
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .insert("model".to_string(), Arc::clone(&provider));
                        Ok(provider)
                    }
                })
                .await
                .expect("load succeeds")
            }));
        }
        let mut results = Vec::new();
        for handle in handles {
            results.push(handle.await.expect("task joins"));
        }
        assert_eq!(loads.load(Ordering::SeqCst), 1, "one in-flight load");
        assert!(
            results.iter().all(|r| Arc::ptr_eq(&results[0], r)),
            "all callers must observe the same loaded model"
        );
    }

    /// A failed load releases the gate so the next caller retries instead of
    /// being stuck on the failed result.
    #[tokio::test]
    async fn failed_load_releases_gate_for_retry() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = Arc::clone(&attempts);
        let err = load_once("failing", &TEST_GATES, cached, move || async move {
            attempts_clone.fetch_add(1, Ordering::SeqCst);
            Err(PluginError::provider("boom"))
        })
        .await
        .expect_err("first load fails");
        assert!(err.to_string().contains("boom"));

        let attempts_clone = Arc::clone(&attempts);
        let provider = load_once("failing", &TEST_GATES, cached, move || async move {
            attempts_clone.fetch_add(1, Ordering::SeqCst);
            let provider = Arc::new("loaded".to_string());
            TEST_CACHE
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert("failing".to_string(), Arc::clone(&provider));
            Ok(provider)
        })
        .await
        .expect("retry succeeds");
        assert_eq!(provider.as_str(), "loaded");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}
