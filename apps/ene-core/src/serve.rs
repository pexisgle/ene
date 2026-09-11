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
//! - Pairing is Owner-confirmed through the durable
//!   [`DevicePairingRepository`]: a request records a pending entry, the
//!   Host-local `approve-device` inlet records the Owner decision, and a later
//!   request for the approved descriptor issues the device key. There is no
//!   same-descriptor auto-approve: an unapproved descriptor always answers
//!   [`PendingOwnerConfirmation`](ene_api::v1::handshake::PairingResult::PendingOwnerConfirmation).
//! - Presence attach happens only on the
//!   [`SubmitTextInput`](ene_api::v1::round::SubmitTextInput) path: capability
//!   and management frames never attach. Socket close runs the symmetric
//!   compare-and-commit for
//!   [`DisconnectObserved`](ene_presence::ThinMoveReason::DisconnectObserved)
//!   through [`HostHandle::note_disconnect`], which [`crate::conn`] calls with
//!   the paired device string.
//! - Presence `active_client` names a device through `device_client`, a
//!   deterministic `UUID v5` mapping rather than issuance.
//! - Authentication is challenge/proof over the pairing secret. Pending nonces
//!   live in memory only, so a restart fails closed; pairing secrets live only
//!   in `device-auth.json` (plus the transient approve-time display scope),
//!   with no secret cache.
//! - The response sender reveals the connection id only on and after
//!   [`Accepted`](ene_api::v1::handshake::AuthResult::Accepted): every
//!   pre-accept response carries [`None`], so a peer that never completed the
//!   challenge never learns the id the ingress gate requires it to echo.
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
use std::sync::{Mutex as StdMutex, MutexGuard};

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
use ene_inference::ProviderTransport;
use ene_permission::EvaluationTracker;
use ene_presence::{
    ClientId, ConfirmTransitionOutcome, LiveReachabilityRef, MoveDecision, PresenceCheckRef,
    PresenceRepository, PresenceState, ThinMoveReason,
};
use ene_presentation::{OpenRound, RoundId};
use ene_primitive::RawId;
use ene_store::Store;
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

mod frames;
mod handshake;
mod lifecycle;

use lifecycle::ensure_data_dir;

pub(crate) use frames::{outgoing_envelope, outgoing_frame, reject_frame, unpaired_close};
pub use lifecycle::serve;

