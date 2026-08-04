//! Feature-independent Kokoro model download and path resolution.
//!
//! Everything in this module compiles and can be called with the
//! `local-tts` feature disabled — [`prefetch_if_configured`] is invoked from
//! the runtime's async bootstrap path
//! (`ene-runtime/src/handle/mod.rs`), which never enables `local-tts` (that
//! feature only gates the `ort` / ONNX Runtime native dependency). Network
//! I/O goes through the shared [`ene_ai::ModelFetcher`], which supplies
//! in-flight coalescing, `.part` + atomic rename, RAII partial cleanup,
//! HTTPS-only enforcement, and progress reporting; this module supplies the
//! two Kokoro-specific [`ene_ai::ModelValidator`]s.

use ene_ai::model_fetch::{PrefixPredicateValidator, SizeMultipleValidator};
use ene_ai::{AudioProviderError, ModelFetcher, ModelValidator};

/// Default `HuggingFace` download URL for the Kokoro 82M ONNX model file.
pub const KOKORO_DEFAULT_MODEL_URL: &str =
    "https://huggingface.co/huangjune/Kokoro-82M-v1.0-ONNX/resolve/main/model.onnx";

/// Default `HuggingFace` download URL for the Kokoro `voices.bin` embeddings file.
pub const KOKORO_DEFAULT_VOICES_URL: &str =
    "https://huggingface.co/huangjune/Kokoro-82M-v1.0-ONNX/resolve/main/voices.bin";

/// Fallback `HuggingFace` download URL for the Kokoro 82M ONNX model file.
pub const KOKORO_FALLBACK_MODEL_URL: &str =
    "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/main/onnx/model.onnx";

/// Fallback `HuggingFace` download URL for the Kokoro `voices.bin` embeddings file.
pub const KOKORO_FALLBACK_VOICES_URL: &str =
    "https://huggingface.co/speaches-ai/Kokoro-82M-v1.0-ONNX-fp16/resolve/main/voices.bin";

/// Dimensionality of a single Kokoro voice style embedding.
///
/// Not gated behind `local-tts`: [`voices_validator`] (this module) needs it
/// to validate a downloaded `voices.bin` regardless of whether the ONNX
/// runtime is compiled in. `provider::load_voice_embedding` reuses this same
/// constant when the feature is enabled, instead of duplicating it.
pub const VOICE_DIM: usize = 256;

/// Get the default local path for `kokoro.onnx`.
#[must_use]
pub fn default_kokoro_model_path() -> std::path::PathBuf {
    ene_config::models_dir().join("gguf").join("kokoro.onnx")
}

/// Get the default local path for `voices.bin`.
#[must_use]
pub fn default_kokoro_voices_path() -> std::path::PathBuf {
    ene_config::models_dir().join("gguf").join("voices.bin")
}

/// Resolve the Kokoro ONNX model path from configuration.
///
/// Precedence: `TtsConfig::model_path` when non-empty, then `TtsConfig::model`
/// when non-empty, then a default cache location. Environment overrides are
/// handled by the config system (`ENE_AI__TTS__MODEL_PATH`).
pub(crate) fn resolve_model_path(ai: &ene_ai::AiConfig) -> std::path::PathBuf {
    if let Some(path) = ai
        .tts
        .model_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        return std::path::PathBuf::from(path);
    }
    if !ai.tts.model.trim().is_empty() {
        return std::path::PathBuf::from(ai.tts.model.trim());
    }
    default_kokoro_model_path()
}

