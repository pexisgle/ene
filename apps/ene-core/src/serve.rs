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
//!   [`PairingRequest`] records a pending request, the Host-local
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
//!   [`DisconnectNotice`] with reason `"unpaired"` and nothing else. There is
//!   no generic reject DTO in `ene-api`, so silence-plus-close (rather than
//!   an oracle denial) is the explicit decision. Pairing frames carry no
//!   checks; capability frames need the paired-device check only (they predate
//!   authentication); [`AuthProof`] frames
//!   need none (they ARE the authentication). Inbound
//!   [`AuthChallenge`] and
//!   [`AuthResult`] frames are never
//!   solicited and answer nothing: the Host mints challenges and issues
//!   results.
//! - Authentication is challenge/proof over the pairing secret: capability
//!   answers [`NegotiatedConnection`]
//!   plus a fresh [`AuthChallenge`]
//!   whose nonce is recorded pending for that connection, and a later
//!   [`AuthProof`] verifies (constant time, inside `ene-credential`)
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
//! - Unknown or deferred inbound variants (reconnect, stream frames from the
//!   Client, facts the Host itself emits) are ignored with an empty response.
//!   There is no `UnsupportedMessage` DTO in `ene-api`, and a
//!   [`DisconnectNotice`] would carry
//!   the wrong semantics for a merely unhandled message, so silence plus this
//!   gap note is the explicit `Stage 2` decision. `Stage 2` transport work may
//!   add a typed reject.
//! - A [`DisconnectNotice`] in a
//!   response vector is terminal: [`crate::conn`] writes it and then closes the
//!   connection. Both the major-version mismatch and the unpaired-gate paths
//!   emit one.
//! - [`HostHandle`] methods take `&self`: per-map `std` mutexes, a leaf
//!   tracker mutex, and the store's own lock provide short interior critical
//!   sections, and every guard is dropped before the next await. No
//!   handle-wide async lock spans provider I/O.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};

