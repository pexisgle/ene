use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use rusqlite::Connection;
use rusqlite::backup::Backup;
use serde_json::json;

use super::error::{ApiReject, bad_request, not_found};

const MAX_IMPORT_BYTES: u64 = 32 * 1024 * 1024;
const MANIFEST_VERSION: u32 = 1;
const SIDECAR_FILES: [&str; 5] = [
    "vault.bin",
    "vault.key",
    "settings.json",
    "mcp.json",
    "policy.json",
];

/// Copy live stores into `<data>/backups/<ts>/` via `SQLite` online backup.
pub fn backup_now(data_dir: &Path, skills_max_bytes: u64) -> Result<(String, PathBuf), ApiReject> {
    let id = Utc::now().format("%Y%m%dT%H%M%S").to_string();
    let dest = data_dir.join("backups").join(&id);
    fs::create_dir_all(&dest).map_err(|err| io_err(&err))?;
    let mut files = Vec::new();
    copy_sqlite_named(
        &data_dir.join("sessions.db"),
        &dest.join("sessions.db"),
        "sessions.db",
        &mut files,
    )?;
    copy_sqlite_named(
        &data_dir.join("companions.db"),
        &dest.join("companions.db"),
        "companions.db",
        &mut files,
    )?;
    copy_sqlite_named(
        &data_dir.join("audit.db"),
        &dest.join("audit.db"),
        "audit.db",
        &mut files,
    )?;
    copy_sidecars(data_dir, &dest, skills_max_bytes, &mut files)?;
    write_manifest(&dest, &files)?;
    Ok((id, dest))
}

/// Validate a backup generation id before restore.
pub fn validate_restore_id(id: &str) -> Result<(), ApiReject> {
    if id.is_empty() || id.contains('/') || id.contains("..") || id == "pre-restore" {
        return Err(bad_request("invalid_message", "invalid backup id"));
    }
    if !id.bytes().all(|byte| byte.is_ascii_digit() || byte == b'T') {
        return Err(bad_request("invalid_message", "invalid backup id"));
    }
    Ok(())
}

pub fn checkpoint_db(path: &Path) -> Result<(), ApiReject> {
    if !path.exists() {
        return Ok(());
    }
    let conn = Connection::open(path).map_err(|err| io_err(&std::io::Error::other(err)))?;
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|err| io_err(&std::io::Error::other(err)))?;
    Ok(())
}

/// Copy a validated backup generation over the live data dir (callers must close writers first).
pub fn restore_copy(data_dir: &Path, id: &str, skills_max_bytes: u64) -> Result<(), ApiReject> {
    validate_restore_id(id)?;
    let src = data_dir.join("backups").join(id);
    if !src.is_dir() {
        return Err(not_found("backup not found"));
    }
    let pre = data_dir.join("backups").join("pre-restore");
    if pre.exists() {
        fs::remove_dir_all(&pre).map_err(|err| io_err(&err))?;
    }
    fs::create_dir_all(&pre).map_err(|err| io_err(&err))?;
    let mut unused = Vec::new();
    copy_sqlite(&data_dir.join("sessions.db"), &pre.join("sessions.db"))?;
    copy_sqlite(&data_dir.join("companions.db"), &pre.join("companions.db"))?;
    copy_sqlite(&data_dir.join("audit.db"), &pre.join("audit.db"))?;
    copy_sidecars(data_dir, &pre, skills_max_bytes, &mut unused)?;
    copy_sqlite(&src.join("sessions.db"), &data_dir.join("sessions.db"))?;
    copy_sqlite(&src.join("companions.db"), &data_dir.join("companions.db"))?;
    copy_sqlite(&src.join("audit.db"), &data_dir.join("audit.db"))?;
    replace_sidecars(&src, data_dir, skills_max_bytes)?;
    Ok(())
}

/// Restore a backup generation into the live data dir (standalone; tests only).
#[cfg(test)]
pub fn restore_now(data_dir: &Path, id: &str, skills_max_bytes: u64) -> Result<(), ApiReject> {
    checkpoint_db(&data_dir.join("sessions.db"))?;
    checkpoint_db(&data_dir.join("companions.db"))?;
    checkpoint_db(&data_dir.join("audit.db"))?;
    restore_copy(data_dir, id, skills_max_bytes)
}

fn copy_sidecars(
    src: &Path,
    dst: &Path,
    skills_max_bytes: u64,
    files: &mut Vec<String>,
) -> Result<(), ApiReject> {
    for name in SIDECAR_FILES {
        copy_if_exists_named(&src.join(name), &dst.join(name), name, files)?;
    }
    copy_skills_dir(
        &src.join("skills"),
        &dst.join("skills"),
        skills_max_bytes,
        files,
    )
}

