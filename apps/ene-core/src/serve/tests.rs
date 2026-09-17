use std::sync::Arc;

use super::{CredStore, CurrentConnection, HostHandle, LiveInput, device_client};
use crate::conn::{ConnectionPhase, ConnectionTable, LiveDecision};
use crate::test_support::{authenticate, live_input, memory_handle};
use ene_api::v1::deletion::{
    DeletionParticipantReportWire, DeletionPhaseWire, DeletionPurposeWire, DeletionStatusPage,
    DeletionStatusRequest, DeletionStatusResponse, deletion_target,
};
use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
use ene_api::v1::handshake::{
    AuthProof, AuthResult, CapabilityAdvertise, NegotiatedConnection, PairingRequest, PairingResult,
};
use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome, RationaleOrigin,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{
    BaseViewMark, ClientIncarnationId, ConnectionWireId, DeletionStatusCursorWire, DeviceWireId,
    ManagementTargetWire, WireMessageType,
};
use ene_api::v1::refs::{ClientLocalId, CommandWireId, CompanionWireRef, TextLangWire};
use ene_api::v1::reject::RejectKind;
use ene_api::v1::round::{HistoryRequest, PresentationStatus, SubmitTextInput, TextBodyWire};
use ene_api::v1::undelivered::{
    GetReportSource, GetTaskReport, ListTasks, PresentationReceiptWireRef, ReportSourceWireRef,
    ResumeTask, SelectTask, TaskWireRef, UndeliveredAck, UndeliveredRequest,
};
use ene_companion::{CompanionRepository as _, UndeliveredRepository as _};
use ene_credential::pairing_proof_hex;
use ene_inference::fake::FakeProviderTransport;
use ene_presence::{
    ClientId, ConfirmTransitionOutcome, LiveReachabilityRef, MoveDecision, PresenceCheckRef,
    PresenceGeneration, PresenceRepository, PresenceState, ThinMoveReason,
};

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

/// Stamps a frame with the connection binding its `live` premises carry, as a
/// connection-table-built frame would: direct-handle tests build envelopes by
/// hand.
fn stamped(mut frame: super::WireFrame, live: &LiveInput) -> super::WireFrame {
    frame.envelope.sender.connection_id = Some(live.connection_id);
    frame
}

/// A fresh table-backed connection in the accepted phase.
fn fresh_conn() -> (Arc<ConnectionTable>, ConnectionWireId) {
    let table = Arc::new(ConnectionTable::new());
    let id = table.note_accept();
    (table, id)
}

/// The current premises of a table-backed connection.
fn live_of(table: &Arc<ConnectionTable>, id: &ConnectionWireId) -> LiveInput {
    table.snapshot(id).expect("the connection must exist")
}

/// Drives a connection to authenticated-and-current for `device_wire`.
fn authenticate_conn(table: &Arc<ConnectionTable>, id: &ConnectionWireId, device_wire: &str) {
    authenticate(table, id, device_wire);
}

/// Runs one frame through the real connection-table premises, as the socket
/// loop does: `live_for` pins/checks the envelope, then the handle decides.
async fn dispatch(
    handle: &HostHandle,
    table: &Arc<ConnectionTable>,
    id: &ConnectionWireId,
    frame: super::WireFrame,
    transport: &FakeProviderTransport,
) -> Vec<super::WireFrame> {
    match table.live_for(id, &frame.envelope) {
        LiveDecision::Ready(live) => handle.handle_frame(frame, live, transport).await,
        LiveDecision::Duplicate | LiveDecision::Invalid => Vec::new(),
    }
}

fn negotiated_v1() -> NegotiatedConnection {
    NegotiatedConnection {
        version: ProtocolVersion::V1,
    }
}

/// Builds a paired [`LiveInput`] with an authenticated table-backed
/// connection for `device_wire`.
fn paired_input(device_wire: &str) -> LiveInput {
    live_input(device_wire)
}

fn pairing_frame(descriptor: &str) -> super::WireFrame {
    pairing_poll(descriptor, None)
}

/// A pairing poll for a previously issued pending id ([`None`] opens a new
/// request, [`Some`] re-asks after Owner approval).
fn pairing_poll(descriptor: &str, pending_id: Option<String>) -> super::WireFrame {
    super::WireFrame {
        envelope: new_outgoing_envelope(
            ProtocolVersion::V1,
            sender(),
            WireMessageType(String::from("PairingRequest")),
        ),
        payload: WirePayload::PairingRequest(PairingRequest {
            device_descriptor: descriptor.to_string(),
            pending_id,
        }),
    }
}

/// The opaque pending id of a
/// [`PendingOwnerConfirmation`](PairingResult::PendingOwnerConfirmation)
/// answer.
fn pending_id_of(frame: &super::WireFrame) -> String {
    let WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation { pending_id }) =
        &frame.payload
    else {
        panic!("the answer must pend, got {:?}", frame.payload);
    };
    pending_id.clone()
}

fn advertise_frame(device: Option<uuid::Uuid>, protocol: ProtocolVersion) -> super::WireFrame {
    let mut envelope = new_outgoing_envelope(
        ProtocolVersion::V1,
        sender(),
        WireMessageType(String::from("CapabilityAdvertise")),
    );
    envelope.sender.device_id = device.map(DeviceWireId);
    super::WireFrame {
        envelope,
        payload: WirePayload::CapabilityAdvertise(CapabilityAdvertise {
            supported_protocol: vec![protocol],
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
            fresh: false,
            local_id: ClientLocalId(String::from("local-1")),
            body: TextBodyWire {
                text: String::from("hello"),
                lang: TextLangWire(String::from("en")),
            },
        }),
    }
}

/// One submit frame claiming `device`, as a paired connection's frames do.
fn submit_for(device: DeviceWireId) -> super::WireFrame {
    let mut frame = submit_frame();
    frame.envelope.sender.device_id = Some(device);
    frame
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
            round: None,
        }),
    }
}

/// One first-party management intent frame with a caller-chosen idempotency
/// key, so replay tests can resend the same logical intent byte-for-byte.
fn management_intent_frame(
    live: &LiveInput,
    kind: ManagementIntentKind,
    target: &str,
    intent_id: CommandWireId,
) -> super::WireFrame {
    management_intent_frame_full(
        live,
        kind,
        target,
        "mark",
        RationaleOrigin::ManagementSurface,
        None,
        intent_id,
    )
}

/// The same frame with an explicit base-view mark, rationale, and id.
fn management_intent_frame_full(
    live: &LiveInput,
    kind: ManagementIntentKind,
    target: &str,
    base_view: &str,
    origin: RationaleOrigin,
    quote: Option<&str>,
    intent_id: CommandWireId,
) -> super::WireFrame {
    stamped(
        super::WireFrame {
            envelope: new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(String::from("ManagementIntent")),
            ),
            payload: WirePayload::ManagementIntent(ManagementIntent {
                intent_id,
                kind,
                target: ManagementTargetWire(target.to_string()),
                base_view: BaseViewMark(base_view.to_string()),
                rationale: IntentRationaleWire {
                    origin,
                    quote: quote.map(str::to_owned),
                },
            }),
        },
        live,
    )
}

/// One Targeted Deletion request frame: the typed wire grammar plus the mark
/// the Client read from the status page.
fn deletion_intent_frame(
    live: &LiveInput,
    exact_text: &str,
    base_view: &str,
    origin: RationaleOrigin,
    quote: Option<&str>,
    intent_id: CommandWireId,
) -> super::WireFrame {
    deletion_intent_frame_raw(
        live,
        &deletion_target(DeletionPurposeWire::Privacy, exact_text).0,
        base_view,
        origin,
        quote,
        intent_id,
    )
}

/// The same frame with a raw target string, for grammar/leak regressions.
fn deletion_intent_frame_raw(
    live: &LiveInput,
    target: &str,
    base_view: &str,
    origin: RationaleOrigin,
    quote: Option<&str>,
    intent_id: CommandWireId,
) -> super::WireFrame {
    management_intent_frame_full(
        live,
        ManagementIntentKind::RequestDeletionBackupRestoreReset,
        target,
        base_view,
        origin,
        quote,
        intent_id,
    )
}

fn deletion_status_frame(
    live: &LiveInput,
    cursor: Option<&str>,
    limit: Option<u32>,
) -> super::WireFrame {
    stamped(
        super::WireFrame {
            envelope: new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(String::from("DeletionStatusRequest")),
            ),
            payload: WirePayload::DeletionStatusRequest(DeletionStatusRequest {
                cursor: cursor.map(|cursor| DeletionStatusCursorWire(cursor.to_string())),
                limit,
            }),
        },
        live,
    )
}

/// The typed status page of one status answer; any other answer fails the
/// test with the payload it carried.
async fn read_deletion_page(
    handle: &HostHandle,
    live: &LiveInput,
    transport: &FakeProviderTransport,
    cursor: Option<&str>,
    limit: Option<u32>,
) -> DeletionStatusPage {
    let responses = handle
        .handle_frame(
            deletion_status_frame(live, cursor, limit),
            live.clone(),
            transport,
        )
        .await;
    let Some(WirePayload::DeletionStatusResponse(DeletionStatusResponse::Page(page))) =
        responses.first().map(|frame| &frame.payload)
    else {
        panic!("the status read must answer a page, got {responses:?}");
    };
    page.clone()
}

/// The domain outcome of one intent answer.
fn outcome_of(responses: &[super::WireFrame]) -> ManagementOutcome {
    match responses.first().map(|frame| &frame.payload) {
        Some(WirePayload::ManagementOutcome(outcome)) => outcome.clone(),
        other => panic!("the intent must answer an outcome, got {other:?}"),
    }
}

/// Premises for a connection that never paired (and so cannot be authed).
fn unpaired_input() -> LiveInput {
    let (table, id) = fresh_conn();
    LiveInput {
        client_ref: String::from("client-a"),
        connection_live: true,
        peer_uid_ok: true,
        paired_device: None,
        connection_known: false,
        authed: false,
        connection_id: id,
        negotiated: None,
        phase: ConnectionPhase::Accepted,
        authority: table,
    }
}

fn fake_transport() -> FakeProviderTransport {
    FakeProviderTransport::new(String::new(), None)
}

async fn open_handle(tag: &str) -> Option<(HostHandle, tempfile::TempDir)> {
    memory_handle(tag).await
}

/// Marks the device's deterministic client as the Present active client, so
/// close-admission fallbacks have something to move.
async fn make_present(handle: &HostHandle, device_wire: &str) {
    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must ensure");
    let current = handle
        .store
        .load_attribution(companion.as_raw())
        .await
        .expect("the attribution must load")
        .expect("the attribution must exist");
    let client = device_client(device_wire);
    let expected = PresenceCheckRef {
        expected_generation: current.generation,
        expected_state: current.state,
        expected_active: current.active_client,
    };
    let begun = handle
        .store
        .compare_and_begin_transition(
            companion.as_raw(),
            expected,
            Some(client),
            ThinMoveReason::InitialAttach,
        )
        .await;
    let MoveDecision::TransitioningToNew { generation } = begun.expect("begin must answer") else {
        panic!("a fresh attribution must admit the begin");
    };
    let confirmed = handle
        .store
        .confirm_transition(
            companion.as_raw(),
            generation,
            LiveReachabilityRef {
                client,
                connection_live: true,
            },
        )
        .await;
    assert!(
        matches!(confirmed, Ok(ConfirmTransitionOutcome::Confirmed(_))),
        "the live confirm must crown the client: {confirmed:?}"
    );
}

async fn presence_state(handle: &HostHandle) -> (PresenceState, Option<ClientId>, u64) {
    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must ensure");
    let attribution = handle
        .store
        .load_attribution(companion.as_raw())
        .await
        .expect("the attribution must load")
        .expect("the attribution must exist");
    (
        attribution.state,
        attribution.active_client,
        attribution.generation.as_u64(),
    )
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
    let (handle, _dir) = open_handle("pair-deny").await.unwrap();
    let denied_input = LiveInput {
        peer_uid_ok: false,
        ..unpaired_input()
    };
    let transport = fake_transport();
    let responses = handle
        .handle_frame(pairing_frame("laptop"), denied_input, &transport)
        .await;
    assert_eq!(responses.len(), 1, "denial answers exactly one frame");
    let first = responses.first().unwrap();
    assert!(
        matches!(
            &first.payload,
            WirePayload::PairingResult(PairingResult::Denied { .. })
        ),
        "an unauthorized peer is denied"
    );
}

