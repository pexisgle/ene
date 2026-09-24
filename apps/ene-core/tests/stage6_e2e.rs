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

use ene_api::codec::{DecodedFrame, WireFrame, decode_frame, encode_frame};
use ene_api::runtime::{HOST_RUNTIME_FILE_NAME, HostRuntimeInfo};
use ene_api::v1::deletion::{
    DeletionParticipantReportWire, DeletionParticipantStatusWire, DeletionPhaseWire,
    DeletionPurposeWire, DeletionStatusPage, DeletionStatusRequest, DeletionStatusResponse,
};
use ene_api::v1::envelope::{ProtocolVersion, WireEnvelope, WireSender, new_outgoing_envelope};
use ene_api::v1::handshake::{PairingRequest, PairingResult};
use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome,
    RationaleOrigin, workspace_target,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::StreamWireId;
use ene_api::v1::refs::{
    BaseViewMark, ClientIncarnationId, CommandWireId, CompanionWireRef, ManagementTargetWire,
    RequestWireId, RoundWireId, TextLangWire, WireMessageId, WireMessageType,
};
use ene_api::v1::reject::RejectKind;
use ene_api::v1::round::{
    HistoryResponse, PresentationStatus, RoundIntakeOutcomeWire, RoundTarget, TextBodyWire,
};
use ene_api::v1::undelivered::{
    TaskListPage, TaskListResponse, UndeliveredAckOutcome, UndeliveredResponse, UndeliveredSummary,
};
use ene_api::v1::usage::{
    UsageCapConsumptionView, UsageSummaryPage, UsageSummaryRequest, UsageSummaryResponse,
};
use ene_core::conn;
use ene_core::serve::{CoreError, CredStore, HostHandle};
use ene_credential::{CredentialRef, CredentialSetRepository as _, MemoryCredentialStore};
use ene_ctl::client::{Client, ClientError, ConnectProgress, PendingPairingClient};
use ene_ctl::cmds;
use ene_inference::cost::UsageEstimate;
use ene_inference::{ProviderRequest, ProviderResponse, ProviderTransport, RawUsage};
use ene_preservation::{ConfirmTargetedDeletionOutcome, DeletionOperationRef};
use ene_primitive::{RawId, WallClockWithTz};
use ene_task::{CancelTaskCommand, TaskCancelOutcome, TaskId};
use rusqlite::OptionalExtension as _;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme,
};
use sha2::Digest as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

// Mirrors `crate::wss::{AUTH_DEADLINE, MAX_PENDING_PAIRINGS}`; integration
// tests cannot name pub(crate) items, so a drift breaks these tests instead
// of silently weakening production.
const MIRRORED_AUTH_DEADLINE: Duration = Duration::from_secs(15);
const MIRRORED_MAX_PENDING_PAIRINGS: usize = 8;

const DESCRIPTOR: &str = "stage6 e2e";
const MODEL: &str = "gpt-4o-mini";
const SHUTDOWN_TASK_OWNER: &str = "create the shutdown task before stopping the Host";
const SHUTDOWN_TASK_PURPOSE: &str = "write the shutdown report";
const TARGET: &str = "TS6-DELETION-CANARY-9137";
const SECRET: &str = "sk-stage6-secret-marker-8821";
const ROTATED_SECRET: &str = "sk-stage6-rotated-marker-4477";

#[test]
#[ignore = "subprocess fixture for Task Agent shutdown escalation"]
fn uncooperative_task_effect_worker_fixture() {
    let entered = std::env::var_os("ENE_TEST_EFFECT_ENTERED").expect("entered path");
    let release = std::env::var_os("ENE_TEST_EFFECT_RELEASE").expect("release path");
    let mutation = std::env::var_os("ENE_TEST_EFFECT_MUTATION").expect("mutation path");
    std::fs::write(entered, b"entered").unwrap();
    while !std::path::Path::new(&release).exists() {
        std::thread::yield_now();
    }
    std::fs::write(mutation, b"late workspace mutation").unwrap();
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
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

#[expect(clippy::expect_used, reason = "test fixture helper")]
fn memory_store_with_rotated(main: &str, rotated: &str) -> MemoryCredentialStore {
    let store = memory_store_with(main);
    store.insert(
        CredentialRef::new("openai", "rotated").expect("valid test fixture"),
        rotated,
    );
    store
}

#[derive(Clone)]
struct Call {
    reply: String,
    usage: Option<RawUsage>,
    lost: bool,
    stream: Option<StreamScript>,
}

/// A provider call that keeps emitting deltas at `pace` until every chunk is
/// pushed, so a business response stream stays active across whole clock
/// windows instead of parking with zero output.
#[derive(Clone)]
struct StreamScript {
    chunks: Vec<String>,
    pace: Duration,
}

impl Call {
    fn text(reply: impl Into<String>) -> Self {
        Self {
            reply: reply.into(),
            usage: None,
            lost: false,
            stream: None,
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
            stream: None,
        }
    }

    fn lost() -> Self {
        Self {
            reply: String::new(),
            usage: None,
            lost: true,
            stream: None,
        }
    }

    fn stream(reply: impl Into<String>, chunks: Vec<String>, pace: Duration) -> Self {
        Self {
            reply: reply.into(),
            usage: None,
            lost: false,
            stream: Some(StreamScript { chunks, pace }),
        }
    }
}

type Matcher = Box<dyn Fn(&str) -> bool + Send + Sync>;

fn on_latest_owner(text: &str) -> Matcher {
    let needle = format!("\nOwner: {text}");
    Box::new(move |input| input.ends_with(&needle))
}

fn on_learning_formation(create: bool) -> Matcher {
    Box::new(move |input| {
        input.contains("learning formation pass")
            && input.contains("Existing memories:\n(none)") == create
    })
}

fn on_task_agent_turn(tool_calls: usize) -> Matcher {
    Box::new(move |input| {
        input.starts_with("[RESPONSE FORMAT]") && input.matches("[TOOL CALL]").count() == tool_calls
    })
}

struct ScriptedTransport {
    scripts: Mutex<Vec<(Matcher, Call)>>,
    default_call: Mutex<Call>,
    blocks: Mutex<BTreeSet<usize>>,
    block_matches: Mutex<Vec<Matcher>>,
    parked: AtomicUsize,
    streaming: AtomicUsize,
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
            streaming: AtomicUsize::new(0),
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

    fn parked_count(&self) -> usize {
        self.parked.load(Ordering::SeqCst)
    }

    /// Provider calls currently inside a stream script. It decrements when
    /// the script ends on its own, so a lingering count means the call was
    /// neither finished nor dropped yet.
    fn streaming_count(&self) -> usize {
        self.streaming.load(Ordering::SeqCst)
    }

    fn input_texts(&self) -> Vec<String> {
        self.inputs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl ProviderTransport for ScriptedTransport {
    fn complete_streaming<'a>(
        &'a self,
        req: ProviderRequest,
        sink: &'a mut (dyn ene_inference::DeltaSink + Send),
    ) -> Pin<
        Box<
            dyn Future<Output = Result<ProviderResponse, ene_inference::InferenceTechnicalError>>
                + Send
                + 'a,
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
            if let Some(script) = call_script.stream {
                self.streaming.fetch_add(1, Ordering::SeqCst);
                for chunk in &script.chunks {
                    match sink.push_delta(chunk).await {
                        ene_inference::DeltaFlow::Continue => {}
                        ene_inference::DeltaFlow::Abort(reason) => {
                            self.streaming.fetch_sub(1, Ordering::SeqCst);
                            return Err(ene_inference::InferenceTechnicalError::StreamAborted {
                                reason: reason.to_owned(),
                            });
                        }
                    }
                    tokio::time::sleep(script.pace).await;
                }
                self.streaming.fetch_sub(1, Ordering::SeqCst);
                return Ok(ProviderResponse {
                    text: call_script.reply,
                    usage: call_script.usage,
                });
            }
            let response = ProviderResponse {
                text: call_script.reply,
                usage: call_script.usage,
            };
            match sink.push_delta(&response.text).await {
                ene_inference::DeltaFlow::Continue => Ok(response),
                ene_inference::DeltaFlow::Abort(reason) => {
                    Err(ene_inference::InferenceTechnicalError::StreamAborted {
                        reason: reason.to_owned(),
                    })
                }
            }
        })
    }

    fn usage_estimate(&self, _req: &ProviderRequest) -> Option<UsageEstimate> {
        self.estimate
    }
}

fn task_reply(directive: serde_json::Value) -> String {
    format!("[task-control] {directive}")
}

fn shared_memory_store(dir: &Path) -> MemoryCredentialStore {
    static SHARED: std::sync::LazyLock<
        Mutex<std::collections::HashMap<PathBuf, MemoryCredentialStore>>,
    > = std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));
    let mut shared = SHARED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = shared.get(dir) {
        return existing.clone();
    }
    let store = memory_store();
    shared.insert(dir.to_path_buf(), store.clone());
    store
}

fn register_memory_store(dir: &Path, store: &MemoryCredentialStore) {
    static SHARED: std::sync::LazyLock<
        Mutex<std::collections::HashMap<PathBuf, MemoryCredentialStore>>,
    > = std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));
    let mut shared = SHARED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    shared.insert(dir.to_path_buf(), store.clone());
}

async fn open_host(dir: &Path) -> Arc<HostHandle> {
    open_host_with(dir, shared_memory_store(dir)).await
}

#[expect(clippy::unwrap_used, reason = "test fixture helper")]
async fn open_host_with(dir: &Path, store: MemoryCredentialStore) -> Arc<HostHandle> {
    register_memory_store(dir, &store);
    let opened = HostHandle::open_with_cred_store(dir, CredStore::Memory(store)).await;
    assert!(opened.is_ok(), "host must open");
    let handle = Arc::new(opened.unwrap());
    handle.set_client_erasure_wait_for_tests(Duration::from_millis(200));
    handle
}

async fn dial_until_pending(dir: &Path) -> Result<PendingPairingClient, String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let attempt = tokio::time::timeout(
            Duration::from_secs(10),
            Client::begin_connect(dir, DESCRIPTOR, "test"),
        )
        .await;
        match attempt {
            Ok(Ok(ConnectProgress::Pending(pending))) => return Ok(pending),
            Ok(Err(ClientError::Transport(reason))) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(format!("the listener never accepted a client: {reason}"));
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok(Err(other)) => return Err(format!("a first pairing must pend, got {other:?}")),
            Ok(Ok(ConnectProgress::Connected(_))) => {
                return Err(String::from(
                    "a first pairing must not authenticate before Owner approval",
                ));
            }
            Err(_) => return Err(String::from("the pairing dial timed out")),
        }
    }
}

#[expect(clippy::panic, reason = "test fixture helper")]
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
            Ok(Err(ClientError::Transport(_))) => {}
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

const ROUND_TRIP_GUARD: Duration = Duration::from_secs(60);

async fn ask(client: &mut Client, payload: WirePayload, what: &str) -> Result<WirePayload, String> {
    match tokio::time::timeout(ROUND_TRIP_GUARD, client.request(payload)).await {
        Ok(Ok(answer)) => Ok(answer),
        Ok(Err(error)) => Err(format!("{what} errored: {error:?}")),
        Err(_) => Err(format!("{what} timed out")),
    }
}

async fn ask_stream(client: &mut Client) -> Result<WirePayload, String> {
    match tokio::time::timeout(ROUND_TRIP_GUARD, client.next_frame()).await {
        Ok(Ok(payload)) => Ok(payload),
        Ok(Err(error)) => Err(format!("stream frame errored: {error:?}")),
        Err(_) => Err(String::from("stream frame timed out")),
    }
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
        confirmed: false,
    })
}

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

#[expect(clippy::panic, reason = "test fixture helper")]
async fn wait_until(mut ready: impl FnMut() -> bool, mut what: impl FnMut() -> String) {
    for _ in 0..1_000_000 {
        if ready() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("{}", what());
}

async fn wait_until_deletion_drivers(handle: &HostHandle, expected: usize) {
    wait_until(
        || handle.live_targeted_deletion_drivers_for_tests() == expected,
        || {
            format!(
                "targeted deletion drivers stayed at {}, expected {expected}",
                handle.live_targeted_deletion_drivers_for_tests()
            )
        },
    )
    .await;
}

async fn wait_until_deletion_blocking(handle: &HostHandle, expected: usize) {
    wait_until(
        || {
            handle
                .store_for_tests()
                .live_deletion_blocking_sections_for_tests()
                == expected
        },
        || {
            format!(
                "deletion blocking sections stayed at {}, expected {expected}",
                handle
                    .store_for_tests()
                    .live_deletion_blocking_sections_for_tests()
            )
        },
    )
    .await;
}

struct Served {
    dir: PathBuf,
    handle: Option<Arc<HostHandle>>,
    server: tokio::task::JoinHandle<Result<(), CoreError>>,
    shutdown: Option<tokio::sync::watch::Sender<bool>>,
    client: Option<Client>,
    transport: Arc<ScriptedTransport>,
    cred_store: Box<dyn Fn() -> MemoryCredentialStore + Send + Sync>,
}

impl Served {
    #[expect(clippy::expect_used, reason = "test fixture helper")]
    async fn start(
        dir: PathBuf,
        cred_store: impl Fn() -> MemoryCredentialStore + Send + Sync + 'static,
        transport: Arc<ScriptedTransport>,
        capabilities: &[&str],
    ) -> Self {
        let seed = cred_store();
        register_memory_store(&dir, &seed);
        let handle = open_host_with(&dir, seed.clone()).await;
        let (stop, shutdown) = tokio::sync::watch::channel(false);
        let server = tokio::spawn(conn::run_until_shutdown(
            dir.clone(),
            Arc::clone(&handle),
            Arc::clone(&transport),
            shutdown,
        ));
        let pending = dial_until_pending(&dir)
            .await
            .expect("listener must accept the first pairing");
        let approved = handle
            .approve_device(pending.pending_id())
            .await
            .expect("approval must succeed");
        assert!(approved.is_some(), "approval must pair");
        let mut client = pending
            .complete()
            .await
            .expect("provision must authenticate");
        setup_flow(&mut client, &handle, capabilities)
            .await
            .expect("setup must complete");
        wait_until_deletion_drivers(&handle, 1).await;
        Self {
            dir,
            handle: Some(handle),
            server,
            shutdown: Some(stop),
            client: Some(client),
            transport,
            cred_store: Box::new(move || seed.clone()),
        }
    }

    #[expect(clippy::expect_used, reason = "test fixture helper")]
    fn client(&mut self) -> &mut Client {
        self.client.as_mut().expect("a live client")
    }

    #[expect(clippy::expect_used, reason = "test fixture helper")]
    fn handle(&self) -> &HostHandle {
        self.handle.as_ref().expect("a live HostHandle")
    }

    #[expect(clippy::expect_used, reason = "test fixture helper")]
    fn handle_arc(&self) -> Arc<HostHandle> {
        Arc::clone(self.handle.as_ref().expect("a live HostHandle"))
    }

    async fn stop(&mut self) {
        self.client = None;
        tokio::task::yield_now().await;
        self.request_graceful_stop();
        self.join_listener().await;
        wait_until_deletion_drivers(self.handle(), 0).await;
        wait_until_deletion_blocking(self.handle(), 0).await;
    }

    fn request_graceful_stop(&self) {
        if let Some(stop) = &self.shutdown {
            stop.send_modify(|stop_requested| *stop_requested = true);
        }
    }

    #[expect(clippy::panic, reason = "test fixture helper")]
    async fn join_listener(&mut self) {
        let finished = std::mem::replace(
            &mut self.server,
            tokio::spawn(async { Ok::<(), CoreError>(()) }),
        );
        match finished.await {
            Ok(Ok(())) => {}
            Err(error) if error.is_cancelled() => {}
            Err(error) => panic!("the listener panicked: {error}"),
            Ok(Err(error)) => panic!("the listener failed: {error}"),
        }
    }

    /// Awaits the listener for at most `limit` and reports whether it
    /// finished. On timeout the handle stays owned, so the same shutdown can
    /// be joined again later.
    #[expect(clippy::panic, reason = "test fixture helper")]
    async fn join_listener_within(&mut self, limit: Duration) -> bool {
        let finished = tokio::time::timeout(limit, &mut self.server).await;
        let Ok(finished) = finished else {
            return false;
        };
        self.server = tokio::spawn(async { Ok::<(), CoreError>(()) });
        match finished {
            Ok(Ok(())) => {}
            Err(error) if error.is_cancelled() => {}
            Err(error) => panic!("the listener panicked: {error}"),
            Ok(Err(error)) => panic!("the listener failed: {error}"),
        }
        true
    }

    async fn abort_listener(&mut self) {
        self.client = None;
        tokio::task::yield_now().await;
        self.server.abort();
        self.join_listener().await;
        wait_until_deletion_drivers(self.handle(), 0).await;
    }

    #[expect(clippy::expect_used, reason = "test fixture helper")]
    async fn serve(&mut self) -> Client {
        wait_until_deletion_drivers(self.handle(), 0).await;
        wait_until_deletion_blocking(self.handle(), 0).await;
        drop(self.handle.take());
        let handle = open_host_with(&self.dir, (self.cred_store)()).await;
        handle
            .run_startup_mutations()
            .await
            .expect("restart startup must complete");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let (stop, shutdown) = tokio::sync::watch::channel(false);
            let mut server = tokio::spawn(conn::run_until_shutdown(
                self.dir.clone(),
                Arc::clone(&handle),
                Arc::clone(&self.transport),
                shutdown,
            ));
            if let Ok(outcome) = tokio::time::timeout(Duration::from_millis(250), &mut server).await
            {
                let failure = outcome.expect("the listener task must not panic");
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the listener never bound after the restart: {failure:?}"
                );
                tokio::time::sleep(Duration::from_millis(1)).await;
                continue;
            }
            wait_until_deletion_drivers(&handle, 1).await;
            self.handle = Some(handle);
            self.server = server;
            self.shutdown = Some(stop);
            return connect(&self.dir).await;
        }
    }

    async fn restart(&mut self) -> Client {
        self.stop().await;
        self.serve().await
    }

    #[expect(clippy::expect_used, reason = "test fixture helper")]
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

