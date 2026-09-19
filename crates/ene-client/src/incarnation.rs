//! Client boot incarnation (#1387): one per-process identity from a durable counter.
//!
//! At process boot the client exclusively advances a per-data-directory
//! persistent counter and combines it with fresh randomness into a single
//! [`ClientIncarnationId`] that is
//! reused for every connection in the process. A same-process reconnect never
//! advances the counter; only a new process (after `reset_for_tests` in
//! tests, a real restart in production) boots again.
//!
//! Layout (alongside [`crate::device::DEVICE_FILE_NAME`]): `counter` holds one
//! decimal `u64` (absent means `0`, the first published value is `1`);
//! `lock` is the stable separate lock file whose OS exclusivity serializes
//! concurrent boots. The update is read → checked increment → stage to a temp
//! file in the same directory + sync + atomic rename, then publish. An
//! existing-but-unreadable or corrupt counter fails closed, as does any other
//! failure: connection start aborts with no PID/time fallback. Only a full
//! connection-metadata wipe removes the counter. Host matching stays
//! current-slot plus the authenticated pair (no high-water record).

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use ene_api::v1::refs::ClientIncarnationId;

use crate::error::ClientError;

pub const COUNTER_FILE_NAME: &str = "client-incarnation.counter";
pub const LOCK_FILE_NAME: &str = "client-incarnation.lock";

/// Per-process staging counter so concurrent renames in this process never
/// collide on the temp name.
static STAGE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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

/// Boots (or reuses) the process incarnation for `data_dir`.
///
/// The first call per directory per process advances the durable counter and
/// publishes one id; later calls with the same directory return that id
/// without touching the counter.
///
/// # Errors
///
/// Returns [`ClientError::Transport`] when the directory cannot be prepared, the
/// lock cannot be taken, the counter cannot be read/incremented/published, or
/// randomness cannot be drawn. The caller aborts connection start.
pub fn boot_incarnation(data_dir: &Path) -> Result<ClientIncarnationId, ClientError> {
    let key = data_dir.to_path_buf();
    // Held across the file update so concurrent first boots in this process
    // advance exactly once; later boots hit the cache without I/O.
    let mut cached = cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(incarnation) = cached.get(&key) {
        return Ok(*incarnation);
    }
    let counter = advance_counter(data_dir)?;
    let incarnation = ClientIncarnationId {
        counter,
        // 63 bits of OS randomness from the v4 UUID: the counter orders
        // boots, this disambiguates colliding counters after a wipe/restore.
        // Masked to the non-negative SQLite INTEGER range because history
        // rows store both halves as INTEGER and fail a full-range u64 closed
        // on encode; every other consumer only equality-checks.
        random: (uuid::Uuid::new_v4().as_u128() & i64::MAX as u128) as u64,
    };
    cached.insert(key, incarnation);
    Ok(incarnation)
}

/// Advances the durable counter once and returns the published value.
///
/// Uncached: each call takes the OS lock and increments. [`boot_incarnation`]
/// is the cached per-process entry point; tests use this directly for the
/// concurrent-serialization case.
///
/// # Errors
///
/// Same as [`boot_incarnation`]; a missing file counts as `0`, while an
/// existing-but-unreadable or corrupt file fails closed.
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
    // Blocking exclusive: concurrent boots serialize here, never fail.
    lock_file.lock().map_err(|error| {
        ClientError::Transport(format!("client incarnation lock failed: {}", error.kind()))
    })?;
    let current = read_counter(data_dir)?;
    let next = current.checked_add(1).ok_or_else(|| {
        ClientError::Transport(String::from("client incarnation counter exhausted"))
    })?;
    stage_and_replace(data_dir, next)?;
    // The OS lock releases when `lock_file` drops.
    Ok(next)
}

/// Test-only: forgets all booted incarnations so the next boot re-reads the
/// counter, simulating a process restart.
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

/// Reads the durable counter: absent means `0`; any existing-but-unreadable
/// or corrupt content fails closed (never re-initialized).
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

fn stage_and_replace(data_dir: &Path, next: u64) -> Result<(), ClientError> {
    let staged = data_dir.join(format!(
        ".{}.{}.{}.tmp",
        COUNTER_FILE_NAME,
        std::process::id(),
        STAGE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let staged_result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
            .map_err(|error| {
                ClientError::Transport(format!(
                    "client incarnation counter store failed: {}",
                    error.kind()
                ))
            })?;
        file.write_all(next.to_string().as_bytes())
            .map_err(|error| {
                ClientError::Transport(format!(
                    "client incarnation counter store failed: {}",
                    error.kind()
                ))
            })?;
        file.sync_all().map_err(|error| {
            ClientError::Transport(format!(
                "client incarnation counter store failed: {}",
                error.kind()
            ))
        })?;
        drop(file);
        std::fs::rename(&staged, counter_path(data_dir)).map_err(|error| {
            ClientError::Transport(format!(
                "client incarnation counter store failed: {}",
                error.kind()
            ))
        })
    })();
    if staged_result.is_err() {
        // Best effort: the temp holds only the counter, but leave no litter
        // behind without masking the real error.
        if std::fs::remove_file(&staged).is_err() {
            // Best effort only.
        }
    }
    staged_result
}
