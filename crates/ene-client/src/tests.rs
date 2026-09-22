//! In-memory only: no sockets are opened and the environment is never
//! mutated; frames go through the in-memory codec or plain in-memory scripts.

use ene_api::v1::envelope::{ProtocolVersion, WireSender};
use ene_api::v1::handshake::{AuthResult, PairingProvisionSecret};
use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, RationaleOrigin, credential_target,
};
use ene_api::v1::payload::{BodyStateHint, WirePayload};
use ene_api::v1::presence::{PresenceAttributionWire, PresenceStateWire};
use ene_api::v1::refs::{BaseViewMark, CommandWireId, WireMessageId};
use ene_api::v1::refs::{
    ClientIncarnationId, ClientLocalId, CompanionWireRef, RoundWireId, TextLangWire,
};
use ene_api::v1::round::{HistoryRequest, SubmitTextInput, TextBodyWire};
use ene_plugin_ipc::WireFrame;

use super::frames::{
    PreparedRequest, auth_rejected_guidance, capability_frame, frame_for, frame_for_session,
    missing_secret_guidance, pairing_frame, proof_frame,
};
use super::session::{
    AuthDecision, DEFERRED_CAP, PENDING_ERASURE_CAP, SessionState, decide_auth, stale_generation_of,
};
use super::{platform_display, socket_path};

fn incarnation() -> ClientIncarnationId {
    ClientIncarnationId {
        counter: 0,
        random: 7,
    }
}

fn history_request(companion: &str, limit: u64) -> HistoryRequest {
    HistoryRequest {
        companion: CompanionWireRef(companion.to_string()),
        since: None,
        limit,
        round: None,
    }
}

fn submit_input(
    companion: &str,
    round: Option<String>,
    fresh: bool,
    text: String,
    lang: String,
) -> SubmitTextInput {
    SubmitTextInput {
        companion: CompanionWireRef(companion.to_string()),
        round: round.map(RoundWireId),
        fresh,
        local_id: ClientLocalId(String::from("local-1")),
        body: TextBodyWire {
            text,
            lang: TextLangWire(lang),
        },
    }
}

fn credential_intent(
    intent_id: CommandWireId,
    base: &BaseViewMark,
    provider: &str,
) -> ManagementIntent {
    ManagementIntent {
        intent_id,
        kind: ManagementIntentKind::ConfigureCredentialIntent,
        target: credential_target(provider, "main"),
        base_view: base.clone(),
        rationale: IntentRationaleWire {
            origin: RationaleOrigin::ManagementSurface,
            quote: None,
        },
        confirmed: false,
    }
}

#[test]
fn socket_path_appends_ene_sock() {
    let dir = std::path::Path::new("/tmp/ene-data");
    assert!(
        socket_path(dir) == dir.join("ene.sock"),
        "socket path must be ene.sock under the data dir"
    );
}

#[test]
fn platform_display_names_os_and_arch() {
    let display = platform_display();
    assert!(
        display == format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        "platform display must name OS and arch, got {display:?}"
    );
}

#[test]
fn pairing_frame_is_pre_pairing_v1() -> Result<(), String> {
    let frame = pairing_frame("Owner laptop", incarnation());
    let WirePayload::PairingRequest(request) = &frame.payload else {
        return Err(String::from("pairing builder must emit PairingRequest"));
    };
    assert!(
        request.device_descriptor == "Owner laptop",
        "pairing keeps the display descriptor"
    );
    assert!(
        frame.envelope.protocol == ProtocolVersion::V1,
        "pairing speaks V1"
    );
    assert!(
        frame.envelope.sender.device_id.is_none(),
        "pre-pairing sender carries no device ID"
    );
    assert!(
        frame.envelope.correlation.request_id.is_some(),
        "pairing carries a Client-minted request id for retry correlation"
    );
    assert!(
        frame.envelope.message_type.0 == "PairingRequest",
        "pairing names its payload shape"
    );
    Ok(())
}

