//! Stage 6 slice D: first-party E2E over the real transport for Targeted
//! Deletion, usage / cost / cap, and credential secret non-exposure.
//!
//! Every leg starts from the production composition: a real listener
//! ([`ene_core::conn::run`]), the real `ene-ctl` [`Client`], the real store,
//! the real management inlets and the real preservation fan-out. Only the
//! provider transport is a controllable fake (scripted replies, per-call
//! barriers, injected usage and failures), never a substitute for a
//! first-party boundary.
//!
//! The file is platform-neutral on purpose: the transport-generic
//! `serve_connection` loop is what the Unix socket and the Windows named pipe
//! both drive, so the same suite runs on both required CI operating systems
//! over their own OS transport.
//!
//! Determinism: provider call order is controlled by per-call barriers and
//! observation (`wait_sends`), never by sleeps that assume ordering; race
//! legs park a supplier at a barrier and commit the other premise while it is
//! held.
//!
//! Coverage map:
//!
//! - E2E 1 (Targeted Deletion): request → Host-local confirmation → bounded
//!   fan-out → finalizing → completed through the real socket, with the
//!   target planted in History, Learning Summary / Memory current + past
//!   revision, Task instruction / result / report, the control/metadata
//!   journal, the workspace path copies, and an undelivered presentation
//!   transient; completed-state scans, search-material destruction, and the
//!   fresh-origin acceptance are asserted after the operation.
//! - E2E 1 Client participant: a Client that received a target-bearing copy
//!   is snapshotted as a required `ClientIncarnation` by the serving Host's
//!   first-party confirmation inlet; its local-erasure answer verifies the
//!   participant, an unreachable Client keeps the operation `Held`, a
//!   disconnect or replacement connection alone completes nothing, and an
//!   already-snapshotted participant survives a Host restart until the
//!   Client's own local erasure (lifecycle §8.1).
//! - E2E 1 races: provider wait and deletion condition in both orders;
//!   Learning formation and deletion condition in both orders; presentation
//!   ACK after the condition is a domain hold, not a Presented write.
//! - E2E 1 restart: an unfinished operation survives Host restart in
//!   `active` and in `finalizing` and is never completed by the restart.
//! - E2E 2 (Usage / Cost / Cap): Reported input/cached/output tokens and the
//!   cost breakdown from the first-party query for dialogue, learning and
//!   Task Agent calls, Unknown distinct from Reported, concurrent admission
//!   for the last cap slot with a zero-byte refusal, `ResponseLost` Unknown
//!   counted across restart, and cap update currentness / replay.
//! - E2E 3 (Credential safety): a registered secret never reaches provider
//!   request bodies, History, Memory, Task data, presentation bodies, or the
//!   frames, errors, and debug renderings this process captures; a rotation
//!   during a parked provider wait leaves no raw value durable.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration-test helpers outside #[test] functions need the fixture allowances clippy.toml grants only to test functions"
)]

use std::collections::BTreeSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ene_api::v1::deletion::{
    DeletionParticipantReportWire, DeletionParticipantStatusWire, DeletionPhaseWire,
    DeletionPurposeWire, DeletionStatusPage, DeletionStatusRequest, DeletionStatusResponse,
};
use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome,
    RationaleOrigin, workspace_target,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::StreamWireId;
use ene_api::v1::refs::{BaseViewMark, CommandWireId, ManagementTargetWire, RoundWireId};
use ene_api::v1::round::{HistoryResponse, PresentationStatus, RoundIntakeOutcomeWire};
use ene_api::v1::undelivered::{
    TaskListPage, TaskListResponse, UndeliveredAckOutcome, UndeliveredResponse, UndeliveredSummary,
};
use ene_api::v1::usage::{
    UsageCapConsumptionView, UsageSummaryPage, UsageSummaryRequest, UsageSummaryResponse,
};
use ene_core::conn;
use ene_core::serve::{CoreError, CredStore, HostHandle};
use ene_credential::{CredentialRef, CredentialSetRepository as _, MemoryCredentialStore};
use ene_ctl::client::Client;
use ene_ctl::cmds;
use ene_ctl::device::{StoredDevice, load_pending_id, store_device};
use ene_ctl::errors::CliError;
use ene_inference::cost::UsageEstimate;
use ene_inference::{ProviderRequest, ProviderResponse, ProviderTransport, RawUsage};
use ene_preservation::{ConfirmTargetedDeletionOutcome, DeletionOperationRef};
use ene_primitive::{RawId, WallClockWithTz};

const DESCRIPTOR: &str = "stage6 e2e";
/// A reviewed first-party route, so the usage surface can project a cost.
const MODEL: &str = "gpt-4o-mini";
/// The mechanical keyword the Targeted Deletion E2E plants everywhere.
const TARGET: &str = "TS6-DELETION-CANARY-9137";
/// The credential value the secret-safety E2E registers and scans for.
const SECRET: &str = "sk-stage6-secret-marker-8821";
/// The value a rotation registers over [`SECRET`].
const ROTATED_SECRET: &str = "sk-stage6-rotated-marker-4477";

fn memory_store_with(secret: &str) -> MemoryCredentialStore {
    let store = MemoryCredentialStore::new();
    store.insert(
        CredentialRef::new("openai", "main").expect("valid test fixture"),
        secret,
    );
    store
}

fn memory_store() -> MemoryCredentialStore {
    memory_store_with("sk-test-only")
}

/// Two usable bearers: the registered `main` value and a not-yet-registered
/// `rotated` value a later approval promotes.
fn memory_store_with_rotated(main: &str, rotated: &str) -> MemoryCredentialStore {
    let store = memory_store_with(main);
    store.insert(
        CredentialRef::new("openai", "rotated").expect("valid test fixture"),
        rotated,
    );
    store
}

/// One scripted provider call.
#[derive(Clone)]
struct Call {
    reply: String,
    usage: Option<RawUsage>,
    /// The provider may have run but the response was lost.
    lost: bool,
}

impl Call {
    fn text(reply: impl Into<String>) -> Self {
        Self {
            reply: reply.into(),
            usage: None,
            lost: false,
        }
    }

    fn reported(reply: impl Into<String>, input: u64, cached: u64, output: u64) -> Self {
        Self {
            reply: reply.into(),
            usage: Some(RawUsage {
                input_tokens: input,
                cached_input_tokens: cached,
                output_tokens: output,
            }),
            lost: false,
        }
    }

    fn lost() -> Self {
        Self {
            reply: String::new(),
            usage: None,
            lost: true,
        }
    }
}

/// One matcher over the provider request's logical input. Content matching
/// (instead of FIFO) keeps a fixture deterministic when background work
/// (Learning formation, Task Agent turns) interleaves with dialogue calls.
type Matcher = Box<dyn Fn(&str) -> bool + Send + Sync>;

/// Matches the dialogue prompt whose *current* owner message is `text`: the
/// same words may legitimately remain in the recent-conversation window, so a
/// plain substring match would answer a later round with an earlier script.
fn on_latest_owner(text: &str) -> Matcher {
    let needle = format!("\nOwner: {text}");
    Box::new(move |input| input.ends_with(&needle))
}

/// Matches one Learning formation prompt; `create` selects the pass whose
/// prompt shows no existing memory.
fn on_learning_formation(create: bool) -> Matcher {
    Box::new(move |input| {
        input.contains("learning formation pass")
            && input.contains("Existing memories:\n(none)") == create
    })
}

/// Matches one Task Agent turn carrying `tool_calls` prior exchanges.
fn on_task_agent_turn(tool_calls: usize) -> Matcher {
    Box::new(move |input| {
        input.starts_with("[RESPONSE FORMAT]") && input.matches("[TOOL CALL]").count() == tool_calls
    })
}

/// Controllable provider fake: content-matched scripted calls, per-call
/// barriers, captured inputs, a send counter, and one fixed safe usage upper
/// bound.
///
/// A barrier is membership-only (it shrinks), so removing a call from
/// `blocks` releases it without a lost-wakeup race. Observation polls
/// (`wait_sends`) watch background completion; they are never an ordering
/// device for a race, which uses the barriers.
struct ScriptedTransport {
    scripts: Mutex<Vec<(Matcher, Call)>>,
    default_call: Mutex<Call>,
    blocks: Mutex<BTreeSet<usize>>,
    /// Content barriers: a request whose logical input matches one of these
    /// parks before answering, so a race commits its other premise while this
    /// exact call is provably held.
    block_matches: Mutex<Vec<Matcher>>,
    /// Calls currently parked on a barrier (observation for `wait_parked`).
    parked: AtomicUsize,
    inputs: Mutex<Vec<String>>,
    sends: AtomicUsize,
    estimate: Option<UsageEstimate>,
}

impl ScriptedTransport {
    fn new(scripts: Vec<(Matcher, Call)>, blocks: &[usize]) -> Self {
        Self {
            scripts: Mutex::new(scripts),
            default_call: Mutex::new(Call::text("acknowledged")),
            blocks: Mutex::new(blocks.iter().copied().collect()),
            block_matches: Mutex::new(Vec::new()),
            parked: AtomicUsize::new(0),
            inputs: Mutex::new(Vec::new()),
            sends: AtomicUsize::new(0),
            estimate: None,
        }
    }

    fn with_estimate(mut self, estimate: UsageEstimate) -> Self {
        self.estimate = Some(estimate);
        self
    }

    fn sends(&self) -> usize {
        self.sends.load(Ordering::SeqCst)
    }

    /// Parks every future call whose input matches `matcher` until
    /// [`Self::release_blocked`] runs.
    fn block_input(&self, matcher: Matcher) {
        self.block_matches
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(matcher);
    }

