//! File-level SQLite backup, restore, and integrity helpers (#239).
//!
//! Migration recovery uses **pre-migration file copies** rather than
//! `Migrator::down()`. SQLite + SeaORM cannot safely roll a half-applied
//! schema change back in-process; restoring a known-good file is the
//! durable recovery path.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

use crate::error::MemoryError;

/// Prefix inserted before the timestamp in backup filenames.
///
/// Given `memory.db`, backups look like `memory.db.bak.1710000000`.
pub const BACKUP_SUFFIX: &str = ".bak.";

/// Options controlling open-time backup / integrity behaviour (#239).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenOptions {
    /// Create a file backup before applying pending migrations.
    pub backup_on_migrate: bool,
    /// Maximum number of `{db}.bak.*` files to keep.
    pub max_backups: usize,
    /// Run `PRAGMA integrity_check` after connecting.
    pub integrity_check_on_open: bool,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            backup_on_migrate: true,
            max_backups: 5,
            integrity_check_on_open: false,
        }
    }
}

impl From<&crate::StoreConfig> for OpenOptions {
    fn from(cfg: &crate::StoreConfig) -> Self {
        Self {
            backup_on_migrate: cfg.backup_on_migrate,
            max_backups: cfg.max_backups.max(1),
            integrity_check_on_open: cfg.integrity_check_on_open,
        }
    }
}

/// Build the backup path for `db_path` using the current unix timestamp.
#[must_use]
pub fn backup_path_for(db_path: &Path) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut name = db_path
        .file_name()
        .map_or_else(|| "memory.db".into(), std::ffi::OsStr::to_os_string);
    name.push(BACKUP_SUFFIX);
    name.push(ts.to_string());
    match db_path.parent() {
        Some(parent) => parent.join(name),
        None => PathBuf::from(name),
    }
}

/// Return whether `path` looks like a backup of `db_path`.
#[must_use]
pub fn is_backup_of(db_path: &Path, path: &Path) -> bool {
    let Some(db_name) = db_path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    let prefix = format!("{db_name}{BACKUP_SUFFIX}");
    let Some(rest) = name.strip_prefix(&prefix) else {
        return false;
    };
    // Accept `{ts}` or `{ts}.{pid}` where both parts are digits.
    if rest.is_empty() {
        return false;
    }
    rest.split('.')
        .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// List backup files for `db_path`, newest first.
pub fn list_backups(db_path: &Path) -> Result<Vec<PathBuf>, MemoryError> {
    let parent = db_path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        return Ok(Vec::new());
    }
    let mut backups = Vec::new();
    for entry in fs::read_dir(parent).map_err(|e| {
        MemoryError::BackupError(format!("failed to list {}: {e}", parent.display()))
    })? {
        let entry = entry.map_err(|e| MemoryError::BackupError(format!("readdir: {e}")))?;
        let path = entry.path();
        if path.is_file() && is_backup_of(db_path, &path) {
            backups.push(path);
        }
    }
    backups.sort_by(|a, b| b.cmp(a));
    Ok(backups)
}

/// Delete oldest backups until at most `max_backups` remain.
pub fn prune_backups(db_path: &Path, max_backups: usize) -> Result<(), MemoryError> {
    let max = max_backups.max(1);
    let backups = list_backups(db_path)?;
    for stale in backups.into_iter().skip(max) {
        fs::remove_file(&stale).map_err(|e| {
            MemoryError::BackupError(format!("failed to remove {}: {e}", stale.display()))
        })?;
    }
    Ok(())
}

/// Checkpoint WAL and copy `db_path` to a new timestamped backup file.
///
/// When `conn` is `Some`, runs `PRAGMA wal_checkpoint(TRUNCATE)` first so the
/// backup captures committed pages. Returns the backup path.
pub async fn backup_database(
    db_path: &Path,
    conn: Option<&DatabaseConnection>,
) -> Result<PathBuf, MemoryError> {
    if !db_path.exists() {
        return Err(MemoryError::BackupError(format!(
            "database does not exist: {}",
            db_path.display()
        )));
    }
    if let Some(db) = conn {
        checkpoint_wal(db).await?;
    }
    let dest = unique_backup_path(db_path);
    fs::copy(db_path, &dest).map_err(|e| {
        MemoryError::BackupError(format!(
            "failed to copy {} -> {}: {e}",
            db_path.display(),
            dest.display()
        ))
    })?;
    // Also copy WAL/SHM if present (should be empty after checkpoint, but
    // keep them for completeness when no connection was provided).
    copy_sidecar(db_path, &dest, "-wal")?;
    copy_sidecar(db_path, &dest, "-shm")?;
    Ok(dest)
}

