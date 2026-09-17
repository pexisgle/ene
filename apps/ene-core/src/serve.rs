//! `Stage 2` Host composition: errors, handle, dispatch, handshake, entry point.
//!
//! [`HostHandle`] is the testable seam: it owns the durable [`ene_store::Store`],
//! the [`EvaluationTracker`], and the per-process Host maps, and
//! [`HostHandle::handle_frame`] runs the full orchestration pipeline over one
//! [`WireFrame`] without touching any socket. [`serve`] wires a handle to the
//! [`crate::conn`] listener with the production inference transport.
//!
//! Trust premises:
//!
//! - The data directory is created by [`HostHandle::open_with_cred_store`] with
//!   mode `0700` on Unix (`Stage 2` owns directory creation). The same-machine
//!   trust premise rests on that directory plus the per-connection same-user
//!   check in [`crate::conn`], never on a Client self-report.
//! - One connection lifecycle boundary owns every Host-memory entry tied to a
//!   connection lifetime (`on_connection_superseded` / `on_connection_closed`
//!   → presentation state, open rounds, first-party Task selection): a newer
//!   authentication for the same device supersedes the old connection
//!   irreversibly, the old socket stays only for typed stale rejections, and
//!   the replacement inherits none of the old connection's transient world.
//!   Streams re-check the connection table before every publication, so no
//!   registry exists for them.
//! - Pairing is Owner-confirmed through the durable
//!   [`DevicePairingRepository`]: a request records a pending entry, the
//!   Host-local `approve-device` inlet records the Owner decision, and a later
//!   request for the approved descriptor issues the device key. There is no
//!   same-descriptor auto-approve: an unapproved descriptor always answers
//!   [`PendingOwnerConfirmation`](ene_api::v1::handshake::PairingResult::PendingOwnerConfirmation).
//! - Presence attach happens only on the
//!   [`SubmitTextInput`](ene_api::v1::round::SubmitTextInput) path: capability
//!   and management frames never attach. Socket close decides and commits the
//!   [`DisconnectObserved`](ene_presence::ThinMoveReason::DisconnectObserved)
//!   fallback in one connection-table section through
//!   `HostHandle::close_connection` (CCT §10.4): the fallback runs only when
//!   the closing connection was still the device's current authenticated one.
//! - Domain ingress and the presence reachability premise use only a
//!   connection that is authenticated, current for its device, open, and
//!   paired: the connection table decides that (IPC §9.3, #1384), never a
//!   paired-socket count or a past authentication. A superseded connection
//!   answers typed `StaleConnection` rejections with the socket kept open
//!   (IPC §11.3).
//! - Presence `active_client` names a device through `device_client`, a
//!   deterministic `UUID v5` mapping rather than issuance.
//! - Authentication is challenge/proof over the pairing secret. Challenge
//!   nonces live in the connection table in memory only, so a restart fails
//!   closed; pairing secrets live only in `device-auth.json` (plus the
//!   transient approve-time display scope), with no secret cache.
//! - The response sender reveals the connection id only on and after
//!   [`Accepted`](ene_api::v1::handshake::AuthResult::Accepted): every
//!   pre-accept response carries [`None`], so a peer that never completed the
//!   challenge never learns the id the ingress gate requires it to echo.
//! - The single-writer `host.lock` ([`crate::host_lock`]) is taken before the
//!   state open, so a second Host in the same data directory is refused
//!   before any startup mutation (PR §6.4).
//! - [`HostHandle::handle_frame`] is infallible by contract: infrastructure
//!   failures map to retry-safe outcome frames (hold or revalidate), never to
//!   fabricated domain facts.
//! - Decoded-but-unhandled inbound variants (reconnect, Client stream frames,
//!   facts the Host itself emits) are ignored with an empty response: they are
//!   known [`WirePayload`] variants outside `Stage 2` scope, and a
//!   [`ene_api::v1::handshake::DisconnectNotice`] would carry the wrong
//!   semantics for them. Silence is the explicit `Stage 2` decision.
//! - The envelope discriminator must name the decoded payload:
//!   [`HostHandle::handle_frame`] answers a typed
//!   [`Reject`](ene_api::v1::payload::WirePayload::Reject) with
//!   `UnsupportedMessage` when they differ. A future/unknown payload variant
//!   cannot reach that reject: [`WirePayload`] is a closed enum decoded as part
//!   of the whole frame, so the codec fails first and [`crate::conn`] closes
//!   the connection.
//! - [`HostHandle`] methods take `&self`: every lock guard is dropped before
//!   the next await, and no handle-wide async lock spans provider I/O.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::OnceLock;

use ene_api::v1::envelope::ProtocolVersion;
use ene_api::v1::handshake::NegotiatedConnection;
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{ConnectionWireId, RoundWireId};
use ene_api::v1::reject::RejectKind;
use ene_companion::{CompanionId, CompanionRepository};
use ene_credential::{
    CredentialApprovalRepository, CredentialRef, CredentialRefRepository as _, CredentialStore,
    CredentialTechnicalError, DevicePairingRepository, DeviceRecord, EnvCredentialStore,
    FileDeviceAuthStore, MemoryCredentialStore,
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
    invalid_phase_reject, outgoing_envelope, outgoing_fact, outgoing_frame, reject_frame,
    stale_reject, unpaired_close,
};
pub(crate) use handshake::attribution_to_wire;
pub use lifecycle::serve;

/// Binary-local Host failure.
///
/// Messages carry operational notes only: no secrets, no body text, and no
/// paths (which stay out for operational brevity, matching the store
/// convention). Infrastructure variants name the failing layer; denial and
/// staleness are domain outcomes on the wire, never this error.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("store unavailable: {0}")]
    Store(String),
    /// Another Host already holds the single-writer lock for this data
    /// directory (PR §6.4); the loser refuses startup before any mutation.
    #[error("another Host is already running for this data directory")]
    AlreadyRunning,
    #[error("bind failed: {0}")]
    Bind(String),
    #[error("inference failed: {0}")]
    Inference(String),
    /// Unknown descriptors list the pending descriptors so the Owner can
    /// retry with the exact value; the message carries display strings only.
    #[error("device approval failed: {0}")]
    Approve(String),
    /// Targeted Deletion composition failure: an invalid fan-out pass, a
    /// duplicate participant registration, or a canonical preservation
    /// refusal. Also carries Host-local inlet failures (malformed status
    /// cursor or limit, unreadable status). Never a global-completion claim
    /// and never a participant fact; messages carry no target body.
    #[error("targeted deletion failed: {0}")]
    Deletion(String),
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(&'static str),
}

/// Where Host responses go at the point they are decided.
///
/// The connection loop forwards each frame to the socket as it is emitted,
/// which is what makes an early accept and incremental provider deltas
/// observable before the host future completes. Control frames `try_send`
/// on the bounded stream channel; the open stream's deltas pace the
/// provider through real backpressure on the same channel.
pub trait FrameSink: Send {
    /// Publishes to a bounded in-memory queue synchronously. Implementations
    /// must not perform socket I/O, block, or re-enter connection ownership:
    /// callers may hold the connection table while publishing control frames.
    fn emit(&mut self, frame: WireFrame) -> Result<(), FrameDeliveryError>;
}

/// How one control-frame delivery ended.
///
/// Closed means the connection is gone: later sends stop, the operation is
/// never treated as delivered, and already-durable state stays untouched.
/// Full means the control allowance broke (each call emits only a handful
/// on a fresh channel far below the bound): never a silent drop, never a
/// normal completion — the operation ends as a delivery failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "a failed delivery must stop the operation, never drop silently"]
pub enum FrameDeliveryError {
    Closed,
    Full,
}

/// Emits one terminal control frame, ending the operation.
/// A delivery failure ends it identically: the frame never reached the
/// client, so no outcome is treated as delivered and nothing further
/// emits. Durable state already committed stays untouched.
pub(crate) fn emit_end(sink: &mut dyn FrameSink, frame: WireFrame) {
    if sink.emit(frame).is_err() {
        // Gone or full channel: the operation ends undelivered either way.
    }
}