use ene_api::v1::envelope::{ProtocolVersion, WireEnvelope, WireSender, new_outgoing_envelope};
use ene_api::v1::handshake::{
    AuthChallenge, AuthProof, AuthResult, CapabilityAdvertise, DisconnectNotice,
    NegotiatedConnection, PairingRequest, PairingResult,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::DeviceWireId;
use ene_api::v1::refs::{ConnectionWireId, WireMessageId, WireMessageType};
use ene_companion::CompanionRepository;
use ene_credential::{
    CredentialApprovalRepository, CredentialRef, CredentialRefRepository, CredentialStore,
    CredentialTechnicalError, DeviceId, DevicePairingRepository, DevicePairingStatus, DeviceRecord,
    EnvCredentialStore, FileDeviceAuthStore, MemoryCredentialStore,
};
use ene_inference::ProviderTransport;
use ene_inference::provider::{DEFAULT_BASE_URL, OpenAiResponsesTransport};
use ene_permission::EvaluationTracker;
use ene_presence::{
    ClientId, LiveReachabilityRef, MoveDecision, PresenceCheckRef, PresenceRepository,
    PresenceState, ThinMoveReason,
};
use ene_presentation::{OpenRound, RoundId};
use ene_primitive::RawId;
use ene_store::Store;
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

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

/// Parses a hyphenated device wire string back into its domain identity.
///
/// Yields [`None`] for anything that is not UUID text. Proof verification
/// treats an unparsable device exactly like a missing secret (a rejected
/// proof with the same reason string), so a malformed table entry can never
/// become an oracle.
fn parse_device_id(text: &str) -> Option<DeviceId> {
    text.parse::<Uuid>()
        .ok()
        .map(RawId::from_uuid)
        .map(DeviceId)
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
        })
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
    /// Ingress rules by frame kind: [`PairingRequest`] frames carry no checks;
    /// [`CapabilityAdvertise`] frames need the paired-device check only (they
    /// predate authentication); [`AuthProof`]
    /// frames need none (they ARE the authentication); inbound
    /// [`AuthChallenge`] and
    /// [`AuthResult`] frames are never
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
            #[expect(
                clippy::match_same_arms,
                reason = "the empty answer is intentional for both arms; the explicit arm documents that inbound challenges/results are never solicited"
            )]
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
    /// There is no generic reject DTO in `ene-api`, so tripping drops
    /// unauthenticated domain service with a terminal disconnect rather than
    /// an oracle denial.
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

    /// Records a domain round under its wire string for later resolution.
    pub(crate) fn record_round(&self, wire: &str, round: RoundId) {
        lock_map(&self.rounds).insert(wire.to_string(), round);
    }

    /// Records the Owner approval of one pending pairing request.
    ///
    /// Host-local trusted inlet behind the `approve-device` subcommand: it
    /// records the Owner decision through
    /// [`approve_pending`](DevicePairingRepository::approve_pending) and never
    /// decides whether pairing is allowed itself. An unknown descriptor
    /// yields `Ok(None)` (the caller lists [`HostHandle::pending_devices`]);
    /// a blank descriptor can never match because wire ingress denies blank
    /// descriptors before they reach the store.
    /// Records one Owner pairing approval and mints its one-time secret.
    ///
    /// The returned secret string is for one-time display on this
    /// Host-local trusted surface only: the caller shows it once and
    /// forgets it. The secret is additionally persisted through the
    /// file-backed `auth_store` under the approved device, so
    /// later [`AuthProof`] frames verify against the file; the handle keeps
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
        let approved = CredentialApprovalRepository::approve_pending(&self.store, provider, label)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        if !approved {
            return Ok(false);
        }
        let credential = CredentialRef {
            id: format!("{provider}:{label}"),
            provider: provider.to_string(),
            label: label.to_string(),
        };
        self.store
            .save_ref(credential)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        Ok(true)
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
        if self
            .store
            .confirm_transition(companion.as_raw(), generation, premise)
            .await
            .is_err()
        {
            // The transition stays unconfirmed; the next intake reads the
            // `InTransition` attribution and reports held, which is honest.
        }
    }

    /// Handles one [`PairingRequest`]: deny unauthorized or blank peers, else
    /// record the request durably.
    ///
    /// Denial carries an operational reason only. A non-blank descriptor goes
    /// to
    /// [`request_pairing`](DevicePairingRepository::request_pairing): an
    /// already-paired descriptor re-issues its device key as
    /// [`Paired`](PairingResult::Paired) (the connection layer then marks the
    /// connection paired), while a fresh descriptor is recorded pending and
    /// answers
    /// [`PendingOwnerConfirmation`](PairingResult::PendingOwnerConfirmation)
    /// until the Host-local `approve-device` inlet records the Owner
    /// decision. Ingress trims surrounding whitespace and denies blank
    /// descriptors with [`Denied`](PairingResult::Denied): the pairing
    /// outcome has no `NeedsClarification` variant, so refusal is the honest
    /// shape. A store failure likewise denies (operational reason only); the
    /// Client retries the same request, which is idempotent. Every answer
    /// here predates authentication, so its sender hides the connection id.
    async fn pair(
        &self,
        frame: &WireFrame,
        request: &PairingRequest,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        if !live.peer_uid_ok {
            return vec![denied_pairing(frame, live, "peer user mismatch")];
        }
        let descriptor = request.device_descriptor.trim().to_string();
        if descriptor.is_empty() {
            return vec![denied_pairing(frame, live, "blank device descriptor")];
        }
        match DevicePairingRepository::request_pairing(&self.store, descriptor).await {
            Ok(DevicePairingStatus::Paired { device }) => {
                let device_id = DeviceWireId(device.id.0.as_uuid());
                vec![outgoing_frame_pre_auth(
                    frame,
                    live,
                    "PairingResult",
                    WirePayload::PairingResult(PairingResult::Paired { device_id }),
                )]
            }
            Ok(DevicePairingStatus::Pending { .. }) => vec![outgoing_frame_pre_auth(
                frame,
                live,
                "PairingResult",
                WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation),
            )],
            Err(_) => vec![denied_pairing(frame, live, "pairing store unavailable")],
        }
    }

    /// Handles one [`CapabilityAdvertise`]: negotiate, then challenge.
    ///
    /// When no advertised version shares the v1 major, the reply is a single
    /// terminal [`DisconnectNotice`] (the connection closes after it is
    /// written; there is no `IncompatibleProtocol` DTO in `ene-api`).
    /// Otherwise the reply carries the negotiated terms (version v1 and every
    /// advertised feature kind as receipt, never as permission) plus a fresh
    /// [`AuthChallenge`] whose nonce is recorded pending for this connection:
    /// the Client answers with an [`AuthProof`] proving possession of its
    /// pairing secret. Re-advertising replaces the pending nonce, so only the
    /// latest challenge can be answered. Both answers predate authentication,
    /// so their senders hide the connection id. Capability frames never
    /// attach presence: attach happens only on the submit path, so a
    /// negotiating-but-never-submitting peer leaves attribution untouched.
    fn advertise(
        &self,
        frame: &WireFrame,
        advertise: &CapabilityAdvertise,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        let negotiable = advertise
            .supported_protocol
            .iter()
            .any(|candidate| candidate.shares_major_with(&ProtocolVersion::V1));
        if !negotiable {
            let notice = DisconnectNotice {
                reason: String::from("incompatible protocol major"),
            };
            return vec![outgoing_frame_pre_auth(
                frame,
                live,
                "DisconnectNotice",
                WirePayload::DisconnectNotice(notice),
            )];
        }
        let accepted = advertise
            .features
            .iter()
            .map(|feature| feature.kind)
            .collect();
        let negotiated = NegotiatedConnection {
            version: ProtocolVersion::V1,
            accepted_features: accepted,
        };
        let nonce = Uuid::new_v4().as_hyphenated().to_string();
        lock_map(&self.pending_nonces).insert(conn_key(&live.connection_id), nonce.clone());
        vec![
            outgoing_frame_pre_auth(
                frame,
                live,
                "NegotiatedConnection",
                WirePayload::NegotiatedConnection(negotiated),
            ),
            outgoing_frame_pre_auth(
                frame,
                live,
                "AuthChallenge",
                WirePayload::AuthChallenge(AuthChallenge { nonce }),
            ),
        ]
    }

    /// Handles one [`AuthProof`]: verify against the persisted secret and answer.
    ///
    /// No prior auth is required: this frame IS the authentication. The
    /// pending nonce for this connection is consumed single-use regardless of
    /// outcome — a missing nonce, a missing sender device, a missing or
    /// unreadable secret, or a bad proof all answer
    /// [`Rejected`](ene_api::v1::handshake::AuthResult::Rejected) with an
    /// operational reason — so a captured proof can never replay. The secret
    /// loads from the file-backed `auth_store` on every call:
    /// there is no cache, so rotations and revocations take effect on the
    /// next authentication. Success answers
    /// [`Accepted`](ene_api::v1::handshake::AuthResult::Accepted)
    /// carrying this connection's table id, which the Client echoes on every
    /// later frame as the auth binding the gate checks; the acceptance (and
    /// its piggybacked presence fact) is the first response on this
    /// connection to reveal the id, while every rejection hides it. Proof
    /// comparison itself runs in constant time inside `ene-credential`.
    async fn verify_proof(
        &self,
        frame: &WireFrame,
        proof: &AuthProof,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        let nonce = lock_map(&self.pending_nonces).remove(&conn_key(&live.connection_id));
        // Device attribution comes from the connection table (paired moments
        // earlier on this same connection), never from the envelope claim:
        // the proof authenticates the pending pairing the Host recorded, and
        // trusting a Client-supplied device here would let any peer claim
        // any identity.
        let device = live.paired_device.clone();
        let reason = match (nonce, device) {
            (Some(nonce), Some(device)) => {
                let verified = parse_device_id(&device).is_some_and(|id| {
                    matches!(
                        self.auth_store
                            .verify_device_proof(&id, &nonce, &proof.proof),
                        Ok(true)
                    )
                });
                if verified {
                    None
                } else {
                    Some("invalid proof")
                }
            }
            (Some(_), None) => Some("unknown device"),
            (None, _) => Some("no pending challenge"),
        };
        match reason {
            None => {
                let mut out = vec![outgoing_frame(
                    frame,
                    live,
                    "AuthResult",
                    WirePayload::AuthResult(AuthResult::Accepted {
                        connection_id: live.connection_id,
                    }),
                )];
                if let Ok(companion) = self.store.ensure_running_companion().await
                    && let Ok(Some(attribution)) =
                        self.store.load_attribution(companion.as_raw()).await
                {
                    out.push(outgoing_frame(
                        frame,
                        live,
                        "PresenceAttribution",
                        WirePayload::PresenceAttribution(attribution_to_wire(&attribution)),
                    ));
                }
                out
            }
            Some(reason) => vec![outgoing_frame_pre_auth(
                frame,
                live,
                "AuthResult",
                WirePayload::AuthResult(AuthResult::Rejected {
                    reason: reason.to_string(),
                }),
            )],
        }
    }
}