/// Binary-local Host failure.
///
/// Messages carry operational notes only: no secrets, no body text, and no
/// paths (which stay out for operational brevity, matching the store
/// convention). Infrastructure variants name the failing layer; denial and
/// staleness are domain outcomes on the wire, never this error.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error(transparent)]
    Config(#[from] ene_config::typed::ConfigError),
    #[error("store unavailable: {0}")]
    Store(String),
    #[error("bind failed: {0}")]
    Bind(String),
    #[error("codec failed: {0}")]
    Codec(String),
    #[error("handshake failed: {0}")]
    Handshake(String),
    #[error("inference failed: {0}")]
    Inference(String),
    /// Unknown descriptors list the pending descriptors so the Owner can
    /// retry with the exact value; the message carries display strings only.
    #[error("device approval failed: {0}")]
    Approve(String),
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(&'static str),
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

    fn delete(&self, cred: &CredentialRef) -> Result<(), CredentialTechnicalError> {
        match self {
            Self::Env(inner) => inner.delete(cred),
            Self::Memory(inner) => inner.delete(cred),
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
/// table paired on this connection (if any), `connection_known` reports
/// whether the connection table knows this connection at all, `authed`
/// reports whether this connection completed the challenge/proof exchange and
/// is still the device's current authed connection (a newer authentication by
/// the same device supersedes it), and `connection_id` is the table key
/// itself. The gate in [`HostHandle::handle_frame`] trusts these conn-filled
/// premises; direct handle callers (tests) construct them explicitly. All
/// fields are public so connection adapters and integration tests can
/// construct the value directly.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Keys the pending-nonce map by connection: the hyphenated wire form of the id.
///
/// The same string form the connection layer uses for paired devices, so map
/// keys match across the Host/connection boundary by construction.
pub(crate) fn conn_key(id: &ConnectionWireId) -> String {
    id.0.as_hyphenated().to_string()
}

/// Locks a Host map mutex, recovering from poisoning.
///
/// Poisoning only follows a panic inside a critical section; sections here
/// run plain map operations that never panic while holding the guard, so
/// recovery preserves the committed state.
fn lock_map<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// `Stage 2` Host handle: durable store, evaluation tracker, and Host maps.
///
/// Interior mutability: `open_rounds` and `rounds` sit behind short `std`
/// mutex sections (clone out before every await, never hold a guard across
/// an await); `tracker` is a leaf async mutex (the inference `send` boundary
/// needs `&mut` across its transport await while only touching the tracker
/// synchronously up front, and the transport never calls back into the
/// handle, so no lock ordering exists); the [`Store`] carries its own lock.
/// Nothing here is durable except through [`Store`] and the device-auth file:
/// a restart drops every map while the database persists, and old wire round
/// refs then surface as stale (never rebound). The device tables and the
/// history `local_id` column are durable in [`Store`], and pairing secrets in
/// `device-auth.json` through the `auth_store` field.
///
/// Map keys: `open_rounds` is keyed by `(client ref, companion key)`; round
/// refs issued on the wire resolve back through `rounds` (wire string to
/// domain round); `pending_nonces` is keyed by the hyphenated connection id
/// string.
pub struct HostHandle {
    pub(crate) store: Store,
    pub(crate) tracker: AsyncMutex<EvaluationTracker>,
    pub(crate) open_rounds: StdMutex<HashMap<(String, String), OpenRound>>,
    pub(crate) rounds: StdMutex<HashMap<String, RoundId>>,
    pub(crate) cred_store: CredStore,
    /// File-backed pairing-secret store by device, opened on
    /// `<data_dir>/device-auth.json`.
    ///
    /// Secrets live here and in the transient approve-time display scope only:
    /// the handle keeps no secret map and no cache. See the
    /// [`FileDeviceAuthStore`] contract for custody, file protection, and the
    /// backup-exclusion rule.
    pub(crate) auth_store: FileDeviceAuthStore,
    /// Single-use auth nonces by connection key.
    ///
    /// In-memory and transient: a restart drops every pending nonce, so
    /// post-restart Clients re-run capability-plus-proof rather than
    /// resuming. Fail-closed: a proof with no pending nonce answers
    /// [`Rejected`](ene_api::v1::handshake::AuthResult::Rejected), never
    /// acceptance. Each nonce is consumed on first proof regardless of
    /// outcome, so a captured proof cannot replay.
    pub(crate) pending_nonces: StdMutex<HashMap<String, String>>,
    /// In-memory, best-effort queue of pinned Experience premises whose
    /// completed replies await a Learning formation pass.
    ///
    /// Each item carries its own source range and transcript, pinned at reply
    /// completion. See
    /// [`crate::dialogue`]: the pass is post-response work, never a condition
    /// of the client-visible completion, and a crash simply drops the queued
    /// derived update instead of replaying an old pass.
    pub(crate) learning_queue: StdMutex<VecDeque<ene_learning::ExperienceCandidate>>,
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
}

impl HostHandle {
    /// Opens (or creates) the Host state under `data_dir` with the production
    /// credential store.
    ///
    /// Ensures `data_dir` exists (`0700` on Unix) and opens `app.db` inside it
    /// through [`Store::open`]. `Stage 2` owns directory creation: resolution
    /// stays pure in `ene-config` while the side effect lives here.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Store`] when the directory cannot be ensured or
    /// the database cannot be opened or migrated.
    pub async fn open(data_dir: &Path) -> Result<Self, CoreError> {
        Self::open_with_cred_store(data_dir, CredStore::Env(EnvCredentialStore::new())).await
    }

    /// Opens (or creates) the Host state under `data_dir` with an explicit
    /// credential store.
    ///
    /// Integration tests pass [`CredStore::Memory`] pre-provisioned with test
    /// bearers to stay hermetic (the environment store reads the real process
    /// environment once when it is constructed). The device-auth file opens on
    /// `<data_dir>/device-auth.json` (created lazily on first approval) after
    /// the data directory is ensured, so the open always has its parent.
    ///
    /// # Errors
    ///
    /// [`CoreError::Store`] as in [`HostHandle::open`], plus when the
    /// device-auth file cannot be opened (unreadable, malformed, or wrongly
    /// permissioned), or when a registered credential value cannot be read
    /// and the startup sweep therefore cannot complete.
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
        let handle = Self {
            store,
            tracker: AsyncMutex::new(EvaluationTracker::new()),
            open_rounds: StdMutex::new(HashMap::new()),
            rounds: StdMutex::new(HashMap::new()),
            cred_store,
            auth_store,
            pending_nonces: StdMutex::new(HashMap::new()),
            learning_queue: StdMutex::new(VecDeque::new()),
            learning_worker: AsyncMutex::new(()),
            companion_wire: RawId::new().as_uuid().to_string(),
        };
        // Startup boundary: the credential store has pinned its values (for
        // the env store, read once), so sweep every registered value out of
        // durable content and advance the revision together before serving.
        // A failed sweep keeps the handle closed rather than serving content
        // prepared under an unknown set.
        handle.sweep_registered_values().await?;
        Ok(handle)
    }

    /// Sweeps every registered pinned value and advances the revision once.
    ///
    /// Runs before the handle serves anything. Every registered value must be
    /// readable: an unreadable value fails the open instead of skipping the
    /// sweep, because absence of the value cannot be proven and the Host must
    /// not serve content that may still hold it in plaintext.
    async fn sweep_registered_values(&self) -> Result<(), CoreError> {
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

    /// Runs the full orchestration pipeline for one inbound frame.
    ///
    /// Transport-free by design: framing, sockets, and peer checks live in
    /// [`crate::conn`], while inference arrives as `transport` so tests pass a
    /// fake and production passes the `OpenAI` transport. The envelope
    /// `message_type`, the negotiated version, and the ingress gate
    /// (`gate_trips`) are checked in that order before dispatch; unhandled
    /// variants answer an empty vector.
    pub async fn handle_frame(
        &self,
        frame: WireFrame,
        live: LiveInput,
        transport: &impl ProviderTransport,
    ) -> Vec<WireFrame> {
        if frame.envelope.message_type.0 != frame.payload.message_type() {
            return vec![reject_frame(
                &frame,
                &live,
                RejectKind::UnsupportedMessage,
                format!("unknown message type {:?}", frame.envelope.message_type.0),
            )];
        }
        let negotiated_version = live.negotiated.as_ref().map(|terms| terms.version);
        match (negotiated_version, frame.envelope.protocol) {
            (Some(want), got) if got != want => {
                return vec![reject_frame(
                    &frame,
                    &live,
                    RejectKind::IncompatibleProtocol,
                    format!("version {got:?} outside negotiated version {want:?}"),
                )];
            }
            (None, got) if got != ProtocolVersion::V1 => {
                return vec![reject_frame(
                    &frame,
                    &live,
                    RejectKind::IncompatibleProtocol,
                    format!("version {got:?} without negotiation"),
                )];
            }
            _ => {}
        }
        match &frame.payload {
            WirePayload::PairingRequest(request) => self.pair(&frame, request, &live).await,
            WirePayload::CapabilityAdvertise(advertise) => {
                if live.paired_device.is_none() {
                    return vec![unpaired_close(&frame, &live)];
                }
                self.advertise(&frame, advertise, &live)
            }
            WirePayload::AuthProof(proof) => self.verify_proof(&frame, proof, &live).await,
            // Inbound challenges and results are never solicited (the Host
            // mints challenges and issues results), so both answer nothing —
            // the same empty vector as the catch-all below, spelled out so
            // the auth direction stays explicit.
            WirePayload::AuthChallenge(_) | WirePayload::AuthResult(_) => Vec::new(),
            WirePayload::SubmitTextInput(submit) => {
                if Self::gate_trips(&frame, &live) {
                    return vec![unpaired_close(&frame, &live)];
                }
                self.submit_text(&frame, submit, &live, transport).await
            }
            WirePayload::ConfirmPresentation(confirm) => {
                if Self::gate_trips(&frame, &live) {
                    return vec![unpaired_close(&frame, &live)];
                }
                self.confirm_presentation(&frame, confirm).await
            }
            WirePayload::HistoryRequest(request) => {
                if Self::gate_trips(&frame, &live) {
                    return vec![unpaired_close(&frame, &live)];
                }
                self.answer_history(&frame, request, &live).await
            }
            WirePayload::ManagementIntent(intent) => {
                if Self::gate_trips(&frame, &live) {
                    return vec![unpaired_close(&frame, &live)];
                }
                self.apply_intent(&frame, intent, &live).await
            }
            WirePayload::ManagementViewRequest(request) => {
                if Self::gate_trips(&frame, &live) {
                    return vec![unpaired_close(&frame, &live)];
                }
                self.answer_view(&frame, request, &live).await
            }
            // Inbound rejects, stray acks, and future variants answer
            // nothing: only the Host rejects, and only in response.
            _ => Vec::new(),
        }
    }

    /// Reports whether the ingress gate trips for `frame` under `live`.
    ///
    /// Pairing, capability, and proof frames never reach this gate (see
    /// [`HostHandle::handle_frame`]): pairing is pre-pairing by definition,
    /// capability predates authentication, and the proof is the
    /// authentication. Every later frame needs all four premises: a device
    /// paired on this connection, a known connection entry, a completed
    /// authentication that is still current for the device (a newer
    /// authentication by the same device supersedes this connection, so a
    /// replayed id on the old connection still trips), and an envelope
    /// connection id equal to the table id. Equality is the auth binding: the
    /// id is minted per accept and revealed only in
    /// [`Accepted`](ene_api::v1::handshake::AuthResult::Accepted), so echoing
    /// it proves the sender completed the challenge on this connection.
    /// Trips on unauthenticated domain service; the answer is a terminal
    /// disconnect, not a [`Reject`](ene_api::v1::payload::WirePayload::Reject)
    /// (which exists for post-auth declines): an unauthenticated peer learns
    /// nothing beyond the drop, never an oracle denial.
    fn gate_trips(frame: &WireFrame, live: &LiveInput) -> bool {
        live.paired_device.is_none()
            || !live.connection_known
            || !live.authed
            || frame.envelope.sender.connection_id != Some(live.connection_id)
    }

    pub(crate) fn round_for(&self, wire: &str) -> Option<RoundId> {
        lock_map(&self.rounds).get(wire).copied()
    }

    pub(crate) fn open_round_for(
        &self,
        client_ref: &str,
        companion_key: &str,
    ) -> Option<OpenRound> {
        lock_map(&self.open_rounds)
            .get(&(client_ref.to_string(), companion_key.to_string()))
            .copied()
    }

    pub(crate) fn record_open_round(&self, client_ref: &str, companion_key: &str, open: OpenRound) {
        lock_map(&self.open_rounds)
            .insert((client_ref.to_string(), companion_key.to_string()), open);
    }

    /// Resolves a domain round back to its issued wire string.
    ///
    /// The map is per-process: after a restart no wire string is mapped and
    /// the caller treats the round as stale (recovery runs through
    /// [`HistoryRequest`](ene_api::v1::round::HistoryRequest), never through
    /// rebinding).
    pub(crate) fn wire_for_round(&self, round: &RoundId) -> Option<String> {
        let maps = lock_map(&self.rounds);
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
        let mut maps = lock_map(&self.rounds);
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
    /// decides whether pairing is allowed itself. An unknown descriptor
    /// yields `Ok(None)` (the caller lists [`HostHandle::pending_devices`]);
    /// a blank descriptor can never match because wire ingress denies blank
    /// descriptors before they reach the store.
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
        descriptor: &str,
    ) -> Result<Option<(DeviceRecord, String)>, CoreError> {
        let approved = DevicePairingRepository::approve_pending(&self.store, descriptor)
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
    /// unknown `approve-device` descriptor can be retried with the exact
    /// value. Descriptors are display strings only, never secrets.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Store`] when the durable pairing tables are
    /// unavailable.
    pub async fn pending_devices(&self) -> Result<Vec<String>, CoreError> {
        let pending = DevicePairingRepository::list_pending(&self.store)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        Ok(pending.into_iter().map(|entry| entry.descriptor).collect())
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

    /// Observes a socket close for `client_ref` and clears presence when owned.
    ///
    /// Best-effort compare-and-commit to `NoActive` for
    /// [`DisconnectObserved`](ene_presence::ThinMoveReason::DisconnectObserved):
    /// only a `Present` attribution owned by the deterministic mapping of
    /// `client_ref` moves (through compare-and-begin plus confirm with a
    /// not-live premise); any other state, a lost compare race, or a store
    /// failure leaves attribution untouched. [`crate::conn`] passes the
    /// paired device wire string, so the deterministic mapping re-derives the
    /// same [`ClientId`] the submit path attached, including after restarts.
    pub async fn note_disconnect(&self, client_ref: &str) {
        let client = device_client(client_ref);
        let Ok(companion) = self.store.ensure_running_companion().await else {
            return;
        };
        let Ok(Some(current)) = self.store.load_attribution(companion.as_raw()).await else {
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
        let Ok(MoveDecision::TransitioningToNew { generation }) = self
            .store
            .compare_and_begin_transition(
                companion.as_raw(),
                expected,
                None,
                ThinMoveReason::DisconnectObserved,
            )
            .await
        else {
            return;
        };
        let premise = LiveReachabilityRef {
            client,
            connection_live: false,
        };
        // Rejected and errored confirms alike leave the transition
        // unconfirmed; the next intake reads the `InTransition`
        // attribution and reports held, which is honest.
        if !matches!(
            self.store
                .confirm_transition(companion.as_raw(), generation, premise)
                .await,
            Ok(ConfirmTransitionOutcome::Confirmed(_))
        ) {}
    }
}

use ene_plugin_ipc::WireFrame;

#[cfg(test)]
mod tests;