/// Queued frames per connection between the Host and the socket writer.
///
/// Bounds slow-client memory: provider-paced deltas backpressure through
/// this capacity (the stream gate awaits room) instead of accumulating
/// without limit, while control frames stay far below it — each call emits
/// only a handful on a fresh channel, so `try_send` there fails only when
/// the receiver is gone.
pub(crate) const STREAM_BUFFER_FRAMES: usize = 32;

impl FrameSink for tokio::sync::mpsc::Sender<WireFrame> {
    fn emit(&mut self, frame: WireFrame) -> Result<(), FrameDeliveryError> {
        use tokio::sync::mpsc::error::TrySendError;

        match self.try_send(frame) {
            Ok(()) => Ok(()),
            // The receiver is gone because the connection is closing; the
            // host future is dropped with it and there is nowhere to write.
            Err(TrySendError::Closed(_)) => Err(FrameDeliveryError::Closed),
            // Control frames are O(1) per call on a fresh channel while the
            // stream gate paces deltas with real backpressure, so reaching
            // this means the control allowance was exceeded.
            Err(TrySendError::Full(_)) => Err(FrameDeliveryError::Full),
        }
    }
}

/// Bearer store behind the Host handle.
///
/// [`CredentialStore::with_bearer`] is generic over its closure return type,
/// so the trait is not dyn-compatible and the handle holds this closed enum
/// instead of a trait object. [`CredStore::Env`] is the production store for
/// the `openai` provider (the bearer is read from the process environment
/// once at Host startup and pinned in memory for the run, never re-read);
/// [`CredStore::Memory`] is the test and local-development store.
#[derive(Debug)]
pub enum CredStore {
    Env(EnvCredentialStore),
    Memory(MemoryCredentialStore),
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
        }
    }

    fn contains(&self, cred: &CredentialRef) -> bool {
        match self {
            Self::Env(inner) => inner.contains(cred),
            Self::Memory(inner) => inner.contains(cred),
        }
    }
}

/// Transport-free liveness and authorization premises for one inbound frame.
///
/// The connection layer builds this per frame from its per-connection table:
/// `client_ref` names the calling client opaquely (device key when paired,
/// otherwise the incarnation pair), `connection_live` carries the out-of-band
/// reachability premise, `peer_uid_ok` carries the same-user proof for this
/// connection, `paired_device` carries the device wire string the connection
/// table bound on this connection (if any), `connection_known` reports
/// whether the connection table knows this connection at all, `authed`
/// reports whether this connection completed the challenge/proof exchange and
/// is still the device's current authenticated connection (a newer
/// authentication by the same device supersedes it), and `connection_id` is
/// the table key itself. `phase` is the connection's one-way phase snapshot
/// (IPC §9.3): the gate answers a typed `StaleConnection` for superseded
/// connections and only then treats the remaining premises as a terminal
/// unpaired-close decision. `authority` is the table handle the handshake
/// paths use to re-check phase, consume the nonce, and install currentness
/// inside one short section, and the close admission uses it to serialize the
/// presence compare/commit (CCT §10.4). The gate in
/// [`HostHandle::handle_frame`] trusts these conn-filled premises; direct
/// handle callers (tests) construct them explicitly through the table.
#[derive(Debug, Clone)]
pub struct LiveInput {
    pub client_ref: String,
    pub connection_live: bool,
    pub peer_uid_ok: bool,
    pub paired_device: Option<String>,
    pub connection_known: bool,
    pub authed: bool,
    pub connection_id: ConnectionWireId,
    /// Negotiated terms recorded when this connection answered capability.
    ///
    /// Filled by the connection table from the Host-selected terms, never by
    /// the Client: later frames must match this version, so version mixing
    /// within one connection is impossible. [`None`] before negotiation.
    pub negotiated: Option<NegotiatedConnection>,
    /// One-way connection phase snapshot for gate decisions.
    pub phase: ConnectionPhase,
    /// The connection table behind these premises.
    ///
    /// Crate-internal: the connection layer mints it with the connection, so
    /// external callers cannot fabricate a table-backed connection.
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

/// What the domain gate decided for one frame (IPC §11.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateDecision {
    /// The frame may proceed to its domain handler.
    Pass,
    /// The connection was superseded: answer a typed `StaleConnection` and
    /// keep the socket open.
    Stale,
    /// The connection cannot serve domain frames: answer the terminal
    /// unpaired close.
    Unpaired,
}

/// Test-only gate that pauses an operation's ownership section (S5-05).
///
/// The gate is entered before the connection-table section, so a test can let
/// a competing authentication install complete while the operation is paused
/// and observe that the operation then re-reads currentness instead of acting
/// on a stale snapshot.
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
    /// Pauses until the test releases the gate, marking entry first.
    pub(crate) async fn pause(&self) {
        self.entered.add_permits(1);
        let permit = self.release.acquire().await.expect("gate stays open");
        permit.forget();
    }

    /// Waits until a paused operation has entered the gate.
    pub(crate) async fn wait_entered(&self) {
        let permit = self.entered.acquire().await.expect("gate is entered");
        permit.forget();
    }

    /// Releases one paused operation.
    pub(crate) fn release(&self) {
        self.release.add_permits(1);
    }
}

/// The close-admission gate keeps its historical name and shape.
#[cfg(test)]
pub(crate) type TestCloseGate = TestGate;

/// Wire-string key for one connection id, shared by every per-connection
/// Host-memory map (presentation state, open rounds, resume epochs): the same
/// string form the connection layer uses, so keys match across the
/// Host/connection boundary by construction.
pub(crate) fn connection_key(id: &ConnectionWireId) -> String {
    id.0.as_hyphenated().to_string()
}

/// Maps a paired device wire string to its per-process [`ClientId`].
///
/// Deterministic mapping, not issuance: `UUID v5` over the device string, so
/// the same device always maps to the same client within and across
/// processes (`Stage 2` runs one client per device). The store persists the
/// resulting [`ClientId`] as the presence `active_client`; after a restart
/// the same device re-derives the same id and re-attaches through the submit
/// path rather than inheriting a stale mapping.
pub(crate) fn device_client(device_wire: &str) -> ClientId {
    ClientId::from_raw(RawId::from_uuid(Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        device_wire.as_bytes(),
    )))
}

