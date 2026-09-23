use std::sync::Arc;

use super::{CurrentConnection, HostHandle, LiveInput, device_client};
use crate::conn::{ConnectionPhase, ConnectionTable, LiveDecision};
use crate::test_support::{live_input, memory_handle};
use ene_api::v1::deletion::{
    DeletionParticipantReportWire, DeletionPhaseWire, DeletionPurposeWire, DeletionStatusPage,
    DeletionStatusRequest, DeletionStatusResponse, deletion_target,
};
use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
use ene_api::v1::handshake::{
    AuthProof, AuthResult, CapabilityAdvertise, PairingProvision, PairingRequest, PairingResult,
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
use ene_api::v1::round::{SubmitTextInput, TextBodyWire};
use ene_companion::CompanionRepository as _;
use ene_credential::pairing_proof_hex;
use ene_inference::fake::FakeProviderTransport;
use ene_presence::{
    ConfirmTransitionOutcome, LiveReachabilityRef, MoveDecision, PresenceCheckRef,
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

/// Builds a paired [`LiveInput`] with an authenticated table-backed
/// connection for `device_wire`.
fn paired_input(device_wire: &str) -> LiveInput {
    live_input(device_wire)
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

fn register_pairing(
    handle: &HostHandle,
    connection: &ConnectionWireId,
) -> tokio::sync::mpsc::Receiver<PairingProvision> {
    handle
        .pairing_deliveries
        .register(connection)
        .expect("the fresh connection must have one pairing slot")
}

async fn receive_provision_and_pair(
    table: &ConnectionTable,
    connection: &ConnectionWireId,
    receiver: &mut tokio::sync::mpsc::Receiver<PairingProvision>,
) -> PairingProvision {
    let provision = receiver
        .recv()
        .await
        .expect("approval must queue one provision");
    assert!(table.note_paired(
        connection,
        &provision.device_id.0.as_hyphenated().to_string()
    ));
    provision
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
                confirmed: false,
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
    let mut provisions = register_pairing(&handle, &id);
    let request = pairing_frame("laptop");
    let expected_reply = request.envelope.message_id;
    let pending = dispatch(&handle, &table, &id, request, &transport).await;
    let answer = pending.first().expect("the request answers once");
    let pending_id = pending_id_of(answer);
    assert_eq!(answer.envelope.correlation.reply_to, Some(expected_reply));
    assert_eq!(table.phase_of(&id), Some(ConnectionPhase::Accepted));
    assert!(
        handle
            .pending_devices()
            .await
            .unwrap()
            .iter()
            .any(|entry| entry.pending_id == pending_id && entry.descriptor == "laptop")
    );
    assert!(
        handle
            .approve_device("unknown box")
            .await
            .unwrap()
            .is_none()
    );
    for response in &pending {
        assert_eq!(
            response.envelope.sender.connection_id, None,
            "a pre-accept pairing answer hides the connection id"
        );
    }
    let record = handle
        .approve_device(&pending_id)
        .await
        .unwrap()
        .expect("owner approval must pair");
    let provision = receive_provision_and_pair(&table, &id, &mut provisions).await;
    assert!(handle.approve_device(&pending_id).await.unwrap().is_none());
    let device_wire = record.wire.clone();
    let device_uuid = provision.device_id.0;
    let device_id = provision.device_id;
    let secret = provision.pairing_secret;
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
    assert_eq!(table.phase_of(&id), Some(ConnectionPhase::Paired));
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
    let proof = pairing_proof_hex(secret.expose_secret(), &nonce);
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

/// The first-party management cancel reaches the same
/// [`HostHandle::cancel_task`] admission as the conversation path, preserves
/// the typed outcome meanings the management vocabulary can carry, and
/// replays a retried intent from its durable snapshot.
#[tokio::test]
async fn management_cancel_reaches_the_cancel_admission_and_replays() {
    use ene_primitive::RawId;
    use ene_task::{
        AssigneeRef, DelegationCreationPremise, DelegationId, DelegationOutcome, DelegationScope,
        TaskAgentEphemeralId, TaskContextEntryId, TaskContextOrigin, TaskContextOriginKind,
        TaskCreationPremise, TaskId, TaskProgress, TaskPurpose, TaskRepository as _,
        TaskResultAcceptance, TaskResultAdoptionClaim,
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
    let arrival = crate::test_support::record_result(&handle.store, delegation, "done").await;
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
    let mut provisions = register_pairing(&handle, &probe_id);
    let pending = dispatch(
        &handle,
        &probe_table,
        &probe_id,
        pairing_frame("laptop"),
        &transport,
    )
    .await;
    let probe_pending = pending_id_of(pending.first().expect("the probe must pend"));
    let record = handle
        .approve_device(&probe_pending)
        .await
        .unwrap()
        .expect("owner approval must pair");
    let provision = receive_provision_and_pair(&probe_table, &probe_id, &mut provisions).await;
    let secret = provision.pairing_secret;
    assert_eq!(
        record.wire,
        provision.device_id.0.as_hyphenated().to_string()
    );
    let device = provision.device_id;

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
        proof_frame(
            device,
            &pairing_proof_hex(secret.expose_secret(), &challenge.nonce),
        ),
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
        proof_frame(
            device,
            &pairing_proof_hex(secret.expose_secret(), &challenge.nonce),
        ),
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
/// canonical operation starts and is driven immediately, and the bounded
/// status view reports it.
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

    // The Host-local trusted confirmation is the only destructive path, and
    // it drives the operation immediately: with no target-bearing data the
    // bounded fan-out verifies every required owner and the sealed boundary
    // commits, closing the current erasure condition.
    let request_text = request.as_raw().as_uuid().as_hyphenated().to_string();
    let ConfirmTargetedDeletionOutcome::Started(current) = handle
        .confirm_targeted_deletion(&request_text)
        .await
        .unwrap()
    else {
        panic!("the Owner confirmation must start the canonical operation");
    };
    assert!(
        handle
            .store
            .current_erasure_conditions(None, 10)
            .await
            .unwrap()
            .is_empty(),
        "the confirmation drive completes an empty sweep and closes the condition"
    );
    assert!(
        handle
            .store
            .deletion_completion_audit(current.operation)
            .await
            .unwrap()
            .is_some(),
        "the driven sweep commits the body-free audit"
    );
    assert!(
        handle
            .pending_targeted_deletions(None, 50)
            .await
            .unwrap()
            .is_empty(),
        "a confirmed request leaves the pending set"
    );

    // The bounded status view reports the operation without any body; the
    // drive is observable as the durable participant verifications.
    let page = read_deletion_page(&handle, &live, &transport, None, None).await;
    assert_eq!(page.operations.len(), 1);
    let view = &page.operations[0];
    assert_eq!(view.phase, DeletionPhaseWire::Completed);
    assert_eq!(view.sweep, 1);
    assert_eq!(view.purpose, DeletionPurposeWire::Privacy);
    let DeletionParticipantReportWire::Reported(participants) = &view.participants else {
        panic!("a driven operation must report its durable participant snapshot");
    };
    assert!(
        !participants.is_empty(),
        "the required snapshot is non-empty from admission"
    );
    assert!(
        participants
            .iter()
            .all(|participant| participant.progress == "verified" && participant.sweep == 1),
        "the confirmation drive verifies every required participant: {participants:?}"
    );
    assert!(
        !format!("{view:?}").contains("leaked key"),
        "the status view never carries the target"
    );

    // A fresh conversation-origin intent after completion is a new origin,
    // but model wording alone cannot confirm it or start a second operation.
    let fresh = handle
        .handle_frame(
            deletion_intent_frame(
                &live,
                "leaked key",
                &page.mark.0,
                RationaleOrigin::Conversation,
                Some("the model suggested deleting this"),
                CommandWireId(RawId::new().as_uuid()),
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert_eq!(outcome_of(&fresh), ManagementOutcome::NeedsClarification);
    assert_eq!(
        handle.store.deletion_status(None, 10).await.unwrap().len(),
        1
    );
    assert_eq!(
        handle
            .pending_targeted_deletions(None, 50)
            .await
            .unwrap()
            .len(),
        1,
        "the untrusted conversation intent remains pending"
    );

    // A duplicate confirmation observes the same single operation; the
    // completed condition stays closed.
    assert_eq!(
        handle
            .confirm_targeted_deletion(&request_text)
            .await
            .unwrap(),
        ConfirmTargetedDeletionOutcome::AlreadyCoveredBy(current)
    );
    assert!(
        handle
            .store
            .current_erasure_conditions(None, 100)
            .await
            .unwrap()
            .is_empty()
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
            data_use: Vec::new(),
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

mod usage_tests;