#[tokio::test]
async fn pairing_denies_a_blank_descriptor() {
    let (handle, _dir) = open_handle("pair-blank").await.unwrap();
    let transport = fake_transport();
    for descriptor in ["", "   "] {
        let responses = handle
            .handle_frame(pairing_frame(descriptor), unpaired_input(), &transport)
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
    let entries = pending.unwrap();
    assert!(
        entries.is_empty(),
        "blank descriptors leave no pending entry"
    );
}

#[tokio::test]
async fn pairing_pends_then_pairs_after_owner_approval() {
    let (handle, _dir) = open_handle("pair-flow").await.unwrap();
    let transport = fake_transport();
    let (table, id) = fresh_conn();
    let live = live_of(&table, &id);
    let pending = handle
        .handle_frame(pairing_frame("laptop"), live.clone(), &transport)
        .await;
    assert_eq!(pending.len(), 1, "the request answers once");
    let first = pending.first().unwrap();
    let WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation { pending_id }) =
        &first.payload
    else {
        panic!("a fresh descriptor pends, never auto-approves");
    };
    assert!(
        !pending_id.is_empty(),
        "the pending answer carries its opaque approval id"
    );
    assert_eq!(
        table.phase_of(&id),
        Some(ConnectionPhase::Accepted),
        "a pending request never advances the phase"
    );
    let listed = handle.pending_devices().await;
    let entries = listed.unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.pending_id == *pending_id && entry.descriptor == "laptop"),
        "the pending id lists for the Owner with its display descriptor"
    );
    let unknown = handle.approve_device("unknown box").await.unwrap();
    assert!(unknown.is_none(), "an unknown id approves nothing");
    let approved = handle.approve_device(pending_id).await.unwrap();
    assert!(approved.is_some(), "owner approval must pair");
    let poll = pairing_poll("laptop", Some(pending_id.clone()));
    let expected_reply = poll.envelope.message_id;
    let paired = handle.handle_frame(poll, live.clone(), &transport).await;
    let answer = paired.first().unwrap();
    assert!(
        matches!(
            &answer.payload,
            WirePayload::PairingResult(PairingResult::Paired { .. })
        ),
        "an approved pending pairs on poll"
    );
    assert_eq!(
        answer.envelope.correlation.reply_to,
        Some(expected_reply),
        "the reply links back to the request"
    );
    assert_eq!(
        table.phase_of(&id),
        Some(ConnectionPhase::Paired),
        "the issued device key is recorded in the same phase operation"
    );
}

#[tokio::test]
async fn identical_descriptors_on_distinct_connections_get_distinct_pendings() {
    let (handle, _dir) = open_handle("pair-distinct").await.unwrap();
    let transport = fake_transport();
    // Two live connections sharing one handle (hence one pairing store), as
    // two sockets would after accept.
    let (first_table, first_id) = fresh_conn();
    let (second_table, second_id) = fresh_conn();
    let first = handle
        .handle_frame(
            pairing_frame("laptop"),
            live_of(&first_table, &first_id),
            &transport,
        )
        .await;
    let first_pending = pending_id_of(first.first().expect("the request answers once"));
    // The identical descriptor on the other connection opens its own pending
    // (#1389): the descriptor is display-only, never a shared identity.
    let second = handle
        .handle_frame(
            pairing_frame("laptop"),
            live_of(&second_table, &second_id),
            &transport,
        )
        .await;
    let second_pending = pending_id_of(second.first().expect("the request answers once"));
    assert_ne!(
        second_pending, first_pending,
        "same-descriptor requests never share a pending identity"
    );
    // A poll for the first pending from the second connection opens yet
    // another request: the mapping is kept only until the origin connection
    // ends, so a new connection cannot authenticate against it.
    let foreign = handle
        .handle_frame(
            pairing_poll("laptop", Some(first_pending.clone())),
            live_of(&second_table, &second_id),
            &transport,
        )
        .await;
    let foreign_pending = pending_id_of(foreign.first().expect("the poll answers once"));
    assert_ne!(
        foreign_pending, first_pending,
        "a new connection opens a new request instead of resuming the old pending"
    );
    // The origin connection still polls its own pending while it waits.
    let same = handle
        .handle_frame(
            pairing_poll("laptop", Some(first_pending.clone())),
            live_of(&first_table, &first_id),
            &transport,
        )
        .await;
    assert_eq!(
        pending_id_of(same.first().expect("the poll answers once")),
        first_pending,
        "the origin connection keeps its pending across polls"
    );
    // Owner approval names the pending id; each pending pairs its own device.
    let approved = handle.approve_device(&first_pending).await.unwrap();
    assert!(approved.is_some(), "owner approval must pair");
    let poll = handle
        .handle_frame(
            pairing_poll("laptop", Some(first_pending.clone())),
            live_of(&first_table, &first_id),
            &transport,
        )
        .await;
    assert!(
        poll.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::PairingResult(PairingResult::Paired { .. })
        )),
        "the approved pending pairs on poll"
    );
}

#[tokio::test]
async fn re_pairing_on_a_paired_connection_is_invalid_phase() {
    let (handle, _dir) = open_handle("pair-immutable").await.unwrap();
    let transport = fake_transport();
    let (table, id) = fresh_conn();
    assert!(table.note_paired(&id, "device-1"));
    let responses = handle
        .handle_frame(pairing_frame("other box"), live_of(&table, &id), &transport)
        .await;
    assert_eq!(responses.len(), 1, "the refusal answers exactly one frame");
    let first = responses.first().unwrap();
    assert!(
        matches!(
            &first.payload,
            WirePayload::Reject(notice) if notice.kind == RejectKind::InvalidHandshakePhase
        ),
        "a paired connection never re-pairs, got {:?}",
        first.payload
    );
    let pending = handle.pending_devices().await;
    let entries = pending.unwrap();
    assert!(
        entries.is_empty(),
        "the refused descriptor leaves no pending entry"
    );
    assert_eq!(
        table.phase_of(&id),
        Some(ConnectionPhase::Paired),
        "the refused request changes no phase"
    );
}

#[tokio::test]
async fn unpaired_domain_frames_close_with_an_unpaired_notice() {
    let (handle, _dir) = open_handle("gate-drop").await.unwrap();
    let transport = fake_transport();
    let responses = handle
        .handle_frame(submit_frame(), unpaired_input(), &transport)
        .await;
    assert_eq!(responses.len(), 1, "the gate answers once");
    let first = responses.first().unwrap();
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
    let view = history.first().unwrap();
    assert!(
        matches!(
            &view.payload,
            WirePayload::DisconnectNotice(notice) if notice.reason == "unpaired"
        ),
        "an unpaired history request closes with the unpaired notice"
    );
}

#[tokio::test]
async fn unknown_connection_closes_even_with_a_device() {
    let (handle, _dir) = open_handle("gate-unknown").await.unwrap();
    let transport = fake_transport();
    let input = LiveInput {
        paired_device: Some(String::from("laptop")),
        connection_known: false,
        ..live_input("client-a")
    };
    let responses = handle.handle_frame(submit_frame(), input, &transport).await;
    let first = responses.first().unwrap();
    assert!(
        matches!(
            &first.payload,
            WirePayload::DisconnectNotice(notice) if notice.reason == "unpaired"
        ),
        "an unknown connection closes even when it names a device"
    );
}

#[tokio::test]
async fn unsolicited_challenge_and_result_answer_nothing() {
    let (handle, _dir) = open_handle("auth-deferred").await.unwrap();
    let transport = fake_transport();
    let challenge = super::WireFrame {
        envelope: new_outgoing_envelope(
            ProtocolVersion::V1,
            sender(),
            WireMessageType(String::from("AuthChallenge")),
        ),
        payload: WirePayload::AuthChallenge(ene_api::v1::handshake::AuthChallenge {
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
}

fn proof_frame(device_id: DeviceWireId, proof: &str) -> super::WireFrame {
    // The incarnation matches `sender()`: one connection pins its first
    // incarnation, so every frame on it must echo the same process identity.
    let mut envelope = new_outgoing_envelope(
        ProtocolVersion::V1,
        WireSender {
            device_id: Some(device_id),
            incarnation_id: sender().incarnation_id,
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
    let (handle, _dir) = open_handle("auth-flow").await.unwrap();
    let transport = fake_transport();
    let (table, id) = fresh_conn();
    let pending = dispatch(&handle, &table, &id, pairing_frame("laptop"), &transport).await;
    let pending_id = pending_id_of(pending.first().expect("the request answers once"));
    for response in &pending {
        assert_eq!(
            response.envelope.sender.connection_id, None,
            "a pre-accept pairing answer hides the connection id"
        );
    }
    let approved = handle.approve_device(&pending_id).await;
    assert!(
        matches!(approved, Ok(Some(_))),
        "owner approval must pair, got {approved:?}"
    );
    let (record, secret) = approved.unwrap().unwrap();
    let device_wire = record.wire.clone();
    let device_uuid = uuid::Uuid::parse_str(&device_wire).unwrap();
    let paired = dispatch(
        &handle,
        &table,
        &id,
        pairing_poll("laptop", Some(pending_id)),
        &transport,
    )
    .await;
    let answer = paired.first().unwrap();
    let WirePayload::PairingResult(PairingResult::Paired { device_id }) = &answer.payload else {
        panic!("the approved pending must pair, got {paired:?}");
    };
    let device_id = *device_id;
    assert_eq!(
        answer.envelope.sender.connection_id, None,
        "even the Paired answer hides the connection id"
    );
    assert_eq!(
        device_id.0.as_hyphenated().to_string(),
        device_wire,
        "the issued device key is the stored opaque projection"
    );
    assert_ne!(
        device_wire,
        record.id.0.as_uuid().as_hyphenated().to_string(),
        "the issued key never renders the domain identity"
    );
    assert_eq!(
        table.phase_of(&id),
        Some(ConnectionPhase::Paired),
        "the table records the issued key"
    );
    let challenged = dispatch(
        &handle,
        &table,
        &id,
        advertise_frame(Some(device_uuid), ProtocolVersion::V1),
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
    let challenge_frame = challenged.get(1).unwrap();
    let WirePayload::AuthChallenge(challenge) = &challenge_frame.payload else {
        panic!("the second answer must be the challenge");
    };
    let nonce = challenge.nonce.clone();
    assert!(!nonce.is_empty(), "the challenge carries a fresh nonce");
    assert_eq!(
        table.challenge_nonce_of(&id).as_ref(),
        Some(&nonce),
        "the nonce is pending in the connection's challenged phase"
    );
    let proof = pairing_proof_hex(&secret, &nonce);
    let attempt = proof_frame(device_id, &proof);
    let expected_reply = attempt.envelope.message_id;
    let answered = dispatch(&handle, &table, &id, attempt, &transport).await;
    assert_eq!(answered.len(), 2, "a proof answers result plus fact");
    let accepted = answered.first().unwrap();
    assert!(
        matches!(
            &accepted.payload,
            WirePayload::AuthResult(AuthResult::Accepted { connection_id })
            if *connection_id == id
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
        sender().incarnation_id,
        "the result echoes the inbound incarnation"
    );
    assert_eq!(
        accepted.envelope.sender.connection_id,
        Some(id),
        "the result echoes the connection"
    );
    assert_eq!(
        table.phase_of(&id),
        Some(ConnectionPhase::Authenticated),
        "the install happened before the acceptance was sent"
    );
    assert!(table.current_authenticated(&device_wire));
    let fact = answered.get(1).unwrap();
    assert!(
        matches!(&fact.payload, WirePayload::PresenceAttribution(_)),
        "acceptance carries the attribution fact, got {:?}",
        fact.payload
    );
    assert_eq!(
        fact.envelope.sender.connection_id,
        Some(id),
        "the piggybacked fact rides the acceptance, so it reveals too"
    );

    // A retried proof after the install cannot race again: the phase left
    // `Challenged`, so the frame is refused without touching currentness.
    let replayed = dispatch(
        &handle,
        &table,
        &id,
        proof_frame(device_id, &proof),
        &transport,
    )
    .await;
    assert!(
        replayed.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::Reject(notice) if notice.kind == RejectKind::InvalidHandshakePhase
        )),
        "a retried proof outside the challenged phase is refused, got {replayed:?}"
    );
    assert_eq!(
        table.phase_of(&id),
        Some(ConnectionPhase::Authenticated),
        "the refused retry changes no phase"
    );
    assert!(table.current_authenticated(&device_wire));

    // Re-advertising after the install is refused with the nonce untouched.
    let rechallenged = dispatch(
        &handle,
        &table,
        &id,
        advertise_frame(Some(device_uuid), ProtocolVersion::V1),
        &transport,
    )
    .await;
    assert!(
        rechallenged.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::Reject(notice) if notice.kind == RejectKind::InvalidHandshakePhase
        )),
        "re-advertising after authentication is refused, got {rechallenged:?}"
    );
    assert_eq!(table.challenge_nonce_of(&id), None);

    // A fresh connection for the same device can challenge again, and a bad
    // proof there consumes the nonce, closes the phase, and leaves the
    // current authenticated connection untouched.
    let (second_table, second) = fresh_conn();
    let second_challenged = dispatch(
        &handle,
        &second_table,
        &second,
        advertise_frame(Some(device_uuid), ProtocolVersion::V1),
        &transport,
    )
    .await;
    let Some(WirePayload::AuthChallenge(second_challenge)) =
        second_challenged.get(1).map(|f| &f.payload)
    else {
        panic!("the reconnect must challenge, got {second_challenged:?}");
    };
    let wrong = dispatch(
        &handle,
        &second_table,
        &second,
        proof_frame(device_id, "deadbeef"),
        &transport,
    )
    .await;
    assert!(
        wrong.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::AuthResult(AuthResult::Rejected { reason })
            if reason == "invalid proof"
        )),
        "a bad proof is rejected, got {wrong:?}"
    );
    assert_eq!(
        second_table.challenge_nonce_of(&second),
        None,
        "the failed proof consumed its nonce"
    );
    assert_eq!(
        second_table.phase_of(&second),
        Some(ConnectionPhase::Closed),
        "a failed authentication closes the connection phase"
    );
    assert!(
        table.current_authenticated(&device_wire),
        "the failed challenge never touches the existing current"
    );
    assert_eq!(
        second_table.challenge_nonce_of(&second),
        None,
        "a closed connection never mints another nonce"
    );
    assert_eq!(
        second_challenged.get(1).map(|f| &f.payload),
        Some(&WirePayload::AuthChallenge(second_challenge.clone())),
        "the challenge nonce was single-use"
    );

    // The authenticated connection passes the domain gate; a foreign
    // connection id closes with the unpaired notice.
    let live = live_of(&table, &id);
    let bound = dispatch(
        &handle,
        &table,
        &id,
        stamped(submit_for(device_id), &live),
        &transport,
    )
    .await;
    assert!(
        !bound.is_empty()
            && !bound
                .first()
                .is_some_and(|first| matches!(&first.payload, WirePayload::DisconnectNotice(_))),
        "a connection-bound frame passes the gate, got {bound:?}"
    );
    let mut stray = submit_for(device_id);
    stray.envelope.sender.connection_id = Some(ConnectionWireId(uuid::Uuid::new_v4()));
    let dropped = dispatch(&handle, &table, &id, stray, &transport).await;
    assert!(
        dropped.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::DisconnectNotice(notice) if notice.reason == "unpaired"
        )),
        "a foreign connection id closes with the unpaired notice, got {dropped:?}"
    );
}