/// One currently authenticated connection as the presence fallback sees it.
///
/// The real connection table lives in [`crate::conn`] (slice A), which owns
/// authentication, supersede, and close admission. The fallback below receives
/// a snapshot source instead of reading the table itself, so the connection
/// layer decides who is current and presence never guesses it. The serving
/// close path ([`HostHandle::close_connection`]) admits through the table's
/// own section and answers `NoActive`; callers that already hold a
/// table-derived snapshot (for example [`crate::conn::ConnectionTable`]
/// polled via [`ConnectionTable::current_authenticated`](crate::conn::ConnectionTable::current_authenticated))
/// pass it here so an eligible same-machine candidate can take the fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CurrentConnection {
    /// Paired device wire string of the current authenticated connection.
    pub client_ref: String,
    /// Transport classification: same-machine (verified OS peer) or remote.
    pub same_machine: bool,
    /// Device permission currently allows client-dependent activity.
    pub device_permitted: bool,
}
/// `Stage 2` Host handle: durable store, evaluation tracker, and Host maps.
///
/// Interior mutability: `open_rounds` and `rounds` sit behind short `std`
/// mutex sections (clone out before every await, never hold a guard across
/// an await); `tracker` is a leaf async mutex (the inference dispatch
/// boundary needs `&mut` across its transport await while only touching
/// the tracker synchronously up front, and the transport never calls back into the
/// handle, so no lock ordering exists); the [`Store`] carries its own lock.
/// Nothing here is durable except through [`Store`] and the device-auth file:
/// a restart drops every map while the database persists, and old wire round
/// refs then surface as stale (never rebound). The device tables and the
/// history `local_id` column are durable in [`Store`], and pairing secrets in
/// `device-auth.json` through the `auth_store` field.
///
/// Map keys: `open_rounds` is keyed by `(connection key, companion key)`, so an
/// open round belongs to exactly one connection lifetime and a same-device
/// replacement can never join it (IPC §9.3 replacement rule); round refs
/// issued on the wire resolve back through `rounds` (wire string to domain
/// round). Challenge nonces and currentness live in the connection layer's
/// `ConnectionTable`, not here.
pub struct HostHandle {
    pub(crate) store: Store,
    pub(crate) tracker: AsyncMutex<EvaluationTracker>,
    pub(crate) open_rounds: StdMutex<HashMap<(String, String), OpenRound>>,
    pub(crate) rounds: Arc<StdMutex<HashMap<String, RoundId>>>,
    pub(crate) cred_store: CredStore,
    /// File-backed pairing-secret store by device, opened on
    /// `<data_dir>/device-auth.json`.
    ///
    /// Secrets live here and in the transient approve-time display scope only:
    /// the handle keeps no secret map and no cache. See the
    /// [`FileDeviceAuthStore`] contract for custody, file protection, and the
    /// backup-exclusion rule.
    pub(crate) auth_store: FileDeviceAuthStore,
    /// In-memory, best-effort queue of pinned Experience premises whose
    /// completed replies await a Learning formation pass.
    ///
    /// Each item carries its own source range and transcript, pinned at reply
    /// completion. See
    /// [`crate::dialogue`]: the pass is post-response work, never a condition
    /// of the client-visible completion, and a crash simply drops the queued
    /// derived update instead of replaying an old pass. Shared with the
    /// Host-transient erasure participant (A3c), which drops covered premises.
    pub(crate) learning_queue: Arc<StdMutex<VecDeque<ene_learning::ExperienceCandidate>>>,
    /// Serializes Learning formation passes for this handle so overlapping
    /// drains cannot run two passes over one companion at once.
    pub(crate) learning_worker: AsyncMutex<()>,
    /// Opaque companion projection issued by this handle.
    ///
    /// The domain wire-ref mapping for the single Stage 2 companion: every
    /// outbound presence/companion ref renders this string, and every
    /// inbound companion ref resolves through
    /// [`HostHandle::resolve_companion`] — exact match against this value,
    /// never parsed, never derived. Minted fresh per handle (restarts
    /// rotate it; Clients relearn it from the next presence fact and
    /// converge through revalidation), so the projection is a genuine
    /// Host-owned mapping entry rather than a function of the domain id.
    pub(crate) companion_wire: String,
    /// Cooperative stop tokens for running Task Agent executions, keyed by
    /// delegation (one execution lifetime; registration is atomic per
    /// delegation). In-memory only: a restart drops every token, and a lost
    /// token never means the durable work or an external effect stopped. The
    /// durable attempt facts carry the one-shot start marker across restarts.
    pub(crate) task_executions: std::sync::Arc<crate::task_run::TaskExecutionRegistry>,
    /// Transient conversation projection of the Task each dialogue is working
    /// on, keyed by Companion.
    ///
    /// In-memory only and never authority: task control directives from the
    /// conversation resolve their target through this projection, while every
    /// operation still goes through the Task owner's durable compare. A
    /// restart drops it (restart continuation is Stage 5).
    pub(crate) conversation_tasks: crate::task_control::ConversationTaskProjection,
    /// Host-memory presentation subscriptions, receipts, query-scoped refs,
    /// cursors, and resume retry-epoch slots (IPC §13.3, §18.2).
    ///
    /// Short `std` mutex sections only (clone out before every await, never
    /// hold across an await); transitions serialize on
    /// [`HostHandle::presentation_lock`]. Restart drops all of it while the
    /// database persists.
    pub(crate) presentations: Arc<StdMutex<crate::presentation::PresentationState>>,
    /// Serializes presentation begin/ack transitions (CCT §10.5). Held only
    /// across short store roundtrips, never across provider I/O.
    pub(crate) presentation_lock: AsyncMutex<()>,
    /// Trusted first-party Task premises (the Owner-selected Workspace).
    ///
    /// In-memory only and never provider output: the model can propose a Task
    /// but can never supply the filesystem authority it runs under. Restart
    /// re-selection is Stage 5.
    pub(crate) trusted_task_premises: crate::task_control::TrustedTaskPremises,
    /// The serving process's Task Agent launcher, installed once by
    /// [`crate::conn::run`] with the shared handle and provider transport.
    ///
    /// A handle without an installed launcher (unit tests) accepts Task
    /// creation but starts no execution; production always installs one.
    pub(crate) task_launcher: OnceLock<std::sync::Arc<dyn crate::task_run::TaskAgentLauncher>>,
    /// Host-composition erasure participants keyed by owner (lifecycle §9).
    ///
    /// Holds implementations only; an owner without one is driven as an
    /// explicit unsupported participant. In-memory and never authority: the
    /// durable `deletion_participant` snapshot persists across restart while a
    /// reopening composition re-registers its implementations before driving,
    /// so a restart can never turn a missing implementation into completion.
    pub(crate) targeted_deletion: StdMutex<crate::targeted_deletion::ErasureParticipantRegistry>,
    /// Invalidation fence for in-flight Host transient payloads (A3c).
    ///
    /// Bumped by the Host-transient erasure demand; a dialogue stream or
    /// assembled reply that started before the bump can no longer prove its
    /// payload is uncovered and fails closed. Holds no deletion condition, so
    /// it is not a second currentness registry.
    pub(crate) transient_fence: Arc<crate::transient_erasure::TransientErasureFence>,
    /// Client incarnations the Host handed body-bearing material to, plus the
    /// in-flight local-erasure demand plumbing (A3c, lifecycle §8.1).
    ///
    /// Host-memory only: restart drops the tracking and every connection, so
    /// an incarnation the Host cannot name is never claimed as required.
    pub(crate) client_transients: Arc<crate::transient_erasure::ClientTransientRegistry>,
    /// Test-only deterministic gate for conversation task-control commands.
    #[cfg(test)]
    pub(crate) task_control_gate:
        StdMutex<Option<std::sync::Arc<crate::task_control::TestTaskControlGate>>>,
    /// Test-only deterministic gate for the close-admission section.
    #[cfg(test)]
    pub(crate) close_gate: StdMutex<Option<std::sync::Arc<TestCloseGate>>>,
    /// Test-only deterministic gate before a Client-dependent submit's
    /// acceptance (owner append) section.
    #[cfg(test)]
    pub(crate) submit_accept_gate: StdMutex<Option<std::sync::Arc<TestGate>>>,
    /// After Owner commit, before open-round installation.
    #[cfg(test)]
    pub(crate) submit_open_gate: StdMutex<Option<std::sync::Arc<TestGate>>>,
    /// After installation, before control-frame publication.
    #[cfg(test)]
    pub(crate) submit_publish_gate: StdMutex<Option<std::sync::Arc<TestGate>>>,
    /// Test-only pause after a confirmation read and before durable commit.
    #[cfg(test)]
    pub(crate) confirm_commit_gate: StdMutex<Option<std::sync::Arc<TestGate>>>,
    /// Test-only deterministic gate before a read query's connection-scoped
    /// ref/cursor mint.
    #[cfg(test)]
    pub(crate) ref_mint_gate: StdMutex<Option<std::sync::Arc<TestGate>>>,
    /// Test-only deterministic gate before a presentation pass takes the
    /// begin/ack transition lock.
    #[cfg(test)]
    pub(crate) fetch_gate: StdMutex<Option<std::sync::Arc<TestGate>>>,
    /// Test-only deterministic gate for one guarded wire resume.
    #[cfg(test)]
    pub(crate) resume_gate: StdMutex<Option<std::sync::Arc<crate::task_control::TestResumeGate>>>,
    /// Test-only deterministic gate for one presentation-start commit.
    #[cfg(test)]
    pub(crate) presentation_commit_gate:
        StdMutex<Option<std::sync::Arc<crate::presentation::TestPresentationCommitGate>>>,
    /// Test-only count of receipt-expiry housekeeping runs, so a test can
    /// pin that an expired receipt is released (and the deadline stops
    /// re-firing) without measuring CPU or sleeping for ordering. Unix-gated
    /// with the socket-loop tests that read it; the Windows lib test build
    /// would otherwise flag it as dead code under warnings-as-errors.
    #[cfg(all(test, unix))]
    pub(crate) receipt_expiry_runs: std::sync::atomic::AtomicUsize,
}