async fn serve_and_setup(
    dir: PathBuf,
    transport: Arc<ScriptedTransport>,
    capabilities: &[&str],
) -> Served {
    Served::start(dir, memory_store, transport, capabilities).await
}

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
    let target = client.round_target();
    let send = ask(
        client,
        WirePayload::SubmitTextInput(cmds::submit_input(
            &companion,
            target,
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

async fn submit_expect_hold(client: &mut Client, text: &str) -> Result<(), String> {
    let companion = client.companion_ref();
    let target = client.round_target();
    let answer = ask(
        client,
        WirePayload::SubmitTextInput(cmds::submit_input(
            &companion,
            target,
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
            confirmed: false,
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
            Some(summary.presence_generation),
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

#[expect(clippy::expect_used, clippy::panic, reason = "test fixture helper")]
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
        let mut drive = std::pin::pin!(handle.run_targeted_deletion_tick());
        loop {
            tokio::select! {
                driven = &mut drive => {
                    driven.map_err(|error| format!("the fan-out pass must run: {error:?}"))?;
                    break;
                }
                () = tokio::time::sleep(Duration::from_millis(5)) => {
                    if tokio::time::Instant::now() >= deadline {
                        break;
                    }
                    match deletion_page(client).await {
                        Ok(page)
                            if page.operations.first().map(|view| view.phase) == Some(wanted) =>
                        {
                            return Ok(page);
                        }
                        Ok(_) | Err(_) => {}
                    }
                }
            }
        }
    }
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
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

async fn seed_owner_message(
    store: &ene_store::Store,
    companion: ene_companion::CompanionId,
    generation: ene_presence::PresenceGeneration,
    text: &str,
) -> RawId {
    use ene_companion::HistoryRepository as _;
    match store
        .append_message(ene_companion::AppendHistoryCommand {
            companion,
            round: RawId::new(),
            role: ene_companion::HistoryRole::Owner,
            text: text.to_owned(),
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
        .expect("the History append must commit")
    {
        ene_companion::HistoryAppendOutcome::CommittedAs { message } => message,
        other => panic!("the History append must commit, got {other:?}"),
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

#[expect(clippy::expect_used, reason = "test fixture helper")]
async fn plant_target_fixture(served: &mut Served) {
    let dir = served.dir.clone();
    let client = served.client();
    let (round, stream, reply) = send_round(client, &format!("please remember {TARGET} for me"))
        .await
        .expect("round one must complete");
    assert!(
        reply.contains(TARGET),
        "the reply carries the target: {reply}"
    );
    confirm_round(client, &round, stream).await;
    wait_for_memory_revision_at_least(client, 1).await;
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
    let workspace = dir.join(format!("workspace-{TARGET}"));
    std::fs::create_dir_all(&workspace).expect("the workspace directory creates");
    std::fs::write(workspace.join("input.txt"), b"notes").expect("input fixture");
    select_workspace(client, &workspace)
        .await
        .expect("workspace must select");
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
    let summary = wait_for_summary_with(client, TARGET).await;
    assert!(
        summary
            .items
            .iter()
            .any(|item| item.excerpt.contains(TARGET)),
        "the carried receipt must quote the target: {summary:?}"
    );
}

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

#[expect(clippy::expect_used, clippy::panic, reason = "test fixture helper")]
async fn history_texts(client: &mut Client) -> Vec<String> {
    let companion = client.companion_ref();
    let history = ask(
        client,
        WirePayload::HistoryRequest(cmds::history_request(&companion, None, 100)),
        "history",
    )
    .await
    .expect("history must answer");
    let WirePayload::HistoryResponse(HistoryResponse::Items(items)) = history else {
        panic!("history must answer items: {history:?}");
    };
    items.into_iter().map(|item| item.text).collect()
}

#[expect(clippy::panic, reason = "test fixture helper")]
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

#[expect(clippy::expect_used, reason = "test fixture helper")]
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
        if summary.receipt.0.is_empty() {
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }
        if ack_summary(client, &summary).await.is_err() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn assert_target_is_planted(served: &Served) {
    let hits = db_target_hits(&served.dir.join("app.db"), TARGET);
    assert!(
        !hits.is_empty(),
        "the fixture must plant the target before the deletion"
    );
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
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
    let current = confirm_deletion(served.handle()).await;
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

    let handle = served.handle_arc();
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
    assert_eq!(
        served.canonical_remainder(TARGET).await,
        0,
        "the canonical system-wide remainder must be zero"
    );
    assert!(
        db_target_hits(&served.dir.join("app.db"), TARGET).is_empty(),
        "no table may keep the target after completion"
    );

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
        WirePayload::HistoryRequest(cmds::history_request(&companion, None, 100)),
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

const FINALIZING_TARGET: &str = "TS6-FINALIZING-CANARY-2201";

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
    let backlog = wait_for_summary_with(served.client(), TARGET).await;
    drop(ack_summary(served.client(), &backlog).await);
    let mut client = served.restart().await;
    let outcome = request_deletion(&mut client, TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(outcome, ManagementOutcome::NeedsClarification);
    transport.block_input(on_latest_owner(&second));
    let handle = served.handle_arc();
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
            handle
                .run_targeted_deletion_tick()
                .await
                .expect("the serving tick must run");
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
    let (round, stream, fresh) = send_round(&mut client, &format!("a fresh note: {TARGET}"))
        .await
        .expect("a fresh origin must be accepted");
    assert!(fresh.contains("acknowledged"), "{fresh}");
    confirm_round(&mut client, &round, stream).await;
    served.server.abort();
}

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
    confirm_deletion(served.handle()).await;
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
    let fresh = fetch_summary(served.client(), "covered subscription")
        .await
        .expect("the subscription must answer");
    for item in &fresh.items {
        assert_absent("covered excerpt", &item.excerpt, TARGET);
    }
    let handle = served.handle_arc();
    let page = drive_until(&handle, served.client(), DeletionPhaseWire::Completed)
        .await
        .expect("the operation must complete with a reachable Client");
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Completed);
    assert_eq!(served.canonical_remainder(TARGET).await, 0);
    assert!(db_target_hits(&served.dir.join("app.db"), TARGET).is_empty());
    served.server.abort();
}

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
    confirm_deletion(served.handle()).await;
    transport.release_blocked();
    wait_for_usage_consumer(served.client(), "companion_learning").await;
    let handle = served.handle_arc();
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
    assert!(
        !memory_view(served.client()).await.contains(TARGET),
        "a covered formation never writes target Memory"
    );
    served.server.abort();
}

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
    let handle = served.handle_arc();
    confirm_deletion(&handle).await;
    let page = drive_until(&handle, served.client(), DeletionPhaseWire::Completed)
        .await
        .expect("the operation must complete while the formation is parked");
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Completed);
    transport.release_blocked();
    wait_for_usage_consumer(served.client(), "companion_learning").await;
    assert_eq!(served.canonical_remainder(TARGET).await, 0);
    assert!(db_target_hits(&served.dir.join("app.db"), TARGET).is_empty());
    assert!(
        !memory_view(served.client()).await.contains(TARGET),
        "a formation claimed before the interval never writes target Memory after completion"
    );
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
    let handle = served.handle_arc();
    confirm_deletion(&handle).await;
    let page = drive_until(&handle, served.client(), DeletionPhaseWire::Completed)
        .await
        .expect("the operation must complete while the final turn is parked");
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Completed);
    transport.release_blocked();
    wait_task_progress(served.client(), "completed", 1)
        .await
        .expect("the task completes on the collected result");
    assert_eq!(served.canonical_remainder(TARGET).await, 0);
    assert!(
        db_target_hits(&served.dir.join("app.db"), TARGET).is_empty(),
        "the delayed result body is never stored"
    );
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
        confirm_deletion(served.handle()).await
    };
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
    let sends_before = transport.sends();
    submit_expect_hold(&mut client, &format!("still {TARGET}"))
        .await
        .expect("the condition survives the restart");
    assert_eq!(transport.sends(), sends_before);
    let page = drive_until(served.handle(), &mut client, DeletionPhaseWire::Completed)
        .await
        .expect("the restarted Host must complete the operation");
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Completed);
    assert_eq!(served.canonical_remainder(TARGET).await, 0);
    assert!(db_target_hits(&served.dir.join("app.db"), TARGET).is_empty());
    served.server.abort();
}

#[tokio::test]
async fn listener_abort_stops_the_deletion_driver() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let transport = Arc::new(ScriptedTransport::new(vec![], &[]));
    let mut served = serve_and_setup(dir, transport, &[cmds::CAPABILITY_DIALOGUE]).await;
    wait_until_deletion_drivers(served.handle(), 1).await;
    assert_eq!(
        served.handle().live_targeted_deletion_drivers_for_tests(),
        1,
        "serving starts exactly one Targeted Deletion driver"
    );
    served.abort_listener().await;
    assert_eq!(
        served.handle().live_targeted_deletion_drivers_for_tests(),
        0,
        "aborting and awaiting conn::run must leave no live deletion driver"
    );
}

#[tokio::test]
async fn restart_has_exactly_one_deletion_driver() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let transport = Arc::new(ScriptedTransport::new(vec![], &[]));
    let mut served = serve_and_setup(dir, transport, &[cmds::CAPABILITY_DIALOGUE]).await;
    wait_until_deletion_drivers(served.handle(), 1).await;
    let predecessor = Arc::downgrade(&served.handle_arc());
    served.stop().await;
    assert_eq!(
        served.handle().live_targeted_deletion_drivers_for_tests(),
        0,
        "graceful shutdown must join the predecessor async driver"
    );
    assert_eq!(
        served
            .handle()
            .store_for_tests()
            .live_deletion_blocking_sections_for_tests(),
        0,
        "graceful shutdown must join started deletion Store work"
    );
    let _client = served.serve().await;
    wait_until_deletion_drivers(served.handle(), 1).await;
    assert_eq!(
        served.handle().live_targeted_deletion_drivers_for_tests(),
        1,
        "restart must not leave predecessor and successor drivers both alive"
    );
    assert!(
        predecessor.upgrade().is_none(),
        "the predecessor HostHandle must drop once its serving driver is gone"
    );
}

#[tokio::test]
async fn shutdown_waits_for_started_deletion_store_work() {
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
    let mut served = serve_and_setup(dir.clone(), transport, &[cmds::CAPABILITY_DIALOGUE]).await;
    let (_round, _stream, _reply) = send_round(served.client(), &first)
        .await
        .expect("the first round must complete");
    let outcome = request_deletion(served.client(), TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(outcome, ManagementOutcome::NeedsClarification);
    confirm_deletion(served.handle()).await;

    served
        .handle()
        .store_for_tests()
        .arm_deletion_blocking_park_for_tests();
    served.handle().wake_deletion_driver_for_tests();
    served
        .handle()
        .store_for_tests()
        .wait_deletion_blocking_park_for_tests()
        .await;
    assert!(
        served
            .handle()
            .store_for_tests()
            .live_deletion_blocking_sections_for_tests()
            >= 1,
        "the driver tick must have entered Store spawn_blocking work"
    );

    served.client = None;
    tokio::task::yield_now().await;
    served.request_graceful_stop();
    assert!(
        served.handle.is_some(),
        "successor Host must not replace the predecessor while Store work is parked"
    );
    let joining = std::mem::replace(
        &mut served.server,
        tokio::spawn(async { Ok::<(), CoreError>(()) }),
    );
    let mut joining = std::pin::pin!(joining);
    for _ in 0..100_000 {
        tokio::select! {
            biased;
            outcome = joining.as_mut() => {
                panic!(
                    "graceful shutdown joined while deletion Store work was parked: {outcome:?}"
                );
            }
            () = tokio::task::yield_now() => {}
        }
    }
    assert!(
        served.handle.is_some(),
        "successor Host must not start while predecessor Store work is parked"
    );
    assert_eq!(
        served.handle().live_targeted_deletion_drivers_for_tests(),
        1,
        "the async driver stays alive until the parked tick finishes"
    );
    assert!(
        served
            .handle()
            .store_for_tests()
            .live_deletion_blocking_sections_for_tests()
            >= 1
    );

    served
        .handle()
        .store_for_tests()
        .release_deletion_blocking_park_for_tests();
    match joining.await {
        Ok(Ok(())) | Err(_) => {}
        Ok(Err(error)) => panic!("the listener failed: {error}"),
    }
    wait_until_deletion_drivers(served.handle(), 0).await;
    wait_until_deletion_blocking(served.handle(), 0).await;

    drop(served.handle.take());
    let successor = open_host(&dir).await;
    successor
        .run_startup_mutations()
        .await
        .expect("successor startup must complete once predecessor Store work is gone");
}

#[tokio::test]
async fn graceful_shutdown_leaves_no_detached_deletion_work() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let transport = Arc::new(ScriptedTransport::new(vec![], &[]));
    let mut served = serve_and_setup(dir, transport, &[cmds::CAPABILITY_DIALOGUE]).await;
    wait_until_deletion_drivers(served.handle(), 1).await;
    served.stop().await;
    assert_eq!(
        served.handle().live_targeted_deletion_drivers_for_tests(),
        0
    );
    assert_eq!(
        served
            .handle()
            .store_for_tests()
            .live_deletion_blocking_sections_for_tests(),
        0
    );
}

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
    let before = transport.sends();
    let page = drive_until(served.handle(), &mut client, DeletionPhaseWire::Completed)
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

#[expect(clippy::expect_used, reason = "test fixture helper")]
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

#[expect(clippy::expect_used, clippy::panic, reason = "test fixture helper")]
async fn confirm_deletion_via_serving_control(served: &mut Served) -> DeletionOperationRef {
    let request = render_single_pending_request(served.handle()).await;
    let dir = served.dir.clone();
    let gui_handle = served.handle_arc();
    let mut gui = ene_core::host_control::seat_test_gui_for_tests(&gui_handle)
        .expect("the private confirmation channel must open");
    let (confirmed_tx, confirmed_rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let mut reader = gui.try_clone().expect("clone");
        if let Ok(Some(ene_local_control::FromConfirmation::ConfirmationChallenge {
            session_id,
            nonce,
            ..
        })) = reader.recv()
        {
            match gui
                .send(&ene_local_control::ToConfirmation::SessionComplete { session_id, nonce })
            {
                Ok(()) | Err(_) => {}
            }
        }
        match confirmed_tx.send(()) {
            Ok(()) | Err(_) => {}
        }
        std::thread::sleep(std::time::Duration::from_secs(60));
    });
    let outcome = match served.client.as_mut() {
        Some(client) => {
            let mut confirmation = Box::pin(ene_core::host_control::confirm_targeted_deletion(
                &dir, &request,
            ));
            loop {
                tokio::select! {
                    outcome = &mut confirmation => break outcome,
                    () = tokio::time::sleep(Duration::from_millis(20)) => {
                        drop(deletion_page(client).await);
                    }
                }
            }
        }
        None => {
            let confirmation = ene_core::host_control::confirm_targeted_deletion(&dir, &request);
            tokio::pin!(confirmation);
            loop {
                tokio::select! {
                    outcome = &mut confirmation => break outcome,
                    () = tokio::time::sleep(std::time::Duration::from_millis(20)) => {}
                }
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(30), confirmed_rx)
        .await
        .expect("the confirmation surface must answer")
        .expect("the surface task must not drop its signal");
    match outcome.expect("the serving Host control inlet must answer") {
        ConfirmTargetedDeletionOutcome::Started(current) => current,
        other => panic!("the Owner confirmation must start the operation, got {other:?}"),
    }
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
async fn local_deletion_page(handle: &HostHandle) -> DeletionStatusPage {
    handle
        .deletion_status_page(None, 20)
        .await
        .expect("the local status must answer")
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
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
        tokio::task::yield_now().await;
    }
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
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
    deliver_target_copy(&mut served).await;

    let outcome = request_deletion(served.client(), TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(
        outcome,
        ManagementOutcome::NeedsClarification,
        "the Client intent only stages"
    );

    let current = confirm_deletion_via_serving_control(&mut served).await;

    let page = local_deletion_page(served.handle()).await;
    let participant = client_incarnation_participant(&page)
        .expect("the durable snapshot must name the delivered Client incarnation");
    assert!(
        participant.sweep >= current.sweep.as_u64(),
        "the Client participant belongs to the current sweep: {participant:?}"
    );

    let handle = served.handle_arc();
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

    served.client = None;
    let _current = confirm_deletion_via_serving_control(&mut served).await;

    let page = drive_until_local(served.handle(), DeletionPhaseWire::Held).await;
    assert_ne!(
        page.operations[0].phase,
        DeletionPhaseWire::Completed,
        "an unreachable Client must never be presumed erased"
    );
    let participant =
        client_incarnation_participant(&page).expect("the snapshotted Client stays required");
    assert_eq!(participant.progress, "held:unavailable");

    let replacement = connect(&served.dir).await;
    let page = local_deletion_page(served.handle()).await;
    assert_ne!(
        page.operations[0].phase,
        DeletionPhaseWire::Completed,
        "a replacement connection must not complete the operation"
    );
    let progress = client_incarnation_participant(&page)
        .expect("the Client participant stays")
        .progress
        .clone();
    assert_ne!(
        progress, "verified",
        "a replacement connection never verifies the participant by itself"
    );
    assert!(
        progress == "held:unavailable" || progress == "running",
        "the unpumped replacement stays held or in a retry demand, got {progress}"
    );
    drop(replacement);

    let mut client = served.restart().await;
    let page = local_deletion_page(served.handle()).await;
    assert_ne!(page.operations[0].phase, DeletionPhaseWire::Completed);
    let participant = client_incarnation_participant(&page)
        .expect("the Client participant must survive the restart");
    assert_ne!(participant.progress, "verified");
    assert!(
        participant.progress == "held:unavailable" || participant.progress == "running",
        "restart must not verify the unpumped Client, got {}",
        participant.progress
    );

    let handle = served.handle_arc();
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

#[tokio::test]
async fn stage6_management_view_memory_body_is_a_required_client_participant() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
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

    let mut client = served.restart().await;
    let body = memory_view(&mut client).await;
    assert!(
        body.contains(TARGET),
        "the management view hands the target-bearing Memory over: {body}"
    );

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

    let page = local_deletion_page(served.handle()).await;
    let participant = client_incarnation_participant(&page)
        .expect("the view-delivered Client incarnation is a required participant");
    assert!(
        participant.sweep >= current.sweep.as_u64(),
        "the Client participant belongs to the current sweep: {participant:?}"
    );

    let body = memory_view(served.client()).await;
    assert_absent("covered management view", &body, TARGET);

    let handle = served.handle_arc();
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

fn reported_cost_micros(input: u64, cached: u64, output: u64) -> u64 {
    (input - cached) * 150_000 / 1_000_000
        + cached * 75_000 / 1_000_000
        + output * 600_000 / 1_000_000
}

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

#[expect(clippy::expect_used, clippy::panic, reason = "test fixture helper")]
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

#[expect(clippy::expect_used, clippy::panic, reason = "test fixture helper")]
async fn set_provider_monthly_cap(
    client: &mut Client,
    intent_id: CommandWireId,
    provider: &str,
    limit_micros: u64,
) -> ManagementOutcome {
    let page = usage_page_for(client, provider).await;
    let mark = cmds::usage_cap_mark_for(&page, Some(provider), "monthly_utc")
        .expect("the page names the provider monthly slot")
        .to_string();
    let answer = ask(
        client,
        WirePayload::ManagementIntent(cmds::usage_cap_intent(
            intent_id,
            &mark,
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

#[expect(clippy::expect_used, clippy::panic, reason = "test fixture helper")]
async fn set_system_daily_cap(
    client: &mut Client,
    intent_id: CommandWireId,
    limit_micros: u64,
) -> ManagementOutcome {
    let page = usage_page(client).await;
    let mark = cmds::usage_cap_mark_for(&page, None, "daily_utc")
        .expect("the page names the system daily slot")
        .to_string();
    let answer = ask(
        client,
        WirePayload::ManagementIntent(cmds::usage_cap_intent(
            intent_id,
            &mark,
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

    wait_for_memory_revision_at_least(served.client(), 3).await;

    let page = usage_page(served.client()).await;
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
    let unknown = page
        .rows
        .iter()
        .find(|row| row.status == "unknown")
        .expect("an Unknown row");
    assert_eq!(unknown.consumer, "companion_dialogue");
    assert!(unknown.tokens.is_none(), "Unknown is never zero tokens");
    assert!(unknown.cost.is_none(), "Unknown is never a zero cost");
    let again = usage_page(served.client()).await;
    assert_eq!(again.rows, page.rows, "the read changes nothing durable");

    let synthetic = ene_inference::pricing::PricingSnapshot {
        provider: String::from("openai"),
        model: String::from(MODEL),
        currency: ene_primitive::CurrencyCode::Usd,
        input_rate: ene_inference::cost::TokenRate::from_micros_per_million(300_000),
        cached_input_rate: ene_inference::cost::TokenRate::from_micros_per_million(150_000),
        output_rate: ene_inference::cost::TokenRate::from_micros_per_million(1_200_000),
        effective_at: WallClockWithTz::now(),
        source_revision: ene_inference::pricing::PricingCatalogRevision::new(
            ene_inference::pricing::FIRST_PARTY_REVISION.as_u64() + 1,
        ),
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
            .filter(|cap| cap.provider.is_none())
            .count(),
        2,
        "the system daily and monthly slots are always reported"
    );
    served.server.abort();
}

fn cap_estimate() -> UsageEstimate {
    UsageEstimate {
        input_tokens_upper_bound: 1_000_000,
        output_tokens_upper_bound: 100_000,
    }
}

const CAP_UPPER_BOUND_MICROS: u64 = 210_001;

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
    transport.block_input(on_learning_formation(true));
    let (round, stream, _) = send_round(served.client(), &first)
        .await
        .expect("the first round must complete");
    confirm_round(served.client(), &round, stream).await;
    transport.wait_parked(1).await;
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
    let observed = usage_page_for(served.client(), "openai").await;
    let provider_slot = observed
        .caps
        .iter()
        .find(|cap| cap.provider.as_deref() == Some("openai") && cap.window == "monthly_utc")
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
        .find(|cap| cap.provider.is_none() && cap.window == "daily_utc")
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
        .find(|cap| cap.provider.is_none() && cap.window == "daily_utc")
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
    let (round, stream, reply) = send_round(served.client(), &third)
        .await
        .expect("the next send must be admitted");
    assert!(reply.contains("Still noted"), "{reply}");
    confirm_round(served.client(), &round, stream).await;
    served.server.abort();
}

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

    let observed = usage_page(served.client()).await;
    let observed_mark = cmds::usage_cap_mark_for(&observed, None, "daily_utc")
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
        .find(|cap| cap.provider.is_none() && cap.window == "daily_utc")
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

#[expect(clippy::expect_used, clippy::panic, reason = "test fixture helper")]
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
        || memory_store_with(SECRET),
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
    let (round, stream, _reply) = send_round(
        served.client(),
        &format!("please remember the passphrase {SECRET}"),
    )
    .await
    .expect("the secret round must complete");
    confirm_round(served.client(), &round, stream).await;
    let (round, stream, _) =
        send_round(served.client(), "please read input.txt and write report.md")
            .await
            .expect("the propose round must complete");
    confirm_round(served.client(), &round, stream).await;
    wait_task_progress(served.client(), "completed", 1)
        .await
        .expect("the task must complete");
    let errored = send_round_raw(served.client(), &format!("an error path with {SECRET}")).await;
    match errored {
        Ok((_, _, text, _)) => assert_absent("errored round text", &text, SECRET),
        Err(rendered) => assert_absent("errored round rendering", &rendered, SECRET),
    }
    assert_absent_all("provider request", &transport.input_texts(), SECRET);
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
        || memory_store_with_rotated(SECRET, ROTATED_SECRET),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE],
    )
    .await;
    let (round, stream, _) = send_round(served.client(), &rotated_round)
        .await
        .expect("the pre-registration round must complete");
    confirm_round(served.client(), &round, stream).await;
    assert!(
        !db_target_hits(&dir.join("app.db"), ROTATED_SECRET).is_empty(),
        "the fixture must plant the not-yet-registered value"
    );
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
            confirmed: false,
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
    let handle = served.handle_arc();
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
    deliver_target_copy(&mut served).await;

    let mut client = served.restart().await;

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

    let current = confirm_deletion_via_serving_control(&mut served).await;
    let page = local_deletion_page(served.handle()).await;
    let participant = client_incarnation_participant(&page)
        .expect("the pre-restart delivery must stay a required participant");
    assert!(
        participant.sweep >= current.sweep.as_u64(),
        "the Client participant belongs to the current sweep: {participant:?}"
    );

    let page = drive_until_local(served.handle(), DeletionPhaseWire::Held).await;
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
            .handle()
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

    let mut client = connect(&served.dir).await;
    let handle = served.handle_arc();
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

const DIALOGUE_RACE_PARAPHRASE: &str = "I still keep that detail in mind.";

#[expect(clippy::expect_used, reason = "test fixture helper")]
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
    let mut client = served.serve().await;
    let outcome = request_deletion(&mut client, TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(outcome, ManagementOutcome::NeedsClarification);
    transport.block_input(on_latest_owner(&second));
    let handle = served.handle_arc();
    let barrier = Arc::clone(&transport);
    let mut parked = Box::pin(send_round_raw(&mut client, &second));
    tokio::select! {
        result = parked.as_mut() => panic!("the parked round cannot finish before completion: {result:?}"),
        () = barrier.wait_parked(1) => {}
    }
    confirm_deletion(&handle).await;
    dialogue_race_drive_deletion_to_completed(&handle).await;
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
    let outcome = request_deletion(served.client(), TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(outcome, ManagementOutcome::NeedsClarification);
    confirm_deletion(served.handle()).await;
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
    let handle = served.handle_arc();
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

#[tokio::test]
async fn stage6_reconciliation_holds_sources_beyond_the_admission_page() {
    use ene_companion::CompanionRepository as _;
    use ene_inference::InferenceAttemptRepository as _;
    use ene_learning::{
        ChangeKind, ExperienceSourceKind, Importance, LearningClaimRef, LearningRepository as _,
        LearningScope, MemoryChange, MemoryChangeCommit, MemoryId, MemoryTarget, SourceRangeRef,
        SummaryId, SummaryRecord, TemporalMeaning,
    };
    use ene_permission::{CapabilityKind, ConsentRepository as _};
    use ene_presence::PresenceRepository as _;

    fn delayed_formation(
        companion: RawId,
        claim: ene_learning::LearningClaimRef,
    ) -> MemoryChangeCommit {
        let bound = RawId::new();
        MemoryChangeCommit {
            summary: Some(SummaryRecord {
                id: SummaryId::generate(),
                scope: LearningScope::companion(companion),
                content: String::from("a clean paraphrase"),
                source: SourceRangeRef {
                    kind: ExperienceSourceKind::Dialogue,
                    start: bound,
                    end: bound,
                },
                formed_at: WallClockWithTz::now(),
            }),
            secret_premise: None,
            claim: Some(claim),
            change: MemoryChange {
                target: MemoryTarget::New {
                    id: MemoryId::generate(),
                },
                scope: LearningScope::companion(companion),
                content: String::from("a clean recall"),
                importance: Importance::default(),
                temporal: TemporalMeaning::Enduring,
                change: ChangeKind::Initial,
                at: WallClockWithTz::now(),
            },
        }
    }

    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE, cmds::CAPABILITY_LEARNING],
    )
    .await;
    served.stop().await;
    let (ticket, companion_raw) = {
        let store = ene_store::Store::open(&dir.join("app.db"))
            .await
            .expect("the state database opens for seeding");
        let consent = store
            .load_current(CapabilityKind::Learning)
            .await
            .expect("the Learning consent must read")
            .expect("the setup flow assigns Learning");
        let credential_set = store
            .current_set_revision()
            .await
            .expect("the credential-set revision must read");
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
        let mut sources = Vec::new();
        for index in 0..(ene_preservation::DELETION_RECONCILIATION_PAGE_SIZE + 6) {
            sources.push(
                seed_owner_message(
                    &store,
                    companion,
                    generation,
                    &format!("note {index} carries {TARGET}"),
                )
                .await,
            );
        }
        let last = *sources
            .iter()
            .max_by_key(|source| source.as_uuid())
            .expect("the fixture has sources");
        let ticket = ene_inference::InferenceTicketId(RawId::new());
        let claimed = store
            .begin_inference_attempt(ene_inference::InferenceAttempt {
                ticket,
                consumer: ene_permission::ConsumerKind::CompanionLearning,
                capability: CapabilityKind::Learning,
                purpose: ene_permission::PurposeKind::MemoryFormation,
                expected_consent: (consent.id.clone(), consent.rev),
                expected_credential_set: credential_set,
                provider: consent.provider.clone(),
                model: consent.model.clone(),
                task_agent: None,
                data_use: vec![last],
                pricing: None,
                usage_estimate: None,
            })
            .await
            .expect("the formation claim must answer");
        assert_eq!(
            claimed,
            ene_inference::AttemptBeginOutcome::Started,
            "the pre-condition claim must start"
        );
        (ticket, companion.as_raw())
    };

    let mut client = served.serve().await;
    let outcome = request_deletion(&mut client, TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(outcome, ManagementOutcome::NeedsClarification);
    let handle = served.handle_arc();
    confirm_deletion(&handle).await;
    let page = drive_until(&handle, &mut client, DeletionPhaseWire::Completed)
        .await
        .expect("the operation must reconcile every page and complete");
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Completed);
    assert_eq!(served.canonical_remainder(TARGET).await, 0);
    assert!(
        db_target_hits(&served.dir.join("app.db"), TARGET).is_empty(),
        "no durable table keeps the target: {:?}",
        db_target_hits(&served.dir.join("app.db"), TARGET)
    );

    served.stop().await;
    let store = ene_store::Store::open(&dir.join("app.db"))
        .await
        .expect("the state database reopens for the delayed arrival");
    let ticket_text = ticket.0.as_uuid().as_hyphenated().to_string();
    let held: i64 = {
        let conn = rusqlite::Connection::open(dir.join("app.db"))
            .expect("the state database opens for inspection");
        conn.query_row(
            "SELECT COUNT(*) FROM erasure_use_hold WHERE use_kind='inference_attempt' AND use_id=?1",
            [&ticket_text],
            |row| row.get(0),
        )
        .expect("the hold probe must answer")
    };
    assert_eq!(held, 1, "the last-page claim is durably associated");

    let delayed = delayed_formation(companion_raw, LearningClaimRef::from_raw(ticket.0));
    assert_eq!(
        store
            .commit_memory_change(delayed)
            .await
            .expect("the delayed commit must answer"),
        ene_learning::MemoryChangeOutcome::HeldForErasure,
        "a formation claimed before the interval stays stale after completion"
    );

    let fresh_consent = store
        .load_current(CapabilityKind::Learning)
        .await
        .expect("the Learning consent must read")
        .expect("the Learning consent stays assigned");
    let fresh_credential_set = store
        .current_set_revision()
        .await
        .expect("the credential-set revision must read");
    let fresh_ticket = ene_inference::InferenceTicketId(RawId::new());
    assert_eq!(
        store
            .begin_inference_attempt(ene_inference::InferenceAttempt {
                ticket: fresh_ticket,
                consumer: ene_permission::ConsumerKind::CompanionLearning,
                capability: CapabilityKind::Learning,
                purpose: ene_permission::PurposeKind::MemoryFormation,
                expected_consent: (fresh_consent.id, fresh_consent.rev),
                expected_credential_set: fresh_credential_set,
                provider: String::from("openai"),
                model: String::from(MODEL),
                task_agent: None,
                data_use: vec![RawId::new()],
                pricing: None,
                usage_estimate: None,
            })
            .await
            .expect("the fresh claim must answer"),
        ene_inference::AttemptBeginOutcome::Started
    );
    assert!(
        matches!(
            store
                .commit_memory_change(delayed_formation(
                    companion_raw,
                    LearningClaimRef::from_raw(fresh_ticket.0)
                ))
                .await
                .expect("the fresh commit must answer"),
            ene_learning::MemoryChangeOutcome::Committed { .. }
        ),
        "a post-completion origin is accepted"
    );
}

#[tokio::test]
async fn stage6_task_transient_observation_after_completion_is_collected() {
    const LEG_TARGET: &str = TARGET;
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let paraphrase = "The input file describes confidential material; I did not copy its contents.";
    let fresh = format!("a fresh note about {LEG_TARGET}");
    let proposal = task_reply(serde_json::json!({
        "kind": "propose_task",
        "purpose": "write a report about the workspace input",
    }));
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_task_agent_turn(0),
                Call::text(r#"{"tool":"read","path":"input.txt"}"#),
            ),
            (
                on_task_agent_turn(1),
                Call::text(format!(r#"{{"final":"{paraphrase}"}}"#)),
            ),
            (
                on_latest_owner("please read input.txt and write report.md"),
                Call::text(proposal),
            ),
            (on_latest_owner(&fresh), Call::text("acknowledged")),
        ],
        &[],
    ));
    transport.block_input(on_task_agent_turn(1));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE],
    )
    .await;
    let workspace = dir.join("workspace");
    let input = workspace.join("input.txt");
    std::fs::create_dir_all(&workspace).expect("the workspace directory creates");
    std::fs::write(&input, format!("confidential: {LEG_TARGET}"))
        .expect("the workspace source writes");
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

    let db = served.dir.join("app.db");
    assert_eq!(
        transient_observation_rows(&db),
        1,
        "the execution-local observation occurrence is durable before deletion"
    );
    assert!(
        transient_observation_body_observed(&db),
        "the occurrence reproduced a target-bearing body at observation time"
    );

    std::fs::write(&input, "ordinary notes after the observation")
        .expect("the workspace source is rewritten clean");
    assert!(
        !std::fs::read_to_string(&input)
            .expect("the rewritten source reads")
            .contains(LEG_TARGET),
        "deletion admission happens after the current workspace read is clean"
    );

    let outcome = request_deletion(served.client(), LEG_TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(outcome, ManagementOutcome::NeedsClarification);
    let handle = served.handle_arc();
    confirm_deletion(&handle).await;
    assert_eq!(
        transient_task_delegation_holds(&db),
        1,
        "the observed covered source associates the delegation at admission"
    );
    let page = drive_until(&handle, served.client(), DeletionPhaseWire::Completed)
        .await
        .expect("the operation must complete while the final turn is parked");
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Completed);

    transport.release_blocked();
    wait_task_progress(served.client(), "completed", 1)
        .await
        .expect("the task completes on the collected result");
    let body = transient_sole_result_body(&db);
    assert_eq!(
        body, "[erased]",
        "the delayed paraphrase is never stored raw"
    );
    assert!(
        !body.contains(paraphrase),
        "the stale paraphrase is not durably adopted"
    );
    assert_eq!(served.canonical_remainder(LEG_TARGET).await, 0);
    assert!(
        db_target_hits(&db, LEG_TARGET).is_empty(),
        "the completed surface keeps no target body: {:?}",
        db_target_hits(&db, LEG_TARGET)
    );
    let (_round, _stream, reply) = send_round(served.client(), &fresh)
        .await
        .expect("a fresh origin must be accepted");
    assert!(reply.contains("acknowledged"), "{reply}");
    assert!(
        history_texts(served.client())
            .await
            .iter()
            .any(|text| text.contains(LEG_TARGET)),
        "a fresh post-completion Owner input remains allowed"
    );
    served.server.abort();
}

#[tokio::test]
async fn stage6_task_sealed_observation_paraphrase_is_erased_after_workspace_rewrite() {
    const LEG_TARGET: &str = TARGET;
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let paraphrase = "The input file describes confidential material; I did not copy its contents.";
    let fresh = format!("a fresh note about {LEG_TARGET}");
    let proposal = task_reply(serde_json::json!({
        "kind": "propose_task",
        "purpose": "write a report about the workspace input",
    }));
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_task_agent_turn(0),
                Call::text(r#"{"tool":"read","path":"input.txt"}"#),
            ),
            (
                on_task_agent_turn(1),
                Call::text(format!(r#"{{"final":"{paraphrase}"}}"#)),
            ),
            (
                on_latest_owner("please read input.txt and write report.md"),
                Call::text(proposal),
            ),
            (on_latest_owner(&fresh), Call::text("acknowledged")),
        ],
        &[],
    ));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE],
    )
    .await;
    let workspace = dir.join("workspace");
    let input = workspace.join("input.txt");
    std::fs::create_dir_all(&workspace).expect("the workspace directory creates");
    std::fs::write(&input, format!("confidential: {LEG_TARGET}"))
        .expect("the workspace source writes");
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
    wait_task_progress(served.client(), "completed", 1)
        .await
        .expect("the paraphrased result seals before deletion");

    let db = served.dir.join("app.db");
    assert_eq!(
        transient_observation_rows(&db),
        1,
        "the execution-local observation occurrence is durable"
    );
    assert!(
        transient_observation_body_observed(&db),
        "the occurrence reproduced a target-bearing body"
    );
    assert_eq!(
        transient_sole_result_body(&db),
        paraphrase,
        "the sealed paraphrase is stored before the workspace rewrite"
    );
    assert_eq!(
        transient_action_success_rows(&db),
        1,
        "the producing Action attempt is already confirmed"
    );

    std::fs::write(&input, "ordinary notes after the observation")
        .expect("the workspace source is rewritten clean");
    assert!(
        !std::fs::read_to_string(&input)
            .expect("the rewritten source reads")
            .contains(LEG_TARGET),
        "deletion admission happens after the current workspace read is clean"
    );

    let outcome = request_deletion(served.client(), LEG_TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(outcome, ManagementOutcome::NeedsClarification);
    let handle = served.handle_arc();
    confirm_deletion(&handle).await;
    let page = drive_until(&handle, served.client(), DeletionPhaseWire::Completed)
        .await
        .expect("the operation must complete");
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Completed);

    let body = transient_sole_result_body(&db);
    assert_eq!(
        body, "[erased]",
        "the sealed paraphrase cannot survive as undeleted derived personal data"
    );
    assert!(!body.contains(paraphrase));
    assert_eq!(
        transient_action_success_rows(&db),
        1,
        "Action certainty is an objective fact and is never rewritten"
    );
    assert_eq!(served.canonical_remainder(LEG_TARGET).await, 0);
    assert!(
        db_target_hits(&db, LEG_TARGET).is_empty(),
        "the completed surface keeps no target body: {:?}",
        db_target_hits(&db, LEG_TARGET)
    );
    wait_task_progress(served.client(), "completed", 1)
        .await
        .expect("the sealed execution stays completed");
    let (_round, _stream, reply) = send_round(served.client(), &fresh)
        .await
        .expect("a fresh origin must be accepted");
    assert!(reply.contains("acknowledged"), "{reply}");
    assert!(
        history_texts(served.client())
            .await
            .iter()
            .any(|text| text.contains(LEG_TARGET)),
        "a fresh post-completion Owner input remains allowed"
    );
    served.server.abort();
}

#[tokio::test]
async fn stage6_observation_write_across_deletion_stays_old_origin() {
    const LEG_TARGET: &str = TARGET;
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let paraphrase = "The input file describes confidential material; I did not copy its contents.";
    let fresh = format!("a fresh note about {LEG_TARGET}");
    let proposal = task_reply(serde_json::json!({
        "kind": "propose_task",
        "purpose": "write a report about the workspace input",
    }));
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_task_agent_turn(0),
                Call::text(r#"{"tool":"read","path":"input.txt"}"#),
            ),
            (
                on_task_agent_turn(1),
                Call::text(format!(r#"{{"final":"{paraphrase}"}}"#)),
            ),
            (
                on_latest_owner("please read input.txt and write report.md"),
                Call::text(proposal),
            ),
            (on_latest_owner(&fresh), Call::text("acknowledged")),
        ],
        &[],
    ));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE],
    )
    .await;
    let workspace = dir.join("workspace");
    let input = workspace.join("input.txt");
    std::fs::create_dir_all(&workspace).expect("the workspace directory creates");
    std::fs::write(&input, format!("confidential: {LEG_TARGET}"))
        .expect("the workspace source writes");
    select_workspace(served.client(), &workspace)
        .await
        .expect("workspace must select");

    served
        .handle()
        .store_for_tests()
        .arm_observation_write_park_for_tests();
    let (round, stream, reply) =
        send_round(served.client(), "please read input.txt and write report.md")
            .await
            .expect("the propose round must complete");
    assert!(
        reply.contains("Task accepted"),
        "the task proposal must be accepted: {reply}"
    );
    confirm_round(served.client(), &round, stream).await;
    served
        .handle()
        .store_for_tests()
        .wait_observation_write_park_for_tests()
        .await;

    let db = served.dir.join("app.db");
    assert_eq!(
        transient_observation_rows(&db),
        0,
        "the occurrence is not durable while the Action body is still in memory"
    );

    std::fs::write(&input, "ordinary notes after the observation")
        .expect("the workspace source is rewritten clean");
    assert!(
        !std::fs::read_to_string(&input)
            .expect("the rewritten source reads")
            .contains(LEG_TARGET),
        "deletion admission happens after the current workspace read is clean"
    );

    let outcome = request_deletion(served.client(), LEG_TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(outcome, ManagementOutcome::NeedsClarification);
    let handle = served.handle_arc();
    confirm_deletion(&handle).await;
    assert_eq!(
        transient_task_delegation_holds(&db),
        1,
        "the unobserved read/list execution is associated at admission"
    );
    assert_eq!(transient_observation_rows(&db), 0);
    let page = drive_until(&handle, served.client(), DeletionPhaseWire::Completed)
        .await
        .expect("the operation must complete while the observation write is parked");
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Completed);

    served
        .handle()
        .store_for_tests()
        .release_observation_write_park_for_tests();
    wait_task_progress(served.client(), "completed", 1)
        .await
        .expect("the task completes on the collected result");

    assert_eq!(transient_observation_rows(&db), 1);
    assert!(
        transient_observation_body_observed(&db),
        "the occurrence reproduced a target-bearing body"
    );
    for input_text in transport.input_texts() {
        if input_text.starts_with("[RESPONSE FORMAT]") && input_text.contains("[TOOL CALL]") {
            assert!(
                !input_text.contains(LEG_TARGET),
                "the old observation body must not enter a later provider turn: {input_text}"
            );
        }
    }
    let body = transient_sole_result_body(&db);
    assert_eq!(
        body, "[erased]",
        "the delayed paraphrase is never stored raw"
    );
    assert!(!body.contains(paraphrase));
    assert!(!body.contains(LEG_TARGET));
    assert_eq!(
        transient_action_success_rows(&db),
        1,
        "Action certainty is an objective fact and is never rewritten"
    );
    assert_eq!(served.canonical_remainder(LEG_TARGET).await, 0);
    assert!(
        db_target_hits(&db, LEG_TARGET).is_empty(),
        "the completed surface keeps no target body: {:?}",
        db_target_hits(&db, LEG_TARGET)
    );
    wait_task_progress(served.client(), "completed", 1)
        .await
        .expect("the sealed execution stays completed");
    let (_round, _stream, reply) = send_round(served.client(), &fresh)
        .await
        .expect("a fresh origin must be accepted");
    assert!(reply.contains("acknowledged"), "{reply}");
    assert!(
        history_texts(served.client())
            .await
            .iter()
            .any(|text| text.contains(LEG_TARGET)),
        "a fresh post-completion Owner input remains allowed"
    );
    served.server.abort();
}