    fn release_blocked(&self) {
        self.block_matches
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    /// Observation poll for `wanted` calls parked on a content barrier.
    async fn wait_parked(&self, wanted: usize) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while self.parked.load(Ordering::SeqCst) < wanted {
            assert!(
                tokio::time::Instant::now() < deadline,
                "no provider call parked on the barrier"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn input_texts(&self) -> Vec<String> {
        self.inputs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl ProviderTransport for ScriptedTransport {
    fn complete(
        &self,
        req: ProviderRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<ProviderResponse, ene_inference::InferenceTechnicalError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            let call = self.sends.fetch_add(1, Ordering::SeqCst) + 1;
            let mut counted = false;
            loop {
                let held = self
                    .blocks
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .contains(&call)
                    || self
                        .block_matches
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .iter()
                        .any(|matcher| matcher(&req.input));
                if !held {
                    if counted {
                        self.parked.fetch_sub(1, Ordering::SeqCst);
                    }
                    break;
                }
                if !counted {
                    self.parked.fetch_add(1, Ordering::SeqCst);
                    counted = true;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            self.inputs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(req.input.clone());
            let matched = {
                let scripts = self
                    .scripts
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                scripts
                    .iter()
                    .find(|(matcher, _)| matcher(&req.input))
                    .map(|(_, call)| call.clone())
            };
            let call_script = matched.unwrap_or_else(|| {
                self.default_call
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone()
            });
            if call_script.lost {
                return Err(ene_inference::InferenceTechnicalError::ResponseLost);
            }
            Ok(ProviderResponse {
                text: call_script.reply,
                usage: call_script.usage,
            })
        })
    }

    fn usage_estimate(&self, _req: &ProviderRequest) -> Option<UsageEstimate> {
        self.estimate
    }
}

/// One provider reply carrying exactly one companion `[task-control]`
/// directive: the marker line must be the reply's first non-empty line.
fn task_reply(directive: serde_json::Value) -> String {
    format!("[task-control] {directive}")
}

async fn open_host(dir: &Path) -> Arc<HostHandle> {
    open_host_with(dir, memory_store()).await
}

async fn open_host_with(dir: &Path, store: MemoryCredentialStore) -> Arc<HostHandle> {
    let opened = HostHandle::open_with_cred_store(dir, CredStore::Memory(store)).await;
    assert!(opened.is_ok(), "host must open");
    Arc::new(opened.unwrap())
}

/// Dials the data directory until the listener takes the pairing request and
/// requires the typed pending outcome. A failed dial is a bounded
/// availability wait, never an ordering device.
async fn dial_until_pending(dir: &Path) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let attempt = tokio::time::timeout(
            Duration::from_secs(10),
            Client::connect(dir, DESCRIPTOR, "test"),
        )
        .await;
        match attempt {
            Ok(Err(CliError::ServerOutcome(_))) => return Ok(()),
            Ok(Err(CliError::Transport(reason))) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(format!("the listener never accepted a client: {reason}"));
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok(Err(other)) => return Err(format!("a first pairing must pend, got {other:?}")),
            Ok(Ok(_)) => {
                return Err(String::from(
                    "a first pairing must not authenticate before Owner approval",
                ));
            }
            Err(_) => return Err(String::from("the pairing dial timed out")),
        }
    }
}

async fn connect(dir: &Path) -> Client {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        match tokio::time::timeout(
            Duration::from_secs(10),
            Client::connect(dir, DESCRIPTOR, "test"),
        )
        .await
        {
            Ok(Ok(client)) => return client,
            Ok(Err(CliError::Transport(_))) => {}
            Ok(Err(other)) => panic!("connect answered {other:?}"),
            Err(_) => {}
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the client never connected"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn ask(client: &mut Client, payload: WirePayload, what: &str) -> Result<WirePayload, String> {
    match tokio::time::timeout(Duration::from_secs(20), client.request(payload)).await {
        Ok(Ok(answer)) => Ok(answer),
        Ok(Err(error)) => Err(format!("{what} errored: {error:?}")),
        Err(_) => Err(format!("{what} timed out")),
    }
}

async fn ask_stream(client: &mut Client) -> Result<WirePayload, String> {
    match tokio::time::timeout(Duration::from_secs(20), client.next_frame()).await {
        Ok(Ok(payload)) => Ok(payload),
        Ok(Err(error)) => Err(format!("stream frame errored: {error:?}")),
        Err(_) => Err(String::from("stream frame timed out")),
    }
}

/// Host-local Owner approval of every pending pairing plus the one-time secret
/// handover into the Client's device file, exactly as the trusted surface does
/// it (the secret never travels a normal payload).
async fn approve_and_provision(dir: &Path, approver: &HostHandle) -> Result<(), String> {
    let pendings = approver
        .pending_devices()
        .await
        .map_err(|error| format!("pendings must list: {error:?}"))?;
    let remembered = load_pending_id(dir);
    let mut ordered: Vec<_> = pendings.iter().collect();
    ordered.sort_by_key(|pending| remembered.as_deref() == Some(pending.pending_id.as_str()));
    assert!(
        !ordered.is_empty(),
        "a pairing request must leave a pending for the Owner to approve"
    );
    for pending in ordered {
        let approval = approver
            .approve_device(&pending.pending_id)
            .await
            .map_err(|error| format!("approve failed: {error:?}"))?;
        let Some((record, secret)) = approval else {
            return Err(format!("approval of {} must pair", pending.pending_id));
        };
        store_device(
            dir,
            &StoredDevice::new(
                record
                    .wire
                    .parse()
                    .map(ene_api::v1::refs::DeviceWireId)
                    .map_err(|error| format!("opaque wire must stay UUID text: {error:?}"))?,
                secret,
            ),
        )
        .map_err(|error| format!("device file must store: {error:?}"))?;
    }
    Ok(())
}

async fn view_mark(client: &mut Client) -> Result<String, String> {
    let answer = ask(
        client,
        WirePayload::ManagementViewRequest(cmds::setup_view_request()),
        "show",
    )
    .await?;
    let WirePayload::ManagementView(view) = answer else {
        return Err(format!("show must answer a view, got {answer:?}"));
    };
    Ok(view.mark.0.clone())
}

fn setup_complete_intent(target: &str, mark: &str) -> WirePayload {
    WirePayload::ManagementIntent(ManagementIntent {
        intent_id: CommandWireId(uuid::Uuid::new_v4()),
        kind: ManagementIntentKind::ManageRuleConsentCap,
        target: ManagementTargetWire(String::from(target)),
        base_view: BaseViewMark(String::from(mark)),
        rationale: IntentRationaleWire {
            origin: RationaleOrigin::ManagementSurface,
            quote: None,
        },
    })
}

/// Registers the credential, assigns every capability the E2E uses, and
/// completes setup.
async fn setup_flow(
    client: &mut Client,
    approver: &HostHandle,
    capabilities: &[&str],
) -> Result<(), String> {
    let mark = view_mark(client).await?;
    let register = ask(
        client,
        WirePayload::ManagementIntent(cmds::credential_intent(
            CommandWireId(uuid::Uuid::new_v4()),
            &BaseViewMark(mark),
            "openai",
        )),
        "register",
    )
    .await?;
    assert!(
        matches!(
            register,
            WirePayload::ManagementOutcome(ManagementOutcome::HeldByOperation)
        ),
        "unapproved register must hold, got {register:?}"
    );
    assert!(
        matches!(
            approver.approve_credential("openai", "main").await,
            Ok(true)
        ),
        "host-local credential approval must succeed"
    );
    for capability in capabilities {
        let mark = view_mark(client).await?;
        let assign = ask(
            client,
            WirePayload::ManagementIntent(cmds::assignment_intent(
                CommandWireId(uuid::Uuid::new_v4()),
                &BaseViewMark(mark),
                capability,
                "openai",
                MODEL,
            )),
            "assign",
        )
        .await?;
        assert!(
            matches!(
                assign,
                WirePayload::ManagementOutcome(ManagementOutcome::StoredAsRuleView { .. })
            ),
            "assign {capability} must store, got {assign:?}"
        );
    }
    let mark = view_mark(client).await?;
    let complete = ask(
        client,
        setup_complete_intent("setup:complete", &mark),
        "complete",
    )
    .await?;
    assert!(
        matches!(
            complete,
            WirePayload::ManagementOutcome(ManagementOutcome::AppliedAsOneTime)
        ),
        "complete must apply, got {complete:?}"
    );
    Ok(())
}

/// One served Host under test: the composition handle, the listener task, and
/// a live first-party client.
///
/// The client is optional because a restart must first close the current
/// connection: the Windows named-pipe listener creates the exclusive *first*
/// instance for the pipe name, so the previous connection's still-open server
/// instance would block the new listener from binding.
struct Served {
    dir: PathBuf,
    handle: Arc<HostHandle>,
    server: tokio::task::JoinHandle<Result<(), CoreError>>,
    client: Option<Client>,
    transport: Arc<ScriptedTransport>,
}

impl Served {
    async fn start(
        dir: PathBuf,
        cred_store: MemoryCredentialStore,
        transport: Arc<ScriptedTransport>,
        capabilities: &[&str],
    ) -> Self {
        let handle = open_host_with(&dir, cred_store).await;
        let server = tokio::spawn(conn::run(
            dir.clone(),
            Arc::clone(&handle),
            Arc::clone(&transport),
        ));
        dial_until_pending(&dir)
            .await
            .expect("listener must accept the first pairing");
        let approver = open_host(&dir).await;
        approve_and_provision(&dir, &approver)
            .await
            .expect("approval must pair");
        let mut client = connect(&dir).await;
        setup_flow(&mut client, &approver, capabilities)
            .await
            .expect("setup must complete");
        Self {
            dir,
            handle,
            server,
            client: Some(client),
            transport,
        }
    }

    /// The live first-party client.
    fn client(&mut self) -> &mut Client {
        self.client.as_mut().expect("a live client")
    }

    /// Closes the current connection and stops the listener. The state stays
    /// open on the current handle, exactly like a Host process that stopped
    /// serving.
    async fn stop(&mut self) {
        self.server.abort();
        self.client = None;
        tokio::task::yield_now().await;
        drop(std::fs::remove_file(conn::socket_path(&self.dir)));
    }

    /// Opens the state again, runs the production startup mutations, and
    /// serves; returns a fresh authenticated client.
    ///
    /// The OS transport may still be releasing the previous connection's
    /// server instance (the Windows named-pipe listener owns the exclusive
    /// first instance for the pipe name), so a listener that exits immediately
    /// is retried until it stays up.
    async fn serve(&mut self) -> Client {
        let handle = open_host(&self.dir).await;
        handle
            .run_startup_mutations()
            .await
            .expect("restart startup must complete");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let mut server = tokio::spawn(conn::run(
                self.dir.clone(),
                Arc::clone(&handle),
                Arc::clone(&self.transport),
            ));
            // Bind is the listener's first await point: a task that exits
            // within this window could not bind (or lost the singleton race).
            if let Ok(outcome) = tokio::time::timeout(Duration::from_millis(250), &mut server).await
            {
                let failure = outcome.expect("the listener task must not panic");
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the listener never bound after the restart: {failure:?}"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            self.handle = handle;
            self.server = server;
            return connect(&self.dir).await;
        }
    }

    /// Stops and serves again, mirroring a Host process restart.
    async fn restart(&mut self) -> Client {
        self.stop().await;
        self.serve().await
    }

    /// Aborts the listener and returns the canonical mechanical remainder the
    /// production completion boundary verifies, read through a fresh store on
    /// the quiesced database.
    async fn canonical_remainder(&self, needle: &str) -> u64 {
        let store = ene_store::Store::open(&self.dir.join("app.db"))
            .await
            .expect("the state database opens");
        store
            .count_exact_text_remainder_for_tests(needle)
            .await
            .expect("the canonical remainder probe must answer")
    }
}

/// Serves `dir`, pairs the first device, and completes setup with the standard
/// test credential.
async fn serve_and_setup(
    dir: PathBuf,
    transport: Arc<ScriptedTransport>,
    capabilities: &[&str],
) -> Served {
    Served::start(dir, memory_store(), transport, capabilities).await
}

/// One conversation round over the socket: submit, require acceptance, drain
/// the stream to completion. The raw variant also returns the close status, so
/// a race leg can observe a fail-closed interruption instead of completion.
async fn send_round_raw(
    client: &mut Client,
    text: &str,
) -> Result<
    (
        String,
        Option<StreamWireId>,
        String,
        ene_api::v1::round::StreamClose,
    ),
    String,
> {
    let companion = client.companion_ref();
    let send = ask(
        client,
        WirePayload::SubmitTextInput(cmds::submit_input(
            &companion,
            None,
            false,
            String::from(text),
            String::from("en"),
        )),
        "send",
    )
    .await?;
    let WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { round }) = send
    else {
        return Err(format!("input must be accepted, got {send:?}"));
    };
    let round_wire = round.0.clone();
    let mut text_out = String::new();
    let mut previous_seq: Option<u64> = None;
    let mut stream_id = None;
    loop {
        match ask_stream(client).await? {
            WirePayload::TextStreamOpen(open) => {
                stream_id = Some(open.stream);
            }
            WirePayload::TextStreamFrame(frame) => {
                assert!(
                    previous_seq.is_none_or(|previous| frame.seq == previous + 1),
                    "stream frames must order by seq"
                );
                previous_seq = Some(frame.seq);
                text_out.push_str(&frame.delta);
            }
            WirePayload::TextStreamClose(close) => {
                return Ok((round_wire, stream_id, text_out, close.status));
            }
            other => return Err(format!("unexpected stream payload: {other:?}")),
        }
    }
}

/// The completed-round convenience wrapper used by fixture seeding.
async fn send_round(
    client: &mut Client,
    text: &str,
) -> Result<(String, Option<StreamWireId>, String), String> {
    let (round, stream, text_out, close) = send_round_raw(client, text).await?;
    assert_eq!(
        close,
        ene_api::v1::round::StreamClose::Completed,
        "the fixture round must complete"
    );
    Ok((round, stream, text_out))
}

/// Submits one input that a current erasure condition must hold at intake.
async fn submit_expect_hold(client: &mut Client, text: &str) -> Result<(), String> {
    let companion = client.companion_ref();
    let answer = ask(
        client,
        WirePayload::SubmitTextInput(cmds::submit_input(
            &companion,
            None,
            false,
            String::from(text),
            String::from("en"),
        )),
        "held submit",
    )
    .await?;
    match answer {
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::HeldForTransition) => Ok(()),
        other => Err(format!("a covered submit must hold, got {other:?}")),
    }
}

async fn confirm_round(client: &mut Client, round_wire: &str, stream_id: Option<StreamWireId>) {
    let notified = tokio::time::timeout(
        Duration::from_secs(20),
        client.notify(WirePayload::ConfirmPresentation(
            ene_api::v1::round::ConfirmPresentationWire {
                round: RoundWireId(String::from(round_wire)),
                stream: stream_id,
                status: PresentationStatus::Presented,
                detail: None,
            },
        )),
    )
    .await;
    assert!(
        matches!(notified, Ok(Ok(()))),
        "confirm must send, got {notified:?}"
    );
}

async fn select_workspace(client: &mut Client, path: &Path) -> Result<(), String> {
    let mark = view_mark(client).await?;
    let answer = ask(
        client,
        WirePayload::ManagementIntent(ManagementIntent {
            intent_id: CommandWireId(uuid::Uuid::new_v4()),
            kind: ManagementIntentKind::SelectWorkspace,
            target: workspace_target(&path.to_string_lossy()),
            base_view: BaseViewMark(mark),
            rationale: IntentRationaleWire {
                origin: RationaleOrigin::ManagementSurface,
                quote: None,
            },
        }),
        "select-workspace",
    )
    .await?;
    assert!(
        matches!(
            answer,
            WirePayload::ManagementOutcome(ManagementOutcome::AppliedAsOneTime)
        ),
        "workspace selection must apply, got {answer:?}"
    );
    Ok(())
}

async fn fetch_summary(client: &mut Client, what: &str) -> Result<UndeliveredSummary, String> {
    let answer = ask(
        client,
        WirePayload::UndeliveredRequest(cmds::undelivered_request(None, None, false)),
        what,
    )
    .await?;
    let WirePayload::UndeliveredResponse(UndeliveredResponse::Summary(summary)) = answer else {
        return Err(format!("fetch must summarize, got {answer:?}"));
    };
    Ok(summary)
}

async fn ack_summary(
    client: &mut Client,
    summary: &UndeliveredSummary,
) -> Result<UndeliveredAckOutcome, String> {
    let acked = client
        .request_observed(
            WirePayload::UndeliveredAck(cmds::undelivered_ack(
                &summary.receipt.0,
                PresentationStatus::Presented,
            )),
            Some(summary.round.clone()),
        )
        .await
        .map_err(|error| format!("ack errored: {error:?}"))?;
    let WirePayload::UndeliveredAckOutcome(outcome) = acked else {
        return Err(format!("ack must answer an outcome, got {acked:?}"));
    };
    Ok(outcome)
}

async fn list_tasks(client: &mut Client) -> Result<TaskListPage, String> {
    let answer = ask(
        client,
        WirePayload::ListTasks(cmds::list_tasks_request(None, None)),
        "list-tasks",
    )
    .await?;
    let WirePayload::TaskListResponse(TaskListResponse::Page(page)) = answer else {
        return Err(format!("list must answer a page, got {answer:?}"));
    };
    Ok(page)
}

async fn wait_task_progress(
    client: &mut Client,
    wanted: &str,
    tasks: usize,
) -> Result<TaskListPage, String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let page = list_tasks(client).await?;
        if page.tasks.len() == tasks && page.tasks.iter().all(|item| item.progress == wanted) {
            return Ok(page);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "tasks did not reach {wanted}: {:?}",
            page.tasks
                .iter()
                .map(|item| item.progress.clone())
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The bounded deletion status page over the real wire.
async fn deletion_page(client: &mut Client) -> Result<DeletionStatusPage, String> {
    let answer = ask(
        client,
        WirePayload::DeletionStatusRequest(DeletionStatusRequest {
            cursor: None,
            limit: Some(20),
        }),
        "deletion-status",
    )
    .await?;
    let WirePayload::DeletionStatusResponse(DeletionStatusResponse::Page(page)) = answer else {
        return Err(format!(
            "deletion status must answer a page, got {answer:?}"
        ));
    };
    Ok(page)
}

/// Runs the production request inlet and returns the typed outcome.
async fn request_deletion(client: &mut Client, text: &str) -> Result<ManagementOutcome, String> {
    let page = deletion_page(client).await?;
    let intent = cmds::deletion_intent(
        CommandWireId(uuid::Uuid::new_v4()),
        &page.mark.0,
        DeletionPurposeWire::Privacy,
        text,
    );
    let answer = ask(
        client,
        WirePayload::ManagementIntent(intent),
        "deletion-intent",
    )
    .await?;
    let WirePayload::ManagementOutcome(outcome) = answer else {
        return Err(format!(
            "deletion intent must answer an outcome, got {answer:?}"
        ));
    };
    Ok(outcome)
}

/// Confirms the single staged request on the Host-local trusted inlet and
/// returns the canonical operation reference.
async fn confirm_deletion(handle: &HostHandle) -> DeletionOperationRef {
    let pending = handle
        .pending_targeted_deletions(None, 10)
        .await
        .expect("pending requests must read");
    assert_eq!(pending.len(), 1, "exactly one staged request");
    let request = pending[0].request();
    let rendered = request.as_raw().as_uuid().as_hyphenated().to_string();
    match handle
        .confirm_targeted_deletion(&rendered)
        .await
        .expect("confirmation must answer")
    {
        ConfirmTargetedDeletionOutcome::Started(current) => current,
        other => panic!("the Owner confirmation must start the operation, got {other:?}"),
    }
}

/// Drives bounded preservation passes while polling the status page, so a
/// parked Client-incarnation demand is answered by the poll's connection read.
async fn drive_until(
    handle: &HostHandle,
    client: &mut Client,
    wanted: DeletionPhaseWire,
) -> Result<DeletionStatusPage, String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(150);
    loop {
        let page = deletion_page(client).await?;
        if page.operations.first().map(|view| view.phase) == Some(wanted) {
            return Ok(page);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the deletion operation did not reach {}: {page:?}",
            wanted.as_str()
        );
        // The serving composition's production tick: the same method the
        // background driver calls, so the E2E does not step the fan-out by hand.
        let drive = handle.run_targeted_deletion_tick();
        let poll = async {
            loop {
                tokio::time::sleep(Duration::from_millis(20)).await;
                let Ok(page) = deletion_page(client).await else {
                    return None;
                };
                if page.operations.first().map(|view| view.phase) == Some(wanted) {
                    return Some(page);
                }
                if tokio::time::Instant::now() >= deadline {
                    return None;
                }
            }
        };
        let (driven, polled) = tokio::join!(drive, poll);
        driven.map_err(|error| format!("the fan-out pass must run: {error:?}"))?;
        if let Some(page) = polled {
            return Ok(page);
        }
    }
}

/// Every `(table, column)` of the state database whose TEXT content carries
/// `needle`, read over an independent connection: the mechanical DB scan the
/// completed-state assertions run.
fn db_target_hits(db: &Path, needle: &str) -> Vec<String> {
    let conn = rusqlite::Connection::open(db).expect("the state database opens for scanning");
    conn.busy_timeout(Duration::from_secs(30))
        .expect("a busy timeout must set");
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .expect("sqlite_master reads")
        .query_map([], |row| row.get(0))
        .expect("the table list reads")
        .collect::<Result<_, _>>()
        .expect("table names decode");
    let mut hits = Vec::new();
    for table in tables {
        let columns: Vec<(String, String)> = conn
            .prepare(&format!("PRAGMA table_info(\"{table}\")"))
            .expect("table_info reads")
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            })
            .expect("the column list reads")
            .collect::<Result<_, _>>()
            .expect("column rows decode");
        for (column, declared) in columns {
            let upper = declared.to_uppercase();
            if upper.contains("INT") || upper.contains("REAL") || upper.contains("BLOB") {
                continue;
            }
            let count: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM \"{table}\" WHERE instr(COALESCE(\"{column}\", ''), ?1) > 0"
                    ),
                    [needle],
                    |row| row.get(0),
                )
                .expect("the content scan answers");
            if count > 0 {
                hits.push(format!("{table}.{column}={count}"));
            }
        }
    }
    hits
}

fn assert_absent(label: &str, text: &str, needle: &str) {
    assert!(
        !text.contains(needle),
        "{label} must not carry the target: {text}"
    );
}

fn assert_absent_all(label: &str, texts: &[String], needle: &str) {
    for (index, text) in texts.iter().enumerate() {
        assert!(
            !text.contains(needle),
            "{label}[{index}] must not carry the target: {text}"
        );
    }
}

fn formation_create_with(needle: &str) -> String {
    format!(
        r##"{{"summary":"The owner asked to keep {needle} for later.","memories":[{{"action":"create","content":"The owner cares about {needle}.","importance":4,"temporal":"enduring"}}]}}"##
    )
}