#[test]
fn capability_frame_speaks_v1_and_threads_device() -> Result<(), String> {
    let sender_device = ene_api::v1::refs::DeviceWireId(uuid::Uuid::new_v4());
    let frame = capability_frame("linux-x86_64", incarnation(), Some(sender_device));
    let WirePayload::CapabilityAdvertise(advertise) = &frame.payload else {
        return Err(String::from(
            "capability builder must emit CapabilityAdvertise",
        ));
    };
    assert!(
        advertise.supported_protocol == vec![ProtocolVersion::V1],
        "capability speaks V1"
    );
    assert!(
        advertise.platform == "linux-x86_64",
        "capability carries the display platform"
    );
    assert!(
        frame.envelope.sender.device_id == Some(sender_device),
        "capability threads the paired device ID"
    );
    assert!(
        frame.envelope.message_type.0 == "CapabilityAdvertise",
        "capability names its payload shape"
    );
    Ok(())
}

#[test]
fn incompatible_protocol_refusal_names_both_maxima_and_hint() {
    use crate::ClientError;
    use crate::transport::incompatible_protocol_error;
    use ene_api::v1::reject::IncompatibleProtocol;

    let error = incompatible_protocol_error(&IncompatibleProtocol {
        host_max: ProtocolVersion::V1,
        client_max: ProtocolVersion { major: 9, minor: 3 },
        hint: String::from("use a client release sharing the host's protocol major 1"),
    });
    let ClientError::ServerRejected(message) = error else {
        panic!("a major mismatch is a terminal refusal, got {error:?}");
    };
    for expected in ["host max 1.0", "client max 9.3", "protocol major 1"] {
        assert!(
            message.contains(expected),
            "the refusal must carry {expected:?}: {message}"
        );
    }
}

#[test]
fn message_type_names_the_variant() {
    let sender = WireSender {
        device_id: None,
        incarnation_id: incarnation(),
        connection_id: None,
    };
    let frame = frame_for(
        WirePayload::HistoryRequest(history_request("companion-1", 3)),
        sender,
    );
    assert!(
        frame.envelope.message_type.0 == "HistoryRequest",
        "discriminator must name the variant"
    );
    assert!(
        frame.payload.message_type() == "HistoryRequest",
        "kind name must match the discriminator"
    );
    assert!(
        frame.envelope.protocol == ProtocolVersion::V1,
        "built frames speak V1"
    );
}

fn presence_fact(generation: u64) -> PresenceAttributionWire {
    PresenceAttributionWire {
        companion: CompanionWireRef(String::from("default")),
        state: PresenceStateWire::Present,
        active_client: None,
        generation,
    }
}

fn stale_answer(current_generation: u64) -> WirePayload {
    WirePayload::RoundIntakeOutcome(ene_api::v1::round::RoundIntakeOutcomeWire::StaleRound {
        current_round: None,
        current_generation,
    })
}

#[test]
fn session_starts_unobserved_and_tracks_latest() {
    let mut session = SessionState::default();
    assert!(
        session.generation().is_none(),
        "a new session observed nothing yet"
    );
    session.observe_presence(&presence_fact(4));
    assert!(
        session.generation() == Some(4),
        "the fact generation becomes current"
    );
    session.observe_presence(&presence_fact(7));
    assert!(
        session.generation() == Some(7),
        "a newer fact supersedes: {:?}",
        session.generation()
    );
    session.note_stale_generation(9);
    assert!(
        session.generation() == Some(9),
        "a stale answer refreshes the running session"
    );
}

