use super::{AttachOutcome, CHUNK_CHARS, chunk_text};
use crate::serve::{CredStore, HostHandle, LiveInput, device_client};
use crate::test_support::{live_input, memory_handle_with};
use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome,
    ManagementViewRequest, RationaleOrigin,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{
    BaseViewMark, ClientIncarnationId, ClientLocalId, CommandWireId, CompanionWireRef,
    ConnectionWireId, ManagementTargetWire, RoundWireId, TextLangWire, WireMessageType,
};
use ene_api::v1::reject::RejectKind;
use ene_api::v1::round::{
    ConfirmPresentationWire, PresentationStatus, RoundIntakeOutcomeWire, StreamClose,
    SubmitTextInput, TextBodyWire,
};
use ene_credential::{CredentialRef, MemoryCredentialStore};
use ene_inference::ProviderTransport;
use ene_inference::fake::{FakeFailure, FakeProviderTransport};
use ene_presentation::RoundId;
use ene_primitive::RawId;

fn sender() -> WireSender {
    WireSender {
        device_id: None,
        incarnation_id: ClientIncarnationId {
            counter: 3,
            random: 4,
        },
        connection_id: None,
    }
}

/// Stamps a hand-built frame with the connection binding its `live`
/// premises carry, so direct-handle frames pass the gate the same way a
/// connection-table-built frame would.
fn stamped(
    mut frame: ene_plugin_ipc::WireFrame,
    connection: ConnectionWireId,
) -> ene_plugin_ipc::WireFrame {
    frame.envelope.sender.connection_id = Some(connection);
    frame
}

fn submit_frame(
    companion: &str,
    generation: Option<u64>,
    round: Option<RoundWireId>,
    local_id: &str,
    text: &str,
    connection: ConnectionWireId,
) -> ene_plugin_ipc::WireFrame {
    let mut envelope = new_outgoing_envelope(
        ProtocolVersion::V1,
        sender(),
        WireMessageType(String::from("SubmitTextInput")),
    );
    envelope.observed.presence_generation_view = generation;
    envelope.observed.round_view = round.clone();
    // Every submit mints a fresh command id: transport retry reuses the
    // id (the replay test resends the same frame), while distinct sends
    // stay distinct durable commands.
    envelope.correlation.command_id = Some(CommandWireId(RawId::new().as_uuid()));
    let frame = ene_plugin_ipc::WireFrame {
        envelope,
        payload: WirePayload::SubmitTextInput(SubmitTextInput {
            companion: CompanionWireRef(companion.to_string()),
            round,
            fresh: false,
            local_id: ClientLocalId(local_id.to_string()),
            body: TextBodyWire {
                text: text.to_string(),
                lang: TextLangWire(String::from("en")),
            },
        }),
    };
    stamped(frame, connection)
}

fn intent_frame(
    kind: ManagementIntentKind,
    target: &str,
    base: &str,
    connection: ConnectionWireId,
) -> ene_plugin_ipc::WireFrame {
    intent_frame_with_id(
        kind,
        target,
        base,
        connection,
        CommandWireId(RawId::new().as_uuid()),
    )
}

/// Builds an intent frame with a caller-chosen idempotency key, so
/// replay tests can resend the same logical intent byte-for-byte.
fn intent_frame_with_id(
    kind: ManagementIntentKind,
    target: &str,
    base: &str,
    connection: ConnectionWireId,
    intent_id: CommandWireId,
) -> ene_plugin_ipc::WireFrame {
    let frame = ene_plugin_ipc::WireFrame {
        envelope: new_outgoing_envelope(
            ProtocolVersion::V1,
            sender(),
            WireMessageType(String::from("ManagementIntent")),
        ),
        payload: WirePayload::ManagementIntent(ManagementIntent {
            intent_id,
            kind,
            target: ManagementTargetWire(target.to_string()),
            base_view: BaseViewMark(base.to_string()),
            rationale: IntentRationaleWire {
                origin: RationaleOrigin::ManagementSurface,
                quote: None,
            },
        }),
    };
    stamped(frame, connection)
}

fn history_frame(companion: &str, connection: ConnectionWireId) -> ene_plugin_ipc::WireFrame {
    let frame = ene_plugin_ipc::WireFrame {
        envelope: new_outgoing_envelope(
            ProtocolVersion::V1,
            sender(),
            WireMessageType(String::from("HistoryRequest")),
        ),
        payload: WirePayload::HistoryRequest(ene_api::v1::round::HistoryRequest {
            companion: CompanionWireRef(companion.to_string()),
            since: None,
            limit: 100,
        }),
    };
    stamped(frame, connection)
}

fn confirm_frame(round: &RoundWireId, connection: ConnectionWireId) -> ene_plugin_ipc::WireFrame {
    let frame = ene_plugin_ipc::WireFrame {
        envelope: new_outgoing_envelope(
            ProtocolVersion::V1,
            sender(),
            WireMessageType(String::from("ConfirmPresentation")),
        ),
        payload: WirePayload::ConfirmPresentation(ConfirmPresentationWire {
            round: round.clone(),
            stream: None,
            status: PresentationStatus::Presented,
            detail: None,
        }),
    };
    stamped(frame, connection)
}

fn view_request_frame(connection: ConnectionWireId) -> ene_plugin_ipc::WireFrame {
    let frame = ene_plugin_ipc::WireFrame {
        envelope: new_outgoing_envelope(
            ProtocolVersion::V1,
            sender(),
            WireMessageType(String::from("ManagementViewRequest")),
        ),
        payload: WirePayload::ManagementViewRequest(ManagementViewRequest {
            sections: Vec::new(),
        }),
    };
    stamped(frame, connection)
}

fn ok_transport() -> FakeProviderTransport {
    FakeProviderTransport::new(String::from("hi there"), None)
}

/// Result-based handle setup for the round regression tests: a failed
/// open or a failed setup is a test failure, never a silent pass.
async fn round_test_handle<T: ProviderTransport>(
    tag: &str,
    live: &LiveInput,
    transport: &T,
) -> Result<(HostHandle, tempfile::TempDir), String> {
    let Some((handle, dir)) = setup_handle(tag).await else {
        return Err(String::from("handle open failed"));
    };
    if !register_assign_complete(&handle, live, transport).await {
        return Err(String::from("setup must complete"));
    }
    Ok((handle, dir))
}

fn accepted_round(responses: &[ene_plugin_ipc::WireFrame]) -> Result<RoundWireId, String> {
    let Some(first) = responses.first() else {
        return Err(String::from("the submit must answer"));
    };
    match &first.payload {
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { round }) => {
            Ok(round.clone())
        }
        other => Err(format!("the submit must accept, got {other:?}")),
    }
}

fn reject_kind(responses: &[ene_plugin_ipc::WireFrame]) -> Result<RejectKind, String> {
    let Some(first) = responses.first() else {
        return Err(String::from("the submit must answer"));
    };
    match &first.payload {
        WirePayload::Reject(notice) => Ok(notice.kind),
        other => Err(format!("expected a typed reject, got {other:?}")),
    }
}

/// Extracts a wire rejection kind, asserting it carries `connection`
/// (IPC §5).
fn reject_on(
    responses: &[ene_plugin_ipc::WireFrame],
    connection: ConnectionWireId,
) -> Result<RejectKind, String> {
    let kind = reject_kind(responses)?;
    let Some(first) = responses.first() else {
        return Err(String::from("the submit must answer"));
    };
    if first.envelope.sender.connection_id != Some(connection) {
        return Err(format!(
            "the post-auth reject must carry the current connection, got {:?}",
            first.envelope.sender.connection_id
        ));
    }
    Ok(kind)
}

async fn timeline_count(handle: &HostHandle) -> Result<usize, String> {
    use ene_companion::CompanionRepository as _;
    use ene_companion::HistoryRepository as _;

    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .map_err(|error| format!("the companion must resolve: {error:?}"))?;
    let timeline = handle
        .store
        .load_timeline(companion, None, 100)
        .await
        .map_err(|error| format!("the timeline must load: {error:?}"))?;
    Ok(timeline.len())
}

async fn current_generation(handle: &HostHandle) -> Result<u64, String> {
    use ene_companion::CompanionRepository as _;
    use ene_presence::PresenceRepository as _;

    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .map_err(|error| format!("the companion must resolve: {error:?}"))?;
    let attribution = handle
        .store
        .load_attribution(companion.as_raw())
        .await
        .map_err(|error| format!("attribution must load: {error:?}"))?;
    let Some(current) = attribution else {
        return Err(String::from("attribution must load"));
    };
    Ok(current.generation.as_u64())
}
async fn setup_handle(tag: &str) -> Option<(HostHandle, tempfile::TempDir)> {
    memory_handle_with(tag, |store| {
        store.insert(
            CredentialRef::new("openai", "main").expect("valid test fixture"),
            "test-bearer",
        );
    })
    .await
}

async fn register_assign_complete<T: ProviderTransport>(
    handle: &HostHandle,
    live: &LiveInput,
    transport: &T,
) -> bool {
    let registered = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::ConfigureCredentialIntent,
                "credential:openai:main",
                "consent-none",
                live.connection_id,
            ),
            live.clone(),
            transport,
        )
        .await;
    if registered.len() != 1 {
        return false;
    }
    if !matches!(handle.approve_credential("openai", "main").await, Ok(true)) {
        return false;
    }
    let assigned = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::ManageRuleConsentCap,
                "consent:dialogue:openai:dialogue-1:openai:main",
                "consent-none",
                live.connection_id,
            ),
            live.clone(),
            transport,
        )
        .await;
    if assigned.len() != 1 {
        return false;
    }
    let completed = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::ManageRuleConsentCap,
                "setup:complete",
                "consent-rev-1",
                live.connection_id,
            ),
            live.clone(),
            transport,
        )
        .await;
    let Some(done) = completed.first() else {
        return false;
    };
    matches!(
        &done.payload,
        WirePayload::ManagementOutcome(ManagementOutcome::AppliedAsOneTime)
    )
}

/// Assigns the learning capability to the same route the dialogue setup
/// uses, returning whether the assignment committed.
async fn assign_learning(
    handle: &HostHandle,
    live: &LiveInput,
    transport: &impl ProviderTransport,
) -> bool {
    let assigned = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::ManageRuleConsentCap,
                "consent:learning:openai:dialogue-1:openai:main",
                "consent-learning-none",
                live.connection_id,
            ),
            live.clone(),
            transport,
        )
        .await;
    matches!(
        assigned.first().map(|answer| &answer.payload),
        Some(WirePayload::ManagementOutcome(
            ManagementOutcome::StoredAsRuleView { .. }
        ))
    )
}

#[test]
fn empty_text_yields_one_empty_chunk() {
    assert_eq!(chunk_text(""), vec![String::new()]);
}

#[test]
fn chunk_boundaries_hold_at_200_chars() {
    let full: String = "a".repeat(CHUNK_CHARS);
    assert_eq!(chunk_text(&full), vec![full]);
    let over: String = "a".repeat(CHUNK_CHARS + 1);
    let chunks = chunk_text(&over);
    assert_eq!(chunks.len(), 2, "one char over fills two chunks");
    let first = chunks.first().unwrap();
    assert_eq!(first.chars().count(), CHUNK_CHARS);
    let second = chunks.get(1).unwrap();
    assert_eq!(second, "a");
}

