//! Stage 7 C2: Task / Workspace management GUI against a real Host.
//!
//! Provider is fake and barrier-gated. The Slint window is not displayed; the
//! same [`DesktopRuntime`] the window projects is driven here. Tasks are
//! created only through companion `[task-control]` delegation, never a
//! GUI-only factory.
//!
//! Covers acceptance §4's GUI path and the GUI subset of §5: Unknown vs
//! interrupted vs Failed vs Cancelled vs Completed; cancel admission vs
//! stop-complete; resume bound to the displayed revision/purpose; GUI close /
//! reconnect; presentation ACK only after the panel copies a receipt.
//! Conversation ACK already lives in [`ene_desktop::session::submit_and_collect`].

#![cfg(any(unix, windows))]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration-test helpers outside #[test] functions need the fixture allowances clippy.toml grants only to test functions"
)]

use std::collections::{BTreeSet, VecDeque};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ene_api::v1::management::ManagementOutcome;
use ene_api::v1::undelivered::{ResumeTaskOutcomeWire, UndeliveredAckOutcome};
use ene_core::conn;
use ene_core::host_control;
use ene_core::serve::{CoreError, CredStore, HostHandle};
use ene_credential::MemoryCredentialStore;
use ene_desktop::ui::{DesktopRuntime, Page};
use ene_inference::{ProviderRequest, ProviderResponse, ProviderTransport};
use ene_local_control::{ControlOutcome, FromConfirmation};

const MODEL: &str = "gpt-slice-test";
const SECRET: &str = "sk-stage7-c2-secret-4408";

const PROPOSE_REPLY: &str =
    r#"[task-control] {"kind":"propose_task","purpose":"read input.txt and write report.md"}"#;
const READ_REPLY: &str = r#"{"tool":"read","path":"input.txt"}"#;
const CREATE_REPLY: &str =
    "{\"tool\":\"create\",\"path\":\"report.md\",\"content\":\"# Report\\nnotes\"}";
const FINAL_REPLY: &str = r#"{"final":"created report.md from input.txt"}"#;

struct GateTransport {
    replies: Mutex<VecDeque<String>>,
    sends: AtomicUsize,
    blocks: Mutex<BTreeSet<usize>>,
    failures: Mutex<BTreeSet<usize>>,
}