#[tokio::test]
async fn pre_accept_denials_and_closes_hide_the_connection_id() {
    let (handle, _dir) = open_handle("auth-hidden").await.unwrap();
    let transport = fake_transport();
    let denied = handle
        .handle_frame(pairing_frame("laptop"), unpaired_input(), &transport)
        .await;
    // The fresh descriptor pends rather than denying; the peer-mismatch
    // denial and the two closes below are the hiding cases.
    assert!(
        denied.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation { .. })
        )),
        "a fresh descriptor pends, got {denied:?}"
    );
    let mismatched = LiveInput {
        peer_uid_ok: false,
        ..unpaired_input()
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
    let (table, id) = fresh_conn();
    let mismatch = dispatch(
        &handle,
        &table,
        &id,
        advertise_frame(None, ProtocolVersion { major: 9, minor: 0 }),
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
}

#[tokio::test]
async fn unauthed_domain_frame_closes_even_without_a_connection_id() {
    let (handle, _dir) = open_handle("gate-bypass").await.unwrap();
    let transport = fake_transport();
    // A paired device that skipped the proof: the bypass attempt carries
    // no connection id because pre-accept responses never reveal it.
    let (table, id) = fresh_conn();
    assert!(table.note_paired(&id, "laptop"));
    let bypass = live_of(&table, &id);
    assert!(!bypass.authed, "pairing alone is not authentication");
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
}

#[tokio::test]
async fn superseded_connection_replay_is_stale_not_a_close() {
    let (handle, _dir) = open_handle("gate-superseded").await.unwrap();
    let transport = fake_transport();
    // Two table-backed connections for one device: the second install
    // supersedes the first, which still holds a live socket.
    let table = Arc::new(ConnectionTable::new());
    let first = table.note_accept();
    authenticate_conn(&table, &first, "laptop");
    let second = table.note_accept();
    authenticate_conn(&table, &second, "laptop");
    let stale = live_of(&table, &first);
    assert_eq!(stale.phase, ConnectionPhase::Superseded);
    assert!(!stale.authed, "a superseded connection is never authed");
    let replay = stamped(submit_frame(), &stale);
    assert_eq!(
        replay.envelope.sender.connection_id,
        Some(stale.connection_id),
        "the replay echoes the table id exactly"
    );
    let answers = handle.handle_frame(replay, stale, &transport).await;
    assert!(
        answers.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::Reject(notice) if notice.kind == RejectKind::StaleConnection
        )),
        "a known-id replay on a superseded connection is a typed stale rejection, got {answers:?}"
    );
    assert!(
        !answers
            .iter()
            .any(|answer| matches!(answer.payload, WirePayload::DisconnectNotice(_))),
        "a stale rejection keeps the socket open (IPC §11.3)"
    );
    assert!(table.current_authenticated("laptop"));
}

#[tokio::test]
async fn approved_secret_verifies_from_a_fresh_handle_on_the_same_dir() {
    use ene_credential::MemoryCredentialStore;

    use super::CredStore;

    let dir = tempfile::tempdir().expect("test scratch directory must be creatable");
    let transport = fake_transport();
    let opened = HostHandle::open_with_cred_store(
        dir.path(),
        CredStore::Memory(MemoryCredentialStore::new()),
    )
    .await;
    let first = opened.unwrap();
    let (first_table, first_id) = fresh_conn();
    let pending = dispatch(
        &first,
        &first_table,
        &first_id,
        pairing_frame("laptop"),
        &transport,
    )
    .await;
    let pending_id = pending_id_of(pending.first().expect("the request answers once"));
    let approved = first.approve_device(&pending_id).await;
    assert!(
        matches!(approved, Ok(Some(_))),
        "owner approval must pair, got {approved:?}"
    );
    let (record, secret) = approved.unwrap().unwrap();
    assert!(
        dir.path().join("device-auth.json").exists(),
        "approval persists the secret to the device-auth file"
    );
    drop(first);
    // A fresh handle holds no secret map at all: if verification reads
    // only memory, this proof must fail. It must pass from the file.
    let reopened = HostHandle::open_with_cred_store(
        dir.path(),
        CredStore::Memory(MemoryCredentialStore::new()),
    )
    .await;
    let second = reopened.unwrap();
    let device_wire = record.wire.clone();
    let device_uuid = uuid::Uuid::parse_str(&device_wire).unwrap();
    let (table, id) = fresh_conn();
    let challenged = dispatch(
        &second,
        &table,
        &id,
        advertise_frame(Some(device_uuid), ProtocolVersion::V1),
        &transport,
    )
    .await;
    let challenge_frame = challenged.get(1).unwrap();
    let WirePayload::AuthChallenge(challenge) = &challenge_frame.payload else {
        panic!("the reconnect must challenge, got {challenged:?}");
    };
    let proof = pairing_proof_hex(&secret, &challenge.nonce);
    let answered = dispatch(
        &second,
        &table,
        &id,
        proof_frame(DeviceWireId(device_uuid), &proof),
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
}

/// A proof without a challenge is out of phase: typed rejection, never a
/// fabricated rejection reason or an acceptance.
#[tokio::test]
async fn proof_without_challenge_is_invalid_phase() {
    let (handle, _dir) = open_handle("auth-nochallenge").await.unwrap();
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
    let (table, id) = fresh_conn();
    let responses = dispatch(&handle, &table, &id, proof, &transport).await;
    assert_eq!(responses.len(), 1, "the refusal answers exactly one frame");
    let first = responses.first().unwrap();
    assert!(
        matches!(
            &first.payload,
            WirePayload::Reject(notice) if notice.kind == RejectKind::InvalidHandshakePhase
        ),
        "a proof with no pending challenge is out of phase, got {:?}",
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
    assert_eq!(
        table.phase_of(&id),
        Some(ConnectionPhase::Accepted),
        "the refused proof changes no phase"
    );
}

/// Re-advertising after the challenge is refused and changes neither the
/// nonce nor the negotiated terms (#1385).
#[tokio::test]
async fn re_advertising_keeps_the_nonce_and_is_invalid_phase() {
    let (handle, _dir) = open_handle("caps-repeat").await.unwrap();
    let transport = fake_transport();
    let device = DeviceWireId(uuid::Uuid::from_u128(11));
    let device_wire = device.0.as_hyphenated().to_string();
    let (table, id) = fresh_conn();
    assert!(table.note_paired(&id, &device_wire));
    let first = dispatch(
        &handle,
        &table,
        &id,
        advertise_frame(Some(device.0), ProtocolVersion::V1),
        &transport,
    )
    .await;
    let Some(WirePayload::AuthChallenge(challenge)) = first.get(1).map(|f| &f.payload) else {
        panic!("the first advertise must challenge, got {first:?}");
    };
    let nonce = challenge.nonce.clone();
    let terms = table.negotiated_of(&id).expect("the terms must record");
    let repeat = dispatch(
        &handle,
        &table,
        &id,
        advertise_frame(Some(device.0), ProtocolVersion::V1),
        &transport,
    )
    .await;
    assert!(
        repeat.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::Reject(notice) if notice.kind == RejectKind::InvalidHandshakePhase
        )),
        "a repeat capability frame is refused, got {repeat:?}"
    );
    assert_eq!(
        table.challenge_nonce_of(&id).as_ref(),
        Some(&nonce),
        "the refusal never mints a new nonce"
    );
    assert_eq!(
        table.negotiated_of(&id),
        Some(terms),
        "the refusal never changes the negotiated terms"
    );
    assert_eq!(table.phase_of(&id), Some(ConnectionPhase::Challenged));
}

#[tokio::test]
async fn negotiated_version_is_fixed_per_connection() {
    let (handle, _dir) = open_handle("version-fixed").await.unwrap();
    let transport = fake_transport();
    let mut negotiated = live_input("device-1");
    negotiated.negotiated = Some(negotiated_v1());
    let mut mixed = submit_frame();
    mixed.envelope.protocol = ProtocolVersion {
        major: 1,
        minor: 99,
    };
    let rejected = handle
        .handle_frame(mixed, negotiated.clone(), &transport)
        .await;
    assert!(
        rejected.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::Reject(notice)
                if notice.kind == ene_api::v1::reject::RejectKind::IncompatibleProtocol
        )),
        "a v1.99 frame on a v1.0 connection must reject, got {rejected:?}"
    );
    assert_eq!(
        rejected
            .first()
            .map(|first| first.envelope.sender.connection_id),
        Some(Some(negotiated.connection_id)),
        "a post-auth protocol reject carries the current connection"
    );
    let agreed = handle
        .handle_frame(submit_frame(), negotiated, &transport)
        .await;
    assert!(
        agreed
            .first()
            .is_none_or(|first| !matches!(&first.payload, WirePayload::Reject(_))),
        "the negotiated version itself must pass the gate, got {agreed:?}"
    );
    let mut early = pairing_frame("laptop");
    early.envelope.protocol = ProtocolVersion { major: 1, minor: 1 };
    let pre = handle
        .handle_frame(early, unpaired_input(), &transport)
        .await;
    assert!(
        pre.first()
            .is_some_and(|first| matches!(&first.payload, WirePayload::Reject(_))),
        "pre-negotiation frames must speak exactly v1.0, got {pre:?}"
    );
    assert_eq!(
        pre.first().map(|first| first.envelope.sender.connection_id),
        Some(None),
        "a pre-auth protocol reject still hides the connection"
    );
}

/// The typed reject sender follows the auth boundary (IPC §5).
#[tokio::test]
async fn reject_sender_follows_the_auth_boundary() {
    let (handle, _dir) = open_handle("reject-sender").await.unwrap();
    let transport = fake_transport();
    let mut mismatch = submit_frame();
    mismatch.envelope.message_type = WireMessageType(String::from("HistoryRequest"));

    let live = paired_input("device-1");
    let post = handle
        .handle_frame(mismatch.clone(), live.clone(), &transport)
        .await;
    let first = post.first().unwrap();
    assert!(
        matches!(
            &first.payload,
            WirePayload::Reject(notice)
                if notice.kind == ene_api::v1::reject::RejectKind::UnsupportedMessage
        ),
        "a discriminator mismatch must reject, got {post:?}"
    );
    assert_eq!(
        first.envelope.sender.connection_id,
        Some(live.connection_id),
        "a post-auth message reject carries the current connection"
    );

    let pre = handle
        .handle_frame(mismatch, unpaired_input(), &transport)
        .await;
    let first = pre.first().unwrap();
    assert!(
        matches!(&first.payload, WirePayload::Reject(_)),
        "the same mismatch must reject pre-auth, got {pre:?}"
    );
    assert_eq!(
        first.envelope.sender.connection_id, None,
        "a pre-auth message reject still hides the connection"
    );
}

#[tokio::test]
async fn capability_mismatch_ends_with_a_disconnect_notice() {
    let (handle, _dir) = open_handle("caps-mismatch").await.unwrap();
    let (table, id) = fresh_conn();
    let frame = advertise_frame(None, ProtocolVersion { major: 9, minor: 0 });
    let transport = fake_transport();
    let responses = dispatch(&handle, &table, &id, frame, &transport).await;
    assert_eq!(responses.len(), 1, "mismatch answers exactly one frame");
    let first = responses.first().unwrap();
    assert!(
        matches!(&first.payload, WirePayload::DisconnectNotice(_)),
        "a major mismatch disconnects"
    );
    assert_eq!(
        table.phase_of(&id),
        Some(ConnectionPhase::Accepted),
        "a refused negotiation changes no phase"
    );
}