#[tokio::test]
async fn stage6_reconciliation_erases_paraphrase_pinned_past_the_page() {
    use ene_companion::{CompanionRepository as _, HistoryRepository as _};
    use ene_learning::{
        ChangeKind, ExperienceSourceKind, Importance, LearningRepository as _, LearningScope,
        MemoryChange, MemoryChangeCommit, MemoryId, MemoryTarget, SourceRangeRef, SummaryId,
        SummaryRecord, TemporalMeaning,
    };
    use ene_presence::PresenceRepository as _;

    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let mut served = serve_and_setup(
        dir.clone(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE, cmds::CAPABILITY_LEARNING],
    )
    .await;
    served.stop().await;
    let (late, paraphrase_id, memory_id, companion_raw) = {
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
        let mut sources = Vec::new();
        for index in 0..(ene_preservation::DELETION_RECONCILIATION_PAGE_SIZE + 6) {
            sources.push(
                seed_owner_message(
                    &store,
                    companion,
                    generation,
                    &format!("note {index} carries {TARGET}"),
                )
                .await,
            );
        }
        let late = *sources
            .iter()
            .max_by_key(|source| source.as_uuid())
            .expect("the fixture has sources");
        let paraphrase_id = SummaryId::generate();
        let memory_id = MemoryId::generate();
        let committed = store
            .commit_memory_change(MemoryChangeCommit {
                summary: Some(SummaryRecord {
                    id: paraphrase_id,
                    scope: LearningScope::companion(companion.as_raw()),
                    content: String::from("The owner keeps a private launch credential."),
                    source: SourceRangeRef {
                        kind: ExperienceSourceKind::Dialogue,
                        start: late,
                        end: late,
                    },
                    formed_at: WallClockWithTz::now(),
                }),
                secret_premise: None,
                claim: None,
                change: MemoryChange {
                    target: MemoryTarget::New { id: memory_id },
                    scope: LearningScope::companion(companion.as_raw()),
                    content: String::from("The owner keeps a private launch credential."),
                    importance: Importance::default(),
                    temporal: TemporalMeaning::Enduring,
                    change: ChangeKind::Initial,
                    at: WallClockWithTz::now(),
                },
            })
            .await
            .expect("the paraphrase formation must answer");
        assert!(
            matches!(
                committed,
                ene_learning::MemoryChangeOutcome::Committed { .. }
            ),
            "the paraphrase Memory must commit: {committed:?}"
        );
        (late, paraphrase_id, memory_id, companion.as_raw())
    };

    let mut client = served.serve().await;
    let outcome = request_deletion(&mut client, TARGET)
        .await
        .expect("the request inlet must answer");
    assert_eq!(outcome, ManagementOutcome::NeedsClarification);
    let handle = served.handle_arc();
    confirm_deletion(&handle).await;
    let page = drive_until(&handle, &mut client, DeletionPhaseWire::Completed)
        .await
        .expect("the operation must reconcile every page and erase derived data");
    assert_eq!(page.operations[0].phase, DeletionPhaseWire::Completed);
    assert_eq!(served.canonical_remainder(TARGET).await, 0);
    assert!(
        db_target_hits(&served.dir.join("app.db"), TARGET).is_empty(),
        "no durable table keeps the target: {:?}",
        db_target_hits(&served.dir.join("app.db"), TARGET)
    );

    served.stop().await;
    let store = ene_store::Store::open(&dir.join("app.db"))
        .await
        .expect("the state database reopens");
    let late_text = late.as_uuid().as_hyphenated().to_string();
    let remaining_late: i64 = {
        let conn = rusqlite::Connection::open(dir.join("app.db"))
            .expect("the state database opens for inspection");
        conn.query_row(
            "SELECT COUNT(*) FROM history_message WHERE message_id=?1",
            [&late_text],
            |row| row.get(0),
        )
        .expect("the late History probe must answer")
    };
    assert_eq!(remaining_late, 0, "H_late is erased");
    let summaries = store
        .load_summaries(&[paraphrase_id])
        .await
        .expect("summaries must load");
    assert!(
        summaries.is_empty(),
        "the paraphrase Summary pinned on H_late must be erased"
    );
    assert!(
        store
            .list_memory_revisions(memory_id, None, 100)
            .await
            .expect("revisions must list")
            .is_empty(),
        "the derived Memory must be erased"
    );
    let recalled = store
        .recall_candidates(companion_raw, &[String::from("launch")], 50)
        .await
        .expect("recall must answer");
    assert!(recalled.iter().all(|item| item.id != memory_id));

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
    let fresh = seed_owner_message(
        &store,
        companion,
        generation,
        &format!("a fresh note about {TARGET}"),
    )
    .await;
    let timeline = store
        .load_timeline(companion, None, None, 200)
        .await
        .expect("the timeline must load");
    assert!(
        timeline
            .iter()
            .any(|item| item.id == fresh && item.text.contains(TARGET)),
        "a post-completion Owner origin of the same string is accepted"
    );
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
fn transient_observation_rows(db: &Path) -> i64 {
    let conn = rusqlite::Connection::open(db).expect("the state database opens");
    conn.busy_timeout(Duration::from_secs(30))
        .expect("a busy timeout must set");
    conn.query_row("SELECT COUNT(*) FROM task_agent_observation", [], |row| {
        row.get(0)
    })
    .expect("the observation probe must run")
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
fn transient_observation_body_observed(db: &Path) -> bool {
    let conn = rusqlite::Connection::open(db).expect("the state database opens");
    conn.busy_timeout(Duration::from_secs(30))
        .expect("a busy timeout must set");
    conn.query_row(
        "SELECT body_observed FROM task_agent_observation",
        [],
        |row| row.get(0),
    )
    .expect("the observation body-observed probe must run")
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
fn transient_action_success_rows(db: &Path) -> i64 {
    let conn = rusqlite::Connection::open(db).expect("the state database opens");
    conn.busy_timeout(Duration::from_secs(30))
        .expect("a busy timeout must set");
    conn.query_row(
        "SELECT COUNT(*) FROM action_attempt WHERE certainty = 'confirmed_success'",
        [],
        |row| row.get(0),
    )
    .expect("the certainty probe must run")
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
fn transient_task_delegation_holds(db: &Path) -> i64 {
    let conn = rusqlite::Connection::open(db).expect("the state database opens");
    conn.busy_timeout(Duration::from_secs(30))
        .expect("a busy timeout must set");
    conn.query_row(
        "SELECT COUNT(*) FROM erasure_use_hold WHERE use_kind = 'task_delegation'",
        [],
        |row| row.get(0),
    )
    .expect("the hold probe must run")
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
fn transient_sole_result_body(db: &Path) -> String {
    let conn = rusqlite::Connection::open(db).expect("the state database opens");
    conn.busy_timeout(Duration::from_secs(30))
        .expect("a busy timeout must set");
    conn.query_row(
        "SELECT body FROM task_result ORDER BY rowid DESC LIMIT 1",
        [],
        |row| row.get(0),
    )
    .expect("the result body must read")
}

#[tokio::test]
async fn stage6_task_result_commits_under_the_credential_set_current_at_its_scrub() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let proposal = task_reply(serde_json::json!({
        "kind": "propose_task",
        "purpose": "write a report",
    }));
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_task_agent_turn(0),
                Call::text(r#"{"tool":"read","path":"input.txt"}"#),
            ),
            (
                on_task_agent_turn(1),
                Call::text(r##"{"tool":"create","path":"report.md","content":"# Report notes"}"##),
            ),
            (
                on_task_agent_turn(2),
                Call::text(format!(
                    r##"{{"final":"created report.md quoting {ROTATED_SECRET}"}}"##
                )),
            ),
            (
                on_latest_owner("please read input.txt and write report.md"),
                Call::text(proposal),
            ),
        ],
        &[],
    ));
    transport.block_input(on_task_agent_turn(2));
    let mut served = Served::start(
        dir.clone(),
        || memory_store_with_rotated(SECRET, ROTATED_SECRET),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE],
    )
    .await;
    let workspace = dir.join("c3-current-workspace");
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
            confirmed: false,
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
    let handle = served.handle_arc();
    assert!(
        matches!(
            handle.approve_credential("openai", "rotated").await,
            Ok(true)
        ),
        "the Host-local approval must register the rotated value"
    );
    transport.release_blocked();
    wait_task_progress(served.client(), "completed", 1)
        .await
        .expect("the final result commits under the advanced set");

    let tasks = list_tasks(served.client()).await.expect("tasks must list");
    let task = tasks.tasks.first().expect("the task exists");
    let report = ask(
        served.client(),
        WirePayload::GetTaskReport(cmds::task_report_request(&task.task.0, None, None)),
        "c3 report",
    )
    .await
    .expect("the report must answer");
    assert_absent("task report debug", &format!("{report:?}"), ROTATED_SECRET);
    let WirePayload::TaskReportResponse(ene_api::v1::undelivered::TaskReportResponse::Page(page)) =
        report
    else {
        panic!("the report must answer a page: {report:?}");
    };
    let mut result_bodies = Vec::new();
    for row in &page.rows {
        if row.kind != "task_result" {
            continue;
        }
        let Some(source) = row.source.as_ref() else {
            continue;
        };
        let body = ask(
            served.client(),
            WirePayload::GetReportSource(cmds::report_source_request(&source.0, None, None)),
            "c3 report source",
        )
        .await
        .expect("the report source must answer");
        assert_absent("report source debug", &format!("{body:?}"), ROTATED_SECRET);
        if let WirePayload::ReportSourceResponse(
            ene_api::v1::undelivered::ReportSourceResponse::Page(source_page),
        ) = body
        {
            result_bodies.push(source_page.text);
        }
    }
    assert!(
        !result_bodies.is_empty(),
        "the completed execution must expose a result body"
    );
    assert!(
        result_bodies
            .iter()
            .any(|body| body.contains(ene_credential::REDACTED_CREDENTIAL)),
        "the result body must carry the redaction marker: {result_bodies:?}"
    );
    for body in &result_bodies {
        assert_absent("result body", body, ROTATED_SECRET);
    }

    assert_absent_all("provider request", &transport.input_texts(), ROTATED_SECRET);
    assert_absent_all("provider request", &transport.input_texts(), SECRET);
    assert_absent_all(
        "history",
        &history_texts(served.client()).await,
        ROTATED_SECRET,
    );
    assert!(
        db_target_hits(&dir.join("app.db"), ROTATED_SECRET).is_empty(),
        "no durable table may carry the rotated value: {:?}",
        db_target_hits(&dir.join("app.db"), ROTATED_SECRET)
    );
    assert!(db_target_hits(&dir.join("app.db"), SECRET).is_empty());
    served.server.abort();
}

#[derive(serde::Serialize)]
struct IngressCrafted {
    envelope: WireEnvelope,
    payload: IngressPayload,
}

#[derive(serde::Serialize)]
enum IngressPayload {
    FutureThing(IngressNote),
    TextStreamClose(IngressClose),
    SubmitTextInput(IngressPartialSubmit),
}

#[derive(serde::Serialize)]
struct IngressNote {
    note: String,
}

#[derive(serde::Serialize)]
struct IngressClose {
    stream: StreamWireId,
    status: String,
}

#[derive(serde::Serialize)]
struct IngressPartialSubmit {
    companion: CompanionWireRef,
    round: Option<RoundWireId>,
    #[serde(default)]
    fresh: bool,
    body: TextBodyWire,
}

fn crafted_envelope(message_type: &str) -> WireEnvelope {
    new_outgoing_envelope(
        ProtocolVersion::V1,
        WireSender {
            device_id: None,
            incarnation_id: ClientIncarnationId {
                counter: 5,
                random: 900,
            },
            connection_id: None,
        },
        WireMessageType(message_type.to_string()),
    )
}

struct RawPinnedVerifier {
    expected_pin: String,
}

impl std::fmt::Debug for RawPinnedVerifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RawPinnedVerifier")
            .finish_non_exhaustive()
    }
}