impl HostHandle {
    /// Opens (or creates) the Host state under `data_dir` with the production
    /// credential store.
    ///
    /// Ensures `data_dir` exists (`0700` on Unix) and opens `app.db` inside it
    /// through [`Store::open`]. `Stage 2` owns directory creation: resolution
    /// stays pure in `ene-config` while the side effect lives here.
    ///
    /// This is the state open, not the serving boundary: it performs no
    /// credential sweep and changes no durable state. Callers that serve
    /// requests run `sweep_registered_values` first; read-only
    /// and local management paths (pending device lists, device approval)
    /// open without touching registered credential content or the
    /// credential-set revision.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Store`] when the directory cannot be ensured or
    /// the database cannot be opened or migrated.
    pub async fn open(data_dir: &Path) -> Result<Self, CoreError> {
        Self::open_with_cred_store(data_dir, CredStore::Env(EnvCredentialStore::new())).await
    }

    /// Runs the serving startup mutations in production order (PR §6.4):
    /// presence normalization, unapproved-pairing cleanup, credential sweep,
    /// sealed-result reconciliation, and orphaned usage-reservation
    /// reconciliation. Normalization goes first because
    /// every client-dependent admission depends on it, while the sweep and
    /// reconciliation do not; unapproved pendings never survive a restart
    /// (paired records are untouched); the sweep keeps the Host from serving
    /// content prepared under an unknown credential set; reconciliation
    /// neither resumes an execution nor replays a provider call or Action,
    /// and a still-blocked result stays withheld. Orphaned reservations
    /// settle `CommittedUnknown` (`usage-cost-cap` §15): a crash never
    /// releases a usage slot and never resets consumption to zero. The
    /// [`crate::serve::lifecycle::serve`] entry point runs this
    /// between the store open and the listener bind; Host-integration tests
    /// run it to restart faithfully without a second listener. Like
    /// [`crate::serve::lifecycle::serve`], any refusal fails startup:
    /// the Host must not serve with an unknown presence state or unreadable
    /// result state.
    ///
    /// # Errors
    ///
    /// [`CoreError::Store`] when normalization, the pairing cleanup, the
    /// sweep, the sealed-result reconciliation, or the usage-reservation
    /// reconciliation cannot complete.
    pub async fn run_startup_mutations(&self) -> Result<(), CoreError> {
        self.normalize_presence_on_startup().await?;
        self.clear_unapproved_pendings().await?;
        self.sweep_registered_values().await?;
        self.reconcile_sealed_results()
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        self.store
            .reconcile_orphaned_usage_reservations()
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        Ok(())
    }

    /// Opens (or creates) the Host state under `data_dir` with an explicit
    /// credential store.
    ///
    /// Integration tests pass [`CredStore::Memory`] pre-provisioned with test
    /// bearers to stay hermetic (the environment store reads the real process
    /// environment once when it is constructed). The device-auth file opens on
    /// `<data_dir>/device-auth.json` (created lazily on first approval) after
    /// the data directory is ensured, so the open always has its parent.
    /// Like [`HostHandle::open`], the state open itself has no credential
    /// side effects; the serving boundary is a separate, explicit step.
    ///
    /// # Errors
    ///
    /// [`CoreError::Store`] as in [`HostHandle::open`], plus when the
    /// device-auth file cannot be opened (unreadable, malformed, or wrongly
    /// permissioned).
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
        let learning_queue = Arc::new(StdMutex::new(VecDeque::new()));
        let transient_fence = Arc::new(crate::transient_erasure::TransientErasureFence::default());
        let client_transients = Arc::new(crate::transient_erasure::ClientTransientRegistry::new());
        let handle = Self {
            store,
            tracker: AsyncMutex::new(EvaluationTracker::new()),
            open_rounds: StdMutex::new(HashMap::new()),
            rounds: Arc::new(StdMutex::new(HashMap::new())),
            cred_store,
            auth_store,
            learning_queue: Arc::clone(&learning_queue),
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
            transient_fence: Arc::clone(&transient_fence),
            client_transients,
            #[cfg(test)]
            task_control_gate: StdMutex::new(None),
            #[cfg(test)]
            close_gate: StdMutex::new(None),
            #[cfg(test)]
            submit_accept_gate: StdMutex::new(None),
            #[cfg(test)]
            submit_open_gate: StdMutex::new(None),
            #[cfg(test)]
            submit_publish_gate: StdMutex::new(None),
            #[cfg(test)]
            confirm_commit_gate: StdMutex::new(None),
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
        // The composition root owns the concrete participant mapping: the
        // current product surface's local owners are registered before the
        // handle is handed out, so a fan-out never sees an admitted
        // requirement whose implementation this process simply forgot to add.
        handle.install_local_erasure_participants()?;
        Ok(handle)
    }

    /// Registers the local-erasure implementations of the current product
    /// surface (lifecycle §9).
    ///
    /// This is the composition seam: `ene-preservation` owns the trait and
    /// never depends on a concrete participant crate, and each implementation
    /// lives with its owner's durable master. An owner without an
    /// implementation stays an explicit unsupported hold; this method only
    /// adds the implementations that exist.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Deletion`] if an owner already has an
    /// implementation, which would make the fan-out nondeterministic.
    fn install_local_erasure_participants(&self) -> Result<(), CoreError> {
        use std::sync::Arc;

        let participants: [Arc<dyn ene_preservation::ErasureParticipant>; 9] = [
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
            Arc::new(crate::transient_erasure::HostTransientParticipant::new(
                self.transient_fence.clone(),
                self.presentations.clone(),
                self.learning_queue.clone(),
            )),
        ];
        for participant in participants {
            self.register_deletion_participant(participant)?;
        }
        Ok(())
    }

    /// Startup credential boundary: sweeps every registered pinned value out
    /// of durable content and advances the revision once.
    ///
    /// The serving Host runs this before accepting any request, after the
    /// state open and before the listener binds. Every registered value must
    /// be readable: an unreadable value fails the boundary instead of
    /// skipping the sweep, because absence of the value cannot be proven and
    /// the Host must not serve content that may still hold it in plaintext.
    /// A failure leaves the handle unusable for serving (the caller returns
    /// without binding). Read-only and local management paths never call this:
    /// they must not move the credential-set revision.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Store`] when the registered refs cannot be read
    /// or a value cannot be read and the sweep therefore cannot complete.
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

    /// Resolves an inbound companion wire ref to its domain companion.
    ///
    /// Only the projection this handle issued resolves; any other string is
    /// unknown (`Ok(None)`), never guessed or derived, and callers answer
    /// `unknown-companion` revalidation (or an empty view), so a rotated
    /// projection (restart) converges through one round trip. Store failures
    /// stay errors (the caller holds), distinct from unknown refs.
    pub(crate) async fn resolve_companion(
        &self,
        wire: &str,
    ) -> Result<Option<CompanionId>, ene_companion::CompanionTechnicalError> {
        if wire != self.companion_wire.as_str() {
            return Ok(None);
        }
        self.store.ensure_running_companion().await.map(Some)
    }

