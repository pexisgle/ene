use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::OnceLock;

use ene_api::v1::envelope::ProtocolVersion;
use ene_api::v1::handshake::{NegotiatedConnection, PairingProvision, PairingProvisionSecret};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{ConnectionWireId, RoundWireId};
use ene_api::v1::reject::RejectKind;
use ene_companion::{CompanionId, CompanionRepository};
use ene_credential::{
    CredentialApprovalRepository, CredentialRef, CredentialRefRepository as _, CredentialStore,
    CredentialTechnicalError, DevicePairingRepository, DeviceRecord, EnvCredentialStore,
    FileDeviceAuthStore, MemoryCredentialStore, VersionedCredentialStore as _,
};
use ene_inference::{ProviderTransport, UsageRepository as _};
use ene_permission::EvaluationTracker;
use ene_presence::{
    ClientId, ConfirmTransitionOutcome, FallbackCandidate, LiveReachabilityRef, MoveDecision,
    PresenceCheckRef, PresenceRepository, PresenceState, ThinMoveReason, select_fallback_candidate,
};
use ene_presentation::{OpenRound, RoundId};
use ene_primitive::RawId;
use ene_store::Store;
use ene_task::{
    CancelTaskCommand, DelegationId, TaskCancelOutcome, TaskRepository as _, TaskTechnicalError,
};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::conn::{ConnectionPhase, ConnectionTable};
use crate::dialogue::HostInference;
use crate::task_agent::{OwnerInstructionSource, TaskAgentInferenceAdapter};
use crate::task_run::{TaskAgentRunError, TaskAgentRunOutcome, TaskAgentRunRefusal};
use ene_credential::CredentialScrubber;

mod frames;
mod handshake;
pub(crate) mod lifecycle;

use lifecycle::ensure_data_dir;

pub(crate) use frames::{
    invalid_phase_reject, outgoing_envelope, outgoing_fact, outgoing_frame,
    outgoing_frame_pre_auth, reject_frame, stale_reject, unpaired_close,
};
pub(crate) use handshake::attribution_to_wire;
pub use lifecycle::serve;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("store unavailable: {0}")]
    Store(String),
    #[error("another Host is already running for this data directory")]
    AlreadyRunning,
    #[error("bind failed: {0}")]
    Bind(String),
    #[error("serving task failed: {0}")]
    Serving(String),
    #[error("inference failed: {0}")]
    Inference(String),
    #[error("device approval failed: {0}")]
    Approve(String),
    #[error("targeted deletion failed: {0}")]
    Deletion(String),
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(&'static str),
    /// No Host is serving this data directory. The CLI guides an explicit
    /// start and never falls back to an offline mutation (first-party-desktop §5.1.5).
    #[error(
        "the Host is not serving; start `ene-core serve` and retry — the Owner's confirmation surface runs there, and an offline command cannot record one"
    )]
    HostUnavailable,
}

pub trait FrameSink: Send {
    fn emit(&mut self, frame: WireFrame) -> Result<(), FrameDeliveryError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "a failed delivery must stop the operation, never drop silently"]
pub enum FrameDeliveryError {
    Closed,
    Full,
}

pub(crate) fn emit_end(sink: &mut dyn FrameSink, frame: WireFrame) {
    if sink.emit(frame).is_err() {
        // Gone or full channel: the operation ends undelivered either way.
    }
}

pub(crate) const STREAM_BUFFER_FRAMES: usize = 32;

impl FrameSink for tokio::sync::mpsc::Sender<WireFrame> {
    fn emit(&mut self, frame: WireFrame) -> Result<(), FrameDeliveryError> {
        use tokio::sync::mpsc::error::TrySendError;

        match self.try_send(frame) {
            Ok(()) => Ok(()),
            Err(TrySendError::Closed(_)) => Err(FrameDeliveryError::Closed),
            Err(TrySendError::Full(_)) => Err(FrameDeliveryError::Full),
        }
    }
}

#[derive(Debug)]
pub enum CredStore {
    Env(EnvCredentialStore),
    Memory(MemoryCredentialStore),
    Os(ene_credential::OsCredentialStore),
    MemoryVersioned(ene_credential::MemoryVersionedStore),
}

