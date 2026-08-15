//! Safe extraction of zip payloads (VOICEVOX VVPP and friends).
//!
//! The catalog signs the *bytes* of the archive, so the archive itself is
//! authentic — but a signed archive can still be malicious (a compromised
//! publisher, or a reuse of a valid signature over crafted bytes is not
//! possible, yet the publisher's own pipeline could ship a hostile entry).
//! Extraction therefore enforces the same rules as any untrusted archive:
//! no absolute paths, no `..` traversal, no symlinks, bounded entry count,
//! and a bounded total uncompressed size.

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, Write};
use std::path::{Component, Path, PathBuf};

use crate::catalog::{MAX_ARCHIVE_ENTRIES, PayloadFormat};
use crate::error::{ArtifactError, Result};
use tokio_util::sync::CancellationToken;

/// Maximum size of `engine_manifest.json`. The manifest is metadata (a few
/// KB in practice); the cap bounds the in-memory read regardless of what a
/// hostile archive claims.
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
/// Copy chunk for entry payloads, so cancellation is observed during large
/// file copies instead of only between entries.
const COPY_CHUNK_BYTES: usize = 64 * 1024;

/// Extracts `archive_bytes` (an in-memory zip, used by tests and small
/// payloads) into `destination`, enforcing the safety rules documented in
/// the module docs.
///
/// `destination` must already exist (created by the caller under the
/// generation root). `entrypoint` is the executable path inside the archive
/// (from `engine_manifest.json`'s `command` for VOICEVOX VVPP); it must be a
/// plain file within the archive and is made executable afterwards.
///
/// # Errors
///
/// Returns an error when the archive is not a valid zip, contains an entry
/// that escapes `destination` (absolute path, `..`, symlink, non-UTF-8
/// name), exceeds [`MAX_ARCHIVE_ENTRIES`] or the unpack limit, or when the
/// declared entrypoint is missing or not a regular file. A partial
/// extraction is left on disk; the caller removes the generation directory
/// on failure.
pub fn extract_zip_vvpp(
    archive_bytes: &[u8],
    destination: &Path,
    entrypoint: Option<&str>,
    unpack_limit: u64,
) -> Result<PathBuf> {
    extract_zip_reader(
        Cursor::new(archive_bytes),
        destination,
        entrypoint,
        unpack_limit,
        None,
    )
}

/// Extracts a zip file (streamed from disk, so multi-gigabyte VOICEVOX
/// archives never load into memory) into `destination`.
///
/// `cancel` is checked between entries; extraction stops with
/// [`ArtifactError::Cancelled`] when it fires. See
/// [`extract_zip_vvpp`] for the safety rules.
pub fn extract_zip_file(
    archive_path: &Path,
    destination: &Path,
    entrypoint: Option<&str>,
    unpack_limit: u64,
    cancel: Option<&CancellationToken>,
) -> Result<PathBuf> {
    let file = File::open(archive_path)?;
    extract_zip_reader(
        BufReader::new(file),
        destination,
        entrypoint,
        unpack_limit,
        cancel,
    )
}

