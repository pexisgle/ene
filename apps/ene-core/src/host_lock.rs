use std::fs::{File, OpenOptions};
use std::path::Path;

use crate::serve::CoreError;
use crate::serve::lifecycle::ensure_data_dir;

const LOCK_NAME: &str = "host.lock";

#[derive(Debug)]
pub struct HostLock {
    _file: File,
}

impl HostLock {
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

    async fn offline_approve(
        dir: &Path,
        pending_id: &str,
    ) -> Result<Option<ene_credential::DeviceRecord>, CoreError> {
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
        )
        .await;
        let Ok(pending) = requested else {
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
            matches!(offline_approve(dir.path(), &pending_id).await, Ok(None)),
            "an offline handle has no originating delivery slot and must not approve"
        );
        let pending = handle.pending_devices().await.expect("pendings must list");
        assert!(
            pending.iter().any(|entry| entry.pending_id == pending_id),
            "the offline attempt leaves the pending request untouched"
        );
    }

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