    /// Accepts one Task cancel request and cooperatively stops its running
    /// execution (AU16).
    ///
    /// The durable admission commits first and is the only authority; the
    /// local signal afterwards is best-effort. A refused admission
    /// (`AlreadyCancelled`, `TaskTerminal`, `MissingTask`) signals nothing,
    /// because no execution of a terminal Task may still be starting work.
    /// The returned outcome is the Task owner's unchanged domain answer; it
    /// does not claim that any provider request or external effect stopped.
    ///
    /// # Errors
    ///
    /// [`TaskTechnicalError`] when the admission commit cannot answer.
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

    /// Installs the serving process's Task Agent launcher.
    ///
    /// [`crate::conn::run`] owns the shared handle and provider transport and
    /// installs exactly one launcher, so a conversation-accepted delegation
    /// starts the existing runner in the background. Returns `false` when a
    /// launcher was already installed; a handle opened outside a serving
    /// composition simply has none.
    pub fn install_task_launcher(
        &self,
        launcher: std::sync::Arc<dyn crate::task_run::TaskAgentLauncher>,
    ) -> bool {
        self.task_launcher.set(launcher).is_ok()
    }

    /// Registers one erasure participant implementation and the owner it
    /// serves (lifecycle §9).
    ///
    /// This is the composition seam A3 slices use: `ene-preservation` owns the
    /// trait and never depends on a concrete participant crate, so the Host
    /// composition is the only place that learns them. Registering a second
    /// implementation for one owner is refused: a nondeterministic fan-out
    /// could record a fact that does not describe the demanded owner.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Deletion`] when the owner already has one.
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

    /// Required participant snapshot for a newly admitted operation: the
    /// current product surface's semantic owners (lifecycle §8).
    ///
    /// The set is snapshotted durably with the operation admission; later
    /// changes in the registered implementations never widen an admitted
    /// operation, and an owner with no implementation is still required.
    /// Client incarnations the Host handed body-bearing material to are
    /// appended here, so a Client that may hold a target-bearing local copy is
    /// snapshotted as required at admission and later driven (or held when
    /// unreachable) — an incarnation the Host cannot name is never invented
    /// (lifecycle §8.1).
    #[must_use]
    pub fn required_deletion_participants(&self) -> Vec<ene_preservation::ParticipantOwnerRef> {
        let mut owners = crate::targeted_deletion::current_product_surface_owners();
        owners.extend(
            self.client_transients
                .tracked_incarnations()
                .into_iter()
                .map(ene_preservation::ParticipantOwnerRef::ClientIncarnation),
        );
        owners
    }

    /// Installs the serving composition's connection table as the authority
    /// for Client-incarnation reachability (lifecycle §8.1).
    ///
    /// A handle opened without a serving composition has no table: every
    /// Client demand is then an explicit unreachable hold instead of a
    /// guessed delivery path.
    pub(crate) fn install_client_connection_table(&self, table: std::sync::Arc<ConnectionTable>) {
        self.client_transients.install_connection_table(table);
    }

    /// The current Host transient erasure fence epoch (A3c).
    ///
    /// A dialogue stream or assembled reply captures it at start and fails
    /// closed when it moved: no transient payload published across the move
    /// can prove it is uncovered.
    #[must_use]
    pub(crate) fn transient_fence_epoch(&self) -> u64 {
        self.transient_fence.epoch()
    }

    /// Test-only: empties the erasure-participant registry.
    ///
    /// The built-in composition registers one implementation per served
    /// owner; a fan-out test that needs the unsupported-hold path, or a
    /// scripted implementation for a served owner, clears the registry first.
    /// Production code has no path that removes an implementation.
    #[cfg(test)]
    pub(crate) fn reset_deletion_participants_for_tests(&self) {
        *crate::lock_unpoison(&self.targeted_deletion) =
            crate::targeted_deletion::ErasureParticipantRegistry::new();
    }

    /// Runs one bounded Targeted Deletion fan-out pass over the durable
    /// unfinished operations.
    ///
    /// Only active operations are driven: held operations wait for an explicit
    /// resume decision and finalizing operations belong to the completion
    /// boundary. Participants already verified for the current sweep are never
    /// demanded again, so a crash mid-fan-out continues with only the
    /// unfinished participants (§14); the durable snapshot and the operation
    /// identity are never regenerated.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Deletion`] for invalid pass parameters or when the
    /// canonical store refuses (torn participant state fails closed).
    pub async fn drive_targeted_deletion(
        &self,
        pass: crate::targeted_deletion::TargetedDeletionPass,
    ) -> Result<crate::targeted_deletion::TargetedDeletionPassOutcome, CoreError> {
        let registry = crate::lock_unpoison(&self.targeted_deletion).clone();
        crate::targeted_deletion::drive_targeted_deletion(&self.store, &registry, pass).await
    }

    pub(crate) fn task_launcher(
        &self,
    ) -> Option<&std::sync::Arc<dyn crate::task_run::TaskAgentLauncher>> {
        self.task_launcher.get()
    }

    /// Arms the test-only task-control race gate and returns it.
    #[cfg(test)]
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

    /// Runs one delegated Task Agent execution through the Host composition.
    ///
    /// Atomically consumes the delegation's launch reservation and registers
    /// the cooperative stop token under it (CCT §7.4): a delegation with no
    /// reservation in this process — including every pre-restart delegation —
    /// is refused as [`ExecutionUnavailable`](TaskAgentRunRefusal::ExecutionUnavailable)
    /// before any provider call or Action, and a second take of the same
    /// execution lifetime is refused as already running. The rest wires
    /// the Task-owned ports to the concrete History, activity, credential,
    /// and inference boundaries, and runs the bounded loop
    /// ([`DEFAULT_MAX_TURNS`](crate::task_run::DEFAULT_MAX_TURNS)). The abort
    /// token reaches the provider wait through the inference adapter, so a
    /// cancel never loses a claimed attempt's usage fact. `transport` is the
    /// provider transport (a fake in tests). Nothing here decides completion:
    /// the loop ends at the owner boundaries and the result adoption is the
    /// Task owner's.
    ///
    /// # Errors
    ///
    /// [`TaskAgentRunError`] for storage and inference technical failures;
    /// stale, terminal, sealed, already-started, already-running,
    /// reservation, refused, and
    /// not-sent answers stay domain outcomes inside [`TaskAgentRunOutcome`].
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
            // No launch reservation in this process: an old delegation, a
            // delegation whose reservation was released, or a caller that
            // only knows the delegation id. Never launch from the rows.
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

