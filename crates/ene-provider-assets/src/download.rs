use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::allowlist::is_allowed_url;
use crate::catalog::AssetKind;
use crate::catalog::CatalogVersion;
use crate::error::AssetError;
use crate::manifest::{InstallRecord, Manifest};
use crate::runtime_catalog::{CatalogVariant, ExtractMode, RuntimeCatalog};
use crate::store::{ensure_parent, variant_root, weight_path};

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
    if !version.url.starts_with("https://") || !is_allowed_url(version.url) {
        return Err(AssetError::UrlNotAllowed);
    }
    let dest = destination_path(plugin_id, kind, asset_id, version);
    if dest.is_file() {
        verify_file(&dest, version.sha256)?;
        register_install(
            plugin_id,
            asset_id,
            version.version,
            &dest,
            None,
            version.sha256,
        )?;
        return Ok(dest);
    }
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(AssetError::Io)?;
    let partial = dest.with_extension("partial");
    download_url(version.url, &partial, progress.clone()).await?;
    if let Some(member) = version.archive_member {
        extract_zip_member_from_file(&partial, member, &dest).await?;
        tokio::fs::remove_file(&partial)
            .await
            .map_err(AssetError::Io)?;
    } else {
        tokio::fs::rename(&partial, &dest)
            .await
            .map_err(AssetError::Io)?;
    }
    verify_file(&dest, version.sha256)?;
    register_install(
        plugin_id,
        asset_id,
        version.version,
        &dest,
        None,
        version.sha256,
    )?;
    Ok(dest)
}

pub async fn install_variant(
    plugin_id: &str,
    asset_id: &str,
    release_tag: &str,
    variant: &CatalogVariant,
    progress: Arc<Mutex<DownloadProgress>>,
) -> Result<PathBuf, AssetError> {
    let install_key = RuntimeCatalog::install_key(release_tag, &variant.id);
    let root = variant_root(plugin_id, asset_id, &install_key);
    if root.exists() {
        tokio::fs::remove_dir_all(&root)
            .await
            .map_err(AssetError::Io)?;
    }
    tokio::fs::create_dir_all(&root)
        .await
        .map_err(AssetError::Io)?;

    for artifact in &variant.artifacts {
        if !is_allowed_url(&artifact.url) {
            return Err(AssetError::UrlNotAllowed);
        }
        let partial = root.join(format!(
            "{}.partial",
            artifact.url.rsplit('/').next().unwrap_or("artifact")
        ));
        download_url(&artifact.url, &partial, progress.clone()).await?;
        match &artifact.extract {
            ExtractMode::RawFile => {
                let dest = if artifact.dest.is_empty() {
                    root.join(artifact.url.rsplit('/').next().unwrap_or("download"))
                } else {
                    root.join(&artifact.dest)
                };
                tokio::fs::rename(&partial, &dest)
                    .await
                    .map_err(AssetError::Io)?;
            }
            ExtractMode::ZipMember { member } => {
                let dest = root.join(member);
                extract_zip_member_from_file(&partial, member, &dest).await?;
                tokio::fs::remove_file(&partial)
                    .await
                    .map_err(AssetError::Io)?;
            }
            ExtractMode::ZipTree => {
                extract_zip_tree_from_file(&partial, &root).await?;
                tokio::fs::remove_file(&partial)
                    .await
                    .map_err(AssetError::Io)?;
            }
        }
    }

    let binary = variant
        .entry_binary
        .as_deref()
        .map(|name| root.join(name))
        .filter(|path| path.is_file())
        .ok_or_else(|| AssetError::Archive("installed binary missing after extract".to_owned()))?;

    let relative = format!("{asset_id}/{install_key}");

    let digest = if variant.artifacts.iter().all(|a| a.sha256.is_empty()) {
        compute_sha256_file(&binary).unwrap_or_default()
    } else {
        variant
            .artifacts
            .first()
            .map_or("", |row| row.sha256.as_str())
            .to_owned()
    };

    let mut manifest = Manifest::load(plugin_id);
    manifest.register_install(
        asset_id,
        InstallRecord {
            relative_path: relative,
            sha256: digest,
            version: Some(install_key.clone()),
            entry_binary: variant.entry_binary.clone(),
        },
    );
    if manifest.active_version(asset_id).is_none() {
        manifest.set_active(asset_id, install_key);
    }
    manifest.save(plugin_id)?;
    Ok(binary)
}

