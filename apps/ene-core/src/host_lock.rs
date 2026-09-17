//! Single-writer Host lock (`host.lock`) for one data directory.
//!
//! State open and serving startup are separate operations (PR §6.4): a process
//! that only reads, or that serves management queries, opens the store without
//! this lock. A process that performs startup mutation or serves the Host
//! holds the exclusive OS lock on `<data_dir>/host.lock` for its whole
//! lifetime, and the lock is taken before [`HostHandle::open`] because opening
//! runs migrations.
//!
//! The lock file is opened read/write/create and never truncated, renamed, or
//! unlinked; the handle owns the OS lock until the process ends. Only the OS
//! lock decides liveness: file contents, PID text, or socket existence are
//! never consulted.
//!
//! [`HostHandle::open`]: crate::serve::HostHandle::open

use std::fs::{File, OpenOptions};
use std::path::Path;

use crate::serve::CoreError;
use crate::serve::lifecycle::ensure_data_dir;

const LOCK_NAME: &str = "host.lock";

/// Exclusive per-data-directory writer lock.
///
/// The OS releases the lock when the handle drops (process exit included);
/// holding the value is the whole protocol, so there is no explicit unlock.
#[derive(Debug)]
pub struct HostLock {
    /// Never read: the open handle owns the lock. Dropping it releases.
    _file: File,
}