impl ServerCertVerifier for RawPinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, TlsError> {
        let (_, certificate) = x509_parser::parse_x509_certificate(end_entity.as_ref())
            .map_err(|_| TlsError::InvalidCertificate(CertificateError::BadEncoding))?;
        let digest = sha2::Sha256::digest(certificate.public_key().raw);
        let offered: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        if offered == self.expected_pin {
            return Ok(rustls::client::danger::ServerCertVerified::assertion());
        }
        Err(TlsError::InvalidCertificate(
            CertificateError::ApplicationVerificationFailure,
        ))
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        let provider = rustls::crypto::aws_lc_rs::default_provider();
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        let provider = rustls::crypto::aws_lc_rs::default_provider();
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        let provider = rustls::crypto::aws_lc_rs::default_provider();
        provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

struct WssClient {
    socket:
        tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
}

impl WssClient {
    async fn connect(dir: &Path) -> Result<Self, String> {
        Self::connect_with(dir, None, None, false, None).await
    }

    async fn load_runtime(dir: &Path) -> Result<HostRuntimeInfo, String> {
        let runtime_path = dir.join(HOST_RUNTIME_FILE_NAME);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !runtime_path.exists() {
            if tokio::time::Instant::now() >= deadline {
                return Err(String::from("runtime information was never published"));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let bytes =
            std::fs::read(&runtime_path).map_err(|error| format!("runtime read: {error}"))?;
        serde_json::from_slice(&bytes).map_err(|error| format!("runtime parse: {error}"))
    }

    async fn connect_with(
        dir: &Path,
        token: Option<&str>,
        pin: Option<&str>,
        origin: bool,
        generation: Option<&str>,
    ) -> Result<Self, String> {
        let runtime = Self::load_runtime(dir).await?;
        let port = runtime
            .local_port()
            .ok_or("runtime is not a local wss url")?;
        let tcp = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|error| format!("connect: {error}"))?;
        Self::handshake_on(&runtime, tcp, token, pin, origin, generation).await
    }

    /// Dials with fixed kernel socket buffers, so a test can make one
    /// direction stall at a known bound instead of depending on autotuning.
    async fn connect_sized(
        dir: &Path,
        recv_buffer: Option<u32>,
        send_buffer: Option<u32>,
    ) -> Result<Self, String> {
        let runtime = Self::load_runtime(dir).await?;
        let port = runtime
            .local_port()
            .ok_or("runtime is not a local wss url")?;
        let socket = tokio::net::TcpSocket::new_v4().map_err(|error| format!("socket: {error}"))?;
        if let Some(size) = recv_buffer {
            socket
                .set_recv_buffer_size(size)
                .map_err(|error| format!("receive buffer: {error}"))?;
        }
        if let Some(size) = send_buffer {
            socket
                .set_send_buffer_size(size)
                .map_err(|error| format!("send buffer: {error}"))?;
        }
        let tcp = socket
            .connect(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                port,
            )))
            .await
            .map_err(|error| format!("connect: {error}"))?;
        Self::handshake_on(&runtime, tcp, None, None, false, None).await
    }

    /// Drives TLS and the WebSocket upgrade over an already-connected stream,
    /// so a test can hold the stream between TCP connect and handshake.
    async fn handshake_on(
        runtime: &HostRuntimeInfo,
        tcp: tokio::net::TcpStream,
        token: Option<&str>,
        pin: Option<&str>,
        origin: bool,
        generation: Option<&str>,
    ) -> Result<Self, String> {
        let port = runtime
            .local_port()
            .ok_or("runtime is not a local wss url")?;
        let expected_pin = pin.unwrap_or(&runtime.host_pin).to_owned();
        let config = ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(std::sync::Arc::new(RawPinnedVerifier {
                expected_pin,
            }))
            .with_no_client_auth();
        let connector = TlsConnector::from(std::sync::Arc::new(config));
        let server_name =
            ServerName::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST).into());
        let tls = connector
            .connect(server_name, tcp)
            .await
            .map_err(|error| format!("tls: {error}"))?;
        let presented_token = token.unwrap_or(&runtime.local_token);
        let presented_generation = generation.unwrap_or(&runtime.startup_generation).to_owned();
        let mut request = format!("wss://127.0.0.1:{port}/")
            .into_client_request()
            .map_err(|error| format!("build request: {error}"))?;
        let authorization = format!("Bearer {presented_token}")
            .parse::<tokio_tungstenite::tungstenite::http::HeaderValue>()
            .map_err(|error| format!("token header: {error}"))?;
        request.headers_mut().insert("authorization", authorization);
        let generation = presented_generation
            .parse::<tokio_tungstenite::tungstenite::http::HeaderValue>()
            .map_err(|error| format!("generation header: {error}"))?;
        request
            .headers_mut()
            .insert("x-ene-startup-generation", generation);
        if origin {
            request.headers_mut().insert(
                "origin",
                tokio_tungstenite::tungstenite::http::HeaderValue::from_static(
                    "https://evil.example",
                ),
            );
        }
        let socket = tokio_tungstenite::client_async_with_config(request, tls, None)
            .await
            .map_err(|error| format!("upgrade refused: {error}"))?;
        Ok(Self { socket: socket.0 })
    }

    async fn send_crafted(&mut self, frame: &IngressCrafted) -> WireMessageId {
        use futures_util::SinkExt as _;

        let message_id = frame.envelope.message_id;
        let body = rmp_serde::to_vec_named(frame).expect("crafted frame encodes");
        self.socket
            .send(Message::Binary(body.into()))
            .await
            .expect("crafted frame must send");
        message_id
    }

    async fn send_wire(&mut self, frame: &WireFrame) {
        use futures_util::SinkExt as _;

        let body = encode_frame(frame).expect("wire frame encodes");
        self.socket
            .send(Message::Binary(body.into()))
            .await
            .expect("wire frame must send");
    }

    async fn recv_wire(&mut self) -> WireFrame {
        use futures_util::{SinkExt as _, StreamExt as _};

        while let Some(message) = self.socket.next().await {
            match message.expect("host message must read") {
                Message::Binary(body) => {
                    return match decode_frame(&body).expect("host frame decodes") {
                        DecodedFrame::Known(frame) => frame,
                        other => panic!("host must send known frames, got {other:?}"),
                    };
                }
                Message::Ping(payload) => {
                    self.socket
                        .send(Message::Pong(payload))
                        .await
                        .expect("transport pong must send");
                }
                Message::Pong(_) | Message::Frame(_) => {}
                other => panic!("unexpected host message: {other:?}"),
            }
        }
        panic!("the Host closed the connection");
    }

    /// Declares an oversize frame without paying for its full payload, over the
    /// raw TLS stream underneath the upgraded socket.
    async fn send_oversize_and_expect_close(self) {
        let mut raw = self.socket.into_inner();
        let mask = *uuid::Uuid::new_v4().as_bytes();
        let mut header = vec![0x82, 0x80 | 127];
        header.extend_from_slice(&(1_073_741_824_u64).to_be_bytes());
        header.extend_from_slice(&mask);
        let junk: Vec<u8> = (0..1024)
            .map(|index: usize| (index % 251) as u8 ^ mask[index % 4])
            .collect();
        header.extend_from_slice(&junk);
        raw.write_all(&header)
            .await
            .expect("oversize header must write");
        expect_host_close(&mut raw).await;
    }

    /// Sends one binary message split across two fragments whose reassembled
    /// size crosses the wire cap, over the raw TLS stream underneath the
    /// upgraded socket.
    async fn send_fragmented_oversize_and_expect_close(self) {
        let mut raw = self.socket.into_inner();
        let mask = [0x11_u8, 0x22, 0x33, 0x44];
        let first = masked_fragment(0x02, 200 * 1024, &mask);
        raw.write_all(&first)
            .await
            .expect("first fragment must write");
        let second = masked_fragment(0x80, 100 * 1024, &mask);
        raw.write_all(&second)
            .await
            .expect("second fragment must write");
        // `write_all` on a TLS stream can complete with records still sitting
        // in the TLS buffer; without this flush the Host never receives the
        // whole fragmented message and has nothing to reject.
        drop(raw.flush().await);
        expect_host_close(&mut raw).await;
    }

    async fn expect_reject(&mut self, reply_to: WireMessageId, kind: RejectKind) {
        let reply = self.recv_wire().await;
        assert_eq!(
            reply.envelope.message_type.0, "Reject",
            "an unsupported value is answered with a wire reject"
        );
        assert_eq!(
            reply.envelope.correlation.reply_to,
            Some(reply_to),
            "the reject correlates to the rejected message"
        );
        match reply.payload {
            WirePayload::Reject(notice) => {
                assert_eq!(
                    notice.kind, kind,
                    "the reject carries the typed reason: {notice:?}"
                );
                assert!(!notice.detail.is_empty(), "{notice:?}");
            }
            other => panic!("expected Reject, got {}", other.message_type()),
        }
    }
}

/// A masked, non-final (`0x02`) or continuation-final (`0x80`) fragment
/// header plus `length` bytes of payload.
fn masked_fragment(opcode: u8, length: usize, mask: &[u8; 4]) -> Vec<u8> {
    let mut fragment = vec![opcode, 0x80 | 127];
    fragment.extend_from_slice(&(length as u64).to_be_bytes());
    fragment.extend_from_slice(mask);
    let payload: Vec<u8> = (0..length)
        .map(|index| (index % 251) as u8 ^ mask[index % 4])
        .collect();
    fragment.extend_from_slice(&payload);
    fragment
}

/// Reads the raw TLS stream until the Host closes the connection.
async fn expect_host_close(raw: &mut tokio_rustls::client::TlsStream<tokio::net::TcpStream>) {
    let closed = tokio::time::timeout(Duration::from_secs(10), async {
        let mut chunk = [0_u8; 1024];
        loop {
            match raw.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(read) => {
                    if chunk[..read].windows(2).any(|pair| pair[0] & 0x0f == 0x8) {
                        return;
                    }
                }
            }
        }
    })
    .await;
    assert!(
        closed.is_ok(),
        "the Host must close the connection after the crafted WebSocket message"
    );
}
async fn raw_dial(dir: &Path) -> WssClient {
    tokio::time::timeout(Duration::from_secs(15), WssClient::connect(dir))
        .await
        .expect("the dial must finish")
        .expect("a same-machine client with the current token must upgrade")
}

#[tokio::test]
async fn a_stalled_pre_auth_connection_does_not_delay_the_next_client() {
    let dir = tempfile::tempdir().expect("temp dir");
    let handle = open_host(dir.path()).await;
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(conn::run_until_shutdown(
        dir.path().to_path_buf(),
        Arc::clone(&handle),
        transport,
        shutdown,
    ));

    let runtime = WssClient::load_runtime(dir.path())
        .await
        .expect("runtime must publish");
    let port = runtime.local_port().expect("a local wss url");
    // Client A completes the TCP handshake and then never speaks TLS, so its
    // server-side upgrade stalls for the whole production upgrade timeout.
    let stalled = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .expect("the stalled client must connect");
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Client B must not pay Client A's upgrade timeout (production: 10s).
    let connected = tokio::time::timeout(Duration::from_secs(5), WssClient::connect(dir.path()))
        .await
        .expect("the next client must not wait for the stalled upgrade")
        .expect("the next client must upgrade");
    drop(connected);

    stop.send_replace(true);
    let joined = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("the listener must stop")
        .expect("the listener must not panic");
    joined.expect("the listener must shut down cleanly");
    drop(stalled);
}

#[tokio::test]
async fn an_upgrade_in_progress_survives_unrelated_select_activity() {
    let dir = tempfile::tempdir().expect("temp dir");
    let handle = open_host(dir.path()).await;
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(conn::run_until_shutdown(
        dir.path().to_path_buf(),
        Arc::clone(&handle),
        transport,
        shutdown,
    ));

    let runtime = WssClient::load_runtime(dir.path())
        .await
        .expect("runtime must publish");
    let port = runtime.local_port().expect("a local wss url");
    let tcp = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .expect("the slow client must connect");
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Unrelated branches of the serving loop fire while that upgrade is in
    // flight: other connections are accepted, upgraded, served and closed,
    // and the control listener takes a requester that immediately leaves.
    for _ in 0..3 {
        let other = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .expect("another client must connect");
        drop(other);
    }
    drop(
        tokio::time::timeout(Duration::from_secs(5), WssClient::connect(dir.path()))
            .await
            .expect("the other client must not hang either")
            .expect("another client must upgrade"),
    );
    drop(
        ene_core::host_control::ControlClient::connect(dir.path())
            .await
            .expect("the control listener must answer"),
    );
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The slow client now finishes its handshake: the in-flight upgrade must
    // have survived every unrelated branch firing in the meantime.
    let finished = tokio::time::timeout(
        Duration::from_secs(5),
        WssClient::handshake_on(&runtime, tcp, None, None, false, None),
    )
    .await
    .expect("the in-flight upgrade must complete")
    .expect("the in-flight upgrade must succeed");
    drop(finished);

    stop.send_replace(true);
    let joined = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("the listener must stop")
        .expect("the listener must not panic");
    joined.expect("the listener must shut down cleanly");
}

