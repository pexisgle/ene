//! Test-only scratch space: temp directories and memory-backed handles.
//!
//! `ene-core` owns no `tempfile` dependency (its manifest is fixed), so tests
//! build their directories from [`std::env::temp_dir`] plus the process id and
//! a process-wide counter. Handles open file-backed stores under those
//! directories with a [`MemoryCredentialStore`], which keeps tests hermetic:
//! the environment store would read the real process environment on every
//! call. Bearers are provisioned through the setup closure before the store
//! moves into the handle.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ene_credential::MemoryCredentialStore;

use crate::serve::{CredStore, HostHandle, LiveInput};

/// Process-wide counter disambiguating directories within one test process.
static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

/// Creates a fresh empty directory for one test and returns its path.
///
/// Returns [`None`] when the directory cannot be created; callers assert and
/// return early in that case.
pub(crate) fn temp_data_dir(tag: &str) -> Option<PathBuf> {
    let serial = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ene-core-{tag}-{}-{serial}", std::process::id()));
    match std::fs::create_dir_all(&dir) {
        Ok(()) => Some(dir),
        Err(_) => None,
    }
}

/// Removes a directory created by [`temp_data_dir`], ignoring the outcome.
///
/// Removal is best-effort cleanup: a leftover directory never affects the next
/// test because every directory is unique.
pub(crate) fn remove_data_dir(dir: &Path) {
    let _removed = std::fs::remove_dir_all(dir);
}

/// Opens a memory-store handle under a fresh temp directory.
///
/// `setup` provisions bearers into the memory store before it moves into the
/// handle. Asserts the open succeeds and returns [`None`] early otherwise.
pub(crate) async fn memory_handle_with(
    tag: &str,
    setup: impl FnOnce(&MemoryCredentialStore),
) -> Option<(HostHandle, PathBuf)> {
    let dir = temp_data_dir(tag)?;
    let store = MemoryCredentialStore::new();
    setup(&store);
    let handle = HostHandle::open_with_cred_store(&dir, CredStore::Memory(store)).await;
    assert!(handle.is_ok(), "handle open must succeed");
    if let Ok(handle) = handle {
        Some((handle, dir))
    } else {
        remove_data_dir(&dir);
        None
    }
}

/// Opens a memory-store handle with an empty bearer store.
pub(crate) async fn memory_handle(tag: &str) -> Option<(HostHandle, PathBuf)> {
    memory_handle_with(tag, |_| {}).await
}

/// Builds a live, authorized [`LiveInput`] for a client ref.
pub(crate) fn live_input(client_ref: &str) -> LiveInput {
    LiveInput {
        client_ref: client_ref.to_string(),
        connection_live: true,
        peer_uid_ok: true,
    }
}
