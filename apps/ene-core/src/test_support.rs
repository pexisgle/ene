//! Test-only helpers: scratch directories and memory-backed credentials.
//!
//! Each test owns a [`tempfile::TempDir`], which removes the directory on
//! drop, so early returns and panics need no manual cleanup. Handles keep the
//! real file-backed SQLite store so reopen/restart semantics and direct
//! `app.db` inspection remain meaningful, but logical Host tests opt out of
//! SQLite crash-durability fsyncs through `ene-store` test support. Credential
//! values stay in [`MemoryCredentialStore`], keeping tests hermetic: the
//! environment store reads the real process environment once when constructed.

use std::sync::Arc;

use ene_api::v1::envelope::ProtocolVersion;
use ene_api::v1::handshake::NegotiatedConnection;
use ene_api::v1::refs::ConnectionWireId;
use ene_credential::{
    CredentialRef, CredentialRefRepository, CredentialScrubber, CredentialSetRepository,
    CredentialSetRevision, CredentialTechnicalError, MemoryCredentialStore,
};
use ene_store::Store;
use ene_task::{
    DelegationId, TaskResultArrivalOutcome, TaskResultRecord, TaskResultScrubPremise,
    orchestrate_result_arrival,
};

use crate::conn::{
    ChallengeOutcome, ConnectionPhase, ConnectionTable, InstallOutcome, NonceAdmission,
};
use crate::serve::{CredStore, HostHandle, LiveInput};

/// A readable credential registry with no refs, pinned at one revision.
struct EmptyRevisionRegistry(CredentialSetRevision);

impl CredentialRefRepository for EmptyRevisionRegistry {
    #[expect(clippy::unused_async_trait_impl, reason = "fixture repository port")]
    async fn list_refs(&self) -> Result<Vec<CredentialRef>, CredentialTechnicalError> {
        Ok(Vec::new())
    }
}

impl CredentialSetRepository for EmptyRevisionRegistry {
    #[expect(clippy::unused_async_trait_impl, reason = "fixture repository port")]
    async fn current_set_revision(
        &self,
    ) -> Result<CredentialSetRevision, CredentialTechnicalError> {
        Ok(self.0)
    }
}

/// Scrubs `text` through the credential-owned boundary under `revision`; the
/// returned premise is the only way a test can name a Task result body.
pub(crate) async fn scrubbed_result_at(
    revision: CredentialSetRevision,
    text: &str,
) -> TaskResultScrubPremise {
    use ene_credential::SecretScrubber as _;

    let registry = EmptyRevisionRegistry(revision);
    let values = MemoryCredentialStore::new();
    TaskResultScrubPremise::from_scrubbed(
        CredentialScrubber {
            refs: &registry,
            store: &values,
        }
        .scrub(text)
        .await
        .expect("the empty fixture registry is readable"),
    )
}

/// Scrubs `text` under the store's current credential-set revision.
pub(crate) async fn scrubbed_result(store: &Store, text: &str) -> TaskResultScrubPremise {
    let revision = store
        .current_set_revision()
        .await
        .expect("the fixture credential-set revision reads");
    scrubbed_result_at(revision, text).await
}

/// Records one result through the production arrival boundary and returns the
/// recorded result. Fixtures scrub at the current revision, so a stale
/// refusal here would be a fixture error.
pub(crate) async fn record_result(
    store: &Store,
    delegation: DelegationId,
    body: &str,
) -> TaskResultRecord {
    match orchestrate_result_arrival(store, delegation, scrubbed_result(store, body).await)
        .await
        .expect("the arrival must answer")
    {
        TaskResultArrivalOutcome::Recorded(record) => record,
        TaskResultArrivalOutcome::StaleCredentialSet { .. } => {
            panic!("the fixture scrubbed at the current revision")
        }
    }
}

/// `setup` provisions bearers before the store moves into the handle; the
/// directory lives as long as the returned [`TempDir`].
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
    let handle = HostHandle::open_with_cred_store(dir.path(), CredStore::Memory(store))
        .await
        .ok()?;
    handle.store.relax_durability_for_tests().await.ok()?;
    Some((handle, dir))
}

pub(crate) async fn memory_handle(tag: &str) -> Option<(HostHandle, tempfile::TempDir)> {
    memory_handle_with(tag, |_| {}).await
}

/// Drives one connection table record to authenticated-and-current without
/// any transport, the way the handshake paths do over a socket.
pub(crate) fn authenticate(table: &Arc<ConnectionTable>, id: &ConnectionWireId, device_wire: &str) {
    assert!(
        table.note_paired(id, device_wire),
        "the connection must be accepted and unpaired"
    );
    let terms = NegotiatedConnection {
        version: ProtocolVersion::V1,
    };
    assert!(
        matches!(
            table.note_challenged(id, None, terms, String::from("test-nonce")),
            ChallengeOutcome::Challenged
        ),
        "the paired connection must accept one challenge"
    );
    assert!(
        matches!(
            table.take_nonce(id),
            NonceAdmission::Nonce(nonce) if nonce == "test-nonce"
        ),
        "the challenge nonce must be pending"
    );
    assert!(
        matches!(
            table.install_authenticated(id),
            InstallOutcome::Installed { .. }
        ),
        "the verified proof installs the connection"
    );
    assert_eq!(
        table.phase_of(id),
        Some(ConnectionPhase::Authenticated),
        "the install must authenticate the connection"
    );
    assert!(
        table.current_authenticated(device_wire),
        "the install must make the connection current for its device"
    );
}

/// Builds a [`LiveInput`] for a paired, known, authenticated-and-current
/// connection on a freshly minted table id, with `paired_device` carrying the
/// client ref as the device wire string. Tests of the gate itself override
/// these fields explicitly.
pub(crate) fn live_input(client_ref: &str) -> LiveInput {
    let table = Arc::new(ConnectionTable::new());
    let id = table.note_accept();
    authenticate(&table, &id, client_ref);
    table
        .snapshot(&id)
        .expect("the authenticated connection must snapshot")
}