fn formation_update_with(needle: &str) -> String {
    format!(
        r##"{{"summary":"The owner repeated {needle} and it now matters more.","memories":[{{"action":"update","target":1,"change":"refined","content":"The owner treats {needle} as critical."}}]}}"##
    )
}

fn formation_create() -> String {
    formation_create_with(TARGET)
}

fn formation_update() -> String {
    formation_update_with(TARGET)
}

/// Stages the fixture that plants [`TARGET`] in every required surface:
/// History, Learning Summary and Memory current + past revision, the Task
/// instruction / result / report, the control journal, the workspace path
/// copies, and the undelivered presentation transient.
async fn plant_target_fixture(served: &mut Served) {
    let dir = served.dir.clone();
    let client = served.client();
    // Round 1: History (owner input + companion reply) and one formation.
    let (round, stream, reply) = send_round(client, &format!("please remember {TARGET} for me"))
        .await
        .expect("round one must complete");
    assert!(
        reply.contains(TARGET),
        "the reply carries the target: {reply}"
    );
    confirm_round(client, &round, stream).await;
    wait_for_memory_revision_at_least(client, 1).await;
    // Round 2: History and an update that leaves the current Memory plus the
    // previous revision carrying the target.
    let (round, stream, reply) =
        send_round(client, &format!("{TARGET} is critical, never forget it"))
            .await
            .expect("round two must complete");
    assert!(
        reply.contains(TARGET),
        "the reply carries the target: {reply}"
    );
    confirm_round(client, &round, stream).await;
    wait_for_memory_revision_at_least(client, 2).await;
    // Workspace selection: the path copies and the control journal carry the
    // target (a plausible Owner folder name).
    let workspace = dir.join(format!("workspace-{TARGET}"));
    std::fs::create_dir_all(&workspace).expect("the workspace directory creates");
    std::fs::write(workspace.join("input.txt"), b"notes").expect("input fixture");
    select_workspace(client, &workspace)
        .await
        .expect("workspace must select");
    // Round 3: the companion proposes a Task whose purpose carries the
    // target; the production launcher runs the agent, whose final result
    // carries it too.
    let (round, stream, reply) = send_round(client, "please read input.txt and write report.md")
        .await
        .expect("the propose round must complete");
    assert!(
        reply.contains("Task accepted"),
        "the task proposal must be accepted: {reply}"
    );
    confirm_round(client, &round, stream).await;
    wait_task_progress(client, "completed", 1)
        .await
        .expect("the delegated task must complete");
    // The completion is delivered as an un-ACKed presentation transient: its
    // excerpt carries the target until the operation invalidates it.
    let summary = wait_for_summary_with(client, TARGET).await;
    assert!(
        summary
            .items
            .iter()
            .any(|item| item.excerpt.contains(TARGET)),
        "the carried receipt must quote the target: {summary:?}"
    );
}

/// Polls the read-only memory view until the companion reports a memory whose
/// revision is at least `wanted` (the formation and its update committed).
async fn wait_for_memory_revision_at_least(client: &mut Client, wanted: u64) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let body = memory_view(client).await;
        let highest = body
            .lines()
            .filter_map(|line| {
                line.split_whitespace()
                    .find_map(|part| part.strip_prefix("revision="))
                    .and_then(|value| value.parse::<u64>().ok())
            })
            .max()
            .unwrap_or(0);
        if highest >= wanted {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the memory revision did not reach {wanted}: {body}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The History page's item texts for the running companion.
async fn history_texts(client: &mut Client) -> Vec<String> {
    let companion = client.companion_ref();
    let history = ask(
        client,
        WirePayload::HistoryRequest(cmds::history_request(&companion, 100)),
        "history",
    )
    .await
    .expect("history must answer");
    let WirePayload::HistoryResponse(HistoryResponse::Items(items)) = history else {
        panic!("history must answer items: {history:?}");
    };
    items.into_iter().map(|item| item.text).collect()
}

/// The read-only memory section body from one management view answer.
async fn memory_view(client: &mut Client) -> String {
    let answer = ask(
        client,
        WirePayload::ManagementViewRequest(cmds::memory_view_request(None, None, None)),
        "memory view",
    )
    .await;
    let Ok(WirePayload::ManagementView(view)) = answer else {
        panic!("memory view must answer a view: {answer:?}");
    };
    view.sections
        .iter()
        .find(|section| section.kind == "memory")
        .map(|section| section.body.clone())
        .unwrap_or_default()
}

/// Polls the undelivered subscription until a carried excerpt quotes `needle`.
async fn wait_for_summary_with(client: &mut Client, needle: &str) -> UndeliveredSummary {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let summary = fetch_summary(client, "undelivered")
            .await
            .expect("the subscription must answer");
        if summary
            .items
            .iter()
            .any(|item| item.excerpt.contains(needle))
        {
            return summary;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no carried excerpt quoted the target: {summary:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The target must be provably planted before the deletion runs, otherwise
/// the completed-state scans would prove nothing.
fn assert_target_is_planted(served: &Served) {
    let hits = db_target_hits(&served.dir.join("app.db"), TARGET);
    assert!(
        !hits.is_empty(),
        "the fixture must plant the target before the deletion"
    );
}

/// Completed-state verification through the bounded first-party reads: the
/// History page, the Memory view, the undelivered subscription, and the Task
/// report must not carry the target, and no provider request issued after the
/// confirmation may quote it.
async fn assert_completed_reads_are_clean(served: &mut Served, sends_at_confirmation: usize) {
    let client = served.client();
    let texts = history_texts(client).await;
    assert_absent_all("history", &texts, TARGET);
    let memory = memory_view(client).await;
    assert_absent("memory view", &memory, TARGET);
    let summary = fetch_summary(client, "post-completion subscription")
        .await
        .expect("the subscription must answer");
    for item in &summary.items {
        assert_absent("presentation excerpt", &item.excerpt, TARGET);
    }
    let tasks = list_tasks(client).await.expect("tasks must list");
    if let Some(task) = tasks.tasks.first() {
        let report = ask(
            client,
            WirePayload::GetTaskReport(cmds::task_report_request(&task.task.0, None, None)),
            "task report",
        )
        .await
        .expect("the report must answer");
        let rendered = format!("{report:?}");
        assert_absent("task report", &rendered, TARGET);
    }
    let inputs = served.transport.input_texts();
    let later: Vec<String> = inputs.into_iter().skip(sends_at_confirmation).collect();
    assert_absent_all("post-confirmation provider request", &later, TARGET);
}

/// E2E 1: the whole Targeted Deletion lifecycle over the real socket, from the
/// first-party request to the completed operation, with system-wide scans,
/// search-material destruction, and the fresh-origin acceptance.
#[tokio::test]
async fn stage6_targeted_deletion_completes_system_wide() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let proposal = task_reply(serde_json::json!({
        "kind": "propose_task",
        "purpose": format!("write a report covering {TARGET}"),
    }));
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (on_learning_formation(true), Call::text(formation_create())),
            (on_learning_formation(false), Call::text(formation_update())),
            (
                on_task_agent_turn(0),
                Call::text(r#"{"tool":"read","path":"input.txt"}"#),
            ),
            (
                on_task_agent_turn(1),
                Call::text(r##"{"tool":"create","path":"report.md","content":"# Report\nnotes"}"##),
            ),
            (
                on_task_agent_turn(2),
                Call::text(format!(
                    r##"{{"final":"created report.md covering {TARGET}"}}"##
                )),
            ),
            (
                on_latest_owner("please read input.txt and write report.md"),
                Call::text(proposal),
            ),
            (
                on_latest_owner(&format!("please remember {TARGET} for me")),
                Call::text(format!("I will keep {TARGET} in mind.")),
            ),
            (
                on_latest_owner(&format!("{TARGET} is critical, never forget it")),
                Call::text(format!("{TARGET} matters to you.")),
            ),
        ],
        &[],
    ));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE, cmds::CAPABILITY_LEARNING],
    )
    .await;
    plant_target_fixture(&mut served).await;
    assert_target_is_planted(&served);

    // First-party request: only stages, never destructive.
    let outcome = request_deletion(served.client(), TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(
        outcome,
        ManagementOutcome::NeedsClarification,
        "the Client intent only stages"
    );
    assert!(
        deletion_page(served.client())
            .await
            .expect("status must answer")
            .operations
            .is_empty(),
        "the wire intent must create no operation"
    );
    // Host-local trusted confirmation starts the canonical operation.
    let current = confirm_deletion(&served.handle).await;
    let sends_at_confirmation = transport.sends();
    let page = deletion_page(served.client())
        .await
        .expect("status must answer");
    assert_eq!(page.operations.len(), 1, "one operation exists");
    assert_ne!(
        page.operations[0].phase,
        DeletionPhaseWire::Completed,
        "a confirmation never completes an operation by itself"
    );
    assert_eq!(
        page.operations[0].operation.0,
        current
            .operation
            .as_raw()
            .as_uuid()
            .as_hyphenated()
            .to_string(),
        "the status view names the started operation"
    );

    let handle = Arc::clone(&served.handle);
    let page = drive_until(&handle, served.client(), DeletionPhaseWire::Completed)
        .await
        .expect("the operation must complete");
    let view = &page.operations[0];
    assert_eq!(view.phase, DeletionPhaseWire::Completed);
    let DeletionParticipantReportWire::Reported(participants) = &view.participants else {
        panic!("a completed operation reports its participant set");
    };
    assert!(
        participants
            .iter()
            .all(|participant| participant.progress == "verified"),
        "every required participant must be verified: {participants:?}"
    );
    assert_completed_reads_are_clean(&mut served, sends_at_confirmation).await;
    // The canonical production remainder probe and the independent DB scan
    // both agree the completed state keeps nothing.
    assert_eq!(
        served.canonical_remainder(TARGET).await,
        0,
        "the canonical system-wide remainder must be zero"
    );
    assert!(
        db_target_hits(&served.dir.join("app.db"), TARGET).is_empty(),
        "no table may keep the target after completion"
    );

    // A completed operation is not a permanent keyword ban: the Owner may
    // provide the same text again as a fresh origin.
    let sends_before = transport.sends();
    let (round, stream, reply) = send_round(served.client(), &format!("a fresh note: {TARGET}"))
        .await
        .expect("a fresh origin must be accepted");
    assert!(
        reply.contains("acknowledged"),
        "the fresh origin round must complete: {reply}"
    );
    confirm_round(served.client(), &round, stream).await;
    assert!(
        transport.sends() > sends_before,
        "the fresh origin is a new provider origin, not a permanent ban"
    );
    let companion = served.client().companion_ref();
    let history = ask(
        served.client(),
        WirePayload::HistoryRequest(cmds::history_request(&companion, 100)),
        "fresh history",
    )
    .await
    .expect("history must answer");
    let WirePayload::HistoryResponse(HistoryResponse::Items(items)) = history else {
        panic!("history must answer items: {history:?}");
    };
    assert!(
        items.iter().any(|item| item.text.contains(TARGET)),
        "the fresh origin is appended as new History"
    );
    served.server.abort();
}

/// A target that only the finalizing-restart leg uses: it is never planted in
/// the store, so the leg can build the crash-consistent `finalizing` marker
/// through the sealed repository boundary without claiming an erasure.
const FINALIZING_TARGET: &str = "TS6-FINALIZING-CANARY-2201";

/// E2E 1 race (design R2): a dialogue provider call already claimed when the
/// deletion condition commits must not publish or adopt its covered reply, a
/// fresh target-bearing submit is held with zero provider bytes while the
/// operation is unfinished, and the completed surface stays clean.
///
/// A Host restart between the fixture and the admission clears the in-process
/// Client-delivery tracking, so this leg drives the fence without parking on a
/// Client demand the single-frame connection loop cannot deliver mid-frame;
/// the Client-incarnation demand path has its own legs below.
#[tokio::test]
async fn stage6_deletion_races_provider_wait_and_delayed_result() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let first = format!("please remember {TARGET} for me");
    let second = format!("a second note about {TARGET}");
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_latest_owner(&first),
                Call::text(format!("I will keep {TARGET} in mind.")),
            ),
            (
                on_latest_owner(&second),
                Call::text(format!("Second reply quoting {TARGET}.")),
            ),
        ],
        &[],
    ));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE, cmds::CAPABILITY_LEARNING],
    )
    .await;
    let (_round, _stream, reply) = send_round(served.client(), &first)
        .await
        .expect("the first round must complete");
    assert!(reply.contains(TARGET));
    // Drain and present the round's backlog before the restart, so no
    // post-restart push re-tracks the reconnecting incarnation: this leg
    // targets the provider-wait race, and the Client-incarnation demand has
    // its own legs.
    let backlog = wait_for_summary_with(served.client(), TARGET).await;
    drop(ack_summary(served.client(), &backlog).await);
    let mut client = served.restart().await;
    // Advisory staging first; the condition is committed while the external
    // provider call is already in flight.
    let outcome = request_deletion(&mut client, TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(outcome, ManagementOutcome::NeedsClarification);
    // The second round's provider call parks after its attempt claim.
    transport.block_input(on_latest_owner(&second));
    let handle = Arc::clone(&served.handle);
    let barrier = Arc::clone(&transport);
    let mut second_round = Box::pin(send_round_raw(&mut client, &second));
    let mut confirmed = false;
    let mut driven = false;
    let round_result = loop {
        tokio::select! {
            result = second_round.as_mut() => break result,
            () = barrier.wait_parked(1), if !confirmed => {
                confirm_deletion(&handle).await;
                confirmed = true;
            }
        }
        if confirmed && !driven {
            // One bounded pass collects the durable owner surfaces and moves
            // the Host transient fence; the parked provider call stays held
            // until the pass returns.
            let pass = handle
                .run_targeted_deletion_tick()
                .await
                .expect("the serving tick must run");
            assert_eq!(pass.held, 0, "no participant holds a reachable owner");
            barrier.release_blocked();
            driven = true;
        }
    };
    drop(second_round);
    let (_round2, _stream2, raced_text, close) = round_result.expect("the raced round must answer");
    assert_eq!(
        close,
        ene_api::v1::round::StreamClose::Interrupted,
        "a reply whose deletion condition committed during its provider wait never completes"
    );
    assert!(
        !raced_text.contains(TARGET),
        "no covered delta may be presented: {raced_text}"
    );
    let page = drive_until(&handle, &mut client, DeletionPhaseWire::Completed)
        .await
        .expect("the operation must complete");
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Completed);
    assert_eq!(served.canonical_remainder(TARGET).await, 0);
    assert!(db_target_hits(&served.dir.join("app.db"), TARGET).is_empty());
    // The completed operation is not a permanent ban on the string.
    let (round, stream, fresh) = send_round(&mut client, &format!("a fresh note: {TARGET}"))
        .await
        .expect("a fresh origin must be accepted");
    assert!(fresh.contains("acknowledged"), "{fresh}");
    confirm_round(&mut client, &round, stream).await;
    served.server.abort();
}

/// E2E 1 race: a receipt created before the condition is neither presented nor
/// acknowledged after it, and a fresh read withholds the covered body.
#[tokio::test]
async fn stage6_deletion_presentation_ack_after_condition_holds() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let first = format!("please remember {TARGET} for me");
    let transport = Arc::new(ScriptedTransport::new(
        vec![(
            on_latest_owner(&first),
            Call::text(format!("I will keep {TARGET} in mind.")),
        )],
        &[],
    ));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE, cmds::CAPABILITY_LEARNING],
    )
    .await;
    let (_round, _stream, _reply) = send_round(served.client(), &first)
        .await
        .expect("the first round must complete");
    let receipt = wait_for_summary_with(served.client(), TARGET).await;
    let outcome = request_deletion(served.client(), TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(outcome, ManagementOutcome::NeedsClarification);
    confirm_deletion(&served.handle).await;
    // Condition first: a fresh target-bearing submit is held at intake with
    // zero provider bytes and leaves no History row.
    let history_before = history_texts(served.client()).await;
    let sends_before = transport.sends();
    submit_expect_hold(served.client(), &format!("still {TARGET}"))
        .await
        .expect("a covered submit must hold");
    assert_eq!(
        transport.sends(),
        sends_before,
        "a held submit sends the provider zero bytes"
    );
    assert_eq!(
        history_texts(served.client()).await.len(),
        history_before.len(),
        "a held submit leaves no History row"
    );
    // The receipt predates the condition. The confirmation's driver pass
    // invalidates the transient receipt, so the ACK answers either as a
    // domain hold or as a stale receipt; neither confirms presentation.
    let acked = ack_summary(served.client(), &receipt)
        .await
        .expect("the ack must answer");
    assert!(
        matches!(
            acked,
            UndeliveredAckOutcome::HeldForErasure | UndeliveredAckOutcome::StalePresentation
        ),
        "an ACK for a covered receipt never confirms presentation, got {acked:?}"
    );
    // A fresh read withholds the covered body.
    let fresh = fetch_summary(served.client(), "covered subscription")
        .await
        .expect("the subscription must answer");
    for item in &fresh.items {
        assert_absent("covered excerpt", &item.excerpt, TARGET);
    }
    let handle = Arc::clone(&served.handle);
    let page = drive_until(&handle, served.client(), DeletionPhaseWire::Completed)
        .await
        .expect("the operation must complete with a reachable Client");
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Completed);
    assert_eq!(served.canonical_remainder(TARGET).await, 0);
    assert!(db_target_hits(&served.dir.join("app.db"), TARGET).is_empty());
    served.server.abort();
}