    /// Runs the full orchestration pipeline for one inbound frame.
    ///
    /// Transport-free by design: framing, sockets, and peer checks live in
    /// [`crate::conn`], while inference arrives as `transport` so tests pass a
    /// fake and production passes the `OpenAI` transport. The envelope
    /// `message_type`, the negotiated version, and the ingress gate
    /// (`gateTrips`) are checked in that order before dispatch; unhandled
    /// variants answer an empty vector.
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
            // The drain task only collects frames, so a panic there is the
            // task's own panic: resume it rather than reporting success.
            Err(join) => std::panic::resume_unwind(join.into_panic()),
        }
    }

    /// [`HostHandle::handle_frame`] with incremental emission.
    ///
    /// Every response is handed to `sink` at the point it is decided. For a
    /// text submit this is what makes `AcceptedForRound` and provider deltas
    /// real: the connection loop forwards them to the socket while this
    /// future still awaits the provider, instead of splitting a completed
    /// response afterwards. Other frames emit their single response as
    /// before. `stream_tx` feeds the same channel and carries the open
    /// stream's ordered frames; the submit path emits each response on
    /// exactly one of the two senders in emission order, so the drained
    /// channel preserves the response order.
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
            // Inbound challenges and results are never solicited (the Host
            // mints challenges and issues results), so both answer nothing —
            // the same silence as the catch-all below, spelled out so the
            // auth direction stays explicit.
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
                // A client report, not a command: it answers an outstanding
                // demand for this connection's incarnation when it matches one
                // and changes nothing otherwise. No reply is emitted — a
                // local-erasure result is itself the fact, and it is never
                // global completion (lifecycle §10).
                self.accept_client_erasure_result(&live, result);
            }
            // Inbound rejects, stray acks, and future variants answer
            // nothing: only the Host rejects, and only in response.
            _ => {}
        }
    }

    /// Decides what the domain gate does with `frame` under `live`.
    ///
    /// Pairing, capability, and proof frames never reach this gate (see
    /// [`HostHandle::handle_frame`]): pairing is pre-pairing by definition,
    /// capability predates authentication, and the proof is the
    /// authentication. A frame from a superseded connection answers a typed
    /// `StaleConnection` and the socket stays open (IPC §11.3): its
    /// attribution is verifiable from the table, so silence or a drop would
    /// lose the honest outcome. Every other non-serviceable frame needs all
    /// four premises — a device bound on this connection, a known connection
    /// entry, a completed authentication that is still current for the device
    /// (a newer authentication by the same device supersedes this connection),
    /// and an envelope connection id equal to the table id. Equality is the
    /// auth binding: the id is minted per accept and revealed only in
    /// [`Accepted`](ene_api::v1::handshake::AuthResult::Accepted), so echoing
    /// it proves the sender completed the challenge on this connection. A
    /// failed premise answers the terminal unpaired close, not a
    /// [`Reject`](ene_api::v1::payload::WirePayload::Reject): an
    /// unauthenticated peer learns nothing beyond the drop, never an oracle
    /// denial.
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

    /// Applies the domain gate to one frame: [`None`] when the frame may
    /// proceed, otherwise the typed refusal to emit. A superseded connection
    /// answers `StaleConnection` with the socket kept (IPC §11.3); every
    /// other non-serviceable frame answers the terminal unpaired close. One
    /// helper for every domain handler keeps the three-way gate decision
    /// single-sourced.
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

    /// The open round this connection currently owns for one companion.
    ///
    /// Keyed by connection lifetime, so a same-device replacement's new
    /// connection finds no inherited round: the old round binding is
    /// connection-transient by construction (IPC §9.3 replacement).
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
    /// `client_ref` is carried for diagnostics only.
    pub(crate) fn record_open_round(
        &self,
        live: &LiveInput,
        client_ref: &str,
        companion_key: &str,
        open: OpenRound,
    ) -> bool {
        let _ = client_ref;
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

    /// Drops one connection's open-round bindings (memory-only; durable
    /// History rows are untouched, so `HistoryRequest` still reads the past
    /// round after a replacement).
    pub(crate) fn drop_open_rounds_for(&self, connection: &ConnectionWireId) {
        let key = connection_key(connection);
        crate::lock_unpoison(&self.open_rounds).retain(|(owner, _), _| owner != &key);
    }

    /// Test-only: whether any conversation open round exists at all.
    #[cfg(test)]
    pub(crate) fn has_open_round_for_test(&self) -> bool {
        !crate::lock_unpoison(&self.open_rounds).is_empty()
    }

    /// Runs one short synchronous commit under the connection-ownership
    /// section (CCT §10.4).
    ///
    /// The one ownership primitive every Client-dependent operation uses:
    /// [`ConnectionTable::with_current_connection`] verifies `live`'s
    /// connection is still its device's current authenticated connection and
    /// holds that section for the whole closure, so a supersession, close, or
    /// replacement cannot interleave between the check and the commit.
    /// Returns [`None`] — running nothing — when the connection is no longer
    /// current. The closure must not await and must not call back into the
    /// connection table.
    pub(crate) fn with_current_connection<R>(
        &self,
        live: &LiveInput,
        commit: impl FnOnce() -> R,
    ) -> Option<R> {
        live.authority
            .with_current_connection(&live.connection_id, commit)
    }

    /// [`HostHandle::with_current_connection`] on the blocking pool.
    ///
    /// Use when the commit needs a synchronous SQLite primitive: the whole
    /// section runs inside `spawn_blocking`, so no `.await` happens while
    /// the connection table is held (CCT §10.4). The lock order is
    /// connection table → Task execution registry → SQLite; a closure that
    /// takes another async lock must take it before entering here.
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

    /// Resolves a domain round back to its issued wire string.
    ///
    /// The map is per-process: after a restart no wire string is mapped and
    /// the caller treats the round as stale (recovery runs through
    /// [`HistoryRequest`](ene_api::v1::round::HistoryRequest), never through
    /// rebinding).
    pub(crate) fn wire_for_round(&self, round: &RoundId) -> Option<String> {
        let maps = crate::lock_unpoison(&self.rounds);
        maps.iter()
            .find(|(_, mapped)| mapped.as_raw() == round.as_raw())
            .map(|(wire, _)| wire.clone())
    }

    /// Atomically resolves-or-mints the wire projection for one round.
    ///
    /// One lock section performs the lookup and the insert, so two
    /// concurrent issuers for the same domain round cannot both see "not
    /// mapped" and mint two projections: one domain round maps to exactly
    /// one wire, and one wire maps to one round. A fresh mint is recorded
    /// immediately; if the caller's durable append later does not commit,
    /// the entry simply stays unpublished (no ack or stream ever names it,
    /// and an unguessable mapping entry is not authority — acceptance still
    /// comes only from intake plus the durable commit) until the restart
    /// drops the map. Removing an entry on failure cannot be done safely:
    /// a racing issuer may already have reused the wire for its own
    /// accepted round, and a removal would break the same invariant.
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

    /// Records one Owner pairing approval and mints its one-time secret.
    ///
    /// Host-local trusted inlet behind the `approve-device` subcommand: it
    /// records the Owner decision through
    /// [`approve_pending`](DevicePairingRepository::approve_pending) and never
    /// decides whether pairing is allowed itself. The approval names the
    /// opaque pending id plus the connection that sent the request (read back
    /// from the stored pending row) and compare-and-swaps that row to Paired;
    /// an unknown id, an already-consumed row, or a mismatched connection
    /// yields `Ok(None)` (the caller lists [`HostHandle::pending_devices`]).
    /// Descriptors are display-only and never the approval key (#1389).
    ///
    /// The returned secret string is for one-time display on this
    /// Host-local trusted surface only: the caller shows it once and forgets
    /// it. It is persisted through `auth_store` for later
    /// [`ene_api::v1::handshake::AuthProof`] verification.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Store`] when the durable pairing tables are
    /// unavailable or the device-auth file cannot be written.
    pub async fn approve_device(
        &self,
        pending_id: &str,
    ) -> Result<Option<(DeviceRecord, String)>, CoreError> {
        let origin = DevicePairingRepository::list_pending(&self.store)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?
            .into_iter()
            .find(|pending| pending.pending_id == pending_id)
            .map(|pending| pending.origin_connection);
        let Some(origin) = origin else {
            // Unknown id here may still be an already-approved id whose
            // pending row is gone but whose paired record survives (rotation):
            // let the store decide from its paired table.
            let approved = DevicePairingRepository::approve_pending(&self.store, pending_id, "")
                .await
                .map_err(|error| CoreError::Store(error.to_string()))?;
            if let Some((record, secret)) = approved.as_ref() {
                self.auth_store
                    .save_secret(&record.id, &record.descriptor, secret)
                    .map_err(|error| CoreError::Store(error.to_string()))?;
            }
            return Ok(approved);
        };
        let approved = DevicePairingRepository::approve_pending(&self.store, pending_id, &origin)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        if let Some((record, secret)) = approved.as_ref() {
            self.auth_store
                .save_secret(&record.id, &record.descriptor, secret)
                .map_err(|error| CoreError::Store(error.to_string()))?;
        }
        Ok(approved)
    }

    /// Host-local trusted inlet surfacing the Owner-visible pending set so an
    /// unknown `approve-device` id can be retried with the exact value. Each
    /// entry carries its opaque approval id plus the display descriptor;
    /// approval names the id, never the descriptor (#1389).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Store`] when the durable pairing tables are
    /// unavailable.
    pub async fn pending_devices(&self) -> Result<Vec<ene_credential::PendingPairing>, CoreError> {
        DevicePairingRepository::list_pending(&self.store)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))
    }

    /// Serving startup boundary: drops every still-unapproved pending request
    /// before the listener binds, so a stale poll after a restart converges
    /// on a fresh pending instead of authenticating (#1389). Paired records
    /// are untouched. Read-only opens never call this.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Store`] when the durable pairing tables are
    /// unavailable.
    pub(crate) async fn clear_unapproved_pendings(&self) -> Result<(), CoreError> {
        DevicePairingRepository::clear_unapproved_pendings(&self.store)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))
    }

    /// Records one Owner credential approval, making the ref usable.
    ///
    /// Host-local trusted inlet: the wire intent only proposes (pending),
    /// and this call flips it. On approval the credential ref row is
    /// created (usable marker); re-approval is idempotent. Unknown pairs
    /// return `Ok(false)` so the caller can list pendings.
    ///
    /// Approval is one atomic store commit: every plaintext occurrence of
    /// the bearer in durable content is swept, the usable ref is created,
    /// and the credential-set revision advances together. A scrub premise
    /// taken before the commit is therefore either covered by the sweep or
    /// refused by the revision, so a value stored before registration cannot
    /// survive as raw content. A missing or unreadable bearer holds the
    /// approval, because absence of the value cannot be proven and the
    /// credential must not become usable unprotected.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Store`] when the durable tables are unavailable
    /// or the bearer cannot be read.
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

    pub async fn pending_credentials(&self) -> Result<Vec<String>, CoreError> {
        let pending = CredentialApprovalRepository::list_pending(&self.store)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        Ok(pending
            .into_iter()
            .map(|entry| format!("{}:{}", entry.provider, entry.label))
            .collect())
    }

    /// Admits one transport close and runs the presence fallback if owned.
    ///
    /// The close decision and the presence compare/commit share one short
    /// connection-table section inside `spawn_blocking` (CCT §10.4): the
    /// table re-reads whether this connection is still the device's current
    /// authenticated one before clearing it, so a close racing a newer
    /// authentication never clears the new current, and a superseded
    /// connection's close never triggers the fallback (#1384, S5-05). The
    /// callback runs synchronously while the section is held, so the presence
    /// commit cannot interleave with an authentication install; the lock order
    /// is connection table → task registry → SQLite (this path takes the
    /// table section, then SQLite, and never the registry in between).
    ///
    /// The fallback itself is a best-effort compare-and-commit to `NoActive`
    /// for
    /// [`DisconnectObserved`](ene_presence::ThinMoveReason::DisconnectObserved):
    /// only a `Present` attribution owned by the deterministic mapping of the
    /// closed device moves (compare-and-begin plus confirm with a not-live
    /// premise); any other state, a lost compare race, or a store failure
    /// leaves attribution untouched. Target selection for a Host-local
    /// fallback client is a later slice; this keeps the single-step move to
    /// `NoActive`.
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
        // Connection-owned presentation state dies with this connection: a
        // close that loses the race to a newer authentication removes no new
        // current (note_closed compares identity), and the cleanup below runs
        // only after the record is gone, so no guarded install can recreate
        // state for this connection afterwards (CCT §10.4).
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
        // The record is gone: every guarded install for this connection is
        // already refused, so the lifecycle cleanup below cannot race a
        // resurrection (CCT §10.4).
        self.on_connection_closed(&connection);
    }

    /// The single Host-memory lifecycle boundary for a superseded connection
    /// (IPC §9.3 replacement).
    ///
    /// Supersession and close differ in meaning — supersession keeps the old
    /// socket for typed stale rejections and never touches presence, while
    /// close ends the transport and may run the presence fallback — but both
    /// invalidate exactly the same connection-transient world. Routing both
    /// through this one boundary keeps a new connection from inheriting any
    /// of the old one's transient state by construction instead of by
    /// per-handler cleanup.
    pub(crate) fn on_connection_superseded(&self, connection: &ConnectionWireId) {
        self.drop_connection_transient_state(connection);
    }

    /// The single Host-memory lifecycle boundary for a closed connection.
    ///
    /// Called after the close admission removed the connection record, so
    /// no guarded install can recreate state for this connection after the
    /// sweep.
    pub(crate) fn on_connection_closed(&self, connection: &ConnectionWireId) {
        self.drop_connection_transient_state(connection);
    }

    /// Invalidates every Host-memory entry owned by one ended connection
    /// lifetime.
    ///
    /// Owners: presentation subscriptions, receipts, query-scoped refs,
    /// cursors, carried item refs, resume retry-epoch slots; conversation
    /// open rounds; the first-party Task selection; and any outstanding Client
    /// local-erasure demand addressed to this connection (its waiter reports a
    /// hold, never a completion). Streams need no registry: every publication
    /// re-checks the connection table (CCT §10.4). Durable rows are never
    /// touched — a released receipt leaves its rows re-presentable, and a
    /// dropped open round leaves its History rows readable through
    /// `HistoryRequest`.
    fn drop_connection_transient_state(&self, connection: &ConnectionWireId) {
        self.drop_presentation_connection_state(connection);
        self.drop_open_rounds_for(connection);
        self.conversation_tasks
            .drop_first_party_selection_for(connection);
        self.client_transients.note_connection_ended(connection);
    }

    /// Arms the confirmation gate before either commit lock is acquired.
    #[cfg(test)]
    pub(crate) fn arm_confirm_commit_gate(&self) -> std::sync::Arc<TestGate> {
        let gate = std::sync::Arc::new(TestGate::default());
        *crate::lock_unpoison(&self.confirm_commit_gate) = Some(std::sync::Arc::clone(&gate));
        gate
    }

    /// The armed confirmation commit gate, when a test installed one.
    #[cfg(test)]
    pub(crate) fn confirm_commit_gate(&self) -> Option<std::sync::Arc<TestGate>> {
        crate::lock_unpoison(&self.confirm_commit_gate).clone()
    }

    /// Disarms the confirmation commit gate.
    #[cfg(test)]
    pub(crate) fn disarm_confirm_commit_gate(&self) {
        *crate::lock_unpoison(&self.confirm_commit_gate) = None;
    }

    /// Arms the test-only close-admission gate and returns it.
    #[cfg(test)]
    pub(crate) fn arm_close_gate(&self) -> std::sync::Arc<TestCloseGate> {
        let gate = std::sync::Arc::new(TestCloseGate::default());
        *crate::lock_unpoison(&self.close_gate) = Some(std::sync::Arc::clone(&gate));
        gate
    }

    /// Arms the test-only submit-acceptance gate and returns it.
    ///
    /// The gate pauses a [`SubmitTextInput`](ene_api::v1::round::SubmitTextInput)
    /// after admission and before the guarded owner append, so a test can
    /// supersede the connection in between and pin that nothing commits.
    #[cfg(test)]
    pub(crate) fn arm_submit_accept_gate(&self) -> std::sync::Arc<TestGate> {
        let gate = std::sync::Arc::new(TestGate::default());
        *crate::lock_unpoison(&self.submit_accept_gate) = Some(std::sync::Arc::clone(&gate));
        gate
    }

    /// The armed submit-acceptance gate, when a test installed one.
    #[cfg(test)]
    pub(crate) fn submit_accept_gate(&self) -> Option<std::sync::Arc<TestGate>> {
        crate::lock_unpoison(&self.submit_accept_gate).clone()
    }

    /// Arms the test-only read-ref mint gate and returns it.
    ///
    /// The gate pauses a read query after its durable read and before the
    /// connection-scoped ref/cursor mint, so a test can supersede the
    /// connection in between and pin that no ref is minted.
    #[cfg(test)]
    pub(crate) fn arm_ref_mint_gate(&self) -> std::sync::Arc<TestGate> {
        let gate = std::sync::Arc::new(TestGate::default());
        *crate::lock_unpoison(&self.ref_mint_gate) = Some(std::sync::Arc::clone(&gate));
        gate
    }

    /// The armed read-ref mint gate, when a test installed one.
    #[cfg(test)]
    pub(crate) fn ref_mint_gate(&self) -> Option<std::sync::Arc<TestGate>> {
        crate::lock_unpoison(&self.ref_mint_gate).clone()
    }

    /// Disarms the test-only submit-acceptance gate.
    #[cfg(test)]
    pub(crate) fn disarm_submit_accept_gate(&self) {
        *crate::lock_unpoison(&self.submit_accept_gate) = None;
    }

    /// Disarms the test-only read-ref mint gate.
    #[cfg(test)]
    pub(crate) fn disarm_ref_mint_gate(&self) {
        *crate::lock_unpoison(&self.ref_mint_gate) = None;
    }

    /// Arms the test-only fetch gate and returns it.
    ///
    /// The gate pauses a presentation pass after its presence checks and
    /// before it takes the begin/ack transition lock, so a test can replace
    /// the connection and let the replacement run its own full pass before
    /// the paused one resumes.
    #[cfg(test)]
    pub(crate) fn arm_fetch_gate(&self) -> std::sync::Arc<TestGate> {
        let gate = std::sync::Arc::new(TestGate::default());
        *crate::lock_unpoison(&self.fetch_gate) = Some(std::sync::Arc::clone(&gate));
        gate
    }

    /// The armed fetch gate, when a test installed one.
    #[cfg(test)]
    pub(crate) fn fetch_gate(&self) -> Option<std::sync::Arc<TestGate>> {
        crate::lock_unpoison(&self.fetch_gate).clone()
    }

    /// Disarms the test-only fetch gate.
    #[cfg(test)]
    pub(crate) fn disarm_fetch_gate(&self) {
        *crate::lock_unpoison(&self.fetch_gate) = None;
    }

    /// Arms the submit publication gate and returns it.
    ///
    /// The gate pauses a submit after the open-round install decision and
    /// before the accepted/open publication, so a test can replace the
    /// connection in between and pin that the old connection publishes
    /// nothing (test gate, never a sleep).
    #[cfg(test)]
    pub(crate) fn arm_submit_publish_gate(&self) -> std::sync::Arc<TestGate> {
        let gate = std::sync::Arc::new(TestGate::default());
        *crate::lock_unpoison(&self.submit_publish_gate) = Some(std::sync::Arc::clone(&gate));
        gate
    }

    /// Disarms the test-only submit publication gate.
    #[cfg(test)]
    pub(crate) fn disarm_submit_publish_gate(&self) {
        *crate::lock_unpoison(&self.submit_publish_gate) = None;
    }

    /// Arms the submit open-round gate and returns it.
    ///
    /// The gate pauses a submit after the durable Owner append and before
    /// the open-round installation, so a test can replace the connection in
    /// between and pin that the durable acceptance stands while the old
    /// connection opens no round and publishes nothing.
    #[cfg(test)]
    pub(crate) fn arm_submit_open_gate(&self) -> std::sync::Arc<TestGate> {
        let gate = std::sync::Arc::new(TestGate::default());
        *crate::lock_unpoison(&self.submit_open_gate) = Some(std::sync::Arc::clone(&gate));
        gate
    }

    /// Disarms the test-only submit open-round gate.
    #[cfg(test)]
    pub(crate) fn disarm_submit_open_gate(&self) {
        *crate::lock_unpoison(&self.submit_open_gate) = None;
    }

    /// Arms the test-only guarded-resume race gate and returns it.
    #[cfg(test)]
    pub(crate) fn arm_resume_gate(&self) -> std::sync::Arc<crate::task_control::TestResumeGate> {
        let gate = std::sync::Arc::new(crate::task_control::TestResumeGate::default());
        *crate::lock_unpoison(&self.resume_gate) = Some(std::sync::Arc::clone(&gate));
        gate
    }

    /// Test-only: how many receipt-expiry housekeeping runs happened.
    #[cfg(all(test, unix))]
    pub(crate) fn receipt_expiry_runs_for_test(&self) -> usize {
        self.receipt_expiry_runs
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Arms the test-only presentation-start commit gate and returns it.
    #[cfg(test)]
    pub(crate) fn arm_presentation_commit_gate(
        &self,
    ) -> std::sync::Arc<crate::presentation::TestPresentationCommitGate> {
        let gate = std::sync::Arc::new(crate::presentation::TestPresentationCommitGate::default());
        *crate::lock_unpoison(&self.presentation_commit_gate) = Some(std::sync::Arc::clone(&gate));
        gate
    }
}

