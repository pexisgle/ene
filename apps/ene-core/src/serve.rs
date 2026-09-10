//! `Stage 2` Host composition: errors, handle, dispatch, handshake, entry point.
//!
//! [`HostHandle`] is the testable seam: it owns the durable [`ene_store::Store`],
//! the [`EvaluationTracker`], and the per-process Host maps, and
//! [`HostHandle::handle_frame`] runs the full orchestration pipeline over one
//! [`WireFrame`] without touching any socket. [`serve`] wires a handle to the
//! [`crate::conn`] listener with the production inference transport.
//!
//! Trust premises, all documented where they are used:
//!
//! - The data directory is created by [`HostHandle::open_with_cred_store`] with
//!   mode `0700` on Unix (`Stage 2` owns directory creation). The same-machine
//!   trust premise rests on that directory plus the per-connection same-user
//!   check in [`crate::conn`], never on a Client self-report.
//! - Pairing is Owner-confirmed through the durable
//!   [`DevicePairingRepository`]:
//!   [`ene_api::v1::handshake::PairingRequest`] records a pending request, the Host-local
//!   `approve-device` inlet records the Owner decision, and a later request
//!   for the approved descriptor issues the device key. There is no
//!   same-descriptor auto-approve: an unapproved descriptor always answers
//!   [`PendingOwnerConfirmation`](ene_api::v1::handshake::PairingResult::PendingOwnerConfirmation).
//! - Presence attach happens only on the
//!   [`SubmitTextInput`](ene_api::v1::round::SubmitTextInput) path: when the
//!   loaded attribution is `NoActive`, intake runs an explicit
//!   compare-and-commit plus confirm for
//!   [`InitialAttach`](ene_presence::ThinMoveReason::InitialAttach).
//!   Capability and management frames never attach. Socket close runs the
//!   symmetric compare-and-commit for
//!   [`DisconnectObserved`](ene_presence::ThinMoveReason::DisconnectObserved)
//!   through [`HostHandle::note_disconnect`], which [`crate::conn`] calls with
//!   the paired device string.
//! - The presence `active_client` names a device through a deterministic
//!   mapping, not issuance: `device_client` derives the per-process
//!   [`ClientId`] from the device wire string with `UUID v5`, so one device
//!   maps to one client within a process and the same device maps the same
//!   way after a restart (`Stage 2` runs one client per device).
//! - The ingress gate in [`HostHandle::handle_frame`] drops unauthenticated
//!   domain service: any post-capability frame whose [`LiveInput`] carries no
//!   paired device, no known connection, no completed authentication on the
//!   current connection, or an envelope connection id that does not equal the
//!   table id answers a single terminal
//!   [`ene_api::v1::handshake::DisconnectNotice`] with reason `"unpaired"` and nothing else. A
//!   generic reject DTO does exist in `ene-api`
//!   ([`Reject`](ene_api::v1::payload::WirePayload::Reject), used for
//!   post-auth declines such as conflicting commands and envelope
//!   violations), but the pre-auth gate deliberately does not use it:
//!   silence-plus-close (rather than an oracle denial) reveals nothing to
//!   an unauthenticated peer. Pairing frames carry no
//!   checks; capability frames need the paired-device check only (they predate
//!   authentication); [`ene_api::v1::handshake::AuthProof`] frames
//!   need none (they ARE the authentication). Inbound
//!   [`ene_api::v1::handshake::AuthChallenge`] and
//!   [`ene_api::v1::handshake::AuthResult`] frames are never
//!   solicited and answer nothing: the Host mints challenges and issues
//!   results.
//! - Authentication is challenge/proof over the pairing secret: capability
//!   answers [`NegotiatedConnection`]
//!   plus a fresh [`ene_api::v1::handshake::AuthChallenge`]
//!   whose nonce is recorded pending for that connection, and a later
//!   [`ene_api::v1::handshake::AuthProof`] verifies (constant time, inside `ene-credential`)
//!   against the secret persisted at approval, consuming the nonce single-use
//!   regardless of outcome. Success answers
//!   [`Accepted`](ene_api::v1::handshake::AuthResult::Accepted) carrying the
//!   connection id the Client echoes on every later frame as the auth
//!   binding; any failure answers
//!   [`Rejected`](ene_api::v1::handshake::AuthResult::Rejected). Pending
//!   nonces live only in [`HostHandle`] memory: a restart drops them
//!   (fail-closed), so Clients re-run capability-plus-proof after a restart.
//!   Pairing secrets live only in the `device-auth.json` file store (plus the
//!   transient approve-time display scope): the handle holds no secret map,
//!   proof verification reads the file through on every authentication, and
//!   there is deliberately no secret cache.
//! - The response sender reveals the connection id only on and after
//!   acceptance: [`Accepted`](ene_api::v1::handshake::AuthResult::Accepted)
//!   and every later (domain) response carry `Some` table connection id, while
//!   every pre-accept response (pairing results and denials, negotiated terms,
//!   challenges, rejections, and unpaired closes) carries [`None`]. A peer
//!   that never completed the challenge therefore never learns the id the
//!   gate requires it to echo. The sender always echoes the inbound
//!   incarnation and names the paired device (or [`None`] pre-pairing).
//! - [`HostHandle::handle_frame`] is infallible by contract: infrastructure
//!   failures map to retry-safe outcome frames (hold or revalidate), never to
//!   fabricated domain facts. The mapping table lives on each pipeline method.
//! - Decoded-but-unhandled inbound variants (reconnect, stream frames from
//!   the Client, facts the Host itself emits) are ignored with an empty
//!   response: they are known [`WirePayload`] variants outside `Stage 2`
//!   scope, and a [`ene_api::v1::handshake::DisconnectNotice`] would carry the wrong semantics for
//!   them. Silence is the explicit `Stage 2` decision for these.
//! - The envelope discriminator must name the decoded payload:
//!   [`HostHandle::handle_frame`] compares `envelope.message_type` against
//!   [`WirePayload::message_type`] and answers a typed
//!   [`Reject`](ene_api::v1::payload::WirePayload::Reject) with
//!   `UnsupportedMessage` when they differ, changing nothing else and
//!   keeping the connection. A future/unknown payload variant itself cannot
//!   reach that reject: [`WirePayload`] is a closed enum decoded as part of
//!   the whole frame, so the codec fails first and [`crate::conn`] closes
//!   the connection. Reaching the typed reject for undecodable variants
//!   needs a wire-format/framing change and is later compatibility
//!   hardening, not a `Stage 2` contract.
//! - A [`ene_api::v1::handshake::DisconnectNotice`] in a
//!   response vector is terminal: [`crate::conn`] writes it and then closes the
//!   connection. Both the major-version mismatch and the unpaired-gate paths
//!   emit one.
//! - [`HostHandle`] methods take `&self`: per-map `std` mutexes, a leaf
//!   tracker mutex, and the store's own lock provide short interior critical
//!   sections, and every guard is dropped before the next await. No
//!   handle-wide async lock spans provider I/O.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex as StdMutex, MutexGuard};

