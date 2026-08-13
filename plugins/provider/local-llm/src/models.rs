//! Per-profile lazy model registry.
//!
//! Each profile key loads at most one chat model and one embedding model per
//! host-configuration generation, kept until that configuration changes (VRAM
//! residency). The initial llama.cpp
//! build is a synchronous FFI-heavy call (`EngineHandle::try_spawn` builds
//! the first model on the calling thread), so loads run in `spawn_blocking`
//! and never block the plugin's async runtime.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use ene_ai::resolve::ResolvedLocalModel;
use ene_plugin::PluginError;

use crate::config::{self, HostConfig, Profile, current_config, profile_for};
use crate::convert;
use crate::embedding::{EneEmbeddingError, GgufEmbeddingProvider};
use crate::gguf::{ensure_gguf_available, ensure_mmproj_available};
use crate::local_llm::{LocalGgufLoadParams, LocalLlamaCppProvider};

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
/// Per-profile unload epochs: `unload` bumps the profile's epoch so a load
/// that started before the eviction cannot re-insert its model afterwards.
type UnloadEpochs = LazyLock<Mutex<HashMap<String, u64>>>;
static CHAT_UNLOAD_EPOCHS: UnloadEpochs = LazyLock::new(|| Mutex::new(HashMap::new()));
static EMBED_UNLOAD_EPOCHS: UnloadEpochs = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Invalidates loaded models and in-flight admission gates after a live host
/// configuration update. Existing request-held `Arc`s keep their engines
/// alive until those requests finish; subsequent requests build from the new
/// config instead.
pub(crate) fn config_changed() {
    let mut chat_models = CHAT_MODELS.lock().unwrap_or_else(PoisonError::into_inner);
    let mut embed_models = EMBED_MODELS.lock().unwrap_or_else(PoisonError::into_inner);
    let mut chat_gates = CHAT_LOAD_GATES
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let mut embed_gates = EMBED_LOAD_GATES
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    config::advance_generation();
    chat_models.clear();
    embed_models.clear();
    chat_gates.clear();
    embed_gates.clear();
    CHAT_UNLOAD_EPOCHS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
    EMBED_UNLOAD_EPOCHS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

/// Releases a profile's resident models (chat and embedding) and load gates.
///
/// In-flight requests hold their own `Arc` clones, so an engine stays alive
/// until those requests finish; the next request reloads the profile.
pub(crate) fn unload(model: &str) {
    // Load gates are deliberately kept: an in-flight load holds its gate, and
    // a concurrent caller must wait for it instead of starting a second load
    // of the same weights (double RAM/VRAM).
    evict_unloaded(&CHAT_UNLOAD_EPOCHS, &CHAT_MODELS, model);
    evict_unloaded(&EMBED_UNLOAD_EPOCHS, &EMBED_MODELS, model);
}

/// Returns the chat model for `model`, loading it on first use.
pub(crate) async fn chat_provider(model: &str) -> Result<Arc<LocalLlamaCppProvider>, PluginError> {
    let model_key = model.to_string();
    load_once(
        model,
        &CHAT_LOAD_GATES,
        &CHAT_UNLOAD_EPOCHS,
        cached_chat_model,
        move |epoch| load_chat_model(model_key, epoch),
    )
    .await
}

/// Returns the embedding model for `model`, loading it on first use.
pub(crate) async fn embed_provider(model: &str) -> Result<Arc<GgufEmbeddingProvider>, PluginError> {
    let model_key = model.to_string();
    load_once(
        model,
        &EMBED_LOAD_GATES,
        &EMBED_UNLOAD_EPOCHS,
        cached_embed_model,
        move |epoch| load_embed_model(model_key, epoch),
    )
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
    epochs: &UnloadEpochs,
    cached: impl Fn(&str) -> Option<Arc<T>>,
    load: F,
) -> Result<Arc<T>, PluginError>
where
    T: Send + Sync + 'static,
    F: FnOnce(u64) -> Fut + Send + 'static,
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
    let epoch = unload_epoch(epochs, model);
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _owner = guard;
        drop(tx.send(load(epoch).await));
    });
    rx.await
        .map_err(|_| PluginError::provider("model load task ended without a result"))?
}

/// Current unload epoch for `model`; `0` when never unloaded.
fn unload_epoch(epochs: &UnloadEpochs, model: &str) -> u64 {
    epochs
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(model)
        .copied()
        .unwrap_or(0)
}

fn bump_unload_epoch(epochs: &UnloadEpochs, model: &str) {
    *epochs
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(model.to_string())
        .or_default() += 1;
}