#[tokio::test]
async fn capability_match_negotiates_and_challenges_without_attaching() {
    use ene_companion::CompanionRepository as _;

    let (handle, _dir) = open_handle("caps-ok").await.unwrap();
    let device = DeviceWireId(uuid::Uuid::from_u128(13));
    let device_wire = device.0.as_hyphenated().to_string();
    let (table, id) = fresh_conn();
    assert!(table.note_paired(&id, &device_wire));
    let frame = advertise_frame(Some(device.0), ProtocolVersion::V1);
    let expected_reply = frame.envelope.message_id;
    let transport = fake_transport();
    let responses = dispatch(&handle, &table, &id, frame, &transport).await;
    assert_eq!(
        responses.len(),
        2,
        "negotiation answers terms plus the auth challenge"
    );
    let first = responses.first().unwrap();
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
    let second = responses.get(1).unwrap();
    let WirePayload::AuthChallenge(challenge) = &second.payload else {
        panic!("the second answer must be the challenge");
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
    assert_eq!(
        table.challenge_nonce_of(&id).as_ref(),
        Some(&challenge.nonce),
        "the challenge nonce is pending in the connection's phase"
    );
    let companion = handle.store.ensure_running_companion().await;
    let companion = companion.unwrap();
    let attribution = handle.store.load_attribution(companion.as_raw()).await;
    let current = attribution.unwrap().unwrap();
    assert!(
        current.active_client.is_none(),
        "capability negotiation never attaches presence"
    );
}

#[tokio::test]
async fn disconnect_without_presence_is_a_no_op() {
    use ene_companion::CompanionRepository as _;

    let (handle, _dir) = open_handle("disc-noop").await.unwrap();
    let live = live_input("never-attached");
    handle
        .close_connection(&live.authority, live.connection_id)
        .await;
    let companion = handle.store.ensure_running_companion().await;
    let companion = companion.unwrap();
    let attribution = handle.store.load_attribution(companion.as_raw()).await;
    let current = attribution.unwrap().unwrap();
    assert_eq!(
        current.state,
        ene_presence::PresenceState::NoActive,
        "a disconnect with nothing attached changes nothing"
    );
}

/// #1384 / S5-04: a superseded connection's capability, proof, and domain
/// frames are all refused as stale, with no new nonce, no current change, and
/// no domain effect.
#[tokio::test]
async fn superseded_connection_replays_are_stale_and_have_no_effect() {
    let (handle, _dir) = open_handle("stale-replay").await.unwrap();
    let transport = fake_transport();
    // Complete one real pairing to obtain an approved device and secret.
    let (probe_table, probe_id) = fresh_conn();
    let pending = dispatch(
        &handle,
        &probe_table,
        &probe_id,
        pairing_frame("laptop"),
        &transport,
    )
    .await;
    let probe_pending = pending_id_of(pending.first().expect("the probe must pend"));
    let approved = handle.approve_device(&probe_pending).await;
    let Ok(Some((record, secret))) = approved else {
        panic!("owner approval must pair, got {approved:?}");
    };
    let device_wire = record.wire.clone();
    let device = DeviceWireId(uuid::Uuid::parse_str(&device_wire).unwrap());
    let paired = dispatch(
        &handle,
        &probe_table,
        &probe_id,
        pairing_poll("laptop", Some(probe_pending)),
        &transport,
    )
    .await;
    assert!(
        paired.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::PairingResult(PairingResult::Paired { .. })
        )),
        "the approved pending must pair on poll, got {paired:?}"
    );

    // C1 authenticates.
    let (table, c1) = fresh_conn();
    let challenged = dispatch(
        &handle,
        &table,
        &c1,
        advertise_frame(Some(device.0), ProtocolVersion::V1),
        &transport,
    )
    .await;
    let Some(WirePayload::AuthChallenge(challenge)) = challenged.get(1).map(|f| &f.payload) else {
        panic!("C1 must challenge, got {challenged:?}");
    };
    let accepted = dispatch(
        &handle,
        &table,
        &c1,
        proof_frame(device, &pairing_proof_hex(&secret, &challenge.nonce)),
        &transport,
    )
    .await;
    assert!(
        accepted.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::AuthResult(AuthResult::Accepted { .. })
        )),
        "C1 must authenticate, got {accepted:?}"
    );

    // C2 authenticates on a fresh connection: C1 is superseded.
    let c2 = table.note_accept();
    let challenged = dispatch(
        &handle,
        &table,
        &c2,
        advertise_frame(Some(device.0), ProtocolVersion::V1),
        &transport,
    )
    .await;
    let Some(WirePayload::AuthChallenge(challenge)) = challenged.get(1).map(|f| &f.payload) else {
        panic!("C2 must challenge, got {challenged:?}");
    };
    let accepted = dispatch(
        &handle,
        &table,
        &c2,
        proof_frame(device, &pairing_proof_hex(&secret, &challenge.nonce)),
        &transport,
    )
    .await;
    assert!(
        accepted.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::AuthResult(AuthResult::Accepted { .. })
        )),
        "C2 must authenticate, got {accepted:?}"
    );
    assert_eq!(table.phase_of(&c1), Some(ConnectionPhase::Superseded));
    assert!(table.current_authenticated(&device_wire));

    // C1 replays capability, proof, and a domain frame: all stale, none may
    // mint a nonce, change the terms, touch the current, or create a round.
    let stale_capability = dispatch(
        &handle,
        &table,
        &c1,
        advertise_frame(Some(device.0), ProtocolVersion::V1),
        &transport,
    )
    .await;
    assert!(
        stale_capability.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::Reject(notice) if notice.kind == RejectKind::StaleConnection
        )),
        "C1 capability replay must be stale, got {stale_capability:?}"
    );
    let stale_proof = dispatch(
        &handle,
        &table,
        &c1,
        proof_frame(device, "deadbeef"),
        &transport,
    )
    .await;
    assert!(
        stale_proof.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::Reject(notice) if notice.kind == RejectKind::StaleConnection
        )),
        "C1 proof replay must be stale, got {stale_proof:?}"
    );
    let c1_live = live_of(&table, &c1);
    let stale_input = dispatch(
        &handle,
        &table,
        &c1,
        stamped(submit_for(device), &c1_live),
        &transport,
    )
    .await;
    assert!(
        stale_input.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::Reject(notice) if notice.kind == RejectKind::StaleConnection
        )),
        "C1 domain replay must be stale, got {stale_input:?}"
    );
    for response in [&stale_capability, &stale_proof, &stale_input] {
        assert!(
            !response
                .iter()
                .any(|answer| matches!(answer.payload, WirePayload::DisconnectNotice(_))),
            "stale rejections keep the socket open (IPC §11.3)"
        );
    }
    assert_eq!(
        table.challenge_nonce_of(&c1),
        None,
        "a superseded connection never mints another nonce"
    );
    assert_eq!(
        table.phase_of(&c1),
        Some(ConnectionPhase::Superseded),
        "C1 stays superseded"
    );
    assert!(
        table.current_authenticated(&device_wire),
        "C1 never reclaims the current slot"
    );
    assert!(
        !handle.has_open_round_for_test(),
        "a stale input creates no conversation round"
    );
    let (state, active, _) = presence_state(&handle).await;
    assert_eq!(state, PresenceState::NoActive);
    assert_eq!(active, None, "a stale input never attaches presence");
}

/// #1385: capability replay on a superseded connection is stale, and the
/// superseded connection can never move its device claim to another device.
#[tokio::test]
async fn superseded_capability_replay_is_stale_and_never_moves_device() {
    let (handle, _dir) = open_handle("stale-unknown").await.unwrap();
    let transport = fake_transport();
    let device = uuid::Uuid::from_u128(7);
    let device_wire = device.as_hyphenated().to_string();
    let table = Arc::new(ConnectionTable::new());
    let first = table.note_accept();
    authenticate_conn(&table, &first, &device_wire);
    let second = table.note_accept();
    authenticate_conn(&table, &second, &device_wire);
    let replay = dispatch(
        &handle,
        &table,
        &first,
        advertise_frame(Some(device), ProtocolVersion::V1),
        &transport,
    )
    .await;
    assert!(
        replay.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::Reject(notice) if notice.kind == RejectKind::StaleConnection
        )),
        "a superseded capability replay is stale, got {replay:?}"
    );
    assert_eq!(table.challenge_nonce_of(&first), None);
    // A different claim on a superseded connection cannot be attributed to
    // it at all: the table drops the frame without a reply (IPC §5).
    let moved = dispatch(
        &handle,
        &table,
        &first,
        advertise_frame(Some(uuid::Uuid::from_u128(99)), ProtocolVersion::V1),
        &transport,
    )
    .await;
    assert!(
        moved.is_empty(),
        "a superseded connection can never move its device claim, got {moved:?}"
    );
    assert_eq!(
        table.phase_of(&first),
        Some(ConnectionPhase::Superseded),
        "the dropped frame changes no phase"
    );
    assert!(table.current_authenticated(&device_wire));
}

/// S5-03 / #1384: with a lingering superseded socket, closing the current
/// connection still satisfies the presence fallback condition (no current
/// authenticated connection), and the lingering socket cannot revive it.
#[tokio::test]
async fn current_close_falls_back_even_with_a_lingering_superseded_socket() {
    let (handle, _dir) = open_handle("close-linger").await.unwrap();
    let table = Arc::new(ConnectionTable::new());
    let first = table.note_accept();
    authenticate_conn(&table, &first, "laptop");
    let second = table.note_accept();
    authenticate_conn(&table, &second, "laptop");
    make_present(&handle, "laptop").await;
    let (state, _, _) = presence_state(&handle).await;
    assert_eq!(state, PresenceState::Present);

    // Closing the current connection falls back even though C1 lingers.
    handle.close_connection(&table, second).await;
    let (state, active, _) = presence_state(&handle).await;
    assert_eq!(
        state,
        PresenceState::NoActive,
        "the current close falls back with a lingering superseded socket"
    );
    assert_eq!(active, None);
    assert!(!table.current_authenticated("laptop"));
    assert_eq!(table.phase_of(&first), Some(ConnectionPhase::Superseded));

    // The lingering socket's close runs once more but never revives the old
    // current: the fallback finds nothing Present to move.
    handle.close_connection(&table, first).await;
    assert!(!table.current_authenticated("laptop"));
    let (state, active, _) = presence_state(&handle).await;
    assert_eq!(state, PresenceState::NoActive);
    assert_eq!(active, None);
}

/// S5-05 order 1: the old connection's close completes before the new
/// authentication. The fallback runs (the old connection was current), and
/// the later install becomes current without reviving presence.
#[tokio::test]
async fn close_before_auth_falls_back_and_the_install_does_not_revive() {
    let (handle, _dir) = open_handle("close-then-auth").await.unwrap();
    let table = Arc::new(ConnectionTable::new());
    let first = table.note_accept();
    authenticate_conn(&table, &first, "laptop");
    make_present(&handle, "laptop").await;
    handle.close_connection(&table, first).await;
    let (state, _, _) = presence_state(&handle).await;
    assert_eq!(state, PresenceState::NoActive, "the close falls back");
    assert!(!table.current_authenticated("laptop"));

    let second = table.note_accept();
    authenticate_conn(&table, &second, "laptop");
    assert!(table.current_authenticated("laptop"));
    let (state, active, _) = presence_state(&handle).await;
    assert_eq!(
        state,
        PresenceState::NoActive,
        "authentication alone never revives presence"
    );
    assert_eq!(active, None);
}

/// S5-05 order 2: the new authentication installs while the old close is
/// paused before its table section. The close then re-reads currentness and
/// never clears the new current nor runs the fallback.
#[tokio::test]
async fn close_racing_a_new_auth_never_clears_the_new_current() {
    let (handle, _dir) = open_handle("auth-then-close").await.unwrap();
    let handle = Arc::new(handle);
    let table = Arc::new(ConnectionTable::new());
    let first = table.note_accept();
    authenticate_conn(&table, &first, "laptop");
    make_present(&handle, "laptop").await;

    let gate = handle.arm_close_gate();
    let close_handle = Arc::clone(&handle);
    let close_table = Arc::clone(&table);
    let closing = tokio::spawn(async move {
        close_handle.close_connection(&close_table, first).await;
    });
    gate.wait_entered().await;
    // The new authentication installs while the close is paused.
    let second = table.note_accept();
    authenticate_conn(&table, &second, "laptop");
    assert_eq!(table.phase_of(&first), Some(ConnectionPhase::Superseded));
    gate.release();
    closing.await.expect("the close task must finish");

    assert!(
        table.current_authenticated("laptop"),
        "the old close never clears the new current"
    );
    assert_eq!(
        table.phase_of(&second),
        Some(ConnectionPhase::Authenticated)
    );
    let (state, active, _) = presence_state(&handle).await;
    assert_eq!(
        state,
        PresenceState::Present,
        "the paused close re-reads currentness and runs no fallback"
    );
    assert_eq!(active, Some(device_client("laptop")));
}