use ene_api::v1::envelope::ProtocolVersion;
use ene_api::v1::handshake::NegotiatedConnection;
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{ConnectionWireId, RoundWireId};
use ene_api::v1::reject::RejectKind;
use ene_companion::{CompanionId, CompanionRepository};
use ene_credential::{
    CredentialApprovalRepository, CredentialRef, CredentialStore, CredentialTechnicalError,
    DevicePairingRepository, DeviceRecord, EnvCredentialStore, FileDeviceAuthStore,
    MemoryCredentialStore,
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
    /// Layered configuration loading or validation failed.
    #[error(transparent)]
    Config(#[from] ene_config::typed::ConfigError),
    /// The data directory or the durable store was unavailable.
    #[error("store unavailable: {0}")]
    Store(String),
    /// The listener socket could not be prepared or bound.
    #[error("bind failed: {0}")]
    Bind(String),
    /// A transport frame could not be encoded or decoded.
    #[error("codec failed: {0}")]
    Codec(String),
    /// The handshake could not be completed.
    #[error("handshake failed: {0}")]
    Handshake(String),
    /// Inference dispatch could not be completed.
    #[error("inference failed: {0}")]
    Inference(String),
    /// Host-local device approval failed: unknown descriptor or store failure.
    ///
    /// Unknown descriptors list the pending descriptors so the Owner can
    /// retry with the exact value; the message carries display strings only.
    #[error("device approval failed: {0}")]
    Approve(String),
    /// The platform has no listener implementation yet.
    ///
    /// Carries a static Hardening note, never runtime data. The Windows
    /// follow-up is a named-pipe listener; see [`crate::conn`].
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(&'static str),
}

/// Bearer store behind the Host handle.
///
/// [`CredentialStore::with_bearer`] is generic over its closure return type,
/// so the trait is not dyn-compatible and the handle holds this closed enum
/// instead of a trait object. [`CredStore::Env`] is the production store (the
/// bearer lives in the process environment and is never cached); [`CredStore::Memory`]
/// is the test and local-development store. The `Clone` and bearer semantics
/// of each variant are unchanged by this dispatch.
#[derive(Debug)]
pub enum CredStore {
    /// Environment-backed bearer store for the `openai` provider.
    Env(EnvCredentialStore),
    /// In-memory bearer store for tests and local development.
    Memory(MemoryCredentialStore),
}

impl CredentialStore for CredStore {
    /// Runs `f` with the bearer for `cred` from the held store.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialTechnicalError::StorageUnavailable`] when the held
    /// store reports the credential unknown, unreadable, or invalid. The error
    /// never carries secret material.
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

    /// Deletes the bearer for `cred` from the held store.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialTechnicalError::StorageUnavailable`] when the held
    /// store rejects the operation.
    fn delete(&self, cred: &CredentialRef) -> Result<(), CredentialTechnicalError> {
        match self {
            Self::Env(inner) => inner.delete(cred),
            Self::Memory(inner) => inner.delete(cred),
        }
    }

    /// Reports whether the held store has a bearer for `cred`.
    ///
    /// Existence is non-secret metadata.
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
/// the same device supersedes this one, flipping `authed` off without
/// touching the record), and `connection_id` is the table key itself. The
/// gate in [`HostHandle::handle_frame`] trusts these conn-filled premises;
/// direct handle callers (tests) construct them explicitly. All fields are
/// public so connection adapters and integration tests can construct the
/// value directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveInput {
    /// Opaque client reference naming the caller for this frame.
    pub client_ref: String,
    /// Whether the underlying connection is currently live.
    pub connection_live: bool,
    /// Whether the peer passed the same-user check.
    pub peer_uid_ok: bool,
    /// Device wire string paired on this connection, if any.
    pub paired_device: Option<String>,
    /// Whether the connection table knows this connection.
    pub connection_known: bool,
    /// Whether this connection is authenticated and current for its device.
    ///
    /// Filled by the connection table, never by the Client: true only after
    /// an [`Accepted`](ene_api::v1::handshake::AuthResult::Accepted) answer
    /// on this connection while no newer authentication by the same device
    /// has superseded it. The ingress gate requires this premise on every
    /// post-capability frame except the proof itself.
    pub authed: bool,
    /// Host-minted connection key for this connection (the table id).
    ///
    /// Filled by the connection table, never by the Client: the ingress gate
    /// requires post-capability envelopes to echo exactly this id, and
    /// [`Accepted`](ene_api::v1::handshake::AuthResult::Accepted) reveals it
    /// to the Client for the first time (pre-accept responses carry [`None`]
    /// instead), so echoing it proves the sender completed the challenge on
    /// this connection.
    pub connection_id: ConnectionWireId,
    /// Negotiated terms recorded when this connection answered capability.
    ///
    /// Filled by the connection table from the Host-selected terms, never
    /// by the Client: the ingress gate enforces the exact negotiated version on
    /// every later frame, so version mixing within one connection is
    /// impossible. [`None`] before negotiation.
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
/// All fields are crate-private except where the Host-local trusted inlets
/// need them: external callers (including integration tests) drive the Host
/// through [`HostHandle::open`] (or
/// [`HostHandle::open_with_cred_store`]) plus [`HostHandle::handle_frame`],
/// [`HostHandle::approve_device`], [`HostHandle::pending_devices`], and
/// [`HostHandle::note_disconnect`], while the pipeline methods in
/// [`crate::dialogue`] and [`crate::setup`] reach the fields as inherent
/// `impl HostHandle` blocks in this crate.
///
/// Interior mutability: `open_rounds` and `rounds` sit behind short `std`
/// mutex sections (clone out before every await, never hold a guard across
/// an await); `tracker` is a leaf async mutex (the inference `send` boundary
/// needs `&mut` across its transport await while only touching the tracker
/// synchronously up front, and the transport never calls back into the
/// handle, so no lock ordering exists); the [`Store`] carries its own lock.
/// Nothing here is durable except through [`Store`] and the device-auth file:
/// a restart drops every map while the database persists, and old wire round
/// refs then surface as stale (never rebound). Pairing, device-auth secrets,
/// and idempotency are durable instead: the device tables and the history
/// `local_id` column live in [`Store`], and pairing secrets live in
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
    /// File-backed pairing-secret store by device.
    ///
    /// Opened on `<data_dir>/device-auth.json` by
    /// [`HostHandle::open_with_cred_store`]. Secrets live here and in the
    /// transient approve-time display scope only: the handle keeps no secret
    /// map and no cache, and proof verification reads the file through on
    /// every authentication. Backup-exclusion: this file holds Group K
    /// verification material with E classification and must never enter
    /// backups or exports (see the [`FileDeviceAuthStore`] contract); a
    /// future backup stage walking the data directory must exclude it by
    /// name.
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
    /// Ensures `data_dir` exists (`0700` on Unix; plain creation elsewhere)
    /// and opens `app.db` inside it through [`Store::open`]. `Stage 2` owns
    /// directory creation: resolution stays pure in `ene-config` while the
    /// side effect lives here.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Store`] when the directory cannot be ensured or
    /// the database cannot be opened or migrated. Messages carry the backend
    /// cause only, never paths, secrets, or body text.
    pub async fn open(data_dir: &Path) -> Result<Self, CoreError> {
        Self::open_with_cred_store(data_dir, CredStore::Env(EnvCredentialStore::new())).await
    }

    /// Opens (or creates) the Host state under `data_dir` with an explicit
    /// credential store.
    ///
    /// Same as [`HostHandle::open`] except for the store: integration tests
    /// pass [`CredStore::Memory`] pre-provisioned with test bearers, which
    /// keeps them hermetic (the environment store would read the real process
    /// environment on every call). The file-backed device-auth store opens on
    /// `<data_dir>/device-auth.json` (created lazily on first approval); the
    /// data directory itself is ensured first, so the open always has its
    /// parent.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Store`] under the same conditions as
    /// [`HostHandle::open`], plus when the device-auth file cannot be opened
    /// (unreadable, malformed, or wrongly permissioned).
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
        Ok(Self {
            store,
            tracker: AsyncMutex::new(EvaluationTracker::new()),
            open_rounds: StdMutex::new(HashMap::new()),
            rounds: StdMutex::new(HashMap::new()),
            cred_store,
            auth_store,
            pending_nonces: StdMutex::new(HashMap::new()),
            companion_wire: RawId::new().as_uuid().to_string(),
        })
    }

    /// Returns the opaque companion projection this handle issues.
    ///
    /// Test scaffolding and the mapping check share one vocabulary through
    /// this accessor; Clients learn the value from presence facts instead.
    pub(crate) fn companion_wire(&self) -> &str {
        &self.companion_wire
    }

    /// Resolves an inbound companion wire ref to its domain companion.
    ///
    /// The domain wire-ref mapping for the single Stage 2 companion: only
    /// the projection this handle issued resolves, to the running
    /// companion; any other string is unknown — never guessed, never
    /// parsed, never derived. Callers answer `unknown-companion`
    /// revalidation (or an empty view) on `Ok(None)`, so a rotated
    /// projection (restart) converges through one revalidation round trip.
    /// Store failures stay errors (the caller holds), distinct from
    /// unknown refs.
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
    /// fake and production passes the `OpenAI` transport. The returned frames
    /// carry response envelopes (paired device or [`None`] pre-pairing, the
    /// inbound incarnation echoed, `reply_to` set to the inbound message id)
    /// whose sender reveals the table connection id only on and after
    /// [`Accepted`](ene_api::v1::handshake::AuthResult::Accepted): domain
    /// responses and the acceptance itself carry `Some` id, while every
    /// pre-accept response (pairing results, negotiated terms, challenges,
    /// rejections, denials, unpaired closes) carries [`None`].
    ///
    /// Ingress rules by frame kind: [`ene_api::v1::handshake::PairingRequest`] frames are refused
    /// once the connection already holds a paired device (one connection,
    /// one device); unpaired connections need no other check.
    /// [`ene_api::v1::handshake::CapabilityAdvertise`] frames need the paired-device check only (they
    /// predate authentication); [`ene_api::v1::handshake::AuthProof`]
    /// frames need none (they ARE the authentication); inbound
    /// [`ene_api::v1::handshake::AuthChallenge`] and
    /// [`ene_api::v1::handshake::AuthResult`] frames are never
    /// solicited and answer nothing. Dispatch then runs:
    /// [`SubmitTextInput`](ene_api::v1::round::SubmitTextInput),
    /// [`ConfirmPresentation`](ene_api::v1::round::ConfirmPresentationWire), and
    /// [`HistoryRequest`](ene_api::v1::round::HistoryRequest) in
    /// [`crate::dialogue`];
    /// [`ManagementIntent`](ene_api::v1::management::ManagementIntent)
    /// and [`ManagementViewRequest`](ene_api::v1::management::ManagementViewRequest)
    /// in [`crate::setup`]. Every other unhandled variant likewise returns an
    /// empty vector (observation or deferred scope; see the module gap note).
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

    /// Resolves an issued wire round string back to its domain round.
    pub(crate) fn round_for(&self, wire: &str) -> Option<RoundId> {
        lock_map(&self.rounds).get(wire).copied()
    }

    /// Returns the open round for a client/companion pair, if any.
    ///
    /// Cloned out under one short section; the caller never holds the guard.
    pub(crate) fn open_round_for(
        &self,
        client_ref: &str,
        companion_key: &str,
    ) -> Option<OpenRound> {
        lock_map(&self.open_rounds)
            .get(&(client_ref.to_string(), companion_key.to_string()))
            .copied()
    }

    /// Records the open round for a client/companion pair.
    pub(crate) fn record_open_round(&self, client_ref: &str, companion_key: &str, open: OpenRound) {
        lock_map(&self.open_rounds)
            .insert((client_ref.to_string(), companion_key.to_string()), open);
    }

    /// Resolves a domain round back to its issued wire string, if still mapped.
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
    /// Host-local trusted surface only: the caller shows it once and
    /// forgets it. The secret is additionally persisted through the
    /// file-backed `auth_store` under the approved device, so
    /// later [`ene_api::v1::handshake::AuthProof`] frames verify against the file; the handle keeps
    /// no in-memory copy and no cache. The persisted copy is never logged
    /// and never rendered in `Debug`.
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

    /// Lists the descriptors of all currently pending pairing requests.
    ///
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
    /// # Errors
    ///
    /// Returns [`CoreError::Store`] when the durable tables are unavailable.
    pub async fn approve_credential(&self, provider: &str, label: &str) -> Result<bool, CoreError> {
        // One atomic store call: the pending drain and the usable-ref insert
        // share a transaction, so a crash cannot strand an approval with no
        // usable marker. The usable ref id follows the `provider:label`
        // convention both sides already use for assignment.
        CredentialApprovalRepository::approve_pending(&self.store, provider, label)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))
    }

    /// Lists pending credential approvals as `provider:label` strings.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Store`] when the durable tables are unavailable.
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