#[test]
fn multibyte_text_never_splits_a_code_point() {
    let emoji: String = "😀".repeat(250);
    let chunks = chunk_text(&emoji);
    assert_eq!(chunks.len(), 2, "250 emoji fill two 200-char chunks");
    assert!(
        chunks
            .iter()
            .all(|chunk| chunk.chars().count() <= CHUNK_CHARS),
        "every chunk respects the bound"
    );
    assert_eq!(chunks.concat(), emoji, "reassembly preserves the text");
}

#[tokio::test]
async fn submit_without_setup_needs_revalidation() {
    use ene_companion::CompanionRepository as _;
    use ene_companion::HistoryRepository as _;
    use ene_presence::PresenceRepository as _;

    let (handle, _dir) = memory_handle_with("dlg-nosetup", |_| {}).await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-a");
    let denied = handle
        .handle_frame(
            submit_frame(
                handle.companion_wire(),
                Some(0),
                None,
                "local-1",
                "hello",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert_eq!(denied.len(), 1, "denial answers once");
    let only = denied.first().unwrap();
    assert!(
        matches!(
            &only.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::NeedsRevalidation {
                reason
            }) if reason.0 == "setup-incomplete"
        ),
        "missing consent denies as setup-incomplete, got {:?}",
        only.payload
    );
    let companion = handle.store.ensure_running_companion().await;
    assert!(
        companion.is_ok(),
        "the companion must resolve, got {companion:?}"
    );
    let companion = companion.unwrap();
    let attribution = handle.store.load_attribution(companion.as_raw()).await;
    assert!(
        matches!(attribution, Ok(Some(_))),
        "attribution must load, got {attribution:?}"
    );
    let current = attribution.unwrap().unwrap();
    assert_eq!(
        current.state,
        ene_presence::PresenceState::Present,
        "the winning attach commits before the consent gate runs"
    );
    assert_eq!(
        current.generation.as_u64(),
        1,
        "one attach moves generation zero to one"
    );
    assert_eq!(
        current.active_client,
        Some(device_client("client-a")),
        "the attach names the submitting device"
    );
    let timeline = handle.store.load_timeline(companion, None, 50).await;
    assert!(
        matches!(&timeline, Ok(items) if items.is_empty()),
        "a declined input must leave no history row, got {timeline:?}"
    );
}

#[tokio::test]
async fn attach_without_generation_view_needs_revalidation() {
    use ene_companion::CompanionRepository as _;
    use ene_presence::PresenceRepository as _;

    let (handle, _dir) = setup_handle("dlg-noview").await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-never-attached");
    let answers = handle
        .handle_frame(
            submit_frame(
                handle.companion_wire(),
                None,
                None,
                "local-1",
                "hello",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert_eq!(answers.len(), 1, "a viewless submit answers once");
    let only = answers.first().unwrap();
    assert!(
        matches!(
            &only.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::NeedsRevalidation {
                reason
            }) if reason.0 == "missing-generation-view"
        ),
        "a missing generation view revalidates, got {:?}",
        only.payload
    );
    let companion = handle.store.ensure_running_companion().await;
    assert!(
        companion.is_ok(),
        "the companion must resolve, got {companion:?}"
    );
    let companion = companion.unwrap();
    let attribution = handle.store.load_attribution(companion.as_raw()).await;
    assert!(
        matches!(attribution, Ok(Some(_))),
        "attribution must load, got {attribution:?}"
    );
    let current = attribution.unwrap().unwrap();
    assert_eq!(
        current.state,
        ene_presence::PresenceState::NoActive,
        "a viewless submit attempts no attach"
    );
    assert_eq!(
        current.generation.as_u64(),
        0,
        "a viewless submit moves no generation"
    );
}

#[tokio::test]
async fn attach_with_stale_view_reports_current_values() {
    use ene_companion::CompanionRepository as _;
    use ene_presence::PresenceRepository as _;

    let (handle, _dir) = setup_handle("dlg-staleview").await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-never-attached");
    let answers = handle
        .handle_frame(
            submit_frame(
                handle.companion_wire(),
                Some(7),
                None,
                "local-1",
                "hello",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert_eq!(answers.len(), 1, "a stale-view submit answers once");
    let only = answers.first().unwrap();
    assert!(
        matches!(
            &only.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::StaleRound {
                current_round: None,
                current_generation: 0,
            })
        ),
        "a stale view is rejected with the current values, got {:?}",
        only.payload
    );
    let companion = handle.store.ensure_running_companion().await;
    assert!(
        companion.is_ok(),
        "the companion must resolve, got {companion:?}"
    );
    let companion = companion.unwrap();
    let attribution = handle.store.load_attribution(companion.as_raw()).await;
    let current = attribution.unwrap().unwrap();
    assert_eq!(
        current.state,
        ene_presence::PresenceState::NoActive,
        "a stale-view submit attempts no attach"
    );
    assert_eq!(
        current.generation.as_u64(),
        0,
        "a stale-view submit moves no generation"
    );
}

#[tokio::test]
async fn attach_compare_loser_reports_raced() {
    use ene_companion::CompanionRepository as _;
    use ene_presence::PresenceRepository as _;

    let (handle, _dir) = setup_handle("dlg-race").await.unwrap();
    let companion = handle.store.ensure_running_companion().await;
    assert!(
        companion.is_ok(),
        "the companion must resolve, got {companion:?}"
    );
    let companion = companion.unwrap();
    let initial = handle.store.load_attribution(companion.as_raw()).await;
    assert!(
        matches!(initial, Ok(Some(_))),
        "attribution must load, got {initial:?}"
    );
    let seen = initial.unwrap().unwrap();
    assert_eq!(
        seen.state,
        ene_presence::PresenceState::NoActive,
        "the race starts from no active client"
    );
    assert_eq!(
        seen.generation.as_u64(),
        0,
        "the race starts from generation zero"
    );
    let first = handle
        .attach_presence("client-race", true, seen.generation)
        .await;
    assert!(
        matches!(
            first,
            AttachOutcome::Attached(fresh)
            if fresh.generation.as_u64() == 1
                && fresh.state == ene_presence::PresenceState::Present
        ),
        "the first compare with the observed premise wins generation one, got {first:?}"
    );
    let second = handle
        .attach_presence("client-race", true, seen.generation)
        .await;
    assert!(
        matches!(second, AttachOutcome::Raced),
        "the second compare with the same observed premise loses, got {second:?}"
    );
}

#[tokio::test]
async fn full_dialogue_round_streams_and_restores() {
    use ene_companion::CompanionRepository as _;
    use ene_presence::PresenceRepository as _;

    let (handle, _dir) = setup_handle("dlg-full").await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-a");
    assert!(
        register_assign_complete(&handle, &live, &transport).await,
        "setup must complete"
    );
    let shown = handle
        .handle_frame(
            view_request_frame(live.connection_id),
            live.clone(),
            &transport,
        )
        .await;
    assert_eq!(shown.len(), 1, "a view request answers once");
    let frame = submit_frame(
        handle.companion_wire(),
        Some(0),
        None,
        "local-1",
        "hello",
        live.connection_id,
    );
    let responses = handle
        .handle_frame(frame.clone(), live.clone(), &transport)
        .await;
    assert_eq!(
        responses.len(),
        4,
        "the winning attach accepts with open, one frame, close, got {responses:?}"
    );
    for response in &responses {
        assert_eq!(
            response.envelope.sender.connection_id,
            Some(live.connection_id),
            "every response echoes the connection"
        );
    }
    let accepted = responses.first().unwrap();
    let WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { round }) =
        &accepted.payload
    else {
        return;
    };
    let round = round.clone();
    let opened = responses.get(1).unwrap();
    assert!(
        matches!(
            &opened.payload,
            WirePayload::TextStreamOpen(open) if open.generation == 1
        ),
        "the stream opens at the freshly attached generation, got {:?}",
        opened.payload
    );
    let stream_frame = responses.get(2).unwrap();
    assert!(
        matches!(
            &stream_frame.payload,
            WirePayload::TextStreamFrame(frame) if frame.is_final && frame.seq == 0
        ),
        "the single frame is final at seq zero"
    );
    let closed = responses.get(3).unwrap();
    assert!(
        matches!(
            &closed.payload,
            WirePayload::TextStreamClose(close) if close.status == StreamClose::Completed
        ),
        "the stream completes"
    );
    let companion = handle.store.ensure_running_companion().await;
    assert!(
        companion.is_ok(),
        "the companion must resolve, got {companion:?}"
    );
    let companion = companion.unwrap();
    let attribution = handle.store.load_attribution(companion.as_raw()).await;
    let current = attribution.unwrap().unwrap();
    assert_eq!(
        current.state,
        ene_presence::PresenceState::Present,
        "the first submit attaches the paired device"
    );
    assert_eq!(
        current.generation.as_u64(),
        1,
        "one attach moves generation zero to one"
    );
    assert_eq!(
        current.active_client,
        Some(device_client("client-a")),
        "the attach names the submitting device"
    );
    let confirmed = handle
        .handle_frame(
            confirm_frame(&round, live.connection_id),
            live.clone(),
            &transport,
        )
        .await;
    assert!(
        confirmed.is_empty(),
        "a presentation observation answers nothing"
    );
    let restored = handle
        .handle_frame(
            history_frame(handle.companion_wire(), live.connection_id),
            live.clone(),
            &transport,
        )
        .await;
    assert_eq!(restored.len(), 1, "history answers once");
    let view = restored.first().unwrap();
    let WirePayload::HistoryView(view) = &view.payload else {
        return;
    };
    assert_eq!(view.items.len(), 2, "owner input plus reply restore");
    let replayed = handle.handle_frame(frame, live.clone(), &transport).await;
    assert_eq!(replayed.len(), 1, "a command replay answers once");
    let replay = replayed.first().unwrap();
    assert!(
        matches!(
            &replay.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound {
                round: replayed,
            }) if *replayed == round
        ),
        "the replay re-acks the original round without re-executing, got {:?}",
        replay.payload
    );
    let again = handle
        .handle_frame(
            history_frame(handle.companion_wire(), live.connection_id),
            live.clone(),
            &transport,
        )
        .await;
    let second = again.first().unwrap();
    let WirePayload::HistoryView(second) = &second.payload else {
        return;
    };
    assert_eq!(second.items.len(), 2, "the replay appends nothing durable");
}

#[tokio::test]
async fn fresh_send_mints_despite_matching_open_round() -> Result<(), String> {
    let transport = ok_transport();
    let live = live_input("client-a");
    let (handle, _dir) = round_test_handle("dlg-fresh", &live, &transport).await?;
    let first = submit_frame(
        handle.companion_wire(),
        Some(0),
        None,
        "local-1",
        "hello",
        live.connection_id,
    );
    let first_round = accepted_round(&handle.handle_frame(first, live.clone(), &transport).await)?;
    // The second send observes the current generation so only the
    // round intent differs from a plain continuation.
    let generation = current_generation(&handle).await?;
    let mut second = submit_frame(
        handle.companion_wire(),
        Some(generation),
        None,
        "local-2",
        "again",
        live.connection_id,
    );
    let WirePayload::SubmitTextInput(ref mut input) = second.payload else {
        return Err(String::from("the send frame must carry its input"));
    };
    input.fresh = true;
    let second_round =
        accepted_round(&handle.handle_frame(second, live.clone(), &transport).await)?;
    assert_ne!(
        second_round, first_round,
        "a fresh send must mint instead of joining, got {second_round:?}"
    );
    Ok(())
}

#[tokio::test]
async fn same_round_reuses_one_wire_projection() -> Result<(), String> {
    let transport = ok_transport();
    let live = live_input("client-a");
    let (handle, _dir) = round_test_handle("dlg-one-wire", &live, &transport).await?;
    let first = submit_frame(
        handle.companion_wire(),
        Some(0),
        None,
        "local-1",
        "hello",
        live.connection_id,
    );
    let first_round = accepted_round(&handle.handle_frame(first, live.clone(), &transport).await)?;
    // The second send observes the current generation so it joins the
    // open round instead of re-attaching.
    let generation = current_generation(&handle).await?;
    let second = submit_frame(
        handle.companion_wire(),
        Some(generation),
        None,
        "local-2",
        "again",
        live.connection_id,
    );
    let second_round =
        accepted_round(&handle.handle_frame(second, live.clone(), &transport).await)?;
    assert_eq!(
        second_round, first_round,
        "a joined round must reuse its one projection, got {second_round:?}"
    );
    Ok(())
}

/// An exact retry — same command id, same body, same language, same
/// incarnation, same round premise, same `fresh` flag — replays the
/// original accept verbatim even after the surrounding round state
/// moved on under a different command. Nothing is re-appended and no
/// provider attempt runs: a re-execution against a failing transport
/// would answer an interrupted stream, the replay answers one frame.
#[tokio::test]
async fn retry_after_round_advance_replays_the_stored_accept() -> Result<(), String> {
    let transport = ok_transport();
    let live = live_input("client-a");
    let (handle, _dir) = round_test_handle("dlg-retry-drift", &live, &transport).await?;
    let first = submit_frame(
        handle.companion_wire(),
        Some(0),
        None,
        "local-1",
        "hello",
        live.connection_id,
    );
    let first_round = accepted_round(
        &handle
            .handle_frame(first.clone(), live.clone(), &transport)
            .await,
    )?;
    // The open round moves on under a different command: the drift is
    // Host state only, never part of the retried request.
    let generation = current_generation(&handle).await?;
    let mut drift = submit_frame(
        handle.companion_wire(),
        Some(generation),
        None,
        "local-2",
        "hello",
        live.connection_id,
    );
    if let WirePayload::SubmitTextInput(ref mut input) = drift.payload {
        input.fresh = true;
    }
    let drift_round = accepted_round(&handle.handle_frame(drift, live.clone(), &transport).await)?;
    assert_ne!(
        drift_round, first_round,
        "the drift send must mint a new round"
    );
    let before = timeline_count(&handle).await?;
    // The exact retry resends the original frame byte-for-byte (same
    // command id, same payload, same incarnation and premises).
    let failing = FakeProviderTransport::failing(FakeFailure::Transport(String::from(
        "must never be reached",
    )));
    let responses = handle.handle_frame(first, live.clone(), &failing).await;
    assert_eq!(
        responses.len(),
        1,
        "the replay answers the accept ack only, with no stream: {responses:?}"
    );
    assert_eq!(
        accepted_round(&responses)?,
        first_round,
        "the retry replays the original accept, not the drifted round"
    );
    assert_eq!(
        timeline_count(&handle).await?,
        before,
        "the retry appends nothing durable"
    );
    Ok(())
}

/// Same command id and same request content, but the client round
/// intent flips from join-or-mint to an explicit join: a different
/// request, answered with the typed wire rejection and no side effects.
#[tokio::test]
async fn retry_with_changed_round_intent_conflicts() -> Result<(), String> {
    let transport = ok_transport();
    let live = live_input("client-a");
    let (handle, _dir) = round_test_handle("dlg-retry-intent", &live, &transport).await?;
    let first = submit_frame(
        handle.companion_wire(),
        Some(0),
        None,
        "local-1",
        "hello",
        live.connection_id,
    );
    let first_round = accepted_round(
        &handle
            .handle_frame(first.clone(), live.clone(), &transport)
            .await,
    )?;
    // Same command id, same body/lang/incarnation, but the premise now
    // names the round explicitly (Auto -> Existing): a different
    // request. The companion projection stays the one this handle
    // issued, so only the round intent differs.
    let mut joined = first.clone();
    if let WirePayload::SubmitTextInput(ref mut input) = joined.payload {
        input.round = Some(first_round.clone());
        joined.envelope.observed.round_view = Some(first_round.clone());
    }
    let declined = handle.handle_frame(joined, live.clone(), &transport).await;
    assert_eq!(
        reject_on(&declined, live.connection_id)?,
        RejectKind::ConflictingCommand,
        "a changed round intent must conflict, got {declined:?}"
    );
    assert_eq!(
        timeline_count(&handle).await?,
        2,
        "the conflicting retry appends nothing durable"
    );
    Ok(())
}

/// Same command id and request content, but `fresh` flips
/// join-or-mint into force-new: a different request semantics, so the
/// key conflicts instead of replaying the stored accept.
#[tokio::test]
async fn retry_with_forced_fresh_conflicts() -> Result<(), String> {
    let transport = ok_transport();
    let live = live_input("client-a");
    let (handle, _dir) = round_test_handle("dlg-retry-fresh", &live, &transport).await?;
    let first = submit_frame(
        handle.companion_wire(),
        Some(0),
        None,
        "local-1",
        "hello",
        live.connection_id,
    );
    let original_round = accepted_round(
        &handle
            .handle_frame(first.clone(), live.clone(), &transport)
            .await,
    )?;
    let mut forced = first.clone();
    if let WirePayload::SubmitTextInput(ref mut input) = forced.payload {
        input.fresh = true;
    }
    let declined = handle.handle_frame(forced, live.clone(), &transport).await;
    assert_eq!(
        reject_on(&declined, live.connection_id)?,
        RejectKind::ConflictingCommand,
        "a fresh flip on the same key must conflict"
    );
    // The stored accept stays authoritative: the exact retry still
    // replays after the conflicting attempt changed nothing.
    let replayed = handle.handle_frame(first, live.clone(), &transport).await;
    assert_eq!(
        accepted_round(&replayed)?,
        original_round,
        "the untouched retry still replays the original accept"
    );
    assert_eq!(
        timeline_count(&handle).await?,
        2,
        "conflict and replay append nothing durable"
    );
    Ok(())
}

/// A force-new request carries no round premise (IPC §13.1:
/// `round = None`, `round_view = None`). A premise in either carrier
/// makes the frame self-contradictory: declined stale with current
/// values, never silently reinterpreted as the flag or joined on the
/// hint. Covers the three shapes: `fresh` + payload round, `fresh` +
/// round view, and `fresh` + mismatched round/round view.
#[tokio::test]
async fn forced_fresh_with_a_round_premise_is_declined() -> Result<(), String> {
    let transport = ok_transport();
    let live = live_input("client-a");
    let (handle, _dir) = round_test_handle("dlg-fresh-premise", &live, &transport).await?;
    let first = submit_frame(
        handle.companion_wire(),
        Some(0),
        None,
        "local-1",
        "hello",
        live.connection_id,
    );
    let first_round = accepted_round(&handle.handle_frame(first, live.clone(), &transport).await)?;
    let generation = current_generation(&handle).await?;
    let build_forced = |round: Option<RoundWireId>, view: Option<RoundWireId>| {
        let mut frame = submit_frame(
            handle.companion_wire(),
            Some(generation),
            None,
            "local-2",
            "again",
            live.connection_id,
        );
        // The helper stamps both carriers from one value; the premise
        // rule under test distinguishes them, so set each explicitly.
        if let WirePayload::SubmitTextInput(ref mut input) = frame.payload {
            input.round = round;
            input.fresh = true;
        }
        frame.envelope.observed.round_view = view;
        frame
    };
    let with_round = build_forced(Some(first_round.clone()), None);
    let answers = handle
        .handle_frame(with_round, live.clone(), &transport)
        .await;
    assert_eq!(answers.len(), 1, "a contradictory frame answers once");
    let Some(only) = answers.first() else {
        return Err(String::from("the submit must answer"));
    };
    assert!(
        matches!(
            &only.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::StaleRound { .. })
        ),
        "a force-new frame carrying a payload round must decline stale, got {:?}",
        only.payload
    );
    let viewed = build_forced(None, Some(first_round.clone()));
    let answers = handle.handle_frame(viewed, live.clone(), &transport).await;
    let Some(only) = answers.first() else {
        return Err(String::from("the submit must answer"));
    };
    assert!(
        matches!(
            &only.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::StaleRound { .. })
        ),
        "a force-new frame carrying a round view must decline stale, got {:?}",
        only.payload
    );
    let mismatched = build_forced(
        Some(first_round.clone()),
        Some(RoundWireId(String::from("other-round"))),
    );
    let answers = handle
        .handle_frame(mismatched, live.clone(), &transport)
        .await;
    let Some(only) = answers.first() else {
        return Err(String::from("the submit must answer"));
    };
    assert!(
        matches!(
            &only.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::StaleRound { .. })
        ),
        "a force-new frame with mismatched premises must decline stale, got {:?}",
        only.payload
    );
    let forced = build_forced(None, None);
    let forced_round =
        accepted_round(&handle.handle_frame(forced, live.clone(), &transport).await)?;
    assert_ne!(
        forced_round, first_round,
        "the premise-free force-new request mints a new round"
    );
    assert_eq!(
        timeline_count(&handle).await?,
        4,
        "the declined shapes append nothing durable (two rounds, owner plus reply)"
    );
    Ok(())
}