/// Attaches `client_ref` as the `Present` client through the production
/// compare-and-commit, returning the companion and the committed generation.
async fn attach_present(
    handle: &HostHandle,
    client_ref: &str,
) -> (ene_companion::CompanionId, PresenceGeneration) {
    use ene_companion::CompanionRepository as _;

    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion resolves");
    let current = handle
        .store
        .load_attribution(companion.as_raw())
        .await
        .expect("attribution loads")
        .expect("the seeded attribution exists");
    assert_eq!(current.state, PresenceState::NoActive);
    let client = device_client(client_ref);
    let begin = handle
        .store
        .compare_and_begin_transition(
            companion.as_raw(),
            PresenceCheckRef {
                expected_generation: current.generation,
                expected_state: PresenceState::NoActive,
                expected_active: None,
            },
            Some(client),
            ThinMoveReason::InitialAttach,
        )
        .await
        .expect("begin answers");
    let MoveDecision::TransitioningToNew { generation } = begin else {
        panic!("the attach begin must transition, got {begin:?}");
    };
    let confirmed = handle
        .store
        .confirm_transition(
            companion.as_raw(),
            generation,
            LiveReachabilityRef {
                client,
                connection_live: true,
            },
        )
        .await
        .expect("confirm answers");
    let ConfirmTransitionOutcome::Confirmed(fact) = confirmed else {
        panic!("the live confirm must crown the pinned client, got {confirmed:?}");
    };
    (companion, fact.generation)
}

#[tokio::test]
async fn disconnect_falls_back_to_a_permitted_same_machine_client() {
    let (handle, _dir) = open_handle("disc-fallback").await.unwrap();
    let (companion, _generation) = attach_present(&handle, "client-a").await;
    let fallback = CurrentConnection {
        client_ref: String::from("client-b"),
        same_machine: true,
        device_permitted: true,
    };
    handle
        .note_disconnect_with("client-a", &|| vec![fallback.clone()])
        .await;

    let fact = handle
        .store
        .load_attribution(companion.as_raw())
        .await
        .expect("attribution loads")
        .expect("the attribution exists");
    assert_eq!(
        fact.state,
        PresenceState::Present,
        "an eligible same-machine candidate takes the fallback"
    );
    assert_eq!(fact.active_client, Some(device_client("client-b")));
    let hint = handle
        .store
        .load_hint(companion.as_raw())
        .await
        .expect("hint loads")
        .expect("the hint row exists");
    assert_eq!(hint.last_client, Some(device_client("client-b")));
    assert_eq!(hint.recovery_destination, None);
}

#[tokio::test]
async fn disconnect_fallback_ignores_non_permitted_or_remote_candidates() {
    let (handle, _dir) = open_handle("disc-fallback-skip").await.unwrap();
    let (companion, _generation) = attach_present(&handle, "client-a").await;
    let remote = CurrentConnection {
        client_ref: String::from("client-b"),
        same_machine: false,
        device_permitted: true,
    };
    let forbidden = CurrentConnection {
        client_ref: String::from("client-c"),
        same_machine: true,
        device_permitted: false,
    };
    handle
        .note_disconnect_with("client-a", &|| vec![remote.clone(), forbidden.clone()])
        .await;

    let fact = handle
        .store
        .load_attribution(companion.as_raw())
        .await
        .expect("attribution loads")
        .expect("the attribution exists");
    assert_eq!(
        fact.state,
        PresenceState::NoActive,
        "no ineligible candidate may be crowned"
    );
    assert_eq!(fact.active_client, None);
    let hint = handle
        .store
        .load_hint(companion.as_raw())
        .await
        .expect("hint loads")
        .expect("the hint row exists");
    assert_eq!(
        hint.last_client,
        Some(device_client("client-a")),
        "a failed fallback keeps the last confirmed client as history"
    );
}

#[tokio::test]
async fn disconnect_fallback_never_crowns_a_target_that_lost_currentness() {
    let (handle, _dir) = open_handle("disc-fallback-lost").await.unwrap();
    let (companion, _generation) = attach_present(&handle, "client-a").await;
    // The snapshot reports B at selection time and is empty by confirm time:
    // the confirm-time re-derivation must answer NoActive, never Present.
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let source = || {
        if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            vec![CurrentConnection {
                client_ref: String::from("client-b"),
                same_machine: true,
                device_permitted: true,
            }]
        } else {
            Vec::new()
        }
    };
    handle.note_disconnect_with("client-a", &source).await;

    let fact = handle
        .store
        .load_attribution(companion.as_raw())
        .await
        .expect("attribution loads")
        .expect("the attribution exists");
    assert_eq!(
        fact.state,
        PresenceState::NoActive,
        "a target that lost currentness before confirm is never Present"
    );
    assert_eq!(fact.active_client, None);
    let hint = handle
        .store
        .load_hint(companion.as_raw())
        .await
        .expect("hint loads")
        .expect("the hint row exists");
    assert_eq!(hint.last_client, Some(device_client("client-a")));
}

#[tokio::test]
async fn plain_state_open_does_not_normalize_presence() {
    use ene_credential::MemoryCredentialStore;

    let (handle, dir) = open_handle("disc-no-normalize").await.unwrap();
    let (companion, generation) = attach_present(&handle, "client-a").await;
    drop(handle);

    // A plain state open (read-only list / report / management paths) must
    // not run the startup normalization: the row reads back exactly.
    let reopened = HostHandle::open_with_cred_store(
        dir.path(),
        CredStore::Memory(MemoryCredentialStore::new()),
    )
    .await
    .expect("the plain state open succeeds");
    let untouched = reopened
        .store
        .load_attribution(companion.as_raw())
        .await
        .expect("attribution loads")
        .expect("the attribution exists");
    assert_eq!(untouched.state, PresenceState::Present);
    assert_eq!(untouched.generation, generation);

    // The explicit serving boundary normalizes once: Present becomes
    // RecoveryWait toward the same client with a new generation.
    reopened
        .normalize_presence_on_startup()
        .await
        .expect("the serving boundary normalizes");
    let normalized = reopened
        .store
        .load_attribution(companion.as_raw())
        .await
        .expect("attribution loads")
        .expect("the attribution exists");
    assert_eq!(normalized.state, PresenceState::RecoveryWait);
    assert_eq!(normalized.generation.as_u64(), generation.as_u64() + 1);
    let hint = reopened
        .store
        .load_hint(companion.as_raw())
        .await
        .expect("hint loads")
        .expect("the hint row exists");
    assert_eq!(hint.recovery_destination, Some(device_client("client-a")));
    drop(reopened);

    // Reopening after the normalization keeps the waiting state (the crash
    // between startup commits must not lose the recovery intent), and a
    // second explicit normalization refreshes it.
    let again = HostHandle::open_with_cred_store(
        dir.path(),
        CredStore::Memory(MemoryCredentialStore::new()),
    )
    .await
    .expect("the reopen succeeds");
    let waiting = again
        .store
        .load_attribution(companion.as_raw())
        .await
        .expect("attribution loads")
        .expect("the attribution exists");
    assert_eq!(waiting.state, PresenceState::RecoveryWait);
    again
        .normalize_presence_on_startup()
        .await
        .expect("the second serving boundary normalizes");
    let refreshed = again
        .store
        .load_attribution(companion.as_raw())
        .await
        .expect("attribution loads")
        .expect("the attribution exists");
    assert_eq!(refreshed.state, PresenceState::RecoveryWait);
    assert_eq!(
        refreshed.generation.as_u64(),
        waiting.generation.as_u64() + 1
    );
}

#[tokio::test]
async fn startup_normalization_refusal_fails_the_serving_boundary() {
    use ene_credential::MemoryCredentialStore;

    let (handle, dir) = open_handle("disc-normalize-fail").await.unwrap();
    let (companion, _generation) = attach_present(&handle, "client-a").await;
    let key = companion.as_raw().as_uuid().as_hyphenated().to_string();
    drop(handle);
    {
        let conn = rusqlite::Connection::open(dir.path().join("app.db"))
            .expect("the store file opens for the probe");
        conn.execute(
            "UPDATE presence_attribution SET generation = ?1 WHERE companion_id = ?2",
            rusqlite::params![i64::MAX, key],
        )
        .expect("the exhaustion fixture writes");
    }
    let reopened = HostHandle::open_with_cred_store(
        dir.path(),
        CredStore::Memory(MemoryCredentialStore::new()),
    )
    .await
    .expect("the state open succeeds");
    let refused = reopened.normalize_presence_on_startup().await;
    assert!(
        refused.is_err(),
        "a per-companion refusal must fail startup, not be silently accepted"
    );
    let fact = reopened
        .store
        .load_attribution(companion.as_raw())
        .await
        .expect("attribution loads")
        .expect("the attribution exists");
    assert_eq!(
        fact.generation.as_u64(),
        i64::MAX as u64,
        "the refused companion keeps its old attribution"
    );
}

/// A full control channel reports Full instead of queueing: the failed
/// frame never enters the channel, so the caller observes the failure
/// rather than silently dropping the frame.
#[test]
fn control_emit_reports_full_without_queueing() {
    use super::{FrameDeliveryError, FrameSink, STREAM_BUFFER_FRAMES};

    let (tx, mut rx) = tokio::sync::mpsc::channel(STREAM_BUFFER_FRAMES);
    let filler = pairing_frame("fill");
    for _ in 0..STREAM_BUFFER_FRAMES {
        tx.try_send(filler.clone()).expect("the prefill must fit");
    }
    let mut sink = tx.clone();
    assert_eq!(
        sink.emit(filler.clone()),
        Err(FrameDeliveryError::Full),
        "a full control channel must fail loudly"
    );
    let mut drained = 0;
    while rx.try_recv().is_ok() {
        drained += 1;
    }
    assert_eq!(
        drained, STREAM_BUFFER_FRAMES,
        "the failed frame was never queued"
    );
}

/// A gone connection reports Closed, distinct from Full.
#[test]
fn control_emit_reports_closed() {
    use super::{FrameDeliveryError, FrameSink};

    let (tx, rx) = tokio::sync::mpsc::channel(2);
    drop(rx);
    let mut sink = tx;
    assert_eq!(
        sink.emit(pairing_frame("gone")),
        Err(FrameDeliveryError::Closed),
        "a gone connection must report Closed, not Full"
    );
}

/// Cancel admission on the handle commits first, then signals the running
/// execution's cooperative token; refused admissions signal nothing.
#[tokio::test]
async fn cancel_task_admission_wires_the_cooperative_stop() {
    use ene_task::{
        AssigneeRef, CancelTaskCommand, DelegationCreationPremise, DelegationId, DelegationOutcome,
        DelegationScope, TaskAgentEphemeralId, TaskAgentOutput, TaskCancelOutcome,
        TaskContextEntryId, TaskContextOrigin, TaskContextOriginKind, TaskCreationPremise, TaskId,
        TaskProgress, TaskPurpose, TaskRepository as _, TaskResultAcceptance,
        TaskResultAdoptionClaim, orchestrate_result_arrival,
    };

    let Some((handle, _dir)) = memory_handle("cancel-task").await else {
        panic!("the handle must open");
    };
    let task = handle
        .store
        .create_task(TaskCreationPremise {
            task: TaskId::generate(),
            purpose: TaskPurpose {
                text: String::from("cancellable"),
            },
            entry: TaskContextEntryId::generate(),
            origin: TaskContextOrigin {
                kind: TaskContextOriginKind::OwnerConversation,
                source: ene_primitive::RawId::new(),
            },
            acquired_at: ene_primitive::WallClockWithTz::now(),
            assignee: AssigneeRef {
                companion: ene_primitive::RawId::new(),
            },
            workspace: None,
        })
        .await
        .expect("task creation commits");

    // A running execution is registered under the Task; the admission signals
    // its cooperative token after the durable commit. The registration key is
    // the execution's delegation identity; this Task has no durable
    // delegation, so the key is just a fresh identity.
    let registration = handle
        .task_executions
        .register(DelegationId::generate(), task.task)
        .expect("the running execution registers");
    assert_eq!(
        handle
            .cancel_task(CancelTaskCommand { task: task.task })
            .await
            .unwrap(),
        TaskCancelOutcome::CancelAccepted
    );
    assert!(
        registration.cancellation.is_aborted(),
        "the admission signals the running execution"
    );
    let loaded = handle.store.load_task(task.task).await.unwrap().unwrap();
    assert_eq!(loaded.task.progress, TaskProgress::Cancelled);

    // Re-requests and missing identities are refused without a signal.
    assert_eq!(
        handle
            .cancel_task(CancelTaskCommand { task: task.task })
            .await
            .unwrap(),
        TaskCancelOutcome::AlreadyCancelled
    );
    let missing = TaskId::generate();
    assert_eq!(
        handle
            .cancel_task(CancelTaskCommand { task: missing })
            .await
            .unwrap(),
        TaskCancelOutcome::MissingTask { task: missing }
    );

    // A completed Task answers TaskTerminal and never signals a registered
    // token (a terminal Task admits no running work).
    let completed = handle
        .store
        .create_task(TaskCreationPremise {
            task: TaskId::generate(),
            purpose: TaskPurpose {
                text: String::from("already done"),
            },
            entry: TaskContextEntryId::generate(),
            origin: TaskContextOrigin {
                kind: TaskContextOriginKind::OwnerConversation,
                source: ene_primitive::RawId::new(),
            },
            acquired_at: ene_primitive::WallClockWithTz::now(),
            assignee: AssigneeRef {
                companion: ene_primitive::RawId::new(),
            },
            workspace: None,
        })
        .await
        .unwrap();
    let completed_delegation = DelegationId::generate();
    let delegated = handle
        .store
        .create_delegation(DelegationCreationPremise {
            delegation: completed_delegation,
            task: completed,
            agent: TaskAgentEphemeralId::generate(),
            scope_copy: DelegationScope { workspace: None },
        })
        .await
        .unwrap();
    assert!(matches!(delegated, DelegationOutcome::Delegated(_)));
    let arrival = orchestrate_result_arrival(
        &handle.store,
        completed_delegation,
        TaskAgentOutput::new(String::from("done")),
    )
    .await
    .unwrap();
    let adopted = handle
        .store
        .adopt_result(TaskResultAdoptionClaim {
            result: arrival.result,
            attempt_refs: Vec::new(),
        })
        .await
        .unwrap();
    assert!(matches!(
        adopted,
        TaskResultAcceptance::AdoptedAsCompletion(_)
    ));
    let completed_registration = handle
        .task_executions
        .register(completed_delegation, completed.task)
        .expect("the running execution registers");
    assert_eq!(
        handle
            .cancel_task(CancelTaskCommand {
                task: completed.task
            })
            .await
            .unwrap(),
        TaskCancelOutcome::TaskTerminal {
            task: completed.task,
            progress: TaskProgress::Completed,
        }
    );
    assert!(
        !completed_registration.cancellation.is_aborted(),
        "a refused admission never signals"
    );
}

