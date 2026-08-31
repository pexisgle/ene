use std::io::Read;
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
    if !is_allowed_url(version.url) {
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
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn enclosed_entry_path<R: Read + ?Sized>(
    file: &zip::read::ZipFile<'_, R>,
) -> Result<PathBuf, AssetError> {
    file.enclosed_name()
        .ok_or_else(|| AssetError::Archive("path traversal".to_owned()))
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
        let relative = enclosed_entry_path(&file)?;
        let out_path = dest.join(relative);
        if file.is_dir() {
            tokio::fs::create_dir_all(&out_path)
                .await
                .map_err(AssetError::Io)?;
            continue;
        }
        if let Some(parent) = out_path.parent()
            && !parent.starts_with(dest)
        {
            return Err(AssetError::Archive("path traversal".to_owned()));
        }
        ensure_parent(&out_path).map_err(AssetError::Io)?;
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
    enclosed_entry_path(&file)?;
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
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            writer.start_file(*name, options).expect("start zip file");
            writer.write_all(data).expect("write zip file");
        }
        writer.finish().expect("finish zip").into_inner()
    }

    #[test]
    fn verify_rejects_mismatch() {
        let tmp = tempfile::NamedTempFile::new().expect("temp");
        std::fs::write(tmp.path(), b"abc").expect("write");
        assert!(verify_file(tmp.path(), "00").is_err());
    }

    #[test]
    fn sha256_matches_known_empty_and_abc() {
        let empty = tempfile::NamedTempFile::new().expect("temp");
        std::fs::write(empty.path(), b"").expect("write");
        assert_eq!(
            compute_sha256_file(empty.path()).expect("hash"),
            EMPTY_SHA256
        );

        let abc = tempfile::NamedTempFile::new().expect("temp");
        std::fs::write(abc.path(), b"abc").expect("write");
        assert_eq!(compute_sha256_file(abc.path()).expect("hash"), ABC_SHA256);
        verify_file(abc.path(), ABC_SHA256).expect("match");
    }

    #[test]
    fn sha256_streams_large_file() {
        let tmp = tempfile::NamedTempFile::new().expect("temp");
        let payload = vec![0x5a_u8; 200 * 1024];
        std::fs::write(tmp.path(), &payload).expect("write");
        let streamed = compute_sha256_file(tmp.path()).expect("hash");
        let expected = hex::encode(Sha256::digest(&payload));
        assert_eq!(streamed, expected);
    }

    #[tokio::test]
    async fn zip_tree_extracts_safe_member() {
        let dir = tempfile::tempdir().expect("tempdir");
        let zip_path = dir.path().join("ok.zip");
        std::fs::write(&zip_path, zip_bytes(&[("nested/ok.txt", b"hello")])).expect("write zip");
        let dest = dir.path().join("out");
        extract_zip_tree_from_file(&zip_path, &dest)
            .await
            .expect("extract");
        assert_eq!(
            std::fs::read_to_string(dest.join("nested/ok.txt")).expect("read"),
            "hello"
        );
    }

    #[tokio::test]
    async fn zip_tree_rejects_parent_traversal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let zip_path = dir.path().join("evil.zip");
        std::fs::write(&zip_path, zip_bytes(&[("../escape.txt", b"nope")])).expect("write zip");
        let dest = dir.path().join("out");
        std::fs::create_dir_all(&dest).expect("mkdir");
        let err = extract_zip_tree_from_file(&zip_path, &dest)
            .await
            .expect_err("traversal");
        assert!(
            matches!(err, AssetError::Archive(ref message) if message.contains("path traversal")),
            "{err:?}"
        );
        assert!(!dir.path().join("escape.txt").exists());
    }

    #[tokio::test]
    async fn zip_tree_does_not_escape_on_absolute_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let zip_path = dir.path().join("abs.zip");
        std::fs::write(
            &zip_path,
            zip_bytes(&[("/tmp/ene-provider-assets-abs.txt", b"nope")]),
        )
        .expect("write zip");
        let dest = dir.path().join("out");
        std::fs::create_dir_all(&dest).expect("mkdir");
        let result = extract_zip_tree_from_file(&zip_path, &dest).await;
        assert!(
            !Path::new("/tmp/ene-provider-assets-abs.txt").exists(),
            "absolute zip name must not write outside dest"
        );
        if let Err(error) = result {
            assert!(error.to_string().contains("path traversal"), "{error:?}");
        }
    }

    #[tokio::test]
    async fn zip_member_rejects_traversal_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("out.bin");
        let err = extract_zip_member(
            &zip_bytes(&[("../escape.bin", b"nope")]),
            "../escape.bin",
            &dest,
        )
        .await
        .expect_err("traversal");
        assert!(
            matches!(err, AssetError::Archive(ref message) if message.contains("path traversal")),
            "{err:?}"
        );
        assert!(!dest.exists());
    }
}