/// The synchronous presence fallback for one close admission.
///
/// Runs while the connection table section is held (CCT §10.4), so every
/// store call here is the sync form; it must never await and never call back
/// into the connection table.
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
    // Rejected and errored confirms alike leave the transition unconfirmed;
    // the next intake reads the `InTransition` attribution and reports held,
    // which is honest.
    if store
        .confirm_transition_sync(companion.as_raw(), generation, premise)
        .is_err()
    {
        // The unconfirmed transition stands: the next intake reports held.
    }
}

impl HostHandle {
    /// Startup presence boundary: normalizes every companion row (PR §6.4).
    ///
    /// The serving Host runs this after the state open and before the
    /// listener binds, alongside the credential sweep and sealed-result
    /// reconciliation. A refusal (generation exhaustion, malformed recovery
    /// intent, missing attribution) fails startup: serving with an unknown
    /// presence state would present a client as current that nobody verified.
    /// Read-only and local management paths never call this; the plain state
    /// open does not normalize presence.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Store`] when the normalization cannot be read or
    /// committed, or when any companion is refused.
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

    /// Observes a socket close for `client_ref` and clears presence when owned.
    ///
    /// Async fallback path: the serving close admission is
    /// `HostHandle::close_connection`, which decides currentness inside the
    /// connection-table section and commits through the synchronous store
    /// primitives. This form is for callers without a table section; it
    /// passes an empty snapshot — the connection layer's cross-device
    /// currentness lives behind [`crate::conn`], not here — so the fallback
    /// deterministically answers `NoActive`, never a guessed client.
    /// Callers holding a table-derived snapshot use
    /// `HostHandle::note_disconnect_with` instead.
    pub async fn note_disconnect(&self, client_ref: &str) {
        self.note_disconnect_with(client_ref, &|| Vec::new()).await;
    }