/// Evicts one profile: removes its resident model and bumps its unload epoch
/// so an in-flight load cannot re-insert. Load gates are not touched — see
/// [`unload`].
fn evict_unloaded<T>(epochs: &UnloadEpochs, cache: &Mutex<HashMap<String, Arc<T>>>, model: &str) {
    cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(model);
    bump_unload_epoch(epochs, model);
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
async fn load_chat_model(
    model: String,
    unload_epoch: u64,
) -> Result<Arc<LocalLlamaCppProvider>, PluginError> {
    let generation = config::generation();
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
    Ok(insert_if_absent(&model, loaded, generation, unload_epoch))
}

/// Loads the embedding model for `model` and inserts it into the cache. The
/// caller must hold the model's load gate.
async fn load_embed_model(
    model: String,
    unload_epoch: u64,
) -> Result<Arc<GgufEmbeddingProvider>, PluginError> {
    let generation = config::generation();
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
    Ok(insert_embed_if_absent(
        &model,
        loaded,
        generation,
        unload_epoch,
    ))
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
        artifact_id: String::new(),
        artifact_version: String::new(),
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
fn insert_if_absent(
    model: &str,
    provider: LocalLlamaCppProvider,
    generation: u64,
    load_epoch: u64,
) -> Arc<LocalLlamaCppProvider> {
    insert_if_unloaded(
        &CHAT_UNLOAD_EPOCHS,
        &CHAT_MODELS,
        model,
        Arc::new(provider),
        generation,
        load_epoch,
    )
}

fn insert_embed_if_absent(
    model: &str,
    provider: GgufEmbeddingProvider,
    generation: u64,
    load_epoch: u64,
) -> Arc<GgufEmbeddingProvider> {
    insert_if_unloaded(
        &EMBED_UNLOAD_EPOCHS,
        &EMBED_MODELS,
        model,
        Arc::new(provider),
        generation,
        load_epoch,
    )
}

/// Inserts a freshly loaded provider unless the world moved on while it was
/// loading: a config change (generation) or an `unload` (per-profile epoch)
/// makes the load's result stale, so it is returned to its caller but never
/// made resident. A model already serving live streams is never replaced.
fn insert_if_unloaded<T>(
    epochs: &UnloadEpochs,
    cache: &Mutex<HashMap<String, Arc<T>>>,
    model: &str,
    provider: Arc<T>,
    generation: u64,
    load_epoch: u64,
) -> Arc<T> {
    let mut guard = cache.lock().unwrap_or_else(PoisonError::into_inner);
    if config::generation() != generation || load_epoch != unload_epoch(epochs, model) {
        return provider;
    }
    if let Some(existing) = guard.get(model) {
        return Arc::clone(existing);
    }
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
    static TEST_EPOCHS: UnloadEpochs = LazyLock::new(|| Mutex::new(HashMap::new()));
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
    #[expect(
        clippy::await_holding_lock,
        reason = "the config-mutation lock serializes cross-test generation bumps; no task on this runtime ever takes it"
    )]
    async fn concurrent_loads_share_one_in_flight_load() {
        // Hold the config-mutation lock for the whole test: a concurrent
        // `set_config` from another test bumps the global generation, which
        // would suppress the load's cache insert, make a later caller
        // re-load, and deadlock it on the started-signal channel once the
        // test body has stopped receiving.
        let _config_guard = crate::config::CONFIG_MUTATION_LOCK
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let loads = Arc::new(AtomicUsize::new(0));
        // The test releases the single load closure only after it has
        // confirmed the load started, so the assertion below observes the
        // gate holding every concurrent caller instead of racing a sleep.
        let (started_tx, mut started_rx) = tokio::sync::mpsc::channel(1);
        let release = Arc::new(tokio::sync::Notify::new());
        let mut handles = Vec::new();
        for _ in 0..8 {
            let loads = Arc::clone(&loads);
            let release = Arc::clone(&release);
            let started_tx = started_tx.clone();
            handles.push(tokio::spawn(async move {
                let generation = config::generation();
                load_once("model", &TEST_GATES, &TEST_EPOCHS, cached, move |epoch| {
                    let loads = Arc::clone(&loads);
                    let release = Arc::clone(&release);
                    let started_tx = started_tx.clone();
                    async move {
                        let first_load = loads.fetch_add(1, Ordering::SeqCst) == 0;
                        if first_load {
                            // Register the waiter before signaling the test
                            // task: `notify_waiters` stores no permit, so a
                            // late registration would wait forever.
                            let release = release.notified();
                            // The send error is `Copy`, so `drop()` would
                            // trip `clippy::dropping_copy_types`.
                            #[expect(
                                clippy::let_underscore_must_use,
                                reason = "mpsc send error is Copy; drop() would trip dropping_copy_types"
                            )]
                            let _ = started_tx.try_send(());
                            release.await;
                        }
                        let provider = Arc::new("loaded".to_string());
                        Ok(insert_if_unloaded(
                            &TEST_EPOCHS,
                            &TEST_CACHE,
                            "model",
                            provider,
                            generation,
                            epoch,
                        ))
                    }
                })
                .await
                .expect("load succeeds")
            }));
        }
        started_rx
            .recv()
            .await
            .expect("the single load closure must start");
        release.notify_waiters();
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
        let err = load_once(
            "failing",
            &TEST_GATES,
            &TEST_EPOCHS,
            cached,
            move |_epoch| async move {
                attempts_clone.fetch_add(1, Ordering::SeqCst);
                Err(PluginError::provider("boom"))
            },
        )
        .await
        .expect_err("first load fails");
        assert!(err.to_string().contains("boom"));

        let attempts_clone = Arc::clone(&attempts);
        let generation = config::generation();
        let provider = load_once(
            "failing",
            &TEST_GATES,
            &TEST_EPOCHS,
            cached,
            move |epoch| async move {
                attempts_clone.fetch_add(1, Ordering::SeqCst);
                let provider = Arc::new("loaded".to_string());
                Ok(insert_if_unloaded(
                    &TEST_EPOCHS,
                    &TEST_CACHE,
                    "failing",
                    provider,
                    generation,
                    epoch,
                ))
            },
        )
        .await
        .expect("retry succeeds");
        assert_eq!(provider.as_str(), "loaded");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    /// `unload` during an in-flight load must neither start a second
    /// concurrent load (the gate is retained) nor let the first load's result
    /// become resident afterwards (the per-profile epoch suppresses the
    /// insert); the next caller loads exactly once, sequentially.
    #[tokio::test]
    async fn unload_during_in_flight_load_evicts_and_defers_next_load() {
        use std::sync::atomic::AtomicBool;

        let loads = Arc::new(AtomicUsize::new(0));
        let load_started = Arc::new(tokio::sync::Notify::new());
        let first_done = Arc::new(AtomicBool::new(false));
        let second_started = Arc::new(tokio::sync::Notify::new());
        let generation = config::generation();

        let loads_clone = Arc::clone(&loads);
        let started = Arc::clone(&load_started);
        let first_done_clone = Arc::clone(&first_done);
        let first = tokio::spawn(async move {
            load_once("racy", &TEST_GATES, &TEST_EPOCHS, cached, move |epoch| {
                let loads = Arc::clone(&loads_clone);
                let started = Arc::clone(&started);
                let first_done = Arc::clone(&first_done_clone);
                async move {
                    loads.fetch_add(1, Ordering::SeqCst);
                    started.notify_one();
                    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                    let provider = Arc::new("stale-load".to_string());
                    first_done.store(true, Ordering::SeqCst);
                    Ok(insert_if_unloaded(
                        &TEST_EPOCHS,
                        &TEST_CACHE,
                        "racy",
                        provider,
                        generation,
                        epoch,
                    ))
                }
            })
            .await
            .expect("first load succeeds")
        });

        load_started.notified().await;
        assert!(cached("racy").is_none(), "nothing resident before unload");

        // The production eviction helper on the test maps (mirrors `unload`).
        evict_unloaded(&TEST_EPOCHS, &TEST_CACHE, "racy");

        let loads_clone = Arc::clone(&loads);
        let first_done_clone = Arc::clone(&first_done);
        let second_started_clone = Arc::clone(&second_started);
        let second = tokio::spawn(async move {
            load_once("racy", &TEST_GATES, &TEST_EPOCHS, cached, move |epoch| {
                let loads = Arc::clone(&loads_clone);
                let first_done = Arc::clone(&first_done_clone);
                let second_started = Arc::clone(&second_started_clone);
                async move {
                    assert!(
                        first_done.load(Ordering::SeqCst),
                        "second load must wait for the in-flight load (gate retained)"
                    );
                    loads.fetch_add(1, Ordering::SeqCst);
                    second_started.notify_one();
                    let provider = Arc::new("fresh-load".to_string());
                    Ok(insert_if_unloaded(
                        &TEST_EPOCHS,
                        &TEST_CACHE,
                        "racy",
                        provider,
                        generation,
                        epoch,
                    ))
                }
            })
            .await
            .expect("second load succeeds")
        });

        // The second load may only begin after the first completed.
        tokio::time::timeout(std::time::Duration::from_secs(5), second_started.notified())
            .await
            .expect("second load must start after the gate is released");
        assert!(
            first_done.load(Ordering::SeqCst),
            "second load started while the first was still in flight"
        );

        let stale = first.await.expect("first task joins");
        let fresh = second.await.expect("second task joins");
        assert_eq!(stale.as_str(), "stale-load");
        assert_eq!(fresh.as_str(), "fresh-load");
        assert_eq!(
            loads.load(Ordering::SeqCst),
            2,
            "exactly two sequential loads"
        );
        // The in-flight load must never become resident after `unload` — the
        // resident entry (if the fresh load already inserted) is not it. (A
        // concurrent host-config change from another test can also suppress
        // the fresh insert, so residency of the fresh load is not asserted.)
        let resident = cached("racy");
        assert!(
            resident.as_ref().is_none_or(|p| !Arc::ptr_eq(&stale, p)),
            "the stale load must not re-insert after unload"
        );
    }
}