impl CredentialStore for CredStore {
    fn with_bearer<R>(
        &self,
        cred: &CredentialRef,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError> {
        match self {
            Self::Env(inner) => inner.with_bearer(cred, f),
            Self::Memory(inner) => inner.with_bearer(cred, f),
            Self::Os(inner) => inner.with_bearer(cred, f),
            Self::MemoryVersioned(inner) => inner.with_bearer(cred, f),
        }
    }

    fn contains(&self, cred: &CredentialRef) -> bool {
        match self {
            Self::Env(inner) => inner.contains(cred),
            Self::Memory(inner) => inner.contains(cred),
            Self::Os(inner) => inner.contains(cred),
            Self::MemoryVersioned(inner) => inner.contains(cred),
        }
    }

    fn put(&self, cred: &CredentialRef, secret: &str) -> Result<(), CredentialTechnicalError> {
        match self {
            Self::Env(inner) => inner.put(cred, secret),
            Self::Memory(inner) => inner.put(cred, secret),
            Self::Os(inner) => inner.put(cred, secret),
            Self::MemoryVersioned(inner) => inner.put(cred, secret),
        }
    }
}

impl CredStore {
    #[must_use]
    pub fn supports_versions(&self) -> bool {
        match self {
            Self::Os(_) | Self::MemoryVersioned(_) => true,
            Self::Env(_) | Self::Memory(_) => false,
        }
    }
}

impl ene_credential::VersionedCredentialStore for CredStore {
    fn put_version(
        &self,
        cred: &CredentialRef,
        version: u64,
        secret: &str,
    ) -> Result<(), CredentialTechnicalError> {
        match self {
            Self::Os(inner) => inner.put_version(cred, version, secret),
            Self::MemoryVersioned(inner) => inner.put_version(cred, version, secret),
            Self::Env(_) | Self::Memory(_) => Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!(
                    "{}: this backend cannot publish credential versions",
                    cred.id()
                ),
            }),
        }
    }

    fn with_version<R>(
        &self,
        cred: &CredentialRef,
        version: u64,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError> {
        match self {
            Self::Os(inner) => inner.with_version(cred, version, f),
            Self::MemoryVersioned(inner) => inner.with_version(cred, version, f),
            Self::Env(_) | Self::Memory(_) => Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!(
                    "{}: this backend cannot read credential versions",
                    cred.id()
                ),
            }),
        }
    }

    fn prepare_snapshot(
        &self,
        cred: &CredentialRef,
        version: u64,
    ) -> Result<ene_credential::PreparedCredentialSnapshot, CredentialTechnicalError> {
        match self {
            Self::Os(inner) => inner.prepare_snapshot(cred, version),
            Self::MemoryVersioned(inner) => inner.prepare_snapshot(cred, version),
            Self::Env(_) | Self::Memory(_) => Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!(
                    "{}: this backend cannot prepare credential snapshots",
                    cred.id()
                ),
            }),
        }
    }

    fn activate(&self, snapshot: ene_credential::PreparedCredentialSnapshot) {
        match self {
            Self::Os(inner) => inner.activate(snapshot),
            Self::MemoryVersioned(inner) => inner.activate(snapshot),
            Self::Env(_) | Self::Memory(_) => {}
        }
    }

    fn deactivate(&self, cred: &CredentialRef) {
        match self {
            Self::Os(inner) => inner.deactivate(cred),
            Self::MemoryVersioned(inner) => inner.deactivate(cred),
            Self::Env(_) | Self::Memory(_) => {}
        }
    }

    fn delete_version(
        &self,
        cred: &CredentialRef,
        version: u64,
    ) -> Result<(), CredentialTechnicalError> {
        match self {
            Self::Os(inner) => inner.delete_version(cred, version),
            Self::MemoryVersioned(inner) => inner.delete_version(cred, version),
            Self::Env(_) | Self::Memory(_) => Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!(
                    "{}: this backend cannot remove credential versions",
                    cred.id()
                ),
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LiveInput {
    pub client_ref: String,
    pub connection_live: bool,
    pub peer_uid_ok: bool,
    pub paired_device: Option<String>,
    pub connection_known: bool,
    pub authed: bool,
    pub connection_id: ConnectionWireId,
    pub negotiated: Option<NegotiatedConnection>,
    pub phase: ConnectionPhase,
    pub(crate) authority: std::sync::Arc<ConnectionTable>,
}

impl PartialEq for LiveInput {
    fn eq(&self, other: &Self) -> bool {
        self.client_ref == other.client_ref
            && self.connection_live == other.connection_live
            && self.peer_uid_ok == other.peer_uid_ok
            && self.paired_device == other.paired_device
            && self.connection_known == other.connection_known
            && self.authed == other.authed
            && self.connection_id == other.connection_id
            && self.negotiated == other.negotiated
            && self.phase == other.phase
            && std::sync::Arc::ptr_eq(&self.authority, &other.authority)
    }
}

impl Eq for LiveInput {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateDecision {
    Pass,
    Stale,
    Unpaired,
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct TestGate {
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
}

#[cfg(test)]
impl Default for TestGate {
    fn default() -> Self {
        Self {
            entered: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
        }
    }
}

#[cfg(test)]
impl TestGate {
    pub(crate) async fn pause(&self) {
        self.entered.add_permits(1);
        let permit = self.release.acquire().await.expect("gate stays open");
        permit.forget();
    }
}

#[cfg(test)]
pub(crate) type TestCloseGate = TestGate;

pub(crate) fn connection_key(id: &ConnectionWireId) -> String {
    id.0.as_hyphenated().to_string()
}

pub(crate) fn device_client(device_wire: &str) -> ClientId {
    ClientId::from_raw(RawId::from_uuid(Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        device_wire.as_bytes(),
    )))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CurrentConnection {
    pub client_ref: String,
    pub same_machine: bool,
    pub device_permitted: bool,
}
pub struct HostHandle {
    pub(crate) store: Store,
    pub(crate) tracker: AsyncMutex<EvaluationTracker>,
    pub(crate) open_rounds: StdMutex<HashMap<(String, String), OpenRound>>,
    pub(crate) rounds: Arc<StdMutex<HashMap<String, RoundId>>>,
    pub(crate) cred_store: CredStore,
    pub(crate) credential_publication: AsyncMutex<()>,
    pub(crate) control_seat: crate::host_control::FirstPartyControlSeat,
    pub(crate) data_dir: std::path::PathBuf,
    pub(crate) gui_open: AsyncMutex<()>,
    pub(crate) gui_child: StdMutex<Option<crate::host_control::GuiProcess>>,
    pub(crate) confirmation_tasks: StdMutex<Vec<tokio::task::JoinHandle<()>>>,
    pub(crate) auth_store: FileDeviceAuthStore,
    pub(crate) pairing_deliveries: crate::pairing_delivery::PairingDeliveryRegistry,
    pub(crate) learning_queue: Arc<StdMutex<crate::transient_erasure::LearningFormationQueue>>,
    pub(crate) host_transient_arrival: Arc<crate::transient_erasure::HostTransientArrival>,
    pub(crate) learning_worker: AsyncMutex<()>,
    pub(crate) companion_wire: String,
    pub(crate) task_executions: std::sync::Arc<crate::task_run::TaskExecutionRegistry>,
    pub(crate) conversation_tasks: crate::task_control::ConversationTaskProjection,
    pub(crate) presentations: Arc<StdMutex<crate::presentation::PresentationState>>,
    pub(crate) presentation_lock: AsyncMutex<()>,
    pub(crate) trusted_task_premises: crate::task_control::TrustedTaskPremises,
    pub(crate) task_launcher: OnceLock<std::sync::Arc<dyn crate::task_run::TaskAgentLauncher>>,
    pub(crate) targeted_deletion: StdMutex<crate::targeted_deletion::ErasureParticipantRegistry>,
    pub(crate) deletion_hold_retry: AsyncMutex<crate::targeted_deletion::HeldRetrySchedule>,
    pub(crate) targeted_deletion_drive: AsyncMutex<()>,
    deletion_drivers: std::sync::atomic::AtomicUsize,
    pub(crate) deletion_driver_wake: tokio::sync::Notify,
    #[cfg(test)]
    pub(crate) host_control_confirm_gate: StdMutex<Option<Arc<TestGate>>>,
    pub(crate) transient_fence: Arc<crate::transient_erasure::TransientErasureFence>,
    pub(crate) client_transients: Arc<crate::transient_erasure::ClientTransientRegistry>,
    #[cfg(test)]
    pub(crate) task_control_gate:
        StdMutex<Option<std::sync::Arc<crate::task_control::TestTaskControlGate>>>,
    #[cfg(test)]
    pub(crate) close_gate: StdMutex<Option<std::sync::Arc<TestCloseGate>>>,
    #[cfg(test)]
    pub(crate) confirm_commit_gate: StdMutex<Option<std::sync::Arc<TestGate>>>,
    #[cfg(test)]
    pub(crate) delivery_evidence_gate: StdMutex<Option<std::sync::Arc<TestGate>>>,
    #[cfg(test)]
    pub(crate) ref_mint_gate: StdMutex<Option<std::sync::Arc<TestGate>>>,
    #[cfg(test)]
    pub(crate) fetch_gate: StdMutex<Option<std::sync::Arc<TestGate>>>,
    #[cfg(test)]
    pub(crate) resume_gate: StdMutex<Option<std::sync::Arc<crate::task_control::TestResumeGate>>>,
    #[cfg(test)]
    pub(crate) presentation_commit_gate:
        StdMutex<Option<std::sync::Arc<crate::presentation::TestPresentationCommitGate>>>,
    #[cfg(all(test, unix))]
    pub(crate) receipt_expiry_runs: std::sync::atomic::AtomicUsize,
}

impl HostHandle {
    #[cfg(test)]
    pub(crate) fn host_control_confirm_gate(&self) -> Option<Arc<TestGate>> {
        crate::lock_unpoison(&self.host_control_confirm_gate).clone()
    }

    pub async fn open(data_dir: &Path) -> Result<Self, CoreError> {
        let store = if std::env::var(ene_credential::ENV_API_KEY).is_ok() {
            CredStore::Env(EnvCredentialStore::new())
        } else {
            CredStore::Os(ene_credential::OsCredentialStore::new(
                ene_credential::DEFAULT_NAMESPACE,
            ))
        };
        Self::open_with_cred_store(data_dir, store).await
    }

    pub async fn run_startup_mutations(&self) -> Result<(), CoreError> {
        self.normalize_presence_on_startup().await?;
        self.clear_unapproved_pendings().await?;
        self.reconcile_credential_publication().await?;
        self.sweep_registered_values().await?;
        self.reconcile_sealed_results()
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        self.store
            .reconcile_orphaned_usage_reservations()
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        self.recover_targeted_deletion_on_startup().await?;
        Ok(())
    }

    pub async fn reconcile_credential_publication(&self) -> Result<(), CoreError> {
        use ene_credential::{CredentialPublicationRepository as _, VersionedCredentialStore as _};

        if !self.cred_store.supports_versions() {
            return Ok(());
        }
        let refs = ene_credential::CredentialRefRepository::list_refs(&self.store)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        for credential in refs {
            let active = self
                .store
                .active_credential_version(credential.provider(), credential.label())
                .await
                .map_err(|error| CoreError::Store(error.to_string()))?;
            let Some(version) = active.active else {
                continue;
            };
            match self
                .cred_store
                .prepare_snapshot(&credential, version.as_u64())
            {
                Ok(snapshot) => self.cred_store.activate(snapshot),
                Err(_) => self.cred_store.deactivate(&credential),
            }
        }
        Ok(())
    }

    pub async fn open_with_cred_store(
        data_dir: &Path,
        cred_store: CredStore,
    ) -> Result<Self, CoreError> {
        ensure_data_dir(data_dir)?;
        let database = data_dir.join("app.db");
        let store = Store::open(&database)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        let auth_store = FileDeviceAuthStore::open(&data_dir.join("device-auth.json"))
            .map_err(|error| CoreError::Store(error.to_string()))?;
        let presentations = Arc::new(StdMutex::new(
            crate::presentation::PresentationState::default(),
        ));
        let learning_queue = Arc::new(StdMutex::new(
            crate::transient_erasure::LearningFormationQueue::default(),
        ));
        let host_transient_arrival =
            Arc::new(crate::transient_erasure::HostTransientArrival::default());
        let transient_fence = Arc::new(crate::transient_erasure::TransientErasureFence::default());
        let client_transients = Arc::new(crate::transient_erasure::ClientTransientRegistry::new(
            store.clone(),
        ));
        let handle = Self {
            store,
            tracker: AsyncMutex::new(EvaluationTracker::new()),
            open_rounds: StdMutex::new(HashMap::new()),
            rounds: Arc::new(StdMutex::new(HashMap::new())),
            cred_store,
            credential_publication: AsyncMutex::new(()),
            control_seat: crate::host_control::FirstPartyControlSeat::default(),
            data_dir: data_dir.to_path_buf(),
            gui_open: AsyncMutex::new(()),
            gui_child: StdMutex::new(None),
            confirmation_tasks: StdMutex::new(Vec::new()),
            auth_store,
            pairing_deliveries: crate::pairing_delivery::PairingDeliveryRegistry::default(),
            learning_queue: Arc::clone(&learning_queue),
            host_transient_arrival: Arc::clone(&host_transient_arrival),
            learning_worker: AsyncMutex::new(()),
            companion_wire: RawId::new().as_uuid().to_string(),
            task_executions: std::sync::Arc::new(crate::task_run::TaskExecutionRegistry::default()),
            conversation_tasks: crate::task_control::ConversationTaskProjection::default(),
            presentations: Arc::clone(&presentations),
            presentation_lock: AsyncMutex::new(()),
            trusted_task_premises: crate::task_control::TrustedTaskPremises::default(),
            task_launcher: OnceLock::new(),
            targeted_deletion: StdMutex::new(
                crate::targeted_deletion::ErasureParticipantRegistry::new(),
            ),
            deletion_hold_retry: AsyncMutex::new(crate::targeted_deletion::HeldRetrySchedule::new()),
            targeted_deletion_drive: AsyncMutex::new(()),
            deletion_drivers: std::sync::atomic::AtomicUsize::new(0),
            deletion_driver_wake: tokio::sync::Notify::new(),
            #[cfg(test)]
            host_control_confirm_gate: StdMutex::new(None),
            transient_fence: Arc::clone(&transient_fence),
            client_transients,
            #[cfg(test)]
            task_control_gate: StdMutex::new(None),
            #[cfg(test)]
            close_gate: StdMutex::new(None),
            #[cfg(test)]
            confirm_commit_gate: StdMutex::new(None),
            #[cfg(test)]
            delivery_evidence_gate: StdMutex::new(None),
            #[cfg(test)]
            ref_mint_gate: StdMutex::new(None),
            #[cfg(test)]
            fetch_gate: StdMutex::new(None),
            #[cfg(test)]
            resume_gate: StdMutex::new(None),
            #[cfg(test)]
            presentation_commit_gate: StdMutex::new(None),
            #[cfg(all(test, unix))]
            receipt_expiry_runs: std::sync::atomic::AtomicUsize::new(0),
        };
        handle.install_local_erasure_participants()?;
        Ok(handle)
    }

    fn install_local_erasure_participants(&self) -> Result<(), CoreError> {
        use std::sync::Arc;

        let participants: [Arc<dyn ene_preservation::ErasureParticipant>; 8] = [
            Arc::new(ene_store::CompanionErasureParticipant::new(
                self.store.clone(),
            )),
            Arc::new(ene_store::LearningErasureParticipant::new(
                self.store.clone(),
            )),
            Arc::new(ene_store::TaskErasureParticipant::new(self.store.clone())),
            Arc::new(ene_store::ActionErasureParticipant::new(self.store.clone())),
            Arc::new(ene_store::InferenceErasureParticipant::new(
                self.store.clone(),
            )),
            Arc::new(ene_permission::PermissionErasureParticipant::new(Arc::new(
                self.store.clone(),
            ))),
            Arc::new(ene_credential::CredentialErasureParticipant::new(
                Arc::new(self.store.clone()),
                Arc::new(self.auth_store.clone()),
            )),
            Arc::new(ene_presence::PresenceErasureParticipant::new(Arc::new(
                self.store.clone(),
            ))),
        ];
        for participant in participants {
            self.register_deletion_participant(participant)?;
        }
        let host_transient = Arc::new(crate::transient_erasure::HostTransientParticipant::new(
            self.store.clone(),
            self.transient_fence.clone(),
            self.presentations.clone(),
            self.learning_queue.clone(),
            Arc::clone(&self.host_transient_arrival),
        ));
        crate::lock_unpoison(&self.targeted_deletion)
            .register_host_transient(host_transient)
            .map_err(|owner| {
                CoreError::Deletion(format!(
                    "duplicate deletion participant for owner class {}",
                    owner.class_name()
                ))
            })?;
        crate::lock_unpoison(&self.targeted_deletion)
            .install_client_transients(Arc::clone(&self.client_transients));
        Ok(())
    }

    pub(crate) async fn sweep_registered_values(&self) -> Result<(), CoreError> {
        let refs = self
            .store
            .list_refs()
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        self.store
            .sweep_registered_values(&refs, &self.cred_store)
            .map_err(|error| CoreError::Store(error.to_string()))?;
        Ok(())
    }

    pub(crate) fn companion_wire(&self) -> &str {
        &self.companion_wire
    }

    #[doc(hidden)]
    pub fn store_for_tests(&self) -> &Store {
        &self.store
    }

    #[doc(hidden)]
    #[must_use]
    pub fn credential_contains_for_tests(&self, provider: &str, label: &str) -> bool {
        CredentialRef::new(provider, label).is_ok_and(|cred| self.cred_store.contains(&cred))
    }

    #[doc(hidden)]
    #[must_use]
    pub fn credential_matches_for_tests(
        &self,
        provider: &str,
        label: &str,
        expected: &str,
    ) -> bool {
        CredentialRef::new(provider, label).is_ok_and(|credential| {
            self.cred_store
                .with_bearer(&credential, |bearer| bearer == expected)
                .unwrap_or(false)
        })
    }

    #[doc(hidden)]
    pub fn live_targeted_deletion_drivers_for_tests(&self) -> usize {
        self.deletion_drivers
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    #[doc(hidden)]
    pub fn wake_deletion_driver_for_tests(&self) {
        self.deletion_driver_wake.notify_waiters();
    }

    #[doc(hidden)]
    pub fn set_client_erasure_wait_for_tests(&self, limit: std::time::Duration) {
        self.client_transients.set_wait_limit_for_test(limit);
    }

    pub(crate) fn begin_deletion_driver(&self) {
        self.deletion_drivers
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn end_deletion_driver(&self) {
        self.deletion_drivers
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) async fn resolve_companion(
        &self,
        wire: &str,
    ) -> Result<Option<CompanionId>, ene_companion::CompanionTechnicalError> {
        if wire != self.companion_wire.as_str() {
            return Ok(None);
        }
        self.store.ensure_running_companion().await.map(Some)
    }

    pub async fn cancel_task(
        &self,
        command: CancelTaskCommand,
    ) -> Result<TaskCancelOutcome, TaskTechnicalError> {
        let outcome = self.store.cancel_task(command.task).await?;
        if outcome == TaskCancelOutcome::CancelAccepted {
            self.task_executions.cancel(command.task);
        }
        Ok(outcome)
    }

    pub fn install_task_launcher(
        &self,
        launcher: std::sync::Arc<dyn crate::task_run::TaskAgentLauncher>,
    ) -> bool {
        self.task_launcher.set(launcher).is_ok()
    }

    pub fn register_deletion_participant(
        &self,
        participant: std::sync::Arc<dyn ene_preservation::ErasureParticipant>,
    ) -> Result<(), CoreError> {
        crate::lock_unpoison(&self.targeted_deletion)
            .register(participant)
            .map_err(|owner| {
                CoreError::Deletion(format!(
                    "duplicate deletion participant for owner class {}",
                    owner.class_name()
                ))
            })
    }

    pub async fn required_deletion_participants(
        &self,
    ) -> Result<Vec<ene_preservation::ParticipantOwnerRef>, CoreError> {
        let mut owners = crate::targeted_deletion::current_product_surface_owners();
        let mut after = None;
        loop {
            let page = self
                .store
                .client_delivery_evidence_incarnations(after, 100)
                .await
                .map_err(|error| CoreError::Store(error.to_string()))?;
            if page.is_empty() {
                break;
            }
            let short = page.len() < 100;
            after = page.last().copied();
            owners.extend(
                page.into_iter()
                    .map(ene_preservation::ParticipantOwnerRef::ClientIncarnation),
            );
            if short {
                break;
            }
        }
        Ok(owners)
    }

    pub(crate) fn install_client_connection_table(&self, table: std::sync::Arc<ConnectionTable>) {
        self.client_transients.install_connection_table(table);
    }

    #[must_use]
    pub(crate) fn transient_fence_epoch(&self) -> u64 {
        self.transient_fence.epoch()
    }

    pub async fn drive_targeted_deletion(
        &self,
        pass: crate::targeted_deletion::TargetedDeletionPass,
    ) -> Result<crate::targeted_deletion::TargetedDeletionPassOutcome, CoreError> {
        let _drive = self.targeted_deletion_drive.lock().await;
        let registry = crate::lock_unpoison(&self.targeted_deletion).clone();
        crate::targeted_deletion::drive_targeted_deletion(&self.store, &registry, pass).await
    }

    pub async fn run_targeted_deletion_tick(
        &self,
    ) -> Result<crate::targeted_deletion::TargetedDeletionPassOutcome, CoreError> {
        let _drive = self.targeted_deletion_drive.lock().await;
        let registry = crate::lock_unpoison(&self.targeted_deletion).clone();
        let mut schedule = self.deletion_hold_retry.lock().await;
        crate::targeted_deletion::tick_targeted_deletion(
            &self.store,
            &registry,
            crate::targeted_deletion::TargetedDeletionPass::default(),
            &mut schedule,
        )
        .await
    }

    pub(crate) async fn recover_targeted_deletion_on_startup(&self) -> Result<(), CoreError> {
        let _drive = self.targeted_deletion_drive.lock().await;
        let registry = crate::lock_unpoison(&self.targeted_deletion).clone();
        crate::targeted_deletion::recover_targeted_deletions(
            &self.store,
            &registry,
            crate::targeted_deletion::TargetedDeletionPass::default(),
            crate::targeted_deletion::BOUNDED_DRIVE_PASS_BUDGET,
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn kick_targeted_deletion(&self) {
        let _drive = self.targeted_deletion_drive.lock().await;
        let registry = crate::lock_unpoison(&self.targeted_deletion).clone();
        let _drive_outcome = crate::targeted_deletion::drive_targeted_deletion_until_settled(
            &self.store,
            &registry,
            crate::targeted_deletion::TargetedDeletionPass::default(),
            crate::targeted_deletion::BOUNDED_DRIVE_PASS_BUDGET,
        )
        .await;
    }

    pub(crate) fn task_launcher(
        &self,
    ) -> Option<&std::sync::Arc<dyn crate::task_run::TaskAgentLauncher>> {
        self.task_launcher.get()
    }

    #[cfg(test)]
    #[expect(dead_code, reason = "test synchronization gate")]
    pub(crate) fn arm_task_control_gate(
        &self,
    ) -> std::sync::Arc<crate::task_control::TestTaskControlGate> {
        let gate = std::sync::Arc::new(crate::task_control::TestTaskControlGate::default());
        *crate::lock_unpoison(&self.task_control_gate) = Some(std::sync::Arc::clone(&gate));
        gate
    }

    #[cfg(test)]
    pub(crate) fn test_task_control_gate(
        &self,
    ) -> Option<std::sync::Arc<crate::task_control::TestTaskControlGate>> {
        crate::lock_unpoison(&self.task_control_gate).clone()
    }

    pub async fn run_task_agent<T: ProviderTransport + Send + Sync>(
        &self,
        transport: &T,
        delegation: DelegationId,
    ) -> Result<TaskAgentRunOutcome, TaskAgentRunError> {
        let Some(correspondence) = self
            .store
            .load_delegation(delegation)
            .await
            .map_err(TaskAgentRunError::from)?
        else {
            return Ok(TaskAgentRunOutcome::Refused(
                TaskAgentRunRefusal::MissingDelegation { delegation },
            ));
        };
        let registration = match self
            .task_executions
            .take_reservation(delegation, correspondence.task.task)
        {
            crate::task_run::TakeReservation::Admitted(registration) => registration,
            crate::task_run::TakeReservation::AlreadyRunning => {
                return Ok(TaskAgentRunOutcome::Refused(
                    TaskAgentRunRefusal::ExecutionAlreadyRunning { delegation },
                ));
            }
            crate::task_run::TakeReservation::Unreserved => {
                return Ok(TaskAgentRunOutcome::Refused(
                    TaskAgentRunRefusal::ExecutionUnavailable { delegation },
                ));
            }
        };
        let executor = HostInference::new(&self.store, &self.cred_store, &self.tracker, transport);
        let inference = TaskAgentInferenceAdapter::new(&executor, Some(&registration.cancellation));
        let instructions = OwnerInstructionSource::new(&self.store, &self.store);
        let scrubber = CredentialScrubber {
            refs: &self.store,
            store: &self.cred_store,
        };
        crate::task_run::run_task_agent_execution(
            &self.store,
            &instructions,
            &inference,
            &scrubber,
            crate::task_run::DEFAULT_MAX_TURNS,
            &registration,
        )
        .await
    }

    pub async fn handle_frame(
        &self,
        frame: WireFrame,
        live: LiveInput,
        transport: &impl ProviderTransport,
    ) -> Vec<WireFrame> {
        let (stream_tx, stream_rx) = tokio::sync::mpsc::channel(STREAM_BUFFER_FRAMES);
        let drain = tokio::spawn(async move {
            let mut frames = Vec::new();
            let mut stream_rx = stream_rx;
            while let Some(frame) = stream_rx.recv().await {
                frames.push(frame);
            }
            frames
        });
        let mut sink = stream_tx.clone();
        self.handle_frame_to(frame, live, transport, &mut sink, &stream_tx)
            .await;
        drop(stream_tx);
        drop(sink);
        match drain.await {
            Ok(frames) => frames,
            Err(join) => std::panic::resume_unwind(join.into_panic()),
        }
    }

    pub async fn handle_frame_to(
        &self,
        frame: WireFrame,
        live: LiveInput,
        transport: &impl ProviderTransport,
        sink: &mut dyn FrameSink,
        stream_tx: &tokio::sync::mpsc::Sender<WireFrame>,
    ) {
        if frame.envelope.message_type.0 != frame.payload.message_type() {
            return emit_end(
                sink,
                reject_frame(
                    &frame,
                    &live,
                    RejectKind::UnsupportedMessage,
                    format!("unknown message type {:?}", frame.envelope.message_type.0),
                ),
            );
        }
        let negotiated_version = live.negotiated.as_ref().map(|terms| terms.version);
        match (negotiated_version, frame.envelope.protocol) {
            (Some(want), got) if got != want => {
                return emit_end(
                    sink,
                    reject_frame(
                        &frame,
                        &live,
                        RejectKind::IncompatibleProtocol,
                        format!("version {got:?} outside negotiated version {want:?}"),
                    ),
                );
            }
            (None, got) if got != ProtocolVersion::V1 => {
                return emit_end(
                    sink,
                    reject_frame(
                        &frame,
                        &live,
                        RejectKind::IncompatibleProtocol,
                        format!("version {got:?} without negotiation"),
                    ),
                );
            }
            _ => {}
        }
        match &frame.payload {
            WirePayload::PairingRequest(request) => match live.phase {
                ConnectionPhase::Superseded => emit_end(
                    sink,
                    stale_reject(&frame, &live, "pairing on a superseded connection"),
                ),
                ConnectionPhase::Accepted => {
                    for response in self.pair(&frame, request, &live).await {
                        if sink.emit(response).is_err() {
                            break;
                        }
                    }
                }
                _ => emit_end(
                    sink,
                    invalid_phase_reject(&frame, &live, "pairing outside the accepted phase"),
                ),
            },
            WirePayload::CapabilityAdvertise(advertise) => match live.phase {
                ConnectionPhase::Superseded => emit_end(
                    sink,
                    stale_reject(&frame, &live, "capability on a superseded connection"),
                ),
                ConnectionPhase::Accepted | ConnectionPhase::Paired => {
                    for response in self.advertise(&frame, advertise, &live).await {
                        if sink.emit(response).is_err() {
                            break;
                        }
                    }
                }
                _ => emit_end(
                    sink,
                    invalid_phase_reject(&frame, &live, "capability outside its phase"),
                ),
            },
            WirePayload::AuthProof(proof) => match live.phase {
                ConnectionPhase::Superseded => emit_end(
                    sink,
                    stale_reject(&frame, &live, "auth proof on a superseded connection"),
                ),
                ConnectionPhase::Challenged => {
                    for response in self.verify_proof(&frame, proof, &live).await {
                        if sink.emit(response).is_err() {
                            break;
                        }
                    }
                }
                _ => emit_end(
                    sink,
                    invalid_phase_reject(&frame, &live, "auth proof outside the challenged phase"),
                ),
            },
            WirePayload::AuthChallenge(_) | WirePayload::AuthResult(_) => {}
            WirePayload::SubmitTextInput(submit) => {
                if let Some(refusal) =
                    Self::gate_refusal(&frame, &live, "input on a superseded connection")
                {
                    return emit_end(sink, refusal);
                }
                self.submit_text(&frame, submit, &live, transport, sink, stream_tx)
                    .await;
            }
            WirePayload::ConfirmPresentation(confirm) => {
                if let Some(refusal) =
                    Self::gate_refusal(&frame, &live, "presentation on a superseded connection")
                {
                    return emit_end(sink, refusal);
                }
                for response in self.confirm_presentation(&frame, &live, confirm).await {
                    if sink.emit(response).is_err() {
                        break;
                    }
                }
            }
            WirePayload::HistoryRequest(request) => {
                if let Some(refusal) =
                    Self::gate_refusal(&frame, &live, "history on a superseded connection")
                {
                    return emit_end(sink, refusal);
                }
                for response in self.answer_history(&frame, request, &live).await {
                    if sink.emit(response).is_err() {
                        break;
                    }
                }
            }
            WirePayload::ManagementIntent(intent) => {
                if let Some(refusal) =
                    Self::gate_refusal(&frame, &live, "intent on a superseded connection")
                {
                    return emit_end(sink, refusal);
                }
                for response in self.apply_intent(&frame, intent, &live).await {
                    if sink.emit(response).is_err() {
                        break;
                    }
                }
            }
            WirePayload::ManagementViewRequest(request) => {
                if let Some(refusal) =
                    Self::gate_refusal(&frame, &live, "view on a superseded connection")
                {
                    return emit_end(sink, refusal);
                }
                for response in self.answer_view(&frame, request, &live).await {
                    if sink.emit(response).is_err() {
                        break;
                    }
                }
            }
            WirePayload::DeletionStatusRequest(query) => {
                if let Some(refusal) =
                    Self::gate_refusal(&frame, &live, "deletion status on a superseded connection")
                {
                    return emit_end(sink, refusal);
                }
                for response in self.deletion_status_wire(&frame, &live, query).await {
                    if sink.emit(response).is_err() {
                        break;
                    }
                }
            }
            WirePayload::UndeliveredRequest(request) => {
                if let Some(refusal) = Self::gate_refusal(
                    &frame,
                    &live,
                    "undelivered request on a superseded connection",
                ) {
                    return emit_end(sink, refusal);
                }
                for response in self.request_undelivered(&frame, &live, request).await {
                    if sink.emit(response).is_err() {
                        break;
                    }
                }
            }
            WirePayload::UndeliveredAck(ack) => {
                if let Some(refusal) =
                    Self::gate_refusal(&frame, &live, "undelivered ack on a superseded connection")
                {
                    return emit_end(sink, refusal);
                }
                for response in self.ack_undelivered(&frame, &live, ack).await {
                    if sink.emit(response).is_err() {
                        break;
                    }
                }
            }
            WirePayload::ListTasks(query) => {
                if let Some(refusal) =
                    Self::gate_refusal(&frame, &live, "task list on a superseded connection")
                {
                    return emit_end(sink, refusal);
                }
                for response in self.list_tasks_wire(&frame, &live, query).await {
                    if sink.emit(response).is_err() {
                        break;
                    }
                }
            }
            WirePayload::GetTaskReport(query) => {
                if let Some(refusal) =
                    Self::gate_refusal(&frame, &live, "task report on a superseded connection")
                {
                    return emit_end(sink, refusal);
                }
                for response in self.report_wire(&frame, &live, query).await {
                    if sink.emit(response).is_err() {
                        break;
                    }
                }
            }
            WirePayload::GetReportSource(query) => {
                if let Some(refusal) =
                    Self::gate_refusal(&frame, &live, "report source on a superseded connection")
                {
                    return emit_end(sink, refusal);
                }
                for response in self.report_source_wire(&frame, &live, query).await {
                    if sink.emit(response).is_err() {
                        break;
                    }
                }
            }
            WirePayload::SelectTask(query) => {
                if let Some(refusal) =
                    Self::gate_refusal(&frame, &live, "task selection on a superseded connection")
                {
                    return emit_end(sink, refusal);
                }
                for response in self.select_task_wire(&frame, &live, query).await {
                    if sink.emit(response).is_err() {
                        break;
                    }
                }
            }
            WirePayload::ResumeTask(command) => {
                if let Some(refusal) =
                    Self::gate_refusal(&frame, &live, "resume on a superseded connection")
                {
                    return emit_end(sink, refusal);
                }
                for response in self.resume_task_wire(&frame, &live, command).await {
                    if sink.emit(response).is_err() {
                        break;
                    }
                }
            }
            WirePayload::UsageSummaryRequest(query) => {
                if let Some(refusal) =
                    Self::gate_refusal(&frame, &live, "usage summary on a superseded connection")
                {
                    return emit_end(sink, refusal);
                }
                for response in self.usage_summary_wire(&frame, &live, query).await {
                    if sink.emit(response).is_err() {
                        break;
                    }
                }
            }
            WirePayload::LocalErasureResult(result) => {
                if let Some(refusal) =
                    Self::gate_refusal(&frame, &live, "erasure result on a superseded connection")
                {
                    return emit_end(sink, refusal);
                }
                self.accept_client_erasure_result(&live, result).await;
            }
            _ => {}
        }
    }

    fn gate(frame: &WireFrame, live: &LiveInput) -> GateDecision {
        if live.phase.is_superseded() {
            return GateDecision::Stale;
        }
        if live.paired_device.is_none()
            || !live.connection_known
            || !live.authed
            || frame.envelope.sender.connection_id != Some(live.connection_id)
        {
            return GateDecision::Unpaired;
        }
        GateDecision::Pass
    }

    fn gate_refusal(frame: &WireFrame, live: &LiveInput, detail: &str) -> Option<WireFrame> {
        match Self::gate(frame, live) {
            GateDecision::Pass => None,
            GateDecision::Stale => Some(stale_reject(frame, live, detail)),
            GateDecision::Unpaired => Some(unpaired_close(frame, live)),
        }
    }

    pub(crate) fn round_for(&self, wire: &str) -> Option<RoundId> {
        crate::lock_unpoison(&self.rounds).get(wire).copied()
    }

    pub(crate) fn open_round_for(
        &self,
        connection: &ConnectionWireId,
        companion_key: &str,
    ) -> Option<OpenRound> {
        crate::lock_unpoison(&self.open_rounds)
            .get(&(connection_key(connection), companion_key.to_string()))
            .copied()
    }

    /// Installs an open round for `live`'s connection under the ownership
    /// section (CCT §10.4).
    ///
    /// Returns `false` — installing nothing — when the connection was
    /// superseded or closed before the section: an operation cannot resurrect
    /// an open-round binding after the replacement cleanup removed it.
    pub(crate) fn record_open_round(
        &self,
        live: &LiveInput,
        companion_key: &str,
        open: OpenRound,
    ) -> bool {
        self.with_current_connection(live, || {
            crate::lock_unpoison(&self.open_rounds).insert(
                (
                    connection_key(&live.connection_id),
                    companion_key.to_string(),
                ),
                open,
            );
        })
        .is_some()
    }

    pub(crate) fn drop_open_rounds_for(&self, connection: &ConnectionWireId) {
        let key = connection_key(connection);
        crate::lock_unpoison(&self.open_rounds).retain(|(owner, _), _| owner != &key);
    }

    pub(crate) fn with_current_connection<R>(
        &self,
        live: &LiveInput,
        commit: impl FnOnce() -> R,
    ) -> Option<R> {
        live.authority
            .with_current_connection(&live.connection_id, commit)
    }

    pub(crate) async fn with_current_connection_blocking<R: Send + 'static>(
        &self,
        live: &LiveInput,
        commit: impl FnOnce() -> R + Send + 'static,
    ) -> Option<R> {
        let table = std::sync::Arc::clone(&live.authority);
        let connection = live.connection_id;
        let joined =
            tokio::task::spawn_blocking(move || table.with_current_connection(&connection, commit))
                .await;
        match joined {
            Ok(value) => value,
            Err(join) => std::panic::resume_unwind(join.into_panic()),
        }
    }

    pub(crate) fn wire_for_round(&self, round: &RoundId) -> Option<String> {
        let maps = crate::lock_unpoison(&self.rounds);
        maps.iter()
            .find(|(_, mapped)| mapped.as_raw() == round.as_raw())
            .map(|(wire, _)| wire.clone())
    }

    pub(crate) fn round_wire_or_mint(&self, round: &RoundId) -> RoundWireId {
        let mut maps = crate::lock_unpoison(&self.rounds);
        if let Some((wire, _)) = maps
            .iter()
            .find(|(_, mapped)| mapped.as_raw() == round.as_raw())
        {
            return RoundWireId(wire.clone());
        }
        let wire = RawId::new().as_uuid().to_string();
        maps.insert(wire.clone(), *round);
        RoundWireId(wire)
    }

    pub async fn approve_device(
        &self,
        pending_id: &str,
    ) -> Result<Option<DeviceRecord>, CoreError> {
        let Some(claim) = self.pairing_deliveries.claim(pending_id) else {
            return Ok(None);
        };
        let origin = claim.connection.0.as_hyphenated().to_string();
        let approved = DevicePairingRepository::approve_pending(&self.store, pending_id, &origin)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        let Some((record, secret)) = approved else {
            return Ok(None);
        };
        self.auth_store
            .save_secret(&record.id, &record.descriptor, secret.expose_secret())
            .map_err(|error| CoreError::Store(error.to_string()))?;
        let device_id = record
            .wire
            .parse()
            .map(ene_api::v1::refs::DeviceWireId)
            .map_err(|_| CoreError::Store(String::from("invalid paired device wire id")))?;
        let provision = PairingProvision {
            device_id,
            pairing_secret: PairingProvisionSecret::new(secret.into_inner()),
        };
        claim.queue(provision).map_err(|_| {
            CoreError::Approve(String::from(
                "approval committed but the originating pairing connection is unavailable",
            ))
        })?;
        Ok(Some(record))
    }

    pub async fn pending_devices(&self) -> Result<Vec<ene_credential::PendingPairing>, CoreError> {
        DevicePairingRepository::list_pending(&self.store)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))
    }

    pub(crate) async fn clear_unapproved_pendings(&self) -> Result<(), CoreError> {
        DevicePairingRepository::clear_unapproved_pendings(&self.store)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))
    }

    pub async fn approve_credential(&self, provider: &str, label: &str) -> Result<bool, CoreError> {
        let Ok(credential) = CredentialRef::new(provider, label) else {
            return Ok(false);
        };
        match self.cred_store.with_bearer(&credential, |bearer| {
            self.store
                .approve_credential_with_sweep(provider, label, bearer)
        }) {
            Ok(Ok(approved)) => Ok(approved),
            Ok(Err(error)) => Err(CoreError::Store(error.to_string())),
            Err(_) => Err(CoreError::Store(String::from(
                "credential bearer is not readable; provision the secret before approving",
            ))),
        }
    }

    pub async fn publish_credential(
        &self,
        provider: &str,
        label: &str,
        mutation_id: &str,
        secret: &str,
    ) -> Result<ene_credential::MutationOutcome, CoreError> {
        use ene_credential::{
            ActivationOutcome, CredentialPublicationRepository as _, MutationKind, MutationOutcome,
            MutationPhase, SecretVersionId,
        };

        let Ok(credential) = CredentialRef::new(provider, label) else {
            return Ok(MutationOutcome::Refused);
        };
        let (mutation, fresh) = match self
            .store
            .credential_mutation(mutation_id)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?
        {
            Some(stored)
                if stored.kind == MutationKind::Register
                    && stored.provider == provider
                    && stored.label == label =>
            {
                (stored, false)
            }
            Some(_) => {
                return Err(CoreError::Store(String::from(
                    "the mutation id is already bound to another credential premise",
                )));
            }
            None => {
                let expected_revision =
                    ene_credential::CredentialSetRepository::current_set_revision(&self.store)
                        .await
                        .map_err(|error| CoreError::Store(error.to_string()))?
                        .as_u64();
                let random = Uuid::new_v4();
                let mut bytes = [0_u8; 8];
                bytes.copy_from_slice(&random.as_bytes()[..8]);
                let version = (u64::from_be_bytes(bytes) & (i64::MAX as u64)).max(1);
                (
                    self.store
                        .begin_credential_mutation(
                            mutation_id.to_string(),
                            MutationKind::Register,
                            provider.to_string(),
                            label.to_string(),
                            Some(expected_revision),
                            Some(SecretVersionId::from_u64(version)),
                        )
                        .await
                        .map_err(|error| CoreError::Store(error.to_string()))?,
                    true,
                )
            }
        };
        if let Some(outcome) = mutation.outcome {
            return Ok(outcome);
        }
        if !self.cred_store.supports_versions() {
            let outcome = MutationOutcome::Refused;
            self.store
                .record_credential_mutation_outcome(mutation_id, outcome.clone())
                .await
                .map_err(|error| CoreError::Store(error.to_string()))?;
            return Ok(outcome);
        }
        let os = &self.cred_store;
        let candidate = mutation.candidate_version.ok_or_else(|| {
            CoreError::Store(String::from(
                "credential registration mutation has no candidate version",
            ))
        })?;
        let version = candidate.as_u64();
        if !fresh {
            // This is an interrupted operation whose confirmation session no
            // longer exists. Reconciliation read only (design §5 row 1): the
            // recorded item is inspected so the unknown external write is
            // never repeated, and the outcome stays Unknown whether or not it
            // matches — a match is never activated without a fresh Owner
            // confirmation.
            let _inspection = os.prepare_snapshot(&credential, version);
            return Ok(MutationOutcome::Unknown);
        }
        if mutation.phase != MutationPhase::Prepared {
            return Ok(MutationOutcome::Unknown);
        }
        match os.put_version(&credential, version, secret) {
            Ok(()) | Err(_) => {}
        }
        let snapshot = match os.prepare_snapshot(&credential, version) {
            Ok(snapshot) if snapshot.matches(secret) => snapshot,
            Ok(_) | Err(_) => return Ok(MutationOutcome::Unknown),
        };
        self.store
            .mark_credential_staged(mutation_id, candidate)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        let previous = self
            .store
            .active_credential_version(provider, label)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        let retired_bearer = previous.active.and_then(|retired| {
            os.with_version(&credential, retired.as_u64(), str::to_owned)
                .ok()
        });
        let activation = {
            let _publication = self.credential_publication.lock().await;
            let activation = self
                .store
                .activate_credential(mutation_id, secret, retired_bearer.as_deref())
                .await
                .map_err(|error| CoreError::Store(error.to_string()))?;
            if matches!(activation, ActivationOutcome::Activated { .. }) {
                os.activate(snapshot);
            }
            activation
        };
        match activation {
            ActivationOutcome::Activated { revision, retired } => {
                if let Some(retired) = retired
                    && os.delete_version(&credential, retired.as_u64()).is_ok()
                {
                    self.store
                        .mark_credential_cleaned(provider, label, retired)
                        .await
                        .map_err(|error| CoreError::Store(error.to_string()))?;
                }
                Ok(MutationOutcome::Activated { revision })
            }
            ActivationOutcome::Stale { .. } => Ok(MutationOutcome::Stale),
            ActivationOutcome::AlreadyDecided(outcome) => Ok(outcome),
            ActivationOutcome::Missing => Ok(MutationOutcome::Unknown),
        }
    }

    pub async fn pending_credentials(&self) -> Result<Vec<String>, CoreError> {
        let pending = CredentialApprovalRepository::list_pending(&self.store)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        Ok(pending
            .into_iter()
            .map(|entry| format!("{}:{}", entry.provider, entry.label))
            .collect())
    }

    #[cfg(any(unix, windows))]
    pub(crate) async fn close_connection(
        &self,
        table: &std::sync::Arc<ConnectionTable>,
        connection: ConnectionWireId,
    ) {
        #[cfg(test)]
        let close_gate = crate::lock_unpoison(&self.close_gate).clone();
        #[cfg(test)]
        if let Some(gate) = close_gate {
            gate.pause().await;
        }
        let companion = self.store.ensure_running_companion().await.ok();
        let store = self.store.clone();
        let table = std::sync::Arc::clone(table);
        let joined = tokio::task::spawn_blocking(move || {
            table.note_closed(&connection, |device| {
                if let Some(companion) = companion {
                    note_disconnect_sync(&store, companion, device);
                }
            });
        })
        .await;
        if let Err(join) = joined {
            std::panic::resume_unwind(join.into_panic());
        }
        self.on_connection_closed(&connection);
        let origin = connection.0.as_hyphenated().to_string();
        if DevicePairingRepository::abandon_pending_by_origin(&self.store, &origin)
            .await
            .is_err()
        {
            // Close is already final in memory; startup cleanup will clear an
            // unapproved durable row if this best-effort removal failed.
        }
    }

    pub(crate) fn on_connection_superseded(&self, connection: &ConnectionWireId) {
        self.drop_connection_transient_state(connection);
    }

    pub(crate) fn on_connection_closed(&self, connection: &ConnectionWireId) {
        self.drop_connection_transient_state(connection);
    }

    fn drop_connection_transient_state(&self, connection: &ConnectionWireId) {
        self.pairing_deliveries.remove(connection);
        self.drop_presentation_connection_state(connection);
        self.drop_open_rounds_for(connection);
        self.conversation_tasks
            .drop_first_party_selection_for(connection);
        self.client_transients.note_connection_ended(connection);
    }

    #[cfg(test)]
    #[expect(dead_code, reason = "test gate hook for race testing infrastructure")]
    pub(crate) fn arm_confirm_commit_gate(&self) -> std::sync::Arc<TestGate> {
        let gate = std::sync::Arc::new(TestGate::default());
        *crate::lock_unpoison(&self.confirm_commit_gate) = Some(std::sync::Arc::clone(&gate));
        gate
    }

    #[cfg(test)]
    pub(crate) fn confirm_commit_gate(&self) -> Option<std::sync::Arc<TestGate>> {
        crate::lock_unpoison(&self.confirm_commit_gate).clone()
    }

    #[cfg(test)]
    #[expect(dead_code, reason = "test gate hook for race testing infrastructure")]
    pub(crate) fn disarm_confirm_commit_gate(&self) {
        *crate::lock_unpoison(&self.confirm_commit_gate) = None;
    }

    #[cfg(test)]
    #[expect(dead_code, reason = "test gate hook for race testing infrastructure")]
    pub(crate) fn arm_delivery_evidence_gate(&self) -> std::sync::Arc<TestGate> {
        let gate = std::sync::Arc::new(TestGate::default());
        *crate::lock_unpoison(&self.delivery_evidence_gate) = Some(std::sync::Arc::clone(&gate));
        gate
    }

    #[cfg(test)]
    #[expect(dead_code, reason = "test gate hook for race testing infrastructure")]
    pub(crate) fn disarm_delivery_evidence_gate(&self) {
        *crate::lock_unpoison(&self.delivery_evidence_gate) = None;
    }

    #[cfg(test)]
    pub(crate) fn delivery_evidence_gate(&self) -> Option<std::sync::Arc<TestGate>> {
        crate::lock_unpoison(&self.delivery_evidence_gate).clone()
    }

    #[cfg(test)]
    #[expect(dead_code, reason = "test gate hook for race testing infrastructure")]
    pub(crate) fn arm_close_gate(&self) -> std::sync::Arc<TestCloseGate> {
        let gate = std::sync::Arc::new(TestCloseGate::default());
        *crate::lock_unpoison(&self.close_gate) = Some(std::sync::Arc::clone(&gate));
        gate
    }

    #[cfg(test)]
    #[expect(dead_code, reason = "test gate hook for race testing infrastructure")]
    pub(crate) fn arm_ref_mint_gate(&self) -> std::sync::Arc<TestGate> {
        let gate = std::sync::Arc::new(TestGate::default());
        *crate::lock_unpoison(&self.ref_mint_gate) = Some(std::sync::Arc::clone(&gate));
        gate
    }

    #[cfg(test)]
    pub(crate) fn ref_mint_gate(&self) -> Option<std::sync::Arc<TestGate>> {
        crate::lock_unpoison(&self.ref_mint_gate).clone()
    }

    #[cfg(test)]
    #[expect(dead_code, reason = "test gate hook for race testing infrastructure")]
    pub(crate) fn disarm_ref_mint_gate(&self) {
        *crate::lock_unpoison(&self.ref_mint_gate) = None;
    }

    #[cfg(test)]
    #[expect(dead_code, reason = "test gate hook for race testing infrastructure")]
    pub(crate) fn arm_fetch_gate(&self) -> std::sync::Arc<TestGate> {
        let gate = std::sync::Arc::new(TestGate::default());
        *crate::lock_unpoison(&self.fetch_gate) = Some(std::sync::Arc::clone(&gate));
        gate
    }

    #[cfg(test)]
    pub(crate) fn fetch_gate(&self) -> Option<std::sync::Arc<TestGate>> {
        crate::lock_unpoison(&self.fetch_gate).clone()
    }

    #[cfg(test)]
    #[expect(dead_code, reason = "test gate hook for race testing infrastructure")]
    pub(crate) fn disarm_fetch_gate(&self) {
        *crate::lock_unpoison(&self.fetch_gate) = None;
    }

    #[cfg(test)]
    #[expect(dead_code, reason = "test gate hook for race testing infrastructure")]
    pub(crate) fn arm_resume_gate(&self) -> std::sync::Arc<crate::task_control::TestResumeGate> {
        let gate = std::sync::Arc::new(crate::task_control::TestResumeGate::default());
        *crate::lock_unpoison(&self.resume_gate) = Some(std::sync::Arc::clone(&gate));
        gate
    }

    #[cfg(all(test, unix))]
    #[expect(dead_code, reason = "test observation probe")]
    pub(crate) fn receipt_expiry_runs_for_test(&self) -> usize {
        self.receipt_expiry_runs
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(test)]
    #[expect(dead_code, reason = "test gate hook for race testing infrastructure")]
    pub(crate) fn arm_presentation_commit_gate(
        &self,
    ) -> std::sync::Arc<crate::presentation::TestPresentationCommitGate> {
        let gate = std::sync::Arc::new(crate::presentation::TestPresentationCommitGate::default());
        *crate::lock_unpoison(&self.presentation_commit_gate) = Some(std::sync::Arc::clone(&gate));
        gate
    }
}

#[cfg(any(unix, windows))]
fn note_disconnect_sync(store: &Store, companion: ene_companion::CompanionId, client_ref: &str) {
    let client = device_client(client_ref);
    let Ok(Some(current)) = store.load_attribution_sync(companion.as_raw()) else {
        return;
    };
    if current.state != PresenceState::Present || current.active_client != Some(client) {
        return;
    }
    let expected = PresenceCheckRef {
        expected_generation: current.generation,
        expected_state: current.state,
        expected_active: current.active_client,
    };
    let Ok(MoveDecision::TransitioningToNew { generation }) = store
        .compare_and_begin_transition_sync(
            companion.as_raw(),
            expected,
            None,
            ThinMoveReason::DisconnectObserved,
        )
    else {
        return;
    };
    let premise = LiveReachabilityRef {
        client,
        connection_live: false,
    };
    if store
        .confirm_transition_sync(companion.as_raw(), generation, premise)
        .is_err()
    {
        // The unconfirmed transition stands: the next intake reports held.
    }
}

impl HostHandle {
    pub(crate) async fn normalize_presence_on_startup(&self) -> Result<(), CoreError> {
        let report = self
            .store
            .normalize_on_startup()
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        if report.failures.is_empty() {
            return Ok(());
        }
        Err(CoreError::Store(format!(
            "presence normalization refused {} companion(s): {:?}",
            report.failures.len(),
            report.failures
        )))
    }

    pub async fn note_disconnect(&self, client_ref: &str) {
        self.note_disconnect_with(client_ref, &|| Vec::new()).await;
    }

    pub(crate) async fn note_disconnect_with(
        &self,
        client_ref: &str,
        current: &(dyn Fn() -> Vec<CurrentConnection> + Send + Sync),
    ) {
        let closing = device_client(client_ref);
        let Ok(companion) = self.store.ensure_running_companion().await else {
            return;
        };
        let Ok(Some(fact)) = self.store.load_attribution(companion.as_raw()).await else {
            return;
        };
        if fact.state != PresenceState::Present || fact.active_client != Some(closing) {
            return;
        }
        let candidates = current()
            .into_iter()
            .filter(|connection| connection.client_ref != client_ref)
            .map(|connection| FallbackCandidate {
                client: device_client(&connection.client_ref),
                current_authenticated: true,
                same_machine: connection.same_machine,
                device_permitted: connection.device_permitted,
            })
            .collect::<Vec<_>>();
        let target = select_fallback_candidate(closing, &candidates);
        let expected = PresenceCheckRef {
            expected_generation: fact.generation,
            expected_state: fact.state,
            expected_active: fact.active_client,
        };
        let Ok(MoveDecision::TransitioningToNew { generation }) = self
            .store
            .compare_and_begin_transition(
                companion.as_raw(),
                expected,
                target,
                ThinMoveReason::DisconnectObserved,
            )
            .await
        else {
            return;
        };
        let live = match target {
            Some(client) => {
                let still_current = current().iter().any(|connection| {
                    connection.same_machine
                        && connection.device_permitted
                        && device_client(&connection.client_ref) == client
                });
                LiveReachabilityRef {
                    client,
                    connection_live: still_current,
                }
            }
            None => LiveReachabilityRef {
                client: closing,
                connection_live: false,
            },
        };
        if !matches!(
            self.store
                .confirm_transition(companion.as_raw(), generation, live)
                .await,
            Ok(ConfirmTransitionOutcome::Confirmed(_))
        ) {}
    }
}

use ene_plugin_ipc::WireFrame;