impl HostLock {
    /// Ensures the `0700` data directory, then takes the exclusive
    /// `<data_dir>/host.lock`.
    ///
    /// # Errors
    ///
    /// [`CoreError::AlreadyRunning`] when another Host holds the lock;
    /// [`CoreError::Store`] when the directory cannot be prepared, the lock
    /// file cannot be opened, or locking fails for an operational reason.
    pub fn acquire(data_dir: &Path) -> Result<Self, CoreError> {
        ensure_data_dir(data_dir)?;
        let path = data_dir.join(LOCK_NAME);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| CoreError::Store(format!("open host lock: {error}")))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => Err(CoreError::AlreadyRunning),
            Err(std::fs::TryLockError::Error(error)) => {
                Err(CoreError::Store(format!("lock host lock: {error}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HostLock;
    use crate::serve::{CoreError, CredStore, HostHandle};
    use ene_companion::CompanionRepository as _;
    use ene_credential::{DevicePairingRepository, MemoryCredentialStore};
    use ene_presence::PresenceRepository as _;
    use std::path::Path;

    /// One offline mutation command's startup: lock, then open the store (the
    /// migrations run inside the open), then mutate.
    async fn offline_approve(
        dir: &Path,
        pending_id: &str,
    ) -> Result<Option<(ene_credential::DeviceRecord, String)>, CoreError> {
        let _lock = HostLock::acquire(dir)?;
        let handle =
            HostHandle::open_with_cred_store(dir, CredStore::Memory(MemoryCredentialStore::new()))
                .await?;
        handle.approve_device(pending_id).await
    }

    async fn open_handle(dir: &Path) -> HostHandle {
        let handle =
            HostHandle::open_with_cred_store(dir, CredStore::Memory(MemoryCredentialStore::new()))
                .await;
        handle.expect("the winning handle must open")
    }

    /// The serving startup sequence through the production mutation entry
    /// point: lock, store open (migrations), then
    /// [`HostHandle::run_startup_mutations`]. Composing the steps here
    /// instead would let this fixture drift from what serving actually runs.
    /// The listener bind is excluded, so the test never serves.
    async fn serve_startup(dir: &Path) -> Result<(HostLock, HostHandle), CoreError> {
        let lock = HostLock::acquire(dir)?;
        let handle =
            HostHandle::open_with_cred_store(dir, CredStore::Memory(MemoryCredentialStore::new()))
                .await?;
        handle.run_startup_mutations().await?;
        Ok((lock, handle))
    }

    async fn generation(handle: &HostHandle) -> u64 {
        let companion = handle
            .store
            .ensure_running_companion()
            .await
            .expect("the companion must ensure");
        handle
            .store
            .load_attribution(companion.as_raw())
            .await
            .expect("the attribution must load")
            .expect("the attribution must exist")
            .generation
            .as_u64()
    }

    #[test]
    fn second_acquisition_of_the_same_directory_is_already_running() {
        let dir = tempfile::tempdir().expect("test scratch directory must be creatable");
        let first = HostLock::acquire(dir.path());
        assert!(
            first.is_ok(),
            "the first acquisition must succeed: {first:?}"
        );
        let second = HostLock::acquire(dir.path());
        assert!(
            matches!(second, Err(CoreError::AlreadyRunning)),
            "the second acquisition must be refused, got {second:?}"
        );
        drop(first);
        let third = HostLock::acquire(dir.path());
        assert!(
            third.is_ok(),
            "a released lock must be acquirable again: {third:?}"
        );
    }

    /// The loser must not reach the store open: migrations create `app.db`, so
    /// its absence proves no startup mutation ran.
    #[tokio::test]
    async fn refused_startup_never_opens_the_store() {
        let dir = tempfile::tempdir().expect("test scratch directory must be creatable");
        let winner = HostLock::acquire(dir.path());
        assert!(winner.is_ok(), "the winning lock must be held: {winner:?}");
        let database = dir.path().join("app.db");
        let loser = offline_approve(dir.path(), "laptop").await;
        assert!(
            matches!(loser, Err(CoreError::AlreadyRunning)),
            "the second startup must be refused at the lock, got {loser:?}"
        );
        assert!(
            !database.exists(),
            "the refused startup must not open or migrate the store"
        );
        drop(winner);
        let admitted = offline_approve(dir.path(), "unknown-pending-id").await;
        assert!(
            matches!(admitted, Ok(None)),
            "the released lock must admit the mutation command, which then \
             reports the unknown pending id, got {admitted:?}"
        );
        assert!(database.exists(), "the admitted startup opens the store");
    }

    /// A running Host keeps its presence generation and pending approvals
    /// untouched while an offline mutation command is refused.
    #[tokio::test]
    async fn refused_mutation_does_not_touch_presence_or_pendings() {
        let dir = tempfile::tempdir().expect("test scratch directory must be creatable");
        let winner = HostLock::acquire(dir.path());
        assert!(winner.is_ok(), "the winning lock must be held: {winner:?}");
        let handle = open_handle(dir.path()).await;
        let requested = DevicePairingRepository::request_pairing(
            &handle.store,
            String::from("laptop"),
            String::from("test-connection"),
            None,
        )
        .await;
        let Ok(ene_credential::DevicePairingStatus::Pending { pending }) = requested else {
            panic!("the fresh descriptor must pend, got {requested:?}");
        };
        let pending_id = pending.pending_id.clone();
        let before = generation(&handle).await;

        let refused = offline_approve(dir.path(), &pending_id).await;
        assert!(
            matches!(refused, Err(CoreError::AlreadyRunning)),
            "the mutation command must be refused while the Host runs, got {refused:?}"
        );
        assert_eq!(
            generation(&handle).await,
            before,
            "the refused command must not advance the presence generation"
        );
        let pending = handle.pending_devices().await.expect("pendings must list");
        assert!(
            pending
                .iter()
                .any(|entry| entry.pending_id == pending_id && entry.descriptor == "laptop"),
            "the refused command must not approve the pending device"
        );

        drop(winner);
        assert!(
            matches!(offline_approve(dir.path(), &pending_id).await, Ok(Some(_))),
            "the released lock must admit the mutation and approve the pending device"
        );
        let pending = handle.pending_devices().await.expect("pendings must list");
        assert!(
            pending.is_empty(),
            "the admitted command approves the pending device"
        );
    }

    /// A second serving startup is refused at the lock before any of the
    /// explicit startup mutations (sweep, reconciliation) or presence
    /// initialization can run, and the winner's durable state is unchanged.
    #[tokio::test]
    async fn second_serve_startup_is_refused_before_startup_mutation() {
        let dir = tempfile::tempdir().expect("test scratch directory must be creatable");
        let started = serve_startup(dir.path()).await;
        let (lock, winner) = started.expect("the winning startup must complete");
        let before = generation(&winner).await;
        let loser = serve_startup(dir.path()).await;
        assert!(
            matches!(loser, Err(CoreError::AlreadyRunning)),
            "the second serving startup must be refused at the lock, got {:?}",
            loser.as_ref().err().map(ToString::to_string)
        );
        assert_eq!(
            generation(&winner).await,
            before,
            "the refused startup must not run presence/notification/reconciliation mutations"
        );
        drop(lock);
        let restarted = serve_startup(dir.path()).await;
        assert!(
            restarted.is_ok(),
            "the released lock must admit a fresh startup: {:?}",
            restarted.as_ref().err().map(ToString::to_string)
        );
    }
}
