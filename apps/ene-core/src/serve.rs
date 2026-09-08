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
//! - Pairing issues a transient [`DeviceWireId`]: there is no durable device
//!   table in `Stage 2` scope, so a Host restart invalidates issued device keys
//!   and the Client re-pairs. Re-pairing is always accepted for an authorized
//!   peer; pairing never fabricates authority beyond issuance.
//! - Presence attach is handshake-driven: the first [`CapabilityAdvertise`]
//!   that negotiates successfully moves a `NoActive` attribution to the calling
//!   client through compare-and-commit plus confirm. Later
//!   [`ene_api::v1::round::SubmitTextInput`]
//!   frames rely on the resulting generation, which the Client learns through
//!   the [`StaleRound`](ene_api::v1::round::RoundIntakeOutcomeWire::StaleRound)
//!   outcome when its view is behind.
//! - [`HostHandle::handle_frame`] is infallible by contract: infrastructure
//!   failures map to retry-safe outcome frames (hold or revalidate), never to
//!   fabricated domain facts. The mapping table lives on each pipeline method.
//! - Unknown or deferred inbound variants (auth frames, reconnect, stream
//!   frames from the Client, facts the Host itself emits) are ignored with an
//!   empty response. There is no `UnsupportedMessage` DTO in `ene-api`, and a
//!   [`DisconnectNotice`] would carry
//!   the wrong semantics for a merely unhandled message, so silence plus this
//!   gap note is the explicit `Stage 2` decision. `Stage 2` transport work may
//!   add a typed reject.
//! - A [`DisconnectNotice`] in a
//!   response vector is terminal: [`crate::conn`] writes it and then closes the
//!   connection. Only the major-version mismatch path emits one.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use ene_api::v1::envelope::{ProtocolVersion, WireEnvelope, WireSender, new_outgoing_envelope};
use ene_api::v1::handshake::{
    CapabilityAdvertise, DisconnectNotice, NegotiatedConnection, PairingRequest, PairingResult,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{ClientIncarnationId, DeviceWireId, WireMessageId, WireMessageType};
use ene_companion::CompanionRepository;
use ene_credential::{
    CredentialRef, CredentialStore, CredentialTechnicalError, EnvCredentialStore,
    MemoryCredentialStore,
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
use tokio::sync::Mutex;

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
/// The connection layer builds this per frame: `client_ref` names the calling
/// client opaquely (device key when paired, otherwise the incarnation pair),
/// `connection_live` carries the out-of-band reachability premise, and
/// `peer_uid_ok` carries the same-user proof for this connection. All fields
/// are public so connection adapters and integration tests can construct the
/// value directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveInput {
    /// Opaque client reference naming the caller for this frame.
    pub client_ref: String,
    /// Whether the underlying connection is currently live.
    pub connection_live: bool,
    /// Whether the peer passed the same-user check.
    pub peer_uid_ok: bool,
}

/// `Stage 2` Host handle: durable store, evaluation tracker, and Host maps.
///
/// All fields are crate-private: external callers (including integration
/// tests) drive the Host through [`HostHandle::open`] (or
/// [`HostHandle::open_with_cred_store`]) plus [`HostHandle::handle_frame`],
/// and the pipeline methods in [`crate::dialogue`] and [`crate::setup`] reach
/// the fields as inherent `impl HostHandle` blocks in this crate.
///
/// Map keys: `open_rounds` is keyed by `(client ref, companion key)`; round
/// refs issued on the wire resolve back through `rounds` (wire string to
/// domain round); `seen_local_ids` holds `(client ref, local id)` pairs as the
/// domain idempotency keys; `paired_devices` holds transiently issued device
/// keys by wire string; `clients` pins each wire client ref to one [`ClientId`].
/// Nothing here is durable except through [`Store`]: a restart drops every map
/// while the database persists, and old wire round refs then surface as stale
/// (never rebound).
pub struct HostHandle {
    pub(crate) store: Store,
    pub(crate) tracker: EvaluationTracker,
    pub(crate) open_rounds: HashMap<(String, String), OpenRound>,
    pub(crate) seen_local_ids: HashSet<(String, String)>,
    pub(crate) paired_devices: HashMap<String, RawId>,
    pub(crate) clients: HashMap<String, ClientId>,
    pub(crate) rounds: HashMap<String, RoundId>,
    pub(crate) cred_store: CredStore,
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
    /// environment on every call).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Store`] under the same conditions as
    /// [`HostHandle::open`].
    pub async fn open_with_cred_store(
        data_dir: &Path,
        cred_store: CredStore,
    ) -> Result<Self, CoreError> {
        ensure_data_dir(data_dir)?;
        let database = data_dir.join("app.db");
        let store = Store::open(&database)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        Ok(Self {
            store,
            tracker: EvaluationTracker::new(),
            open_rounds: HashMap::new(),
            seen_local_ids: HashSet::new(),
            paired_devices: HashMap::new(),
            clients: HashMap::new(),
            rounds: HashMap::new(),
            cred_store,
        })
    }

    /// Runs the full orchestration pipeline for one inbound frame.
    ///
    /// Transport-free by design: framing, sockets, and peer checks live in
    /// [`crate::conn`], while inference arrives as `transport` so tests pass a
    /// fake and production passes the `OpenAI` transport. The returned frames
    /// carry `outgoing_envelope` envelopes with `reply_to` set to the inbound
    /// message id.
    ///
    /// Dispatch: [`PairingRequest`] and [`CapabilityAdvertise`] run the
    /// handshake below; [`SubmitTextInput`](ene_api::v1::round::SubmitTextInput),
    /// [`ConfirmPresentation`](ene_api::v1::round::ConfirmPresentationWire), and
    /// [`HistoryRequest`](ene_api::v1::round::HistoryRequest) run in
    /// [`crate::dialogue`]; [`ManagementIntent`](ene_api::v1::management::ManagementIntent)
    /// and [`ManagementViewRequest`](ene_api::v1::management::ManagementViewRequest)
    /// run in [`crate::setup`]. Every other variant returns an empty vector
    /// (observation or deferred scope; see the module gap note).
    pub async fn handle_frame(
        &mut self,
        frame: WireFrame,
        live: LiveInput,
        transport: &impl ProviderTransport,
    ) -> Vec<WireFrame> {
        match &frame.payload {
            WirePayload::PairingRequest(request) => self.pair(&frame, request, &live),
            WirePayload::CapabilityAdvertise(advertise) => {
                self.advertise(&frame, advertise, &live).await
            }
            WirePayload::SubmitTextInput(submit) => {
                self.submit_text(&frame, submit, &live, transport).await
            }
            WirePayload::ConfirmPresentation(confirm) => {
                self.confirm_presentation(&frame, confirm).await
            }
            WirePayload::HistoryRequest(request) => self.answer_history(&frame, request).await,
            WirePayload::ManagementIntent(intent) => self.apply_intent(&frame, intent).await,
            WirePayload::ManagementViewRequest(request) => self.answer_view(&frame, request).await,
            _ => Vec::new(),
        }
    }

    /// Returns the [`ClientId`] pinned to a wire client ref, minting on first use.
    ///
    /// The pin is per-process: a restart mints fresh ids while presence
    /// attribution persists, so a restarted Host re-attaches through the
    /// handshake rather than inheriting the old mapping.
    pub(crate) fn client_for(&mut self, client_ref: &str) -> ClientId {
        if let Some(client) = self.clients.get(client_ref) {
            return *client;
        }
        let client = ClientId::generate();
        self.clients.insert(client_ref.to_string(), client);
        client
    }

    /// Resolves an issued wire round string back to its domain round.
    pub(crate) fn round_for(&self, wire: &str) -> Option<RoundId> {
        self.rounds.get(wire).copied()
    }

    /// Records a domain round under its wire string for later resolution.
    pub(crate) fn record_round(&mut self, wire: &str, round: RoundId) {
        self.rounds.insert(wire.to_string(), round);
    }

    /// Handles one [`PairingRequest`]: deny unauthorized peers, else issue.
    ///
    /// Denial carries an operational reason only. Issuance mints a fresh v4
    /// [`DeviceWireId`] recorded transiently in `paired_devices`; without a
    /// durable device table a restart invalidates it and the Client re-pairs,
    /// which is the documented `Stage 2` pairing shape. The request descriptor
    /// is display-only and intentionally unused: there is no owner surface in
    /// `Stage 2` to confirm against, and same-user socket trust stands in for
    /// it (see [`crate::conn`]).
    fn pair(
        &mut self,
        frame: &WireFrame,
        _request: &PairingRequest,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        if !live.peer_uid_ok {
            let denied = PairingResult::Denied {
                reason: String::from("peer user mismatch"),
            };
            return vec![outgoing_frame(
                frame,
                "PairingResult",
                WirePayload::PairingResult(denied),
            )];
        }
        let raw = RawId::new();
        let device_id = DeviceWireId(raw.as_uuid());
        self.paired_devices
            .insert(device_id.0.as_hyphenated().to_string(), raw);
        vec![outgoing_frame(
            frame,
            "PairingResult",
            WirePayload::PairingResult(PairingResult::Paired { device_id }),
        )]
    }

    /// Handles one [`CapabilityAdvertise`]: negotiate or disconnect, then attach.
    ///
    /// When no advertised version shares the v1 major, the reply is a single
    /// terminal [`DisconnectNotice`] (the connection closes after it is
    /// written; there is no `IncompatibleProtocol` DTO in `ene-api`). Otherwise
    /// presence attaches best-effort (see [`HostHandle::attach_presence`]) and
    /// the reply carries the negotiated terms: version v1 and every advertised
    /// feature kind as receipt, never as permission.
    async fn advertise(
        &mut self,
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
            return vec![outgoing_frame(
                frame,
                "DisconnectNotice",
                WirePayload::DisconnectNotice(notice),
            )];
        }
        self.attach_presence(live).await;
        let accepted = advertise
            .features
            .iter()
            .map(|feature| feature.kind)
            .collect();
        let negotiated = NegotiatedConnection {
            version: ProtocolVersion::V1,
            accepted_features: accepted,
        };
        vec![outgoing_frame(
            frame,
            "NegotiatedConnection",
            WirePayload::NegotiatedConnection(negotiated),
        )]
    }

    /// Attaches presence for the calling client when none is active.
    ///
    /// Best-effort by design: only a `NoActive` attribution moves (through
    /// compare-and-begin plus confirm with the [`LiveInput`] liveness premise
    /// and [`ThinMoveReason::InitialAttach`]); any other state, a lost compare
    /// race, or a store failure leaves attribution untouched, and the next
    /// intake surfaces the resulting staleness honestly instead. No reply is
    /// produced here; the caller ([`HostHandle::advertise`]) answers.
    async fn attach_presence(&mut self, live: &LiveInput) {
        let client = self.client_for(&live.client_ref);
        let Ok(companion) = self.store.ensure_running_companion().await else {
            return;
        };
        let Ok(Some(current)) = self.store.load_attribution(companion.as_raw()).await else {
            return;
        };
        if current.state != PresenceState::NoActive {
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
                Some(client),
                ThinMoveReason::InitialAttach,
            )
            .await
        else {
            return;
        };
        let premise = LiveReachabilityRef {
            client,
            connection_live: live.connection_live,
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
}

/// Builds the Host sender for outgoing envelopes.
///
/// Host-to-Client addressing carries no device or connection key in `Stage 2`
/// (authentication arrives later): only the reply correspondence links the
/// response to its request. The zero incarnation is a placeholder, never
/// authority.
fn host_sender() -> WireSender {
    WireSender {
        device_id: None,
        incarnation_id: ClientIncarnationId {
            counter: 0,
            random: 0,
        },
        connection_id: None,
    }
}

/// Builds an outgoing envelope for a `Stage 2` message type.
///
/// `reply_to` links the response to its request for transport pairing; domain
/// correspondence travels in the payloads, never here.
pub(crate) fn outgoing_envelope(
    message_type: &str,
    reply_to: Option<WireMessageId>,
) -> WireEnvelope {
    let mut envelope = new_outgoing_envelope(
        ProtocolVersion::V1,
        host_sender(),
        WireMessageType(message_type.to_string()),
    );
    envelope.correlation.reply_to = reply_to;
    envelope
}

/// Builds one response frame answering `frame` with `payload`.
///
/// The envelope follows the `Stage 2` `message_type` convention documented on
/// the crate root and links back through `reply_to`.
pub(crate) fn outgoing_frame(
    frame: &WireFrame,
    message_type: &str,
    payload: WirePayload,
) -> WireFrame {
    WireFrame {
        envelope: outgoing_envelope(message_type, Some(frame.envelope.message_id)),
        payload,
    }
}

/// Ensures the Host data directory exists.
#[cfg(unix)]
fn ensure_data_dir(data_dir: &Path) -> Result<(), CoreError> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(data_dir)
        .map_err(|error| CoreError::Store(format!("create data directory: {error}")))?;
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
        &crate::conn::socket_path(data_dir),
        Arc::new(Mutex::new(handle)),
        Arc::new(transport),
    )
    .await
}

