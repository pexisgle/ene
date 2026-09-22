use ene_api::v1::envelope::WireSender;
use ene_api::v1::handshake::PairingProvisionSecret;
use ene_api::v1::payload::{BodyStateHint, WirePayload};
use ene_api::v1::presence::{PresenceAttributionWire, PresenceStateWire};
use ene_api::v1::refs::WireMessageId;
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

fn presence_fact(generation: u64) -> PresenceAttributionWire {
    PresenceAttributionWire {
        companion: CompanionWireRef(String::from("default")),
        state: PresenceStateWire::Present,
        active_client: None,
        generation,
    }
}

#[test]
fn session_starts_unobserved_and_tracks_latest() {
    let mut session = SessionState::default();
    assert!(
        session.generation().is_none(),
        "a new session observed nothing yet"
    );
    assert!(
        session.presence_state().is_none(),
        "missing presence is not Stopped"
    );
    session.observe_presence(&presence_fact(4));
    assert_eq!(
        session.presence_state(),
        Some(PresenceStateWire::Present),
        "presence state is observed, not invented"
    );
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
    for index in 0..DEFERRED_CAP {
        let reply_to = u128::try_from(index).map_or(0, |value| value + 1000);
        session.push_deferred(script_frame(
            history_answer(7),
            message_id(reply_to + 500),
            Some(message_id(reply_to)),
        ));
    }
    let overflow = message_id(9999);
    session.push_deferred(script_frame(history_answer(8), overflow, Some(overflow)));
    assert!(
        session.take_deferred_reply(message_id(1000)).is_none(),
        "the oldest frame drops first"
    );
    assert!(
        session.take_deferred_reply(overflow) == Some(history_answer(8)),
        "the overflow frame is queued"
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
    assert!(
        crate::pairing::verify_pairing_proof("pairing-secret", "nonce-1", &carried.proof),
        "the carried proof must verify against the secret and nonce"
    );
    assert!(
        !crate::pairing::verify_pairing_proof("pairing-secret", "nonce-2", &carried.proof),
        "the proof must not verify against another nonce (single-use)"
    );
    Ok(())
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
