//! Test-only helpers: scratch directories and memory-backed handles.
//!
//! Each test owns a [`tempfile::TempDir`], which removes the directory on
//! drop, so early returns and panics need no manual cleanup. Handles open
//! file-backed stores under those directories with a
//! [`MemoryCredentialStore`], which keeps tests hermetic: the environment
//! store would read the real process environment on every call. Bearers are
//! provisioned through the setup closure before the store moves into the
//! handle.

use ene_api::v1::refs::ConnectionWireId;
use ene_credential::MemoryCredentialStore;

use crate::serve::{CredStore, HostHandle, LiveInput};

/// Opens a memory-store handle under a fresh temp directory.
///
/// `setup` provisions bearers into the memory store before it moves into the
/// handle. The directory lives as long as the returned [`TempDir`].
pub(crate) async fn memory_handle_with(
    tag: &str,
    setup: impl FnOnce(&MemoryCredentialStore),
) -> Option<(HostHandle, tempfile::TempDir)> {
    let dir = tempfile::Builder::new()
        .prefix(&format!("ene-core-{tag}-"))
        .tempdir()
        .expect("test scratch directory must be creatable");
    let store = MemoryCredentialStore::new();
    setup(&store);
    let handle = HostHandle::open_with_cred_store(dir.path(), CredStore::Memory(store)).await;
    handle.ok().map(|handle| (handle, dir))
}

/// Opens a memory-store handle with an empty bearer store.
pub(crate) async fn memory_handle(tag: &str) -> Option<(HostHandle, tempfile::TempDir)> {
    memory_handle_with(tag, |_| {}).await
}

/// Builds a live, authorized [`LiveInput`] for a client ref.
///
/// The premises describe a paired, known, authenticated connection on a
/// freshly minted table id: `paired_device` carries the client ref as the
/// device wire string, `connection_known` and `authed` hold, and
/// `connection_id` is the id domain frames must echo in the envelope. Tests
/// for the gate itself override these fields explicitly.
pub(crate) fn live_input(client_ref: &str) -> LiveInput {
    LiveInput {
        client_ref: client_ref.to_string(),
        connection_live: true,
        peer_uid_ok: true,
        paired_device: Some(client_ref.to_string()),
        connection_known: true,
        authed: true,
        connection_id: ConnectionWireId(uuid::Uuid::new_v4()),
        negotiated: None,
    }
}