/// Concurrent sends that join the same open round all use its one wire
/// projection: the atomic get-or-create never mints a second one.
#[tokio::test]
async fn concurrent_joins_of_one_round_share_one_wire() -> Result<(), String> {
    let transport = ok_transport();
    let live = live_input("client-a");
    let (handle, _dir) = round_test_handle("dlg-concurrent", &live, &transport).await?;
    let original = submit_frame(
        handle.companion_wire(),
        Some(0),
        None,
        "local-1",
        "hello",
        live.connection_id,
    );
    let open_wire = accepted_round(
        &handle
            .handle_frame(original, live.clone(), &transport)
            .await,
    )?;
    let generation = current_generation(&handle).await?;
    let first = submit_frame(
        handle.companion_wire(),
        Some(generation),
        None,
        "local-2",
        "one",
        live.connection_id,
    );
    let second = submit_frame(
        handle.companion_wire(),
        Some(generation),
        None,
        "local-3",
        "two",
        live.connection_id,
    );
    let (left, right) = tokio::join!(
        handle.handle_frame(first, live.clone(), &transport),
        handle.handle_frame(second, live.clone(), &transport)
    );
    let left_wire = accepted_round(&left)?;
    let right_wire = accepted_round(&right)?;
    assert_eq!(
        left_wire, right_wire,
        "concurrent joins of one round must share its one wire"
    );
    assert_eq!(left_wire, open_wire, "the open round keeps its projection");
    Ok(())
}

/// The get-or-create itself is atomic: truly parallel requests for one
/// round all receive the same wire, never two mints.
#[tokio::test]
async fn concurrent_projection_requests_mint_one_wire() -> Result<(), String> {
    let Some((handle, _dir)) = setup_handle("dlg-mint-race").await else {
        return Err(String::from("handle open must succeed"));
    };
    let round = RoundId::from_raw(RawId::new());
    let wires: Vec<RoundWireId> = std::thread::scope(|scope| {
        let joins: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| handle.round_wire_or_mint(&round)))
            .collect();
        joins
            .into_iter()
            .map(|join| join.join().map_err(|_| String::from("mint task panicked")))
            .collect::<Vec<Result<RoundWireId, String>>>()
    })
    .into_iter()
    .collect::<Result<Vec<_>, String>>()?;
    let mut distinct: Vec<String> = wires.iter().map(|wire| wire.0.clone()).collect();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        1,
        "one round must mint exactly one wire, got {distinct:?}"
    );
    Ok(())
}