/// The first-party management cancel reaches the same
/// [`HostHandle::cancel_task`] admission as the conversation path, preserves
/// the typed outcome meanings the management vocabulary can carry, and
/// replays a retried intent from its durable snapshot.
#[tokio::test]
async fn management_cancel_reaches_the_cancel_admission_and_replays() {
    use ene_primitive::RawId;
    use ene_task::{
        AssigneeRef, DelegationCreationPremise, DelegationId, DelegationOutcome, DelegationScope,
        TaskAgentEphemeralId, TaskAgentOutput, TaskContextEntryId, TaskContextOrigin,
        TaskContextOriginKind, TaskCreationPremise, TaskId, TaskProgress, TaskPurpose,
        TaskRepository as _, TaskResultAcceptance, TaskResultAdoptionClaim,
        orchestrate_result_arrival,
    };

    let Some((handle, _dir)) = memory_handle("management-cancel").await else {
        panic!("the handle must open");
    };
    let live = paired_input("management-cancel");
    let transport = fake_transport();

    let task = handle
        .store
        .create_task(TaskCreationPremise {
            task: TaskId::generate(),
            purpose: TaskPurpose {
                text: String::from("cancel me"),
            },
            entry: TaskContextEntryId::generate(),
            origin: TaskContextOrigin {
                kind: TaskContextOriginKind::OwnerConversation,
                source: RawId::new(),
            },
            acquired_at: ene_primitive::WallClockWithTz::now(),
            assignee: AssigneeRef {
                companion: RawId::new(),
            },
            workspace: None,
        })
        .await
        .expect("task creation commits");
    let target = format!("task:{}", task.task.as_raw().as_uuid().as_hyphenated());
    let intent_id = CommandWireId(RawId::new().as_uuid());
    let frame =
        management_intent_frame(&live, ManagementIntentKind::CancelTask, &target, intent_id);
    let responses = handle
        .handle_frame(frame.clone(), live.clone(), &transport)
        .await;
    let Some(WirePayload::ManagementOutcome(outcome)) = responses.first().map(|f| &f.payload)
    else {
        panic!("the intent must answer an outcome, got {responses:?}");
    };
    assert_eq!(outcome, &ManagementOutcome::AppliedAsOneTime);
    assert_eq!(
        handle
            .store
            .load_task(task.task)
            .await
            .unwrap()
            .unwrap()
            .task
            .progress,
        TaskProgress::Cancelled
    );

    // An exact retry replays the stored snapshot without a second admission.
    let replays = handle.handle_frame(frame, live.clone(), &transport).await;
    let Some(WirePayload::ManagementOutcome(outcome)) = replays.first().map(|f| &f.payload) else {
        panic!("the replay must answer an outcome, got {replays:?}");
    };
    assert_eq!(outcome, &ManagementOutcome::AppliedAsOneTime);

    // A fresh intent observing the already-cancelled Task answers the same
    // one-time application: admission happened exactly once.
    let fresh_id = CommandWireId(RawId::new().as_uuid());
    let responses = handle
        .handle_frame(
            management_intent_frame(&live, ManagementIntentKind::CancelTask, &target, fresh_id),
            live.clone(),
            &transport,
        )
        .await;
    let Some(WirePayload::ManagementOutcome(outcome)) = responses.first().map(|f| &f.payload)
    else {
        panic!("the fresh intent must answer an outcome");
    };
    assert_eq!(outcome, &ManagementOutcome::AppliedAsOneTime);

    // A terminal Task for another reason cannot be cancelled: clarify, never
    // a fabricated admission.
    let completed = handle
        .store
        .create_task(TaskCreationPremise {
            task: TaskId::generate(),
            purpose: TaskPurpose {
                text: String::from("already done"),
            },
            entry: TaskContextEntryId::generate(),
            origin: TaskContextOrigin {
                kind: TaskContextOriginKind::OwnerConversation,
                source: RawId::new(),
            },
            acquired_at: ene_primitive::WallClockWithTz::now(),
            assignee: AssigneeRef {
                companion: RawId::new(),
            },
            workspace: None,
        })
        .await
        .unwrap();
    let delegation = DelegationId::generate();
    let delegated = handle
        .store
        .create_delegation(DelegationCreationPremise {
            delegation,
            task: completed,
            agent: TaskAgentEphemeralId::generate(),
            scope_copy: DelegationScope { workspace: None },
        })
        .await
        .unwrap();
    assert!(matches!(delegated, DelegationOutcome::Delegated(_)));
    let arrival = orchestrate_result_arrival(
        &handle.store,
        delegation,
        TaskAgentOutput::new(String::from("done")),
    )
    .await
    .unwrap();
    assert!(matches!(
        handle
            .store
            .adopt_result(TaskResultAdoptionClaim {
                result: arrival.result,
                attempt_refs: Vec::new(),
            })
            .await
            .unwrap(),
        TaskResultAcceptance::AdoptedAsCompletion(_)
    ));
    let completed_target = format!("task:{}", completed.task.as_raw().as_uuid().as_hyphenated());
    let responses = handle
        .handle_frame(
            management_intent_frame(
                &live,
                ManagementIntentKind::CancelTask,
                &completed_target,
                CommandWireId(RawId::new().as_uuid()),
            ),
            live,
            &transport,
        )
        .await;
    let Some(WirePayload::ManagementOutcome(outcome)) = responses.first().map(|f| &f.payload)
    else {
        panic!("the completed intent must answer an outcome");
    };
    assert_eq!(outcome, &ManagementOutcome::NeedsClarification);
}

/// Builds one Stage 5 domain frame under `live`, stamped with the
/// connection binding the premises carry.
fn stage5_frame(live: &LiveInput, payload: WirePayload) -> super::WireFrame {
    let mut frame = super::WireFrame {
        envelope: new_outgoing_envelope(
            ProtocolVersion::V1,
            sender(),
            WireMessageType(payload.message_type().to_string()),
        ),
        payload,
    };
    frame.envelope.sender.connection_id = Some(live.connection_id);
    frame.envelope.correlation.command_id = Some(CommandWireId(uuid::Uuid::new_v4()));
    frame
}

fn stage5_payloads() -> Vec<(&'static str, WirePayload)> {
    vec![
        (
            "undelivered request",
            WirePayload::UndeliveredRequest(UndeliveredRequest {
                companion: None,
                cursor: None,
                limit: None,
                redisplay: false,
            }),
        ),
        (
            "undelivered ack",
            WirePayload::UndeliveredAck(UndeliveredAck {
                receipt: PresentationReceiptWireRef(String::from("receipt")),
                status: PresentationStatus::Presented,
            }),
        ),
        (
            "task list",
            WirePayload::ListTasks(ListTasks {
                cursor: None,
                limit: None,
            }),
        ),
        (
            "task report",
            WirePayload::GetTaskReport(GetTaskReport {
                task: TaskWireRef(String::from("task")),
                cursor: None,
                limit: None,
            }),
        ),
        (
            "report source",
            WirePayload::GetReportSource(GetReportSource {
                source: ReportSourceWireRef(String::from("source")),
                cursor: None,
                limit_bytes: None,
            }),
        ),
        (
            "task selection",
            WirePayload::SelectTask(SelectTask {
                task: TaskWireRef(String::from("task")),
            }),
        ),
        (
            "resume",
            WirePayload::ResumeTask(ResumeTask {
                task: TaskWireRef(String::from("task")),
                expected_revision: 1,
                expected_purpose: String::from("purpose"),
                instruction: String::from("continue"),
            }),
        ),
    ]
}

/// Stage 5 handlers keep the existing three-way gate semantics (IPC §11.3):
/// a superseded connection answers a typed `StaleConnection` with the socket
/// kept open and zero domain effect, while an unauthenticated connection
/// still gets the terminal unpaired close.
#[tokio::test]
async fn stage5_frames_distinguish_superseded_from_unauthenticated() {
    let (handle, _dir) = open_handle("stage5-gate").await.unwrap();
    let transport = fake_transport();
    let device = uuid::Uuid::from_u128(11);
    let device_wire = device.as_hyphenated().to_string();

    // C1 authenticates, then C2 supersedes it through the real install.
    let table = Arc::new(ConnectionTable::new());
    let c1 = table.note_accept();
    authenticate_conn(&table, &c1, &device_wire);
    let c2 = table.note_accept();
    authenticate_conn(&table, &c2, &device_wire);
    assert_eq!(table.phase_of(&c1), Some(ConnectionPhase::Superseded));

    let stale_live = live_of(&table, &c1);
    assert!(stale_live.phase.is_superseded());
    for (what, payload) in stage5_payloads() {
        let answers = handle
            .handle_frame(
                stage5_frame(&stale_live, payload),
                stale_live.clone(),
                &transport,
            )
            .await;
        assert_eq!(answers.len(), 1, "{what} answers exactly one frame");
        assert!(
            matches!(
                &answers[0].payload,
                WirePayload::Reject(notice) if notice.kind == RejectKind::StaleConnection
            ),
            "a superseded connection's {what} must be a typed stale rejection, got {:?}",
            answers[0].payload
        );
    }
    // The stale frames changed nothing: C2 is still current, presence stays
    // untouched, and no round, task, or undelivered row appeared.
    assert!(table.current_authenticated(&device_wire));
    assert_eq!(table.phase_of(&c1), Some(ConnectionPhase::Superseded));
    let (state, active, _) = presence_state(&handle).await;
    assert_eq!(state, PresenceState::NoActive);
    assert_eq!(active, None);
    assert!(
        !handle.has_open_round_for_test(),
        "stage 5 frames create no conversation round"
    );
    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must ensure");
    assert!(
        handle
            .store
            .list_unpresented(companion, None, 50)
            .await
            .expect("the listing must read")
            .entries
            .is_empty(),
        "stale frames register nothing undelivered"
    );

    // An accepted-but-unauthenticated connection keeps the terminal unpaired
    // close for the same frames.
    let (fresh_table, unauthed) = fresh_conn();
    let unpaired_live = live_of(&fresh_table, &unauthed);
    for (what, payload) in stage5_payloads() {
        let answers = handle
            .handle_frame(
                stage5_frame(&unpaired_live, payload),
                unpaired_live.clone(),
                &transport,
            )
            .await;
        assert_eq!(answers.len(), 1, "{what} answers exactly one frame");
        assert!(
            matches!(&answers[0].payload, WirePayload::DisconnectNotice(_)),
            "an unauthenticated {what} must keep the unpaired close, got {:?}",
            answers[0].payload
        );
    }
}