/// E2E 1 race: a Learning formation already claimed when the deletion
/// condition commits must not form a Summary or Memory from the covered
/// transcript, and the completed state keeps nothing.
#[tokio::test]
async fn stage6_deletion_during_learning_formation_never_forms_target_memory() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let first = format!("please remember {TARGET} for me");
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (on_learning_formation(true), Call::text(formation_create())),
            (
                on_latest_owner(&first),
                Call::text(format!("I will keep {TARGET} in mind.")),
            ),
        ],
        &[],
    ));
    // The formation call parks after its claim; the condition commits while
    // the Learning provider work is in flight (design R2 for the Learning
    // owner).
    transport.block_input(on_learning_formation(true));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE, cmds::CAPABILITY_LEARNING],
    )
    .await;
    let (_round, _stream, reply) = send_round(served.client(), &first)
        .await
        .expect("the first round must complete");
    assert!(reply.contains(TARGET));
    transport.wait_parked(1).await;
    let outcome = request_deletion(served.client(), TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(outcome, ManagementOutcome::NeedsClarification);
    confirm_deletion(&served.handle).await;
    transport.release_blocked();
    // The formation commit lands against the current condition; give it the
    // bounded window the fan-out would use, then prove no Memory exists.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !memory_view(served.client()).await.contains(TARGET),
        "a covered formation never writes target Memory"
    );
    let handle = Arc::clone(&served.handle);
    let page = drive_until(&handle, served.client(), DeletionPhaseWire::Completed)
        .await
        .expect("the operation must complete");
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Completed);
    assert!(
        transport
            .input_texts()
            .iter()
            .any(|input| input.contains("learning formation pass") && input.contains(TARGET)),
        "the fixture must reach the formation provider call"
    );
    assert_eq!(served.canonical_remainder(TARGET).await, 0);
    assert!(db_target_hits(&served.dir.join("app.db"), TARGET).is_empty());
    served.server.abort();
}

/// E2E 1 race (design R2, post-completion): a Learning formation already
/// claimed when the deletion condition commits is refused at its commit even
/// after the operation completed and every current condition closed; the
/// completed surface keeps no target body. A fresh Owner origin after
/// completion is learned normally: the durable correspondence names the
/// claim, never the text.
#[tokio::test]
async fn stage6_delayed_formation_after_completion_is_refused() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let first = format!("please remember {TARGET} for me");
    let fresh = format!("a fresh note about {TARGET}");
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (on_learning_formation(true), Call::text(formation_create())),
            (
                on_latest_owner(&first),
                Call::text(format!("I will keep {TARGET} in mind.")),
            ),
            (on_latest_owner(&fresh), Call::text("acknowledged")),
        ],
        &[],
    ));
    // The formation pass parks after its durable claim; the deletion runs to
    // completion while the provider work is still in flight.
    transport.block_input(on_learning_formation(true));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE, cmds::CAPABILITY_LEARNING],
    )
    .await;
    let (_round, _stream, reply) = send_round(served.client(), &first)
        .await
        .expect("the first round must complete");
    assert!(reply.contains(TARGET));
    transport.wait_parked(1).await;
    let outcome = request_deletion(served.client(), TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(outcome, ManagementOutcome::NeedsClarification);
    let handle = Arc::clone(&served.handle);
    confirm_deletion(&handle).await;
    let page = drive_until(&handle, served.client(), DeletionPhaseWire::Completed)
        .await
        .expect("the operation must complete while the formation is parked");
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Completed);
    // The parked provider call resumes after completion. Its settlement is
    // durable before the formation's commit attempt, so observing it orders
    // the refusal asserted below.
    transport.release_blocked();
    wait_for_usage_consumer(served.client(), "companion_learning").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !memory_view(served.client()).await.contains(TARGET),
        "a formation claimed before the interval never writes target Memory after completion"
    );
    assert_eq!(served.canonical_remainder(TARGET).await, 0);
    assert!(db_target_hits(&served.dir.join("app.db"), TARGET).is_empty());
    // The completed operation is not a permanent ban: a fresh Owner origin
    // after completion is learned as a new experience.
    let (_round, _stream, reply) = send_round(served.client(), &fresh)
        .await
        .expect("a fresh origin must be accepted");
    assert!(reply.contains("acknowledged"), "{reply}");
    wait_for_memory_revision_at_least(served.client(), 1).await;
    assert!(
        memory_view(served.client()).await.contains(TARGET),
        "the fresh origin forms a new Memory"
    );
    served.server.abort();
}

/// E2E 1 race (design R2, post-completion): a Task Agent execution already
/// claimed when the deletion condition commits has its delayed final result
/// collected after the operation completed; the execution seal, certainty,
/// and adoption facts survive and the task still completes. A fresh Owner
/// origin after completion is a new History row.
#[tokio::test]
async fn stage6_delayed_task_result_after_completion_is_collected() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let proposal = task_reply(serde_json::json!({
        "kind": "propose_task",
        "purpose": format!("write a report covering {TARGET}"),
    }));
    let fresh = format!("a fresh note about {TARGET}");
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_task_agent_turn(0),
                Call::text(r#"{"tool":"read","path":"input.txt"}"#),
            ),
            (
                on_task_agent_turn(1),
                Call::text(r##"{"tool":"create","path":"report.md","content":"# Report\nnotes"}"##),
            ),
            (
                on_task_agent_turn(2),
                Call::text(format!(
                    r##"{{"final":"created report.md covering {TARGET}"}}"##
                )),
            ),
            (
                on_latest_owner("please read input.txt and write report.md"),
                Call::text(proposal),
            ),
            (on_latest_owner(&fresh), Call::text("acknowledged")),
        ],
        &[],
    ));
    // The final turn parks after its durable claim; the deletion runs to
    // completion while the provider work is still in flight.
    transport.block_input(on_task_agent_turn(2));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE],
    )
    .await;
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace).expect("the workspace directory creates");
    std::fs::write(workspace.join("input.txt"), b"notes").expect("input fixture");
    select_workspace(served.client(), &workspace)
        .await
        .expect("workspace must select");
    let (round, stream, reply) =
        send_round(served.client(), "please read input.txt and write report.md")
            .await
            .expect("the propose round must complete");
    assert!(
        reply.contains("Task accepted"),
        "the task proposal must be accepted: {reply}"
    );
    confirm_round(served.client(), &round, stream).await;
    transport.wait_parked(1).await;
    let outcome = request_deletion(served.client(), TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(outcome, ManagementOutcome::NeedsClarification);
    let handle = Arc::clone(&served.handle);
    confirm_deletion(&handle).await;
    let page = drive_until(&handle, served.client(), DeletionPhaseWire::Completed)
        .await
        .expect("the operation must complete while the final turn is parked");
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Completed);
    // The parked final answer arrives after completion: the delegation's
    // admission-time hold collects the body, and the durable execution facts
    // still seal, adopt, and complete the task.
    transport.release_blocked();
    wait_task_progress(served.client(), "completed", 1)
        .await
        .expect("the task completes on the collected result");
    assert_eq!(served.canonical_remainder(TARGET).await, 0);
    assert!(
        db_target_hits(&served.dir.join("app.db"), TARGET).is_empty(),
        "the delayed result body is never stored"
    );
    // The completed operation is not a permanent ban: a fresh Owner origin
    // after completion is a new History row.
    let (_round, _stream, reply) = send_round(served.client(), &fresh)
        .await
        .expect("a fresh origin must be accepted");
    assert!(reply.contains("acknowledged"), "{reply}");
    assert!(
        history_texts(served.client())
            .await
            .iter()
            .any(|text| text.contains(TARGET)),
        "the fresh origin is appended as new History"
    );
    served.server.abort();
}

/// E2E 1 restart: an unfinished operation survives a Host restart in `active`,
/// keeps its current condition, and resumes to completion; a restart never
/// completes it by itself.
#[tokio::test]
async fn stage6_deletion_restart_during_active_resumes() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let first = format!("please remember {TARGET} for me");
    let transport = Arc::new(ScriptedTransport::new(
        vec![(
            on_latest_owner(&first),
            Call::text(format!("I will keep {TARGET} in mind.")),
        )],
        &[],
    ));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE, cmds::CAPABILITY_LEARNING],
    )
    .await;
    let (_round, _stream, _reply) = send_round(served.client(), &first)
        .await
        .expect("the first round must complete");
    let current = {
        let outcome = request_deletion(served.client(), TARGET)
            .await
            .expect("the request inlet must answer");
        assert_eq!(outcome, ManagementOutcome::NeedsClarification);
        confirm_deletion(&served.handle).await
    };
    // Restart while the operation is unfinished. The confirmation already
    // kicked one bounded pass, so the durable phase may be Active or Held on
    // an unreachable holder; the restart must neither lose the operation nor
    // complete it by itself.
    let mut client = served.restart().await;
    let page = deletion_page(&mut client)
        .await
        .expect("status must answer");
    assert_eq!(page.operations.len(), 1, "the operation is never lost");
    assert_ne!(
        page.operations[0].phase,
        DeletionPhaseWire::Completed,
        "a restart never completes a deletion operation by itself"
    );
    assert_eq!(
        page.operations[0].operation.0,
        current
            .operation
            .as_raw()
            .as_uuid()
            .as_hyphenated()
            .to_string(),
        "the operation identity survives the restart"
    );
    assert_eq!(page.operations[0].sweep, current.sweep.as_u64());
    // The current condition survived too: a fresh target submit is still held
    // with zero provider bytes.
    let sends_before = transport.sends();
    submit_expect_hold(&mut client, &format!("still {TARGET}"))
        .await
        .expect("the condition survives the restart");
    assert_eq!(transport.sends(), sends_before);
    // The restarted Host resumes and completes the operation through the
    // production fan-out.
    let page = drive_until(&served.handle, &mut client, DeletionPhaseWire::Completed)
        .await
        .expect("the restarted Host must complete the operation");
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Completed);
    assert_eq!(served.canonical_remainder(TARGET).await, 0);
    assert!(db_target_hits(&served.dir.join("app.db"), TARGET).is_empty());
    served.server.abort();
}

/// E2E 1 restart: a durable `finalizing` marker (the crash-consistent state
/// between the two sealed completion calls, built here through the public
/// preservation repository because only a real crash can interleave them)
/// survives the restart and resumes to completion; the restart itself never
/// completes it.
#[tokio::test]
async fn stage6_deletion_restart_during_finalizing_resumes() {
    use ene_preservation::{
        DeletionFinalizationOutcome, ParticipantCompletionFact, PreservationRepository as _,
    };

    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let transport = Arc::new(ScriptedTransport::new(vec![], &[]));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE],
    )
    .await;
    // The Host is stopped, and the operation is built through the canonical
    // store producer (the same admission the Host-local confirmation runs)
    // because the serving composition's driver would otherwise finish the
    // operation before the crash point can be staged. Only a real crash can
    // interleave the two sealed completion calls, so the marker is a fixture.
    served.stop().await;
    let store = ene_store::Store::open(&dir.join("app.db"))
        .await
        .expect("the state database opens for the crash fixture");
    let staged = store
        .stage_targeted_deletion(ene_preservation::StageTargetedDeletionRequestCommand::new(
            ene_preservation::TargetedDeletionTarget {
                mechanical: ene_preservation::MechanicalDeletionTarget::ExactText(
                    ene_preservation::DeletionSearchMaterial::new(FINALIZING_TARGET.to_owned()),
                ),
                semantic_hints: Vec::new(),
            },
            ene_preservation::DeletionPurpose::Privacy,
            WallClockWithTz::now(),
        ))
        .await
        .expect("the canonical staged request must commit");
    let request = match staged {
        ene_preservation::StageTargetedDeletionRequestOutcome::Staged(request)
        | ene_preservation::StageTargetedDeletionRequestOutcome::AlreadyStaged(request)
        | ene_preservation::StageTargetedDeletionRequestOutcome::Confirmed(request) => request,
        other => panic!("the request must be staged, got {other:?}"),
    };
    let current = match store
        .confirm_targeted_deletion(
            request,
            ene_core::targeted_deletion::current_product_surface_owners(),
        )
        .await
        .expect("the canonical confirmation must answer")
    {
        ConfirmTargetedDeletionOutcome::Started(current) => current,
        other => panic!("the confirmation must start one operation, got {other:?}"),
    };

    // Build the crash point: every required participant is verified for the
    // current sweep, then the durable `finalizing` marker commits and the
    // process would have crashed before the completion commit.
    let participants = store
        .deletion_participants(current.operation, None, 100)
        .await
        .expect("the participant snapshot must read");
    assert!(!participants.is_empty());
    for record in &participants {
        store
            .record_participant_completion(ParticipantCompletionFact::verified(
                current.condition(),
                record.participant.owner,
                0,
                WallClockWithTz::now(),
            ))
            .await
            .expect("the verified fact must record");
    }
    assert_eq!(
        store
            .begin_deletion_finalizing(current)
            .await
            .expect("the finalizing marker must commit"),
        DeletionFinalizationOutcome::Finalizing
    );
    drop(store);

    // Restart: the startup recovery reads the durable `finalizing` marker and
    // finishes the remaining completion steps (lifecycle §14). It resumes the
    // completion boundary, never a phase guess and never a second sweep.
    let mut client = served.serve().await;
    let page = deletion_page(&mut client)
        .await
        .expect("status must answer");
    assert_eq!(page.operations.len(), 1, "the operation is never lost");
    assert_eq!(
        page.operations[0].phase,
        DeletionPhaseWire::Completed,
        "the finalizing marker must resume to the sealed completion"
    );
    assert_eq!(
        page.operations[0].operation.0,
        current
            .operation
            .as_raw()
            .as_uuid()
            .as_hyphenated()
            .to_string()
    );
    // The resume is a completion step, not a second participant sweep.
    let before = transport.sends();
    let page = drive_until(&served.handle, &mut client, DeletionPhaseWire::Completed)
        .await
        .expect("the finalizing marker must resume to completion");
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Completed);
    assert_eq!(
        transport.sends(),
        before,
        "the resume demands no new provider call"
    );
    served.server.abort();
}

// ---------------------------------------------------------------------------
// E2E 1: the Client-incarnation required participant (lifecycle §8.1)
// ---------------------------------------------------------------------------

/// The Client-incarnation participant row of the first operation in one
/// status page, when the durable snapshot named one (lifecycle §8.1).
fn client_incarnation_participant(
    page: &DeletionStatusPage,
) -> Option<&DeletionParticipantStatusWire> {
    let view = page.operations.first()?;
    let DeletionParticipantReportWire::Reported(participants) = &view.participants else {
        return None;
    };
    participants
        .iter()
        .find(|participant| participant.owner.starts_with("client_incarnation:"))
}

/// Renders the single staged request identity, exactly as the Host-local
/// preview does.
async fn render_single_pending_request(handle: &HostHandle) -> String {
    let pending = handle
        .pending_targeted_deletions(None, 10)
        .await
        .expect("pending requests must read");
    assert_eq!(pending.len(), 1, "exactly one staged request");
    pending[0]
        .request()
        .as_raw()
        .as_uuid()
        .as_hyphenated()
        .to_string()
}

