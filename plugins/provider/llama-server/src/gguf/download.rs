//! GGUF download via the shared `ene_ai::ModelFetcher`, plus content-addressed
//! cache filename derivation.
//!
//! The download mechanics (in-flight coalescing, `.part` + atomic rename,
//! RAII partial cleanup, HTTPS-only enforcement, progress reporting) live in
//! [`ene_ai::model_fetch`] and are shared with `ene-voice`'s Kokoro model
//! downloads. This module supplies the one GGUF-specific piece — a
//! [`ene_ai::MagicBytesValidator`] for the `GGUF` magic — plus the
//! blake3 content-addressed cache filename scheme, which stays local since
//! it is orthogonal to the download mechanics themselves.

use ene_ai::error::LlmProviderError;
use ene_ai::model_fetch::MagicBytesValidator;
use ene_ai::{ModelFetcher, ModelValidator};
use std::path::{Path, PathBuf};

const GGUF_MAGIC: &[u8; 4] = b"GGUF";
const CACHE_HASH_HEX_LEN: usize = 12;

static GGUF_VALIDATOR: MagicBytesValidator = MagicBytesValidator::new("gguf", GGUF_MAGIC);

pub async fn file_has_gguf_magic(path: &Path) -> bool {
    GGUF_VALIDATOR.validate(path).await
}

pub fn filename_from_url(url: &str) -> Result<String, LlmProviderError> {
    ene_ai::validate_https_url(url)?;
    let path = ene_ai::model_fetch::strip_url_path(url);
    let segment = path
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty() && s.contains('.'))
        .ok_or_else(|| {
            LlmProviderError::Provider(format!("cannot derive GGUF filename from URL: {url}"))
        })?;
    let stem = ene_ai::model_fetch::sanitize_basename(segment)?;
    let hash = blake3::hash(url.trim().as_bytes());
    let hex = hash.to_hex();
    // blake3 hex is ASCII; take a fixed prefix without UTF-8 slicing risk.
    let short: String = hex.chars().take(CACHE_HASH_HEX_LEN).collect();
    Ok(format!("{stem}-{short}.gguf"))
}

/// Download `url` to `dest`, skipping when a valid GGUF already exists.
pub async fn download_gguf(url: &str, dest: &Path) -> Result<(), LlmProviderError> {
    ModelFetcher::new()
        .fetch(url, dest, &GGUF_VALIDATOR)
        .await
        .map_err(Into::into)
}

pub fn gguf_cache_dir() -> PathBuf {
    ene_config::models_dir().join("gguf")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_rejects_traversal_and_separators() {
        assert!(filename_from_url("https://cdn.example/foo/..evil.gguf").is_err());
        assert!(filename_from_url("https://cdn.example/foo/..%2Fevil.gguf").is_err());
        assert!(filename_from_url("https://cdn.example/foo/bar\\baz.gguf").is_err());
        let ok = filename_from_url("https://cdn.example/models/safe.gguf").expect("safe");
        assert!(!ok.contains(".."));
        assert!(!ok.contains('/'));
        assert!(!ok.contains('\\'));
    }

    #[test]
    fn filename_strips_query_and_is_stable_per_url() {
        let a = filename_from_url("https://cdn.example/models/v5-small.gguf?download=true")
            .expect("url");
        let b = filename_from_url("https://cdn.example/models/v5-small.gguf?download=true")
            .expect("url");
        assert_eq!(a, b);
        assert!(a.starts_with("v5-small-"));
        assert!(
            std::path::Path::new(&a)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
        );
        assert!(!a.contains('?'));
    }

    #[test]
    fn filename_differs_for_same_basename_different_urls() {
        let a = filename_from_url("https://cdn.example/repo-a/model.gguf").expect("a");
        let b = filename_from_url("https://cdn.example/repo-b/model.gguf").expect("b");
        assert_ne!(a, b);
        assert!(a.starts_with("model-"));
        assert!(b.starts_with("model-"));
    }

    #[tokio::test]
    async fn file_has_gguf_magic_detects_header() {
        use tokio::io::AsyncWriteExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let good = dir.path().join("good.gguf");
        let bad = dir.path().join("bad.gguf");
        {
            let mut f = tokio::fs::File::create(&good).await.expect("create");
            f.write_all(b"GGUF\x00\x00\x00\x01").await.expect("write");
        }
        {
            let mut f = tokio::fs::File::create(&bad).await.expect("create");
            f.write_all(b"NOTG").await.expect("write");
        }
        assert!(file_has_gguf_magic(&good).await);
        assert!(!file_has_gguf_magic(&bad).await);
    }
}