/// S5 connection lifecycle: a superseding install drops the replaced
/// connection's presentation state at the install point, and the superseded
/// socket's stale frames cannot rebuild or extend it.
#[tokio::test]
async fn supersession_drops_the_replaced_connections_presentation_state() {
    use ene_api::v1::undelivered::{
        GetTaskReport, ListTasks, TaskListResponse, TaskReportResponse,
    };
    use ene_primitive::{RawId, WallClockWithTz};
    use ene_task::{
        AssigneeRef, TaskContextEntryId, TaskContextOrigin, TaskContextOriginKind,
        TaskCreationPremise, TaskId, TaskPurpose, TaskRepository as _,
    };

    let (handle, _dir) = open_handle("stale-purge").await.unwrap();
    let transport = fake_transport();
    // Pair a device through the real approval flow.
    let (probe_table, probe_id) = fresh_conn();
    let pending = dispatch(
        &handle,
        &probe_table,
        &probe_id,
        pairing_frame("laptop"),
        &transport,
    )
    .await;
    let probe_pending = pending_id_of(pending.first().expect("the probe must pend"));
    let approved = handle.approve_device(&probe_pending).await;
    let Ok(Some((record, secret))) = approved else {
        panic!("owner approval must pair, got {approved:?}");
    };
    let device_wire = record.wire.clone();
    let device = DeviceWireId(uuid::Uuid::parse_str(&device_wire).unwrap());
    let paired = dispatch(
        &handle,
        &probe_table,
        &probe_id,
        pairing_poll("laptop", Some(probe_pending)),
        &transport,
    )
    .await;
    assert!(
        paired.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::PairingResult(PairingResult::Paired { .. })
        )),
        "the approved pending must pair on poll"
    );

    // Seed one Task so the list/report queries mint connection-scoped refs.
    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must ensure");
    let _task = handle
        .store
        .create_task(TaskCreationPremise {
            task: TaskId::generate(),
            purpose: TaskPurpose {
                text: String::from("look at the input"),
            },
            entry: TaskContextEntryId::generate(),
            origin: TaskContextOrigin {
                kind: TaskContextOriginKind::OwnerConversation,
                source: RawId::new(),
            },
            acquired_at: WallClockWithTz::now(),
            assignee: AssigneeRef {
                companion: companion.as_raw(),
            },
            workspace: None,
        })
        .await
        .expect("the task must seed");

    let stage5 = |payload: WirePayload, live: &LiveInput, device: DeviceWireId| {
        let mut frame = super::WireFrame {
            envelope: new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(payload.message_type().to_string()),
            ),
            payload,
        };
        frame.envelope.sender.device_id = Some(device);
        frame.envelope.sender.connection_id = Some(live.connection_id);
        frame
    };
    let list_query_of = |frame: &super::WireFrame| match &frame.payload {
        WirePayload::ListTasks(query) => query.clone(),
        other => panic!("expected a list frame, got {other:?}"),
    };
    let list_page_of =
        |frames: Vec<super::WireFrame>| match frames.into_iter().next().unwrap().payload {
            WirePayload::TaskListResponse(TaskListResponse::Page(page)) => page,
            other => panic!("expected a list page, got {other:?}"),
        };

    // C1 authenticates through the real challenge/proof exchange.
    let (table, c1) = fresh_conn();
    let challenged = dispatch(
        &handle,
        &table,
        &c1,
        advertise_frame(Some(device.0), ProtocolVersion::V1),
        &transport,
    )
    .await;
    let Some(WirePayload::AuthChallenge(challenge)) = challenged.get(1).map(|f| &f.payload) else {
        panic!("C1 must challenge, got {challenged:?}");
    };
    let accepted = dispatch(
        &handle,
        &table,
        &c1,
        proof_frame(device, &pairing_proof_hex(&secret, &challenge.nonce)),
        &transport,
    )
    .await;
    assert!(
        accepted.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::AuthResult(AuthResult::Accepted { .. })
        )),
        "C1 must authenticate, got {accepted:?}"
    );

    // C1's connection-owned presentation state: a Task ref, its report
    // source ref, and the list cursor.
    let c1_live = live_of(&table, &c1);
    let list = stage5(
        WirePayload::ListTasks(ListTasks {
            cursor: None,
            limit: Some(1),
        }),
        &c1_live,
        device,
    );
    let listed = list_page_of(
        handle
            .list_tasks_wire(&list, &c1_live, &list_query_of(&list))
            .await,
    );
    assert_eq!(listed.tasks.len(), 1);
    let report = stage5(
        WirePayload::GetTaskReport(GetTaskReport {
            task: listed.tasks[0].task.clone(),
            cursor: None,
            limit: None,
        }),
        &c1_live,
        device,
    );
    let report_query = match &report.payload {
        WirePayload::GetTaskReport(query) => query.clone(),
        other => panic!("expected a report frame, got {other:?}"),
    };
    let reports = handle.report_wire(&report, &c1_live, &report_query).await;
    assert!(
        matches!(
            reports.first().map(|frame| &frame.payload),
            Some(WirePayload::TaskReportResponse(TaskReportResponse::Page(_)))
        ),
        "the report must mint its source ref"
    );
    {
        let counts = handle.presentation_counts_for_test(&c1);
        assert!(counts.task_refs > 0, "C1 owns a Task ref");
        assert!(counts.source_refs > 0, "C1 owns a source ref");
        assert!(counts.cursors > 0, "C1 owns a page cursor");
    }

    // C2 authenticates on a fresh connection: C1 is superseded and its
    // presentation state dies at the install point.
    let c2 = table.note_accept();
    let challenged = dispatch(
        &handle,
        &table,
        &c2,
        advertise_frame(Some(device.0), ProtocolVersion::V1),
        &transport,
    )
    .await;
    let Some(WirePayload::AuthChallenge(challenge)) = challenged.get(1).map(|f| &f.payload) else {
        panic!("C2 must challenge, got {challenged:?}");
    };
    let accepted = dispatch(
        &handle,
        &table,
        &c2,
        proof_frame(device, &pairing_proof_hex(&secret, &challenge.nonce)),
        &transport,
    )
    .await;
    assert!(
        accepted.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::AuthResult(AuthResult::Accepted { .. })
        )),
        "C2 must authenticate, got {accepted:?}"
    );
    assert_eq!(table.phase_of(&c1), Some(ConnectionPhase::Superseded));
    assert!(
        handle.presentation_counts_for_test(&c1).is_empty(),
        "supersession drops every replaced-connection entry"
    );

    // C2's own state is never touched by C1's stale traffic.
    let c2_live = live_of(&table, &c2);
    let list_c2 = stage5(
        WirePayload::ListTasks(ListTasks {
            cursor: None,
            limit: Some(1),
        }),
        &c2_live,
        device,
    );
    let _ = list_page_of(
        handle
            .list_tasks_wire(&list_c2, &c2_live, &list_query_of(&list_c2))
            .await,
    );
    let c2_before = handle.presentation_counts_for_test(&c2);
    assert!(c2_before.task_refs > 0);

    let stale = dispatch(
        &handle,
        &table,
        &c1,
        stage5(
            WirePayload::ListTasks(ListTasks {
                cursor: None,
                limit: Some(1),
            }),
            &c1_live,
            device,
        ),
        &transport,
    )
    .await;
    assert!(
        stale.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::Reject(notice) if notice.kind == RejectKind::StaleConnection
        )),
        "C1's list must be stale, got {stale:?}"
    );
    assert!(
        handle.presentation_counts_for_test(&c1).is_empty(),
        "a stale frame cannot rebuild purged state"
    );
    assert_eq!(
        handle.presentation_counts_for_test(&c2),
        c2_before,
        "C2's state is untouched"
    );
}