/// Records the Owner confirmation through the serving Host's Host-local
/// first-party control inlet: the same production path `ene-core
/// confirm-deletion` dials, never an offline state open.
///
/// While a Client is attached it keeps reading frames, so the Host's bounded
/// local-erasure demand is answered inline exactly as an interactive Client
/// would. With no attached Client the demand resolves to an explicit hold.
async fn confirm_deletion_via_serving_control(served: &mut Served) -> DeletionOperationRef {
    let request = render_single_pending_request(&served.handle).await;
    let dir = served.dir.clone();
    let outcome = match served.client.as_mut() {
        Some(client) => {
            let mut confirmation = Box::pin(ene_core::host_control::confirm_targeted_deletion(
                &dir, &request,
            ));
            loop {
                tokio::select! {
                    outcome = &mut confirmation => break outcome,
                    () = tokio::time::sleep(Duration::from_millis(20)) => {
                        // A read drives the frame pump, which answers the
                        // Host's demand inline; a failed read means the
                        // connection ended and the participant must hold.
                        drop(deletion_page(client).await);
                    }
                }
            }
        }
        None => ene_core::host_control::confirm_targeted_deletion(&dir, &request).await,
    };
    match outcome.expect("the serving Host control inlet must answer") {
        ConfirmTargetedDeletionOutcome::Started(current) => current,
        other => panic!("the Owner confirmation must start the operation, got {other:?}"),
    }
}

/// One bounded status page read through the Host handle. Used by legs whose
/// Client connection must stay absent: a reconnect would make the incarnation
/// reachable again and change the premise under test.
async fn local_deletion_page(handle: &HostHandle) -> DeletionStatusPage {
    match handle
        .deletion_status_page(None, 20)
        .await
        .expect("the local status must answer")
    {
        DeletionStatusResponse::Page(page) => page,
        other => panic!("the local status must answer a page: {other:?}"),
    }
}

/// Drives production serving ticks while reading status through the handle
/// until the operation reaches `wanted`.
async fn drive_until_local(handle: &HostHandle, wanted: DeletionPhaseWire) -> DeletionStatusPage {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(150);
    loop {
        let page = local_deletion_page(handle).await;
        if page.operations.first().map(|view| view.phase) == Some(wanted) {
            return page;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the deletion operation did not reach {}: {page:?}",
            wanted.as_str()
        );
        handle
            .run_targeted_deletion_tick()
            .await
            .expect("the serving tick must run");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Delivers one target-bearing transient copy to the live Client: the
/// companion reply quotes the target on the text stream and the carried
/// presentation excerpt quotes it again, left un-ACKed. The Host therefore
/// observed a real body delivery to this incarnation.
async fn deliver_target_copy(served: &mut Served) {
    let first = format!("please remember {TARGET} for me");
    let (_round, _stream, reply) = send_round(served.client(), &first)
        .await
        .expect("the target-bearing round must complete");
    assert!(
        reply.contains(TARGET),
        "the streamed reply must carry the target: {reply}"
    );
    let carried = wait_for_summary_with(served.client(), TARGET).await;
    assert!(
        carried
            .items
            .iter()
            .any(|item| item.excerpt.contains(TARGET)),
        "the Client must receive a target-bearing transient copy: {carried:?}"
    );
}

/// Waits until the Learning Memory row itself carries the target, observed
/// through the independent DB scan. The fixture needs the planted body
/// without reading any body-bearing Client surface first.
async fn wait_for_target_memory_row(served: &Served) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if db_target_hits(&served.dir.join("app.db"), TARGET)
            .iter()
            .any(|hit| hit.starts_with("learning_memory."))
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the Learning Memory row never carried the target"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Stage 6 Blocker 1: the Owner confirmation runs in the serving Host, so the
/// Client that received a target-bearing copy is snapshotted as a required
/// participant; its own local-erasure confirmation is what verifies the
/// participant and lets global completion commit.
#[tokio::test]
async fn stage6_client_incarnation_confirmed_in_serving_host_and_verified() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let first = format!("please remember {TARGET} for me");
    let transport = Arc::new(ScriptedTransport::new(
        vec![(
            on_latest_owner(&first),
            Call::text(format!("I will keep {TARGET} in mind.")),
        )],
        &[],
    ));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE, cmds::CAPABILITY_LEARNING],
    )
    .await;
    // 1: a target-bearing copy actually reaches the Client.
    deliver_target_copy(&mut served).await;

    // 2: the first-party request only stages.
    let outcome = request_deletion(served.client(), TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(
        outcome,
        ManagementOutcome::NeedsClarification,
        "the Client intent only stages"
    );

    // 3: the production trusted first-party confirmation through the serving
    // Host control inlet; the Client answers the bounded demand inline.
    let current = confirm_deletion_via_serving_control(&mut served).await;

    // 4: the durable participant snapshot names the Client incarnation.
    let page = local_deletion_page(&served.handle).await;
    let participant = client_incarnation_participant(&page)
        .expect("the durable snapshot must name the delivered Client incarnation");
    assert!(
        participant.sweep >= current.sweep.as_u64(),
        "the Client participant belongs to the current sweep: {participant:?}"
    );

    // 6: the Client's local erasure confirmation verifies the participant and
    // the operation reaches the sealed global completion.
    let handle = Arc::clone(&served.handle);
    let page = drive_until(&handle, served.client(), DeletionPhaseWire::Completed)
        .await
        .expect("the operation must complete");
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Completed);
    let participant = client_incarnation_participant(&page)
        .expect("the completed operation still reports the Client participant");
    assert_eq!(
        participant.progress, "verified",
        "the Client's own local erasure pass is the verification premise"
    );
    assert_eq!(served.canonical_remainder(TARGET).await, 0);
    assert!(db_target_hits(&served.dir.join("app.db"), TARGET).is_empty());
    served.server.abort();
}

/// Stage 6 Blocker 1: an unreachable Client that received a target-bearing
/// copy keeps the operation `Held`; a disconnect, a replacement connection,
/// and a Host restart alone never verify it or complete the operation. The
/// snapshotted Client participant survives the restart, and only its own
/// local-erasure confirmation lets completion commit.
#[tokio::test]
async fn stage6_client_incarnation_unreachable_holds_across_restart() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let first = format!("please remember {TARGET} for me");
    let transport = Arc::new(ScriptedTransport::new(
        vec![(
            on_latest_owner(&first),
            Call::text(format!("I will keep {TARGET} in mind.")),
        )],
        &[],
    ));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE, cmds::CAPABILITY_LEARNING],
    )
    .await;
    deliver_target_copy(&mut served).await;
    let outcome = request_deletion(served.client(), TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(outcome, ManagementOutcome::NeedsClarification);

    // The Client disconnects before the Owner confirmation. The delivery
    // evidence is not a connection fact, so the incarnation stays in the
    // snapshot, but its local erasure can no longer be confirmed.
    served.client = None;
    let _current = confirm_deletion_via_serving_control(&mut served).await;

    // 5: the unreachable Client is an explicit hold, never a completion.
    let page = drive_until_local(&served.handle, DeletionPhaseWire::Held).await;
    assert_ne!(
        page.operations[0].phase,
        DeletionPhaseWire::Completed,
        "an unreachable Client must never be presumed erased"
    );
    let participant =
        client_incarnation_participant(&page).expect("the snapshotted Client stays required");
    assert_eq!(participant.progress, "held:unavailable");

    // 7a: a replacement connection alone (same incarnation, new connection
    // lifetime) changes no durable fact and confirms nothing.
    let replacement = connect(&served.dir).await;
    let page = local_deletion_page(&served.handle).await;
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Held);
    assert_eq!(
        client_incarnation_participant(&page)
            .expect("the Client participant stays")
            .progress,
        "held:unavailable",
        "a replacement connection never verifies the participant by itself"
    );
    drop(replacement);

    // 8: a Host restart keeps the durable snapshot; the restart itself
    // completes nothing and the same Client participant survives.
    let mut client = served.restart().await;
    let page = local_deletion_page(&served.handle).await;
    assert_ne!(page.operations[0].phase, DeletionPhaseWire::Completed);
    let participant = client_incarnation_participant(&page)
        .expect("the Client participant must survive the restart");
    assert_eq!(participant.progress, "held:unavailable");

    // 6: the reconnected Client reads frames, answers the bounded demand, and
    // only then does the durable participant verify and completion commit.
    let handle = Arc::clone(&served.handle);
    let page = drive_until(&handle, &mut client, DeletionPhaseWire::Completed)
        .await
        .expect("the reconnected Client must let the operation complete");
    let participant = client_incarnation_participant(&page)
        .expect("the completed operation still reports the Client participant");
    assert_eq!(participant.progress, "verified");
    assert_eq!(served.canonical_remainder(TARGET).await, 0);
    assert!(db_target_hits(&served.dir.join("app.db"), TARGET).is_empty());
    served.server.abort();
}

/// Stage 6 deletion final review, Blocker B1: the management Memory view is a
/// body-bearing first-party read. After a Host restart cleared the in-memory
/// delivery evidence of the earlier non-target round, an incarnation that
/// receives the target-bearing Memory only through the management view must
/// still be snapshotted as a required participant by the serving Host's
/// control inlet, and its own local erasure is what verifies it and lets
/// global completion commit.
#[tokio::test]
async fn stage6_management_view_memory_body_is_a_required_client_participant() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    // The owner turn and the companion reply carry no target: the Memory the
    // Learning pass forms is the only target-bearing surface, and the current
    // list page is the only Client-facing read that renders its body.
    let owner = "please keep this note for later";
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (on_learning_formation(true), Call::text(formation_create())),
            (
                on_latest_owner(owner),
                Call::text(String::from("I will keep that in mind.")),
            ),
        ],
        &[],
    ));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE, cmds::CAPABILITY_LEARNING],
    )
    .await;
    let (_round, _stream, reply) = send_round(served.client(), owner)
        .await
        .expect("the note round must complete");
    assert!(
        !reply.contains(TARGET),
        "the streamed reply must not carry the target: {reply}"
    );
    assert_absent_all(
        "pre-deletion history",
        &history_texts(served.client()).await,
        TARGET,
    );
    wait_for_target_memory_row(&served).await;

    // A Host restart drops the Host-memory delivery evidence; the durable
    // Memory survives and the reconnected incarnation has received no
    // target-bearing body in this serving process yet.
    let mut client = served.restart().await;
    let body = memory_view(&mut client).await;
    assert!(
        body.contains(TARGET),
        "the management view hands the target-bearing Memory over: {body}"
    );

    // The first-party request only stages; the serving Host's control inlet
    // snapshots the required participants and starts the operation.
    let outcome = request_deletion(&mut client, TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(
        outcome,
        ManagementOutcome::NeedsClarification,
        "the Client intent only stages"
    );
    served.client = Some(client);
    let current = confirm_deletion_via_serving_control(&mut served).await;

    // The durable participant snapshot names the incarnation whose only
    // target-bearing delivery was the management Memory view.
    let page = local_deletion_page(&served.handle).await;
    let participant = client_incarnation_participant(&page)
        .expect("the view-delivered Client incarnation is a required participant");
    assert!(
        participant.sweep >= current.sweep.as_u64(),
        "the Client participant belongs to the current sweep: {participant:?}"
    );

    // While the operation is unfinished, a fresh management Memory view is
    // covered by the current condition: no covered body is displayed again,
    // whether the row is still present (withheld) or already erased.
    let body = memory_view(served.client()).await;
    assert_absent("covered management view", &body, TARGET);

    // The Client answers the bounded demand and only then does the operation
    // reach the sealed global completion.
    let handle = Arc::clone(&served.handle);
    let page = drive_until(&handle, served.client(), DeletionPhaseWire::Completed)
        .await
        .expect("the operation must complete");
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Completed);
    let participant = client_incarnation_participant(&page)
        .expect("the completed operation still reports the Client participant");
    assert_eq!(
        participant.progress, "verified",
        "the Client's own local erasure pass is the verification premise"
    );
    assert_eq!(served.canonical_remainder(TARGET).await, 0);
    assert!(db_target_hits(&served.dir.join("app.db"), TARGET).is_empty());
    served.server.abort();
}

// ---------------------------------------------------------------------------
// E2E 2: usage / cost / cap
// ---------------------------------------------------------------------------

/// One reviewed-route usage report: `(input, cached, output)` token counts
/// whose cost under the first-party `gpt-4o-mini` rates is exactly
/// representable in micro-USD.
fn reported_cost_micros(input: u64, cached: u64, output: u64) -> u64 {
    // Reviewed rates are micro-USD per 1,000,000 tokens:
    // input 150_000, cached 75_000, output 600_000.
    (input - cached) * 150_000 / 1_000_000
        + cached * 75_000 / 1_000_000
        + output * 600_000 / 1_000_000
}

/// One bounded first-party usage read over the socket.
async fn usage_page(client: &mut Client) -> UsageSummaryPage {
    let answer = ask(
        client,
        WirePayload::UsageSummaryRequest(UsageSummaryRequest {
            from: None,
            to: None,
            provider: None,
            model: None,
            consumer: None,
            purpose: None,
            status: None,
            cursor: None,
            limit: None,
        }),
        "usage",
    )
    .await
    .expect("the usage read must answer");
    let WirePayload::UsageSummaryResponse(UsageSummaryResponse::Page(page)) = answer else {
        panic!("the usage read must answer a page: {answer:?}");
    };
    page
}

/// Polls the first-party usage surface until one row for `consumer` exists.
///
/// The usage fact settles inside dispatch *before* the caller can adopt or
/// commit the provider output, so observing the row orders the delayed
/// adoption/commit attempt that follows it: a test can assert the refusal is
/// decided after the parked call actually resumed, not before it.
async fn wait_for_usage_consumer(client: &mut Client, consumer: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let page = usage_page(client).await;
        if page.rows.iter().any(|row| row.consumer == consumer) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no {consumer} usage row appeared: {:?}",
            page.rows
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// One bounded usage read with an explicit provider filter, so the page names
/// that provider's cap slots (the system scope is always included).
async fn usage_page_for(client: &mut Client, provider: &str) -> UsageSummaryPage {
    let answer = ask(
        client,
        WirePayload::UsageSummaryRequest(UsageSummaryRequest {
            from: None,
            to: None,
            provider: Some(provider.to_owned()),
            model: None,
            consumer: None,
            purpose: None,
            status: None,
            cursor: None,
            limit: None,
        }),
        "provider usage",
    )
    .await
    .expect("the usage read must answer");
    let WirePayload::UsageSummaryResponse(UsageSummaryResponse::Page(page)) = answer else {
        panic!("the usage read must answer a page: {answer:?}");
    };
    page
}

/// Sets or updates one provider's monthly cap through the first-party
/// management intent and returns the typed outcome.
async fn set_provider_monthly_cap(
    client: &mut Client,
    intent_id: CommandWireId,
    provider: &str,
    limit_micros: u64,
) -> ManagementOutcome {
    let page = usage_page_for(client, provider).await;
    let mark = cmds::usage_cap_mark_for(&page, "provider", Some(provider), "monthly_utc")
        .expect("the page names the provider monthly slot")
        .to_string();
    let answer = ask(
        client,
        WirePayload::ManagementIntent(cmds::usage_cap_intent(
            intent_id,
            &mark,
            "provider",
            Some(provider),
            "monthly_utc",
            "USD",
            limit_micros,
        )),
        "provider-cap",
    )
    .await
    .expect("the cap intent must answer");
    let WirePayload::ManagementOutcome(outcome) = answer else {
        panic!("the cap intent must answer an outcome: {answer:?}");
    };
    outcome
}

/// Sets or updates the system daily cap through the first-party management
/// intent and returns the typed outcome.
async fn set_system_daily_cap(
    client: &mut Client,
    intent_id: CommandWireId,
    limit_micros: u64,
) -> ManagementOutcome {
    let page = usage_page(client).await;
    let mark = cmds::usage_cap_mark_for(&page, "system", None, "daily_utc")
        .expect("the page names the system daily slot")
        .to_string();
    let answer = ask(
        client,
        WirePayload::ManagementIntent(cmds::usage_cap_intent(
            intent_id,
            &mark,
            "system",
            None,
            "daily_utc",
            "USD",
            limit_micros,
        )),
        "usage-cap",
    )
    .await
    .expect("the cap intent must answer");
    let WirePayload::ManagementOutcome(outcome) = answer else {
        panic!("the cap intent must answer an outcome: {answer:?}");
    };
    outcome
}

/// The usage page's first row for one consumer.
fn row_for<'a>(
    page: &'a UsageSummaryPage,
    consumer: &str,
) -> &'a ene_api::v1::usage::UsageSummaryRowView {
    page.rows
        .iter()
        .find(|row| row.consumer == consumer)
        .unwrap_or_else(|| panic!("a {consumer} row exists: {:?}", page.rows))
}