#[test]
fn local_erasure_demand_wipes_the_deferred_buffer_and_reports_classes() {
    use ene_api::v1::deletion::{
        ClientTempClass, DeletionDemand, DeletionDemandWireId, DeletionTargetWire,
        LocalErasureResult,
    };

    let mut session = SessionState::default();
    let response_frame = || {
        crate::frames::frame_for(
            WirePayload::UndeliveredResponse(
                ene_api::v1::undelivered::UndeliveredResponse::FrameTooLarge,
            ),
            WireSender {
                device_id: None,
                incarnation_id: incarnation(),
                connection_id: None,
            },
        )
    };
    session.push_deferred(response_frame());
    assert_eq!(
        session.take_undelivered().len(),
        1,
        "the deferred queue holds the response before the demand"
    );
    session.push_deferred(response_frame());
    let drained = session.wipe_transient();
    assert_eq!(
        drained,
        vec![
            ClientTempClass::PresentationBuffer,
            ClientTempClass::InputDraft
        ]
    );
    assert!(
        session.take_undelivered().is_empty(),
        "the deferred presentation buffer is dropped whole"
    );

    let demand = DeletionDemand {
        demand: DeletionDemandWireId(String::from("demand-1")),
        operation: ene_api::v1::refs::DeletionOperationWireRef(String::from("operation-1")),
        sweep: 3,
        targets: vec![DeletionTargetWire::WipeClass {
            class: ClientTempClass::PresentationBuffer,
        }],
    };
    let result = LocalErasureResult {
        demand: demand.demand.clone(),
        operation: demand.operation.clone(),
        sweep: demand.sweep,
        wiped: drained,
        unverified: Vec::new(),
    };
    let json = serde_json::to_string(&result).expect("the result must serialize");
    assert!(
        !json.to_lowercase().contains("deletion:"),
        "the wire result carries no mechanical target: {json}"
    );
}

#[test]
fn defer_erasure_stashes_the_demand_without_claiming_gui_classes() {
    use ene_api::v1::deletion::{
        ClientTempClass, DeletionDemand, DeletionDemandWireId, DeletionTargetWire,
    };

    let mut session = SessionState::default();
    session.set_defer_erasure(true);
    session.push_deferred(crate::frames::frame_for(
        WirePayload::PresenceAttribution(presence_fact(1)),
        WireSender {
            device_id: None,
            incarnation_id: incarnation(),
            connection_id: None,
        },
    ));
    let demand = DeletionDemand {
        demand: DeletionDemandWireId(String::from("demand-gui")),
        operation: ene_api::v1::refs::DeletionOperationWireRef(String::from("operation-gui")),
        sweep: 1,
        targets: vec![DeletionTargetWire::WipeClass {
            class: ClientTempClass::PresentationBuffer,
        }],
    };
    session.clear_deferred_frames();
    session.push_pending_erasure(demand.clone());
    let stashed = session.take_pending_erasure().expect("stashed demand");
    assert_eq!(stashed.demand, demand.demand);
    assert!(
        session.take_pending_erasure().is_none(),
        "one demand is consumed by the GUI participant"
    );
}

#[test]
fn stale_generation_of_reads_only_stale_answers() {
    assert!(
        stale_generation_of(&stale_answer(21)) == Some(21),
        "a stale answer yields its current generation"
    );
    let accepted = WirePayload::RoundIntakeOutcome(
        ene_api::v1::round::RoundIntakeOutcomeWire::AcceptedForRound {
            round: RoundWireId(String::from("round-1")),
        },
    );
    assert!(
        stale_generation_of(&accepted).is_none(),
        "a non-stale answer yields nothing"
    );
    let history = WirePayload::HistoryRequest(history_request("companion-1", 1));
    assert!(
        stale_generation_of(&history).is_none(),
        "an unrelated payload yields nothing"
    );
}

#[test]
fn session_frames_stamp_only_text_inputs() {
    let sender = WireSender {
        device_id: None,
        incarnation_id: incarnation(),
        connection_id: None,
    };
    let input = WirePayload::SubmitTextInput(submit_input(
        "companion-1",
        None,
        false,
        String::from("hello"),
        String::from("en"),
    ));
    let stamped = frame_for_session(input, sender, Some(6));
    assert!(
        stamped.envelope.observed.presence_generation_view == Some(6),
        "text input carries the session generation"
    );
    let bootstrap = frame_for_session(
        WirePayload::SubmitTextInput(submit_input(
            "companion-1",
            None,
            false,
            String::from("hello"),
            String::from("en"),
        )),
        sender,
        None,
    );
    assert!(
        bootstrap
            .envelope
            .observed
            .presence_generation_view
            .is_none(),
        "pre-fact bootstrap stamps None (NeedsRevalidation is correct)"
    );
    let history = frame_for_session(
        WirePayload::HistoryRequest(history_request("companion-1", 1)),
        sender,
        Some(6),
    );
    assert!(
        history.envelope.observed.presence_generation_view.is_none(),
        "non-input payloads keep the None default"
    );
}