impl GateTransport {
    fn new(replies: Vec<String>, blocks: &[usize]) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            sends: AtomicUsize::new(0),
            blocks: Mutex::new(blocks.iter().copied().collect()),
            failures: Mutex::new(BTreeSet::new()),
        })
    }

    fn sends(&self) -> usize {
        self.sends.load(Ordering::SeqCst)
    }

    fn unblock(&self, call: usize) {
        self.blocks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&call);
    }

    fn fail(&self, call: usize) {
        self.failures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(call);
        self.unblock(call);
    }

    async fn wait_sends(&self, wanted: usize) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while self.sends() < wanted {
            assert!(
                tokio::time::Instant::now() < deadline,
                "provider sends did not reach {wanted}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

impl ProviderTransport for GateTransport {
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
        let _ = req;
        Box::pin(async move {
            let call = self.sends.fetch_add(1, Ordering::SeqCst) + 1;
            loop {
                let held = self
                    .blocks
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .contains(&call);
                if !held {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            if self
                .failures
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&call)
            {
                return Err(
                    ene_inference::InferenceTechnicalError::ProviderTransportFailed(String::from(
                        "fixture provider disconnected before returning a result",
                    )),
                );
            }
            let text = self
                .replies
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop_front()
                .unwrap_or_default();
            Ok(ProviderResponse { text, usage: None })
        })
    }
}

struct ServingTask {
    shutdown: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<Result<(), CoreError>>,
}

impl ServingTask {
    fn start(dir: &Path, handle: Arc<HostHandle>, transport: Arc<GateTransport>) -> Self {
        let (shutdown, rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(conn::run_until_shutdown(
            dir.to_path_buf(),
            handle,
            transport,
            rx,
        ));
        Self { shutdown, task }
    }

    async fn shutdown_and_join(self) {
        self.shutdown.send_replace(true);
        tokio::time::timeout(Duration::from_secs(30), self.task)
            .await
            .expect("serving shutdown must drain; release provider gates before restart")
            .expect("serving task must join")
            .expect("serving shutdown must succeed");
    }
}

async fn open_host(dir: &Path) -> Arc<HostHandle> {
    match HostHandle::open_with_cred_store(dir, CredStore::Memory(MemoryCredentialStore::new()))
        .await
    {
        Ok(handle) => Arc::new(handle),
        Err(error) => panic!("host must open: {error}"),
    }
}

async fn wait_for_control(dir: &Path) -> bool {
    for _ in 0..200 {
        #[cfg(unix)]
        if tokio::net::UnixStream::connect(host_control::control_socket_path(dir))
            .await
            .is_ok()
        {
            return true;
        }
        #[cfg(windows)]
        if host_control::ControlClient::connect(dir).await.is_ok() {
            tokio::task::yield_now().await;
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

async fn pair_and_seat(desktop: &mut DesktopRuntime, handle: &Arc<HostHandle>) {
    let channel = host_control::seat_test_gui_for_tests(handle).expect("private channel");
    desktop
        .attach_confirmation(channel)
        .expect("the private channel is the seat");
    desktop
        .connect_or_begin_pairing()
        .await
        .expect("pairing must challenge");
    match desktop.confirm_owner().await.expect("owner confirm pairs") {
        FromConfirmation::Outcome(ControlOutcome::DeviceApproved { .. }) => {}
        other => panic!("expected DeviceApproved, got {other:?}"),
    }
}

async fn complete_setup(desktop: &mut DesktopRuntime) {
    desktop.set_secret(String::from(SECRET));
    desktop
        .begin_credential_put()
        .await
        .expect("credential put must challenge");
    match desktop
        .confirm_owner()
        .await
        .expect("owner confirm stores the key")
    {
        FromConfirmation::Outcome(ControlOutcome::CredentialStored { .. }) => {}
        other => panic!("expected CredentialStored, got {other:?}"),
    }
    desktop.set_model(String::from(MODEL));
    let assigned = desktop.assign_model().await.expect("dialogue assign");
    assert!(
        matches!(assigned, ManagementOutcome::StoredAsRuleView { .. }),
        "dialogue assignment must store, got {assigned:?}"
    );
}

async fn say(desktop: &mut DesktopRuntime, text: &str) {
    desktop.composer_mut().set_draft(text.to_owned());
    desktop.send_text().await.expect("chat turn must complete");
}

fn workspace_with_input() -> tempfile::TempDir {
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    workspace
}

async fn wait_listed(desktop: &mut DesktopRuntime, needle: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        desktop.refresh_tasks().await.expect("task list");
        let snap = desktop.snapshot();
        if snap.tasks.iter().any(|line| line.contains(needle)) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "task list never showed {needle:?}: {:?}",
            snap.tasks
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_path(path: &Path) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !path.exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "workspace write did not land: {}",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn select_first_task(desktop: &mut DesktopRuntime) {
    desktop.open_tasks().await.expect("open tasks");
    desktop
        .select_listed_task(0)
        .await
        .expect("select listed task");
}

/// §4: Workspace → companion-created Task → parallel chat → complete with
/// changed files / save location / remaining work on the management panel.
#[tokio::test]
async fn acceptance_4_workspace_task_gui_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = workspace_with_input();
    let transport = GateTransport::new(
        vec![
            String::from(PROPOSE_REPLY),
            String::from("You are welcome."),
            String::from(READ_REPLY),
            String::from(CREATE_REPLY),
            String::from(FINAL_REPLY),
        ],
        &[2],
    );
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_seat(&mut desktop, &handle).await;
    complete_setup(&mut desktop).await;
    desktop.try_spawn_body(&desktop.bundled_ene_asset());
    assert_eq!(desktop.snapshot().body_status, "Absent");

    desktop.open_tasks().await.expect("empty tasks page");
    assert_eq!(desktop.snapshot().page, "Tasks");
    assert!(
        desktop.snapshot().tasks.is_empty(),
        "the GUI must not mint tasks: {:?}",
        desktop.snapshot().tasks
    );

    let selected = desktop
        .select_workspace_folder(workspace.path())
        .await
        .expect("workspace select");
    assert!(
        matches!(selected, ManagementOutcome::AppliedAsOneTime),
        "workspace must apply, got {selected:?}"
    );

    say(
        &mut desktop,
        "please read input.txt and write report.md as a task",
    )
    .await;
    let timeline = desktop.snapshot().timeline.join("\n");
    assert!(
        timeline.contains("Task accepted"),
        "creation stays on companion delegation: {timeline}"
    );
    assert!(
        !timeline.contains("[task-control]"),
        "protocol must not appear: {timeline}"
    );
    transport.wait_sends(2).await;

    wait_listed(&mut desktop, "in_progress").await;
    wait_listed(&mut desktop, " running").await;
    select_first_task(&mut desktop).await;
    let detail = desktop.snapshot().task_detail;
    assert!(detail.contains("rev 1"), "{detail}");
    assert!(detail.contains("in_progress"), "{detail}");
    assert!(detail.contains("running=yes"), "{detail}");
    assert!(detail.contains("interrupted=false"), "{detail}");
    assert!(
        detail.contains("purpose-id ") && detail.contains(':'),
        "purpose identity is Host-published: {detail}"
    );
    assert!(
        detail.contains("workspace ")
            && detail.contains(&workspace.path().to_string_lossy().into_owned()),
        "workspace path is the Owner-sent folder: {detail}"
    );
    assert!(detail.contains("actions "), "{detail}");
    assert!(
        !desktop.snapshot().contains_secret(SECRET),
        "registered secret must not appear in the task snapshot"
    );

    desktop.open_page(Page::Chat);
    say(&mut desktop, "thanks, keep going").await;
    assert_eq!(
        desktop
            .snapshot()
            .timeline
            .iter()
            .filter(|line| line.contains("You are welcome."))
            .count(),
        1
    );
    desktop.open_tasks().await.expect("tasks while running");
    assert!(
        desktop
            .snapshot()
            .tasks
            .iter()
            .any(|line| line.contains("in_progress")),
        "parallel chat must not cancel the Host-only Task: {:?}",
        desktop.snapshot().tasks
    );

    transport.unblock(2);
    wait_path(&workspace.path().join("report.md")).await;
    wait_listed(&mut desktop, "completed").await;
    select_first_task(&mut desktop).await;
    let detail = desktop.snapshot().task_detail;
    assert!(detail.contains("completed"), "{detail}");
    assert!(detail.contains("running=no"), "{detail}");
    assert!(detail.contains("interrupted=false"), "{detail}");
    assert!(
        detail.contains("adopted-rev") || detail.contains("task_result"),
        "result adoption is Host-authored: {detail}"
    );
    assert!(
        detail.contains("report.md") || detail.contains("created report.md"),
        "changed files / remaining work come from the report source: {detail}"
    );
    assert!(workspace.path().join("input.txt").exists());
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("report.md")).unwrap(),
        "# Report\nnotes"
    );
    assert_eq!(transport.sends(), 5, "no second launch");
    server.shutdown_and_join().await;
}

/// Cancel admission (`AppliedAsOneTime`) is not stop-complete. Failed is
/// never inferred from a technical provider drop; that is interrupted.
#[tokio::test]
async fn cancel_admission_is_not_stop_complete() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = workspace_with_input();
    let transport = GateTransport::new(
        vec![String::from(PROPOSE_REPLY), String::from(READ_REPLY)],
        &[2],
    );
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_seat(&mut desktop, &handle).await;
    complete_setup(&mut desktop).await;
    desktop
        .select_workspace_folder(workspace.path())
        .await
        .expect("workspace");
    say(&mut desktop, "please read input.txt and write report.md").await;
    transport.wait_sends(2).await;
    wait_listed(&mut desktop, " running").await;
    select_first_task(&mut desktop).await;

    let outcome = desktop
        .cancel_displayed_task()
        .await
        .expect("cancel must answer");
    assert!(
        matches!(outcome, ManagementOutcome::AppliedAsOneTime),
        "cancel admission must apply, got {outcome:?}"
    );
    let detail = desktop.snapshot().task_detail;
    assert!(
        detail.contains("accepted (stop not yet complete)"),
        "admission is not stop-complete: {detail}"
    );
    assert!(!detail.contains("failed"), "cancel is not Failed: {detail}");

    transport.fail(2);
    wait_listed(&mut desktop, "cancelled").await;
    select_first_task(&mut desktop).await;
    let detail = desktop.snapshot().task_detail;
    assert!(detail.contains("cancelled"), "{detail}");
    assert!(detail.contains("running=no"), "{detail}");
    assert!(detail.contains("interrupted=false"), "{detail}");
    assert!(workspace.path().join("input.txt").exists());

    let refused = desktop
        .resume_displayed_task(String::from("try again"))
        .await
        .expect("terminal resume is a domain outcome");
    assert!(
        matches!(
            refused,
            ResumeTaskOutcomeWire::TaskTerminal { ref progress } if progress == "cancelled"
        ),
        "cancelled is terminal, got {refused:?}"
    );
    server.shutdown_and_join().await;
}

/// S5-01/S5-02 GUI subset: close mid-wait keeps the Host-only Task; reconnect
/// does not cancel. ACK is refused until the panel has copied a receipt.
/// Conversation ConfirmPresentation is the existing chat hook in session.
#[tokio::test]
async fn gui_close_reconnect_and_ack_only_after_present() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = workspace_with_input();
    let transport = GateTransport::new(
        vec![
            String::from(PROPOSE_REPLY),
            String::from("Still here."),
            String::from(READ_REPLY),
            String::from(CREATE_REPLY),
            String::from(FINAL_REPLY),
        ],
        &[2],
    );
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_seat(&mut desktop, &handle).await;
    complete_setup(&mut desktop).await;
    desktop
        .select_workspace_folder(workspace.path())
        .await
        .expect("workspace");
    say(&mut desktop, "please read input.txt and write report.md").await;
    transport.wait_sends(2).await;
    wait_listed(&mut desktop, "in_progress").await;

    let ack_early = desktop.ack_presented_tasks().await;
    assert!(
        ack_early.is_err(),
        "receiving a connection is not presentation: {ack_early:?}"
    );
    assert!(!desktop.has_presented_task_receipt());

    drop(desktop.take_client());
    desktop
        .reconnect()
        .await
        .expect("GUI reconnect against a live Host");
    wait_listed(&mut desktop, "in_progress").await;
    assert!(
        desktop
            .snapshot()
            .tasks
            .iter()
            .any(|line| line.contains("in_progress")),
        "disconnect must not cancel: {:?}",
        desktop.snapshot().tasks
    );
    assert!(!desktop.has_presented_task_receipt());

    say(&mut desktop, "are you still working?").await;
    assert!(
        desktop
            .snapshot()
            .timeline
            .iter()
            .any(|line| line.contains("Still here.")),
        "another valid round must run mid-execution"
    );

    transport.unblock(2);
    wait_path(&workspace.path().join("report.md")).await;
    wait_listed(&mut desktop, "completed").await;

    desktop
        .present_task_undelivered()
        .await
        .expect("copy the receipt into the panel");
    let detail = desktop.snapshot().task_detail;
    if desktop.has_presented_task_receipt() {
        assert!(detail.contains("presentation ready-to-ack"), "{detail}");
        let acked = desktop
            .ack_presented_tasks()
            .await
            .expect("ACK after present");
        assert!(
            matches!(
                acked,
                UndeliveredAckOutcome::Presented { .. }
                    | UndeliveredAckOutcome::AlreadyPresented
                    | UndeliveredAckOutcome::KeptUnknown
            ),
            "ACK after present is a domain outcome, got {acked:?}"
        );
        assert!(!desktop.has_presented_task_receipt());
        assert!(
            desktop.snapshot().task_detail.contains("ack "),
            "{}",
            desktop.snapshot().task_detail
        );
    } else {
        assert!(
            detail.contains("presentation not-presented") || detail.contains("undelivered none"),
            "empty backlog still is not an ACK: {detail}"
        );
    }
    let debug = format!("{:?}", desktop.snapshot());
    assert!(!debug.contains(SECRET), "Debug of snapshot must not leak");
    server.shutdown_and_join().await;
}

/// S5-16/S5-17 GUI subset: interrupted is in_progress without execution
/// registration. Resume echoes the displayed revision/purpose; a later list
/// must not replace that premise. Failed is not this technical path.
#[tokio::test]
async fn resume_is_bound_to_the_displayed_premise() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = workspace_with_input();
    let transport_a = GateTransport::new(
        vec![String::from(PROPOSE_REPLY), String::from(READ_REPLY)],
        &[2],
    );
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport_a));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_seat(&mut desktop, &handle).await;
    complete_setup(&mut desktop).await;
    desktop
        .select_workspace_folder(workspace.path())
        .await
        .expect("workspace");
    say(&mut desktop, "please read input.txt and write report.md").await;
    transport_a.wait_sends(2).await;
    wait_listed(&mut desktop, "in_progress").await;
    select_first_task(&mut desktop).await;
    let shown_revision = desktop.displayed_task_revision().expect("selected");
    let shown_purpose = desktop.displayed_task_purpose().expect("purpose");
    assert_eq!(shown_revision, 1);

    transport_a.fail(2);
    server.shutdown_and_join().await;
    handle
        .run_startup_mutations()
        .await
        .expect("restart mutations");
    let transport_b = GateTransport::new(
        vec![String::from(CREATE_REPLY), String::from(FINAL_REPLY)],
        &[1],
    );
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport_b));
    assert!(wait_for_control(dir.path()).await);
    desktop
        .reconnect()
        .await
        .expect("reconnect after host restart");
    wait_listed(&mut desktop, "in_progress").await;
    assert!(
        desktop
            .snapshot()
            .tasks
            .iter()
            .any(|line| !line.contains(" running") && line.contains("in_progress")),
        "restart holds no launch reservation: {:?}",
        desktop.snapshot().tasks
    );
    select_first_task(&mut desktop).await;
    let detail = desktop.snapshot().task_detail;
    assert!(detail.contains("interrupted=true"), "{detail}");
    assert!(detail.contains("in_progress"), "{detail}");
    assert!(
        !detail.contains("failed"),
        "technical drop is not Failed: {detail}"
    );
    assert_eq!(desktop.displayed_task_revision(), Some(1));
    assert_eq!(
        desktop.displayed_task_purpose().as_deref(),
        Some(shown_purpose.as_str())
    );

    let resumed = desktop
        .resume_displayed_task(String::from("finish the remaining work"))
        .await
        .expect("explicit resume");
    assert!(
        matches!(resumed, ResumeTaskOutcomeWire::Resumed { revision: 2, .. }),
        "resume must mint r+1, got {resumed:?}"
    );
    desktop.refresh_tasks().await.expect("list after resume");
    assert!(
        desktop
            .snapshot()
            .tasks
            .iter()
            .any(|line| line.contains("rev 2")),
        "Host list may move: {:?}",
        desktop.snapshot().tasks
    );
    assert_eq!(
        desktop.displayed_task_revision(),
        Some(1),
        "stale view must not auto-replace with latest"
    );
    assert_eq!(
        desktop.displayed_task_purpose().as_deref(),
        Some(shown_purpose.as_str())
    );
    let stale = desktop
        .resume_displayed_task(String::from("again"))
        .await
        .expect("stale resume is a domain outcome");
    assert!(
        matches!(
            stale,
            ResumeTaskOutcomeWire::StalePremise {
                current_revision: 2
            }
        ),
        "displayed premise stays bound, got {stale:?}"
    );
    assert!(
        desktop
            .snapshot()
            .task_detail
            .contains("stale displayed-rev 1"),
        "{}",
        desktop.snapshot().task_detail
    );
    assert_eq!(desktop.displayed_task_revision(), Some(1));

    transport_b.fail(1);
    server.shutdown_and_join().await;
}

#[test]
fn conversation_ack_hook_is_confirm_presentation() {
    let src = include_str!("../src/session.rs");
    assert!(
        src.contains("ConfirmPresentation"),
        "slice E keeps the chat ACK hook in submit_and_collect"
    );
    assert!(
        src.contains("PresentationStatus::Presented"),
        "chat ACK is issued after the stream has been collected"
    );
}