fn assert_reported_cost(row: &ene_api::v1::usage::UsageSummaryRowView, label: &str) {
    let tokens = row.tokens.as_ref().expect("a reported row carries tokens");
    assert_eq!(row.status, "reported", "{label} must be Reported");
    let cost = row.cost.as_ref().expect("a reported row carries a cost");
    let expected = reported_cost_micros(
        tokens.input_tokens,
        tokens.cached_input_tokens,
        tokens.output_tokens,
    );
    assert_eq!(cost.total.micros, expected, "{label} total cost");
    assert_eq!(
        cost.input.micros + cost.cached_input.micros + cost.output.micros,
        cost.total.micros,
        "{label} cost components must sum to the total"
    );
    assert_eq!(cost.total.currency, "USD");
}

/// E2E 2: dialogue, Learning and Task Agent provider calls appear in the
/// first-party usage / cost surface with their Reported token split and cost
/// breakdown, an Unknown settlement is never rendered as zero, and the
/// historical cost is bound to the admission pricing snapshot (a later
/// admission under a different snapshot does not reprice it).
#[tokio::test]
async fn stage6_usage_cost_reported_unknown_and_historical_snapshot() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let proposal = task_reply(serde_json::json!({
        "kind": "propose_task",
        "purpose": "write a report from input.txt",
    }));
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_latest_owner("please read input.txt and write report.md"),
                Call::text(proposal),
            ),
            (
                on_task_agent_turn(0),
                Call::reported(r#"{"tool":"read","path":"input.txt"}"#, 1_000, 0, 500),
            ),
            (
                on_task_agent_turn(1),
                Call::reported(
                    r##"{"tool":"create","path":"report.md","content":"# Report\nnotes"}"##,
                    2_000,
                    1_000,
                    500,
                ),
            ),
            (
                on_task_agent_turn(2),
                Call::reported(r##"{"final":"created report.md"}"##, 1_000, 0, 500),
            ),
            (
                on_learning_formation(true),
                Call::reported(formation_create(), 2_000, 0, 500),
            ),
            (
                on_learning_formation(false),
                Call::reported(formation_update(), 200, 0, 40),
            ),
            (
                on_latest_owner("please remember the kettle"),
                Call::reported("Noted the kettle.", 1_000, 400, 200),
            ),
            (
                on_latest_owner("what about the kettle"),
                Call::text("The provider reported no usage for this one."),
            ),
        ],
        &[],
    ));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE, cmds::CAPABILITY_LEARNING],
    )
    .await;
    let workspace = dir.join("usage-workspace");
    std::fs::create_dir_all(&workspace).expect("the workspace directory creates");
    std::fs::write(workspace.join("input.txt"), b"notes").expect("input fixture");
    select_workspace(served.client(), &workspace)
        .await
        .expect("workspace must select");
    let (round, stream, _) = send_round(served.client(), "please remember the kettle")
        .await
        .expect("the reported round must complete");
    confirm_round(served.client(), &round, stream).await;
    let (round, stream, _) =
        send_round(served.client(), "please read input.txt and write report.md")
            .await
            .expect("the propose round must complete");
    confirm_round(served.client(), &round, stream).await;
    wait_task_progress(served.client(), "completed", 1)
        .await
        .expect("the task must complete");
    let (round, stream, _) = send_round(served.client(), "what about the kettle")
        .await
        .expect("the unknown round must complete");
    confirm_round(served.client(), &round, stream).await;

    let page = usage_page(served.client()).await;
    // Attribution: one Reported row per consumer, each with the exact
    // reviewed-rate breakdown.
    let dialogue = page
        .rows
        .iter()
        .find(|row| row.consumer == "companion_dialogue" && row.status == "reported")
        .expect("a Reported dialogue row");
    assert_eq!(dialogue.purpose, "dialogue_response");
    assert_eq!(dialogue.provider, "openai");
    assert_eq!(dialogue.model, MODEL);
    assert_eq!(
        dialogue.tokens.as_ref().map(|tokens| (
            tokens.input_tokens,
            tokens.cached_input_tokens,
            tokens.output_tokens
        )),
        Some((1_000, 400, 200))
    );
    assert_reported_cost(dialogue, "dialogue");
    let learning = page
        .rows
        .iter()
        .find(|row| row.consumer == "companion_learning" && row.status == "reported")
        .expect("a Reported learning row");
    assert_eq!(learning.purpose, "memory_formation");
    assert_reported_cost(learning, "learning");
    let task = row_for(&page, "task_agent");
    assert_eq!(task.purpose, "task_agent_turn");
    assert_reported_cost(task, "task agent");
    // Reported and Unknown are distinct states: the Unknown row carries no
    // token counts and no zero cost.
    let unknown = page
        .rows
        .iter()
        .find(|row| row.status == "unknown")
        .expect("an Unknown row");
    assert_eq!(unknown.consumer, "companion_dialogue");
    assert!(unknown.tokens.is_none(), "Unknown is never zero tokens");
    assert!(unknown.cost.is_none(), "Unknown is never a zero cost");
    // Read-only: a second read answers the same rows and settles nothing.
    let again = usage_page(served.client()).await;
    assert_eq!(again.rows, page.rows, "the read changes nothing durable");

    // Historical pricing: a settlement admitted under a different reviewed
    // snapshot keeps its own rate. The current milestone has no runtime
    // catalog-update producer, so the changed-price premise is constructed as
    // a durable fixture through the production claim/settlement boundary (the
    // same shape the store's `usage_query` suite injects).
    let synthetic = ene_inference::pricing::PricingSnapshot {
        provider: String::from("openai"),
        model: String::from(MODEL),
        currency: ene_primitive::CurrencyCode::Usd,
        input_rate: ene_inference::cost::TokenRate::from_micros_per_million(300_000),
        cached_input_rate: ene_inference::cost::TokenRate::from_micros_per_million(150_000),
        output_rate: ene_inference::cost::TokenRate::from_micros_per_million(1_200_000),
        effective_at: WallClockWithTz::now(),
        source_revision: ene_inference::pricing::PricingCatalogRevision::new(2),
    };
    {
        use ene_inference::{
            AttemptBeginOutcome, InferenceAttempt, InferenceAttemptRepository as _,
            InferenceTicketId, UsageFact, UsageRepository as _, UsageSource,
        };
        use ene_permission::{CapabilityKind, ConsentRepository as _, ConsumerKind, PurposeKind};
        let store = ene_store::Store::open(&dir.join("app.db"))
            .await
            .expect("the state database opens for the pricing fixture");
        let consent = store
            .load_current(CapabilityKind::Dialogue)
            .await
            .expect("the consent read must answer")
            .expect("the dialogue consent exists");
        let credential_set = store
            .current_set_revision()
            .await
            .expect("the credential-set revision must read");
        let ticket = InferenceTicketId(RawId::new());
        assert_eq!(
            store
                .begin_inference_attempt(InferenceAttempt {
                    ticket,
                    consumer: ConsumerKind::CompanionDialogue,
                    capability: CapabilityKind::Dialogue,
                    purpose: PurposeKind::DialogueResponse,
                    expected_consent: (consent.id.clone(), consent.rev),
                    expected_credential_set: credential_set,
                    provider: String::from("openai"),
                    model: String::from(MODEL),
                    data_use: Vec::new(),
                    task_agent: None,
                    pricing: Some(synthetic.clone()),
                    usage_estimate: Some(UsageEstimate {
                        input_tokens_upper_bound: 1_000,
                        output_tokens_upper_bound: 1_000,
                    }),
                })
                .await
                .expect("the synthetic claim must answer"),
            AttemptBeginOutcome::Started
        );
        store
            .record_usage(UsageFact {
                ticket,
                provider: String::from("openai"),
                model: String::from(MODEL),
                input_tokens: Some(1_000),
                cached_input_tokens: Some(0),
                output_tokens: Some(100),
                source: UsageSource::Reported,
            })
            .await
            .expect("the synthetic settlement must record");
    }
    let after = usage_page(served.client()).await;
    // The synthetic ticket settled at the revision-2 rates
    // (1000 x 300_000 + 100 x 1_200_000 per million tokens = 300 + 120).
    let synthetic_row = after
        .rows
        .iter()
        .find(|row| {
            row.consumer == "companion_dialogue"
                && row
                    .cost
                    .as_ref()
                    .is_some_and(|cost| cost.total.micros == 420)
        })
        .expect("the revision-2 settlement keeps its own rate");
    assert!(synthetic_row.cost.is_some());
    // Every pre-existing row keeps the exact cost it was admitted under.
    for before_row in &page.rows {
        let same = after
            .rows
            .iter()
            .find(|row| {
                row.started_at == before_row.started_at
                    && row.consumer == before_row.consumer
                    && row.purpose == before_row.purpose
            })
            .expect("the pre-existing row is still present");
        assert_eq!(
            same.cost, before_row.cost,
            "a later snapshot never reprices a historical row"
        );
    }
    assert_eq!(
        after
            .caps
            .iter()
            .filter(|cap| cap.scope == "system")
            .count(),
        2,
        "the system daily and monthly slots are always reported"
    );
    served.server.abort();
}

/// The safe upper bound the cap tests reserve: 1,000,000 input tokens at the
/// non-cached input rate plus 100,000 output tokens at the output rate under
/// the reviewed `gpt-4o-mini` snapshot plus the one-micro-unit allowance for
/// the separately rounded input components = 210,001 micro-USD.
fn cap_estimate() -> UsageEstimate {
    UsageEstimate {
        input_tokens_upper_bound: 1_000_000,
        output_tokens_upper_bound: 100_000,
    }
}

const CAP_UPPER_BOUND_MICROS: u64 = 210_001;