/// The boot cache is process-global; this serializes the boot tests so a
/// reset in one never clears another's cached boot mid-assertion (nextest
/// already isolates tests per process, this covers the shared-process runner
/// too).
static BOOT_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn boot_incarnation_is_one_per_process_and_advances_per_boot() {
    use crate::incarnation::{advance_counter, boot_incarnation, counter_path, reset_for_tests};

    let _guard = BOOT_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ene-ctl-incarnation-{}-{name}", std::process::id(),))
    }

    fn remove_dir(dir: &std::path::Path) {
        if std::fs::remove_dir_all(dir).is_err() {
            // Best effort.
        }
    }

    let dir = scratch("boot");
    if std::fs::remove_dir_all(&dir).is_err() {
        // Absent is the expected case; leftovers from a failed run clear here.
    }
    let created = std::fs::create_dir_all(&dir);
    assert!(created.is_ok(), "scratch dir must create: {created:?}");
    reset_for_tests();
    let first = boot_incarnation(&dir);
    assert!(first.is_ok(), "first boot must succeed: {first:?}");
    let first = first.unwrap_or_else(|_| panic!("first boot must succeed"));
    assert!(
        first.counter == 1,
        "first published counter is 1, got {first:?}"
    );
    assert!(
        first.random <= i64::MAX as u64,
        "random stays in the non-negative SQLite INTEGER range history stores, got {first:?}"
    );
    let second = boot_incarnation(&dir);
    assert!(second.is_ok(), "second boot must succeed: {second:?}");
    assert!(
        second.unwrap_or_else(|_| panic!("second boot must succeed")) == first,
        "same-process reconnect must reuse the boot incarnation"
    );
    let stored = std::fs::read_to_string(counter_path(&dir)).unwrap_or_default();
    assert!(
        stored.trim() == "1",
        "reconnect must not advance the counter file, got {stored:?}"
    );
    reset_for_tests();
    let third = boot_incarnation(&dir);
    assert!(third.is_ok(), "post-restart boot must succeed");
    let third = third.unwrap_or_else(|_| panic!("post-restart boot must succeed"));
    assert!(
        third.counter == 2,
        "restart must publish the next counter, got {third:?}"
    );
    assert!(
        third.random != first.random || third.counter != first.counter,
        "restart must not repeat the boot identity: {first:?} vs {third:?}"
    );
    reset_for_tests();
    let advanced = advance_counter(&dir);
    assert!(
        matches!(advanced, Ok(3)),
        "direct advance publishes the next counter, got {advanced:?}"
    );
    remove_dir(&dir);
    reset_for_tests();
}