#[tokio::test]
async fn shutdown_joins_a_stalled_pre_auth_upgrade() {
    let dir = tempfile::tempdir().expect("temp dir");
    let handle = open_host(dir.path()).await;
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(conn::run_until_shutdown(
        dir.path().to_path_buf(),
        Arc::clone(&handle),
        transport,
        shutdown,
    ));

    let runtime = WssClient::load_runtime(dir.path())
        .await
        .expect("runtime must publish");
    let port = runtime.local_port().expect("a local wss url");
    let stalled = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .expect("the stalled client must connect");
    tokio::time::sleep(Duration::from_millis(200)).await;

    stop.send_replace(true);
    // Shutdown must stop the stalled upgrade task instead of waiting out its
    // production timeout.
    let joined = tokio::time::timeout(Duration::from_secs(6), server)
        .await
        .expect("shutdown must not wait out the upgrade timeout")
        .expect("the listener must not panic");
    joined.expect("the listener must shut down cleanly");
    drop(stalled);
}

#[tokio::test]
async fn unknown_wire_values_are_typed_rejects_and_the_connection_stays_usable() {
    let dir = tempfile::tempdir().expect("temp dir");
    let handle = open_host(dir.path()).await;
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(conn::run_until_shutdown(
        dir.path().to_path_buf(),
        Arc::clone(&handle),
        transport,
        shutdown,
    ));
    let mut host = raw_dial(dir.path()).await;

    let unknown_type = host
        .send_crafted(&IngressCrafted {
            envelope: crafted_envelope("FutureThing"),
            payload: IngressPayload::FutureThing(IngressNote {
                note: String::from("from a newer peer"),
            }),
        })
        .await;
    host.expect_reject(unknown_type, RejectKind::UnsupportedMessage)
        .await;

    let unknown_value = host
        .send_crafted(&IngressCrafted {
            envelope: crafted_envelope("TextStreamClose"),
            payload: IngressPayload::TextStreamClose(IngressClose {
                stream: StreamWireId(uuid::Uuid::new_v4()),
                status: String::from("Suspended"),
            }),
        })
        .await;
    host.expect_reject(unknown_value, RejectKind::UnsupportedFieldValue)
        .await;

    let missing_field = host
        .send_crafted(&IngressCrafted {
            envelope: crafted_envelope("SubmitTextInput"),
            payload: IngressPayload::SubmitTextInput(IngressPartialSubmit {
                companion: CompanionWireRef(String::from("default")),
                round: None,
                fresh: false,
                body: TextBodyWire {
                    text: String::from("local_id deliberately absent"),
                    lang: TextLangWire(String::from("en")),
                },
            }),
        })
        .await;
    host.expect_reject(missing_field, RejectKind::MissingRequiredField)
        .await;

    let mut pairing = WireFrame {
        envelope: crafted_envelope("PairingRequest"),
        payload: WirePayload::PairingRequest(PairingRequest {
            device_descriptor: String::from("raw ingress"),
        }),
    };
    pairing.envelope.correlation.request_id = Some(RequestWireId(uuid::Uuid::new_v4()));
    let pairing_id = pairing.envelope.message_id;
    host.send_wire(&pairing).await;
    let reply = host.recv_wire().await;
    assert_eq!(
        reply.envelope.correlation.reply_to,
        Some(pairing_id),
        "the same connection still answers a real handshake step"
    );
    match reply.payload {
        WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation { .. }) => {}
        other => panic!(
            "a live connection must still pair, got {}",
            other.message_type()
        ),
    }

    stop.send_replace(true);
    let joined = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("the listener must stop")
        .expect("the listener must not panic");
    joined.expect("the listener must shut down cleanly");
}

#[tokio::test]
async fn the_host_refuses_an_upgrade_without_the_current_local_token() {
    let dir = tempfile::tempdir().expect("temp dir");
    let handle = open_host(dir.path()).await;
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(conn::run_until_shutdown(
        dir.path().to_path_buf(),
        Arc::clone(&handle),
        transport,
        shutdown,
    ));
    let refusal = match tokio::time::timeout(
        Duration::from_secs(15),
        WssClient::connect_with(dir.path(), Some("not-the-token"), None, false, None),
    )
    .await
    {
        Ok(Ok(_)) => panic!("a wrong token must be refused at the upgrade"),
        Ok(Err(refusal)) => refusal,
        Err(_) => panic!("the refusal must finish in time"),
    };
    assert!(
        refusal.contains("403"),
        "the refusal is a forbidden response: {refusal}"
    );
    stop.send_replace(true);
    let joined = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("the listener must stop")
        .expect("the listener must not panic");
    joined.expect("the listener must shut down cleanly");
}

#[tokio::test]
async fn the_host_refuses_an_upgrade_that_carries_an_origin() {
    let dir = tempfile::tempdir().expect("temp dir");
    let handle = open_host(dir.path()).await;
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(conn::run_until_shutdown(
        dir.path().to_path_buf(),
        Arc::clone(&handle),
        transport,
        shutdown,
    ));
    let refusal = match tokio::time::timeout(
        Duration::from_secs(15),
        WssClient::connect_with(dir.path(), None, None, true, None),
    )
    .await
    {
        Ok(Ok(_)) => panic!("an Origin-bearing upgrade must be refused"),
        Ok(Err(refusal)) => refusal,
        Err(_) => panic!("the refusal must finish in time"),
    };
    assert!(
        refusal.contains("403"),
        "the refusal is a forbidden response: {refusal}"
    );
    stop.send_replace(true);
    let joined = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("the listener must stop")
        .expect("the listener must not panic");
    joined.expect("the listener must shut down cleanly");
}

#[tokio::test]
async fn an_oversize_ws_frame_closes_the_connection_without_allocating_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let handle = open_host(dir.path()).await;
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(conn::run_until_shutdown(
        dir.path().to_path_buf(),
        Arc::clone(&handle),
        transport,
        shutdown,
    ));
    let host = raw_dial(dir.path()).await;
    host.send_oversize_and_expect_close().await;
    stop.send_replace(true);
    let joined = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("the listener must stop")
        .expect("the listener must not panic");
    joined.expect("the listener must shut down cleanly");
}

#[tokio::test]
async fn the_runtime_information_is_published_while_serving_and_removed_on_graceful_stop() {
    let dir = tempfile::tempdir().expect("temp dir");
    let handle = open_host(dir.path()).await;
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(conn::run_until_shutdown(
        dir.path().to_path_buf(),
        Arc::clone(&handle),
        transport,
        shutdown,
    ));
    dial_until_pending(dir.path())
        .await
        .expect("the listener must accept the first pairing");
    let path = dir.path().join(HOST_RUNTIME_FILE_NAME);
    let bytes = std::fs::read(&path).expect("runtime information must be published");
    let runtime: HostRuntimeInfo = serde_json::from_slice(&bytes).expect("runtime must parse");
    let rendered = format!("{runtime:?}");
    assert!(
        !rendered.contains(&runtime.local_token),
        "runtime Debug must redact the local token: {rendered}"
    );
    assert!(
        runtime.local_port().is_some(),
        "the published url is a local wss endpoint: {}",
        runtime.url
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path)
            .expect("runtime metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "the runtime file is owner-only");
    }
    stop.send_replace(true);
    let joined = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("the listener must stop")
        .expect("the listener must not panic");
    joined.expect("the listener must shut down cleanly");
    assert!(
        !path.exists(),
        "a graceful stop removes the runtime information"
    );
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
async fn expect_stale_connection(client: &mut Client, payload: WirePayload, what: &str) {
    let answer = ask(client, payload, what).await;
    let WirePayload::Reject(reject) = answer.expect("a superseded connection answers typed") else {
        panic!("{what} must answer a wire reject");
    };
    assert_eq!(
        reject.kind,
        RejectKind::StaleConnection,
        "{what} must name the stale connection"
    );
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
fn presence_row(dir: &Path) -> Option<(String, i64)> {
    let conn = rusqlite::Connection::open(dir.join("app.db")).expect("the store file must open");
    conn.query_row(
        "SELECT state, generation FROM presence_attribution",
        (),
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .expect("the presence attribution must read")
}

async fn wait_presence(dir: &Path, wanted: &str) -> (String, i64) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(row) = presence_row(dir)
            && row.0 == wanted
        {
            return row;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "presence never reached {wanted}: {:?}",
            presence_row(dir)
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_superseded_connection_answers_a_typed_stale_connection() {
    let dir = tempfile::tempdir().expect("temp dir");
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let mut served = serve_and_setup(
        dir.path().to_path_buf(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE],
    )
    .await;
    let mut c1 = served.client.take().expect("the started client exists");

    let (round1, stream1, text1) = send_round(&mut c1, "first client round")
        .await
        .expect("c1 must serve");
    confirm_round(&mut c1, &round1, stream1).await;

    let mut c2 = connect(dir.path()).await;
    let (round2, stream2, _text2) = send_round(&mut c2, "second client round")
        .await
        .expect("c2 must serve");
    confirm_round(&mut c2, &round2, stream2).await;

    let companion = c2.companion_ref();
    let stale_round = ask(
        &mut c2,
        WirePayload::SubmitTextInput(cmds::submit_input(
            &companion,
            RoundTarget::Existing(RoundWireId(round1.clone())),
            String::from("join c1's round"),
            String::from("en"),
        )),
        "old round join",
    )
    .await
    .expect("the old-round submit must answer typed");
    assert!(
        matches!(
            stale_round,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::StaleRound { .. })
        ),
        "c2 must not inherit c1's open round, got {stale_round:?}"
    );

    let companion = c1.companion_ref();
    let target = c1.round_target();
    expect_stale_connection(
        &mut c1,
        WirePayload::SubmitTextInput(cmds::submit_input(
            &companion,
            target,
            String::from("replay on the superseded connection"),
            String::from("en"),
        )),
        "superseded input",
    )
    .await;
    expect_stale_connection(
        &mut c1,
        WirePayload::ManagementViewRequest(cmds::setup_view_request()),
        "superseded view",
    )
    .await;

    let (round3, stream3, text3) = send_round(&mut c2, "still current")
        .await
        .expect("c2 must stay current");
    confirm_round(&mut c2, &round3, stream3).await;
    assert_eq!(
        text3, text1,
        "the scripted provider reply must stream to the current client"
    );
    assert_eq!(
        transport.sends(),
        3,
        "one provider call per accepted round: the stale replays sent nothing"
    );

    drop(c1);
    let (round4, stream4, _text4) = send_round(&mut c2, "after the old connection closed")
        .await
        .expect("c2 must keep serving after the superseded connection closes");
    confirm_round(&mut c2, &round4, stream4).await;
    served.stop().await;
}

#[tokio::test]
async fn reconnect_reauths_without_restoring_presence_and_a_fresh_summon_serves() {
    let dir = tempfile::tempdir().expect("temp dir");
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let mut served = serve_and_setup(
        dir.path().to_path_buf(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE],
    )
    .await;
    let mut c1 = served.client.take().expect("the started client exists");

    let (round1, stream1, _text1) = send_round(&mut c1, "summon round")
        .await
        .expect("the summon round must complete");
    confirm_round(&mut c1, &round1, stream1).await;
    wait_presence(dir.path(), "present").await;

    drop(c1);
    let (_, fallen_back) = wait_presence(dir.path(), "no_active").await;

    let mut c2 = connect(dir.path()).await;
    assert_eq!(
        presence_row(dir.path()),
        Some((String::from("no_active"), fallen_back)),
        "authentication alone must not restore attribution"
    );

    let (round2, stream2, _text2) = send_round(&mut c2, "summon again")
        .await
        .expect("the post-reconnect summon must complete");
    confirm_round(&mut c2, &round2, stream2).await;
    let (state, attached) = presence_row(dir.path()).expect("the attach must commit a row");
    assert_eq!(state, "present", "the fresh summon attaches presence");
    assert!(
        attached > fallen_back,
        "attaching advances the presence generation: {fallen_back} -> {attached}"
    );
    assert_eq!(
        transport.sends(),
        2,
        "one provider call per round: the reconnect and the attach sent nothing"
    );
    drop(c2);
    served.stop().await;
}

#[tokio::test]
async fn the_host_refuses_a_stale_startup_generation() {
    let dir = tempfile::tempdir().expect("temp dir");
    let handle = open_host(dir.path()).await;
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(conn::run_until_shutdown(
        dir.path().to_path_buf(),
        Arc::clone(&handle),
        transport,
        shutdown,
    ));
    let refusal = match tokio::time::timeout(
        Duration::from_secs(15),
        WssClient::connect_with(dir.path(), None, None, false, Some("stale-generation")),
    )
    .await
    {
        Ok(Ok(_)) => panic!("a stale startup generation must be refused"),
        Ok(Err(refusal)) => refusal,
        Err(_) => panic!("the refusal must finish in time"),
    };
    assert!(
        refusal.contains("403"),
        "the refusal is a forbidden response: {refusal}"
    );
    let runtime: HostRuntimeInfo = serde_json::from_slice(
        &std::fs::read(dir.path().join(HOST_RUNTIME_FILE_NAME)).expect("runtime must read"),
    )
    .expect("runtime must parse");
    assert!(
        !refusal.contains(&runtime.local_token),
        "the refusal must not echo the local token: {refusal}"
    );
    stop.send_replace(true);
    let joined = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("the listener must stop")
        .expect("the listener must not panic");
    joined.expect("the listener must shut down cleanly");
}

#[tokio::test]
async fn text_and_malformed_frames_close_the_connection() {
    let dir = tempfile::tempdir().expect("temp dir");
    let handle = open_host(dir.path()).await;
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(conn::run_until_shutdown(
        dir.path().to_path_buf(),
        Arc::clone(&handle),
        transport,
        shutdown,
    ));
    for attempt in 0..2_u8 {
        let mut client = raw_dial(dir.path()).await;
        use futures_util::{SinkExt as _, StreamExt as _};
        if attempt == 0 {
            client
                .socket
                .send(Message::Text("not part of the wire".into()))
                .await
                .expect("text must send");
        } else {
            client
                .socket
                .send(Message::Binary(vec![0xC1_u8, 0x01, 0xAA].into()))
                .await
                .expect("malformed body must send");
        }
        let closed = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match client.socket.next().await {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                    Some(Ok(_)) => continue,
                }
            }
        })
        .await;
        assert!(
            closed.is_ok(),
            "the Host must close the connection after a {} frame",
            if attempt == 0 { "text" } else { "malformed" }
        );
    }
    stop.send_replace(true);
    let joined = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("the listener must stop")
        .expect("the listener must not panic");
    joined.expect("the listener must shut down cleanly");
}