async fn download_url(
    url: &str,
    dest: &Path,
    progress: Arc<Mutex<DownloadProgress>>,
) -> Result<(), AssetError> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(AssetError::Io)?;
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(8))
        .build()
        .map_err(|err| AssetError::Download(err.to_string()))?;
    let response = client
        .get(url)
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
    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(AssetError::Io)?;
    let mut stream = response.bytes_stream();
    use futures::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| AssetError::Download(err.to_string()))?;
        file.write_all(&chunk).await.map_err(AssetError::Io)?;
        progress.lock().received += chunk.len() as u64;
    }
    file.flush().await.map_err(AssetError::Io)?;
    Ok(())
}

fn destination_path(
    plugin_id: &str,
    kind: AssetKind,
    asset_id: &str,
    version: &CatalogVersion,
) -> PathBuf {
    match kind {
        AssetKind::Sidecar => crate::store::sidecar_binary_path(
            plugin_id,
            asset_id,
            version.version,
            version.filename,
        ),
        AssetKind::Weight => weight_path(plugin_id, asset_id, version.filename),
    }
}

fn register_install(
    plugin_id: &str,
    asset_id: &str,
    version: &str,
    dest: &Path,
    entry_binary: Option<String>,
    sha256: &str,
) -> Result<(), AssetError> {
    let mut manifest = Manifest::load(plugin_id);
    let relative = dest
        .strip_prefix(crate::store::store_root(plugin_id))
        .map_or_else(
            |_| dest.display().to_string(),
            |path| path.to_string_lossy().into_owned(),
        );
    manifest.register_install(
        asset_id,
        InstallRecord {
            relative_path: relative,
            sha256: sha256.to_owned(),
            version: Some(version.to_owned()),
            entry_binary,
        },
    );
    if manifest.active_version(asset_id).is_none() {
        manifest.set_active(asset_id, version);
    }
    manifest.save(plugin_id)?;
    Ok(())
}

pub fn verify_file(path: &Path, expected_sha256: &str) -> Result<(), AssetError> {
    if expected_sha256.is_empty() {
        return Ok(());
    }
    let got = compute_sha256_file(path)?;
    if got.eq_ignore_ascii_case(expected_sha256) {
        Ok(())
    } else {
        Err(AssetError::DigestMismatch)
    }
}

fn compute_sha256_file(path: &Path) -> Result<String, AssetError> {
    let bytes = std::fs::read(path)?;
    let digest = Sha256::digest(bytes);
    Ok(hex::encode(digest))
}

async fn extract_zip_member_from_file(
    zip_path: &Path,
    member: &str,
    dest: &Path,
) -> Result<(), AssetError> {
    let bytes = tokio::fs::read(zip_path).await.map_err(AssetError::Io)?;
    extract_zip_member(&bytes, member, dest).await
}

async fn extract_zip_tree_from_file(zip_path: &Path, dest: &Path) -> Result<(), AssetError> {
    let bytes = tokio::fs::read(zip_path).await.map_err(AssetError::Io)?;
    let cursor = std::io::Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|err| AssetError::Archive(err.to_string()))?;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|err| AssetError::Archive(err.to_string()))?;
        let name = file.name().replace('\\', "/");
        if name.contains("..") {
            return Err(AssetError::Archive("path traversal".to_owned()));
        }
        let out_path = dest.join(name);
        if file.is_dir() {
            tokio::fs::create_dir_all(&out_path)
                .await
                .map_err(AssetError::Io)?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            ensure_parent(parent).map_err(AssetError::Io)?;
        }
        let mut out = tokio::fs::File::create(&out_path)
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
    }
    Ok(())
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