/// Maps a durable attribution to its wire fact: refs stay readable,
/// generation travels as a value copy. Reporting only, never authority.
fn attribution_to_wire(
    attribution: &ene_presence::PresenceAttribution,
) -> ene_api::v1::presence::PresenceAttributionWire {
    use ene_api::v1::presence::PresenceStateWire;
    use ene_api::v1::refs::{ClientWireRef, CompanionWireRef};
    use ene_presence::PresenceState;
    ene_api::v1::presence::PresenceAttributionWire {
        companion: CompanionWireRef(attribution.companion.as_uuid().to_string()),
        state: match attribution.state {
            PresenceState::Present => PresenceStateWire::Present,
            PresenceState::NoActive => PresenceStateWire::NoActive,
            PresenceState::InTransition => PresenceStateWire::InTransition,
            PresenceState::Stopped => PresenceStateWire::Stopped,
            PresenceState::RecoveryWait => PresenceStateWire::RecoveryWait,
        },
        active_client: attribution
            .active_client
            .as_ref()
            .map(|client| ClientWireRef(client.as_raw().as_uuid().to_string())),
        generation: attribution.generation.as_u64(),
        move_reason: None,
    }
}

/// Builds a pairing denial frame with an operational reason only.
///
/// A denial predates authentication, so its sender hides the connection id.
fn denied_pairing(frame: &WireFrame, live: &LiveInput, reason: &str) -> WireFrame {
    outgoing_frame_pre_auth(
        frame,
        live,
        "PairingResult",
        WirePayload::PairingResult(PairingResult::Denied {
            reason: reason.to_string(),
        }),
    )
}

/// Builds the terminal gate frame dropping unauthenticated domain service.
///
/// The connection closes after this frame is written. The `"unpaired"` reason
/// names the gate trip only; no generic reject DTO exists in `ene-api`, so a
/// disconnect (rather than an oracle denial) is the explicit decision. The
/// gate trips exactly when the sender is not authenticated, so the frame
/// hides the connection id: a peer that never completed the challenge must
/// not learn it from the drop.
pub(crate) fn unpaired_close(frame: &WireFrame, live: &LiveInput) -> WireFrame {
    outgoing_frame_pre_auth(
        frame,
        live,
        "DisconnectNotice",
        WirePayload::DisconnectNotice(DisconnectNotice {
            reason: String::from("unpaired"),
        }),
    )
}

/// Builds the Host sender for one response to `frame` under `live`.
///
/// Host-to-Client addressing always echoes the inbound incarnation (so the
/// Client pairs the response with its connection state) and names the paired
/// device target when this connection paired one (parsed back from the
/// hyphenated wire string the table holds; an unparsable entry — never
/// written by this Host — maps to [`None`]); pre-pairing responses carry
/// device [`None`]. The connection id travels only when `reveal_connection`
/// holds: acceptance and later domain responses reveal this connection's
/// table id, while every pre-accept response hides it ([`None`]), so a peer
/// that never completed the challenge never learns the id the gate requires
/// it to echo.
fn response_sender(frame: &WireFrame, live: &LiveInput, reveal_connection: bool) -> WireSender {
    WireSender {
        device_id: live
            .paired_device
            .as_deref()
            .and_then(|text| Uuid::parse_str(text).ok())
            .map(DeviceWireId),
        incarnation_id: frame.envelope.sender.incarnation_id,
        connection_id: reveal_connection.then_some(live.connection_id),
    }
}

/// Builds an outgoing envelope for a `Stage 2` message type.
///
/// `reply_to` links the response to its request for transport pairing; domain
/// correspondence travels in the payloads, never here. The sender follows
/// [`response_sender`]: the paired device (or [`None`] pre-pairing), the
/// inbound incarnation echoed, and this connection's table id.
pub(crate) fn outgoing_envelope(
    frame: &WireFrame,
    live: &LiveInput,
    message_type: &str,
    reply_to: Option<WireMessageId>,
) -> WireEnvelope {
    outgoing_envelope_inner(frame, live, message_type, reply_to, true)
}

/// Builds a pre-accept outgoing envelope for a `Stage 2` message type.
///
/// Same as [`outgoing_envelope`] except the sender hides the connection id
/// ([`None`]): pairing results and denials, negotiated terms, challenges,
/// rejections, and unpaired closes all predate the acceptance that first
/// reveals the id, so none of them may carry it.
pub(crate) fn outgoing_envelope_pre_auth(
    frame: &WireFrame,
    live: &LiveInput,
    message_type: &str,
    reply_to: Option<WireMessageId>,
) -> WireEnvelope {
    outgoing_envelope_inner(frame, live, message_type, reply_to, false)
}

/// Builds an outgoing envelope with an explicit connection-id reveal rule.
fn outgoing_envelope_inner(
    frame: &WireFrame,
    live: &LiveInput,
    message_type: &str,
    reply_to: Option<WireMessageId>,
    reveal_connection: bool,
) -> WireEnvelope {
    let mut envelope = new_outgoing_envelope(
        ProtocolVersion::V1,
        response_sender(frame, live, reveal_connection),
        WireMessageType(message_type.to_string()),
    );
    envelope.correlation.reply_to = reply_to;
    envelope
}

/// Builds one response frame answering `frame` with `payload`.
///
/// The envelope follows the `Stage 2` `message_type` convention documented on
/// the crate root and links back through `reply_to`, revealing the connection
/// through [`response_sender`]. Use only on and after
/// [`Accepted`](ene_api::v1::handshake::AuthResult::Accepted): the acceptance
/// itself, the piggybacked presence fact, and every domain response.
pub(crate) fn outgoing_frame(
    frame: &WireFrame,
    live: &LiveInput,
    message_type: &str,
    payload: WirePayload,
) -> WireFrame {
    WireFrame {
        envelope: outgoing_envelope(frame, live, message_type, Some(frame.envelope.message_id)),
        payload,
    }
}