#[tokio::test]
async fn a_pre_auth_connection_that_never_negotiates_is_closed_at_the_machine_deadline() {
    let dir = tempfile::tempdir().expect("temp dir");
    let handle = open_host(dir.path()).await;
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(conn::run_until_shutdown(
        dir.path().to_path_buf(),
        Arc::clone(&handle),
        transport,
        shutdown,
    ));
    let mut silent = raw_dial(dir.path()).await;
    // Freeze only the clock to cross the machine deadline; every socket
    // operation stays on the real clock.
    tokio::time::pause();
    tokio::time::advance(MIRRORED_AUTH_DEADLINE + Duration::from_secs(5)).await;
    tokio::time::resume();
    let closed = tokio::time::timeout(Duration::from_secs(10), async {
        use futures_util::StreamExt as _;
        loop {
            match silent.socket.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await;
    assert!(
        closed.is_ok(),
        "a machine-controlled pre-auth stall must be closed at the deadline"
    );
    stop.send_replace(true);
    let joined = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("the listener must stop")
        .expect("the listener must not panic");
    joined.expect("the listener must shut down cleanly");
}

#[tokio::test]
async fn owner_confirmation_may_exceed_the_machine_deadline() {
    let dir = tempfile::tempdir().expect("temp dir");
    let handle = open_host(dir.path()).await;
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(conn::run_until_shutdown(
        dir.path().to_path_buf(),
        Arc::clone(&handle),
        transport,
        shutdown,
    ));
    let pending = dial_until_pending(dir.path())
        .await
        .expect("the first pairing must pend");
    // The owner reads the confirmation for longer than the machine deadline.
    tokio::time::pause();
    tokio::time::advance(MIRRORED_AUTH_DEADLINE + Duration::from_secs(30)).await;
    tokio::time::resume();
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let approved = handle
        .approve_device(pending.pending_id())
        .await
        .expect("approval must succeed");
    assert!(approved.is_some(), "approval must pair");
    let mut client = pending
        .complete()
        .await
        .expect("the connection must survive an owner wait longer than the machine deadline");
    let companion = client.companion_ref();
    let answer = ask(
        &mut client,
        WirePayload::HistoryRequest(cmds::history_request(&companion, None, 1)),
        "post-approval history",
    )
    .await
    .expect("the approved connection must answer");
    assert!(
        matches!(answer, WirePayload::HistoryResponse(_)),
        "got {answer:?}"
    );
    stop.send_replace(true);
    let joined = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("the listener must stop")
        .expect("the listener must not panic");
    joined.expect("the listener must shut down cleanly");
}

#[tokio::test]
async fn an_idle_client_keeps_one_connection_alive_past_the_liveness_window() {
    let dir = tempfile::tempdir().expect("temp dir");
    let handle = open_host(dir.path()).await;
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(conn::run_until_shutdown(
        dir.path().to_path_buf(),
        Arc::clone(&handle),
        transport,
        shutdown,
    ));
    let pending = dial_until_pending(dir.path())
        .await
        .expect("the first pairing must pend");
    let approved = handle
        .approve_device(pending.pending_id())
        .await
        .expect("approval must succeed");
    assert!(approved.is_some(), "approval must pair");
    let mut client = pending
        .complete()
        .await
        .expect("provision must authenticate");
    // Idle far past the host's liveness window: only the clock moves, and
    // the transport's background pump keeps answering the Host's pings.
    tokio::time::pause();
    for _ in 0..12 {
        tokio::time::advance(Duration::from_secs(10)).await;
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }
    tokio::time::resume();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let companion = client.companion_ref();
    let answer = ask(
        &mut client,
        WirePayload::HistoryRequest(cmds::history_request(&companion, None, 1)),
        "history after idle",
    )
    .await
    .expect("the same connection must still answer after more than the liveness window of idling");
    assert!(
        matches!(answer, WirePayload::HistoryResponse(_)),
        "got {answer:?}"
    );
    stop.send_replace(true);
    let joined = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("the listener must stop")
        .expect("the listener must not panic");
    joined.expect("the listener must shut down cleanly");
}

#[tokio::test]
async fn pending_pairings_are_capped_with_a_typed_denial() {
    let dir = tempfile::tempdir().expect("temp dir");
    let handle = open_host(dir.path()).await;
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(conn::run_until_shutdown(
        dir.path().to_path_buf(),
        Arc::clone(&handle),
        transport,
        shutdown,
    ));
    let mut held: Vec<WssClient> = Vec::new();
    let mut denied = false;
    for index in 0..=MIRRORED_MAX_PENDING_PAIRINGS {
        let mut client = raw_dial(dir.path()).await;
        let mut pairing = WireFrame {
            envelope: crafted_envelope("PairingRequest"),
            payload: WirePayload::PairingRequest(PairingRequest {
                device_descriptor: format!("pending-{index}"),
            }),
        };
        pairing.envelope.correlation.request_id = Some(RequestWireId(uuid::Uuid::new_v4()));
        client.send_wire(&pairing).await;
        let reply = client.recv_wire().await;
        match reply.payload {
            WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation { .. }) => {
                assert!(
                    !denied,
                    "the cap must only refuse once the limit is reached"
                );
                held.push(client);
            }
            WirePayload::PairingResult(PairingResult::Denied { reason }) => {
                assert_eq!(
                    index, MIRRORED_MAX_PENDING_PAIRINGS,
                    "only the connection past the cap is refused"
                );
                assert!(
                    reason.contains("too many pending pairings"),
                    "the refusal names the bound: {reason}"
                );
                denied = true;
            }
            other => panic!(
                "pairing must answer pending or denied, got {}",
                other.message_type()
            ),
        }
    }
    assert!(
        denied,
        "the {MIRRORED_MAX_PENDING_PAIRINGS}th pending pairing must be refused"
    );
    drop(held);
    stop.send_replace(true);
    let joined = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("the listener must stop")
        .expect("the listener must not panic");
    joined.expect("the listener must shut down cleanly");
}

// Mirrors `crate::serve::STREAM_BUFFER_FRAMES`; integration tests cannot
// name pub(crate) items, so a drift breaks these tests instead of silently
// weakening production.
const MIRRORED_STREAM_BUFFER_FRAMES: usize = 32;
// Mirrors `crate::conn::READ_AHEAD_FRAMES`: the reader's bounded window on
// top of the business queue. Queue + read-ahead together are the application
// capacity; one frame past that boundary must fail the connection.
const MIRRORED_READ_AHEAD_FRAMES: usize = 8;

/// Boots a serving Host with one completed summon round, arms the scripted
/// barrier on the next dialogue, and returns once that dialogue's provider
/// call is parked inside the connection's business handler.
async fn serve_and_park_dialogue(
    dir: PathBuf,
    transport: Arc<ScriptedTransport>,
    summon_text: &str,
    park_text: &str,
) -> Served {
    let mut served = serve_and_setup(dir, transport.clone(), &[cmds::CAPABILITY_DIALOGUE]).await;
    // The summon round intentionally skips the presentation confirmation: a
    // receipt would arm the connection's receipt timer, and this helper's
    // callers drive the liveness window through the tokio clock.
    let (_round, _stream, _reply) = send_round(served.client(), summon_text)
        .await
        .expect("the summon round must complete");
    wait_presence(&served.dir, "present").await;

    transport.block_input(on_latest_owner(park_text));
    let client = served.client();
    let companion = client.companion_ref();
    let target = client.round_target();
    let intake = ask(
        client,
        WirePayload::SubmitTextInput(cmds::submit_input(
            &companion,
            target,
            String::from(park_text),
            String::from("en"),
        )),
        "park submit",
    )
    .await
    .expect("the park submit must be answered while the intake precedes the provider call");
    let WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { round: _ }) =
        intake
    else {
        panic!("the park submit must be accepted, got {intake:?}");
    };
    transport.wait_parked(1).await;
    served
}

/// One frame more than the mirrored queue capacity: with the business handler
/// parked this leaves the business frame queue full and one frame in the
/// read-ahead window — below the queue + read-ahead overload boundary.
async fn saturate_the_application_queue(served: &mut Served) {
    notify_some(served, MIRRORED_STREAM_BUFFER_FRAMES + 1).await;
}

/// Exactly the mirrored application capacity — business queue plus the full
/// read-ahead window — with the business handler parked. The connection is
/// saturated but still at capacity, not over it.
async fn saturate_queue_and_read_ahead(served: &mut Served) {
    notify_some(
        served,
        MIRRORED_STREAM_BUFFER_FRAMES + MIRRORED_READ_AHEAD_FRAMES,
    )
    .await;
}

async fn notify_some(served: &mut Served, count: usize) {
    for _ in 0..count {
        tokio::time::timeout(
            Duration::from_secs(5),
            served
                .client()
                .notify(WirePayload::HistoryRequest(cmds::history_request(
                    "default", None, 1,
                ))),
        )
        .await
        .expect("the saturating write must be accepted by the client")
        .expect("the saturating write must reach the transport");
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
}

/// Moves only the clock across more than the liveness window while every
/// socket operation stays on the real clock.
async fn cross_the_liveness_window() {
    // Let in-flight socket work settle on the real clock before the clock
    // freezes, so the window measures liveness instead of setup races.
    tokio::time::sleep(Duration::from_millis(200)).await;
    tokio::time::pause();
    for _ in 0..12 {
        tokio::time::advance(Duration::from_secs(10)).await;
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
    }
    tokio::time::resume();
    tokio::time::sleep(Duration::from_millis(200)).await;
}

#[tokio::test]
async fn a_business_handler_over_the_liveness_window_keeps_ping_and_the_connection_alive() {
    let dir = tempfile::tempdir().expect("temp dir");
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let mut served = serve_and_park_dialogue(
        dir.path().to_path_buf(),
        Arc::clone(&transport),
        "summon for liveness",
        "park beyond the liveness window",
    )
    .await;

    // The business handler now runs past the liveness window while the client
    // only answers the Host's pings: those pings must keep flowing, and the
    // pongs must keep the responsive connection live.
    cross_the_liveness_window().await;
    assert!(
        matches!(
            presence_row(dir.path()),
            Some((ref state, _)) if state == "present"
        ),
        "a responsive client must not be liveness-closed while its handler is parked"
    );

    // Completing the parked handler lets the Host re-check liveness against
    // activity that stopped at the park: without transport pings inside the
    // window this would be over the liveness window already.
    transport.release_blocked();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        matches!(
            presence_row(dir.path()),
            Some((ref state, _)) if state == "present"
        ),
        "the liveness re-check after the parked handler must not close a client that kept answering"
    );
    let companion = served.client().companion_ref();
    let answer = ask(
        served.client(),
        WirePayload::HistoryRequest(cmds::history_request(&companion, None, 1)),
        "history after the parked handler",
    )
    .await
    .expect("the same connection must still answer past the liveness window");
    assert!(
        matches!(answer, WirePayload::HistoryResponse(_)),
        "got {answer:?}"
    );
    served.stop().await;
}

#[tokio::test]
async fn client_close_during_a_parked_handler_closes_promptly_and_never_returns_to_current() {
    let dir = tempfile::tempdir().expect("temp dir");
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_latest_owner("summon for close"),
                Call::text("SUMMON-REPLY"),
            ),
            (
                on_latest_owner("reconnect round"),
                Call::text("RECONNECT-REPLY"),
            ),
        ],
        &[],
    ));
    let mut served = serve_and_park_dialogue(
        dir.path().to_path_buf(),
        Arc::clone(&transport),
        "summon for close",
        "park before the client leaves",
    )
    .await;

    // The client leaves while its business operation is still parked: the
    // connection must close promptly, not when the operation finishes.
    drop(served.client.take());
    let (_, fallen_back) = wait_presence(dir.path(), "no_active").await;

    // The old connection no longer passes current admission: a fresh
    // connection authenticates and serves while the old handler is parked.
    let mut replacement = connect(dir.path()).await;
    assert_eq!(
        presence_row(dir.path()),
        Some((String::from("no_active"), fallen_back)),
        "authentication alone must not restore attribution"
    );
    let (round, stream, text) = send_round(&mut replacement, "reconnect round")
        .await
        .expect("the replacement connection must serve");
    assert_eq!(text, "RECONNECT-REPLY", "the new round must stream");
    confirm_round(&mut replacement, &round, stream).await;
    let (_, live_generation) = wait_presence(dir.path(), "present").await;

    // Completing the old operation must not be adopted as current work.
    transport.release_blocked();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let texts = history_texts(&mut replacement).await;
    assert!(
        !texts.iter().any(|text| text == "acknowledged"),
        "the closed connection's provider reply must not be committed: {texts:?}"
    );
    assert_eq!(
        presence_row(dir.path()),
        Some((String::from("present"), live_generation)),
        "the old connection's completion must not move presence"
    );
    drop(replacement);
    served.stop().await;
}

#[tokio::test]
async fn a_saturated_application_queue_still_moves_pongs_and_close() {
    let dir = tempfile::tempdir().expect("temp dir");
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let mut served = serve_and_park_dialogue(
        dir.path().to_path_buf(),
        Arc::clone(&transport),
        "summon for saturation",
        "park under saturation",
    )
    .await;
    saturate_the_application_queue(&mut served).await;

    // Past the liveness window the Host still pings and still reads the pongs
    // behind the saturated application queue: the connection stays present.
    cross_the_liveness_window().await;
    assert!(
        matches!(
            presence_row(dir.path()),
            Some((ref state, _)) if state == "present"
        ),
        "a saturated queue must not stop the pong reads that prove liveness"
    );

    // The reader also keeps processing EOF behind the saturated queue.
    drop(served.client.take());
    let (_, fallen_back) = wait_presence(dir.path(), "no_active").await;

    transport.release_blocked();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        presence_row(dir.path()),
        Some((String::from("no_active"), fallen_back)),
        "the closed connection's completion must not resurrect presence"
    );
    let mut replacement = connect(dir.path()).await;
    let (round, stream, _text) =
        send_round(&mut replacement, "after the saturated connection closed")
            .await
            .expect("the replacement connection must serve after saturation closed the old one");
    confirm_round(&mut replacement, &round, stream).await;
    drop(replacement);
    served.stop().await;
}

#[tokio::test]
async fn an_unparked_burst_dispatches_before_held_overflow_and_answers_ping() {
    let dir = tempfile::tempdir().expect("temp dir");
    let input = "unparked burst input";
    let transport = Arc::new(ScriptedTransport::new(
        vec![(
            on_latest_owner(input),
            Call::stream(
                "BURST-STREAM-REPLY",
                (0..8).map(|index| format!("burst-{index} ")).collect(),
                Duration::from_millis(20),
            ),
        )],
        &[],
    ));
    let mut served = serve_and_setup(
        dir.path().to_path_buf(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE],
    )
    .await;
    let companion = served.client().companion_ref();
    let target = served.client().round_target();
    let first = served
        .client()
        .prepare(WirePayload::SubmitTextInput(cmds::submit_input(
            &companion,
            target,
            String::from(input),
            String::from("en"),
        )));
    served
        .client()
        .send_prepared_for_tests(&first)
        .await
        .expect("the first application frame must enter the transport");

    // Keep the application burst ahead of the first business operation. The
    // provider is actively streaming, not parked; the old select order could
    // nevertheless keep pulling frames into `held` before dispatching it.
    let burst = MIRRORED_STREAM_BUFFER_FRAMES + MIRRORED_READ_AHEAD_FRAMES - 1;
    let ping_payload = b"unparked-burst-ping";
    let probe = served.client().transport_probe_for_tests();
    for index in 0..burst {
        if index == 8 {
            served.client().send_transport_ping_for_tests(ping_payload);
        }
        let request = served
            .client()
            .prepare(WirePayload::HistoryRequest(cmds::history_request(
                &companion, None, 1,
            )));
        served
            .client()
            .send_prepared_for_tests(&request)
            .await
            .expect("every bounded application burst frame must be accepted");
    }

    tokio::time::timeout(Duration::from_secs(5), async {
        while transport.sends() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the first business operation must dispatch before held overflow");
    assert!(
        transport.sends() >= 1,
        "the burst must not close the connection before business dispatch"
    );

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if probe.take_pongs().iter().any(|pong| pong == ping_payload) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the Host must answer the burst's Client Ping with a bounded Pong");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let WirePayload::TextStreamClose(close) = served
                .client()
                .next_frame()
                .await
                .expect("the connection must remain usable after the burst")
            {
                assert_eq!(
                    close.status,
                    ene_api::v1::round::StreamClose::Completed,
                    "the first burst operation must complete normally"
                );
                break;
            }
        }
    })
    .await
    .expect("the active business stream must finish after the burst");

    let answer = ask(
        served.client(),
        WirePayload::HistoryRequest(cmds::history_request(&companion, None, 1)),
        "history after the unparked burst",
    )
    .await
    .expect("the same connection must answer a later request after the burst");
    assert!(
        matches!(answer, WirePayload::HistoryResponse(_)),
        "got {answer:?}"
    );
    served.stop().await;
}

#[tokio::test]
async fn an_application_frame_past_the_read_ahead_window_fails_the_connection() {
    let dir = tempfile::tempdir().expect("temp dir");
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let mut served = serve_and_park_dialogue(
        dir.path().to_path_buf(),
        Arc::clone(&transport),
        "summon for the overload",
        "park under the overload",
    )
    .await;

    // Saturation, not queue-full alone: the business queue and the whole
    // read-ahead window are both full. At exactly this capacity the control
    // plane still moves — the Host's pings reach the client and the pongs
    // come back past the liveness window.
    saturate_queue_and_read_ahead(&mut served).await;
    cross_the_liveness_window().await;
    assert!(
        matches!(
            presence_row(dir.path()),
            Some((ref state, _)) if state == "present"
        ),
        "a fully saturated application backlog must not stop the pong reads that prove liveness"
    );

    // One application frame past that boundary is the explicit bounded
    // failure: the connection ends instead of waiting for the parked handler
    // or dropping the frame in silence. The write itself may already be
    // refused by the closing Host; either outcome is the bounded failure.
    let over_capacity = tokio::time::timeout(
        Duration::from_secs(5),
        served
            .client()
            .notify(WirePayload::HistoryRequest(cmds::history_request(
                "default", None, 1,
            ))),
    )
    .await;
    drop(over_capacity);
    let (_, fallen_back) = wait_presence(dir.path(), "no_active").await;
    assert_eq!(
        presence_row(dir.path()),
        Some((String::from("no_active"), fallen_back)),
        "the over-capacity frame must close the connection's currentness"
    );

    // The serving composition does not hang: while the old handler is still
    // parked, a replacement connection is accepted and serves normally.
    let mut replacement = connect(dir.path()).await;
    let (round, stream, _text) =
        send_round(&mut replacement, "after the overloaded connection closed")
            .await
            .expect("the replacement must serve after the overload closed the old connection");
    confirm_round(&mut replacement, &round, stream).await;
    drop(replacement);

    transport.release_blocked();
    tokio::time::sleep(Duration::from_millis(300)).await;
    served.stop().await;
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
fn db_scalar(db: &Path, sql: &str) -> u64 {
    let conn = rusqlite::Connection::open(db).expect("the store file must open");
    let counted: i64 = conn
        .query_row(sql, [], |row| row.get(0))
        .expect("the store query must answer");
    u64::try_from(counted).expect("a row count never goes negative")
}

async fn prepare_shutdown_task(dir: PathBuf, transport: Arc<ScriptedTransport>) -> Served {
    let mut served = serve_and_setup(dir.clone(), transport, &[cmds::CAPABILITY_DIALOGUE]).await;
    let workspace = dir.join("shutdown-task-workspace");
    std::fs::create_dir_all(&workspace).expect("the workspace directory creates");
    select_workspace(served.client(), &workspace)
        .await
        .expect("workspace must select");
    served
}

async fn propose_shutdown_task(served: &mut Served) {
    let (round, stream, reply) = send_round(served.client(), SHUTDOWN_TASK_OWNER)
        .await
        .expect("the proposal round must complete");
    assert!(reply.contains("Task accepted"), "{reply}");
    confirm_round(served.client(), &round, stream).await;
}

async fn serve_and_start_shutdown_task(dir: PathBuf, transport: Arc<ScriptedTransport>) -> Served {
    let mut served = prepare_shutdown_task(dir, transport).await;
    propose_shutdown_task(&mut served).await;
    served
}

#[cfg(feature = "test-support")]
#[tokio::test]
async fn supported_host_runtime_action_smoke_uses_the_packaged_worker() {
    let temp = tempfile::TempDir::new().unwrap();
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_latest_owner(SHUTDOWN_TASK_OWNER),
                Call::reported(
                    task_reply(serde_json::json!({
                        "kind": "propose_task",
                        "purpose": SHUTDOWN_TASK_PURPOSE,
                    })),
                    100,
                    0,
                    10,
                ),
            ),
            (
                on_task_agent_turn(0),
                Call::reported(
                    r##"{"tool":"create","path":"smoke.md","content":"worker smoke"}"##,
                    100,
                    0,
                    10,
                ),
            ),
            (
                on_task_agent_turn(1),
                Call::reported(r##"{"tool":"read","path":"smoke.md"}"##, 100, 0, 10),
            ),
            (
                on_task_agent_turn(2),
                Call::reported(r#"{"final":"worker smoke complete"}"#, 100, 0, 10),
            ),
        ],
        &[],
    ));
    transport.block_input(on_task_agent_turn(2));
    let mut served =
        serve_and_start_shutdown_task(temp.path().to_path_buf(), Arc::clone(&transport)).await;
    transport.wait_parked(1).await;
    transport.release_blocked();
    tokio::time::timeout(Duration::from_secs(10), async {
        while served.handle().running_task_executions_for_tests() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the supported Host path must join the completed Task Agent");
    assert_eq!(
        std::fs::read(temp.path().join("shutdown-task-workspace/smoke.md"))
            .expect("the worker must persist the create effect"),
        b"worker smoke"
    );
    assert_eq!(
        db_scalar(
            &temp.path().join("app.db"),
            "SELECT COUNT(*) FROM action_attempt WHERE certainty = 'confirmed_success'",
        ),
        2,
        "create and read must both retain observed certainty",
    );
    served.stop().await;
}

#[cfg(feature = "test-support")]
#[tokio::test]
async fn missing_worker_fails_closed_before_action_claim() {
    let temp = tempfile::TempDir::new().unwrap();
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_latest_owner(SHUTDOWN_TASK_OWNER),
                Call::reported(
                    task_reply(serde_json::json!({
                        "kind": "propose_task",
                        "purpose": SHUTDOWN_TASK_PURPOSE,
                    })),
                    100,
                    0,
                    10,
                ),
            ),
            (
                on_task_agent_turn(0),
                Call::reported(
                    r##"{"tool":"create","path":"missing-worker.md","content":"blocked"}"##,
                    100,
                    0,
                    10,
                ),
            ),
        ],
        &[],
    ));
    let mut served = prepare_shutdown_task(temp.path().to_path_buf(), Arc::clone(&transport)).await;
    served
        .handle()
        .install_uncooperative_task_effect_runtime_for_tests(
            temp.path().join("missing-ene-action-worker"),
            Vec::new(),
            Vec::new(),
        );
    propose_shutdown_task(&mut served).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while served.handle().running_task_executions_for_tests() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the missing worker must fail the Task Agent promptly");
    assert_eq!(
        db_scalar(
            &temp.path().join("app.db"),
            "SELECT COUNT(*) FROM action_attempt",
        ),
        0,
        "worker preflight must fail before AU5",
    );
    assert!(
        !temp
            .path()
            .join("shutdown-task-workspace/missing-worker.md")
            .exists()
    );
    served.stop().await;
}

#[tokio::test]
async fn graceful_shutdown_aborts_a_parked_task_agent_preserves_unknown_and_does_not_replay() {
    let temp = tempfile::TempDir::new().unwrap();
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_latest_owner(SHUTDOWN_TASK_OWNER),
                Call::reported(
                    task_reply(serde_json::json!({
                        "kind": "propose_task",
                        "purpose": SHUTDOWN_TASK_PURPOSE,
                    })),
                    100,
                    0,
                    10,
                ),
            ),
            (
                on_task_agent_turn(0),
                Call::reported(
                    r##"{"tool":"create","path":"report.md","content":"shutdown report"}"##,
                    100,
                    0,
                    10,
                ),
            ),
            (
                on_task_agent_turn(1),
                Call::reported(r#"{"final":"done"}"#, 100, 0, 10),
            ),
        ],
        &[],
    ));
    transport.block_input(on_task_agent_turn(1));
    let mut served =
        serve_and_start_shutdown_task(temp.path().to_path_buf(), Arc::clone(&transport)).await;
    transport.wait_parked(1).await;
    let tasks = list_tasks(served.client())
        .await
        .expect("the active task must list");
    assert_eq!(tasks.tasks.len(), 1);
    assert_eq!(tasks.tasks[0].progress, "in_progress");
    assert!(tasks.tasks[0].running);

    served.request_graceful_stop();
    assert!(
        served.join_listener_within(Duration::from_secs(30)).await,
        "the Task Agent cooperative abort must bound its parked provider call"
    );
    assert_eq!(
        transport.parked_count(),
        1,
        "the provider fixture was never released"
    );
    wait_until_deletion_drivers(served.handle(), 0).await;
    wait_until_deletion_blocking(served.handle(), 0).await;

    let db = temp.path().join("app.db");
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM inference_attempt WHERE consumer = 'task_agent'",
        ),
        2,
        "the completed action turn and the parked final turn each claimed once"
    );
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM usage_fact f JOIN inference_attempt a ON f.ticket = a.ticket \
             WHERE a.consumer = 'task_agent' AND f.source = 'unknown'",
        ),
        1,
        "the post-claim Host abort records Unknown usage"
    );
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM action_attempt WHERE certainty = 'confirmed_success'",
        ),
        1,
        "an effect completed before Host shutdown keeps its confirmed fact"
    );
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM task WHERE progress = 'in_progress'",
        ),
        1,
        "Host shutdown is not a durable Task cancellation"
    );
    assert_eq!(
        db_scalar(&db, "SELECT COUNT(*) FROM task_result"),
        0,
        "the interrupted execution cannot finalize from shutdown"
    );

    let sends_after_shutdown = transport.sends();
    let mut client = served.restart().await;
    let restarted = list_tasks(&mut client)
        .await
        .expect("the interrupted task must remain listed after restart");
    assert_eq!(restarted.tasks.len(), 1);
    assert_eq!(restarted.tasks[0].progress, "in_progress");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        transport.sends(),
        sends_after_shutdown,
        "restart must not replay an already-started Task Agent execution"
    );
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM inference_attempt WHERE consumer = 'task_agent'",
        ),
        2,
        "restart must not claim another inference attempt"
    );
    drop(client);
    served.stop().await;
}

