use super::AttachOutcome;
use crate::serve::{HostHandle, LiveInput, device_client};
use crate::test_support::{live_input, memory_handle_with};
use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome,
    ManagementViewRequest, RationaleOrigin,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::presence::{PresenceAttributionWire, PresenceStateWire};
use ene_api::v1::refs::{
    BaseViewMark, ClientIncarnationId, ClientLocalId, CommandWireId, CompanionWireRef,
    ConnectionWireId, ManagementTargetWire, RoundWireId, TextLangWire, WireMessageType,
};
use ene_api::v1::round::{
    ConfirmPresentationWire, HistoryResponse, PresentationStatus, RoundIntakeOutcomeWire,
    StreamClose, SubmitTextInput, TextBodyWire,
};
use ene_credential::CredentialRef;
use ene_inference::ProviderTransport;
use ene_inference::fake::{FakeFailure, FakeProviderTransport};
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
            confirmed: false,
        }),
    };
    stamped(frame, connection)
}

fn history_frame(companion: &str, connection: ConnectionWireId) -> ene_plugin_ipc::WireFrame {
    history_frame_filtered(companion, connection, None, 100, None)
}

fn history_frame_filtered(
    companion: &str,
    connection: ConnectionWireId,
    since: Option<String>,
    limit: u64,
    round: Option<RoundWireId>,
) -> ene_plugin_ipc::WireFrame {
    let frame = ene_plugin_ipc::WireFrame {
        envelope: new_outgoing_envelope(
            ProtocolVersion::V1,
            sender(),
            WireMessageType(String::from("HistoryRequest")),
        ),
        payload: WirePayload::HistoryRequest(ene_api::v1::round::HistoryRequest {
            companion: CompanionWireRef(companion.to_string()),
            since,
            limit,
            round,
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
            memory_after: None,
            memory_revisions_of: None,
            memory_revisions_after: None,
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

/// Splits the presence fact a committed summon-attach publishes (IPC §12.2)
/// off a submit's response batch.
///
/// The round tests below all start from `NoActive` and summon with their
/// first submit, so those batches open with the one fact that teaches the
/// Client the fresh generation — ahead of the summary it must ACK against.
/// The assertion here is only that a leading fact is unsolicited (no
/// `reply_to`), so a Client awaiting its answer never reads it as one; the
/// fact's count, position relative to the summary, and absence on the
/// non-attaching paths are pinned by the dedicated presence-publication
/// tests. Dropping it keeps the round assertions reading the domain answer.
fn split_presence_fact(
    responses: &[ene_plugin_ipc::WireFrame],
) -> (
    Option<PresenceAttributionWire>,
    &[ene_plugin_ipc::WireFrame],
) {
    let Some(head) = responses.first() else {
        return (None, responses);
    };
    let WirePayload::PresenceAttribution(fact) = &head.payload else {
        return (None, responses);
    };
    assert_eq!(
        head.envelope.correlation.reply_to, None,
        "the presence fact is unsolicited"
    );
    (Some(fact.clone()), &responses[1..])
}

fn accepted_round(responses: &[ene_plugin_ipc::WireFrame]) -> Result<RoundWireId, String> {
    let (_, answers) = split_presence_fact(responses);
    let Some(first) = answers.first() else {
        return Err(String::from("the submit must answer"));
    };
    match &first.payload {
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { round }) => {
            Ok(round.clone())
        }
        other => Err(format!("the submit must accept, got {other:?}")),
    }
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
        .load_timeline(companion, None, None, 100)
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

/// The durable attribution of the running companion, as the Host reads it
/// when it decides a submit.
async fn current_attribution(handle: &HostHandle) -> ene_presence::PresenceAttribution {
    use ene_companion::CompanionRepository as _;
    use ene_presence::PresenceRepository as _;

    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must resolve");
    handle
        .store
        .load_attribution(companion.as_raw())
        .await
        .expect("attribution must read")
        .expect("attribution must exist")
}

/// Commits one Companion reply while nobody is present, so the next summon
/// has an absence backlog to auto-present.
async fn append_absent_reply(
    handle: &HostHandle,
    text: &str,
    generation: ene_presence::PresenceGeneration,
) {
    use ene_companion::CompanionRepository as _;
    use ene_companion::{AppendHistoryCommand, HistoryRepository as _, HistoryRole};
    use ene_primitive::WallClockWithTz;

    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must resolve");
    let (outcome, registered) = handle
        .store
        .append_reply_with_undelivered(
            AppendHistoryCommand {
                companion,
                round: RawId::new(),
                role: HistoryRole::Companion,
                text: text.to_string(),
                lang: String::from("en"),
                at: WallClockWithTz::now(),
                expected_generation: generation,
                expected_consent: None,
                expected_credential_set: None,
                expected_owner_message: None,
                command_id: None,
                round_wire: Some(RawId::new().as_uuid().as_hyphenated().to_string()),
                round_intent: None,
                incarnation: None,
                local_id: None,
            },
            true,
            None,
        )
        .await
        .expect("the absent reply must commit");
    assert!(
        matches!(
            outcome,
            ene_companion::HistoryAppendOutcome::CommittedAs { .. }
        ),
        "the absent reply must commit, got {outcome:?}"
    );
    assert!(
        registered.is_some(),
        "the absent reply registers its undelivered correlation"
    );
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

/// The summon attach publishes the authoritative presence fact before the
/// absence summary it enables: one attach, one unsolicited fact, and a
/// summary stamped with the generation that fact taught. The Client can
/// only ACK the summary against what it observed, so the order decides
/// between a presented backlog and a stale refusal.
#[tokio::test]
async fn summon_attach_publishes_one_unsolicited_fact_ahead_of_the_summary() {
    use ene_api::v1::undelivered::UndeliveredResponse;

    let (handle, _dir) = setup_handle("dlg-summon-fact").await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-a");
    assert!(
        register_assign_complete(&handle, &live, &transport).await,
        "setup must complete"
    );
    // Nobody is present: one completion lands as an absence row, so the
    // summon below has a backlog to auto-present without an Owner query.
    let absent = current_attribution(&handle).await;
    assert_eq!(
        absent.state,
        ene_presence::PresenceState::NoActive,
        "a fresh handle starts with no active client"
    );
    append_absent_reply(&handle, "completion while absent", absent.generation).await;

    let submitted = submit_frame(
        handle.companion_wire(),
        Some(absent.generation.as_u64()),
        None,
        "local-summon",
        "hello",
        live.connection_id,
    );
    let submit_message = submitted.envelope.message_id;
    let responses = handle
        .handle_frame(submitted, live.clone(), &transport)
        .await;

    // One attach commits one transition, so exactly one fact is published,
    // and it opens the batch: everything carrying the fresh generation is
    // built on a fact the Client has not seen yet.
    let facts: Vec<&ene_plugin_ipc::WireFrame> = responses
        .iter()
        .filter(|frame| matches!(frame.payload, WirePayload::PresenceAttribution(_)))
        .collect();
    assert_eq!(
        facts.len(),
        1,
        "one attach publishes one fact, got {responses:?}"
    );
    let fact_frame = facts.first().unwrap();
    assert_eq!(
        fact_frame.envelope.correlation.reply_to, None,
        "the fact is unsolicited: it never answers the submit"
    );
    assert_eq!(
        fact_frame.envelope.sender.connection_id,
        Some(live.connection_id),
        "facts only flow on the authenticated connection"
    );
    let Some(WirePayload::PresenceAttribution(fact)) =
        responses.first().map(|frame| &frame.payload)
    else {
        panic!("the fact leads the batch, got {responses:?}");
    };
    assert_eq!(
        fact.generation,
        absent.generation.as_u64() + 1,
        "the fact carries the generation the attach just committed"
    );
    assert!(
        matches!(fact.state, PresenceStateWire::Present),
        "the summon moved the companion to present"
    );
    assert_eq!(fact.companion.0, handle.companion_wire());
    assert!(
        fact.active_client.is_some(),
        "the fact names this device as the active client"
    );
    let current = current_attribution(&handle).await;
    assert_eq!(
        current.active_client,
        Some(device_client("client-a")),
        "the durable attribution names the summoning device"
    );

    // The auto-presented summary follows the fact and carries the fresh
    // generation on its receipt: the Client echoes that generation, not the
    // pre-summon one, once it has absorbed the fact.
    let Some(WirePayload::UndeliveredResponse(UndeliveredResponse::Summary(summary))) =
        responses.get(1).map(|frame| &frame.payload)
    else {
        panic!("the summary follows the fact, got {responses:?}");
    };
    assert_eq!(
        summary.presence_generation, fact.generation,
        "the receipt is stamped with the generation the fact just taught"
    );
    assert_eq!(summary.items.len(), 1, "the absence row auto-presents");
    assert!(
        responses[1].envelope.correlation.reply_to.is_none(),
        "the auto-presented summary is unsolicited too"
    );

    // The submit has its own answer, correlated to it: the facts were
    // absorbed while the Client waited, never mistaken for that answer.
    let accepted = responses
        .iter()
        .find(|frame| matches!(frame.payload, WirePayload::RoundIntakeOutcome(_)))
        .expect("the submit answers");
    assert_eq!(
        accepted.envelope.correlation.reply_to,
        Some(submit_message),
        "the round answer correlates to the submit"
    );
    assert_eq!(
        accepted.envelope.sender.connection_id,
        Some(live.connection_id),
        "the answer echoes the connection"
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
    let (fact, answers) = split_presence_fact(&responses);
    assert_eq!(
        fact.map(|fact| (fact.state, fact.generation)),
        Some((PresenceStateWire::Present, 1)),
        "the attach publishes the fresh attribution ahead of its answers, got {responses:?}"
    );
    assert_eq!(
        answers.len(),
        5,
        "the winning attach emits accept, open, one delta, the final marker, close, got {responses:?}"
    );
    for response in &responses {
        assert_eq!(
            response.envelope.sender.connection_id,
            Some(live.connection_id),
            "every response echoes the connection"
        );
    }
    let accepted = answers.first().unwrap();
    let WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { round }) =
        &accepted.payload
    else {
        return;
    };
    let round = round.clone();
    let opened = answers.get(1).unwrap();
    assert!(
        matches!(
            &opened.payload,
            WirePayload::TextStreamOpen(open) if open.generation == 1
        ),
        "the stream opens at the freshly attached generation, got {:?}",
        opened.payload
    );
    let stream_frame = answers.get(2).unwrap();
    assert!(
        matches!(
            &stream_frame.payload,
            WirePayload::TextStreamFrame(frame) if !frame.is_final && frame.seq == 0
        ),
        "the fallback delta arrives at seq zero"
    );
    let final_frame = answers.get(3).unwrap();
    assert!(
        matches!(
            &final_frame.payload,
            WirePayload::TextStreamFrame(frame) if frame.is_final && frame.seq == 1 && frame.delta.is_empty()
        ),
        "the final marker closes the delta sequence"
    );
    let closed = answers.get(4).unwrap();
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
    let restored_frame = restored.first().unwrap();
    let WirePayload::HistoryResponse(HistoryResponse::Items(items)) = &restored_frame.payload
    else {
        return;
    };
    assert_eq!(items.len(), 2, "owner input plus reply restore");
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
    let second_frame = again.first().unwrap();
    let WirePayload::HistoryResponse(HistoryResponse::Items(second)) = &second_frame.payload else {
        return;
    };
    assert_eq!(second.len(), 2, "the replay appends nothing durable");
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
    let (fact, answers) = split_presence_fact(&accepted);
    assert!(
        fact.is_some_and(|fact| fact.generation == 1),
        "the first send attaches and publishes the fresh generation, got {accepted:?}"
    );
    assert!(
        answers.first().is_some_and(|first| matches!(
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
    // Nothing to attach: the client is already present, so this batch
    // carries no presence fact, only the answer and its interrupted stream.
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

/// Records provider inputs and returns the configured dialogue or formation reply.
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

/// Streaming provider that emits deltas and pauses after the first one until
/// released, so a test can observe delivery before completion. It honors a
/// sink abort the way production transports do: the read stops and the call
/// reports the abort instead of completing with a gap.
struct GatedStreamingTransport {
    deltas: Vec<String>,
    release: tokio::sync::Notify,
    completed: std::sync::atomic::AtomicBool,
}

impl GatedStreamingTransport {
    fn new(deltas: &[&str]) -> Self {
        Self {
            deltas: deltas.iter().map(|delta| (*delta).to_owned()).collect(),
            release: tokio::sync::Notify::new(),
            completed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    async fn release(&self) {
        self.release.notify_one();
    }

    fn completed(&self) -> bool {
        self.completed.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl ProviderTransport for GatedStreamingTransport {
    fn complete(
        &self,
        _req: ene_inference::ProviderRequest,
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
        let text = self.deltas.concat();
        Box::pin(async move { Ok(ene_inference::ProviderResponse { text, usage: None }) })
    }

    fn complete_streaming<'a>(
        &'a self,
        _req: ene_inference::ProviderRequest,
        sink: &'a mut (dyn ene_inference::DeltaSink + Send),
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        ene_inference::ProviderResponse,
                        ene_inference::InferenceTechnicalError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            for (index, delta) in self.deltas.iter().enumerate() {
                if let ene_inference::DeltaFlow::Abort(reason) = sink.push_delta(delta).await {
                    return Err(ene_inference::InferenceTechnicalError::StreamAborted {
                        reason: reason.to_owned(),
                    });
                }
                if index == 0 && self.deltas.len() > 1 {
                    self.release.notified().await;
                }
            }
            self.completed
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(ene_inference::ProviderResponse {
                text: self.deltas.concat(),
                usage: None,
            })
        })
    }
}

/// Acceptance and the first delta reach the sink before the provider call
/// completes, `seq` stays monotonic without gaps, and completion is reported
/// only after the durable reply exists.
#[tokio::test]
async fn streaming_emits_accept_and_first_delta_before_provider_completion() {
    let transport = GatedStreamingTransport::new(&["Hel", "lo"]);
    let live = live_input("client-stream");
    let (handle, _dir) = round_test_handle("dlg-stream", &live, &transport)
        .await
        .unwrap();
    let frame = submit_frame(
        handle.companion_wire(),
        Some(0),
        None,
        "local-stream",
        "hi",
        live.connection_id,
    );
    let (stream_tx, mut rx) = tokio::sync::mpsc::channel(crate::serve::STREAM_BUFFER_FRAMES);
    let mut sink = stream_tx.clone();
    let mut host =
        Box::pin(handle.handle_frame_to(frame, live.clone(), &transport, &mut sink, &stream_tx));

    let mut early = Vec::new();
    while early.len() < 4 {
        tokio::select! {
            biased;
            () = &mut host => panic!("the provider completed before the early frames"),
            maybe = rx.recv() => early.push(maybe.expect("frames must arrive")),
        }
    }
    assert!(
        matches!(
            &early[0].payload,
            WirePayload::PresenceAttribution(fact) if fact.generation == 1
        ),
        "the attach fact leads the batch"
    );
    assert!(
        matches!(
            &early[1].payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
        ),
        "the durable acceptance follows the presence fact"
    );
    assert!(
        matches!(&early[2].payload, WirePayload::TextStreamOpen(_)),
        "the stream opens before deltas"
    );
    let WirePayload::TextStreamFrame(first) = &early[3].payload else {
        panic!("the fourth frame is the first delta");
    };
    assert_eq!(first.delta, "Hel");
    assert_eq!(first.seq, 0);
    assert!(
        !transport.completed(),
        "the first delta must arrive while the provider call is still pending"
    );

    transport.release().await;
    host.await;
    drop(sink);
    drop(stream_tx);
    let mut frames = early;
    while let Some(frame) = rx.recv().await {
        frames.push(frame);
    }

    let mut sequences = Vec::new();
    let mut final_seen = false;
    let mut close = None;
    for frame in &frames {
        match &frame.payload {
            WirePayload::TextStreamFrame(delta) => {
                sequences.push(delta.seq);
                if delta.is_final {
                    final_seen = true;
                    assert!(
                        delta.delta.is_empty(),
                        "the final frame only closes the sequence"
                    );
                }
            }
            WirePayload::TextStreamClose(status) => close = Some(status.status),
            _ => {}
        }
    }
    assert_eq!(
        sequences,
        vec![0, 1, 2],
        "delta sequence numbers are monotonic and gap-free"
    );
    assert!(final_seen, "completion carries a final frame");
    assert_eq!(close, Some(StreamClose::Completed));
    assert_eq!(
        timeline_count(&handle).await.unwrap(),
        2,
        "the owner input and the adopted reply are durable"
    );
}

/// A provider transport that counts streaming calls without performing I/O.
struct CountingTransport {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    text: String,
}

impl CountingTransport {
    fn new(text: &str) -> Self {
        Self {
            calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            text: text.to_owned(),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl ene_inference::ProviderTransport for CountingTransport {
    fn complete(
        &self,
        _req: ene_inference::ProviderRequest,
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
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let text = self.text.clone();
        Box::pin(async move { Ok(ene_inference::ProviderResponse { text, usage: None }) })
    }

    fn complete_streaming<'a>(
        &'a self,
        _req: ene_inference::ProviderRequest,
        _sink: &'a mut (dyn ene_inference::DeltaSink + Send),
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        ene_inference::ProviderResponse,
                        ene_inference::InferenceTechnicalError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let text = self.text.clone();
        Box::pin(async move { Ok(ene_inference::ProviderResponse { text, usage: None }) })
    }
}

/// The accepted round wire from a submit's response batch.
fn accepted_round_wire(frames: &[ene_plugin_ipc::WireFrame]) -> Result<RoundWireId, String> {
    frames
        .iter()
        .find_map(|frame| match &frame.payload {
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { round }) => {
                Some(round.clone())
            }
            _ => None,
        })
        .ok_or_else(|| format!("no accept frame in {frames:?}"))
}

#[tokio::test]
async fn replacement_after_install_before_publication_publishes_nothing() -> Result<(), String> {
    accepted_replacement(false).await?;
    accepted_replacement(true).await
}

async fn accepted_replacement(after_install: bool) -> Result<(), String> {
    let live = live_input("accepted-replacement");
    let (handle, _dir) = round_test_handle("accepted-replacement", &live, &ok_transport()).await?;
    assert!(matches!(
        handle
            .attach_presence(
                &live,
                "accepted-replacement",
                true,
                ene_presence::PresenceState::NoActive,
                ene_presence::PresenceGeneration::from_u64(0)
            )
            .await,
        AttachOutcome::Attached(_)
    ));
    let gate = if after_install {
        handle.arm_submit_publish_gate()
    } else {
        handle.arm_submit_open_gate()
    };
    let transport = CountingTransport::new("durable reply");
    let request = submit_frame(
        handle.companion_wire(),
        Some(1),
        None,
        "accepted-old",
        "durable owner",
        live.connection_id,
    );
    let pending = handle.handle_frame(request, live.clone(), &transport);
    tokio::pin!(pending);
    tokio::select! {
        () = gate.wait_entered() => {},
        frames = &mut pending => panic!("escaped gate: {frames:?}"),
    }
    assert_eq!(timeline_count(&handle).await?, 1);
    assert_eq!(handle.has_open_round_for_test(), after_install);
    let c2 = live.authority.note_accept();
    crate::test_support::authenticate(&live.authority, &c2, "accepted-replacement");
    handle.on_connection_superseded(&live.connection_id);
    let live2 = live.authority.snapshot(&c2).expect("C2 is current");
    handle.disarm_submit_open_gate();
    handle.disarm_submit_publish_gate();
    gate.release();
    let frames = pending.await;
    assert!(
        frames.is_empty(),
        "no Accepted/Open/delta/close on old connection: {frames:?}"
    );
    assert!(!handle.has_open_round_for_test());
    assert_eq!(
        transport.calls(),
        1,
        "accepted work continues independently of publication"
    );
    assert_eq!(
        timeline_count(&handle).await?,
        2,
        "Owner and adopted reply remain durable"
    );
    let next = handle
        .handle_frame(
            submit_frame(
                handle.companion_wire(),
                Some(1),
                None,
                "accepted-new",
                "fresh C2",
                c2,
            ),
            live2,
            &ok_transport(),
        )
        .await;
    assert!(
        accepted_round_wire(&next).is_ok(),
        "C2 admits a fresh request"
    );
    Ok(())
}

mod credential_suite;
mod task_agent;
mod task_control;
mod task_run;

/// Stage 6 A3c: a Targeted Deletion condition that becomes durable while a
/// reply is streaming stops the remaining deltas and refuses the assembled
/// reply, so the payload is neither displayed nor appended to History.
#[tokio::test]
async fn a3c_a_deletion_mid_stream_stops_deltas_and_reply_adoption() {
    use crate::targeted_deletion::TargetedDeletionPass;
    use ene_preservation::{
        DeletionPurpose, DeletionSearchMaterial, MechanicalDeletionTarget, ParticipantOwnerRef,
        PreservationRepository as _, StartTargetedDeletionCommand, StartTargetedDeletionOutcome,
        TargetedDeletionTarget,
    };

    let transport = GatedStreamingTransport::new(&["Hel", "lo deleted body"]);
    let live = live_input("client-stream-erasure");
    let (handle, _dir) = round_test_handle("dlg-stream-erasure", &live, &transport)
        .await
        .unwrap();
    let frame = submit_frame(
        handle.companion_wire(),
        Some(0),
        None,
        "local-stream-erasure",
        "hi",
        live.connection_id,
    );
    let (stream_tx, mut rx) = tokio::sync::mpsc::channel(crate::serve::STREAM_BUFFER_FRAMES);
    let mut sink = stream_tx.clone();
    let mut host =
        Box::pin(handle.handle_frame_to(frame, live.clone(), &transport, &mut sink, &stream_tx));
    let mut early = Vec::new();
    while early.len() < 4 {
        tokio::select! {
            biased;
            () = &mut host => panic!("the provider completed before the early frames"),
            maybe = rx.recv() => early.push(maybe.expect("frames must arrive")),
        }
    }
    let WirePayload::TextStreamFrame(first) = &early[3].payload else {
        panic!("the fourth frame is the first delta");
    };
    assert_eq!(
        first.delta, "Hel",
        "the first delta is displayed before the condition"
    );

    // The condition becomes durable, then the Host transient holder is
    // demanded: the in-flight stream and its assembled reply are invalidated.
    let command = StartTargetedDeletionCommand::new(
        TargetedDeletionTarget {
            mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                String::from("deleted body"),
            )),
            semantic_hints: Vec::new(),
        },
        DeletionPurpose::Privacy,
        ene_primitive::WallClockWithTz::now(),
        Vec::new(),
        vec![ParticipantOwnerRef::HostTransient],
    )
    .confirmed_for_tests();
    assert!(matches!(
        handle
            .store
            .start_targeted_deletion(command)
            .await
            .expect("admission commits"),
        StartTargetedDeletionOutcome::Started(_)
    ));
    let outcome = handle
        .drive_targeted_deletion(TargetedDeletionPass::new(100, 4))
        .await
        .expect("the pass runs");
    assert!(
        outcome.verified >= 1,
        "the host-transient demand verified its bounded work: {outcome:?}"
    );

    transport.release().await;
    host.await;
    drop(sink);
    drop(stream_tx);
    let mut frames = early;
    while let Some(frame) = rx.recv().await {
        frames.push(frame);
    }

    let deltas: Vec<&str> = frames
        .iter()
        .filter_map(|frame| match &frame.payload {
            WirePayload::TextStreamFrame(delta) => Some(delta.delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        deltas,
        vec!["Hel"],
        "no delta after the condition is displayed: {deltas:?}"
    );
    let close = frames.iter().find_map(|frame| match &frame.payload {
        WirePayload::TextStreamClose(close) => Some(close.status),
        _ => None,
    });
    assert_eq!(
        close,
        Some(StreamClose::Interrupted),
        "the stream fails closed instead of completing a covered reply"
    );
    assert_eq!(
        timeline_count(&handle).await.unwrap(),
        1,
        "the owner input is durable and the covered reply is never adopted"
    );
}

/// A4: an Owner submit whose body is under a canonical current condition is
/// held at the intake boundary — no History row, no round acceptance.
///
/// The matching "fresh Owner input after completion" ordering is asserted at
/// the canonical boundary (the store's `delayed_arrival` suite); the
/// end-to-end completion walk belongs to integration slice D, which owns the
/// A5 completion authority this slice does not have.
#[tokio::test]
async fn a4_a_covered_submit_is_held_without_a_history_row() {
    use ene_preservation::{
        DeletionPurpose, DeletionSearchMaterial, MechanicalDeletionTarget, ParticipantOwnerRef,
        PreservationRepository as _, StartTargetedDeletionCommand, StartTargetedDeletionOutcome,
        TargetedDeletionTarget,
    };
    let (handle, _dir) = setup_handle("dlg-a4-held").await.unwrap();
    let transport = ok_transport();
    let live = live_input("client-a4-held");
    assert!(
        register_assign_complete(&handle, &live, &transport).await,
        "setup must complete"
    );
    let command = StartTargetedDeletionCommand::new(
        TargetedDeletionTarget {
            mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                String::from("the private key"),
            )),
            semantic_hints: Vec::new(),
        },
        DeletionPurpose::Privacy,
        ene_primitive::WallClockWithTz::now(),
        Vec::new(),
        vec![ParticipantOwnerRef::Companion],
    )
    .confirmed_for_tests();
    match handle
        .store
        .start_targeted_deletion(command)
        .await
        .expect("admission commits")
    {
        StartTargetedDeletionOutcome::Started(_) => {}
        other => panic!("the operation must start, got {other:?}"),
    }

    let responses = handle
        .handle_frame(
            submit_frame(
                handle.companion_wire(),
                Some(0),
                None,
                "local-a4",
                "please keep the private key",
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    let (_, answers) = split_presence_fact(&responses);
    assert!(
        answers.iter().all(|frame| !matches!(
            frame.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
        )),
        "a covered submit is never accepted, got {responses:?}"
    );
    assert!(
        answers.iter().any(|frame| matches!(
            frame.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::HeldForTransition)
        )),
        "the intake answers a retry-later hold, got {responses:?}"
    );
    assert_eq!(
        timeline_count(&handle).await.unwrap(),
        0,
        "no History row exists for the held submit"
    );
}