/// E2E 2: the reservation linearization. While one claimed call's reservation
/// holds the last cap slot, a second send is refused with zero provider bytes;
/// a Reported settlement releases the unused reservation and the next send is
/// admitted.
#[tokio::test]
async fn stage6_usage_cap_reservation_refuses_the_second_concurrent_send() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let first = String::from("please remember the kettle");
    let second = String::from("what about the kettle");
    let third = String::from("one more kettle note");
    let transport = Arc::new(
        ScriptedTransport::new(
            vec![
                (
                    on_learning_formation(true),
                    Call::reported(formation_create(), 2_000, 0, 500),
                ),
                (
                    on_latest_owner(&first),
                    Call::reported("Noted the kettle.", 1_000, 400, 200),
                ),
                (
                    on_latest_owner(&second),
                    Call::reported("The kettle is noted.", 1_000, 0, 100),
                ),
                (
                    on_latest_owner(&third),
                    Call::reported("Still noted.", 1_000, 0, 100),
                ),
            ],
            &[],
        )
        .with_estimate(cap_estimate()),
    );
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE, cmds::CAPABILITY_LEARNING],
    )
    .await;
    // A loose system daily cap plus a tight provider monthly cap: the
    // provider scope is the binding one for the next send.
    let outcome = set_system_daily_cap(
        served.client(),
        CommandWireId(uuid::Uuid::new_v4()),
        1_000_000,
    )
    .await;
    assert!(
        matches!(outcome, ManagementOutcome::StoredAsRuleView { .. }),
        "the system cap must store, got {outcome:?}"
    );
    let outcome = set_provider_monthly_cap(
        served.client(),
        CommandWireId(uuid::Uuid::new_v4()),
        "openai",
        CAP_UPPER_BOUND_MICROS + 40_000,
    )
    .await;
    assert!(
        matches!(outcome, ManagementOutcome::StoredAsRuleView { .. }),
        "the provider cap must store, got {outcome:?}"
    );
    // The formation call parks after its claim: its reservation holds the
    // last provider slot (250,000 limit against a 210,000 bound while the
    // system daily cap alone would still admit the send).
    transport.block_input(on_learning_formation(true));
    let (round, stream, _) = send_round(served.client(), &first)
        .await
        .expect("the first round must complete");
    confirm_round(served.client(), &round, stream).await;
    transport.wait_parked(1).await;
    // A concurrent dialogue send cannot claim: zero provider bytes, never a
    // second attempt or reservation.
    let sends_before = transport.sends();
    let raced = send_round_raw(served.client(), &second).await;
    assert!(
        raced.is_err()
            || raced.as_ref().is_ok_and(|(_, _, text, close)| {
                *close != ene_api::v1::round::StreamClose::Completed && text.is_empty()
            }),
        "a cap-refused send never completes a reply: {raced:?}"
    );
    assert_eq!(
        transport.sends(),
        sends_before,
        "the provider receives zero bytes for a cap-refused claim"
    );
    // Which scope refused: the provider monthly slot is out of room while the
    // system daily slot alone would still admit the send.
    let observed = usage_page_for(served.client(), "openai").await;
    let provider_slot = observed
        .caps
        .iter()
        .find(|cap| {
            cap.scope == "provider"
                && cap.provider.as_deref() == Some("openai")
                && cap.window == "monthly_utc"
        })
        .expect("the provider monthly slot");
    let UsageCapConsumptionView::Known {
        remaining: provider_remaining,
        ..
    } = &provider_slot
        .stored
        .as_ref()
        .expect("the provider cap is stored")
        .consumption
    else {
        panic!("the provider consumption must be Known: {provider_slot:?}");
    };
    assert!(
        provider_remaining.micros < CAP_UPPER_BOUND_MICROS,
        "the provider monthly cap has no room for the refused bound: {provider_slot:?}"
    );
    let system_slot = observed
        .caps
        .iter()
        .find(|cap| cap.scope == "system" && cap.window == "daily_utc")
        .expect("the system daily slot");
    let UsageCapConsumptionView::Known {
        remaining: system_remaining,
        ..
    } = &system_slot
        .stored
        .as_ref()
        .expect("the system cap is stored")
        .consumption
    else {
        panic!("the system consumption must be Known: {system_slot:?}");
    };
    assert!(
        system_remaining.micros > CAP_UPPER_BOUND_MICROS,
        "the system cap alone would admit the send: {system_slot:?}"
    );
    // Release the reservation: the Reported settlement releases the unused
    // bound, and the next send fits.
    transport.release_blocked();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let page = loop {
        let page = usage_page(served.client()).await;
        let settled = page.rows.iter().any(|row| {
            row.consumer == "companion_learning"
                && row.status == "reported"
                && row
                    .cost
                    .as_ref()
                    .is_some_and(|cost| cost.total.micros == 600)
        });
        if settled {
            break page;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the formation settlement did not land: {:?}",
            page.rows
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let system_daily = page
        .caps
        .iter()
        .find(|cap| cap.scope == "system" && cap.window == "daily_utc")
        .expect("the system daily slot");
    let stored = system_daily.stored.as_ref().expect("the cap is stored");
    let UsageCapConsumptionView::Known {
        committed_reported,
        consumed,
        held,
        ..
    } = &stored.consumption
    else {
        panic!("the consumption must be Known: {system_daily:?}");
    };
    assert_eq!(
        committed_reported.micros, 840,
        "the window aggregates every Reported settlement (dialogue 240 + formation 600)"
    );
    assert_eq!(consumed.micros, 840);
    assert!(!held, "the settled reservation frees the slot");
    // The next send is admitted and settles Reported.
    let (round, stream, reply) = send_round(served.client(), &third)
        .await
        .expect("the next send must be admitted");
    assert!(reply.contains("Still noted"), "{reply}");
    confirm_round(served.client(), &round, stream).await;
    served.server.abort();
}

/// E2E 2: Unknown usage is never released or zeroed. A `ResponseLost` call
/// keeps its reserved upper bound counted against the cap, an orphaned
/// reservation is settled `CommittedUnknown` by the restart, a new send stays
/// refused with zero provider bytes, and the first-party cap update passes
/// currentness, replay, and stale-premise checks.
#[tokio::test]
async fn stage6_usage_cap_unknown_accounting_and_update_currentness() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let lost = String::from("the lost kettle note");
    let parked = String::from("the parked kettle note");
    let after = String::from("the admitted kettle note");
    let transport = Arc::new(
        ScriptedTransport::new(
            vec![
                (on_latest_owner(&lost), Call::lost()),
                (
                    on_latest_owner(&parked),
                    Call::reported("parked", 1_000, 0, 100),
                ),
                (
                    on_latest_owner(&after),
                    Call::reported("admitted", 1_000, 0, 100),
                ),
            ],
            &[],
        )
        .with_estimate(cap_estimate()),
    );
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE],
    )
    .await;
    let outcome = set_system_daily_cap(
        served.client(),
        CommandWireId(uuid::Uuid::new_v4()),
        CAP_UPPER_BOUND_MICROS + 40_000,
    )
    .await;
    assert!(matches!(
        outcome,
        ManagementOutcome::StoredAsRuleView { .. }
    ));
    // ResponseLost: the attempt may have run, so the upper bound stays counted.
    let raced = send_round_raw(served.client(), &lost).await;
    assert!(
        raced.is_err()
            || raced.as_ref().is_ok_and(|(_, _, text, close)| {
                *close != ene_api::v1::round::StreamClose::Completed && text.is_empty()
            }),
        "a lost provider response never completes a reply: {raced:?}"
    );
    let page = usage_page(served.client()).await;
    let unknown = page
        .rows
        .iter()
        .find(|row| row.status == "unknown" && row.reserved.is_some())
        .expect("an Unknown row keeps its reservation counted");
    assert_eq!(
        unknown.reserved.as_ref().map(|money| money.micros),
        Some(CAP_UPPER_BOUND_MICROS),
        "Unknown keeps the reserved upper bound"
    );
    assert!(unknown.tokens.is_none() && unknown.cost.is_none());
    // A new send cannot fit while the Unknown counts.
    let sends_before = transport.sends();
    let refused = send_round_raw(served.client(), "one more note").await;
    assert!(
        refused.is_err()
            || refused.as_ref().is_ok_and(|(_, _, text, close)| {
                *close != ene_api::v1::round::StreamClose::Completed && text.is_empty()
            }),
        "a cap-refused send never completes: {refused:?}"
    );
    assert_eq!(transport.sends(), sends_before);

    // First-party cap update: currentness, replay, and stale premise.
    let observed = usage_page(served.client()).await;
    let observed_mark = cmds::usage_cap_mark_for(&observed, "system", None, "daily_utc")
        .expect("the slot mark")
        .to_string();
    let old_mark = observed_mark.clone();
    let intent_id = CommandWireId(uuid::Uuid::new_v4());
    let outcome = set_system_daily_cap_mark(
        served.client(),
        intent_id,
        &observed_mark,
        2 * CAP_UPPER_BOUND_MICROS,
    )
    .await;
    assert!(
        matches!(outcome, ManagementOutcome::StoredAsRuleView { .. }),
        "the cap raise must store, got {outcome:?}"
    );
    // An exact replay of the same intent id observes the first decision and
    // never applies a second write.
    let replay = set_system_daily_cap_mark(
        served.client(),
        intent_id,
        &observed_mark,
        2 * CAP_UPPER_BOUND_MICROS,
    )
    .await;
    assert!(
        matches!(replay, ManagementOutcome::StoredAsRuleView { .. }),
        "the replayed intent observes the stored decision, got {replay:?}"
    );
    // A conflicting reuse of the key (same id, different body) decides
    // nothing and never rewrites the cap.
    let conflicting =
        set_system_daily_cap_mark(served.client(), intent_id, &observed_mark, 999).await;
    assert!(
        matches!(conflicting, ManagementOutcome::NeedsClarification),
        "a conflicting reuse is clarified, got {conflicting:?}"
    );
    let after_replay = usage_page(served.client()).await;
    assert!(
        after_replay
            .caps
            .iter()
            .filter_map(|cap| cap.stored.as_ref())
            .all(|stored| stored.limit.micros != 999),
        "no replay path rewrites the cap: {after_replay:?}"
    );
    // A stale base view is refused and decides nothing.
    let stale = set_system_daily_cap_mark(
        served.client(),
        CommandWireId(uuid::Uuid::new_v4()),
        &old_mark,
        5,
    )
    .await;
    assert!(
        matches!(stale, ManagementOutcome::StaleBaseView { .. }),
        "a stale cap premise is refused, got {stale:?}"
    );

    // Crash with an orphaned reservation: the Host stops, and a claim left
    // mid-flight by the stopped process (built here through the production
    // claim boundary, because only a real crash leaves a Reserved row) is
    // reconciled to CommittedUnknown by the restart, still counted.
    served.stop().await;
    {
        use ene_credential::CredentialSetRepository as _;
        use ene_inference::pricing::{PricingCatalog, PricingResolution};
        use ene_inference::{InferenceAttempt, InferenceAttemptRepository as _, InferenceTicketId};
        use ene_permission::{CapabilityKind, ConsentRepository as _, ConsumerKind, PurposeKind};
        let store = ene_store::Store::open(&served.dir.join("app.db"))
            .await
            .expect("the state database opens for the crash fixture");
        let consent = store
            .load_current(CapabilityKind::Dialogue)
            .await
            .expect("the consent read must answer")
            .expect("the dialogue consent exists");
        let credential_set = store
            .current_set_revision()
            .await
            .expect("the credential-set revision must read");
        let PricingResolution::Priced(pricing) = PricingCatalog::first_party()
            .expect("the reviewed catalog must be valid")
            .resolve("openai", MODEL, WallClockWithTz::now())
        else {
            panic!("the reviewed route must be priced");
        };
        assert_eq!(
            store
                .begin_inference_attempt(InferenceAttempt {
                    ticket: InferenceTicketId(RawId::new()),
                    consumer: ConsumerKind::CompanionDialogue,
                    capability: CapabilityKind::Dialogue,
                    purpose: PurposeKind::DialogueResponse,
                    expected_consent: (consent.id.clone(), consent.rev),
                    expected_credential_set: credential_set,
                    provider: String::from("openai"),
                    model: String::from(MODEL),
                    data_use: Vec::new(),
                    task_agent: None,
                    pricing: Some(pricing),
                    usage_estimate: Some(cap_estimate()),
                })
                .await
                .expect("the crash claim must answer"),
            ene_inference::AttemptBeginOutcome::Started,
            "the crash claim reserves the last slot before the process stops"
        );
    }
    let mut client = served.serve().await;
    let page = usage_page(&mut client).await;
    let unknowns: Vec<_> = page
        .rows
        .iter()
        .filter(|row| row.status == "unknown" && row.reserved.is_some())
        .collect();
    assert_eq!(
        unknowns.len(),
        2,
        "the orphaned reservation settles Unknown after the restart: {:?}",
        page.rows
    );
    let system_daily = page
        .caps
        .iter()
        .find(|cap| cap.scope == "system" && cap.window == "daily_utc")
        .expect("the system daily slot");
    let stored = system_daily.stored.as_ref().expect("the raised cap");
    let UsageCapConsumptionView::Known { consumed, .. } = &stored.consumption else {
        panic!("the consumption must be Known: {system_daily:?}");
    };
    assert_eq!(
        consumed.micros,
        CAP_UPPER_BOUND_MICROS * 2,
        "the restart keeps every unknown upper bound counted: {system_daily:?}"
    );
    // A send that would overshoot the raised limit is still refused with zero
    // provider bytes; after a further first-party raise it is admitted.
    let refused = send_round_raw(&mut client, "another note").await;
    assert!(
        refused.is_err()
            || refused.as_ref().is_ok_and(|(_, _, text, close)| {
                *close != ene_api::v1::round::StreamClose::Completed && text.is_empty()
            }),
        "the counted Unknown keeps the cap closed: {refused:?}"
    );
    let sends_before = transport.sends();
    let outcome = set_system_daily_cap(
        &mut client,
        CommandWireId(uuid::Uuid::new_v4()),
        CAP_UPPER_BOUND_MICROS * 10,
    )
    .await;
    assert!(matches!(
        outcome,
        ManagementOutcome::StoredAsRuleView { .. }
    ));
    let (round, stream, reply) = send_round(&mut client, &after)
        .await
        .expect("the raised cap admits the next send");
    assert!(reply.contains("admitted"), "{reply}");
    confirm_round(&mut client, &round, stream).await;
    assert!(transport.sends() > sends_before);
    served.server.abort();
}

/// Sets the system daily cap on an explicit mark and intent id, for the
/// replay and stale-premise legs.
async fn set_system_daily_cap_mark(
    client: &mut Client,
    intent_id: CommandWireId,
    mark: &str,
    limit_micros: u64,
) -> ManagementOutcome {
    let answer = ask(
        client,
        WirePayload::ManagementIntent(cmds::usage_cap_intent(
            intent_id,
            mark,
            "system",
            None,
            "daily_utc",
            "USD",
            limit_micros,
        )),
        "stale-usage-cap",
    )
    .await
    .expect("the cap intent must answer");
    let WirePayload::ManagementOutcome(outcome) = answer else {
        panic!("the cap intent must answer an outcome: {answer:?}");
    };
    outcome
}

// ---------------------------------------------------------------------------
// E2E 3: credential secret non-exposure
// ---------------------------------------------------------------------------

/// E2E 3: a registered secret never reaches provider request bodies, History,
/// Memory, Task data, presentation bodies, the management view, or the
/// structured errors the first-party client observes — across dialogue,
/// Learning, Task Agent, management, and provider-failure paths.
#[tokio::test]
async fn stage6_registered_secret_absent_from_every_first_party_surface() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let redacted = format!(
        "please remember the passphrase {}",
        ene_credential::REDACTED_CREDENTIAL
    );
    let proposal = task_reply(serde_json::json!({
        "kind": "propose_task",
        "purpose": format!("write a report about {SECRET}"),
    }));
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_learning_formation(true),
                Call::reported(formation_create_with(SECRET), 2_000, 0, 500),
            ),
            (
                on_learning_formation(false),
                Call::reported(formation_update_with(SECRET), 200, 0, 40),
            ),
            (
                on_latest_owner(&redacted),
                Call::reported(
                    format!("Noted; the passphrase {SECRET} stays with you."),
                    1_000,
                    400,
                    200,
                ),
            ),
            (
                on_latest_owner("please read input.txt and write report.md"),
                Call::text(proposal),
            ),
            (
                on_task_agent_turn(0),
                Call::text(r#"{"tool":"read","path":"input.txt"}"#),
            ),
            (
                on_task_agent_turn(1),
                Call::text(format!(
                    r##"{{"tool":"create","path":"report.md","content":"# Report {SECRET}"}}"##
                )),
            ),
            (
                on_task_agent_turn(2),
                Call::text(format!(
                    r##"{{"final":"created report.md quoting {SECRET}"}}"##
                )),
            ),
            (
                on_latest_owner(&format!(
                    "an error path with {}",
                    ene_credential::REDACTED_CREDENTIAL
                )),
                Call::lost(),
            ),
        ],
        &[],
    ));
    let mut served = Served::start(
        dir.clone(),
        memory_store_with(SECRET),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE, cmds::CAPABILITY_LEARNING],
    )
    .await;
    let workspace = dir.join("secret-workspace");
    std::fs::create_dir_all(&workspace).expect("the workspace directory creates");
    std::fs::write(workspace.join("input.txt"), b"notes").expect("input fixture");
    select_workspace(served.client(), &workspace)
        .await
        .expect("workspace must select");
    // Dialogue + Learning: the owner input, the provider reply, and the
    // formation answer all quote the registered value.
    let (round, stream, reply) = send_round(
        served.client(),
        &format!("please remember the passphrase {SECRET}"),
    )
    .await
    .expect("the secret round must complete");
    // The provider answer is synthetic (a real model never sees the scrubbed
    // value), so only the durable adoption is the invariant: the History row
    // must carry the redaction marker instead of the value.
    confirm_round(served.client(), &round, stream).await;
    let _ = reply;
    // Task Agent: the directive purpose and the final answer quote it.
    let (round, stream, _) =
        send_round(served.client(), "please read input.txt and write report.md")
            .await
            .expect("the propose round must complete");
    confirm_round(served.client(), &round, stream).await;
    wait_task_progress(served.client(), "completed", 1)
        .await
        .expect("the task must complete");
    // Provider failure: the request must already be scrubbed.
    let errored = send_round_raw(served.client(), &format!("an error path with {SECRET}")).await;
    match errored {
        Ok((_, _, text, _)) => assert_absent("errored round text", &text, SECRET),
        Err(rendered) => assert_absent("errored round rendering", &rendered, SECRET),
    }
    // Provider captures: every request body is scrubbed.
    assert_absent_all("provider request", &transport.input_texts(), SECRET);
    // Durable surfaces: History, Memory, the Task report and its sources, the
    // undelivered excerpts, and the whole state database.
    let history = history_texts(served.client()).await;
    assert_absent_all("history", &history, SECRET);
    assert_absent("memory view", &memory_view(served.client()).await, SECRET);
    let summary = fetch_summary(served.client(), "secret subscription")
        .await
        .expect("the subscription must answer");
    for item in &summary.items {
        assert_absent("presentation excerpt", &item.excerpt, SECRET);
    }
    assert_absent(
        "presentation summary debug",
        &format!("{summary:?}"),
        SECRET,
    );
    let tasks = list_tasks(served.client()).await.expect("tasks must list");
    let task = tasks.tasks.first().expect("the task exists");
    let report = ask(
        served.client(),
        WirePayload::GetTaskReport(cmds::task_report_request(&task.task.0, None, None)),
        "secret report",
    )
    .await
    .expect("the report must answer");
    assert_absent("task report debug", &format!("{report:?}"), SECRET);
    let WirePayload::TaskReportResponse(ene_api::v1::undelivered::TaskReportResponse::Page(page)) =
        report
    else {
        panic!("the report must answer a page: {report:?}");
    };
    // Every report source body (the purpose and the result rows) is scrubbed.
    for row in &page.rows {
        let Some(source) = row.source.as_ref() else {
            continue;
        };
        let body = ask(
            served.client(),
            WirePayload::GetReportSource(cmds::report_source_request(&source.0, None, None)),
            "secret report source",
        )
        .await
        .expect("the report source must answer");
        assert_absent("report source debug", &format!("{body:?}"), SECRET);
        if let WirePayload::ReportSourceResponse(
            ene_api::v1::undelivered::ReportSourceResponse::Page(source_page),
        ) = body
        {
            assert_absent("report source body", &source_page.text, SECRET);
        }
    }
    // Management view: credential/consent metadata renders refs, never values.
    let view = ask(
        served.client(),
        WirePayload::ManagementViewRequest(cmds::setup_view_request()),
        "secret view",
    )
    .await
    .expect("the view must answer");
    let WirePayload::ManagementView(view) = view else {
        panic!("the view must answer a view");
    };
    for section in &view.sections {
        assert_absent("management section", &section.body, SECRET);
    }
    assert_absent("management view debug", &format!("{view:?}"), SECRET);
    assert!(
        db_target_hits(&dir.join("app.db"), SECRET).is_empty(),
        "no durable table may carry the registered value: {:?}",
        db_target_hits(&dir.join("app.db"), SECRET)
    );
    served.server.abort();
}