/// Builds one pre-accept response frame answering `frame` with `payload`.
///
/// Same as [`outgoing_frame`] except the sender hides the connection id:
/// pairing results and denials, negotiated terms, challenges, rejections,
/// and unpaired closes must not reveal the id the gate later requires the
/// Client to echo.
pub(crate) fn outgoing_frame_pre_auth(
    frame: &WireFrame,
    live: &LiveInput,
    message_type: &str,
    payload: WirePayload,
) -> WireFrame {
    WireFrame {
        envelope: outgoing_envelope_pre_auth(
            frame,
            live,
            message_type,
            Some(frame.envelope.message_id),
        ),
        payload,
    }
}

/// Ensures the Host data directory exists.
#[cfg(unix)]
fn ensure_data_dir(data_dir: &Path) -> Result<(), CoreError> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(data_dir)
        .map_err(|error| CoreError::Store(format!("create data directory: {error}")))?;
    // Creation mode applies only to created directories: a pre-existing dir
    // keeps whatever mode it had, which may predate this Host. The
    // same-machine socket trust premise needs owner-only, so tighten rather
    // than serve exposed; a tighten failure fails startup (fail-closed).
    let mode = std::fs::metadata(data_dir)
        .map_err(|error| CoreError::Store(format!("stat data directory: {error}")))?
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o077 != 0 {
        std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| CoreError::Store(format!("protect data directory: {error}")))?;
    }
    Ok(())
}

/// Ensures the Host data directory exists.
#[cfg(not(unix))]
fn ensure_data_dir(data_dir: &Path) -> Result<(), CoreError> {
    std::fs::create_dir_all(data_dir)
        .map_err(|error| CoreError::Store(format!("create data directory: {error}")))?;
    Ok(())
}

/// Runs the `Stage 2` Host: opens state, builds transport, serves the socket.
///
/// The inference transport binds the credential resolved at startup
/// ([`crate::setup`] documents the resolution); per-frame consent checks stay
/// authoritative, and the environment bearer store is label-insensitive within
/// the `openai` provider, so a later consent reassignment cannot silently
/// misbill. Rebinding the transport on consent change is deferred hardening.
/// Socket-path assembly stays inside [`crate::conn`]: this entry point passes
/// the data directory, never the socket path.
///
/// # Errors
///
/// Returns [`CoreError::Store`] when the state cannot be opened and
/// [`CoreError::Bind`] (or [`CoreError::UnsupportedPlatform`]) when the
/// listener cannot run.
pub async fn serve(data_dir: &Path) -> Result<(), CoreError> {
    let handle = HostHandle::open(data_dir).await?;
    let credential = handle.startup_credential().await;
    let transport =
        OpenAiResponsesTransport::new(DEFAULT_BASE_URL, credential, EnvCredentialStore::new());
    crate::conn::run(
        data_dir.to_path_buf(),
        Arc::new(handle),
        Arc::new(transport),
    )
    .await
}

use ene_plugin_ipc::WireFrame;

#[cfg(test)]
mod tests {
    use super::{HostHandle, LiveInput, conn_key, device_client};
    use crate::test_support::{live_input, memory_handle, remove_data_dir};
    use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
    use ene_api::v1::handshake::{
        AuthChallenge, AuthProof, AuthResult, CapabilityAdvertise, PairingRequest, PairingResult,
    };
    use ene_api::v1::payload::WirePayload;
    use ene_api::v1::refs::{ClientIncarnationId, ConnectionWireId, DeviceWireId, WireMessageType};
    use ene_api::v1::refs::{ClientLocalId, CompanionWireRef, TextLangWire};
    use ene_api::v1::round::{HistoryRequest, SubmitTextInput, TextBodyWire};
    use ene_credential::pairing_proof_hex;
    use ene_inference::fake::FakeProviderTransport;
    use ene_presence::{ClientId, PresenceRepository};

    fn sender() -> WireSender {
        WireSender {
            device_id: None,
            incarnation_id: ClientIncarnationId {
                counter: 1,
                random: 2,
            },
            connection_id: None,
        }
    }

    /// Stamps a frame with the connection binding its `live` premises carry.
    ///
    /// Direct-handle tests build envelopes by hand, so every post-capability
    /// frame needs this stamp to pass the gate the same way a
    /// connection-table-built frame would.
    fn stamped(mut frame: super::WireFrame, live: &LiveInput) -> super::WireFrame {
        frame.envelope.sender.connection_id = Some(live.connection_id);
        frame
    }

    /// Builds the paired [`LiveInput`] premises for one device on a fresh id.
    ///
    /// Constructed explicitly (never the shared helper): auth-flow tests bind
    /// several frames to one connection, so the id must stay fixed across
    /// them. `authed` holds: these premises stand in for a connection table
    /// entry after its `Accepted`, so post-accept domain frames pass the
    /// gate; bypass tests override it explicitly.
    fn paired_input(device_wire: &str) -> LiveInput {
        LiveInput {
            client_ref: device_wire.to_string(),
            connection_live: true,
            peer_uid_ok: true,
            paired_device: Some(device_wire.to_string()),
            connection_known: true,
            authed: true,
            connection_id: ConnectionWireId(uuid::Uuid::new_v4()),
        }
    }