/// After a Host restart, the exact retry replays the original accept
/// from the durable row: the stored wire projection travels verbatim,
/// nothing re-executes, nothing re-appends.
#[tokio::test]
async fn replay_after_restart_replays_from_durable_wire() -> Result<(), String> {
    let Some((handle, dir)) = setup_handle("dlg-restart").await else {
        return Err(String::from("handle open must succeed"));
    };
    let transport = ok_transport();
    let live = live_input("client-a");
    if !register_assign_complete(&handle, &live, &transport).await {
        return Err(String::from("setup must complete"));
    }
    let frame = submit_frame(
        handle.companion_wire(),
        Some(0),
        None,
        "local-9",
        "hello",
        live.connection_id,
    );
    let accepted = handle
        .handle_frame(frame.clone(), live.clone(), &transport)
        .await;
    let accepted_wire = accepted_round(&accepted)?.0;
    drop(handle);
    let fresh = MemoryCredentialStore::new();
    fresh.insert(
        CredentialRef::new("openai", "main").expect("valid test fixture"),
        "test-bearer",
    );
    let reopened = HostHandle::open_with_cred_store(dir.path(), CredStore::Memory(fresh))
        .await
        .map_err(|error| format!("the store must reopen: {error:?}"))?;
    let relive = live_input("client-a");
    let mut resent = frame;
    resent.envelope.sender.connection_id = Some(relive.connection_id);
    // A restarted handle rotates its companion projection: a real Client
    // relearns it from the reconnect presence fact before retrying. The
    // retried command (id, content, incarnation) is unchanged, so the
    // replay path still answers the original accept.
    if let WirePayload::SubmitTextInput(ref mut input) = resent.payload {
        input.companion = CompanionWireRef(reopened.companion_wire().to_string());
    }
    let replayed = reopened
        .handle_frame(resent, relive.clone(), &transport)
        .await;
    assert_eq!(
        replayed.len(),
        1,
        "the restart replay answers once, got {replayed:?}"
    );
    assert_eq!(
        accepted_round(&replayed)?.0,
        accepted_wire,
        "the restart replay answers the original accept from durable state"
    );
    let restored = reopened
        .handle_frame(
            history_frame(reopened.companion_wire(), relive.connection_id),
            relive.clone(),
            &transport,
        )
        .await;
    let Some(view_frame) = restored.first() else {
        return Err(String::from("history must answer"));
    };
    let WirePayload::HistoryView(view) = &view_frame.payload else {
        return Err(String::from("history must answer with a view"));
    };
    assert_eq!(
        view.items.len(),
        2,
        "the restart replay appends nothing durable"
    );
    Ok(())
}

#[tokio::test]
async fn disconnect_clears_an_attached_device() {
    use ene_companion::CompanionRepository as _;
    use ene_presence::PresenceRepository as _;

    let (handle, _dir) = setup_handle("dlg-disc").await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-a");
    assert!(
        register_assign_complete(&handle, &live, &transport).await,
        "setup must complete"
    );
    let accepted = handle
        .handle_frame(
            submit_frame(
                handle.companion_wire(),
                Some(0),
                None,
                "local-1",
                "hello",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert!(
        accepted.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
        )),
        "the send attaches and accepts, got {accepted:?}"
    );
    handle.note_disconnect("client-a").await;
    let companion = handle.store.ensure_running_companion().await;
    let companion = companion.unwrap();
    let attribution = handle.store.load_attribution(companion.as_raw()).await;
    let current = attribution.unwrap().unwrap();
    assert_eq!(
        current.state,
        ene_presence::PresenceState::NoActive,
        "socket close falls back to no active client"
    );
    assert_eq!(
        current.active_client, None,
        "socket close clears the active client"
    );
}

#[tokio::test]
async fn replay_after_disconnect_neither_stales_nor_reattaches() {
    use ene_companion::CompanionRepository as _;
    use ene_presence::PresenceRepository as _;

    let (handle, _dir) = setup_handle("dlg-replay-disc").await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-a");
    assert!(
        register_assign_complete(&handle, &live, &transport).await,
        "setup must complete"
    );
    let frame = submit_frame(
        handle.companion_wire(),
        Some(0),
        None,
        "local-1",
        "hello",
        live.connection_id,
    );
    let accepted = handle
        .handle_frame(frame.clone(), live.clone(), &transport)
        .await;
    assert!(
        accepted.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
        )),
        "the send attaches and accepts, got {accepted:?}"
    );
    handle.note_disconnect("client-a").await;
    // The durable replay check precedes presence attach: the same
    // command replays its original accept even though the device is
    // NoActive again, and presence stays untouched (no re-attach, no
    // generation advance for a send that changes nothing).
    let replayed = handle.handle_frame(frame, live.clone(), &transport).await;
    assert!(
        replayed.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
        )),
        "the post-disconnect retry must replay, not stale, got {replayed:?}"
    );
    let companion = handle.store.ensure_running_companion().await;
    let companion = companion.unwrap();
    let attribution = handle.store.load_attribution(companion.as_raw()).await;
    assert!(
        matches!(&attribution, Ok(Some(current)) if current.state == ene_presence::PresenceState::NoActive && current.active_client.is_none()),
        "the replay must not re-attach presence, got {attribution:?}"
    );
}

#[tokio::test]
async fn provider_failure_interrupts_after_accept() {
    let (handle, _dir) = setup_handle("dlg-fail").await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-a");
    assert!(
        register_assign_complete(&handle, &live, &transport).await,
        "setup must complete"
    );
    let accepted = handle
        .handle_frame(
            submit_frame(
                handle.companion_wire(),
                Some(0),
                None,
                "local-1",
                "probe",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert!(
        accepted.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
        )),
        "the first send attaches and accepts, got {accepted:?}"
    );
    let failing = FakeProviderTransport::failing(FakeFailure::Transport(String::from("down")));
    let responses = handle
        .handle_frame(
            submit_frame(
                handle.companion_wire(),
                Some(1),
                None,
                "local-9",
                "hello",
                live.connection_id,
            ),
            live.clone(),
            &failing,
        )
        .await;
    assert_eq!(responses.len(), 3, "accept plus an interrupted stream");
    let closed = responses.get(2).unwrap();
    assert!(
        matches!(
            &closed.payload,
            WirePayload::TextStreamClose(close) if close.status == StreamClose::Interrupted
        ),
        "a send failure interrupts the stream"
    );
}

#[tokio::test]
async fn consent_replay_is_idempotent_and_moves_report_staleness() {
    let (handle, _dir) = setup_handle("dlg-cas").await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-a");
    let registered = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::ConfigureCredentialIntent,
                "credential:openai:main",
                "consent-none",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert_eq!(registered.len(), 1, "register answers once");
    assert!(
        handle.approve_credential("openai", "main").await.is_ok(),
        "approval must succeed"
    );
    let assigned = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::ManageRuleConsentCap,
                "consent:dialogue:openai:dialogue-1:openai:main",
                "consent-none",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    let stored = assigned.first().unwrap();
    assert!(
        matches!(
            &stored.payload,
            WirePayload::ManagementOutcome(ManagementOutcome::StoredAsRuleView { .. })
        ),
        "the first assign commits"
    );
    let replayed = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::ManageRuleConsentCap,
                "consent:dialogue:openai:dialogue-1:openai:main",
                "consent-none",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    let same = replayed.first().unwrap();
    assert!(
        matches!(
            &same.payload,
            WirePayload::ManagementOutcome(ManagementOutcome::StaleBaseView { current })
            if current.0 == "consent-dialogue-rev-1"
        ),
        "the identical replay on its stale base reports staleness (not silent success), got {:?}",
        same.payload
    );
    let converged = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::ManageRuleConsentCap,
                "consent:dialogue:openai:dialogue-1:openai:main",
                "consent-rev-1",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    let same = converged.first().unwrap();
    assert!(
        matches!(
            &same.payload,
            WirePayload::ManagementOutcome(ManagementOutcome::StoredAsRuleView {
                revision
            }) if revision.0 == "consent-dialogue-rev-1"
        ),
        "repeating the identical assign on a fresh base is a no-op at the same revision, got {:?}",
        same.payload
    );
    let moved = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::ManageRuleConsentCap,
                "consent:dialogue:openai:dialogue-2:openai:main",
                "consent-none",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    let stale = moved.first().unwrap();
    assert!(
        matches!(
            &stale.payload,
            WirePayload::ManagementOutcome(ManagementOutcome::StaleBaseView { current })
            if current.0 == "consent-dialogue-rev-1"
        ),
        "a changed assign on a stale base reports the rebuilt current mark"
    );
}

#[tokio::test]
async fn assign_intent_replay_returns_the_stored_success() {
    let (handle, _dir) = setup_handle("dlg-intentreplay").await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-a");
    let registered = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::ConfigureCredentialIntent,
                "credential:openai:main",
                "consent-none",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert_eq!(registered.len(), 1, "register answers once");
    assert!(
        handle.approve_credential("openai", "main").await.is_ok(),
        "approval must succeed"
    );
    let intent_id = CommandWireId(RawId::new().as_uuid());
    let assign_x = |base: &str| {
        intent_frame_with_id(
            ManagementIntentKind::ManageRuleConsentCap,
            "consent:dialogue:openai:dialogue-1:openai:main",
            base,
            live.connection_id,
            intent_id,
        )
    };
    let assigned = handle
        .handle_frame(assign_x("consent-none"), live.clone(), &transport)
        .await;
    assert!(
        matches!(
            &assigned.first().map(|first| &first.payload),
            Some(WirePayload::ManagementOutcome(
                ManagementOutcome::StoredAsRuleView { .. }
            ))
        ),
        "the first assign commits, got {assigned:?}"
    );
    // Exact retry (same id, same bytes; the base is stale now): the
    // durable replay answers the stored success at the current revision
    // instead of reporting staleness.
    let replayed = handle
        .handle_frame(assign_x("consent-none"), live.clone(), &transport)
        .await;
    let same = replayed.first().unwrap();
    assert!(
        matches!(
            &same.payload,
            WirePayload::ManagementOutcome(ManagementOutcome::StoredAsRuleView {
                revision
            }) if revision.0 == "consent-dialogue-rev-1"
        ),
        "the exact retry must replay success at rev 1, got {:?}",
        same.payload
    );
    // The route moves on under a different intent; retrying X still
    // answers its own prior outcome verbatim (§6.2: never re-executed).
    // The rev-1 mark no longer names current state, so the caller's
    // NEXT intent built on it reports stale and converges — replay
    // stays honest by returning history, not by recomputing the present.
    let moved = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::ManageRuleConsentCap,
                "consent:dialogue:openai:dialogue-2:openai:main",
                "consent-rev-1",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert!(
        matches!(
            &moved.first().map(|first| &first.payload),
            Some(WirePayload::ManagementOutcome(
                ManagementOutcome::StoredAsRuleView { .. }
            ))
        ),
        "the route move commits, got {moved:?}"
    );
    let stale_retry = handle
        .handle_frame(assign_x("consent-none"), live.clone(), &transport)
        .await;
    assert!(
        matches!(
            &stale_retry.first().map(|first| &first.payload),
            Some(WirePayload::ManagementOutcome(ManagementOutcome::StoredAsRuleView {
                revision
            })) if revision.0 == "consent-dialogue-rev-1"
        ),
        "a replay after the route moved still answers its prior outcome, got {stale_retry:?}"
    );
}

