use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::catalog::{AssetKind, CatalogVersion};
use crate::error::AssetError;
use crate::manifest::{InstallRecord, Manifest};
use crate::store::{ensure_parent, sidecar_binary_path, weight_path};

#[derive(Debug, Clone, Copy, Default)]
pub struct DownloadProgress {
    pub received: u64,
    pub total: Option<u64>,
}

pub async fn install_version(
    plugin_id: &str,
    kind: AssetKind,
    asset_id: &str,
    version: &CatalogVersion,
    progress: Arc<Mutex<DownloadProgress>>,
) -> Result<PathBuf, AssetError> {
    if !version.url.starts_with("https://") {
        return Err(AssetError::UrlNotAllowed);
    }
    let dest = destination_path(plugin_id, kind, asset_id, version)?;
    if dest.is_file() {
        verify_file(&dest, version.sha256)?;
        register_install(plugin_id, asset_id, version, &dest)?;
        return Ok(dest);
    }
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(AssetError::Io)?;
    let partial = dest.with_extension("partial");
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(8))
        .build()
        .map_err(|err| AssetError::Download(err.to_string()))?;
    let response = client
        .get(version.url)
        .send()
        .await
        .map_err(|err| AssetError::Download(err.to_string()))?;
    if !response.status().is_success() {
        return Err(AssetError::Download(format!("HTTP {}", response.status())));
    }
    {
        let mut slot = progress.lock();
        slot.received = 0;
        slot.total = response.content_length();
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|err| AssetError::Download(err.to_string()))?;
    progress.lock().received = bytes.len() as u64;
    if let Some(member) = version.archive_member {
        extract_zip_member(&bytes, member, &partial).await?;
    } else {
        tokio::fs::write(&partial, &bytes)
            .await
            .map_err(AssetError::Io)?;
    }
    verify_file(&partial, version.sha256)?;
    tokio::fs::rename(&partial, &dest)
        .await
        .map_err(AssetError::Io)?;
    register_install(plugin_id, asset_id, version, &dest)?;
    Ok(dest)
}

fn destination_path(
    plugin_id: &str,
    kind: AssetKind,
    asset_id: &str,
    version: &CatalogVersion,
) -> Result<PathBuf, AssetError> {
    Ok(match kind {
        AssetKind::Sidecar => {
            sidecar_binary_path(plugin_id, asset_id, version.version, version.filename)
        }
        AssetKind::Weight => weight_path(plugin_id, asset_id, version.filename),
    })
}

fn register_install(
    plugin_id: &str,
    asset_id: &str,
    version: &CatalogVersion,
    dest: &Path,
) -> Result<(), AssetError> {
    let mut manifest = Manifest::load(plugin_id);
    let relative = dest
        .strip_prefix(crate::store::store_root(plugin_id))
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| dest.display().to_string());
    manifest.register_install(
        asset_id,
        InstallRecord {
            relative_path: relative,
            sha256: version.sha256.to_owned(),
            version: Some(version.version.to_owned()),
        },
    );
    if manifest.active_version(asset_id).is_none() {
        manifest.set_active(asset_id, version.version);
    }
    manifest.save(plugin_id)?;
    Ok(())
}

pub fn verify_file(path: &Path, expected_sha256: &str) -> Result<(), AssetError> {
    if expected_sha256.is_empty() {
        return Ok(());
    }
    let bytes = std::fs::read(path)?;
    let digest = Sha256::digest(bytes);
    let got = hex::encode(digest);
    if got.eq_ignore_ascii_case(expected_sha256) {
        Ok(())
    } else {
        Err(AssetError::DigestMismatch)
    }
}

async fn extract_zip_member(zip_bytes: &[u8], member: &str, dest: &Path) -> Result<(), AssetError> {
    let cursor = std::io::Cursor::new(zip_bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|err| AssetError::Archive(err.to_string()))?;
    let mut file = archive
        .by_name(member)
        .map_err(|err| AssetError::Archive(err.to_string()))?;
    if file.name().contains("..") {
        return Err(AssetError::Archive("path traversal".to_owned()));
    }
    if let Some(parent) = dest.parent() {
        ensure_parent(parent).map_err(AssetError::Io)?;
    }
    let mut out = tokio::fs::File::create(dest)
        .await
        .map_err(AssetError::Io)?;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)
            .map_err(|err| AssetError::Archive(err.to_string()))?;
        if read == 0 {
            break;
        }
        out.write_all(&buffer[..read])
            .await
            .map_err(AssetError::Io)?;
    }
    out.flush().await.map_err(AssetError::Io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_rejects_mismatch() {
        let tmp = tempfile::NamedTempFile::new().expect("temp");
        std::fs::write(tmp.path(), b"abc").expect("write");
        assert!(verify_file(tmp.path(), "00").is_err());
    }
}