#[test]
fn boot_incarnation_fails_closed_on_corrupt_or_exhausted_counters() {
    use crate::incarnation::{boot_incarnation, counter_path, reset_for_tests};

    let _guard = BOOT_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let dir = std::env::temp_dir().join(format!(
        "ene-ctl-incarnation-corrupt-{}",
        std::process::id(),
    ));
    assert!(std::fs::create_dir_all(&dir).is_ok());
    for (name, bytes) in [
        ("empty", b"".as_slice()),
        ("blank", b"   \n".as_slice()),
        ("alpha", b"not-a-number".as_slice()),
        ("negative", b"-3".as_slice()),
        ("trailing", b"12x".as_slice()),
        ("binary", &[0xff, 0x00, 0x31]),
    ] {
        assert!(
            std::fs::write(counter_path(&dir), bytes).is_ok(),
            "{name} fixture must write"
        );
        reset_for_tests();
        assert!(
            boot_incarnation(&dir).is_err(),
            "{name} counter must fail closed, never re-initialized"
        );
        // Fail-closed leaves the corrupt bytes untouched for the operator.
        assert!(
            std::fs::read(counter_path(&dir)).unwrap_or_default() == bytes,
            "{name} failure must not rewrite the counter"
        );
    }
    assert!(
        std::fs::write(counter_path(&dir), u64::MAX.to_string()).is_ok(),
        "the exhausted fixture must write"
    );
    reset_for_tests();
    assert!(
        boot_incarnation(&dir).is_err(),
        "an exhausted counter must fail closed, never wrap"
    );
    assert!(std::fs::remove_dir_all(&dir).is_ok() || !dir.exists());
    reset_for_tests();
}

#[test]
fn concurrent_boot_advances_serialize_without_loss() {
    use crate::incarnation::{advance_counter, counter_path, reset_for_tests};

    let _guard = BOOT_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let dir = std::env::temp_dir().join(format!(
        "ene-ctl-incarnation-concurrent-{}",
        std::process::id(),
    ));
    if std::fs::remove_dir_all(&dir).is_err() {
        // Absent is the expected case.
    }
    assert!(std::fs::create_dir_all(&dir).is_ok());
    reset_for_tests();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let dir = dir.clone();
        let barrier = std::sync::Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            advance_counter(&dir)
        }));
    }
    let mut published = Vec::new();
    for worker in workers {
        let next = worker.join().expect("boot worker must not panic");
        published.push(next.expect("concurrent advance must succeed"));
    }
    published.sort_unstable();
    assert!(
        published == vec![1, 2, 3, 4, 5, 6, 7, 8],
        "concurrent boots must serialize to distinct counters, got {published:?}"
    );
    let stored = std::fs::read_to_string(counter_path(&dir)).unwrap_or_default();
    assert!(
        stored.trim() == "8",
        "every advance must land exactly once, got {stored:?}"
    );
    assert!(std::fs::remove_dir_all(&dir).is_ok() || !dir.exists());
    reset_for_tests();
}

fn script_frame(
    payload: WirePayload,
    message_id: WireMessageId,
    reply_to: Option<WireMessageId>,
) -> WireFrame {
    let mut frame = frame_for(
        payload,
        WireSender {
            device_id: None,
            incarnation_id: incarnation(),
            connection_id: None,
        },
    );
    frame.envelope.message_id = message_id;
    frame.envelope.correlation.reply_to = reply_to;
    frame
}

fn message_id(value: u128) -> WireMessageId {
    WireMessageId(uuid::Uuid::from_u128(value))
}

fn answer_payload() -> WirePayload {
    WirePayload::HistoryRequest(history_request("companion-1", 1))
}