fn extract_zip_reader<R: Read + Seek>(
    reader: R,
    destination: &Path,
    declared_entrypoint: Option<&str>,
    unpack_limit: u64,
    cancel: Option<&CancellationToken>,
) -> Result<PathBuf> {
    let reader = reader;
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| ArtifactError::UnsafeArchive(format!("invalid zip archive: {e}")))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(ArtifactError::UnsafeArchive(format!(
            "archive has {} entries (limit {MAX_ARCHIVE_ENTRIES})",
            archive.len()
        )));
    }

    let mut total: u64 = 0;
    let mut manifest_command: Option<String> = None;
    for index in 0..archive.len() {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err(ArtifactError::Cancelled);
        }
        let mut entry = archive
            .by_index(index)
            .map_err(|e| ArtifactError::UnsafeArchive(format!("unreadable archive entry: {e}")))?;
        let name = entry.name().to_string();
        let relative = safe_relative_path(&name)?;
        if entry.is_symlink() {
            return Err(ArtifactError::UnsafeArchive(format!(
                "archive entry '{name}' is a symlink"
            )));
        }
        // The size budget applies to every entry (manifest included) before
        // any content is read, so a lying size field cannot bypass the
        // unpack limit.
        let entry_size = entry.size();
        let Some(new_total) = entry_size.checked_add(total) else {
            return Err(ArtifactError::UnsafeArchive(format!(
                "archive entry '{name}' overflows the unpack budget"
            )));
        };
        if new_total > unpack_limit {
            return Err(ArtifactError::UnsafeArchive(format!(
                "archive expands beyond the {unpack_limit}-byte unpack limit"
            )));
        }
        total = new_total;

        if relative == Path::new("engine_manifest.json") {
            if entry_size > MAX_MANIFEST_BYTES {
                return Err(ArtifactError::UnsafeArchive(format!(
                    "engine_manifest.json exceeds the {MAX_MANIFEST_BYTES}-byte limit"
                )));
            }
            let mut manifest_bytes = Vec::new();
            entry
                .by_ref()
                .take(MAX_MANIFEST_BYTES)
                .read_to_end(&mut manifest_bytes)?;
            // The manifest is part of the payload: engines read it from
            // their own directory at runtime, so it is extracted like any
            // other file.
            let manifest_target = destination.join("engine_manifest.json");
            std::fs::write(&manifest_target, &manifest_bytes)?;
            let manifest: serde_json::Value =
                serde_json::from_slice(&manifest_bytes).map_err(|e| {
                    ArtifactError::UnsafeArchive(format!("invalid engine_manifest.json: {e}"))
                })?;
            manifest_command = manifest
                .get("command")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            if manifest_command
                .as_deref()
                .is_none_or(|command| command.trim().is_empty())
            {
                return Err(ArtifactError::UnsafeArchive(
                    "engine_manifest.json has no non-empty command".to_string(),
                ));
            }
            continue;
        }
        let is_dir = entry.is_dir();

        let target = destination.join(&relative);
        if is_dir {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(&target)?;
        // Chunked copy with cancellation checks: a multi-gigabyte entry is
        // copied in bounded chunks, and a payload that decompresses beyond
        // its declared size is rejected mid-copy instead of silently
        // overflowing the budget.
        let mut written: u64 = 0;
        let mut buffer = vec![0u8; COPY_CHUNK_BYTES];
        loop {
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                return Err(ArtifactError::Cancelled);
            }
            let read = entry.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            written = written.saturating_add(read as u64);
            if written > entry_size {
                return Err(ArtifactError::UnsafeArchive(format!(
                    "archive entry '{name}' decompresses beyond its declared \
                     {entry_size}-byte size"
                )));
            }
            file.write_all(&buffer[..read])?;
        }
    }

    // A zip-vvpp payload's entrypoint comes from engine_manifest.json's
    // `command`; a catalog-declared entrypoint must match it exactly (the
    // signature covers the declared value, so a mismatch means the payload
    // does not match the catalog).
    let entrypoint = if let Some(command) = manifest_command {
        let command_path = safe_relative_path(&command)?;
        if let Some(declared) = declared_entrypoint
            && declared != command_path.to_string_lossy()
        {
            return Err(ArtifactError::UnsafeArchive(format!(
                "archive entrypoint '{declared}' does not match engine_manifest.json command \
                 '{command}'"
            )));
        }
        command_path
    } else if let Some(declared) = declared_entrypoint {
        safe_relative_path(declared)?
    } else {
        return Err(ArtifactError::UnsafeArchive(
            "archive has no engine_manifest.json and no declared entrypoint".to_string(),
        ));
    };
    let path = destination.join(&entrypoint);
    if !path.is_file() {
        return Err(ArtifactError::UnsafeArchive(format!(
            "archive entrypoint '{}' not found or not a regular file",
            entrypoint.display()
        )));
    }
    set_executable(&path)?;
    Ok(path)
}

/// Rejects names that could escape `destination`: absolute paths, `..`
/// components, empty/`.`-only paths, and non-UTF-8 names (the archive's raw
/// name bytes are compared, not a lossy conversion).
fn safe_relative_path(name: &str) -> Result<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute()
        || name
            .split(['\\', '/'])
            .any(|part| part == ".." || part.is_empty() || part == "." || part.contains('\u{0}'))
    {
        return Err(ArtifactError::UnsafeArchive(format!(
            "archive entry '{name}' escapes the extraction root"
        )));
    }
    let mut components = path.components().peekable();
    if matches!(components.peek(), Some(Component::CurDir)) {
        components.next();
    }
    let relative: PathBuf = components.collect();
    if relative.as_os_str().is_empty() {
        return Err(ArtifactError::UnsafeArchive(
            "archive contains an empty entry name".to_string(),
        ));
    }
    Ok(relative)
}