fn replace_sidecars(src: &Path, dst: &Path, skills_max_bytes: u64) -> Result<(), ApiReject> {
    let mut files = Vec::new();
    for name in SIDECAR_FILES {
        replace_optional_file(&src.join(name), &dst.join(name))?;
    }
    let skills_dst = dst.join("skills");
    if skills_dst.exists() {
        fs::remove_dir_all(&skills_dst).map_err(|err| io_err(&err))?;
    }
    copy_skills_dir(
        &src.join("skills"),
        &skills_dst,
        skills_max_bytes,
        &mut files,
    )
}

fn replace_optional_file(src: &Path, dst: &Path) -> Result<(), ApiReject> {
    if dst.exists() {
        fs::remove_file(dst).map_err(|err| io_err(&err))?;
    }
    copy_if_exists(src, dst)
}

fn copy_sqlite_named(
    src: &Path,
    dst: &Path,
    name: &str,
    files: &mut Vec<String>,
) -> Result<(), ApiReject> {
    copy_sqlite(src, dst)?;
    if src.exists() {
        files.push(name.to_owned());
    }
    Ok(())
}

fn copy_sqlite(src: &Path, dst: &Path) -> Result<(), ApiReject> {
    if !src.exists() {
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|err| io_err(&err))?;
    }
    let source = Connection::open(src).map_err(|err| io_err(&std::io::Error::other(err)))?;
    let mut dest_conn = Connection::open(dst).map_err(|err| io_err(&std::io::Error::other(err)))?;
    let backup =
        Backup::new(&source, &mut dest_conn).map_err(|err| io_err(&std::io::Error::other(err)))?;
    backup
        .run_to_completion(128, Duration::from_millis(5), None)
        .map_err(|err| io_err(&std::io::Error::other(err)))?;
    Ok(())
}

fn copy_if_exists_named(
    src: &Path,
    dst: &Path,
    name: &str,
    files: &mut Vec<String>,
) -> Result<(), ApiReject> {
    if src.exists() {
        copy_if_exists(src, dst)?;
        files.push(name.to_owned());
    }
    Ok(())
}

fn copy_if_exists(src: &Path, dst: &Path) -> Result<(), ApiReject> {
    if src.exists() {
        let meta = fs::metadata(src).map_err(|err| io_err(&err))?;
        if meta.len() > MAX_IMPORT_BYTES {
            return Err(bad_request("invalid_message", "file too large"));
        }
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(|err| io_err(&err))?;
        }
        fs::copy(src, dst).map_err(|err| io_err(&err))?;
    }
    Ok(())
}

fn copy_skills_dir(
    src: &Path,
    dst: &Path,
    skills_max_bytes: u64,
    files: &mut Vec<String>,
) -> Result<(), ApiReject> {
    if !src.is_dir() {
        return Ok(());
    }
    let size = dir_size(src).map_err(|err| io_err(&err))?;
    if size > skills_max_bytes {
        return Err(bad_request(
            "invalid_message",
            "skills directory exceeds backup size limit",
        ));
    }
    copy_dir(src, dst)?;
    files.push("skills".to_owned());
    Ok(())
}

fn dir_size(path: &Path) -> std::io::Result<u64> {
    let mut total = 0_u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            total = total.saturating_add(dir_size(&entry.path())?);
        } else {
            total = total.saturating_add(entry.metadata()?.len());
        }
    }
    Ok(total)
}

fn copy_dir(src: &Path, dst: &Path) -> Result<(), ApiReject> {
    fs::create_dir_all(dst).map_err(|err| io_err(&err))?;
    for entry in fs::read_dir(src).map_err(|err| io_err(&err))? {
        let entry = entry.map_err(|err| io_err(&err))?;
        let ft = entry.file_type().map_err(|err| io_err(&err))?;
        if ft.is_symlink() {
            continue;
        }
        let to = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), to).map_err(|err| io_err(&err))?;
        }
    }
    Ok(())
}

fn write_manifest(dest: &Path, files: &[String]) -> Result<(), ApiReject> {
    let body = serde_json::to_string_pretty(&json!({
        "version": MANIFEST_VERSION,
        "created_at": Utc::now().to_rfc3339(),
        "files": files,
    }))
    .map_err(|err| io_err(&std::io::Error::other(err)))?;
    fs::write(dest.join("manifest.json"), body).map_err(|err| io_err(&err))
}