#[test]
fn prepare_keeps_command_identity_and_leaves_requests_unstamped() -> Result<(), String> {
    let sender = WireSender {
        device_id: None,
        incarnation_id: incarnation(),
        connection_id: None,
    };
    // A pure request/response payload pairs by `request_id` only.
    let request = PreparedRequest::new(answer_payload());
    let request_frame = request.frame(sender, Some(6));
    assert!(
        request_frame.envelope.correlation.command_id.is_none(),
        "a pure request must carry no command identity"
    );
    assert!(
        request_frame.envelope.correlation.request_id.is_some(),
        "a pure request still pairs its response with a fresh request ID"
    );
    // A management intent keeps one canonical identity: the envelope reuses
    // the payload's `intent_id`, never a second minted command ID.
    let intent_id = CommandWireId(uuid::Uuid::new_v4());
    let intent = credential_intent(intent_id, &BaseViewMark(String::from("mark-1")), "openai");
    let prepared_intent = PreparedRequest::new(WirePayload::ManagementIntent(intent));
    let intent_frame = prepared_intent.frame(sender, None);
    assert_eq!(
        intent_frame.envelope.correlation.command_id,
        Some(intent_id),
        "the envelope command ID is the payload's intent ID, not a second one"
    );
    let WirePayload::ManagementIntent(carried) = &intent_frame.payload else {
        return Err(String::from("intent preparation must keep the payload"));
    };
    assert_eq!(
        carried.intent_id, intent_id,
        "the envelope and the payload carry one identity"
    );
    // A text input mints a fresh command ID per prepared send.
    let submit = || {
        WirePayload::SubmitTextInput(submit_input(
            "companion-1",
            None,
            false,
            String::from("hi"),
            String::from("en"),
        ))
    };
    let first = PreparedRequest::new(submit()).frame(sender, Some(3));
    let second = PreparedRequest::new(submit()).frame(sender, Some(3));
    let (Some(first_command), Some(second_command)) = (
        first.envelope.correlation.command_id,
        second.envelope.correlation.command_id,
    ) else {
        return Err(String::from("prepared text inputs must carry command ids"));
    };
    assert!(
        first_command != second_command,
        "every prepared command mints a fresh command ID: {first_command:?} vs {second_command:?}"
    );
    assert!(
        first.envelope.correlation.request_id.is_some(),
        "every send carries a request ID for pairing"
    );
    Ok(())
}

#[test]
fn session_echoes_the_learned_companion_projection() {
    use super::session::SessionState;

    let mut state = SessionState::default();
    assert_eq!(
        state.companion_ref(),
        String::from(crate::DEFAULT_COMPANION_REF),
        "bootstrap echoes the fallback until the first fact"
    );
    let mut fact = presence_fact(3);
    fact.companion = CompanionWireRef(String::from("host-issued-projection"));
    state.observe_presence(&fact);
    assert_eq!(
        state.companion_ref(),
        String::from("host-issued-projection"),
        "after presence the session echoes the learned projection"
    );
    assert_eq!(
        state.generation(),
        Some(3),
        "generation bookkeeping is untouched"
    );
}

#[test]
fn prepared_retry_reuses_command_with_fresh_transport_ids() {
    let sender = WireSender {
        device_id: None,
        incarnation_id: incarnation(),
        connection_id: None,
    };
    let input = || {
        WirePayload::SubmitTextInput(submit_input(
            "companion-1",
            None,
            false,
            String::from("hi"),
            String::from("en"),
        ))
    };
    let prepared = PreparedRequest::new(input());
    let first = prepared.frame(sender, Some(3));
    let Some(command) = first.envelope.correlation.command_id else {
        panic!("a prepared text input must carry a command identity");
    };
    let second = prepared.frame(sender, Some(3));
    assert_eq!(
        second.envelope.correlation.command_id,
        Some(command),
        "retry reuses the logical command ID"
    );
    assert!(
        first.envelope.message_id != second.envelope.message_id,
        "retries pair transport-fresh"
    );
    assert!(
        first.envelope.correlation.request_id != second.envelope.correlation.request_id,
        "retries mint a fresh request ID per attempt"
    );
    assert_eq!(
        first.envelope.observed.presence_generation_view,
        second.envelope.observed.presence_generation_view,
        "retries preserve the observed premise"
    );
}

#[test]
fn decide_frame_classifies_facts_answers_and_deferrals() {
    use super::session::{FrameDecision, decide_frame};

    let own = message_id(1);
    let fact = script_frame(
        WirePayload::PresenceAttribution(presence_fact(3)),
        message_id(2),
        Some(own),
    );
    assert!(
        matches!(decide_frame(own, &fact), FrameDecision::AbsorbPresence(_)),
        "facts absorb"
    );
    let answer = script_frame(answer_payload(), message_id(3), Some(own));
    assert!(
        decide_frame(own, &answer) == FrameDecision::Answer(answer_payload()),
        "a reply_to match answers"
    );
    let stranger = script_frame(answer_payload(), message_id(4), Some(message_id(9)));
    assert!(
        decide_frame(own, &stranger) == FrameDecision::Defer,
        "anything else defers"
    );
    let hint = script_frame(
        WirePayload::BodyStateHint(BodyStateHint {
            asset_ref: String::from("bundled:ene"),
            pose_hint: String::from("idle"),
        }),
        message_id(5),
        Some(own),
    );
    assert!(
        matches!(decide_frame(own, &hint), FrameDecision::AbsorbBodyHint(_)),
        "BodyStateHint is a fact, never an answer, even with matching reply_to"
    );
}

