use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use rusqlite::Connection;
use rusqlite::backup::Backup;

use super::error::{ApiReject, bad_request, not_found};

/// Copy live stores into `<data>/backups/<ts>/` via `SQLite` online backup.
pub fn backup_now(data_dir: &Path) -> Result<(String, PathBuf), ApiReject> {
    let id = Utc::now().format("%Y%m%dT%H%M%S").to_string();
    let dest = data_dir.join("backups").join(&id);
    fs::create_dir_all(&dest).map_err(|err| io_err(&err))?;
    copy_sqlite(&data_dir.join("sessions.db"), &dest.join("sessions.db"))?;
    copy_sqlite(&data_dir.join("companions.db"), &dest.join("companions.db"))?;
    copy_sqlite(&data_dir.join("audit.db"), &dest.join("audit.db"))?;
    copy_if_exists(&data_dir.join("vault.bin"), &dest.join("vault.bin"))?;
    copy_if_exists(&data_dir.join("settings.json"), &dest.join("settings.json"))?;
    Ok((id, dest))
}

/// Restore a backup generation into the live data dir.
pub fn restore_now(data_dir: &Path, id: &str) -> Result<(), ApiReject> {
    if id.is_empty() || id.contains('/') || id.contains("..") {
        return Err(bad_request("invalid_message", "invalid backup id"));
    }
    let src = data_dir.join("backups").join(id);
    if !src.is_dir() {
        return Err(not_found("backup not found"));
    }
    let pre = data_dir.join("backups").join("pre-restore");
    fs::create_dir_all(&pre).map_err(|err| io_err(&err))?;
    copy_sqlite(&data_dir.join("sessions.db"), &pre.join("sessions.db"))?;
    copy_sqlite(&data_dir.join("companions.db"), &pre.join("companions.db"))?;
    copy_sqlite(&data_dir.join("audit.db"), &pre.join("audit.db"))?;
    copy_sqlite(&src.join("sessions.db"), &data_dir.join("sessions.db"))?;
    copy_sqlite(&src.join("companions.db"), &data_dir.join("companions.db"))?;
    copy_sqlite(&src.join("audit.db"), &data_dir.join("audit.db"))?;
    copy_if_exists(&src.join("vault.bin"), &data_dir.join("vault.bin"))?;
    copy_if_exists(&src.join("settings.json"), &data_dir.join("settings.json"))?;
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

fn copy_if_exists(src: &Path, dst: &Path) -> Result<(), ApiReject> {
    if src.exists() {
        fs::copy(src, dst).map_err(|err| io_err(&err))?;
    }
    Ok(())
}

fn io_err(err: &std::io::Error) -> ApiReject {
    super::error::ApiReject::new(axum::http::StatusCode::INTERNAL_SERVER_ERROR, "fault", "io")
        .with_detail(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::{backup_now, restore_now};
    use rusqlite::Connection;
    use tempfile::TempDir;

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
        let (id, dest) = backup_now(dir.path()).unwrap();
        assert!(dest.join("sessions.db").exists());
        std::fs::remove_file(dir.path().join("sessions.db")).unwrap();
        seed(&dir.path().join("sessions.db"), 99);
        assert_eq!(marker(&dir.path().join("sessions.db")), 99);
        restore_now(dir.path(), &id).unwrap();
        assert_eq!(marker(&dir.path().join("sessions.db")), 11);
        assert_eq!(marker(&dir.path().join("companions.db")), 22);
        assert_eq!(marker(&dir.path().join("audit.db")), 33);
        assert!(dir.path().join("backups").join("pre-restore").is_dir());
    }
}