#[cfg(feature = "test-support")]
#[tokio::test]
async fn host_shutdown_kills_reaps_and_joins_uncooperative_task_effect_before_returning() {
    let temp = tempfile::TempDir::new().unwrap();
    let entered = temp.path().join("effect-entered");
    let release = temp.path().join("effect-release");
    let mutation = temp.path().join("effect-late-mutation");
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_latest_owner(SHUTDOWN_TASK_OWNER),
                Call::reported(
                    task_reply(serde_json::json!({
                        "kind": "propose_task",
                        "purpose": SHUTDOWN_TASK_PURPOSE,
                    })),
                    100,
                    0,
                    10,
                ),
            ),
            (
                on_task_agent_turn(0),
                Call::reported(
                    r##"{"tool":"create","path":"uncooperative.md","content":"late"}"##,
                    100,
                    0,
                    10,
                ),
            ),
        ],
        &[],
    ));
    let mut served = prepare_shutdown_task(temp.path().to_path_buf(), Arc::clone(&transport)).await;
    let executable = std::env::current_exe().expect("the integration-test executable path");
    served
        .handle()
        .install_uncooperative_task_effect_runtime_for_tests(
            executable,
            vec![
                String::from("--ignored"),
                String::from("--exact"),
                String::from("uncooperative_task_effect_worker_fixture"),
                String::from("--nocapture"),
            ],
            vec![
                (
                    String::from("ENE_TEST_EFFECT_ENTERED"),
                    entered.to_string_lossy().into_owned(),
                ),
                (
                    String::from("ENE_TEST_EFFECT_RELEASE"),
                    release.to_string_lossy().into_owned(),
                ),
                (
                    String::from("ENE_TEST_EFFECT_MUTATION"),
                    mutation.to_string_lossy().into_owned(),
                ),
            ],
        );
    served
        .handle()
        .set_task_agent_quiesce_timeout_for_tests(Duration::from_millis(100));
    propose_shutdown_task(&mut served).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !entered.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the Task Agent must enter uncooperative external-effect work");
    let db = temp.path().join("app.db");
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM action_attempt WHERE certainty = 'unknown'",
        ),
        1
    );
    assert_eq!(served.handle().live_task_effect_workers_for_tests(), 1);
    assert_eq!(served.handle().running_task_executions_for_tests(), 1);
    let sends_before_shutdown = transport.sends();

    served.request_graceful_stop();
    let finished = tokio::time::timeout(Duration::from_secs(5), &mut served.server)
        .await
        .expect("Host shutdown must remain bounded after the escalation");
    served.server = tokio::spawn(async { Ok::<(), CoreError>(()) });
    assert!(matches!(
        finished,
        Ok(Err(CoreError::Serving(reason))) if reason.contains("quiesce exceeded")
    ));
    wait_until_deletion_drivers(served.handle(), 0).await;
    wait_until_deletion_blocking(served.handle(), 0).await;

    assert_eq!(served.handle().live_task_effect_workers_for_tests(), 0);
    assert_eq!(served.handle().running_task_executions_for_tests(), 0);
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM action_attempt WHERE certainty = 'unknown'",
        ),
        1,
        "the killed effect keeps AU5 Unknown"
    );
    assert_eq!(
        db_scalar(&db, "SELECT COUNT(*) FROM task_agent_observation"),
        0,
        "the stopped runner records no later observation mutation"
    );
    assert_eq!(transport.sends(), sends_before_shutdown);
    assert!(
        !temp
            .path()
            .join("shutdown-task-workspace/uncooperative.md")
            .exists()
    );
    std::fs::write(&release, b"released").unwrap();
    assert!(
        !mutation.exists(),
        "the reaped worker cannot mutate after Host shutdown returned"
    );
}

#[cfg(feature = "test-support")]
#[tokio::test]
async fn shutdown_before_task_agent_inference_claim_creates_no_attempt_or_provider_io() {
    let temp = tempfile::TempDir::new().unwrap();
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_latest_owner(SHUTDOWN_TASK_OWNER),
                Call::reported(
                    task_reply(serde_json::json!({
                        "kind": "propose_task",
                        "purpose": SHUTDOWN_TASK_PURPOSE,
                    })),
                    100,
                    0,
                    10,
                ),
            ),
            (
                on_task_agent_turn(0),
                Call::reported(r#"{"final":"done"}"#, 100, 0, 10),
            ),
        ],
        &[],
    ));
    let mut served = prepare_shutdown_task(temp.path().to_path_buf(), Arc::clone(&transport)).await;
    served.handle().arm_inference_claim_pause_for_tests();
    propose_shutdown_task(&mut served).await;
    tokio::time::timeout(
        Duration::from_secs(5),
        served.handle().wait_task_claim_pause_for_tests(),
    )
    .await
    .expect("the inference claim must reach the pre-claim barrier");
    let sends_before_shutdown = transport.sends();

    served.request_graceful_stop();
    tokio::time::timeout(
        Duration::from_secs(5),
        served.handle().wait_task_host_shutdown_for_tests(),
    )
    .await
    .expect("Host shutdown must linearize while the claim is paused");
    served.handle().release_task_claim_pause_for_tests();
    assert!(
        served.join_listener_within(Duration::from_secs(30)).await,
        "the shutdown-first claim must quiesce"
    );

    let db = temp.path().join("app.db");
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM inference_attempt WHERE consumer = 'task_agent'",
        ),
        0
    );
    assert_eq!(transport.sends(), sends_before_shutdown);
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM usage_fact WHERE source = 'unknown'"
        ),
        0
    );
}

#[cfg(feature = "test-support")]
#[tokio::test]
async fn shutdown_before_task_agent_action_start_creates_no_attempt_or_workspace_effect() {
    let temp = tempfile::TempDir::new().unwrap();
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_latest_owner(SHUTDOWN_TASK_OWNER),
                Call::reported(
                    task_reply(serde_json::json!({
                        "kind": "propose_task",
                        "purpose": SHUTDOWN_TASK_PURPOSE,
                    })),
                    100,
                    0,
                    10,
                ),
            ),
            (
                on_task_agent_turn(0),
                Call::reported(
                    r##"{"tool":"create","path":"linearized.md","content":"blocked"}"##,
                    100,
                    0,
                    10,
                ),
            ),
        ],
        &[],
    ));
    let mut served = prepare_shutdown_task(temp.path().to_path_buf(), Arc::clone(&transport)).await;
    served.handle().arm_action_claim_pause_for_tests();
    propose_shutdown_task(&mut served).await;
    tokio::time::timeout(
        Duration::from_secs(5),
        served.handle().wait_task_claim_pause_for_tests(),
    )
    .await
    .expect("the Action start must reach the pre-claim barrier");
    let sends_before_shutdown = transport.sends();

    served.request_graceful_stop();
    tokio::time::timeout(
        Duration::from_secs(5),
        served.handle().wait_task_host_shutdown_for_tests(),
    )
    .await
    .expect("Host shutdown must linearize before AU5");
    served.handle().release_task_claim_pause_for_tests();
    assert!(served.join_listener_within(Duration::from_secs(30)).await);

    let db = temp.path().join("app.db");
    assert_eq!(
        db_scalar(&db, "SELECT COUNT(*) FROM action_attempt"),
        0,
        "shutdown-first must not insert AU5"
    );
    assert_eq!(transport.sends(), sends_before_shutdown);
    assert!(
        !temp
            .path()
            .join("shutdown-task-workspace/linearized.md")
            .exists(),
        "shutdown-first must not execute the workspace effect"
    );
}

#[cfg(feature = "test-support")]
#[tokio::test]
async fn action_claim_first_keeps_started_fact_and_completed_effect_across_shutdown() {
    let temp = tempfile::TempDir::new().unwrap();
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_latest_owner(SHUTDOWN_TASK_OWNER),
                Call::reported(
                    task_reply(serde_json::json!({
                        "kind": "propose_task",
                        "purpose": SHUTDOWN_TASK_PURPOSE,
                    })),
                    100,
                    0,
                    10,
                ),
            ),
            (
                on_task_agent_turn(0),
                Call::reported(
                    r##"{"tool":"create","path":"claimed-first.md","content":"finished"}"##,
                    100,
                    0,
                    10,
                ),
            ),
            (
                on_task_agent_turn(1),
                Call::reported(r#"{"final":"done"}"#, 100, 0, 10),
            ),
        ],
        &[],
    ));
    let mut served = prepare_shutdown_task(temp.path().to_path_buf(), Arc::clone(&transport)).await;
    served.handle().arm_action_effect_pause_for_tests();
    propose_shutdown_task(&mut served).await;
    tokio::time::timeout(
        Duration::from_secs(5),
        served.handle().wait_task_claim_pause_for_tests(),
    )
    .await
    .expect("the claimed Action must reach the post-effect barrier");
    let db = temp.path().join("app.db");
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM action_attempt WHERE certainty = 'unknown'",
        ),
        1,
        "AU5 committed before the effect"
    );
    assert!(
        temp.path()
            .join("shutdown-task-workspace/claimed-first.md")
            .exists()
    );

    served.request_graceful_stop();
    tokio::time::timeout(
        Duration::from_secs(5),
        served.handle().wait_task_host_shutdown_for_tests(),
    )
    .await
    .expect("shutdown must follow the committed AU5");
    served.handle().release_task_claim_pause_for_tests();
    assert!(served.join_listener_within(Duration::from_secs(30)).await);

    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM action_attempt WHERE certainty = 'confirmed_success'",
        ),
        1,
        "the observed effect must settle its started attempt"
    );
    assert_eq!(db_scalar(&db, "SELECT COUNT(*) FROM task_result"), 0);
}

#[tokio::test]
async fn task_cancel_racing_host_shutdown_keeps_the_durable_cancel_distinct() {
    let temp = tempfile::TempDir::new().unwrap();
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_latest_owner(SHUTDOWN_TASK_OWNER),
                Call::reported(
                    task_reply(serde_json::json!({
                        "kind": "propose_task",
                        "purpose": SHUTDOWN_TASK_PURPOSE,
                    })),
                    100,
                    0,
                    10,
                ),
            ),
            (
                on_task_agent_turn(0),
                Call::reported(r#"{"final":"done"}"#, 100, 0, 10),
            ),
        ],
        &[],
    ));
    transport.block_input(on_task_agent_turn(0));
    let mut served =
        serve_and_start_shutdown_task(temp.path().to_path_buf(), Arc::clone(&transport)).await;
    transport.wait_parked(1).await;
    let tasks = list_tasks(served.client())
        .await
        .expect("the active task must list");
    assert_eq!(tasks.tasks.len(), 1);
    let db = temp.path().join("app.db");
    let conn = rusqlite::Connection::open(&db).expect("the state database opens");
    let stored_task_id: String = conn
        .query_row("SELECT task_id FROM task", [], |row| row.get(0))
        .expect("the active task id must exist");
    drop(conn);
    let task_id =
        uuid::Uuid::parse_str(&stored_task_id).expect("the stored task id must be a UUID");
    let task = TaskId::from_raw(RawId::from_uuid(task_id));
    let handle = served.handle_arc();
    let cancel = tokio::spawn(async move { handle.cancel_task(CancelTaskCommand { task }).await });

    served.request_graceful_stop();
    let cancel_outcome = cancel.await.expect("the cancel task must join");
    assert_eq!(
        cancel_outcome,
        Ok(TaskCancelOutcome::CancelAccepted),
        "the durable cancel outcome must be reported"
    );
    assert!(
        served.join_listener_within(Duration::from_secs(30)).await,
        "cancel and Host shutdown must both quiesce the parked task"
    );
    wait_until_deletion_drivers(served.handle(), 0).await;
    wait_until_deletion_blocking(served.handle(), 0).await;

    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM task WHERE progress = 'cancelled'",
        ),
        1,
        "the durable cancel remains the Task lifecycle outcome"
    );
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM usage_fact f JOIN inference_attempt a ON f.ticket = a.ticket \
             WHERE a.consumer = 'task_agent' AND f.source = 'unknown'",
        ),
        1,
        "the claimed cancelled attempt keeps Unknown usage"
    );
    assert_eq!(
        db_scalar(&db, "SELECT COUNT(*) FROM task_result"),
        0,
        "neither race path fabricates a final result"
    );
}

#[tokio::test]
async fn graceful_shutdown_aborts_a_parked_dialogue_dispatch_and_keeps_the_unknown_fact() {
    let dir = tempfile::tempdir().expect("temp dir");
    let transport = Arc::new(ScriptedTransport::new(
        vec![(
            on_latest_owner("summon before shutdown"),
            // Reported usage for the completed summon, so the only unknown
            // usage fact in this test is the aborted post-claim attempt.
            Call::reported("SUMMON-REPLY", 120, 0, 30),
        )],
        &[],
    ));
    let mut served = serve_and_park_dialogue(
        dir.path().to_path_buf(),
        Arc::clone(&transport),
        "summon before shutdown",
        "park across the shutdown",
    )
    .await;
    // The barrier is the dispatch's own provider call: the attempt claim is
    // durably `Started` before the fixture can park, so the shutdown below
    // aborts a post-claim dispatch.
    transport.wait_parked(1).await;
    assert_eq!(transport.parked_count(), 1);

    served.request_graceful_stop();
    // Concurrency Control shutdown owns the running dispatch: without ever
    // releasing the provider fixture, the Host-lifecycle cooperative abort
    // alone must let the shutdown finish — joining the business task inside
    // the Host lifecycle instead of detaching it.
    assert!(
        served.join_listener_within(Duration::from_secs(30)).await,
        "graceful shutdown must quiesce the parked dispatch through the cooperative abort"
    );
    assert_eq!(
        transport.parked_count(),
        1,
        "the provider fixture was never released; only the abort ended the wait"
    );
    wait_until_deletion_drivers(served.handle(), 0).await;
    wait_until_deletion_blocking(served.handle(), 0).await;

    // The post-claim durable outcome: exactly one attempt per dispatch, the
    // aborted claim keeps its unknown usage fact, the owner input stays
    // persisted, and no reply commits as a success.
    let db = dir.path().join("app.db");
    assert_eq!(
        db_scalar(&db, "SELECT COUNT(*) FROM inference_attempt"),
        2,
        "the summon and the aborted dispatch claim exactly one attempt each"
    );
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM usage_fact WHERE source = 'unknown'"
        ),
        1,
        "the aborted post-claim attempt records its unknown usage fact"
    );
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM history_message WHERE role = 'owner'"
        ),
        2,
        "both accepted owner inputs stay committed"
    );
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM history_message WHERE role = 'companion'"
        ),
        1,
        "only the completed summon commits a reply; the aborted dispatch never does"
    );

    // With the business task joined, nothing derived from it may still
    // mutate the store after the shutdown completed.
    let settled = served.canonical_remainder("park across the shutdown").await;
    assert!(settled > 0, "the accepted owner input must be persisted");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        served.canonical_remainder("park across the shutdown").await,
        settled,
        "no store mutation may appear from the joined business task after shutdown"
    );
}

#[cfg(feature = "test-support")]
#[tokio::test]
async fn graceful_shutdown_stops_admission_before_abort_and_joins_without_new_ingress() {
    let dir = tempfile::tempdir().expect("temp dir");
    let transport = Arc::new(ScriptedTransport::new(
        vec![(
            on_latest_owner("summon before the shutdown boundary"),
            Call::reported("SUMMON-BOUNDARY-REPLY", 10, 0, 1),
        )],
        &[],
    ));
    let mut served = serve_and_setup(
        dir.path().to_path_buf(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE],
    )
    .await;
    let (_round, _stream, _reply) =
        send_round(served.client(), "summon before the shutdown boundary")
            .await
            .expect("the setup round must complete");
    wait_presence(&served.dir, "present").await;
    let summary = fetch_summary(served.client(), "the boundary receipt")
        .await
        .expect("the setup round must produce a receipt");
    assert_eq!(served.handle().receipts_held_for_tests(), 1);

    let db = dir.path().join("app.db");
    let owner_rows_before = db_scalar(
        &db,
        "SELECT COUNT(*) FROM history_message WHERE role = 'owner'",
    );
    let attempts_before = db_scalar(&db, "SELECT COUNT(*) FROM inference_attempt");
    let task_rows_before = db_scalar(&db, "SELECT COUNT(*) FROM task");
    let sends_before = transport.sends();
    let inputs_before = transport.input_texts().len();

    served.handle().arm_shutdown_boundary_for_tests();
    served.request_graceful_stop();
    tokio::time::timeout(
        Duration::from_secs(5),
        served.handle().wait_shutdown_boundary_signal_for_tests(),
    )
    .await
    .expect("shutdown must publish the admission-stop signal");
    tokio::time::timeout(
        Duration::from_secs(5),
        served
            .handle()
            .wait_shutdown_boundary_connection_for_tests(),
    )
    .await
    .expect("the existing connection must observe the admission-stop signal");

    let companion = served.client().companion_ref();
    let target = served.client().round_target();
    let submit = tokio::time::timeout(
        Duration::from_secs(5),
        served
            .client()
            .notify(WirePayload::SubmitTextInput(cmds::submit_input(
                &companion,
                target,
                String::from("input after the shutdown boundary"),
                String::from("en"),
            ))),
    )
    .await;
    assert!(
        submit.is_ok(),
        "the post-boundary input attempt must be bounded, got {submit:?}"
    );
    let control = tokio::time::timeout(
        Duration::from_secs(5),
        served.client().notify(WirePayload::ConfirmPresentation(
            ene_api::v1::round::ConfirmPresentationWire {
                round: summary.round,
                stream: None,
                status: PresentationStatus::Presented,
                detail: None,
            },
        )),
    )
    .await;
    assert!(
        control.is_ok(),
        "the post-boundary control attempt must be bounded, got {control:?}"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM history_message WHERE role = 'owner'"
        ),
        owner_rows_before,
        "input after the admission boundary must not commit an owner row"
    );
    assert_eq!(
        db_scalar(&db, "SELECT COUNT(*) FROM inference_attempt"),
        attempts_before,
        "input after the admission boundary must not create an inference attempt"
    );
    assert_eq!(
        db_scalar(&db, "SELECT COUNT(*) FROM task"),
        task_rows_before,
        "control input after the admission boundary must not mutate task state"
    );
    assert_eq!(
        transport.sends(),
        sends_before,
        "input after the admission boundary must not reach the provider"
    );
    assert_eq!(
        transport.input_texts().len(),
        inputs_before,
        "input after the admission boundary must not add provider input"
    );
    assert_eq!(
        served.handle().receipts_held_for_tests(),
        1,
        "control input after the admission boundary must not settle the receipt"
    );

    served.handle().release_shutdown_boundary_for_tests();
    assert!(
        served.join_listener_within(Duration::from_secs(30)).await,
        "admission stop must precede abort and the handler join"
    );
    wait_until_deletion_drivers(served.handle(), 0).await;
    wait_until_deletion_blocking(served.handle(), 0).await;
    let settled = (
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM history_message WHERE role = 'owner'",
        ),
        db_scalar(&db, "SELECT COUNT(*) FROM inference_attempt"),
        db_scalar(&db, "SELECT COUNT(*) FROM task"),
        transport.sends(),
        transport.input_texts().len(),
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        settled,
        (
            db_scalar(
                &db,
                "SELECT COUNT(*) FROM history_message WHERE role = 'owner'"
            ),
            db_scalar(&db, "SELECT COUNT(*) FROM inference_attempt"),
            db_scalar(&db, "SELECT COUNT(*) FROM task"),
            transport.sends(),
            transport.input_texts().len(),
        ),
        "no store or provider mutation may appear after conn::run() returns"
    );
}

#[tokio::test]
async fn graceful_shutdown_aborts_a_parked_learning_dispatch_and_keeps_the_unknown_fact() {
    let temp = tempfile::tempdir().expect("temp dir");
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
    transport.block_input(on_learning_formation(true));
    let mut served = serve_and_setup(
        dir,
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE, cmds::CAPABILITY_LEARNING],
    )
    .await;
    let (_round, _stream, reply) = send_round(served.client(), &first)
        .await
        .expect("the first round must complete");
    assert!(reply.contains(TARGET));
    // The formation provider call runs inside the learning dispatch after its
    // claim, so parking it parks a post-claim attempt.
    transport.wait_parked(1).await;

    served.request_graceful_stop();
    assert!(
        served.join_listener_within(Duration::from_secs(30)).await,
        "the cooperative abort must bound a parked learning dispatch"
    );
    assert_eq!(
        transport.parked_count(),
        1,
        "the learning fixture was never released; only the abort ended the wait"
    );
    wait_until_deletion_drivers(served.handle(), 0).await;
    wait_until_deletion_blocking(served.handle(), 0).await;

    let db = temp.path().join("app.db");
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM inference_attempt WHERE consumer = 'companion_learning'"
        ),
        1,
        "the parked learning dispatch had claimed exactly one attempt"
    );
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM usage_fact f JOIN inference_attempt a ON f.ticket = a.ticket \
             WHERE f.source = 'unknown' AND a.consumer = 'companion_learning'"
        ),
        1,
        "the aborted learning claim keeps its unknown usage fact"
    );
    assert_eq!(
        db_scalar(&db, "SELECT COUNT(*) FROM learning_formation"),
        0,
        "the settled formation leaves no dangling in-flight claim"
    );
    assert!(
        !db_target_hits(&db, TARGET)
            .iter()
            .any(|hit| hit.starts_with("learning_memory.") || hit.starts_with("learning_summary.")),
        "an aborted learning result must never commit as success: {:?}",
        db_target_hits(&db, TARGET)
    );
    // The join above is the worker's lifecycle proof: the learning worker
    // ended inside the Host lifecycle, not outside it.
}