/// Carries `limit` so out-of-order answers stay distinguishable by payload.
fn history_answer(limit: u64) -> WirePayload {
    WirePayload::HistoryRequest(history_request("companion-1", limit))
}

#[test]
fn deferred_queue_drops_the_oldest_frame_at_capacity() {
    let mut session = SessionState::default();
    // The oldest frame is distinguishable, so its loss is observable through
    // the only drain left: `take_undelivered`.
    session.push_deferred(script_frame(
        WirePayload::UndeliveredResponse(
            ene_api::v1::undelivered::UndeliveredResponse::NoCurrentPresence,
        ),
        message_id(999),
        None,
    ));
    for index in 0..DEFERRED_CAP {
        let reply_to = u128::try_from(index).map_or(0, |value| value + 1000);
        session.push_deferred(script_frame(
            WirePayload::UndeliveredResponse(
                ene_api::v1::undelivered::UndeliveredResponse::FrameTooLarge,
            ),
            message_id(reply_to + 500),
            Some(message_id(reply_to)),
        ));
    }
    let drained = session.take_undelivered();
    assert_eq!(
        drained.len(),
        DEFERRED_CAP,
        "the queue stays at its cap, so the overflow dropped the oldest frame"
    );
    assert!(
        drained.iter().all(|frame| matches!(
            frame.payload,
            WirePayload::UndeliveredResponse(
                ene_api::v1::undelivered::UndeliveredResponse::FrameTooLarge
            )
        )),
        "the distinguishable oldest frame dropped first"
    );
}

#[test]
fn pending_erasure_queue_drops_the_oldest_demand_at_capacity() {
    use ene_api::v1::deletion::{
        ClientTempClass, DeletionDemand, DeletionDemandWireId, DeletionTargetWire,
    };

    let demand = |name: String| DeletionDemand {
        demand: DeletionDemandWireId(name),
        operation: ene_api::v1::refs::DeletionOperationWireRef(String::from("operation-1")),
        sweep: 1,
        targets: vec![DeletionTargetWire::WipeClass {
            class: ClientTempClass::PresentationBuffer,
        }],
    };
    let mut session = SessionState::default();
    for index in 0..PENDING_ERASURE_CAP {
        session.push_pending_erasure(demand(format!("demand-{index}")));
    }
    session.push_pending_erasure(demand(String::from("demand-overflow")));
    assert_eq!(
        session.take_pending_erasure().map(|item| item.demand.0),
        Some(String::from("demand-1")),
        "the oldest demand drops first"
    );
}

#[test]
fn decide_auth_rejection_guides_reprovisioning() -> Result<(), String> {
    let decision = decide_auth(&WirePayload::AuthResult(AuthResult::Rejected {
        reason: String::from("unknown proof"),
    }));
    let AuthDecision::Guidance { message } = decision else {
        return Err(String::from("rejection must guide reprovisioning"));
    };
    assert!(
        message.contains("unknown proof"),
        "guidance keeps the operational Host reason: {message:?}"
    );
    assert!(
        message.contains("fresh pairing request"),
        "guidance names the provisioning step: {message:?}"
    );
    Ok(())
}

#[test]
fn decide_auth_names_unexpected_kinds() -> Result<(), String> {
    let decision = decide_auth(&answer_payload());
    let AuthDecision::Unexpected { message } = decision else {
        return Err(String::from("foreign kinds must be unexpected"));
    };
    assert!(
        message.contains("HistoryRequest") && message.contains("AuthResult"),
        "the refusal must name both kinds: {message:?}"
    );
    Ok(())
}

