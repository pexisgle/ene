//! GGUF download, cache, and path resolution.

mod download;

use download::{download_gguf, file_has_gguf_magic};
use ene_ai::error::LlmProviderError;
use ene_ai::resolve::ResolvedLocalModel;
use std::path::PathBuf;

pub use download::{filename_from_url, gguf_cache_dir};

/// Ensure a local model's GGUF weights are present on disk (may download).
pub async fn ensure_gguf_available(
    local: &ResolvedLocalModel,
) -> Result<PathBuf, LlmProviderError> {
    let path = resolve_target_path(local)?;
    if path.is_file() {
        if file_has_gguf_magic(&path).await {
            return Ok(path);
        }
        if !local.model_path.trim().is_empty() {
            return Err(LlmProviderError::Provider(format!(
                "local model_path is not a GGUF (missing magic): {}",
                local.model_path
            )));
        }
        tracing::warn!(
            component = "GgufDownload",
            path = %path.display(),
            "cached file lacks GGUF magic; re-downloading"
        );
        drop(tokio::fs::remove_file(&path).await);
    }
    if !local.model_path.trim().is_empty() {
        return Err(LlmProviderError::Provider(format!(
            "local model_path does not exist: {}",
            local.model_path
        )));
    }
    // Manual / legacy downloads often land as `{stem}.gguf` without the URL hash suffix.
    if let Some(reused) = try_reuse_unhashed_cache(&local.url, &path).await? {
        return Ok(reused);
    }
    download_gguf(&local.url, &path).await?;
    Ok(path)
}

/// Ensure a local model's multimodal projector GGUF is present (may download).
pub async fn ensure_mmproj_available(
    local: &ResolvedLocalModel,
) -> Result<Option<PathBuf>, LlmProviderError> {
    if !local.has_mmproj() {
        return Ok(None);
    }
    let path = resolve_mmproj_path(local)?;
    if path.is_file() {
        if file_has_gguf_magic(&path).await {
            return Ok(Some(path));
        }
        if !local.mmproj_path.trim().is_empty() {
            return Err(LlmProviderError::Provider(format!(
                "local mmproj_path is not a GGUF (missing magic): {}",
                local.mmproj_path
            )));
        }
        tracing::warn!(
            component = "GgufDownload",
            path = %path.display(),
            "cached mmproj lacks GGUF magic; re-downloading"
        );
        drop(tokio::fs::remove_file(&path).await);
    }
    if !local.mmproj_path.trim().is_empty() {
        return Err(LlmProviderError::Provider(format!(
            "local mmproj_path does not exist: {}",
            local.mmproj_path
        )));
    }
    if let Some(reused) = try_reuse_unhashed_cache(&local.mmproj_url, &path).await? {
        return Ok(Some(reused));
    }
    download_gguf(&local.mmproj_url, &path).await?;
    Ok(Some(path))
}

/// Cache path for a URL-derived `{stem}.gguf` (no hash), used for legacy/manual weights.
fn unhashed_cache_path_from_url(url: &str) -> Result<PathBuf, LlmProviderError> {
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
    Ok(gguf_cache_dir().join(format!("{stem}.gguf")))
}

/// If `{stem}.gguf` exists beside the hashed cache path, link and reuse it.
async fn try_reuse_unhashed_cache(
    url: &str,
    hashed_path: &PathBuf,
) -> Result<Option<PathBuf>, LlmProviderError> {
    let unhashed = unhashed_cache_path_from_url(url)?;
    if unhashed == *hashed_path || !unhashed.is_file() {
        return Ok(None);
    }
    if !file_has_gguf_magic(&unhashed).await {
        return Ok(None);
    }

    if !hashed_path.exists() {
        if let Some(parent) = hashed_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| LlmProviderError::Provider(format!("create GGUF cache dir: {e}")))?;
        }
        let link_target = unhashed.file_name().ok_or_else(|| {
            LlmProviderError::Provider("unhashed GGUF has no file name".to_string())
        })?;
        match link_cache_entry(link_target, hashed_path) {
            Ok(()) => {
                tracing::info!(
                    component = "GgufDownload",
                    hashed = %hashed_path.display(),
                    unhashed = %unhashed.display(),
                    "Reusing unhashed GGUF cache entry"
                );
            }
            Err(e) => {
                tracing::warn!(
                    component = "GgufDownload",
                    hashed = %hashed_path.display(),
                    unhashed = %unhashed.display(),
                    error = %e,
                    "Could not link hashed cache name; using unhashed path directly"
                );
                return Ok(Some(unhashed));
            }
        }
    }

    if hashed_path.is_file() && file_has_gguf_magic(hashed_path).await {
        return Ok(Some(hashed_path.clone()));
    }
    Ok(Some(unhashed))
}

fn link_cache_entry(link_target: &std::ffi::OsStr, hashed_path: &PathBuf) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(link_target, hashed_path)
    }
    #[cfg(not(unix))]
    {
        let parent = hashed_path
            .parent()
            .map_or_else(|| PathBuf::from("."), PathBuf::from);
        let src = parent.join(link_target);
        std::fs::hard_link(&src, hashed_path).or_else(|_| {
            std::fs::copy(&src, hashed_path)?;
            Ok(())
        })
    }
}

fn resolve_mmproj_path(local: &ResolvedLocalModel) -> Result<PathBuf, LlmProviderError> {
    if !local.mmproj_path.trim().is_empty() {
        return Ok(PathBuf::from(local.mmproj_path.trim()));
    }
    if local.mmproj_url.trim().is_empty() {
        return Err(LlmProviderError::Provider(format!(
            "local model {:?} has no mmproj_url or mmproj_path",
            local.name
        )));
    }
    let filename = filename_from_url(&local.mmproj_url)?;
    Ok(gguf_cache_dir().join(filename))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unhashed_cache_path_uses_stem_without_url_hash() {
        let url = "https://cdn.example/models/gemma-4-E4B-it-Q4_0.gguf";
        let hashed = filename_from_url(url).expect("hashed");
        let unhashed = unhashed_cache_path_from_url(url).expect("unhashed");
        assert_eq!(
            unhashed.file_name().and_then(|s| s.to_str()),
            Some("gemma-4-E4B-it-Q4_0.gguf")
        );
        assert_ne!(hashed, "gemma-4-E4B-it-Q4_0.gguf");
        assert!(hashed.starts_with("gemma-4-E4B-it-Q4_0-"));
    }
}
