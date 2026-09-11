//! In-memory only: no sockets are opened and the environment is never
//! mutated; frames go through the in-memory codec or plain in-memory scripts.

use std::collections::VecDeque;

use ene_api::v1::envelope::{ProtocolVersion, WireSender};
use ene_api::v1::handshake::AuthResult;
use ene_api::v1::payload::WirePayload;
use ene_api::v1::presence::{PresenceAttributionWire, PresenceStateWire};
use ene_api::v1::refs::{ClientIncarnationId, CompanionWireRef, RoundWireId};
use ene_api::v1::refs::{CommandWireId, WireMessageId};
use ene_plugin_ipc::WireFrame;

use super::frames::{
    auth_rejected_guidance, capability_frame, frame_for, frame_for_session, message_type_for,
    missing_secret_guidance, new_incarnation, pairing_frame, payload_kind, pending_guidance,
    proof_frame, retry_frame, stamp_request,
};
use super::session::{
    AuthDecision, DEFERRED_CAP, SessionState, decide_auth, presence_generation_of_fact,
    select_answer, stale_generation_of,
};
use super::{platform_display, socket_path};

/// Deterministic stand-in for a process incarnation.
fn incarnation() -> ClientIncarnationId {
    ClientIncarnationId {
        counter: 0,
        random: 7,
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
fn message_type_names_the_variant() {
    let sender = WireSender {
        device_id: None,
        incarnation_id: incarnation(),
        connection_id: None,
    };
    let frame = frame_for(
        WirePayload::HistoryRequest(crate::cmds::history_request("companion-1", 3)),
        sender,
    );
    assert!(
        message_type_for(&frame.payload).0 == "HistoryRequest",
        "discriminator must name the variant"
    );
    assert!(
        payload_kind(&frame.payload) == "HistoryRequest",
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
    let mut session = SessionState::new();
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
    let history = WirePayload::HistoryRequest(crate::cmds::history_request("companion-1", 1));
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
    let input = WirePayload::SubmitTextInput(crate::cmds::submit_input(
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
        WirePayload::SubmitTextInput(crate::cmds::submit_input(
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
        WirePayload::HistoryRequest(crate::cmds::history_request("companion-1", 1)),
        sender,
        Some(6),
    );
    assert!(
        history.envelope.observed.presence_generation_view.is_none(),
        "non-input payloads keep the None default"
    );
}

#[test]
fn incarnation_names_this_process_and_advances() {
    let first = new_incarnation();
    let second = new_incarnation();
    assert!(
        first.counter == u64::from(std::process::id()),
        "incarnation counter is this process pid: {first:?}"
    );
    assert!(
        first.random != second.random,
        "successive incarnations differ: {first:?} vs {second:?}"
    );
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

/// The payload kind never matters to selection.
fn answer_payload() -> WirePayload {
    WirePayload::HistoryRequest(crate::cmds::history_request("companion-1", 1))
}

#[test]
fn request_stamps_a_fresh_command_id_per_send() -> Result<(), String> {
    let sender = WireSender {
        device_id: None,
        incarnation_id: incarnation(),
        connection_id: None,
    };
    let mut first = frame_for(answer_payload(), sender);
    let mut second = frame_for(answer_payload(), sender);
    assert!(
        first.envelope.correlation.command_id.is_none(),
        "builders stamp no command ID by themselves"
    );
    let first_id = stamp_request(&mut first);
    stamp_request(&mut second);
    assert!(
        first_id == first.envelope.message_id,
        "the stamp reports the echoed message ID"
    );
    let (Some(first_command), Some(second_command)) = (
        first.envelope.correlation.command_id,
        second.envelope.correlation.command_id,
    ) else {
        return Err(String::from("stamped requests must carry command ids"));
    };
    assert!(
        first_command != second_command,
        "every send mints a fresh command ID: {first_command:?} vs {second_command:?}"
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

    let mut state = SessionState::new();
    assert_eq!(
        state.companion_ref(),
        String::from(crate::cmds::DEFAULT_COMPANION_REF),
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
fn retry_frame_reuses_command_with_fresh_transport_ids() {
    let sender = WireSender {
        device_id: None,
        incarnation_id: incarnation(),
        connection_id: None,
    };
    let command = CommandWireId(uuid::Uuid::new_v4());
    let input = || {
        WirePayload::SubmitTextInput(crate::cmds::submit_input(
            "companion-1",
            None,
            false,
            String::from("hi"),
            String::from("en"),
        ))
    };
    let first = retry_frame(input(), sender, Some(3), command);
    let second = retry_frame(input(), sender, Some(3), command);
    assert_eq!(
        first.envelope.correlation.command_id,
        Some(command),
        "retry reuses the logical command ID"
    );
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
        first.envelope.correlation.request_id.is_some(),
        "retries carry request IDs"
    );
    assert_eq!(
        first.envelope.observed.presence_generation_view,
        second.envelope.observed.presence_generation_view,
        "retries preserve the observed premise"
    );
}

#[test]
fn decide_frame_rules_one_frame_for_both_callers() {
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
}

#[test]
fn select_answer_returns_a_lone_correlated_answer() {
    let own = message_id(1);
    let frames = [script_frame(answer_payload(), message_id(2), Some(own))];
    let (absorbed, answer, deferred) = select_answer(own, &VecDeque::new(), &frames);
    assert!(
        absorbed.is_empty(),
        "no fact means no absorption: {absorbed:?}"
    );
    assert!(
        answer == Some(answer_payload()),
        "the correlated frame is the answer, got {answer:?}"
    );
    assert!(
        deferred.is_empty(),
        "a direct hit queues nothing: {deferred:?}"
    );
}

#[test]
fn select_answer_absorbs_facts_then_answers() {
    let own = message_id(7);
    let frames = [
        script_frame(
            WirePayload::PresenceAttribution(presence_fact(3)),
            message_id(8),
            Some(own),
        ),
        script_frame(
            WirePayload::PresenceAttribution(presence_fact(5)),
            message_id(9),
            Some(own),
        ),
        script_frame(answer_payload(), message_id(10), Some(own)),
    ];
    let (absorbed, answer, deferred) = select_answer(own, &VecDeque::new(), &frames);
    assert!(
        absorbed
            .iter()
            .map(presence_generation_of_fact)
            .collect::<Vec<u64>>()
            == vec![3, 5],
        "pipelined facts absorb in order, got {absorbed:?}"
    );
    assert!(
        answer == Some(answer_payload()),
        "the correlated non-fact ends the wait, got {answer:?}"
    );
    assert!(
        deferred.is_empty(),
        "facts and the hit queue nothing: {deferred:?}"
    );
}

#[test]
fn select_answer_without_an_answer_absorbs_only() {
    let own = message_id(11);
    let frames = [
        script_frame(
            WirePayload::PresenceAttribution(presence_fact(2)),
            message_id(12),
            Some(own),
        ),
        script_frame(
            WirePayload::PresenceAttribution(presence_fact(4)),
            message_id(13),
            None,
        ),
    ];
    let (absorbed, answer, deferred) = select_answer(own, &VecDeque::new(), &frames);
    assert!(
        absorbed.len() == 2,
        "facts absorb even without a reply link, got {absorbed:?}"
    );
    assert!(
        answer.is_none(),
        "facts alone are never an answer: {answer:?}"
    );
    assert!(
        deferred.is_empty(),
        "facts alone queue nothing: {deferred:?}"
    );
}

#[test]
fn select_answer_defers_mismatches_instead_of_answering() {
    let own = message_id(21);
    let other = message_id(22);
    for frames in [
        [script_frame(answer_payload(), message_id(23), Some(other))].as_slice(),
        [script_frame(answer_payload(), message_id(24), None)].as_slice(),
    ] {
        let (absorbed, answer, deferred) = select_answer(own, &VecDeque::new(), frames);
        assert!(
            absorbed.is_empty(),
            "a non-fact absorbs nothing: {absorbed:?}"
        );
        assert!(
            answer.is_none(),
            "an uncorrelated frame is never the answer, got {answer:?}"
        );
        assert!(
            deferred.len() == 1,
            "the mismatch is deferred, not dropped: {deferred:?}"
        );
    }
}

#[test]
fn select_answer_defers_a_mismatch_then_answers() {
    let own = message_id(31);
    let other = message_id(32);
    let frames = [
        script_frame(answer_payload(), message_id(33), Some(other)),
        script_frame(answer_payload(), message_id(34), Some(own)),
    ];
    let (absorbed, answer, deferred) = select_answer(own, &VecDeque::new(), &frames);
    assert!(
        absorbed.is_empty(),
        "no fact means no absorption: {absorbed:?}"
    );
    assert!(
        answer == Some(answer_payload()),
        "the correlated frame answers after the mismatch, got {answer:?}"
    );
    assert!(
        deferred.len() == 1,
        "the mismatch stays deferred: {deferred:?}"
    );
    assert!(
        deferred[0].envelope.correlation.reply_to == Some(other),
        "the deferred frame is the mismatch: {deferred:?}"
    );
}

/// Carries `limit` so out-of-order answers stay distinguishable by payload.
fn history_answer(limit: u64) -> WirePayload {
    WirePayload::HistoryRequest(crate::cmds::history_request("companion-1", limit))
}

#[test]
fn select_answer_serves_the_second_request_from_the_queue() {
    let first = message_id(41);
    let second = message_id(42);
    let script = [
        script_frame(history_answer(2), message_id(43), Some(second)),
        script_frame(history_answer(1), message_id(44), Some(first)),
    ];
    let (absorbed, answer, deferred) = select_answer(first, &VecDeque::new(), &script);
    assert!(
        absorbed.is_empty(),
        "no fact means no absorption: {absorbed:?}"
    );
    assert!(
        answer == Some(history_answer(1)),
        "the first request takes its own reply: {answer:?}"
    );
    assert!(
        deferred.len() == 1,
        "the future answer stays queued: {deferred:?}"
    );
    let (absorbed_next, queued, deferred_next) = select_answer(second, &deferred, &[]);
    assert!(
        absorbed_next.is_empty(),
        "a queue hit absorbs nothing: {absorbed_next:?}"
    );
    assert!(
        queued == Some(history_answer(2)),
        "the second request finds its answer already queued: {queued:?}"
    );
    assert!(
        deferred_next.is_empty(),
        "the hit removes the queued frame: {deferred_next:?}"
    );
}

#[test]
fn select_answer_prefers_the_queue_over_new_frames() {
    let own = message_id(51);
    let queued_frame = script_frame(history_answer(9), message_id(52), Some(own));
    let queued: VecDeque<WireFrame> = [queued_frame].into_iter().collect();
    let fresh = [script_frame(history_answer(8), message_id(53), Some(own))];
    let (absorbed, answer, deferred) = select_answer(own, &queued, &fresh);
    assert!(
        absorbed.is_empty(),
        "a queue hit absorbs nothing: {absorbed:?}"
    );
    assert!(
        answer == Some(history_answer(9)),
        "the queued answer wins without socket I/O: {answer:?}"
    );
    assert!(
        deferred.is_empty(),
        "the hit drains the queue and ignores fresh frames: {deferred:?}"
    );
}

#[test]
fn select_answer_bounds_the_queue_oldest_drop() {
    let own = message_id(61);
    let mut queued: VecDeque<WireFrame> = VecDeque::new();
    for index in 0..DEFERRED_CAP {
        let id = u128::try_from(index).map_or(0, |value| value + 100);
        queued.push_back(script_frame(history_answer(7), message_id(id), None));
    }
    assert!(
        queued.len() == DEFERRED_CAP,
        "the fixture queue starts full: {queued:?}"
    );
    let overflow = [script_frame(
        history_answer(7),
        message_id(999),
        Some(message_id(998)),
    )];
    let (_, answer, deferred) = select_answer(own, &queued, &overflow);
    assert!(
        answer.is_none(),
        "a lone mismatch never answers: {answer:?}"
    );
    assert!(
        deferred.len() == DEFERRED_CAP,
        "the queue stays bounded: {deferred:?}"
    );
    assert!(
        deferred[0].envelope.message_id != queued[0].envelope.message_id,
        "the oldest frame drops first"
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
        message.contains(crate::device::BOOTSTRAP_SECRET_ENV),
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
    let proof = ene_credential::pairing_proof_hex("pairing-secret", "nonce-1");
    let frame = proof_frame(&proof, incarnation(), DeviceWireId(uuid::Uuid::new_v4()));
    let WirePayload::AuthProof(carried) = &frame.payload else {
        return Err(String::from("proof builder must emit AuthProof"));
    };
    assert!(
        ene_credential::verify_pairing_proof("pairing-secret", "nonce-1", &carried.proof),
        "the carried proof must verify against the secret and nonce"
    );
    assert!(
        !ene_credential::verify_pairing_proof("pairing-secret", "nonce-2", &carried.proof),
        "the proof must not verify against another nonce (single-use)"
    );
    Ok(())
}

#[test]
fn guidance_names_provisioning_without_secrets() {
    let pending = pending_guidance();
    assert!(
        pending.contains("pending owner confirmation")
            && pending.contains(crate::device::BOOTSTRAP_SECRET_ENV),
        "pending guidance must name the approval plus the provisioning step: {pending:?}"
    );
    let missing = missing_secret_guidance();
    assert!(
        missing.contains("no pairing secret") && missing.contains("approve"),
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
    let mut session = SessionState::new();
    session.set_pairing_secret(String::from("secret-hex-marker-9d4e"));
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
    let mut session = SessionState::new();
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
fn session_deferred_queue_takes_only_the_matching_reply() {
    let mut session = SessionState::new();
    assert!(
        session.deferred_len() == 0,
        "a new session defers nothing: {session:?}"
    );
    let first = message_id(81);
    let second = message_id(82);
    session.push_deferred(script_frame(
        history_answer(2),
        message_id(83),
        Some(second),
    ));
    session.push_deferred(script_frame(history_answer(1), message_id(84), Some(first)));
    assert!(
        session.deferred_len() == 2,
        "both mismatches queue: {session:?}"
    );
    assert!(
        session.take_deferred_reply(first) == Some(history_answer(1)),
        "the take finds the matching reply out of order"
    );
    assert!(
        session.deferred_len() == 1,
        "the hit removes only its frame: {session:?}"
    );
    assert!(
        session.take_deferred_reply(message_id(85)).is_none(),
        "an unknown reply finds nothing"
    );
    assert!(
        session.take_deferred_reply(second) == Some(history_answer(2)),
        "the remaining reply is still queued"
    );
    assert!(session.deferred_len() == 0, "the queue drains: {session:?}");
}