fn io_err(err: &std::io::Error) -> ApiReject {
    super::error::ApiReject::new(axum::http::StatusCode::INTERNAL_SERVER_ERROR, "fault", "io")
        .with_detail(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::{backup_now, restore_now, validate_restore_id};
    use rusqlite::Connection;
    use tempfile::TempDir;

    const SKILLS_CAP: u64 = 100 * 1024 * 1024;

    fn seed(path: &std::path::Path, marker: i64) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE t(id INTEGER PRIMARY KEY); INSERT INTO t(id) VALUES ({marker});"
        ))
        .unwrap();
    }

    fn marker(path: &std::path::Path) -> i64 {
        let conn = Connection::open(path).unwrap();
        conn.query_row("SELECT id FROM t", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn backup_and_restore_roundtrip() {
        let dir = TempDir::new().unwrap();
        seed(&dir.path().join("sessions.db"), 11);
        seed(&dir.path().join("companions.db"), 22);
        seed(&dir.path().join("audit.db"), 33);
        std::fs::write(dir.path().join("vault.key"), "key-bytes").unwrap();
        std::fs::write(dir.path().join("mcp.json"), "{\"servers\":[]}").unwrap();
        std::fs::write(dir.path().join("policy.json"), "{\"rules\":[]}").unwrap();
        std::fs::create_dir_all(dir.path().join("skills")).unwrap();
        std::fs::write(dir.path().join("skills").join("note.md"), "hello skill").unwrap();
        let (id, dest) = backup_now(dir.path(), SKILLS_CAP).unwrap();
        assert!(dest.join("sessions.db").exists());
        assert!(dest.join("vault.key").exists());
        assert!(dest.join("mcp.json").exists());
        assert!(dest.join("policy.json").exists());
        assert!(dest.join("skills").join("note.md").exists());
        let manifest = std::fs::read_to_string(dest.join("manifest.json")).unwrap();
        assert!(manifest.contains("\"version\": 1"));
        assert!(manifest.contains("mcp.json"));
        assert!(manifest.contains("skills"));
        std::fs::remove_file(dir.path().join("sessions.db")).unwrap();
        seed(&dir.path().join("sessions.db"), 99);
        std::fs::write(dir.path().join("mcp.json"), "{\"servers\":[1]}").unwrap();
        std::fs::write(dir.path().join("policy.json"), "{\"rules\":[1]}").unwrap();
        std::fs::write(dir.path().join("skills").join("note.md"), "changed").unwrap();
        std::fs::write(dir.path().join("skills").join("extra.md"), "leftover").unwrap();
        assert_eq!(marker(&dir.path().join("sessions.db")), 99);
        restore_now(dir.path(), &id, SKILLS_CAP).unwrap();
        assert_eq!(marker(&dir.path().join("sessions.db")), 11);
        assert_eq!(marker(&dir.path().join("companions.db")), 22);
        assert_eq!(marker(&dir.path().join("audit.db")), 33);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("mcp.json")).unwrap(),
            "{\"servers\":[]}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("policy.json")).unwrap(),
            "{\"rules\":[]}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("skills").join("note.md")).unwrap(),
            "hello skill"
        );
        assert!(!dir.path().join("skills").join("extra.md").exists());
        assert!(dir.path().join("backups").join("pre-restore").is_dir());
    }

    #[test]
    fn restore_removes_sidecars_absent_from_snapshot() {
        let dir = TempDir::new().unwrap();
        seed(&dir.path().join("sessions.db"), 1);
        seed(&dir.path().join("companions.db"), 2);
        seed(&dir.path().join("audit.db"), 3);
        let (id, _) = backup_now(dir.path(), SKILLS_CAP).unwrap();

        std::fs::write(dir.path().join("mcp.json"), "newer mcp").unwrap();
        std::fs::write(dir.path().join("policy.json"), "newer policy").unwrap();
        std::fs::write(dir.path().join("settings.json"), "newer settings").unwrap();
        std::fs::write(dir.path().join("vault.key"), "newer key").unwrap();

        restore_now(dir.path(), &id, SKILLS_CAP).unwrap();

        assert!(!dir.path().join("mcp.json").exists());
        assert!(!dir.path().join("policy.json").exists());
        assert!(!dir.path().join("settings.json").exists());
        assert!(!dir.path().join("vault.key").exists());
    }

    #[test]
    fn skills_over_cap_fails_backup() {
        let dir = TempDir::new().unwrap();
        seed(&dir.path().join("sessions.db"), 1);
        seed(&dir.path().join("companions.db"), 2);
        seed(&dir.path().join("audit.db"), 3);
        std::fs::create_dir_all(dir.path().join("skills")).unwrap();
        std::fs::write(dir.path().join("skills").join("big.bin"), vec![0_u8; 64]).unwrap();
        let err = backup_now(dir.path(), 16).unwrap_err();
        assert_eq!(err.0.error_class, "invalid_message");
        assert!(err.0.title.contains("skills"), "{}", err.0.title);
    }

    #[test]
    fn restore_id_rejects_traversal_and_reserved() {
        assert!(validate_restore_id("pre-restore").is_err());
        assert!(validate_restore_id("../etc").is_err());
        assert!(validate_restore_id("20240101T120000").is_ok());
    }
}