/// Makes `path` executable (Unix). No-op on platforms without the concept.
#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(permissions.mode() | 0o111);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Whether a payload needs extraction before activation.
#[must_use]
pub fn needs_extraction(payload: &crate::catalog::ArtifactPayload) -> bool {
    payload.format == PayloadFormat::ZipVvpp
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        for (name, bytes) in entries {
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file(*name, options).expect("start file");
            writer.write_all(bytes).expect("write file");
        }
        writer.finish().expect("finish zip").into_inner()
    }

    #[test]
    fn extracts_flat_archive_and_entrypoint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = make_zip(&[
            ("engine_manifest.json", br#"{"command":"run.sh"}"#),
            ("run.sh", b"#!/bin/sh\necho hi\n"),
        ]);
        let entrypoint =
            extract_zip_vvpp(&bytes, dir.path(), Some("run.sh"), 1024 * 1024).expect("extract");
        assert_eq!(entrypoint, dir.path().join("run.sh"));
        assert!(dir.path().join("engine_manifest.json").is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&entrypoint)
                .expect("metadata")
                .permissions()
                .mode();
            assert_ne!(mode & 0o111, 0, "entrypoint must be executable");
        }
    }

    #[test]
    fn rejects_traversal_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = make_zip(&[("../escape", b"evil"), ("ok", b"fine")]);
        let err = extract_zip_vvpp(&bytes, dir.path(), None, 1024 * 1024)
            .expect_err("traversal rejected");
        assert!(err.to_string().contains("escapes"));
        assert!(!dir.path().parent().expect("parent").join("escape").exists());
    }

    #[test]
    fn rejects_absolute_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = make_zip(&[("/etc/passwd", b"evil")]);
        let err = extract_zip_vvpp(&bytes, dir.path(), None, 1024 * 1024)
            .expect_err("absolute path rejected");
        assert!(err.to_string().contains("escapes"));
    }

    #[test]
    fn rejects_missing_entrypoint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = make_zip(&[("run.sh", b"#!/bin/sh\n")]);
        let err = extract_zip_vvpp(&bytes, dir.path(), Some("missing.sh"), 1024 * 1024)
            .expect_err("missing entrypoint rejected");
        assert!(err.to_string().contains("not found"));
    }

    /// The manifest's `command` is the entrypoint when the catalog does not
    /// declare one.
    #[test]
    fn manifest_command_becomes_entrypoint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = make_zip(&[
            ("engine_manifest.json", br#"{"command":"run/engine.sh"}"#),
            ("run/engine.sh", b"#!/bin/sh\n"),
        ]);
        let entrypoint =
            extract_zip_vvpp(&bytes, dir.path(), None, 1024 * 1024).expect("manifest command used");
        assert_eq!(entrypoint, dir.path().join("run").join("engine.sh"));
    }

    /// A catalog-declared entrypoint must equal the manifest command; a
    /// mismatch means the payload does not match the catalog.
    #[test]
    fn declared_entrypoint_must_match_manifest_command() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = make_zip(&[
            ("engine_manifest.json", br#"{"command":"real.sh"}"#),
            ("real.sh", b"#!/bin/sh\n"),
            ("other.sh", b"#!/bin/sh\n"),
        ]);
        let err = extract_zip_vvpp(&bytes, dir.path(), Some("other.sh"), 1024 * 1024)
            .expect_err("mismatch rejected");
        assert!(
            err.to_string()
                .contains("does not match engine_manifest.json")
        );
    }

    /// A malformed or command-less manifest is rejected.
    #[test]
    fn manifest_without_command_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = make_zip(&[
            ("engine_manifest.json", br#"{"version":"0.15"}"#),
            ("run.sh", b"#!/bin/sh\n"),
        ]);
        let err = extract_zip_vvpp(&bytes, dir.path(), Some("run.sh"), 1024 * 1024)
            .expect_err("command-less manifest rejected");
        assert!(err.to_string().contains("command"));
    }

    /// The manifest read is bounded: a hostile archive cannot make the
    /// extractor hold a multi-gigabyte manifest in memory.
    #[test]
    fn oversized_manifest_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let huge = vec![b'x'; (MAX_MANIFEST_BYTES + 1) as usize];
        let bytes = make_zip(&[("engine_manifest.json", &huge)]);
        let err = extract_zip_vvpp(&bytes, dir.path(), None, 1024 * 1024 * 1024)
            .expect_err("oversized manifest rejected");
        assert!(err.to_string().contains("manifest"));
    }

    /// Symlink checks apply to the manifest entry too.
    #[test]
    fn symlink_manifest_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        writer
            .add_symlink(
                "engine_manifest.json",
                "elsewhere",
                zip::write::SimpleFileOptions::default(),
            )
            .expect("add symlink");
        let bytes = writer.finish().expect("finish zip").into_inner();
        let err = extract_zip_vvpp(&bytes, dir.path(), None, 1024 * 1024)
            .expect_err("symlink manifest rejected");
        assert!(err.to_string().contains("symlink"));
    }

    /// A cancelled token aborts extraction instead of writing the payload.
    #[test]
    fn cancelled_token_aborts_extraction() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = make_zip(&[
            ("engine_manifest.json", br#"{"command":"run.sh"}"#),
            ("run.sh", &[0u8; 4096]),
        ]);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = extract_zip_file(
            &write_temp_zip(&bytes),
            dir.path(),
            Some("run.sh"),
            1024 * 1024,
            Some(&cancel),
        )
        .expect_err("cancelled extraction aborts");
        assert!(matches!(err, ArtifactError::Cancelled));
    }

    fn write_temp_zip(bytes: &[u8]) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("ene-extract-test-{}.zip", std::process::id()));
        std::fs::write(&path, bytes).expect("write temp zip");
        path
    }

    #[test]
    fn rejects_expansion_beyond_unpack_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = make_zip(&[("big.bin", &[0u8; 2048]), ("run.sh", b"x")]);
        let err = extract_zip_vvpp(&bytes, dir.path(), Some("run.sh"), 1024)
            .expect_err("unpack limit enforced");
        assert!(err.to_string().contains("unpack limit"));
    }

    #[test]
    fn symlink_entries_are_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        writer
            .add_symlink("link", "target", zip::write::SimpleFileOptions::default())
            .expect("add symlink");
        let bytes = writer.finish().expect("finish zip").into_inner();
        let err =
            extract_zip_vvpp(&bytes, dir.path(), None, 1024 * 1024).expect_err("symlink rejected");
        assert!(err.to_string().contains("symlink"));
    }
}