#[test]
fn proof_frame_names_the_paired_device() -> Result<(), String> {
    use ene_api::v1::refs::DeviceWireId;
    let device = DeviceWireId(uuid::Uuid::new_v4());
    let frame = proof_frame("proof-hex-abc", incarnation(), device);
    let WirePayload::AuthProof(proof) = &frame.payload else {
        return Err(String::from("proof builder must emit AuthProof"));
    };
    assert!(
        proof.proof == "proof-hex-abc",
        "the proof value travels in the auth frame"
    );
    assert!(
        frame.envelope.sender.device_id == Some(device)
            && frame.envelope.sender.connection_id.is_none()
            && frame.envelope.sender.incarnation_id == incarnation(),
        "the proof names the paired device but no connection: {:?}",
        frame.envelope.sender
    );
    let rendered = format!("{frame:?}");
    assert!(
        !rendered.contains("proof-hex-abc"),
        "frame Debug must not leak the proof: {rendered:?}"
    );
    let encoded = (ene_plugin_ipc::encode_frame(&frame)).expect("encode proof frame");
    let (decoded, _consumed) =
        (ene_plugin_ipc::decode_frame(&encoded)).expect("decode proof frame");
    assert!(decoded == frame, "codec must preserve the proof frame");
    Ok(())
}

#[test]
fn proof_derives_from_the_secret_and_the_single_use_nonce() -> Result<(), String> {
    use ene_api::v1::refs::DeviceWireId;
    let proof = crate::pairing::pairing_proof_hex("pairing-secret", "nonce-1");
    let frame = proof_frame(&proof, incarnation(), DeviceWireId(uuid::Uuid::new_v4()));
    let WirePayload::AuthProof(carried) = &frame.payload else {
        return Err(String::from("proof builder must emit AuthProof"));
    };
    assert_eq!(
        carried.proof, proof,
        "the carried proof is the MAC of the secret and nonce"
    );
    assert_ne!(
        crate::pairing::pairing_proof_hex("pairing-secret", "nonce-2"),
        carried.proof,
        "the proof is bound to the single-use nonce"
    );
    Ok(())
}

#[test]
fn guidance_names_provisioning_without_secrets() {
    let missing = missing_secret_guidance();
    assert!(
        missing.contains("no pairing secret") && missing.contains("fresh pairing request"),
        "missing-secret guidance must direct approval and provisioning: {missing:?}"
    );
    let rejected = auth_rejected_guidance("unknown proof");
    assert!(
        rejected.contains("unknown proof"),
        "rejection guidance keeps the Host reason: {rejected:?}"
    );
}

#[test]
fn session_debug_redacts_the_secret() {
    let mut session = SessionState::default();
    session.set_pairing_secret(PairingProvisionSecret::new(String::from(
        "secret-hex-marker-9d4e",
    )));
    let rendered = format!("{session:?}");
    assert!(
        !rendered.contains("secret-hex-marker-9d4e"),
        "session Debug must not leak the secret: {rendered:?}"
    );
    assert!(
        rendered.contains("[redacted]"),
        "session Debug must mark the redaction: {rendered:?}"
    );
}

#[test]
fn session_debug_reports_the_queue_length_without_bodies() {
    let mut session = SessionState::default();
    session.push_deferred(script_frame(history_answer(3), message_id(71), None));
    let rendered = format!("{session:?}");
    assert!(
        rendered.contains("deferred_len"),
        "session Debug must name the queue length: {rendered:?}"
    );
    assert!(
        !rendered.contains("HistoryRequest"),
        "session Debug must not dump queued payloads: {rendered:?}"
    );
}

#[test]
fn ene_client_manifest_stays_a_client_library() {
    let manifest = include_str!("../Cargo.toml");
    for forbidden in ["ene-local-control", "ene-store", "ene-credential", "clap"] {
        assert!(
            !manifest.contains(forbidden),
            "ene-client must not depend on {forbidden}: {manifest}"
        );
    }
}