use ene_plugin_ipc::WireFrame;

#[cfg(test)]
mod tests {
    use super::{HostHandle, LiveInput};
    use crate::test_support::{live_input, memory_handle, remove_data_dir};
    use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
    use ene_api::v1::handshake::{CapabilityAdvertise, PairingRequest, PairingResult};
    use ene_api::v1::payload::WirePayload;
    use ene_api::v1::refs::{ClientIncarnationId, WireMessageType};
    use ene_inference::fake::FakeProviderTransport;

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

    fn fake_transport() -> FakeProviderTransport {
        FakeProviderTransport::new(String::new(), None)
    }

    async fn open_handle(tag: &str) -> Option<(HostHandle, std::path::PathBuf)> {
        memory_handle(tag).await
    }

    #[tokio::test]
    async fn pairing_denies_an_unauthorized_peer() {
        let Some((mut handle, dir)) = open_handle("pair-deny").await else {
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
    async fn pairing_issues_a_device_key_for_an_authorized_peer() {
        let Some((mut handle, dir)) = open_handle("pair-ok").await else {
            return;
        };
        let frame = pairing_frame("laptop");
        let expected_reply = frame.envelope.message_id;
        let transport = fake_transport();
        let responses = handle
            .handle_frame(frame, live_input("client-a"), &transport)
            .await;
        assert_eq!(responses.len(), 1, "pairing answers exactly one frame");
        let Some(first) = responses.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &first.payload,
                WirePayload::PairingResult(PairingResult::Paired { .. })
            ),
            "an authorized peer is paired"
        );
        assert_eq!(
            first.envelope.correlation.reply_to,
            Some(expected_reply),
            "the reply links back to the request"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn capability_mismatch_ends_with_a_disconnect_notice() {
        let Some((mut handle, dir)) = open_handle("caps-mismatch").await else {
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
    async fn capability_match_negotiates_and_links_the_reply() {
        let Some((mut handle, dir)) = open_handle("caps-ok").await else {
            return;
        };
        let frame = advertise_frame(ProtocolVersion::V1);
        let expected_reply = frame.envelope.message_id;
        let transport = fake_transport();
        let responses = handle
            .handle_frame(frame, live_input("client-a"), &transport)
            .await;
        assert_eq!(responses.len(), 1, "negotiation answers exactly one frame");
        let Some(first) = responses.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(&first.payload, WirePayload::NegotiatedConnection(_)),
            "a major match negotiates"
        );
        assert_eq!(
            first.envelope.correlation.reply_to,
            Some(expected_reply),
            "the reply links back to the request"
        );
        remove_data_dir(&dir);
    }
}
