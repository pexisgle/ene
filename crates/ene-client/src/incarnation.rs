use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use ene_api::v1::refs::ClientIncarnationId;

use crate::error::ClientError;

pub const COUNTER_FILE_NAME: &str = "client-incarnation.counter";
pub const LOCK_FILE_NAME: &str = "client-incarnation.lock";

fn cache() -> &'static Mutex<HashMap<PathBuf, ClientIncarnationId>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, ClientIncarnationId>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[must_use]
pub fn counter_path(data_dir: &Path) -> PathBuf {
    data_dir.join(COUNTER_FILE_NAME)
}

#[must_use]
pub fn lock_path(data_dir: &Path) -> PathBuf {
    data_dir.join(LOCK_FILE_NAME)
}

pub fn boot_incarnation(data_dir: &Path) -> Result<ClientIncarnationId, ClientError> {
    let key = data_dir.to_path_buf();
    let mut cached = cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(incarnation) = cached.get(&key) {
        return Ok(*incarnation);
    }
    let counter = advance_counter(data_dir)?;
    let incarnation = ClientIncarnationId {
        counter,
        // Masked to the non-negative SQLite INTEGER range because history
        // rows store both halves as INTEGER and fail a full-range u64 closed
        // on encode; every other consumer only equality-checks.
        random: (uuid::Uuid::new_v4().as_u128() & i64::MAX as u128) as u64,
    };
    cached.insert(key, incarnation);
    Ok(incarnation)
}

pub fn advance_counter(data_dir: &Path) -> Result<u64, ClientError> {
    ensure_data_dir(data_dir)?;
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path(data_dir))
        .map_err(|error| {
            ClientError::Transport(format!("client incarnation lock failed: {}", error.kind()))
        })?;
    lock_file.lock().map_err(|error| {
        ClientError::Transport(format!("client incarnation lock failed: {}", error.kind()))
    })?;
    let current = read_counter(data_dir)?;
    let next = current.checked_add(1).ok_or_else(|| {
        ClientError::Transport(String::from("client incarnation counter exhausted"))
    })?;
    crate::device::atomic_replace(
        &counter_path(data_dir),
        next.to_string().as_bytes(),
        None,
        "client incarnation counter store failed",
    )?;
    // The OS lock releases when `lock_file` drops.
    Ok(next)
}

#[cfg(test)]
pub fn reset_for_tests() {
    cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

fn ensure_data_dir(data_dir: &Path) -> Result<(), ClientError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(data_dir)
            .map_err(|error| {
                ClientError::Transport(format!("client incarnation dir failed: {}", error.kind()))
            })?;
        let mode = std::fs::metadata(data_dir)
            .map_err(|error| {
                ClientError::Transport(format!("client incarnation dir failed: {}", error.kind()))
            })?
            .permissions()
            .mode()
            & 0o777;
        if mode & 0o077 != 0 {
            std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(0o700)).map_err(
                |error| {
                    ClientError::Transport(format!(
                        "client incarnation dir failed: {}",
                        error.kind()
                    ))
                },
            )?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(data_dir).map_err(|error| {
            ClientError::Transport(format!("client incarnation dir failed: {}", error.kind()))
        })
    }
}

fn read_counter(data_dir: &Path) -> Result<u64, ClientError> {
    let bytes = match std::fs::read(counter_path(data_dir)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(ClientError::Transport(format!(
                "client incarnation counter unreadable: {}",
                error.kind()
            )));
        }
    };
    let text = core::str::from_utf8(&bytes)
        .map_err(|_| ClientError::Transport(String::from("client incarnation counter corrupt")))?;
    text.trim()
        .parse::<u64>()
        .map_err(|_| ClientError::Transport(String::from("client incarnation counter corrupt")))
}