    fn pairing_frame(descriptor: &str) -> super::WireFrame {
        super::WireFrame {
            envelope: new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(String::from("PairingRequest")),
            ),
            payload: WirePayload::PairingRequest(PairingRequest {
                device_descriptor: descriptor.to_string(),
            }),
        }
    }

    fn advertise_frame(protocol: ProtocolVersion) -> super::WireFrame {
        super::WireFrame {
            envelope: new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(String::from("CapabilityAdvertise")),
            ),
            payload: WirePayload::CapabilityAdvertise(CapabilityAdvertise {
                supported_protocol: vec![protocol],
                features: Vec::new(),
                platform: String::from("test"),
            }),
        }
    }

    fn submit_frame() -> super::WireFrame {
        super::WireFrame {
            envelope: new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(String::from("SubmitTextInput")),
            ),
            payload: WirePayload::SubmitTextInput(SubmitTextInput {
                companion: CompanionWireRef(String::from("companion-echo")),
                round: None,
                local_id: ClientLocalId(String::from("local-1")),
                body: TextBodyWire {
                    text: String::from("hello"),
                    lang: TextLangWire(String::from("en")),
                },
            }),
        }
    }

    fn history_frame() -> super::WireFrame {
        super::WireFrame {
            envelope: new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(String::from("HistoryRequest")),
            ),
            payload: WirePayload::HistoryRequest(HistoryRequest {
                companion: CompanionWireRef(String::from("companion-echo")),
                since: None,
                limit: 10,
            }),
        }
    }

    fn unpaired_input() -> LiveInput {
        LiveInput {
            paired_device: None,
            connection_known: false,
            ..live_input("client-a")
        }
    }

    fn fake_transport() -> FakeProviderTransport {
        FakeProviderTransport::new(String::new(), None)
    }

    async fn open_handle(tag: &str) -> Option<(HostHandle, std::path::PathBuf)> {
        memory_handle(tag).await
    }

    #[test]
    fn device_mapping_is_deterministic_per_device() {
        let first = device_client("laptop");
        let second = device_client("laptop");
        assert_eq!(first, second, "one device maps to one client");
        let other = device_client("phone");
        assert_ne!(first, other, "different devices map to different clients");
    }

    #[test]
    fn device_mapping_is_not_issuance() {
        let mapped = device_client("laptop");
        assert_ne!(
            mapped,
            ClientId::generate(),
            "the mapping never mints a fresh identity"
        );
    }

    #[tokio::test]
    async fn pairing_denies_an_unauthorized_peer() {
        let Some((handle, dir)) = open_handle("pair-deny").await else {
            return;
        };
        let denied_input = LiveInput {
            peer_uid_ok: false,
            ..live_input("client-a")
        };
        let transport = fake_transport();
        let responses = handle
            .handle_frame(pairing_frame("laptop"), denied_input, &transport)
            .await;
        assert_eq!(responses.len(), 1, "denial answers exactly one frame");
        let Some(first) = responses.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &first.payload,
                WirePayload::PairingResult(PairingResult::Denied { .. })
            ),
            "an unauthorized peer is denied"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn pairing_denies_a_blank_descriptor() {
        let Some((handle, dir)) = open_handle("pair-blank").await else {
            return;
        };
        let transport = fake_transport();
        for descriptor in ["", "   "] {
            let responses = handle
                .handle_frame(
                    pairing_frame(descriptor),
                    live_input("client-a"),
                    &transport,
                )
                .await;
            assert_eq!(responses.len(), 1, "blank denial answers once");
            let Some(first) = responses.first() else {
                continue;
            };
            assert!(
                matches!(
                    &first.payload,
                    WirePayload::PairingResult(PairingResult::Denied { .. })
                ),
                "a blank descriptor is denied, never pended"
            );
        }
        let pending = handle.pending_devices().await;
        let Ok(descriptors) = pending else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            descriptors.is_empty(),
            "blank descriptors leave no pending entry"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn pairing_pends_then_pairs_after_owner_approval() {
        let Some((handle, dir)) = open_handle("pair-flow").await else {
            return;
        };
        let transport = fake_transport();
        let pending = handle
            .handle_frame(pairing_frame("laptop"), live_input("client-a"), &transport)
            .await;
        assert_eq!(pending.len(), 1, "the request answers once");
        let Some(first) = pending.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &first.payload,
                WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation)
            ),
            "a fresh descriptor pends, never auto-approves"
        );
        let listed = handle.pending_devices().await;
        let Ok(descriptors) = listed else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            descriptors.iter().any(|name| name == "laptop"),
            "the pending descriptor lists for the Owner"
        );
        let unknown = handle.approve_device("unknown box").await;
        let Ok(None) = unknown else {
            remove_data_dir(&dir);
            return;
        };
        let approved = handle.approve_device("laptop").await;
        let Ok(Some(_)) = approved else {
            remove_data_dir(&dir);
            return;
        };
        let paired_frame = pairing_frame("laptop");
        let expected_reply = paired_frame.envelope.message_id;
        let paired = handle
            .handle_frame(paired_frame, live_input("client-a"), &transport)
            .await;
        let Some(answer) = paired.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &answer.payload,
                WirePayload::PairingResult(PairingResult::Paired { .. })
            ),
            "an approved descriptor pairs on re-request"
        );
        assert_eq!(
            answer.envelope.correlation.reply_to,
            Some(expected_reply),
            "the reply links back to the request"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn unpaired_domain_frames_close_with_an_unpaired_notice() {
        let Some((handle, dir)) = open_handle("gate-drop").await else {
            return;
        };
        let transport = fake_transport();
        let responses = handle
            .handle_frame(submit_frame(), unpaired_input(), &transport)
            .await;
        assert_eq!(responses.len(), 1, "the gate answers once");
        let Some(first) = responses.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &first.payload,
                WirePayload::DisconnectNotice(notice) if notice.reason == "unpaired"
            ),
            "an unpaired submit closes with the unpaired notice"
        );
        let history = handle
            .handle_frame(history_frame(), unpaired_input(), &transport)
            .await;
        let Some(view) = history.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &view.payload,
                WirePayload::DisconnectNotice(notice) if notice.reason == "unpaired"
            ),
            "an unpaired history request closes with the unpaired notice"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn unknown_connection_closes_even_with_a_device() {
        let Some((handle, dir)) = open_handle("gate-unknown").await else {
            return;
        };
        let transport = fake_transport();
        let input = LiveInput {
            paired_device: Some(String::from("laptop")),
            connection_known: false,
            ..live_input("client-a")
        };
        let responses = handle.handle_frame(submit_frame(), input, &transport).await;
        let Some(first) = responses.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &first.payload,
                WirePayload::DisconnectNotice(notice) if notice.reason == "unpaired"
            ),
            "an unknown connection closes even when it names a device"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn unsolicited_challenge_and_result_answer_nothing() {
        let Some((handle, dir)) = open_handle("auth-deferred").await else {
            return;
        };
        let transport = fake_transport();
        let challenge = super::WireFrame {
            envelope: new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(String::from("AuthChallenge")),
            ),
            payload: WirePayload::AuthChallenge(AuthChallenge {
                nonce: String::from("nonce-1"),
            }),
        };
        let result = super::WireFrame {
            envelope: new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(String::from("AuthResult")),
            ),
            payload: WirePayload::AuthResult(AuthResult::Rejected {
                reason: String::from("nope"),
            }),
        };
        for frame in [challenge, result] {
            let responses = handle
                .handle_frame(frame, unpaired_input(), &transport)
                .await;
            assert!(
                responses.is_empty(),
                "unsolicited challenge/result frames answer nothing, even unpaired"
            );
        }
        remove_data_dir(&dir);
    }

    fn proof_frame(device_id: DeviceWireId, proof: &str) -> super::WireFrame {
        let mut envelope = new_outgoing_envelope(
            ProtocolVersion::V1,
            WireSender {
                device_id: Some(device_id),
                incarnation_id: ClientIncarnationId {
                    counter: 5,
                    random: 6,
                },
                connection_id: None,
            },
            WireMessageType(String::from("AuthProof")),
        );
        envelope.observed.presence_generation_view = None;
        super::WireFrame {
            envelope,
            payload: WirePayload::AuthProof(AuthProof {
                proof: proof.to_string(),
            }),
        }
    }

    #[tokio::test]
    async fn challenge_proof_accepts_and_binds_the_connection() {
        let Some((handle, dir)) = open_handle("auth-flow").await else {
            return;
        };
        let transport = fake_transport();
        let pending = handle
            .handle_frame(pairing_frame("laptop"), live_input("client-a"), &transport)
            .await;
        assert!(
            pending.first().is_some_and(|first| matches!(
                &first.payload,
                WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation)
            )),
            "a fresh descriptor pends, got {pending:?}"
        );
        for response in &pending {
            assert_eq!(
                response.envelope.sender.connection_id, None,
                "a pre-accept pairing answer hides the connection id"
            );
        }
        let approved = handle.approve_device("laptop").await;
        assert!(
            matches!(approved, Ok(Some(_))),
            "owner approval must pair, got {approved:?}"
        );
        let Ok(Some((record, secret))) = approved else {
            remove_data_dir(&dir);
            return;
        };
        let device_wire = record.id.0.as_uuid().as_hyphenated().to_string();
        let paired = handle
            .handle_frame(pairing_frame("laptop"), live_input("client-a"), &transport)
            .await;
        let Some(answer) = paired.first() else {
            remove_data_dir(&dir);
            return;
        };
        let WirePayload::PairingResult(PairingResult::Paired { device_id }) = &answer.payload
        else {
            remove_data_dir(&dir);
            return;
        };
        let device_id = *device_id;
        assert_eq!(
            answer.envelope.sender.connection_id, None,
            "even the Paired answer hides the connection id"
        );
        assert_eq!(
            device_id.0.as_hyphenated().to_string(),
            device_wire,
            "the issued device key names the approved device"
        );
        let live = paired_input(&device_wire);
        let challenged = handle
            .handle_frame(
                advertise_frame(ProtocolVersion::V1),
                live.clone(),
                &transport,
            )
            .await;
        assert_eq!(
            challenged.len(),
            2,
            "capability answers terms plus challenge"
        );
        for response in &challenged {
            assert_eq!(
                response.envelope.sender.connection_id, None,
                "negotiation and challenge hide the connection id"
            );
        }
        let Some(challenge_frame) = challenged.get(1) else {
            remove_data_dir(&dir);
            return;
        };
        let WirePayload::AuthChallenge(challenge) = &challenge_frame.payload else {
            remove_data_dir(&dir);
            return;
        };
        let nonce = challenge.nonce.clone();
        assert!(!nonce.is_empty(), "the challenge carries a fresh nonce");
        let proof = pairing_proof_hex(&secret, &nonce);
        let attempt = proof_frame(device_id, &proof);
        let expected_reply = attempt.envelope.message_id;
        let answered = handle.handle_frame(attempt, live.clone(), &transport).await;
        assert_eq!(answered.len(), 2, "a proof answers result plus fact");
        let Some(accepted) = answered.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &accepted.payload,
                WirePayload::AuthResult(AuthResult::Accepted { connection_id })
                if *connection_id == live.connection_id
            ),
            "a valid proof is accepted with this connection id, got {:?}",
            accepted.payload
        );
        assert_eq!(
            accepted.envelope.correlation.reply_to,
            Some(expected_reply),
            "the result links back to the proof"
        );
        assert_eq!(
            accepted.envelope.sender.device_id,
            Some(device_id),
            "the result names the paired target"
        );
        assert_eq!(
            accepted.envelope.sender.incarnation_id,
            ClientIncarnationId {
                counter: 5,
                random: 6,
            },
            "the result echoes the inbound incarnation"
        );
        assert_eq!(
            accepted.envelope.sender.connection_id,
            Some(live.connection_id),
            "the result echoes the connection"
        );
        let Some(fact) = answered.get(1) else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(&fact.payload, WirePayload::PresenceAttribution(_)),
            "acceptance carries the attribution fact, got {:?}",
            fact.payload
        );
        assert_eq!(
            fact.envelope.sender.connection_id,
            Some(live.connection_id),
            "the piggybacked fact rides the acceptance, so it reveals too"
        );
        let replayed = handle
            .handle_frame(proof_frame(device_id, &proof), live.clone(), &transport)
            .await;
        assert!(
            replayed.first().is_some_and(|first| matches!(
                &first.payload,
                WirePayload::AuthResult(AuthResult::Rejected { .. })
            )),
            "the consumed nonce never answers twice, got {replayed:?}"
        );
        for response in &replayed {
            assert_eq!(
                response.envelope.sender.connection_id, None,
                "a rejection hides the connection id even post-accept"
            );
        }
        let rechallenged = handle
            .handle_frame(
                advertise_frame(ProtocolVersion::V1),
                live.clone(),
                &transport,
            )
            .await;
        let Some(fresh) = rechallenged.get(1) else {
            remove_data_dir(&dir);
            return;
        };
        let WirePayload::AuthChallenge(fresh_challenge) = &fresh.payload else {
            remove_data_dir(&dir);
            return;
        };
        assert_ne!(
            fresh_challenge.nonce, nonce,
            "re-advertising mints a fresh nonce"
        );
        let wrong = handle
            .handle_frame(proof_frame(device_id, "deadbeef"), live.clone(), &transport)
            .await;
        assert!(
            wrong.first().is_some_and(|first| matches!(
                &first.payload,
                WirePayload::AuthResult(AuthResult::Rejected { reason })
                if reason == "invalid proof"
            )),
            "a bad proof is rejected, got {wrong:?}"
        );
        let rechallenged = handle
            .handle_frame(
                advertise_frame(ProtocolVersion::V1),
                live.clone(),
                &transport,
            )
            .await;
        let Some(fresh) = rechallenged.get(1) else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(&fresh.payload, WirePayload::AuthChallenge(_)),
            "re-advertising challenges again, got {:?}",
            fresh.payload
        );
        let unknown_device = DeviceWireId(uuid::Uuid::new_v4());
        let unknown = handle
            .handle_frame(
                proof_frame(unknown_device, "deadbeef"),
                live.clone(),
                &transport,
            )
            .await;
        assert!(
            unknown.first().is_some_and(|first| matches!(
                &first.payload,
                WirePayload::AuthResult(AuthResult::Rejected { .. })
            )),
            "an unknown device is rejected, got {unknown:?}"
        );
        let bound = handle
            .handle_frame(stamped(submit_frame(), &live), live.clone(), &transport)
            .await;
        assert!(
            !bound
                .first()
                .is_some_and(|first| matches!(&first.payload, WirePayload::DisconnectNotice(_))),
            "a connection-bound frame passes the gate, got {bound:?}"
        );
        let mut stray = submit_frame();
        stray.envelope.sender.connection_id = Some(ConnectionWireId(uuid::Uuid::new_v4()));
        let dropped = handle.handle_frame(stray, live.clone(), &transport).await;
        assert!(
            dropped.first().is_some_and(|first| matches!(
                &first.payload,
                WirePayload::DisconnectNotice(notice) if notice.reason == "unpaired"
            )),
            "a foreign connection id closes with the unpaired notice, got {dropped:?}"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn pre_accept_denials_and_closes_hide_the_connection_id() {
        let Some((handle, dir)) = open_handle("auth-hidden").await else {
            return;
        };
        let transport = fake_transport();
        let live = live_input("client-a");
        let denied = handle
            .handle_frame(pairing_frame("laptop"), live.clone(), &transport)
            .await;
        // Fresh descriptor pends (no denial here); the peer-mismatch and
        // blank denials below are the hiding cases.
        assert!(
            denied.first().is_some_and(|first| matches!(
                &first.payload,
                WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation)
            )),
            "a fresh descriptor pends, got {denied:?}"
        );
        let mismatched = LiveInput {
            peer_uid_ok: false,
            ..live_input("client-a")
        };
        let refused = handle
            .handle_frame(pairing_frame("laptop"), mismatched, &transport)
            .await;
        assert!(
            refused.first().is_some_and(|first| matches!(
                &first.payload,
                WirePayload::PairingResult(PairingResult::Denied { .. })
            )),
            "an unauthorized peer is denied, got {refused:?}"
        );
        let mismatch = handle
            .handle_frame(
                advertise_frame(ProtocolVersion { major: 9, minor: 0 }),
                live.clone(),
                &transport,
            )
            .await;
        assert!(
            mismatch
                .first()
                .is_some_and(|first| matches!(&first.payload, WirePayload::DisconnectNotice(_))),
            "a major mismatch disconnects, got {mismatch:?}"
        );
        let dropped = handle
            .handle_frame(submit_frame(), unpaired_input(), &transport)
            .await;
        assert!(
            dropped.first().is_some_and(|first| matches!(
                &first.payload,
                WirePayload::DisconnectNotice(notice) if notice.reason == "unpaired"
            )),
            "an unpaired submit closes, got {dropped:?}"
        );
        for response in denied
            .iter()
            .chain(&refused)
            .chain(&mismatch)
            .chain(&dropped)
        {
            assert_eq!(
                response.envelope.sender.connection_id, None,
                "no pre-accept response may reveal the connection id, got {:?}",
                response.payload
            );
            assert!(
                response.envelope.correlation.reply_to.is_some(),
                "hiding never drops the reply link, got {:?}",
                response.payload
            );
        }
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn unauthed_domain_frame_closes_even_without_a_connection_id() {
        let Some((handle, dir)) = open_handle("gate-bypass").await else {
            return;
        };
        let transport = fake_transport();
        // A paired device that skipped the proof: the bypass attempt carries
        // no connection id because pre-accept responses never reveal it.
        let bypass = LiveInput {
            paired_device: Some(String::from("laptop")),
            connection_known: true,
            authed: false,
            ..live_input("client-a")
        };
        let without_id = submit_frame();
        assert_eq!(
            without_id.envelope.sender.connection_id, None,
            "the bypass frame carries no connection id"
        );
        let dropped = handle
            .handle_frame(without_id, bypass.clone(), &transport)
            .await;
        assert!(
            dropped.first().is_some_and(|first| matches!(
                &first.payload,
                WirePayload::DisconnectNotice(notice) if notice.reason == "unpaired"
            )),
            "an unauthed domain frame closes, got {dropped:?}"
        );
        for response in &dropped {
            assert_eq!(
                response.envelope.sender.connection_id, None,
                "the drop itself reveals nothing"
            );
        }
        // Even a fully authed connection must still echo the id: `None` is
        // never a valid binding.
        let authed = LiveInput {
            authed: true,
            ..bypass
        };
        let dropped = handle
            .handle_frame(submit_frame(), authed, &transport)
            .await;
        assert!(
            dropped.first().is_some_and(|first| matches!(
                &first.payload,
                WirePayload::DisconnectNotice(notice) if notice.reason == "unpaired"
            )),
            "an id-less frame on an authed connection still closes, got {dropped:?}"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn superseded_connection_replay_closes_despite_a_known_id() {
        let Some((handle, dir)) = open_handle("gate-superseded").await else {
            return;
        };
        let transport = fake_transport();
        // The connection table reports a superseded connection as unauthed
        // (see the conn-level supersede test): the envelope echoes the table
        // id exactly, yet the gate must still drop the frame because the
        // device authenticated anew elsewhere.
        let stale = LiveInput {
            paired_device: Some(String::from("laptop")),
            connection_known: true,
            authed: false,
            ..live_input("client-a")
        };
        let replay = stamped(submit_frame(), &stale);
        assert_eq!(
            replay.envelope.sender.connection_id,
            Some(stale.connection_id),
            "the replay echoes the table id exactly"
        );
        let dropped = handle.handle_frame(replay, stale, &transport).await;
        assert!(
            dropped.first().is_some_and(|first| matches!(
                &first.payload,
                WirePayload::DisconnectNotice(notice) if notice.reason == "unpaired"
            )),
            "a known-id replay on a superseded connection closes, got {dropped:?}"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn approved_secret_verifies_from_a_fresh_handle_on_the_same_dir() {
        use crate::test_support::temp_data_dir;
        use ene_credential::MemoryCredentialStore;

        use super::CredStore;

        let Some(dir) = temp_data_dir("auth-durable") else {
            return;
        };
        let transport = fake_transport();
        let opened =
            HostHandle::open_with_cred_store(&dir, CredStore::Memory(MemoryCredentialStore::new()))
                .await;
        assert!(opened.is_ok(), "the first open must succeed");
        let Ok(first) = opened else {
            remove_data_dir(&dir);
            return;
        };
        let pending = first
            .handle_frame(pairing_frame("laptop"), live_input("client-a"), &transport)
            .await;
        assert!(
            pending.first().is_some_and(|first| matches!(
                &first.payload,
                WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation)
            )),
            "a fresh descriptor pends, got {pending:?}"
        );
        let approved = first.approve_device("laptop").await;
        assert!(
            matches!(approved, Ok(Some(_))),
            "owner approval must pair, got {approved:?}"
        );
        let Ok(Some((record, secret))) = approved else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            dir.join("device-auth.json").exists(),
            "approval persists the secret to the device-auth file"
        );
        drop(first);
        // A fresh handle holds no secret map at all: if verification reads
        // only memory, this proof must fail. It must pass from the file.
        let reopened =
            HostHandle::open_with_cred_store(&dir, CredStore::Memory(MemoryCredentialStore::new()))
                .await;
        assert!(reopened.is_ok(), "the second open must succeed");
        let Ok(second) = reopened else {
            remove_data_dir(&dir);
            return;
        };
        let device_wire = record.id.0.as_uuid().as_hyphenated().to_string();
        let live = paired_input(&device_wire);
        let challenged = second
            .handle_frame(
                advertise_frame(ProtocolVersion::V1),
                live.clone(),
                &transport,
            )
            .await;
        let Some(challenge_frame) = challenged.get(1) else {
            remove_data_dir(&dir);
            return;
        };
        let WirePayload::AuthChallenge(challenge) = &challenge_frame.payload else {
            remove_data_dir(&dir);
            return;
        };
        let proof = pairing_proof_hex(&secret, &challenge.nonce);
        let Ok(device_uuid) = uuid::Uuid::parse_str(&device_wire) else {
            remove_data_dir(&dir);
            return;
        };
        let answered = second
            .handle_frame(
                proof_frame(DeviceWireId(device_uuid), &proof),
                live,
                &transport,
            )
            .await;
        assert!(
            answered.first().is_some_and(|first| matches!(
                &first.payload,
                WirePayload::AuthResult(AuthResult::Accepted { .. })
            )),
            "the file-backed secret verifies with no re-approval, got {answered:?}"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn proof_without_challenge_is_rejected() {
        let Some((handle, dir)) = open_handle("auth-nochallenge").await else {
            return;
        };
        let transport = fake_transport();
        let proof = super::WireFrame {
            envelope: new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(String::from("AuthProof")),
            ),
            payload: WirePayload::AuthProof(AuthProof {
                proof: String::from("proof-1"),
            }),
        };
        let live = live_input("client-a");
        let responses = handle.handle_frame(proof, live.clone(), &transport).await;
        assert_eq!(responses.len(), 1, "a proof answers exactly one frame");
        let Some(first) = responses.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &first.payload,
                WirePayload::AuthResult(AuthResult::Rejected { reason })
                if reason == "no pending challenge"
            ),
            "a proof with no pending challenge is rejected, got {:?}",
            first.payload
        );
        assert_eq!(
            first.envelope.sender.connection_id, None,
            "a pre-accept rejection hides the connection id"
        );
        assert_eq!(
            first.envelope.sender.incarnation_id,
            sender().incarnation_id,
            "hiding the connection never drops the incarnation echo"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn capability_mismatch_ends_with_a_disconnect_notice() {
        let Some((handle, dir)) = open_handle("caps-mismatch").await else {
            return;
        };
        let frame = advertise_frame(ProtocolVersion { major: 9, minor: 0 });
        let transport = fake_transport();
        let responses = handle
            .handle_frame(frame, live_input("client-a"), &transport)
            .await;
        assert_eq!(responses.len(), 1, "mismatch answers exactly one frame");
        let Some(first) = responses.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(&first.payload, WirePayload::DisconnectNotice(_)),
            "a major mismatch disconnects"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn capability_match_negotiates_and_challenges_without_attaching() {
        use ene_companion::CompanionRepository as _;

        let Some((handle, dir)) = open_handle("caps-ok").await else {
            return;
        };
        let live = live_input("client-a");
        let frame = advertise_frame(ProtocolVersion::V1);
        let expected_reply = frame.envelope.message_id;
        let transport = fake_transport();
        let responses = handle.handle_frame(frame, live.clone(), &transport).await;
        assert_eq!(
            responses.len(),
            2,
            "negotiation answers terms plus the auth challenge"
        );
        let Some(first) = responses.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(&first.payload, WirePayload::NegotiatedConnection(_)),
            "a major match negotiates, got {:?}",
            first.payload
        );
        assert_eq!(
            first.envelope.correlation.reply_to,
            Some(expected_reply),
            "the reply links back to the request"
        );
        let Some(second) = responses.get(1) else {
            remove_data_dir(&dir);
            return;
        };
        let WirePayload::AuthChallenge(challenge) = &second.payload else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            !challenge.nonce.is_empty(),
            "negotiation opens authentication with a fresh nonce"
        );
        for response in &responses {
            assert_eq!(
                response.envelope.sender.connection_id, None,
                "pre-accept negotiation hides the connection id"
            );
        }
        let recorded = match handle.pending_nonces.lock() {
            Ok(map) => map.get(&conn_key(&live.connection_id)).cloned(),
            Err(_) => None,
        };
        assert_eq!(
            recorded.as_ref(),
            Some(&challenge.nonce),
            "the challenge nonce is pending for this connection"
        );
        let companion = handle.store.ensure_running_companion().await;
        let Ok(companion) = companion else {
            remove_data_dir(&dir);
            return;
        };
        let attribution = handle.store.load_attribution(companion.as_raw()).await;
        let Ok(Some(current)) = attribution else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            current.active_client.is_none(),
            "capability negotiation never attaches presence"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn disconnect_without_presence_is_a_no_op() {
        use ene_companion::CompanionRepository as _;

        let Some((handle, dir)) = open_handle("disc-noop").await else {
            return;
        };
        handle.note_disconnect("never-attached").await;
        let companion = handle.store.ensure_running_companion().await;
        let Ok(companion) = companion else {
            remove_data_dir(&dir);
            return;
        };
        let attribution = handle.store.load_attribution(companion.as_raw()).await;
        let Ok(Some(current)) = attribution else {
            remove_data_dir(&dir);
            return;
        };
        assert_eq!(
            current.state,
            ene_presence::PresenceState::NoActive,
            "a disconnect with nothing attached changes nothing"
        );
        remove_data_dir(&dir);
    }
}