/// Stage 6 A1b production path: a Client intent only stages a Targeted
/// Deletion request, the Owner confirms on the Host-local trusted inlet, the
/// canonical operation starts, and the bounded status view reports it.
#[tokio::test]
async fn targeted_deletion_awaits_host_confirmation_then_starts_once() {
    use ene_preservation::{
        ConfirmTargetedDeletionOutcome, DeletionPurpose, PreservationRepository as _,
    };
    use ene_primitive::RawId;

    let Some((handle, _dir)) = memory_handle("management-deletion").await else {
        panic!("the handle must open");
    };
    let live = paired_input("management-deletion");
    let transport = fake_transport();

    // The Client learns the live surface mark from the bounded status read.
    let initial = read_deletion_page(&handle, &live, &transport, None, None).await;
    assert!(
        initial.operations.is_empty(),
        "a fresh surface has no operations"
    );
    let mark = initial.mark.0;

    // The intent only stages. The request body never renders through Debug.
    let intent_id = CommandWireId(RawId::new().as_uuid());
    let frame = deletion_intent_frame(
        &live,
        "leaked key",
        &mark,
        RationaleOrigin::ManagementSurface,
        Some("please remove the leaked key from everywhere"),
        intent_id,
    );
    let rendered = format!("{:?}", frame.payload);
    assert!(
        !rendered.contains("leaked key"),
        "the deletion request body never renders: {rendered}"
    );
    let responses = handle
        .handle_frame(frame.clone(), live.clone(), &transport)
        .await;
    assert_eq!(
        outcome_of(&responses),
        ManagementOutcome::NeedsClarification,
        "awaiting the Host PC confirmation"
    );
    assert!(
        handle
            .store
            .current_erasure_conditions(None, 10)
            .await
            .unwrap()
            .is_empty(),
        "the wire intent must publish no erasure condition"
    );
    assert!(
        handle
            .store
            .deletion_status(None, 10)
            .await
            .unwrap()
            .is_empty(),
        "the wire intent must create no operation"
    );

    // The Host-local preview shows the exact target; the Client never sees it.
    let pending = handle.pending_targeted_deletions(None, 50).await.unwrap();
    assert_eq!(pending.len(), 1, "one staged request");
    assert_eq!(pending[0].purpose(), DeletionPurpose::Privacy);
    assert_eq!(pending[0].owner_review_text(), "leaked key");
    let request = pending[0].request();

    // An exact retry replays the stored answer without a second request.
    let replay = handle.handle_frame(frame, live.clone(), &transport).await;
    assert_eq!(outcome_of(&replay), ManagementOutcome::NeedsClarification);
    assert_eq!(
        handle
            .pending_targeted_deletions(None, 50)
            .await
            .unwrap()
            .len(),
        1
    );

    // The Host-local trusted confirmation is the only destructive path.
    let request_text = request.as_raw().as_uuid().as_hyphenated().to_string();
    let ConfirmTargetedDeletionOutcome::Started(current) = handle
        .confirm_targeted_deletion(&request_text)
        .await
        .unwrap()
    else {
        panic!("the Owner confirmation must start the canonical operation");
    };
    assert_eq!(
        handle
            .store
            .current_erasure_conditions(None, 10)
            .await
            .unwrap()[0]
            .condition,
        current.condition(),
        "the current erasure condition is the operation's"
    );
    assert!(
        handle
            .pending_targeted_deletions(None, 50)
            .await
            .unwrap()
            .is_empty(),
        "a confirmed request leaves the pending set"
    );

    // The bounded status view reports the active operation without any body.
    let page = read_deletion_page(&handle, &live, &transport, None, None).await;
    assert_eq!(page.operations.len(), 1);
    let view = &page.operations[0];
    assert_eq!(view.phase, DeletionPhaseWire::Active);
    assert_eq!(view.sweep, 1);
    assert_eq!(view.purpose, DeletionPurposeWire::Privacy);
    // The durable snapshot is reported: every required owner is pending for
    // the current sweep, and the view never claims a completion it has none of.
    let DeletionParticipantReportWire::Reported(participants) = &view.participants else {
        panic!("a fresh operation must report its durable participant snapshot");
    };
    assert!(
        !participants.is_empty(),
        "the required snapshot is non-empty from admission"
    );
    assert!(
        participants
            .iter()
            .all(|participant| participant.progress == "pending" && participant.sweep == 1),
        "a freshly admitted operation has every participant pending: {participants:?}"
    );
    assert!(
        !format!("{view:?}").contains("leaked key"),
        "the status view never carries the target"
    );

    // A fresh duplicate intent observing the live surface is held by the
    // running operation; no second operation exists.
    let fresh = handle
        .handle_frame(
            deletion_intent_frame(
                &live,
                "leaked key",
                &page.mark.0,
                RationaleOrigin::ManagementSurface,
                None,
                CommandWireId(RawId::new().as_uuid()),
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert_eq!(outcome_of(&fresh), ManagementOutcome::HeldByOperation);
    assert_eq!(
        handle.store.deletion_status(None, 10).await.unwrap().len(),
        1
    );

    // A duplicate confirmation observes the same single operation.
    assert_eq!(
        handle
            .confirm_targeted_deletion(&request_text)
            .await
            .unwrap(),
        ConfirmTargetedDeletionOutcome::AlreadyCoveredBy(current)
    );
    assert_eq!(
        handle
            .store
            .current_erasure_conditions(None, 100)
            .await
            .unwrap()
            .len(),
        1
    );
}

/// The wire alone never reaches the destructive confirmation: conversation
/// wording, LLM-style rationale, and repeated fresh intents only stage.
#[tokio::test]
async fn targeted_deletion_is_unreachable_from_untrusted_client_wire_alone() {
    use ene_preservation::PreservationRepository as _;
    use ene_primitive::RawId;

    let Some((handle, _dir)) = memory_handle("management-deletion-untrusted").await else {
        panic!("the handle must open");
    };
    let live = paired_input("management-deletion-untrusted");
    let transport = fake_transport();

    for (index, text) in ["leaked key", "customer name", "private note"]
        .into_iter()
        .enumerate()
    {
        // Re-read the mark every time: the previous stage moved it.
        let page = read_deletion_page(&handle, &live, &transport, None, None).await;
        let responses = handle
            .handle_frame(
                deletion_intent_frame(
                    &live,
                    text,
                    &page.mark.0,
                    RationaleOrigin::Conversation,
                    Some("the model suggested deleting this"),
                    CommandWireId(RawId::new().as_uuid()),
                ),
                live.clone(),
                &transport,
            )
            .await;
        assert_eq!(
            outcome_of(&responses),
            ManagementOutcome::NeedsClarification,
            "request {index} only stages"
        );
    }
    assert_eq!(
        handle
            .pending_targeted_deletions(None, 50)
            .await
            .unwrap()
            .len(),
        3,
        "every advisory request is staged"
    );
    assert!(
        handle
            .store
            .deletion_status(None, 10)
            .await
            .unwrap()
            .is_empty(),
        "no untrusted request starts an operation"
    );
    assert!(
        handle
            .store
            .current_erasure_conditions(None, 10)
            .await
            .unwrap()
            .is_empty(),
        "no untrusted request publishes a condition"
    );
}

/// Stale premised, malformed, and non-deletion targets change nothing, and the
/// management journal never keeps the Owner's exact text.
#[tokio::test]
async fn targeted_deletion_stale_and_foreign_targets_leave_no_trace() {
    use ene_preservation::PreservationRepository as _;
    use ene_primitive::RawId;

    let Some((handle, dir)) = memory_handle("management-deletion-stale").await else {
        panic!("the handle must open");
    };
    let live = paired_input("management-deletion-stale");
    let transport = fake_transport();

    // A stale base view decides nothing and is not recorded, so the Client can
    // re-read and retry with the same intent id.
    let stale = handle
        .handle_frame(
            deletion_intent_frame(
                &live,
                "stale secret",
                "deletion-view/99/-/99/-",
                RationaleOrigin::ManagementSurface,
                None,
                CommandWireId(RawId::new().as_uuid()),
            ),
            live.clone(),
            &transport,
        )
        .await;
    let ManagementOutcome::StaleBaseView { current } = outcome_of(&stale) else {
        panic!("a stale mark must answer StaleBaseView, got {stale:?}");
    };
    let page = read_deletion_page(&handle, &live, &transport, None, None).await;
    assert_eq!(current.0, page.mark.0, "the answer names the live mark");
    assert!(
        handle
            .pending_targeted_deletions(None, 50)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        handle
            .store
            .deletion_status(None, 10)
            .await
            .unwrap()
            .is_empty()
    );

    // Malformed deletion targets and the deferred backup / restore / reset
    // families clarify; none of them stages anything. A refused target may
    // still carry the Owner's text, so the journal probe below covers these
    // too.
    for target in [
        "deletion:privacy:",
        "deletion:unknown:journal secret",
        "deletion:privacy",
        "deletion:",
        "backup:daily:journal secret",
        "restore:backup-1",
        "reset:all",
    ] {
        let responses = handle
            .handle_frame(
                deletion_intent_frame_raw(
                    &live,
                    target,
                    "deletion-view/99/-/99/-",
                    RationaleOrigin::ManagementSurface,
                    Some("journal secret in the rationale too"),
                    CommandWireId(RawId::new().as_uuid()),
                ),
                live.clone(),
                &transport,
            )
            .await;
        assert_eq!(
            outcome_of(&responses),
            ManagementOutcome::NeedsClarification,
            "target {target:?} clarifies"
        );
    }
    assert!(
        handle
            .pending_targeted_deletions(None, 50)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        handle
            .store
            .deletion_status(None, 10)
            .await
            .unwrap()
            .is_empty()
    );

    // One admissible request, then the durable journal probe: the management
    // intent journal stores the family and purpose only, never the body.
    let page = read_deletion_page(&handle, &live, &transport, None, None).await;
    let responses = handle
        .handle_frame(
            deletion_intent_frame(
                &live,
                "journal secret",
                &page.mark.0,
                RationaleOrigin::ManagementSurface,
                Some("journal secret in the rationale too"),
                CommandWireId(RawId::new().as_uuid()),
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert_eq!(
        outcome_of(&responses),
        ManagementOutcome::NeedsClarification
    );
    let conn = rusqlite::Connection::open(dir.path().join("app.db"))
        .expect("the journal must open for the probe");
    let leaked: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM management_intent WHERE target LIKE '%journal secret%' OR COALESCE(rationale_quote,'') LIKE '%journal secret%'",
            (),
            |row| row.get(0),
        )
        .expect("the journal probe must run");
    assert_eq!(leaked, 0, "the intent journal never keeps the exact text");
}

/// The status read is bounded and typed: limit and cursor misuse are typed
/// rejections, and paging walks operations once.
#[tokio::test]
async fn deletion_status_pages_are_bounded_and_reject_malformed_queries() {
    use ene_preservation::{PreservationRepository as _, TargetedDeletionRequest};
    use ene_primitive::RawId;

    let Some((handle, _dir)) = memory_handle("management-deletion-status").await else {
        panic!("the handle must open");
    };
    let live = paired_input("management-deletion-status");
    let transport = fake_transport();

    // Two operations from the production path (two distinct scopes).
    for text in ["first secret", "second secret"] {
        let page = read_deletion_page(&handle, &live, &transport, None, None).await;
        let responses = handle
            .handle_frame(
                deletion_intent_frame(
                    &live,
                    text,
                    &page.mark.0,
                    RationaleOrigin::ManagementSurface,
                    None,
                    CommandWireId(RawId::new().as_uuid()),
                ),
                live.clone(),
                &transport,
            )
            .await;
        assert_eq!(
            outcome_of(&responses),
            ManagementOutcome::NeedsClarification
        );
        let pending: Vec<TargetedDeletionRequest> =
            handle.pending_targeted_deletions(None, 50).await.unwrap();
        let request = pending
            .iter()
            .find(|request| request.owner_review_text() == text)
            .expect("the staged request must be listed")
            .request();
        handle
            .confirm_targeted_deletion(&request.as_raw().as_uuid().as_hyphenated().to_string())
            .await
            .unwrap();
    }

    // A page is limit-bounded and hands out a Host-issued cursor for the next.
    let first = read_deletion_page(&handle, &live, &transport, None, Some(1)).await;
    assert_eq!(first.operations.len(), 1);
    let cursor = first
        .next_cursor
        .as_ref()
        .expect("a full page hands out the next cursor");
    let second = read_deletion_page(&handle, &live, &transport, Some(&cursor.0), Some(1)).await;
    assert_eq!(second.operations.len(), 1);
    assert_ne!(
        first.operations[0].operation.0, second.operations[0].operation.0,
        "the pages cover distinct operations"
    );
    // The full-page convention (IPC §18.2) may hand out one more cursor after a
    // page that happened to be full; the following page is empty, never a
    // fabricated row.
    if let Some(next) = second.next_cursor.as_ref() {
        let third = read_deletion_page(&handle, &live, &transport, Some(&next.0), Some(1)).await;
        assert!(third.operations.is_empty());
        assert!(third.next_cursor.is_none());
    }

    // Malformed limits and cursors are typed rejections, never empty pages.
    for (cursor, limit) in [
        (None, Some(0)),
        (None, Some(51)),
        (Some("not-a-cursor"), None),
        (Some("deletion-status:not-a-uuid"), None),
    ] {
        let responses = handle
            .handle_frame(
                deletion_status_frame(&live, cursor, limit),
                live.clone(),
                &transport,
            )
            .await;
        assert!(
            responses.first().is_some_and(|frame| matches!(
                &frame.payload,
                WirePayload::Reject(notice) if notice.kind == RejectKind::UnsupportedFieldValue
            )),
            "({cursor:?}, {limit:?}) must reject, got {responses:?}"
        );
    }

    // A held operation stays visible with its hold class (lifecycle §5.1).
    let operations = handle.store.deletion_status(None, 10).await.unwrap();
    let held = handle
        .store
        .change_deletion_lifecycle(
            operations[0].current,
            ene_preservation::DeletionLifecycleChange::Hold,
        )
        .await
        .unwrap();
    assert!(matches!(
        held,
        ene_preservation::DeletionLifecycleOutcome::Applied(_)
    ));
    let page = read_deletion_page(&handle, &live, &transport, None, None).await;
    let view = page
        .operations
        .iter()
        .find(|view| {
            view.operation.0
                == operations[0]
                    .current
                    .operation
                    .as_raw()
                    .as_uuid()
                    .as_hyphenated()
                    .to_string()
        })
        .expect("the held operation stays visible");
    assert_eq!(view.phase, DeletionPhaseWire::Held);
    assert_eq!(
        view.hold,
        Some(ene_api::v1::deletion::DeletionHoldWire::Unavailable)
    );
}

/// The serving startup sequence re-evaluates reservations orphaned by a
/// crash: the ticket settles `CommittedUnknown` and the reserved upper bound
/// keeps occupying the cap, never released and never reset to zero.
#[tokio::test]
async fn startup_reconciliation_settles_orphaned_usage_reservations() {
    use ene_inference::{InferenceAttempt, InferenceAttemptRepository as _, UsageRepository as _};
    use ene_permission::{
        CapabilityKind, ConsentCommitOutcome, ConsentRecord, ConsentRevision, IntentFingerprint,
        IntentOutcomeRepository as _, IntentResolution, SetUsageCapCommand, SetUsageCapOutcome,
        UsageCapRepository as _, UsageCapScope, UsageCapWindow, UsageReservationState,
    };
    use ene_primitive::{Money, RawId, WallClockWithTz};

    let (handle, _dir) = memory_handle("usage-cap-recovery")
        .await
        .expect("the memory handle must open");
    let store = &handle.store;
    // Seed the dialogue consent through the production intent-atomic path.
    let fingerprint = IntentFingerprint {
        intent_id: RawId::new().as_uuid().to_string(),
        kind: String::from("assign"),
        target: String::from("consent:usage-cap-recovery"),
        base: String::from("consent-none"),
        rationale_origin: String::from("management-surface"),
        rationale_quote: None,
    };
    let committed = store
        .assign_with_intent(
            None,
            ConsentRecord {
                capability: CapabilityKind::Dialogue,
                id: String::from("usage-cap-consent"),
                rev: ConsentRevision::from_u64(1),
                provider: String::from("openai"),
                model: String::from("gpt-4o"),
                credential_id: String::from("openai:main"),
            },
            fingerprint,
        )
        .await
        .expect("the consent write must answer");
    assert!(
        matches!(
            committed,
            IntentResolution::Decided(ConsentCommitOutcome::Committed { .. })
        ),
        "the consent must commit, got {committed:?}"
    );
    let cap = store
        .set_usage_cap(SetUsageCapCommand {
            expected: None,
            scope: UsageCapScope::System,
            window: UsageCapWindow::DailyUtc,
            limit: Money::from_micros(ene_primitive::CurrencyCode::Usd, 100_000),
        })
        .await
        .expect("the cap write must answer");
    assert!(
        matches!(cap, SetUsageCapOutcome::StoredAs(_)),
        "the cap must store, got {cap:?}"
    );
    let ene_inference::pricing::PricingResolution::Priced(pricing) =
        ene_inference::pricing::PricingCatalog::first_party()
            .expect("the reviewed catalog must be valid")
            .resolve("openai", "gpt-4o", WallClockWithTz::now())
    else {
        panic!("the reviewed route must be priced");
    };
    let ticket = ene_inference::InferenceTicketId(RawId::new());
    let claim = store
        .begin_inference_attempt(InferenceAttempt {
            ticket,
            consumer: ene_permission::ConsumerKind::CompanionDialogue,
            capability: CapabilityKind::Dialogue,
            purpose: ene_permission::PurposeKind::DialogueResponse,
            expected_consent: (
                String::from("usage-cap-consent"),
                ConsentRevision::from_u64(1),
            ),
            expected_credential_set: ene_credential::CredentialSetRevision::initial(),
            provider: String::from("openai"),
            model: String::from("gpt-4o"),
            task_agent: None,
            pricing: Some(pricing),
            usage_estimate: Some(ene_inference::cost::UsageEstimate {
                input_tokens_upper_bound: 100,
                output_tokens_upper_bound: 50,
            }),
        })
        .await
        .expect("the claim must answer");
    assert_eq!(
        claim,
        ene_inference::AttemptBeginOutcome::Started,
        "the claim must reserve under the configured cap"
    );
    // The crash leaves the reservation non-terminal; startup must settle it.
    handle
        .run_startup_mutations()
        .await
        .expect("startup mutations must complete");
    let reservation = store
        .load_usage_reservation(ticket)
        .await
        .expect("the reservation read must answer")
        .expect("the reservation stays durable");
    assert_eq!(reservation.state, UsageReservationState::CommittedUnknown);
    let cost = store
        .load_usage_cost(ticket)
        .await
        .expect("the settlement read must answer")
        .expect("startup recovery records the unknown usage fact");
    assert_eq!(
        cost.usage.source,
        ene_inference::UsageSource::Unknown,
        "an orphaned reservation settles unknown token usage, never zero"
    );
    // A second startup is idempotent: the terminal state is not re-settled.
    handle
        .run_startup_mutations()
        .await
        .expect("startup mutations must stay idempotent");
    let again = store
        .load_usage_reservation(ticket)
        .await
        .expect("the reservation read must answer")
        .expect("the reservation stays durable");
    assert_eq!(again.state, UsageReservationState::CommittedUnknown);
}