    /// Observes a socket close and runs the normal-disconnect fallback.
    ///
    /// `current` is the connection layer's snapshot source: each call returns
    /// the currently authenticated connections, so the confirm below
    /// re-derives the chosen target's currentness instead of trusting the
    /// snapshot taken at selection time (S5-05). Candidates must be
    /// current-authenticated, SameMachine-verified, device-permitted, and not
    /// the closing client; [`select_fallback_candidate`] picks the
    /// lexicographically smallest. With no eligible candidate — or a target
    /// that stopped being current before confirm — the transition confirms
    /// `NoActive`: the Host never crowns a client nobody verified and never
    /// auto-starts one.
    ///
    /// Best-effort compare-and-commit for
    /// [`DisconnectObserved`](ene_presence::ThinMoveReason::DisconnectObserved):
    /// only a `Present` attribution owned by the deterministic mapping of
    /// `client_ref` moves; any other state, a lost compare race, or a store
    /// failure leaves attribution untouched.
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
                // Re-derive at confirm time: a target that is no longer the
                // current authenticated connection confirms `NoActive`, not
                // `Present`.
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
        // Rejected and errored confirms alike leave the transition
        // unconfirmed; the next intake reads the `InTransition`
        // attribution and reports held, which is honest.
        if !matches!(
            self.store
                .confirm_transition(companion.as_raw(), generation, live)
                .await,
            Ok(ConfirmTransitionOutcome::Confirmed(_))
        ) {}
    }
}

use ene_plugin_ipc::WireFrame;

#[cfg(test)]
mod tests;