#[tokio::test]
async fn shutdown_before_the_learning_claim_creates_no_attempt_and_no_provider_io() {
    let temp = tempfile::tempdir().expect("temp dir");
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
    let mut served = serve_and_setup(
        dir,
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE, cmds::CAPABILITY_LEARNING],
    )
    .await;
    // The store park sits after the queue take and before the formation and
    // the claim: whatever continues afterwards is a genuine pre-claim abort.
    served
        .handle()
        .store_for_tests()
        .arm_learning_take_park_for_tests();
    let (_round, _stream, _reply) = send_round(served.client(), &first)
        .await
        .expect("the first round must complete");
    tokio::time::timeout(
        Duration::from_secs(30),
        served
            .handle()
            .store_for_tests()
            .wait_learning_take_park_for_tests(),
    )
    .await
    .expect("the learning worker must reach the pre-claim park");
    let sends_before = transport.sends();

    served.request_graceful_stop();
    // Stop first, then release: presence falls back only after the shutdown
    // has fired the cooperative abort, so the released worker observes the
    // stop before any claim or provider call.
    let (_, fallen_back) = wait_presence(served.dir.as_path(), "no_active").await;
    assert_eq!(
        presence_row(served.dir.as_path()),
        Some((String::from("no_active"), fallen_back)),
        "the shutdown must close currentness before the worker continues"
    );
    served
        .handle()
        .store_for_tests()
        .release_learning_take_park_for_tests();
    assert!(
        served.join_listener_within(Duration::from_secs(30)).await,
        "the pre-claim stop must let the shutdown finish"
    );
    wait_until_deletion_drivers(served.handle(), 0).await;
    wait_until_deletion_blocking(served.handle(), 0).await;

    let db = temp.path().join("app.db");
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM inference_attempt WHERE consumer = 'companion_learning'"
        ),
        0,
        "a stop before the claim never fabricates an attempt"
    );
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM usage_fact f JOIN inference_attempt a ON f.ticket = a.ticket \
             WHERE f.source = 'unknown' AND a.consumer = 'companion_learning'"
        ),
        0,
        "a never-claimed use records no usage fact"
    );
    assert_eq!(
        transport.sends(),
        sends_before,
        "no provider call may start after the stop won"
    );
    assert!(
        !transport
            .input_texts()
            .iter()
            .any(|input| input.contains("learning formation pass")),
        "a pre-claim abort must never reach the provider"
    );
    assert_eq!(
        db_scalar(&db, "SELECT COUNT(*) FROM learning_formation"),
        0,
        "the pre-claim stop settles the formation it never claimed"
    );
}

/// Continuous business streaming — not a parked provider — across more than
/// the receipt deadline: transport control, liveness, and the monotonic
/// deadline keep progressing the whole time, and the business stream itself
/// is neither starved by the control priority nor reordered.
#[tokio::test]
async fn continuous_business_streaming_keeps_transport_control_and_the_receipt_deadline_moving() {
    let dir = tempfile::tempdir().expect("temp dir");
    let stream_text = "stream across the receipt deadline";
    let chunk_script: Vec<String> = (0..2000)
        .map(|index| format!("[stream {index:05}]"))
        .collect();
    let expected_stream: String = chunk_script.concat();
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_latest_owner("summon before the stream"),
                Call::text("SUMMON-STREAM-REPLY"),
            ),
            (
                on_latest_owner(stream_text),
                // 2000 chunks × 20 ms ≈ 40 seconds of continuously
                // produced business frames — well past the 30-second
                // receipt deadline.
                Call::stream(
                    "STREAM-COMPLETE-REPLY",
                    chunk_script,
                    Duration::from_millis(20),
                ),
            ),
        ],
        &[],
    ));
    let mut served = serve_and_setup(
        dir.path().to_path_buf(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE],
    )
    .await;
    let (round, stream, _reply) = send_round(served.client(), "summon before the stream")
        .await
        .expect("the summon round must complete");
    wait_presence(&served.dir, "present").await;
    confirm_round(served.client(), &round, stream).await;

    // A receipt is issued on this connection and never acknowledged during
    // the stream.
    let stale = fetch_summary(served.client(), "the unacked receipt")
        .await
        .expect("the subscription must answer");
    assert!(
        !stale.receipt.0.is_empty(),
        "the fixture must issue a receipt"
    );
    assert_eq!(served.handle().receipts_held_for_tests(), 1);
    let receipt_issued_at = std::time::Instant::now();

    let companion = served.client().companion_ref();
    let target = served.client().round_target();
    let intake = ask(
        served.client(),
        WirePayload::SubmitTextInput(cmds::submit_input(
            &companion,
            target,
            String::from(stream_text),
            String::from("en"),
        )),
        "stream submit",
    )
    .await
    .expect("the stream submit must be answered while the intake precedes the provider call");
    assert!(
        matches!(
            intake,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
        ),
        "the stream submit must be accepted, got {intake:?}"
    );

    let probe = served.client().transport_probe_for_tests();
    let pings_at_stream_start = probe.host_pings();
    let pongs_at_stream_start = probe.pongs_sent();
    let started = std::time::Instant::now();

    let mut ping_sent_at: Option<std::time::Instant> = None;
    let mut pong_observed = false;
    let mut host_ping_observed = false;
    let mut receipt_expired = false;
    let mut checkpoint_at_15s: Option<usize> = None;
    let mut opened = false;
    let mut frames = 0usize;
    let mut previous_seq: Option<u64> = None;
    let mut text_out = String::new();
    let close_status = loop {
        match tokio::time::timeout(Duration::from_millis(500), served.client().next_frame()).await {
            Ok(Ok(WirePayload::TextStreamOpen(_))) => opened = true,
            Ok(Ok(WirePayload::TextStreamFrame(frame))) => {
                assert!(
                    previous_seq.is_none_or(|previous| frame.seq == previous + 1),
                    "stream frames must order by seq across the control traffic"
                );
                previous_seq = Some(frame.seq);
                text_out.push_str(&frame.delta);
                frames += 1;
            }
            Ok(Ok(WirePayload::TextStreamClose(close))) => break close.status,
            Ok(Ok(WirePayload::PresenceAttribution(_))) => {}
            Ok(Ok(other)) => panic!("unexpected payload during the stream: {other:?}"),
            Ok(Err(error)) => panic!("the stream must keep reading: {error:?}"),
            Err(_) => {}
        };
        let elapsed = started.elapsed();
        if ping_sent_at.is_none() && elapsed >= Duration::from_secs(3) {
            served
                .client()
                .send_transport_ping_for_tests(b"control-under-stream");
            ping_sent_at = Some(std::time::Instant::now());
        }
        if let Some(sent_at) = ping_sent_at
            && !pong_observed
        {
            if probe
                .take_pongs()
                .iter()
                .any(|pong| pong == b"control-under-stream")
            {
                pong_observed = true;
            } else {
                assert!(
                    sent_at.elapsed() < Duration::from_secs(3),
                    "the Host must answer a client ping with a bounded pong while business frames stream"
                );
            }
        }
        if !host_ping_observed && elapsed >= Duration::from_secs(16) {
            assert!(
                probe.host_pings() > pings_at_stream_start,
                "the Host must keep pinging while the business stream runs"
            );
            host_ping_observed = true;
        }
        if checkpoint_at_15s.is_none() && elapsed >= Duration::from_secs(15) {
            // The later checkpoint proves the stream continues; this first
            // checkpoint is only a liveness floor because Windows runners
            // can schedule the 20 ms fixture below its nominal rate.
            assert!(
                frames >= 50,
                "business frames must keep flowing across the window, got {frames}"
            );
            checkpoint_at_15s = Some(frames);
        }
        if let Some(at_15s) = checkpoint_at_15s
            && elapsed >= Duration::from_secs(25)
            && frames <= at_15s
        {
            panic!("the transport-control priority must not starve business delivery");
        }
        if !receipt_expired && receipt_issued_at.elapsed() >= Duration::from_secs(31) {
            assert_eq!(
                transport.streaming_count(),
                1,
                "the business stream must still be running at the receipt deadline"
            );
            assert_eq!(
                served.handle().receipts_held_for_tests(),
                0,
                "the receipt must expire at its monotonic 30-second deadline while business frames keep streaming"
            );
            assert!(
                matches!(presence_row(dir.path()), Some((ref state, _)) if state == "present"),
                "the streaming connection must stay current"
            );
            receipt_expired = true;
        }
    };

    assert!(opened, "the stream must open before it frames");
    assert!(
        receipt_expired,
        "the receipt deadline must be observed during the stream"
    );
    assert!(
        pong_observed,
        "the bounded client-ping pong must be observed during the stream"
    );
    assert!(
        host_ping_observed,
        "the Host ping must be observed during the stream"
    );
    assert_eq!(
        close_status,
        ene_api::v1::round::StreamClose::Completed,
        "the business stream must complete rather than starve"
    );
    assert_eq!(
        text_out, expected_stream,
        "the streamed deltas must arrive complete and in order"
    );
    let host_pings_during_stream = probe.host_pings() - pings_at_stream_start;
    assert!(
        host_pings_during_stream >= 2,
        "at least two Host pings must cross the 40-second stream, got {host_pings_during_stream}"
    );
    assert!(
        probe.pongs_sent() - pongs_at_stream_start >= host_pings_during_stream,
        "the client must pong every Host ping while streaming"
    );

    // The late acknowledgement of the expired receipt is stale, and the same
    // connection keeps serving after prioritizing transport control.
    let acked = ack_summary(served.client(), &stale)
        .await
        .expect("the late ack must answer");
    assert_eq!(
        acked,
        UndeliveredAckOutcome::StalePresentation,
        "an ack after the receipt deadline must be stale"
    );
    let companion = served.client().companion_ref();
    let answer = ask(
        served.client(),
        WirePayload::HistoryRequest(cmds::history_request(&companion, None, 1)),
        "history after the stream",
    )
    .await
    .expect("the connection must still answer after the stream");
    assert!(
        matches!(answer, WirePayload::HistoryResponse(_)),
        "got {answer:?}"
    );
    served.stop().await;
}

/// The client leaves mid-stream: currentness drops promptly while the
/// handler is provably still inside the provider's stream, and the Host
/// shutdown afterwards bounds the leftover operation with the cooperative
/// abort instead of waiting for the provider.
#[tokio::test]
async fn client_eof_during_a_stream_drops_currentness_before_the_handler_finishes() {
    let dir = tempfile::tempdir().expect("temp dir");
    let sentinel = "EOF-STREAM-SENTINEL";
    let stream_text = "stream until the client leaves";
    let chunk_script: Vec<String> = (0..3).map(|index| format!("{sentinel}-{index}")).collect();
    let transport = Arc::new(ScriptedTransport::new(
        vec![
            (
                on_latest_owner("summon before the eof"),
                // Reported usage for the completed summon, so the only
                // unknown usage fact is the aborted stream attempt.
                Call::reported("SUMMON-EOF-REPLY", 120, 0, 30),
            ),
            (
                on_latest_owner(stream_text),
                // Each chunk is pushed and then the provider sleeps for the
                // whole window, so the handler provably cannot finish on its
                // own while the test observes it.
                Call::stream(
                    "NEVER-COMMITTED-EOF-REPLY",
                    chunk_script,
                    Duration::from_secs(30),
                ),
            ),
        ],
        &[],
    ));
    let mut served = serve_and_setup(
        dir.path().to_path_buf(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE],
    )
    .await;
    let (_round, _stream, _reply) = send_round(served.client(), "summon before the eof")
        .await
        .expect("the summon round must complete");
    wait_presence(&served.dir, "present").await;

    let companion = served.client().companion_ref();
    let target = served.client().round_target();
    let intake = ask(
        served.client(),
        WirePayload::SubmitTextInput(cmds::submit_input(
            &companion,
            target,
            String::from(stream_text),
            String::from("en"),
        )),
        "eof stream submit",
    )
    .await
    .expect("the stream submit must be answered while the intake precedes the provider call");
    assert!(
        matches!(
            intake,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
        ),
        "the stream submit must be accepted, got {intake:?}"
    );
    let open = ask_stream(served.client())
        .await
        .expect("the stream must open");
    assert!(
        matches!(open, WirePayload::TextStreamOpen(_)),
        "got {open:?}"
    );
    let first = ask_stream(served.client())
        .await
        .expect("the first streamed frame must arrive");
    assert!(
        matches!(first, WirePayload::TextStreamFrame(_)),
        "got {first:?}"
    );
    assert_eq!(
        transport.streaming_count(),
        1,
        "the provider must be inside the stream"
    );

    // EOF mid-stream: `wait_presence` is bounded well below the provider's
    // 30-second sleep, so reaching `no_active` here proves the connection
    // stopped being current without waiting for the handler.
    drop(served.client.take());
    let (_, fallen_back) = wait_presence(dir.path(), "no_active").await;
    assert_eq!(
        presence_row(dir.path()),
        Some((String::from("no_active"), fallen_back)),
        "the closed connection must fall back promptly"
    );
    assert_eq!(
        transport.streaming_count(),
        1,
        "currentness must drop while the handler is still inside the stream"
    );

    // Host shutdown sends the cooperative abort; the provider is never
    // released by the test.
    served.request_graceful_stop();
    assert!(
        served.join_listener_within(Duration::from_secs(30)).await,
        "the cooperative abort must bound the shutdown"
    );
    wait_until_deletion_drivers(served.handle(), 0).await;
    wait_until_deletion_blocking(served.handle(), 0).await;

    let db = dir.path().join("app.db");
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM usage_fact WHERE source = 'unknown'"
        ),
        1,
        "the aborted stream attempt keeps its unknown usage fact"
    );
    assert_eq!(
        db_scalar(
            &db,
            "SELECT COUNT(*) FROM history_message WHERE role = 'companion'"
        ),
        1,
        "only the completed summon commits a reply"
    );
    assert_eq!(
        served.canonical_remainder(sentinel).await,
        0,
        "the streamed reply must never commit"
    );
}

/// Moves only the clock forward in small steps so timers fire at their
/// boundaries while every socket operation stays on the real clock.
async fn advance_clock_in_steps(span: Duration) {
    let mut left = span;
    while left > Duration::ZERO {
        let take = Duration::from_secs(5).min(left);
        tokio::time::advance(take).await;
        left -= take;
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
    }
}

#[tokio::test]
async fn a_receipt_expires_while_the_business_handler_is_parked() {
    let dir = tempfile::tempdir().expect("temp dir");
    let park_text = "park past the receipt deadline";
    let transport = Arc::new(ScriptedTransport::new(
        vec![(
            on_latest_owner(park_text),
            Call::text(format!("REPLY-DONE {park_text}")),
        )],
        &[],
    ));
    let mut served = serve_and_setup(
        dir.path().to_path_buf(),
        Arc::clone(&transport),
        &[cmds::CAPABILITY_DIALOGUE],
    )
    .await;
    let (round, stream, _reply) = send_round(served.client(), "summon for the receipt")
        .await
        .expect("the summon round must complete");
    wait_presence(&served.dir, "present").await;
    confirm_round(served.client(), &round, stream).await;

    // A receipt is issued on this connection and never acknowledged.
    let stale = fetch_summary(served.client(), "the unacked receipt")
        .await
        .expect("the subscription must answer");
    assert!(
        !stale.receipt.0.is_empty(),
        "the fixture must issue a receipt"
    );
    assert_eq!(served.handle().receipts_held_for_tests(), 1);

    transport.block_input(on_latest_owner(park_text));
    let companion = served.client().companion_ref();
    let target = served.client().round_target();
    let intake = ask(
        served.client(),
        WirePayload::SubmitTextInput(cmds::submit_input(
            &companion,
            target,
            String::from(park_text),
            String::from("en"),
        )),
        "park submit",
    )
    .await
    .expect("the park submit must be answered while the intake precedes the provider call");
    assert!(
        matches!(
            intake,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
        ),
        "the park submit must be accepted, got {intake:?}"
    );
    transport.wait_parked(1).await;
    assert_eq!(
        served.handle().receipts_held_for_tests(),
        1,
        "the receipt is still held when the park begins"
    );

    // The deadline is 30 seconds from the receipt's commit on the monotonic
    // clock. Only the clock moves here; the handler stays parked throughout,
    // and the receipt's state transition never waits for it.
    tokio::time::pause();
    advance_clock_in_steps(Duration::from_secs(25)).await;
    assert_eq!(
        transport.parked_count(),
        1,
        "the handler must still be parked before the deadline"
    );
    assert_eq!(
        served.handle().receipts_held_for_tests(),
        1,
        "the receipt is still held before its 30-second deadline"
    );
    advance_clock_in_steps(Duration::from_secs(10)).await;
    tokio::time::resume();
    assert_eq!(
        transport.parked_count(),
        1,
        "the handler must still be parked at the deadline"
    );
    assert_eq!(
        served.handle().receipts_held_for_tests(),
        0,
        "the receipt must expire at its deadline while the business handler is parked"
    );

    transport.release_blocked();
    // The late acknowledgement of the expired receipt is stale — it is never
    // adopted as the presentation of anything current.
    let acked = ack_summary(served.client(), &stale)
        .await
        .expect("the late ack must answer");
    assert_eq!(
        acked,
        UndeliveredAckOutcome::StalePresentation,
        "an ack after the receipt deadline must be stale"
    );

    // After the business work completes, the subsequent undelivered page
    // becomes deliverable in order: the reply row exists only once the
    // parked round has committed, and the page acknowledges without
    // replaying the parked provider call.
    let inputs_before = transport
        .input_texts()
        .iter()
        .filter(|input| input.contains(park_text))
        .count();
    assert_eq!(inputs_before, 1, "the parked call ran exactly once");
    let next = wait_for_summary_with(served.client(), "REPLY-DONE").await;
    assert!(
        next.items
            .iter()
            .any(|item| item.excerpt.contains("REPLY-DONE")),
        "the page after the expired receipt must carry the completed round: {next:?}"
    );
    assert_eq!(
        transport
            .input_texts()
            .iter()
            .filter(|input| input.contains(park_text))
            .count(),
        1,
        "delivering the subsequent page must not replay the parked work"
    );
    assert_eq!(transport.parked_count(), 0, "the handler is done");
    served.stop().await;
}

#[tokio::test]
async fn a_stalled_peer_write_is_bounded_and_the_serving_task_exits() {
    let dir = tempfile::tempdir().expect("temp dir");
    let handle = open_host(dir.path()).await;
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(conn::run_until_shutdown(
        dir.path().to_path_buf(),
        Arc::clone(&handle),
        transport,
        shutdown,
    ));

    // A tiny receive window on this raw client lets the Host's pong writes
    // fill the TCP path quickly; the pending pairing keeps the connection in
    // the owner-confirmation window so only the write wait may close it.
    let mut client = WssClient::connect_sized(dir.path(), Some(2048), Some(1024 * 1024))
        .await
        .expect("a same-machine client with the current token must upgrade");
    let mut pairing = WireFrame {
        envelope: crafted_envelope("PairingRequest"),
        payload: WirePayload::PairingRequest(PairingRequest {
            device_descriptor: String::from("stalled writer"),
        }),
    };
    pairing.envelope.correlation.request_id = Some(RequestWireId(uuid::Uuid::new_v4()));
    client.send_wire(&pairing).await;
    let reply = client.recv_wire().await;
    assert!(
        matches!(
            reply.payload,
            WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation { .. })
        ),
        "the pairing must pend before the stall, got {}",
        reply.payload.message_type()
    );

    use futures_util::StreamExt as _;
    let (mut sink, mut reader) = client.socket.split();
    let pinger = tokio::spawn(async move {
        use futures_util::SinkExt as _;
        let payload = vec![0x5a_u8; 120];
        for _ in 0..20_000 {
            if sink
                .send(Message::Ping(payload.clone().into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // The client never reads: the Host's pong write parks on the socket, and
    // the write wait bound must still end the connection instead of owning
    // the serving task forever.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(32)).await;
    tokio::time::resume();
    let closed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match reader.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await;
    assert!(
        closed.is_ok(),
        "the stalled write must be bounded by the write wait and close the connection"
    );
    pinger.abort();

    stop.send_replace(true);
    let joined = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("the listener must stop")
        .expect("the listener must not panic");
    joined.expect("the listener must shut down cleanly");
}

#[tokio::test]
async fn the_host_answers_a_transport_ping_with_a_pong_and_closes_on_close() {
    let dir = tempfile::tempdir().expect("temp dir");
    let handle = open_host(dir.path()).await;
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(conn::run_until_shutdown(
        dir.path().to_path_buf(),
        Arc::clone(&handle),
        transport,
        shutdown,
    ));
    let mut client = raw_dial(dir.path()).await;
    use futures_util::{SinkExt as _, StreamExt as _};

    let expected = vec![0x37_u8; 64];
    client
        .socket
        .send(Message::Ping(expected.clone().into()))
        .await
        .expect("the ping must send");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match client.socket.next().await {
                Some(Ok(Message::Pong(payload))) if payload.as_ref() == expected.as_slice() => {
                    return;
                }
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => {
                    panic!("the ping must be answered before the connection ends")
                }
                Some(Ok(_)) => continue,
            }
        }
    })
    .await
    .expect("the Host must answer a transport ping with a matching pong");

    client
        .socket
        .send(Message::Close(None))
        .await
        .expect("close");
    let closed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match client.socket.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "a Close frame must end the connection");

    stop.send_replace(true);
    let joined = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("the listener must stop")
        .expect("the listener must not panic");
    joined.expect("the listener must shut down cleanly");
}

#[tokio::test]
async fn a_fragmented_message_over_the_limit_closes_the_connection() {
    let dir = tempfile::tempdir().expect("temp dir");
    let handle = open_host(dir.path()).await;
    let transport = Arc::new(ScriptedTransport::new(Vec::new(), &[]));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(conn::run_until_shutdown(
        dir.path().to_path_buf(),
        Arc::clone(&handle),
        transport,
        shutdown,
    ));
    let host = raw_dial(dir.path()).await;
    host.send_fragmented_oversize_and_expect_close().await;
    stop.send_replace(true);
    let joined = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("the listener must stop")
        .expect("the listener must not panic");
    joined.expect("the listener must shut down cleanly");
}
