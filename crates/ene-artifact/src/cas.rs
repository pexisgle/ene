use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use sha2::{Digest, Sha256};

use crate::digest::validate_digest;
use crate::error::{ArtifactError, Result};

/// A verified object in the content-addressable store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CasEntry {
    /// Hex SHA-256 of the object.
    pub sha256: String,
    /// Exact byte size of the object.
    pub size: u64,
    /// Absolute path of the activated object.
    pub path: PathBuf,
}

/// SHA-256 content-addressable store.
///
/// Layout: `root/objects/{first-two-hex}/{remaining-hex}`. Objects are
/// written to a `.part` file under an exclusive per-digest lock, verified
/// (size + digest), fsynced, and atomically renamed into place, so a
/// concurrent or crashed writer can never activate a corrupt object.
#[derive(Debug, Clone)]
pub struct Cas {
    root: PathBuf,
}

impl Cas {
    /// Opens (creating if needed) a CAS rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(root.join("objects"))?;
        std::fs::create_dir_all(root.join(".locks"))?;
        Ok(Self { root })
    }

    /// The store root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether an object with this digest is present and valid.
    pub fn contains(&self, sha256: &str) -> Result<bool> {
        validate_digest(sha256)?;
        let path = self.object_path(sha256);
        if !path.is_file() {
            return Ok(false);
        }
        crate::digest::verify_sha256(&path, sha256)
    }

    /// Path of the object for `sha256` (may not exist yet).
    #[must_use]
    pub fn object_path(&self, sha256: &str) -> PathBuf {
        let prefix = sha256.get(..2).unwrap_or_default();
        let remainder = sha256.get(2..).unwrap_or_default();
        self.root.join("objects").join(prefix).join(remainder)
    }

    /// Opens the object for reading.
    pub fn open(&self, sha256: &str) -> Result<File> {
        validate_digest(sha256)?;
        let path = self.object_path(sha256);
        File::open(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ArtifactError::CasMissing(sha256.to_string()),
            _ => ArtifactError::Io(e),
        })
    }

    /// Stores a reader as a verified object.
    ///
    /// `max_bytes` caps the accepted object size (disk-exhaustion guard);
    /// larger input aborts the write and returns [`ArtifactError::SizeExceeded`].
    /// If the object already exists it is returned without re-reading.
    pub fn put(
        &self,
        mut reader: impl Read,
        expected_sha256: &str,
        expected_size: u64,
        max_bytes: u64,
    ) -> Result<CasEntry> {
        validate_digest(expected_sha256)?;
        if self.contains(expected_sha256)? {
            let path = self.object_path(expected_sha256);
            let size = std::fs::metadata(&path)?.len();
            return Ok(CasEntry {
                sha256: expected_sha256.to_string(),
                size,
                path,
            });
        }

        let lock_path = self
            .root
            .join(".locks")
            .join(format!("{expected_sha256}.lock"));
        let lock_file = File::create(&lock_path)?;
        lock_file.lock_exclusive()?;

        // Re-check under the lock: another writer may have completed the
        // object while we waited.
        let result = if self.contains(expected_sha256)? {
            let path = self.object_path(expected_sha256);
            let size = std::fs::metadata(&path)?.len();
            Ok(CasEntry {
                sha256: expected_sha256.to_string(),
                size,
                path,
            })
        } else {
            self.put_locked(&mut reader, expected_sha256, expected_size, max_bytes)
        };
        drop(lock_file.unlock());
        result
    }

    fn put_locked(
        &self,
        reader: &mut impl Read,
        expected_sha256: &str,
        expected_size: u64,
        max_bytes: u64,
    ) -> Result<CasEntry> {
        let final_path = self.object_path(expected_sha256);
        if let Some(parent) = final_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut temp = tempfile::NamedTempFile::new_in(
            final_path
                .parent()
                .ok_or_else(|| ArtifactError::InvalidDigest(expected_sha256.to_string()))?,
        )?;
        let mut hasher = Sha256::new();
        let mut written = 0_u64;
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            written = written.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
            if written > max_bytes {
                return Err(ArtifactError::SizeExceeded {
                    max: max_bytes,
                    got: written,
                });
            }
            hasher.update(&buffer[..read]);
            temp.write_all(&buffer[..read])?;
        }
        if written != expected_size {
            return Err(ArtifactError::SizeMismatch {
                artifact: expected_sha256.to_string(),
                expected: expected_size,
                actual: written,
            });
        }
        let actual_digest = hex::encode(hasher.finalize());
        if actual_digest != expected_sha256 {
            return Err(ArtifactError::DigestMismatch {
                artifact: expected_sha256.to_string(),
                expected: expected_sha256.to_string(),
                actual: actual_digest,
            });
        }
        temp.as_file().sync_all()?;
        temp.persist(&final_path)
            .map_err(|e| ArtifactError::Io(e.error))?;
        // Best-effort directory fsync so the rename survives a crash.
        if let Ok(dir) = File::open(
            final_path
                .parent()
                .ok_or_else(|| ArtifactError::InvalidDigest(expected_sha256.to_string()))?,
        ) {
            drop(dir.sync_all());
        }
        Ok(CasEntry {
            sha256: expected_sha256.to_string(),
            size: written,
            path: final_path,
        })
    }

    /// Removes an object (used by GC and rollback).
    pub fn remove(&self, sha256: &str) -> Result<()> {
        validate_digest(sha256)?;
        let path = self.object_path(sha256);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(ArtifactError::Io(e)),
        }
    }

    /// Deletes every object whose digest is not in `keep`, returning the
    /// number of removed objects.
    pub fn gc(&self, keep: &std::collections::HashSet<String>) -> Result<usize> {
        let mut removed = 0_usize;
        let objects = self.root.join("objects");
        for prefix in std::fs::read_dir(&objects)? {
            let prefix = prefix?;
            if !prefix.path().is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(prefix.path())? {
                let entry = entry?;
                let digest = format!(
                    "{}{}",
                    prefix.file_name().to_string_lossy(),
                    entry.file_name().to_string_lossy()
                );
                if !keep.contains(&digest) {
                    std::fs::remove_file(entry.path())?;
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }
}

/// Convenience: hashes a byte slice and stores it via [`Cas::put`].
#[cfg(test)]
pub(crate) fn put_bytes(cas: &Cas, data: &[u8]) -> Result<CasEntry> {
    let digest = crate::digest::sha256_hex(data);
    cas.put(
        std::io::Cursor::new(data),
        &digest,
        data.len() as u64,
        1024 * 1024,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::sha256_hex;

    #[test]
    fn put_verifies_digest_and_size() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cas = Cas::new(dir.path()).expect("cas");
        let data = b"hello artifact";
        let digest = sha256_hex(data);
        let entry = cas
            .put(std::io::Cursor::new(data), &digest, data.len() as u64, 1024)
            .expect("put");
        assert_eq!(entry.sha256, digest);
        assert_eq!(entry.size, data.len() as u64);
        assert_eq!(std::fs::read(&entry.path).expect("read"), data);
        assert!(cas.contains(&digest).expect("contains"));
        assert_eq!(
            cas.open(&digest)
                .expect("open")
                .metadata()
                .expect("meta")
                .len(),
            data.len() as u64
        );
    }

    #[test]
    fn put_rejects_digest_mismatch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cas = Cas::new(dir.path()).expect("cas");
        let err = cas
            .put(std::io::Cursor::new(b"data"), &"0".repeat(64), 4, 1024)
            .expect_err("mismatch must fail");
        assert!(matches!(err, ArtifactError::DigestMismatch { .. }));
    }

    #[test]
    fn put_rejects_size_mismatch_and_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cas = Cas::new(dir.path()).expect("cas");
        let data = b"0123456789";
        let digest = sha256_hex(data);
        let err = cas
            .put(std::io::Cursor::new(data), &digest, 99, 1024)
            .expect_err("size mismatch must fail");
        assert!(matches!(err, ArtifactError::SizeMismatch { .. }));
        let err = cas
            .put(std::io::Cursor::new(data), &digest, data.len() as u64, 5)
            .expect_err("cap must fail");
        assert!(matches!(err, ArtifactError::SizeExceeded { .. }));
    }

    #[test]
    fn put_deduplicates_and_gc_removes_unreferenced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cas = Cas::new(dir.path()).expect("cas");
        let a = put_bytes(&cas, b"alpha").expect("put alpha");
        let b = put_bytes(&cas, b"beta").expect("put beta");
        let a2 = put_bytes(&cas, b"alpha").expect("put alpha again");
        assert_eq!(a.path, a2.path);

        let keep = std::collections::HashSet::from([a.sha256.clone()]);
        let removed = cas.gc(&keep).expect("gc");
        assert_eq!(removed, 1);
        assert!(!cas.contains(&b.sha256).expect("contains beta"));
        assert!(cas.contains(&a.sha256).expect("contains alpha"));
    }
}
