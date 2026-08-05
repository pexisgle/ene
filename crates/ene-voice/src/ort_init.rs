//! Shared ONNX Runtime initialization for local audio providers.
//!
//! `ort` requires its dynamic library to be loaded and its environment
//! committed before any [`ort::session::Session`] is built. Both the Kokoro
//! TTS provider and the Silero VAD engine create sessions, so they share this
//! one-time initializer to guarantee `ort::init` / `ort::init_from` runs
//! exactly once per process.

use std::sync::OnceLock;

use ene_ai::AudioProviderError;

/// Result of the one-time ORT initialization, cached process-wide.
static ORT_INIT: OnceLock<Result<(), String>> = OnceLock::new();

/// The normalized dylib path used by the first caller that initialized ORT,
/// cached so later callers can be warned when they request a conflicting path.
static ORT_DYLIB_PATH: OnceLock<Option<String>> = OnceLock::new();

/// Ensure ONNX Runtime is initialized exactly once for the process.
///
/// When `dylib_path` is `Some` (and non-empty), the ONNX Runtime dynamic
/// library is loaded from that path via [`ort::init_from`]; otherwise the
/// default `ort` resolution is used via [`ort::init`]. The environment is then
/// committed so the settings take effect before any session is built.
///
/// The first caller's `dylib_path` wins: the initialization runs exactly once
/// and subsequent calls return the cached result regardless of their argument.
/// If a later caller supplies a different `dylib_path`, a `tracing::warn!` is
/// emitted noting that the request is ignored.
///
/// # Errors
///
/// Returns [`AudioProviderError::Init`] if the dylib fails to load. The first
/// error is cached and returned on every subsequent call.
pub fn ensure_ort_init(dylib_path: Option<&str>) -> Result<(), AudioProviderError> {
    let normalized = dylib_path
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string);
    let result = ORT_INIT.get_or_init(|| {
        // Record the first caller's path for later conflict detection.
        let _already_set = ORT_DYLIB_PATH.set(normalized.clone());
        init_ort(normalized.as_deref())
    });
    // The first caller's dylib wins and cannot be changed; warn when a later
    // caller requested a different path so the mismatch is visible.
    if ORT_DYLIB_PATH.get() != Some(&normalized) {
        tracing::warn!(
            component = "OrtInit",
            requested = normalized.as_deref().unwrap_or("<default>"),
            active = ORT_DYLIB_PATH
                .get()
                .and_then(Option::as_deref)
                .unwrap_or("<default>"),
            "ONNX Runtime already initialized with a different dylib_path; ignoring this request"
        );
    }
    result.clone().map_err(AudioProviderError::Init)
}

fn init_ort(dylib_path: Option<&str>) -> Result<(), String> {
    let path = dylib_path.map(str::trim).filter(|p| !p.is_empty());
    let builder = match path {
        Some(p) => ort::init_from(p).map_err(|e| format!("ort::init_from({p}) failed: {e}"))?,
        None => ort::init(),
    };
    // `commit()` returns false when an environment was already configured
    // (e.g. by the host application); that is not an error for us.
    let _committed = builder.commit();
    tracing::info!(
        component = "OrtInit",
        dylib = path.unwrap_or("<default>"),
        "initialized ONNX Runtime"
    );
    Ok(())
}