/// Resolve the Kokoro `voices.bin` path from configuration.
///
/// Precedence: the Kokoro plugin profile
/// `plugins.list.kokoro.profiles.kokoro.voices_path` when set (the previous
/// `TtsConfig::voices_path` moved here in #313), then a default cache
/// location.
///
/// Reads from the passed-in config document (not the global singleton) so the
/// result agrees with the other `plugins.list` reads made from that same
/// document (e.g. `ort_dylib_path` in `LocalTtsProviderFactory::build`).
pub(crate) fn resolve_voices_path(config: &ene_config::EneConfig) -> std::path::PathBuf {
    let profile = ene_ai::plugin_config::plugin_profile_blob(
        config,
        ene_ai::plugin_config::KOKORO_PLUGIN,
        ene_ai::plugin_config::KOKORO_DEFAULT_PROFILE,
    );
    if let Some(path) = profile
        .as_ref()
        .and_then(|p| p.get("voices_path"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        return std::path::PathBuf::from(path);
    }
    default_kokoro_voices_path()
}

/// Heuristic ONNX validity check.
///
/// ONNX model files are protobuf-encoded `ModelProto` messages and, unlike
/// GGUF, have no fixed magic byte sequence at all. This checks that the
/// leading byte decodes as a structurally valid protobuf field tag (a
/// non-deprecated wire type), which is enough to reject the failure mode
/// this validator exists for: a truncated download or an HTML/JSON error
/// page (`<`, `{`) landing in place of the model.
fn looks_like_onnx_protobuf(prefix: &[u8]) -> bool {
    prefix
        .first()
        .is_some_and(|b| matches!(b & 0x07, 0 | 1 | 2 | 5))
}

/// Minimum plausible size for a real Kokoro ONNX model file. Comfortably
/// below the real ~300 MB file and comfortably above a typical error page,
/// so this only ever rejects obviously-wrong responses.
const MIN_ONNX_BYTES: u64 = 4096;

fn onnx_validator() -> PrefixPredicateValidator {
    PrefixPredicateValidator::new("onnx", MIN_ONNX_BYTES, 8, looks_like_onnx_protobuf)
}

/// `voices.bin` is a flat little-endian `f32` array of stacked
/// [`VOICE_DIM`]-float voice embeddings with no header at all — the honest
/// validity check is that its size is a whole number of embedding vectors,
/// not a magic-byte match.
fn voices_validator() -> SizeMultipleValidator {
    let element_bytes = (VOICE_DIM * std::mem::size_of::<f32>()) as u64;
    SizeMultipleValidator::new("voices-bin", element_bytes, element_bytes)
}

/// Ensure a file exists at `dest`, downloading it from `url` via the shared
/// [`ModelFetcher`] if missing or invalid.
async fn ensure_file_downloaded(
    url: &str,
    dest: &std::path::Path,
    validator: &dyn ModelValidator,
) -> Result<(), AudioProviderError> {
    ModelFetcher::new()
        .fetch(url, dest, validator)
        .await
        .map_err(Into::into)
}

/// Ensure Kokoro ONNX model and `voices.bin` files exist on disk, downloading them
/// automatically if missing.
///
/// Performs network I/O; must be awaited from an async context (settings UI
/// download button, or [`prefetch_if_configured`] during bootstrap). Never
/// called from `provider::open`.
pub async fn ensure_kokoro_files_exist(
    model_path: &std::path::Path,
    voices_path: &std::path::Path,
) -> Result<(), AudioProviderError> {
    let onnx = onnx_validator();
    if let Err(e) = ensure_file_downloaded(KOKORO_DEFAULT_MODEL_URL, model_path, &onnx).await {
        tracing::warn!(error = %e, "Primary Kokoro model URL failed; attempting fallback URL...");
        ensure_file_downloaded(KOKORO_FALLBACK_MODEL_URL, model_path, &onnx).await?;
    }
    let voices = voices_validator();
    if let Err(e) = ensure_file_downloaded(KOKORO_DEFAULT_VOICES_URL, voices_path, &voices).await {
        tracing::warn!(error = %e, "Primary Kokoro voices URL failed; attempting fallback URL...");
        ensure_file_downloaded(KOKORO_FALLBACK_VOICES_URL, voices_path, &voices).await?;
    }
    Ok(())
}

/// Prefetch the Kokoro ONNX model and `voices.bin` files if the AI config's
/// TTS section selects the local Kokoro provider ([`super::PROVIDER_NAME`]).
///
/// Reads its settings from the passed-in config document — the same one the
/// provider plugin and the in-process fallback factory will later use — so
/// the prefetched paths agree with the ones the active path resolves. The
/// kokoro plugin's own config blob (`plugins.list.kokoro.config.model_path`
/// / `voices_path`) wins, matching the plugin's resolution precedence;
/// `ai.tts.*` / the `kokoro` profile are used when the plugin config is
/// absent.
///
/// Intended to be called once from the runtime's async bootstrap path
/// (mirrors `ene_ai_local::prefetch_configured_gguf`), before any TTS
/// provider is constructed, so `provider::open` never needs to perform
/// network I/O. A no-op when TTS is disabled or configured for a different
/// provider (e.g. `"openai"`).
///
/// # Errors
///
/// Returns [`AudioProviderError`] when the download fails (or the AI config
/// section cannot be parsed). Callers should treat this as non-fatal: log it
/// and continue, since `provider::open` performs its own file-existence check
/// and reports a clear error either way.
pub async fn prefetch_if_configured(
    config: &ene_config::EneConfig,
) -> Result<(), AudioProviderError> {
    let ai = config
        .get_section::<ene_ai::AiConfig>()
        .map_err(|e| AudioProviderError::Init(format!("failed to parse AI config: {e}")))?;
    let Some(resolved) = ai.resolve_tts() else {
        return Ok(());
    };
    if resolved.provider != super::PROVIDER_NAME {
        return Ok(());
    }
    let model_path =
        plugin_config_path(config, "model_path").unwrap_or_else(|| resolve_model_path(&ai));
    let voices_path =
        plugin_config_path(config, "voices_path").unwrap_or_else(|| resolve_voices_path(config));
    ensure_kokoro_files_exist(&model_path, &voices_path).await
}

/// Reads a path key from the kokoro plugin's config blob
/// (`plugins.list.kokoro.config`), the source the provider plugin resolves
/// before the profile or the `ai.tts.*` fallbacks.
fn plugin_config_path(config: &ene_config::EneConfig, key: &str) -> Option<std::path::PathBuf> {
    ene_ai::plugin_config::plugin_config_blob(config, ene_ai::plugin_config::KOKORO_PLUGIN)
        .as_ref()
        .and_then(|blob| blob.get(key))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(std::path::PathBuf::from)
}

#[cfg(test)]
mod tests {
    #![expect(clippy::expect_used, reason = "unit tests use expect for assertions")]

    use super::*;

    #[test]
    fn default_urls_and_paths_are_valid() {
        assert!(KOKORO_DEFAULT_MODEL_URL.starts_with("https://"));
        assert!(KOKORO_DEFAULT_VOICES_URL.starts_with("https://"));
        assert!(
            default_kokoro_model_path()
                .to_string_lossy()
                .contains("kokoro.onnx")
        );
        assert!(
            default_kokoro_voices_path()
                .to_string_lossy()
                .contains("voices.bin")
        );
    }

    #[test]
    fn plugin_config_path_reads_custom_paths() {
        let mut config = ene_config::EneConfig::default();
        config
            .set_path(
                "plugins.list.kokoro.config.model_path",
                "/custom/kokoro.onnx",
            )
            .expect("set model path");
        config
            .set_path(
                "plugins.list.kokoro.config.voices_path",
                "/custom/voices.bin",
            )
            .expect("set voices path");

        assert_eq!(
            plugin_config_path(&config, "model_path"),
            Some(std::path::PathBuf::from("/custom/kokoro.onnx"))
        );
        assert_eq!(
            plugin_config_path(&config, "voices_path"),
            Some(std::path::PathBuf::from("/custom/voices.bin"))
        );
        assert_eq!(plugin_config_path(&config, "voice"), None);
    }

    #[tokio::test]
    async fn onnx_validator_accepts_protobuf_and_rejects_html() {
        let dir = tempfile::tempdir().expect("tempdir");
        let good = dir.path().join("model.onnx");
        let bad = dir.path().join("error.html");
        std::fs::write(&good, vec![0x08_u8; MIN_ONNX_BYTES as usize]).expect("write good");
        std::fs::write(&bad, b"<!DOCTYPE html><html>not a model</html>").expect("write bad");

        let validator = onnx_validator();
        assert!(validator.validate(&good).await);
        assert!(!validator.validate(&bad).await);
    }

    #[tokio::test]
    async fn voices_validator_requires_whole_number_of_embeddings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ok = dir.path().join("voices-ok.bin");
        let short = dir.path().join("voices-short.bin");
        let element_bytes = VOICE_DIM * std::mem::size_of::<f32>();
        std::fs::write(&ok, vec![0u8; element_bytes * 2]).expect("write ok");
        std::fs::write(&short, vec![0u8; element_bytes - 4]).expect("write short");

        let validator = voices_validator();
        assert!(validator.validate(&ok).await);
        assert!(!validator.validate(&short).await);
    }
}