#[tokio::test]
async fn assign_intent_conflict_clarifies_without_side_effects() {
    use ene_permission::{CapabilityKind, ConsentRepository as _};

    let (handle, _dir) = setup_handle("dlg-intentconflict").await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-a");
    assert!(
        register_assign_complete(&handle, &live, &transport).await,
        "setup must complete"
    );
    let intent_id = CommandWireId(RawId::new().as_uuid());
    let assigned = handle
        .handle_frame(
            intent_frame_with_id(
                ManagementIntentKind::ManageRuleConsentCap,
                "consent:dialogue:openai:dialogue-1:openai:main",
                "consent-rev-1",
                live.connection_id,
                intent_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert!(
        matches!(
            &assigned.first().map(|first| &first.payload),
            Some(WirePayload::ManagementOutcome(
                ManagementOutcome::StoredAsRuleView { .. }
            ))
        ),
        "the first assign commits, got {assigned:?}"
    );
    // Same intent id, different content: declined without adopting the
    // new meaning, and the stored route is untouched.
    let conflicted = handle
        .handle_frame(
            intent_frame_with_id(
                ManagementIntentKind::ManageRuleConsentCap,
                "consent:dialogue:openai:dialogue-2:openai:main",
                "consent-rev-1",
                live.connection_id,
                intent_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    let only = conflicted.first().unwrap();
    assert!(
        matches!(
            &only.payload,
            WirePayload::ManagementOutcome(ManagementOutcome::NeedsClarification)
        ),
        "a reused id with new content must clarify, got {:?}",
        only.payload
    );
    let current = handle.store.load_current(CapabilityKind::Dialogue).await;
    assert!(
        matches!(&current, Ok(Some(record)) if record.model == "dialogue-1"),
        "the conflict must not move consent, got {current:?}"
    );
}

#[tokio::test]
async fn malformed_target_reuse_conflicts_without_side_effects() {
    use ene_permission::{CapabilityKind, ConsentRepository as _};

    let (handle, _dir) = setup_handle("dlg-malformed-reuse").await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-a");
    assert!(
        register_assign_complete(&handle, &live, &transport).await,
        "setup must complete"
    );
    let intent_id = CommandWireId(RawId::new().as_uuid());
    // Malformed target first: clarifies AND claims the id, so the row
    // exists for what follows.
    let malformed = handle
        .handle_frame(
            intent_frame_with_id(
                ManagementIntentKind::ManageRuleConsentCap,
                "consent:bogus",
                "consent-rev-1",
                live.connection_id,
                intent_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert!(
        matches!(
            &malformed.first().map(|first| &first.payload),
            Some(WirePayload::ManagementOutcome(
                ManagementOutcome::NeedsClarification
            ))
        ),
        "a malformed target must clarify, got {malformed:?}"
    );
    // Same id with a now-valid target: must conflict, never proceed to
    // assign — the prior row owns this id.
    let reused = handle
        .handle_frame(
            intent_frame_with_id(
                ManagementIntentKind::ManageRuleConsentCap,
                "consent:dialogue:openai:dialogue-9:openai:main",
                "consent-rev-1",
                live.connection_id,
                intent_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    let only = reused.first().unwrap();
    assert!(
        matches!(
            &only.payload,
            WirePayload::ManagementOutcome(ManagementOutcome::NeedsClarification)
        ),
        "reusing a clarified id with new content must clarify, got {:?}",
        only.payload
    );
    let current = handle.store.load_current(CapabilityKind::Dialogue).await;
    assert!(
        matches!(&current, Ok(Some(record)) if record.model == "dialogue-1"),
        "the conflict must not move consent, got {current:?}"
    );
}

#[tokio::test]
async fn complete_replay_returns_the_stored_snapshot() {
    let (handle, _dir) = setup_handle("dlg-completeray").await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-a");
    assert!(
        register_assign_complete(&handle, &live, &transport).await,
        "setup must complete"
    );
    let intent_id = CommandWireId(RawId::new().as_uuid());
    let complete = intent_frame_with_id(
        ManagementIntentKind::ManageRuleConsentCap,
        "setup:complete",
        "consent-rev-1",
        live.connection_id,
        intent_id,
    );
    let applied = handle
        .handle_frame(complete.clone(), live.clone(), &transport)
        .await;
    assert!(
        matches!(
            &applied.first().map(|first| &first.payload),
            Some(WirePayload::ManagementOutcome(
                ManagementOutcome::AppliedAsOneTime
            ))
        ),
        "the first completion applies, got {applied:?}"
    );
    let replayed = handle
        .handle_frame(complete, live.clone(), &transport)
        .await;
    assert!(
        matches!(
            &replayed.first().map(|first| &first.payload),
            Some(WirePayload::ManagementOutcome(
                ManagementOutcome::AppliedAsOneTime
            ))
        ),
        "the exact retry must replay applied, got {replayed:?}"
    );
}

#[tokio::test]
async fn complete_stale_replay_returns_its_own_mark() {
    let (handle, _dir) = setup_handle("dlg-completestale").await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-a");
    assert!(
        register_assign_complete(&handle, &live, &transport).await,
        "setup must complete"
    );
    let intent_id = CommandWireId(RawId::new().as_uuid());
    let stale = intent_frame_with_id(
        ManagementIntentKind::ManageRuleConsentCap,
        "setup:complete",
        "consent-none",
        live.connection_id,
        intent_id,
    );
    let first = handle
        .handle_frame(stale.clone(), live.clone(), &transport)
        .await;
    assert!(
        matches!(
            &first.first().map(|first| &first.payload),
            Some(WirePayload::ManagementOutcome(ManagementOutcome::StaleBaseView {
                current
            })) if current.0 == "consent-dialogue-rev-1"
        ),
        "the stale completion reports rev 1, got {first:?}"
    );
    // Move the route on under a different id, then retry the stale id:
    // the snapshot still names rev 1 (history, not present).
    let moved = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::ManageRuleConsentCap,
                "consent:dialogue:openai:dialogue-2:openai:main",
                "consent-rev-1",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert!(
        matches!(
            &moved.first().map(|first| &first.payload),
            Some(WirePayload::ManagementOutcome(
                ManagementOutcome::StoredAsRuleView { .. }
            ))
        ),
        "the route move commits, got {moved:?}"
    );
    let replayed = handle.handle_frame(stale, live.clone(), &transport).await;
    assert!(
        matches!(
            &replayed.first().map(|first| &first.payload),
            Some(WirePayload::ManagementOutcome(ManagementOutcome::StaleBaseView {
                current
            })) if current.0 == "consent-dialogue-rev-1"
        ),
        "the stale retry must replay its own mark, got {replayed:?}"
    );
}

#[tokio::test]
async fn assign_stale_replay_returns_its_own_mark() {
    let (handle, _dir) = setup_handle("dlg-assignstale").await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-a");
    let intent_id = CommandWireId(RawId::new().as_uuid());
    // No consent yet: a rev-9 base is stale on its face.
    let stale = intent_frame_with_id(
        ManagementIntentKind::ManageRuleConsentCap,
        "consent:dialogue:openai:dialogue-1:openai:main",
        "consent-rev-9",
        live.connection_id,
        intent_id,
    );
    for attempt in 0..2 {
        let answered = handle
            .handle_frame(stale.clone(), live.clone(), &transport)
            .await;
        assert!(
            matches!(
                &answered.first().map(|first| &first.payload),
                Some(WirePayload::ManagementOutcome(ManagementOutcome::StaleBaseView {
                    current
                })) if current.0 == "consent-dialogue-none"
            ),
            "attempt {attempt} must report the empty mark, got {answered:?}"
        );
    }
}

#[tokio::test]
async fn submit_without_command_id_is_declined_without_side_effects() {
    use ene_companion::{CompanionRepository as _, HistoryRepository as _};

    let (handle, _dir) = setup_handle("dlg-nocmd").await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-a");
    assert!(
        register_assign_complete(&handle, &live, &transport).await,
        "setup must complete"
    );
    let mut frame = submit_frame(
        handle.companion_wire(),
        Some(0),
        None,
        "local-1",
        "hello",
        live.connection_id,
    );
    frame.envelope.correlation.command_id = None;
    let declined = handle.handle_frame(frame, live.clone(), &transport).await;
    let only = declined.first().unwrap();
    assert!(
        matches!(
            &only.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::NeedsRevalidation {
                reason
            }) if reason.0 == "missing-command-id"
        ),
        "a keyless command must decline explicitly, got {:?}",
        only.payload
    );
    let companion = handle.store.ensure_running_companion().await;
    let companion = companion.unwrap();
    let timeline = handle.store.load_timeline(companion, None, 50).await;
    assert!(
        matches!(&timeline, Ok(items) if items.is_empty()),
        "a declined keyless input must leave no history row, got {timeline:?}"
    );
}

#[tokio::test]
async fn submit_with_reused_command_and_new_text_is_declined() {
    use ene_companion::{CompanionRepository as _, HistoryRepository as _};

    let (handle, _dir) = setup_handle("dlg-mismatch").await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-a");
    assert!(
        register_assign_complete(&handle, &live, &transport).await,
        "setup must complete"
    );
    let frame = submit_frame(
        handle.companion_wire(),
        Some(0),
        None,
        "local-1",
        "hello",
        live.connection_id,
    );
    let first = handle
        .handle_frame(frame.clone(), live.clone(), &transport)
        .await;
    assert!(
        first.first().is_some_and(|answer| matches!(
            &answer.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
        )),
        "the first send must accept"
    );
    let mut forged = frame;
    if let WirePayload::SubmitTextInput(ref mut input) = forged.payload {
        input.body.text = String::from("different words, same command");
    }
    let declined = handle.handle_frame(forged, live.clone(), &transport).await;
    let only = declined.first().unwrap();
    assert!(
        matches!(
            &only.payload,
            WirePayload::Reject(notice) if notice.kind == RejectKind::ConflictingCommand
        ),
        "a reused key with new content must decline, got {:?}",
        only.payload
    );
    assert_eq!(
        only.envelope.sender.connection_id,
        Some(live.connection_id),
        "the post-auth conflict reject carries the current connection"
    );
    let companion = handle.store.ensure_running_companion().await;
    let companion = companion.unwrap();
    let timeline = handle.store.load_timeline(companion, None, 50).await;
    assert!(
        matches!(&timeline, Ok(items) if items.len() == 2),
        "the declined forgery must append nothing (owner plus reply only), got {timeline:?}"
    );
}

/// Transport that revokes consent mid-flight: it bumps the stored
/// consent revision inside `complete` (before delegating to the inner
/// fake), so the adoption gate after the await sees a moved record.
/// Models a real revocation landing during a slow provider call.
struct RevokingTransport {
    db: std::path::PathBuf,
    inner: FakeProviderTransport,
}

impl ene_inference::ProviderTransport for RevokingTransport {
    fn complete(
        &self,
        req: ene_inference::ProviderRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        ene_inference::ProviderResponse,
                        ene_inference::InferenceTechnicalError,
                    >,
                > + Send
                + '_,
        >,
    > {
        let db = self.db.clone();
        let inner = self.inner.clone();
        Box::pin(async move {
            use ene_permission::{CapabilityKind, ConsentRepository as _};
            if let Ok(store) = ene_store::Store::open(&db).await
                && let Ok(Some(current)) = store.load_current(CapabilityKind::Dialogue).await
            {
                use ene_permission::{ConsentRepository as _, ConsentRevision};
                let bumped = ene_permission::ConsentRecord {
                    capability: CapabilityKind::Dialogue,
                    id: current.id.clone(),
                    rev: ConsentRevision::from_u64(current.rev.as_u64() + 1),
                    provider: current.provider.clone(),
                    model: current.model.clone(),
                    credential_id: current.credential_id.clone(),
                };
                let _bumped = store
                    .compare_and_save(Some((current.id, current.rev)), bumped)
                    .await;
            }
            inner.complete(req).await
        })
    }
}

#[tokio::test]
async fn submit_with_unknown_companion_needs_revalidation() {
    use ene_companion::{CompanionRepository as _, HistoryRepository as _};

    let (handle, _dir) = setup_handle("dlg-unknowncomp").await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-a");
    assert!(
        register_assign_complete(&handle, &live, &transport).await,
        "setup must complete"
    );
    let declined = handle
        .handle_frame(
            submit_frame(
                "not-the-issued-projection",
                Some(0),
                None,
                "local-1",
                "hello",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    let only = declined.first().unwrap();
    assert!(
        matches!(
            &only.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::NeedsRevalidation {
                reason
            }) if reason.0 == "unknown-companion"
        ),
        "an unresolvable companion ref must revalidate, got {:?}",
        only.payload
    );
    let companion = handle.store.ensure_running_companion().await;
    let companion = companion.unwrap();
    let timeline = handle.store.load_timeline(companion, None, 50).await;
    assert!(
        matches!(&timeline, Ok(items) if items.is_empty()),
        "the unknown-companion send must append nothing, got {timeline:?}"
    );
}

#[tokio::test]
async fn consent_move_mid_flight_interrupts_adoption() {
    use ene_companion::{CompanionRepository as _, HistoryRepository as _};

    let (handle, dir) = setup_handle("dlg-midflight").await.unwrap();
    let live = live_input("client-a");
    let fake = ok_transport();
    assert!(
        register_assign_complete(&handle, &live, &fake).await,
        "setup must complete"
    );
    let transport = RevokingTransport {
        db: dir.path().join("app.db"),
        inner: fake,
    };
    let frame = submit_frame(
        handle.companion_wire(),
        Some(0),
        None,
        "local-1",
        "hello",
        live.connection_id,
    );
    let responses = handle.handle_frame(frame, live.clone(), &transport).await;
    let last = responses.last().unwrap();
    assert!(
        matches!(
            &last.payload,
            WirePayload::TextStreamClose(close)
                if close.status == StreamClose::Interrupted
        ),
        "a mid-flight consent move must interrupt, got {:?}",
        last.payload
    );
    let companion = handle.store.ensure_running_companion().await;
    let companion = companion.unwrap();
    let timeline = handle.store.load_timeline(companion, None, 50).await;
    assert!(
        matches!(&timeline, Ok(items) if items.len() == 1),
        "only the owner row commits on interrupted adoption, got {timeline:?}"
    );
}

#[tokio::test]
async fn register_holds_until_host_local_approval() {
    // Approval now requires a readable bearer: it must sweep any prior
    // plaintext occurrence before the ref becomes usable.
    let (handle, _dir) = memory_handle_with("dlg-credgate", |store| {
        store.insert(
            CredentialRef::new("openai", "main").expect("valid test fixture"),
            "test-bearer",
        );
    })
    .await
    .unwrap();
    let transport = ok_transport();
    let live = live_input("client-a");
    let pending = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::ConfigureCredentialIntent,
                "credential:openai:main",
                "consent-none",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    let first = pending.first().unwrap();
    assert!(
        matches!(
            &first.payload,
            WirePayload::ManagementOutcome(ManagementOutcome::HeldByOperation)
        ),
        "an unapproved registration holds, got {:?}",
        first.payload
    );
    assert!(
        handle.approve_credential("openai", "main").await.is_ok(),
        "host-local approval must succeed"
    );
    let usable = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::ConfigureCredentialIntent,
                "credential:openai:main",
                "consent-none",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    let second = usable.first().unwrap();
    assert!(
        matches!(
            &second.payload,
            WirePayload::ManagementOutcome(ManagementOutcome::AppliedAsOneTime)
        ),
        "re-request after approval applies, got {:?}",
        second.payload
    );
}

#[tokio::test]
async fn setup_edge_cases_clarify_or_hold() {
    let (handle, _dir) = memory_handle_with("dlg-edge", |_| {}).await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-a");
    let malformed = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::ConfigureCredentialIntent,
                "credential:lonely",
                "consent-none",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    let first = malformed.first().unwrap();
    assert!(
        matches!(
            &first.payload,
            WirePayload::ManagementOutcome(ManagementOutcome::NeedsClarification)
        ),
        "a malformed register target clarifies"
    );
    let stale_base = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::ManageRuleConsentCap,
                "consent:dialogue:openai:dialogue-1:openai:main",
                "consent-rev-99",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    let second = stale_base.first().unwrap();
    assert!(
        matches!(
            &second.payload,
            WirePayload::ManagementOutcome(ManagementOutcome::StaleBaseView { .. })
        ),
        "a moved base view reports staleness"
    );
    let foreign = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::StopCompanion,
                "companion-1",
                "consent-none",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    let third = foreign.first().unwrap();
    assert!(
        matches!(
            &third.payload,
            WirePayload::ManagementOutcome(ManagementOutcome::NeedsClarification)
        ),
        "a non-setup kind clarifies"
    );
    let incomplete = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::ManageRuleConsentCap,
                "setup:complete",
                "consent-none",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    let fourth = incomplete.first().unwrap();
    assert!(
        matches!(
            &fourth.payload,
            WirePayload::ManagementOutcome(ManagementOutcome::NeedsClarification)
        ),
        "an incomplete premise cannot complete setup"
    );
}

/// The learning consumer is admitted through its own candidate triple and
/// its own capability consent, so a dialogue assignment can never authorize
/// a formation pass; both admissions then coexist under their separately
/// assigned routes.
#[tokio::test]
async fn learning_admission_requires_its_own_capability_assignment() {
    use super::HostInference;
    use ene_inference::{Admission, InferenceExecutor as _};

    let live = live_input("dlg-learning-admit");
    let transport = ok_transport();
    let setup = round_test_handle("dlg-learning-admit", &live, &transport).await;
    let (handle, _dir) = setup.unwrap();
    let executor = HostInference {
        store: &handle.store,
        cred_store: &handle.cred_store,
        tracker: &handle.tracker,
        transport: &transport,
    };
    // Stage 2 setup assigned the dialogue capability only.
    assert!(
        matches!(executor.admit_dialogue().await, Ok(Admission::Admitted(_))),
        "dialogue admission still works independently"
    );
    assert!(
        matches!(executor.admit_learning().await, Ok(Admission::Declined(_))),
        "dialogue consent must not authorize learning formation"
    );

    // The Owner assigns the same route to learning explicitly.
    let assigned = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::ManageRuleConsentCap,
                "consent:learning:openai:dialogue-1:openai:main",
                "consent-learning-none",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert!(
        matches!(
            assigned.first().map(|answer| &answer.payload),
            Some(WirePayload::ManagementOutcome(
                ManagementOutcome::StoredAsRuleView { .. }
            ))
        ),
        "the learning assignment must commit, got {assigned:?}"
    );
    assert!(
        matches!(executor.admit_learning().await, Ok(Admission::Admitted(_))),
        "learning is admitted only after its own consent exists"
    );
    assert!(
        matches!(executor.admit_dialogue().await, Ok(Admission::Admitted(_))),
        "the dialogue assignment is untouched"
    );
}

/// A transport that answers the learning formation prompt with a configured
/// JSON answer and every dialogue call with fixed reply text.
struct LearningAwareTransport {
    reply: String,
    formation: Option<String>,
    inputs: std::sync::Mutex<Vec<String>>,
}

impl LearningAwareTransport {
    fn new(reply: &str, formation: Option<&str>) -> Self {
        Self {
            reply: reply.to_owned(),
            formation: formation.map(str::to_owned),
            inputs: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn inputs(&self) -> Vec<String> {
        self.inputs.lock().expect("recorded input lock").clone()
    }
}

impl ProviderTransport for LearningAwareTransport {
    fn complete(
        &self,
        req: ene_inference::ProviderRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        ene_inference::ProviderResponse,
                        ene_inference::InferenceTechnicalError,
                    >,
                > + Send
                + '_,
        >,
    > {
        self.inputs
            .lock()
            .expect("recorded input lock")
            .push(req.input.clone());
        let text = if req.input.contains("learning formation pass") {
            self.formation.clone().unwrap_or_default()
        } else {
            self.reply.clone()
        };
        Box::pin(async move { Ok(ene_inference::ProviderResponse { text, usage: None }) })
    }
}

/// Transport whose learning formation call blocks until released, so a test
/// can prove the client-visible reply does not wait for Learning.
struct BlockingLearningTransport {
    reply: String,
    learning_started: std::sync::Arc<tokio::sync::Notify>,
    release: std::sync::Arc<tokio::sync::Notify>,
}

impl ProviderTransport for BlockingLearningTransport {
    fn complete(
        &self,
        req: ene_inference::ProviderRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        ene_inference::ProviderResponse,
                        ene_inference::InferenceTechnicalError,
                    >,
                > + Send
                + '_,
        >,
    > {
        let learning = req.input.contains("learning formation pass");
        let reply = self.reply.clone();
        let started = std::sync::Arc::clone(&self.learning_started);
        let release = std::sync::Arc::clone(&self.release);
        Box::pin(async move {
            if learning {
                started.notify_one();
                release.notified().await;
                return Ok(ene_inference::ProviderResponse {
                    text: String::from(r#"{"summary": "Nothing new.", "memories": []}"#),
                    usage: None,
                });
            }
            Ok(ene_inference::ProviderResponse {
                text: reply,
                usage: None,
            })
        })
    }
}

fn assert_stream_completed(responses: &[ene_plugin_ipc::WireFrame]) {
    let Some(last) = responses.last() else {
        panic!("the turn must answer");
    };
    assert!(
        matches!(
            &last.payload,
            WirePayload::TextStreamClose(close) if close.status == StreamClose::Completed
        ),
        "the dialogue must still complete, got {:?}",
        last.payload
    );
}

#[tokio::test]
async fn completed_reply_forms_memory_and_keeps_summary_evidence() {
    use ene_companion::CompanionRepository as _;
    use ene_learning::LearningRepository as _;

    let transport = LearningAwareTransport::new(
        "noted",
        Some(
            r#"{"summary": "The owner likes jasmine tea.", "memories": [{"action": "create", "content": "The owner likes jasmine tea.", "importance": 4, "temporal": "enduring"}]}"#,
        ),
    );
    let live = live_input("client-formation");
    let setup = round_test_handle("dlg-formation", &live, &transport).await;
    let (handle, _dir) = setup.unwrap();
    assert!(
        assign_learning(&handle, &live, &transport).await,
        "learning formation needs its own capability assignment"
    );
    let frame = submit_frame(
        handle.companion_wire(),
        Some(0),
        None,
        "local-formation",
        "please remember that I like jasmine tea",
        live.connection_id,
    );
    let responses = handle.handle_frame(frame, live.clone(), &transport).await;
    assert_stream_completed(&responses);
    // The formation pass is post-response work; drain it explicitly here.
    handle.run_pending_learning(&transport).await;

    let companion = handle.store.ensure_running_companion().await.unwrap();
    let memories = handle
        .store
        .list_current_memories(companion.as_raw(), 10)
        .await
        .unwrap();
    assert_eq!(memories.len(), 1, "one compressed memory is formed");
    assert_eq!(memories[0].content, "The owner likes jasmine tea.");
    assert_eq!(memories[0].importance.as_u8(), 4);
    let revisions = handle
        .store
        .list_memory_revisions(memories[0].id)
        .await
        .unwrap();
    assert_eq!(revisions.len(), 1, "the initial revision is recorded");
    let summary = handle
        .store
        .load_summary(revisions[0].summary.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(summary.content.contains("jasmine tea"), "grounds are kept");
}

/// A provider or transport failure in the Learning pass is a technical
/// failure, not a semantic decline: an experience the model never judged must
/// not be reported as "nothing worth keeping".
#[tokio::test]
async fn learning_transport_failure_is_reported_as_unavailable() {
    use super::{CredentialScrubber, HostInference};
    use ene_companion::CompanionRepository as _;
    use ene_learning::{
        ExperienceCandidate, ExperienceCorrespondence, ExperienceRole, ExperienceSourceKind,
        ExperienceTurn, LearningTechnicalError, SourceRangeRef,
    };
    use ene_primitive::WallClockWithTz;

    let live = live_input("client-learning-failure");
    let transport = ok_transport();
    let setup = round_test_handle("dlg-learning-failure", &live, &transport).await;
    let (handle, _dir) = setup.unwrap();
    assert!(assign_learning(&handle, &live, &transport).await);
    let companion = handle.store.ensure_running_companion().await.unwrap();

    let failing =
        FakeProviderTransport::failing(FakeFailure::Transport(String::from("provider down")));
    let executor = HostInference {
        store: &handle.store,
        cred_store: &handle.cred_store,
        tracker: &handle.tracker,
        transport: &failing,
    };
    let scrubber = CredentialScrubber {
        refs: &handle.store,
        store: &handle.cred_store,
    };
    let candidate = ExperienceCandidate {
        companion: companion.as_raw(),
        source: SourceRangeRef {
            kind: ExperienceSourceKind::Dialogue,
            start: RawId::new(),
            end: RawId::new(),
        },
        transcript: vec![ExperienceTurn {
            role: ExperienceRole::Owner,
            text: String::from("remember this"),
        }],
        at: WallClockWithTz::now(),
        correspondence: ExperienceCorrespondence::default(),
    };

    let outcome =
        ene_companion::dialogue::propose_experience(candidate, &handle.store, &executor, &scrubber)
            .await;
    assert!(
        matches!(
            outcome,
            Err(LearningTechnicalError::InferenceUnavailable { .. })
        ),
        "a transport failure must not be reported as a decline, got {outcome:?}"
    );
}

#[tokio::test]
async fn formation_scrubs_registered_credentials_from_prompt_and_storage() {
    use ene_companion::CompanionRepository as _;
    use ene_companion::HistoryRepository as _;
    use ene_learning::LearningRepository as _;

    // The fixture handle provisions `openai:main` with "test-bearer".
    let transport = LearningAwareTransport::new(
        "noted",
        Some(
            r#"{"summary": "The owner shared test-bearer.", "memories": [{"action": "create", "content": "The owner's key is test-bearer.", "importance": 5, "temporal": "enduring"}]}"#,
        ),
    );
    let live = live_input("client-secret");
    let setup = round_test_handle("dlg-secret", &live, &transport).await;
    let (handle, _dir) = setup.unwrap();
    assert!(assign_learning(&handle, &live, &transport).await);
    let frame = submit_frame(
        handle.companion_wire(),
        Some(0),
        None,
        "local-secret",
        "remember my key test-bearer",
        live.connection_id,
    );
    let responses = handle.handle_frame(frame, live.clone(), &transport).await;
    assert_stream_completed(&responses);
    handle.run_pending_learning(&transport).await;

    // Nothing that reaches the model may carry the registered value: the
    // dialogue prompt and the formation prompt are both recorded.
    for (position, input) in transport.inputs().iter().enumerate() {
        assert!(
            !input.contains("test-bearer"),
            "provider input {position} must not carry the credential"
        );
    }
    // Durable History holds the redacted owner input too.
    let companion = handle.store.ensure_running_companion().await.unwrap();
    let timeline = handle
        .store
        .load_timeline(companion, None, 10)
        .await
        .unwrap();
    let owner = timeline
        .iter()
        .find(|item| item.role == ene_companion::HistoryRole::Owner)
        .expect("the owner row is durable");
    assert!(
        !owner.text.contains("test-bearer"),
        "a registered credential never reaches History: {}",
        owner.text
    );
    assert!(
        owner.text.contains("[credential]"),
        "the History occurrence is visibly redacted"
    );

    let memories = handle
        .store
        .list_current_memories(companion.as_raw(), 10)
        .await
        .unwrap();
    assert_eq!(memories.len(), 1);
    assert!(
        !memories[0].content.contains("test-bearer"),
        "a registered credential never reaches Memory: {}",
        memories[0].content
    );
    assert!(
        memories[0].content.contains("[credential]"),
        "the credential position is visibly redacted"
    );
    let revisions = handle
        .store
        .list_memory_revisions(memories[0].id)
        .await
        .unwrap();
    let summary = handle
        .store
        .load_summary(revisions[0].summary.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(
        !summary.content.contains("test-bearer"),
        "a registered credential never reaches Summary: {}",
        summary.content
    );
}

/// A Learning pass that blocks must not hold the client-visible completion:
/// the reply is durable and streamed first, and the queued pass runs after.
#[tokio::test]
async fn learning_latency_never_delays_the_client_visible_completion() {
    use std::sync::Arc;
    use std::time::Duration;

    let transport = Arc::new(BlockingLearningTransport {
        reply: String::from("noted"),
        learning_started: Arc::new(tokio::sync::Notify::new()),
        release: Arc::new(tokio::sync::Notify::new()),
    });
    let live = live_input("client-decoupled");
    let setup = round_test_handle("dlg-decoupled", &live, transport.as_ref()).await;
    let (handle, _dir) = setup.unwrap();
    assert!(assign_learning(&handle, &live, transport.as_ref()).await);
    let frame = submit_frame(
        handle.companion_wire(),
        Some(0),
        None,
        "local-decoupled",
        "remember this",
        live.connection_id,
    );
    let responses = tokio::time::timeout(
        Duration::from_secs(5),
        handle.handle_frame(frame, live.clone(), transport.as_ref()),
    )
    .await
    .expect("client-visible completion must not wait for learning");
    assert_stream_completed(&responses);

    // The queued pass then runs (and blocks until released), proving the
    // response was produced independently of it.
    let handle = Arc::new(handle);
    let worker_handle = Arc::clone(&handle);
    let worker_transport = Arc::clone(&transport);
    let worker = tokio::spawn(async move {
        worker_handle
            .run_pending_learning(worker_transport.as_ref())
            .await;
    });
    tokio::time::timeout(
        Duration::from_secs(5),
        transport.learning_started.notified(),
    )
    .await
    .expect("the queued formation must start once drained");
    transport.release.notify_one();
    worker.await.unwrap();
}

/// The queue carries the Experience premise pinned at reply completion, so a
/// delayed worker cannot silently widen the pass to later turns. Coalescing
/// is not used: every completed turn keeps its own source range and
/// correspondence.
#[tokio::test]
async fn queued_experience_keeps_its_completion_premise() {
    use ene_companion::{CompanionRepository as _, HistoryRepository as _};
    use ene_presence::PresenceRepository as _;
    use ene_primitive::WallClockWithTz;

    let transport = LearningAwareTransport::new("noted", None);
    let live = live_input("client-premise");
    let setup = round_test_handle("dlg-premise", &live, &transport).await;
    let (handle, _dir) = setup.unwrap();
    assert!(assign_learning(&handle, &live, &transport).await);

    for (round, local, text) in [
        (0_u64, "local-premise-1", "first turn"),
        (1_u64, "local-premise-2", "second turn"),
    ] {
        let frame = submit_frame(
            handle.companion_wire(),
            Some(round),
            None,
            local,
            text,
            live.connection_id,
        );
        let responses = handle.handle_frame(frame, live.clone(), &transport).await;
        assert_stream_completed(&responses);
    }

    let queued = handle.pending_learning_premises();
    assert_eq!(queued.len(), 2, "one pinned premise per completed turn");
    let expected_client = device_client(&live.client_ref).as_raw();
    for premise in &queued {
        assert_eq!(
            premise.source.kind,
            ene_learning::ExperienceSourceKind::Dialogue
        );
        assert_eq!(
            premise.correspondence.client,
            Some(expected_client),
            "the Client correspondence survives the queue"
        );
        assert!(
            premise.correspondence.round.is_some() && premise.correspondence.generation.is_some(),
            "round and continuity survive the queue: {premise:?}"
        );
    }
    let (first, second) = (&queued[0], &queued[1]);
    assert_ne!(
        (first.source.start, first.source.end),
        (second.source.start, second.source.end),
        "each turn keeps its own source boundary"
    );

    // A later turn lands durably before the worker drains. It must not be
    // folded into either pinned premise.
    let companion = handle.store.ensure_running_companion().await.unwrap();
    let generation = handle
        .store
        .load_attribution(companion.as_raw())
        .await
        .unwrap()
        .unwrap()
        .generation;
    handle
        .store
        .append_message(ene_companion::AppendHistoryCommand {
            companion,
            round: RawId::new(),
            role: ene_companion::HistoryRole::Owner,
            text: String::from("later turn must not leak"),
            lang: String::from("en"),
            at: WallClockWithTz::now(),
            expected_generation: generation,
            expected_consent: None,
            expected_credential_set: None,
            command_id: None,
            round_wire: None,
            round_intent: None,
            incarnation: None,
            local_id: None,
        })
        .await
        .unwrap();

    handle.run_pending_learning(&transport).await;
    let prompts: Vec<String> = transport
        .inputs()
        .into_iter()
        .filter(|input| input.contains("learning formation pass"))
        .collect();
    assert_eq!(prompts.len(), 2, "one formation call per pinned premise");
    assert!(
        prompts[0].contains("first turn") && !prompts[0].contains("later turn must not leak"),
        "the first premise must not absorb the later turn: {}",
        prompts[0]
    );
    assert!(
        prompts[1].contains("second turn") && !prompts[1].contains("later turn must not leak"),
        "the second premise must not absorb the later turn: {}",
        prompts[1]
    );
}

/// A string already stored before the credential became registered must be
/// redacted when the Owner approves the credential: registration establishes
/// the non-exposure contract over existing durable state, not just over
/// future writes.
#[tokio::test]
async fn approving_a_credential_redacts_its_prior_occurrences() {
    use ene_companion::{CompanionRepository as _, HistoryRepository as _};
    use ene_presence::PresenceRepository as _;
    use ene_primitive::WallClockWithTz;

    let (handle, _dir) = setup_handle("dlg-sweep").await.unwrap();
    let live = live_input("client-sweep");
    let transport = ok_transport();
    let registered = handle
        .handle_frame(
            intent_frame(
                ManagementIntentKind::ConfigureCredentialIntent,
                "credential:openai:main",
                "consent-none",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert!(
        matches!(
            registered.first().map(|answer| &answer.payload),
            Some(WirePayload::ManagementOutcome(
                ManagementOutcome::HeldByOperation
            ))
        ),
        "the registration must wait as a pending approval"
    );
    let companion = handle.store.ensure_running_companion().await.unwrap();
    let generation = handle
        .store
        .load_attribution(companion.as_raw())
        .await
        .unwrap()
        .unwrap()
        .generation;
    let appended = handle
        .store
        .append_message(ene_companion::AppendHistoryCommand {
            companion,
            round: RawId::new(),
            role: ene_companion::HistoryRole::Owner,
            text: String::from("my old key was test-bearer"),
            lang: String::from("en"),
            at: WallClockWithTz::now(),
            expected_generation: generation,
            expected_consent: None,
            expected_credential_set: None,
            command_id: None,
            round_wire: None,
            round_intent: None,
            incarnation: None,
            local_id: None,
        })
        .await;
    assert!(
        matches!(
            appended,
            Ok(ene_companion::HistoryAppendOutcome::CommittedAs { .. })
        ),
        "the pre-registration row must commit, got {appended:?}"
    );

    assert!(
        matches!(handle.approve_credential("openai", "main").await, Ok(true)),
        "the approval must succeed and sweep before making the ref usable"
    );

    let timeline = handle
        .store
        .load_timeline(companion, None, 10)
        .await
        .unwrap();
    let row = timeline
        .iter()
        .find(|item| item.text.contains("old key"))
        .expect("the prior row is still held");
    assert!(
        !row.text.contains("test-bearer"),
        "approval must sweep the prior plaintext occurrence: {}",
        row.text
    );
    assert!(row.text.contains("[credential]"));
}

/// A value change is adopted only at a Host boundary: the next start pins
/// the new value, sweeps its durable occurrences, and advances the revision
/// before serving, so a restart never uses a new bearer with stale content
/// and never keeps serving content prepared under the old set.
#[tokio::test]
async fn restart_sweeps_and_advances_before_the_new_value_is_used() {
    use ene_companion::{CompanionRepository as _, HistoryRepository as _};
    use ene_credential::{
        CredentialRefRepository as _, CredentialSetRepository as _, EnvCredentialStore,
    };
    use ene_learning::SecretScrubber as _;
    use ene_presence::PresenceRepository as _;
    use ene_primitive::WallClockWithTz;

    let dir = tempfile::tempdir().expect("test scratch directory must be creatable");
    let companion_store = EnvCredentialStore::from_lookup(|_| Some(String::from("test-bearer")));
    let first = HostHandle::open_with_cred_store(dir.path(), CredStore::Env(companion_store))
        .await
        .expect("the first open must succeed");
    first
        .store
        .save_ref(CredentialRef::new("openai", "main").expect("valid test fixture"))
        .await
        .expect("the ref must register");
    let companion = first.store.ensure_running_companion().await.unwrap();
    let generation = first
        .store
        .load_attribution(companion.as_raw())
        .await
        .unwrap()
        .unwrap()
        .generation;
    // The future value already sits in durable content (for example, typed
    // as ordinary text while the old value was still pinned). The running
    // Host cannot know it yet, so the row keeps the plaintext.
    let old_revision = first.store.current_set_revision().await.unwrap();
    first
        .store
        .append_message(ene_companion::AppendHistoryCommand {
            companion,
            round: RawId::new(),
            role: ene_companion::HistoryRole::Owner,
            text: String::from("the rotated key is rotated-bearer"),
            lang: String::from("en"),
            at: WallClockWithTz::now(),
            expected_generation: generation,
            expected_consent: None,
            expected_credential_set: Some(old_revision),
            command_id: None,
            round_wire: None,
            round_intent: None,
            incarnation: None,
            local_id: None,
        })
        .await
        .expect("the row commits under the old set");
    drop(first);

    // Restart: construction pins the rotated value, and the startup sweep
    // must replace its occurrences and advance the revision before the
    // handle serves anything.
    let companion_store = EnvCredentialStore::from_lookup(|_| Some(String::from("rotated-bearer")));
    let restarted = HostHandle::open_with_cred_store(dir.path(), CredStore::Env(companion_store))
        .await
        .expect("the restarted open must succeed");
    let new_revision = restarted.store.current_set_revision().await.unwrap();
    assert!(
        new_revision > old_revision,
        "the startup boundary must advance the revision"
    );
    let timeline = restarted
        .store
        .load_timeline(companion, None, 10)
        .await
        .unwrap();
    let row = timeline
        .iter()
        .find(|item| item.text.contains("[credential]"))
        .expect("the swept row is still present");
    assert!(
        !row.text.contains("rotated-bearer"),
        "the startup sweep must redact the newly pinned value: {}",
        row.text
    );

    // A write prepared under the old revision is refused; a fresh scrub
    // names the new revision and commits normally.
    let stale = restarted
        .store
        .append_message(ene_companion::AppendHistoryCommand {
            companion,
            round: RawId::new(),
            role: ene_companion::HistoryRole::Owner,
            text: String::from("a second stale row"),
            lang: String::from("en"),
            at: WallClockWithTz::now(),
            expected_generation: generation,
            expected_consent: None,
            expected_credential_set: Some(old_revision),
            command_id: None,
            round_wire: None,
            round_intent: None,
            incarnation: None,
            local_id: None,
        })
        .await;
    assert_eq!(
        stale,
        Ok(ene_companion::HistoryAppendOutcome::StaleCredentialSet)
    );
    let scrubber = super::CredentialScrubber {
        refs: &restarted.store,
        store: &restarted.cred_store,
    };
    let fresh = scrubber
        .scrub("my key is rotated-bearer")
        .await
        .expect("the fresh scrub must prove absence");
    assert_eq!(fresh.credential_set, new_revision);
    assert!(!fresh.text.contains("rotated-bearer"));
    let committed = restarted
        .store
        .append_message(ene_companion::AppendHistoryCommand {
            companion,
            round: RawId::new(),
            role: ene_companion::HistoryRole::Owner,
            text: fresh.text,
            lang: String::from("en"),
            at: WallClockWithTz::now(),
            expected_generation: generation,
            expected_consent: None,
            expected_credential_set: Some(fresh.credential_set),
            command_id: None,
            round_wire: None,
            round_intent: None,
            incarnation: None,
            local_id: None,
        })
        .await;
    assert!(matches!(
        committed,
        Ok(ene_companion::HistoryAppendOutcome::CommittedAs { .. })
    ));
}

/// A Stage 2 environment can hold a registered CredentialRef together with
/// plaintext History stored before the scrub boundary. If the registered
/// value cannot be read at startup, the Host must not open at all: skipping
/// the sweep while advancing the revision would serve the plaintext under a
/// fresh set. With the value readable, the same startup must redact it and
/// advance the revision before serving.
#[tokio::test]
async fn startup_with_an_unreadable_registered_value_never_opens() {
    use ene_companion::{CompanionRepository as _, HistoryRepository as _};
    use ene_credential::{CredentialRefRepository as _, CredentialSetRepository as _};
    use ene_presence::PresenceRepository as _;
    use ene_primitive::WallClockWithTz;

    let dir = tempfile::tempdir().expect("test scratch directory must be creatable");
    let credential = CredentialRef::new("openai", "main").expect("valid test fixture");
    let readable = MemoryCredentialStore::new();
    readable.insert(credential.clone(), "test-bearer");
    let first = HostHandle::open_with_cred_store(dir.path(), CredStore::Memory(readable))
        .await
        .expect("the first open must succeed");
    first
        .store
        .save_ref(credential.clone())
        .await
        .expect("the ref must register");
    let companion = first.store.ensure_running_companion().await.unwrap();
    let generation = first
        .store
        .load_attribution(companion.as_raw())
        .await
        .unwrap()
        .unwrap()
        .generation;
    first
        .store
        .append_message(ene_companion::AppendHistoryCommand {
            companion,
            round: RawId::new(),
            role: ene_companion::HistoryRole::Owner,
            text: String::from("the legacy key is test-bearer"),
            lang: String::from("en"),
            at: WallClockWithTz::now(),
            expected_generation: generation,
            expected_consent: None,
            expected_credential_set: None,
            command_id: None,
            round_wire: None,
            round_intent: None,
            incarnation: None,
            local_id: None,
        })
        .await
        .expect("the legacy row must commit");
    let old_revision = first.store.current_set_revision().await.unwrap();
    drop(first);

    // The registered ref is unreadable: the open must fail closed instead of
    // sweeping nothing and advancing past the boundary.
    let missing = HostHandle::open_with_cred_store(
        dir.path(),
        CredStore::Memory(MemoryCredentialStore::new()),
    )
    .await;
    assert!(
        missing.is_err(),
        "an unreadable registered value must keep the Host closed"
    );

    // The failed open leaves both the revision and the plaintext untouched.
    let store = ene_store::Store::open(&dir.path().join("app.db"))
        .await
        .expect("the durable store stays readable");
    assert_eq!(
        store.current_set_revision().await,
        Ok(old_revision),
        "a failed open must not advance the credential-set revision"
    );
    let timeline = store.load_timeline(companion, None, 10).await.unwrap();
    let legacy = timeline
        .iter()
        .find(|item| item.text.contains("legacy key"))
        .expect("the legacy row is still held");
    assert_eq!(legacy.text, "the legacy key is test-bearer");
    drop(store);

    // With the value readable the startup sweep completes before serving.
    let present = MemoryCredentialStore::new();
    present.insert(credential, "test-bearer");
    let reopened = HostHandle::open_with_cred_store(dir.path(), CredStore::Memory(present))
        .await
        .expect("a readable registered value lets the Host open");
    assert!(
        reopened.store.current_set_revision().await.unwrap() > old_revision,
        "the successful startup boundary advances the revision"
    );
    let timeline = reopened
        .store
        .load_timeline(companion, None, 10)
        .await
        .unwrap();
    let swept = timeline
        .iter()
        .find(|item| item.text.contains("legacy key"))
        .expect("the swept row is still present");
    assert_eq!(swept.text, "the legacy key is [credential]");
}

/// A running Host never re-reads the environment: the value is pinned when
/// the store is constructed, so an external change is adopted only by the
/// next start, where the sweep and revision advance bracket the new value.
#[tokio::test]
async fn running_host_never_re_reads_the_environment() {
    use ene_credential::{CredentialRefRepository as _, EnvCredentialStore};
    use ene_learning::SecretScrubber as _;
    use std::cell::Cell;

    let dir = tempfile::tempdir().expect("test scratch directory must be creatable");
    let reads = Cell::new(0_u32);
    // The single construction read sees the old value; any later read would
    // see the rotated one, modelling an external rotation while Host runs.
    let companion_store = EnvCredentialStore::from_lookup(|_| {
        reads.set(reads.get() + 1);
        Some(String::from(if reads.get() == 1 {
            "test-bearer"
        } else {
            "rotated-bearer"
        }))
    });
    let handle = HostHandle::open_with_cred_store(dir.path(), CredStore::Env(companion_store))
        .await
        .expect("the open must succeed");
    handle
        .store
        .save_ref(CredentialRef::new("openai", "main").expect("valid test fixture"))
        .await
        .expect("the ref must register");
    let scrubber = super::CredentialScrubber {
        refs: &handle.store,
        store: &handle.cred_store,
    };
    let scrubbed = scrubber
        .scrub("my key is test-bearer")
        .await
        .expect("the pinned scrub must prove absence");
    assert!(!scrubbed.text.contains("test-bearer"));
    assert_eq!(
        reads.get(),
        1,
        "the running Host must not re-read the environment"
    );
    let unknown = scrubber
        .scrub("a note says rotated-bearer")
        .await
        .expect("the scrub must prove absence of the pinned value");
    assert!(
        unknown.text.contains("rotated-bearer"),
        "the rotated value is not the active bearer, so it stays untouched"
    );
}