/// E2E 3: registering a value sweeps its prior durable occurrences, and a
/// registration that commits while a provider call is parked leaves no raw
/// value durable anywhere.
#[tokio::test]
async fn stage6_credential_registration_sweeps_prior_occurrences_during_a_parked_send() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let rotated_round = format!("please remember the old passphrase {ROTATED_SECRET}");
    let parked_round = format!("please remember the passphrase {SECRET}");
    let rotated_redacted = format!(
        "please remember the old passphrase {}",
        ene_credential::REDACTED_CREDENTIAL
    );
    let parked_redacted = format!(
        "please remember the passphrase {}",
        ene_credential::REDACTED_CREDENTIAL
    );
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_latest_owner(&rotated_redacted),
                Call::text("Noted the old passphrase."),
            ),
            (
                on_latest_owner(&parked_redacted),
                Call::reported("Noted the passphrase.", 1_000, 400, 200),
            ),
        ],
        &[],
    ));
    let mut served = Served::start(
        dir.clone(),
        memory_store_with_rotated(SECRET, ROTATED_SECRET),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE],
    )
    .await;
    // The rotated value is not yet registered: the round's durable History
    // legitimately carries it raw, and the registration's approval sweep must
    // redact it.
    let (round, stream, _) = send_round(served.client(), &rotated_round)
        .await
        .expect("the pre-registration round must complete");
    confirm_round(served.client(), &round, stream).await;
    assert!(
        !db_target_hits(&dir.join("app.db"), ROTATED_SECRET).is_empty(),
        "the fixture must plant the not-yet-registered value"
    );
    // Stage the rotation; the Host-local approval commits it while the next
    // provider call is parked.
    let mark = view_mark(served.client()).await.expect("show");
    let staged = ask(
        served.client(),
        WirePayload::ManagementIntent(ManagementIntent {
            intent_id: CommandWireId(uuid::Uuid::new_v4()),
            kind: ManagementIntentKind::ConfigureCredentialIntent,
            target: ene_api::v1::management::credential_target("openai", "rotated"),
            base_view: BaseViewMark(mark),
            rationale: IntentRationaleWire {
                origin: RationaleOrigin::ManagementSurface,
                quote: None,
            },
        }),
        "rotate",
    )
    .await
    .expect("the rotation intent must answer");
    assert!(
        matches!(
            staged,
            WirePayload::ManagementOutcome(ManagementOutcome::HeldByOperation)
        ),
        "the rotation waits for the Host-local approval, got {staged:?}"
    );
    transport.block_input(on_latest_owner(&parked_redacted));
    let handle = Arc::clone(&served.handle);
    let barrier = Arc::clone(&transport);
    let mut parked = Box::pin(send_round_raw(served.client(), &parked_round));
    tokio::select! {
        result = parked.as_mut() => panic!("the parked round cannot finish before the sweep: {result:?}"),
        () = barrier.wait_parked(1) => {}
    }
    assert!(
        matches!(
            handle.approve_credential("openai", "rotated").await,
            Ok(true)
        ),
        "the Host-local approval must register the rotated value"
    );
    barrier.release_blocked();
    let (_round, _stream, reply, _close) = parked.await.expect("the parked round must answer");
    assert_absent("parked reply", &reply, SECRET);
    // The approval sweep removed the prior durable occurrence; nothing keeps
    // either value raw.
    assert!(
        db_target_hits(&dir.join("app.db"), ROTATED_SECRET).is_empty(),
        "the registration sweep must redact prior occurrences: {:?}",
        db_target_hits(&dir.join("app.db"), ROTATED_SECRET)
    );
    assert!(db_target_hits(&dir.join("app.db"), SECRET).is_empty());
    assert_absent_all("provider request", &transport.input_texts(), SECRET);
    assert_absent_all(
        "history",
        &history_texts(served.client()).await,
        ROTATED_SECRET,
    );
    served.server.abort();
}

// ---------------------------------------------------------------------------
// E2E 1 (M1): durable Client body-delivery evidence across a Host restart
// ---------------------------------------------------------------------------

/// M1: a Client that received a target-bearing copy before a Host restart must
/// still be snapshotted as a required `ClientIncarnation` by a later
/// serving-Host confirmation — the delivery evidence is durable, so the
/// restart must not clear it. The unreachable old incarnation is an explicit
/// hold, and only the same incarnation's reconnected, verified local erasure
/// lets the sealed global completion commit; the verified wipe then clears the
/// durable evidence.
#[tokio::test]
async fn stage6_client_delivery_evidence_survives_restart_before_admission() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let first = format!("please remember {TARGET} for me");
    let transport = Arc::new(ScriptedTransport::new(
        vec![(
            on_latest_owner(&first),
            Call::text(format!("I will keep {TARGET} in mind.")),
        )],
        &[],
    ));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE, cmds::CAPABILITY_LEARNING],
    )
    .await;
    // 1: the target-bearing copy reaches the Client in the first Host process.
    deliver_target_copy(&mut served).await;

    // 2: the Host restarts before any deletion exists. The in-memory delivery
    // tracking is gone; only the durable evidence can survive.
    let mut client = served.restart().await;

    // 3: stage the request through the reconnected Client, then drop the
    // connection so the confirmation meets the old incarnation unreachable.
    let outcome = request_deletion(&mut client, TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(
        outcome,
        ManagementOutcome::NeedsClarification,
        "the Client intent only stages"
    );
    served.client = None;
    drop(client);

    // 4: the serving Host's trusted confirmation snapshots the durable
    // evidence and names the incarnation that received the copy before the
    // restart.
    let current = confirm_deletion_via_serving_control(&mut served).await;
    let page = local_deletion_page(&served.handle).await;
    let participant = client_incarnation_participant(&page)
        .expect("the pre-restart delivery must stay a required participant");
    assert!(
        participant.sweep >= current.sweep.as_u64(),
        "the Client participant belongs to the current sweep: {participant:?}"
    );

    // 5: the unreachable incarnation is an explicit hold, never a completion.
    let page = drive_until_local(&served.handle, DeletionPhaseWire::Held).await;
    assert_ne!(
        page.operations[0].phase,
        DeletionPhaseWire::Completed,
        "an unreachable Client must never be presumed erased"
    );
    let participant =
        client_incarnation_participant(&page).expect("the snapshotted Client stays required");
    assert_eq!(
        participant.progress, "held:unavailable",
        "a disconnected incarnation is an explicit unreachable hold"
    );
    assert_eq!(
        served
            .handle
            .required_deletion_participants()
            .await
            .expect("the required snapshot must read")
            .iter()
            .filter(|owner| matches!(
                owner,
                ene_preservation::ParticipantOwnerRef::ClientIncarnation(_)
            ))
            .count(),
        1,
        "the restart keeps exactly the delivered incarnation's evidence"
    );

    // 6: the same Client boot incarnation reconnects (a new connection, same
    // identity). Only its verified local erasure lets the operation reach the
    // sealed global completion.
    let mut client = connect(&served.dir).await;
    let handle = Arc::clone(&served.handle);
    let page = drive_until(&handle, &mut client, DeletionPhaseWire::Completed)
        .await
        .expect("the reconnected Client must let the operation complete");
    let participant = client_incarnation_participant(&page)
        .expect("the completed operation still reports the Client participant");
    assert_eq!(
        participant.progress, "verified",
        "the Client's own local erasure pass is the verification premise"
    );
    assert_eq!(served.canonical_remainder(TARGET).await, 0);
    assert!(db_target_hits(&served.dir.join("app.db"), TARGET).is_empty());
    // The verified full-class wipe cleared the durable evidence: no later
    // admission claims a copy that no longer exists. Read it over the same
    // fresh store connection the canonical remainder probe uses.
    let store = ene_store::Store::open(&served.dir.join("app.db"))
        .await
        .expect("the state database opens");
    assert_eq!(
        store
            .client_delivery_evidence_incarnations(None, 100)
            .await
            .expect("the durable evidence must read"),
        Vec::new(),
        "a verified local erasure clears the delivery evidence"
    );
    served.server.abort();
}

// ---------------------------------------------------------------------------
// E2E 1 dialogue currentness (Stage 6 M2 / #1627)
// ---------------------------------------------------------------------------

/// The delayed reply's paraphrase: it never quotes the target, so only the
/// durable old-claim provenance can refuse it.
const DIALOGUE_RACE_PARAPHRASE: &str = "I still keep that detail in mind.";

/// Drives bounded serving ticks until no unfinished deletion operation
/// remains.
///
/// Used by a race leg whose foreground connection is parked on a provider
/// barrier and cannot poll the status page: the tick outcome's `operations`
/// count is the durable unfinished-set size, so `0` proves the completion
/// commit ran. Held operations stay in the unfinished set, so a hold can
/// never read as completion.
async fn dialogue_race_drive_deletion_to_completed(handle: &HostHandle) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(150);
    loop {
        let pass = handle
            .run_targeted_deletion_tick()
            .await
            .expect("the serving tick must run");
        if pass.operations == 0 {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the deletion operation did not complete: {pass:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// E2E 1 race (design R2, post-completion): a dialogue provider call already
/// claimed when the deletion condition commits must not publish or adopt its
/// delayed paraphrase after the operation completed.
///
/// The dialogue prompt's read-set is the durable correspondence: admission
/// associates the claim with the interval through the target-bearing History
/// and Memory identities the prompt actually consumed, and the released
/// result is refused even though the paraphrase has no literal target and no
/// current condition is readable.
#[tokio::test]
async fn stage6_dialogue_claim_before_completion_refuses_the_delayed_paraphrase() {
    use ene_companion::{
        AppendHistoryCommand, CompanionRepository as _, HistoryRepository as _, HistoryRole,
    };
    use ene_learning::{
        ChangeKind, Importance, LearningRepository as _, LearningScope, MemoryChange,
        MemoryChangeCommit, MemoryId, MemoryTarget, TemporalMeaning,
    };
    use ene_presence::PresenceRepository as _;

    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let second = String::from("what do you remember about that?");
    let fresh = format!("a fresh note about {TARGET}");
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_latest_owner(&second),
                Call::text(DIALOGUE_RACE_PARAPHRASE),
            ),
            (on_latest_owner(&fresh), Call::text("acknowledged")),
        ],
        &[],
    ));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE, cmds::CAPABILITY_LEARNING],
    )
    .await;
    // Quiesce the serving Host and seed the target-bearing canonical context
    // directly through the production repository boundaries: one Owner History
    // message and one Memory. Nothing target-bearing was ever handed to the
    // Client, so its parked connection is not a required local-erasure
    // participant and the operation can reach the sealed global completion
    // while the pre-deletion provider call is still held by the barrier.
    served.stop().await;
    {
        let store = ene_store::Store::open(&dir.join("app.db"))
            .await
            .expect("the state database opens for seeding");
        let companion = store
            .ensure_running_companion()
            .await
            .expect("the companion must resolve");
        let generation = store
            .load_attribution(companion.as_raw())
            .await
            .expect("attribution must load")
            .expect("attribution must exist")
            .generation;
        match store
            .append_message(AppendHistoryCommand {
                companion,
                round: RawId::new(),
                role: HistoryRole::Owner,
                text: format!("please remember {TARGET} for me"),
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
            })
            .await
            .expect("the History seed must commit")
        {
            ene_companion::HistoryAppendOutcome::CommittedAs { .. } => {}
            other => panic!("the History seed must commit, got {other:?}"),
        }
        assert!(
            matches!(
                store
                    .commit_memory_change(MemoryChangeCommit {
                        summary: None,
                        secret_premise: None,
                        claim: None,
                        change: MemoryChange {
                            target: MemoryTarget::New {
                                id: MemoryId::generate(),
                            },
                            scope: LearningScope::companion(companion.as_raw()),
                            content: format!("the owner mentioned {TARGET}"),
                            importance: Importance::default(),
                            temporal: TemporalMeaning::Enduring,
                            change: ChangeKind::Initial,
                            recall_suppressed: false,
                            at: WallClockWithTz::now(),
                        },
                    })
                    .await
                    .expect("the Memory seed must answer"),
                ene_learning::MemoryChangeOutcome::Committed { .. }
            ),
            "the Memory seed must commit"
        );
    }
    // Stage the request before parking: the client is the only connection.
    let mut client = served.serve().await;
    // Stage the request before parking: the client is the only connection.
    let outcome = request_deletion(&mut client, TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(outcome, ManagementOutcome::NeedsClarification);
    // The second turn's provider call parks after its claim; its prompt read
    // the target-bearing History rows and the target-bearing Memory.
    transport.block_input(on_latest_owner(&second));
    let handle = Arc::clone(&served.handle);
    let barrier = Arc::clone(&transport);
    let mut parked = Box::pin(send_round_raw(&mut client, &second));
    tokio::select! {
        result = parked.as_mut() => panic!("the parked round cannot finish before completion: {result:?}"),
        () = barrier.wait_parked(1) => {}
    }
    // Condition + erase/verify + global completion while the provider call is
    // still held by the barrier.
    confirm_deletion(&handle).await;
    dialogue_race_drive_deletion_to_completed(&handle).await;
    // The parked result is released only after completion: the durable
    // old-claim hold refuses presentation and adoption. The transport records
    // the request text when it answers, so the fixture premise (the prompt
    // read the target-bearing sources before the condition committed) is
    // asserted from the captured pre-deletion request.
    barrier.release_blocked();
    let (_round2, _stream2, raced_text, close) = parked.await.expect("the raced round must answer");
    assert!(
        transport.input_texts().iter().any(|input| {
            input.ends_with(&format!("\nOwner: {second}")) && input.contains(TARGET)
        }),
        "the fixture must show the prompt read the target-bearing sources"
    );
    assert_eq!(
        close,
        ene_api::v1::round::StreamClose::Interrupted,
        "a reply whose claim belongs to the deletion interval never completes"
    );
    assert!(
        !raced_text.contains(DIALOGUE_RACE_PARAPHRASE),
        "the delayed paraphrase must not be presented: {raced_text}"
    );
    assert!(
        !raced_text.contains(TARGET),
        "no covered delta may be presented: {raced_text}"
    );
    let history = history_texts(&mut client).await;
    assert!(
        !history
            .iter()
            .any(|text| text.contains(DIALOGUE_RACE_PARAPHRASE)),
        "the delayed paraphrase must never be adopted into History"
    );
    assert_eq!(served.canonical_remainder(TARGET).await, 0);
    assert!(
        db_target_hits(&served.dir.join("app.db"), TARGET).is_empty(),
        "no table may keep the target after completion"
    );
    // The completed operation is not a permanent ban: a fresh Owner origin
    // after completion is accepted as a new History row.
    let (round, stream, reply) = send_round(&mut client, &fresh)
        .await
        .expect("a fresh origin must be accepted");
    assert!(reply.contains("acknowledged"), "{reply}");
    confirm_round(&mut client, &round, stream).await;
    assert!(
        history_texts(&mut client)
            .await
            .iter()
            .any(|text| text.contains(TARGET)),
        "the fresh origin is appended as new History"
    );
    served.server.abort();
}

/// E2E 1 race (read-side gate): while a deletion condition is current, a
/// dialogue turn's recent-History and Memory reads must not hand the covered
/// rows to the provider. The turn still serves the uncovered remainder, so
/// the withheld context is proven by the provider input rather than by a
/// refused turn.
#[tokio::test]
async fn stage6_active_deletion_keeps_covered_context_from_the_provider() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let first = format!("please remember {TARGET} for me");
    let second = String::from("what do you remember about that?");
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (on_learning_formation(true), Call::text(formation_create())),
            (
                on_latest_owner(&first),
                Call::text(format!("I will keep {TARGET} in mind.")),
            ),
            (on_latest_owner(&second), Call::text("a clean answer")),
        ],
        &[],
    ));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE, cmds::CAPABILITY_LEARNING],
    )
    .await;
    let (round, stream, reply) = send_round(served.client(), &first)
        .await
        .expect("the first round must complete");
    assert!(reply.contains(TARGET));
    confirm_round(served.client(), &round, stream).await;
    wait_for_memory_revision_at_least(served.client(), 1).await;
    // Condition first: the confirmation commits the erasure condition and
    // its bounded fan-out, and the operation stays unfinished (the delivered
    // Client incarnation is an un-answered local-erasure participant), so the
    // condition is provably current when the next dialogue turn starts.
    let outcome = request_deletion(served.client(), TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(outcome, ManagementOutcome::NeedsClarification);
    confirm_deletion(&served.handle).await;
    let sends_before = transport.sends();
    let (round, stream, reply) = send_round(served.client(), &second)
        .await
        .expect("a turn with uncovered context must still serve");
    assert!(reply.contains("a clean answer"), "{reply}");
    confirm_round(served.client(), &round, stream).await;
    assert!(
        transport.sends() > sends_before,
        "the filtered turn reaches the provider"
    );
    let second_inputs: Vec<String> = transport
        .input_texts()
        .into_iter()
        .filter(|input| input.ends_with(&format!("\nOwner: {second}")))
        .collect();
    assert!(
        !second_inputs.is_empty(),
        "the fixture must reach the second provider call"
    );
    assert_absent_all("dialogue provider input", &second_inputs, TARGET);
    // The operation completes and the completed surface stays clean.
    let handle = Arc::clone(&served.handle);
    let page = drive_until(&handle, served.client(), DeletionPhaseWire::Completed)
        .await
        .expect("the operation must complete");
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Completed);
    assert_eq!(served.canonical_remainder(TARGET).await, 0);
    assert!(
        db_target_hits(&served.dir.join("app.db"), TARGET).is_empty(),
        "no table may keep the target after completion"
    );
    served.server.abort();
}