fn unique_backup_path(db_path: &Path) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let pid = std::process::id();
    let mut name = db_path
        .file_name()
        .map_or_else(|| "memory.db".into(), std::ffi::OsStr::to_os_string);
    name.push(BACKUP_SUFFIX);
    name.push(format!("{ts}.{pid}"));
    match db_path.parent() {
        Some(parent) => parent.join(name),
        None => PathBuf::from(name),
    }
}

fn copy_sidecar(db_path: &Path, backup: &Path, suffix: &str) -> Result<(), MemoryError> {
    let mut src_name = db_path.as_os_str().to_os_string();
    src_name.push(suffix);
    let src = PathBuf::from(src_name);
    if !src.exists() {
        return Ok(());
    }
    let mut dest_name = backup.as_os_str().to_os_string();
    dest_name.push(suffix);
    let dest = PathBuf::from(dest_name);
    fs::copy(&src, &dest).map_err(|e| {
        MemoryError::BackupError(format!(
            "failed to copy sidecar {} -> {}: {e}",
            src.display(),
            dest.display()
        ))
    })?;
    Ok(())
}

/// Replace `db_path` with the contents of `backup_path`.
///
/// Removes leftover `-wal` / `-shm` sidecars so the restored file opens cleanly.
pub fn restore_database(backup_path: &Path, db_path: &Path) -> Result<(), MemoryError> {
    if !backup_path.exists() {
        return Err(MemoryError::BackupError(format!(
            "backup does not exist: {}",
            backup_path.display()
        )));
    }
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            MemoryError::BackupError(format!("failed to create {}: {e}", parent.display()))
        })?;
    }
    remove_sidecar(db_path, "-wal")?;
    remove_sidecar(db_path, "-shm")?;
    fs::copy(backup_path, db_path).map_err(|e| {
        MemoryError::BackupError(format!(
            "failed to restore {} -> {}: {e}",
            backup_path.display(),
            db_path.display()
        ))
    })?;
    Ok(())
}

fn remove_sidecar(db_path: &Path, suffix: &str) -> Result<(), MemoryError> {
    let mut name = db_path.as_os_str().to_os_string();
    name.push(suffix);
    let path = PathBuf::from(name);
    if path.exists() {
        fs::remove_file(&path).map_err(|e| {
            MemoryError::BackupError(format!("failed to remove {}: {e}", path.display()))
        })?;
    }
    Ok(())
}

/// Run `PRAGMA wal_checkpoint(TRUNCATE)` so the main DB file is self-contained.
pub async fn checkpoint_wal(db: &DatabaseConnection) -> Result<(), MemoryError> {
    db.execute_unprepared("PRAGMA wal_checkpoint(TRUNCATE)")
        .await
        .map_err(|e| MemoryError::BackupError(format!("wal_checkpoint failed: {e}")))?;
    Ok(())
}

/// Run `PRAGMA integrity_check` and return Ok when the result is `ok`.
pub async fn check_integrity(db: &DatabaseConnection) -> Result<(), MemoryError> {
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA integrity_check".to_string(),
        ))
        .await
        .map_err(|e| MemoryError::IntegrityCheckFailed(format!("query failed: {e}")))?;
    let mut messages = Vec::new();
    for row in rows {
        let msg: String = row
            .try_get_by_index(0)
            .map_err(|e| MemoryError::IntegrityCheckFailed(format!("decode failed: {e}")))?;
        messages.push(msg);
    }
    if messages.len() == 1
        && messages
            .first()
            .is_some_and(|m| m.eq_ignore_ascii_case("ok"))
    {
        return Ok(());
    }
    Err(MemoryError::IntegrityCheckFailed(messages.join("; ")))
}

#[cfg(test)]
mod tests {
    #![expect(clippy::expect_used, reason = "unit tests use expect for assertions")]

    use super::*;

    #[test]
    fn is_backup_of_matches_timestamped_names() {
        let db = Path::new("/tmp/memory.db");
        assert!(is_backup_of(db, Path::new("/tmp/memory.db.bak.1710000000")));
        assert!(!is_backup_of(db, Path::new("/tmp/memory.db.bak.")));
        assert!(!is_backup_of(db, Path::new("/tmp/memory.db.bak.abc")));
        assert!(!is_backup_of(db, Path::new("/tmp/other.db.bak.1710000000")));
    }

    #[test]
    fn prune_backups_keeps_newest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("memory.db");
        fs::write(&db, b"db").expect("write db");
        for ts in ["100", "200", "300"] {
            fs::write(dir.path().join(format!("memory.db.bak.{ts}")), b"bak").expect("write bak");
        }
        prune_backups(&db, 2).expect("prune");
        let left = list_backups(&db).expect("list");
        assert_eq!(left.len(), 2);
        assert!(
            left.first()
                .is_some_and(|p| p.ends_with("memory.db.bak.300"))
        );
        assert!(
            left.get(1)
                .is_some_and(|p| p.ends_with("memory.db.bak.200"))
        );
    }
}
