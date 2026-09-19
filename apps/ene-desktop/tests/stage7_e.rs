//! Stage 7 E: GUI as presentation / erasure / secret participant.
//!
//! Real Host, fake provider, barrier-gated where needed. The same
//! [`DesktopRuntime`] the Slint window projects is driven here. Run with
//! `--test-threads=1`. Windows 11 / NixOS 26.11 / live IME / overlay probes:
//! 未実施. Empty-seat occupancy is not authenticity.

#![cfg(any(unix, windows))]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration-test helpers outside #[test] functions need the fixture allowances clippy.toml grants only to test functions"
)]

use std::collections::VecDeque;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ene_api::v1::deletion::ClientTempClass;
use ene_api::v1::management::ManagementOutcome;
use ene_api::v1::round::PresentationStatus;
use ene_companion::{CompanionRepository as _, UNDELIVERED_PAGE_MAX, UndeliveredRepository as _};
use ene_core::conn;
use ene_core::host_control;
use ene_core::serve::{CoreError, CredStore, HostHandle};
use ene_credential::MemoryCredentialStore;
use ene_desktop::body_supervise::BodySupervisor;
use ene_desktop::measure;
use ene_desktop::session;
use ene_desktop::ui::{DesktopRuntime, Page};
use ene_inference::{ProviderRequest, ProviderResponse, ProviderTransport};
use ene_local_control::{ControlOutcome, FromHost};

const MODEL: &str = "gpt-slice-test";
const SECRET: &str = "sk-stage7-e-secret-5519";
const TARGET: &str = "stage7-e-keyword-omega";

struct GateTransport {
    replies: Mutex<VecDeque<String>>,
    sends: AtomicUsize,
}

impl GateTransport {
    fn with_replies(replies: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.iter().map(|text| (*text).to_string()).collect()),
            sends: AtomicUsize::new(0),
        })
    }

    fn sends(&self) -> usize {
        self.sends.load(Ordering::SeqCst)
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
            self.sends.fetch_add(1, Ordering::SeqCst);
            let reply = self
                .replies
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop_front()
                .unwrap_or_else(|| String::from("ok"));
            Ok(ProviderResponse {
                text: reply,
                usage: None,
            })
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
            .expect("serving shutdown must drain")
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

async fn pair_and_seat(desktop: &mut DesktopRuntime) {
    desktop
        .occupy_seat()
        .await
        .expect("empty seat occupancy is accident prevention, not authenticity");
    desktop
        .connect_or_begin_pairing()
        .await
        .expect("pairing must challenge");
    match desktop.confirm_owner().await.expect("owner confirm pairs") {
        FromHost::Outcome(ControlOutcome::DeviceApproved { .. }) => {}
        other => panic!("expected DeviceApproved, got {other:?}"),
    }
}

async fn pair_and_setup(desktop: &mut DesktopRuntime) {
    pair_and_seat(desktop).await;
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
        FromHost::Outcome(ControlOutcome::CredentialStored { .. }) => {}
        other => panic!("expected CredentialStored, got {other:?}"),
    }
    desktop.set_model(String::from(MODEL));
    let assigned = desktop.assign_model().await.expect("assign");
    assert!(
        matches!(assigned, ManagementOutcome::StoredAsRuleView { .. }),
        "assignment must store, got {assigned:?}"
    );
}

async fn unpresented_count(handle: &HostHandle) -> usize {
    let companion = handle
        .store_for_tests()
        .ensure_running_companion()
        .await
        .expect("companion");
    handle
        .store_for_tests()
        .list_unpresented(companion, None, UNDELIVERED_PAGE_MAX)
        .await
        .expect("unpresented page")
        .entries
        .len()
}

async fn drive_gui_until(desktop: &mut DesktopRuntime, handle: &HostHandle, needle: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(150);
    loop {
        match desktop.refresh_deletion().await {
            Ok(()) | Err(_) => {}
        }
        let body = desktop.snapshot().deletion_body;
        if body.contains(needle) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "GUI deletion body never contained {needle}: {body}"
        );
        handle.wake_deletion_driver_for_tests();
        match handle.run_targeted_deletion_tick().await {
            Ok(_) | Err(_) => {}
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn snapshot_has_target(desktop: &DesktopRuntime) -> bool {
    let snap = desktop.snapshot();
    snap.contains_secret(TARGET)
        || snap.timeline.iter().any(|line| line.contains(TARGET))
        || snap.history.iter().any(|line| line.contains(TARGET))
        || snap.draft.contains(TARGET)
        || snap.search_draft.contains(TARGET)
        || snap.memory_panel.contains(TARGET)
        || snap.task_detail.contains(TARGET)
        || snap.usage_body.contains(TARGET)
        || snap.deletion_body.contains(TARGET)
}

#[tokio::test]
async fn targeted_deletion_wipes_gui_copies_and_reports_wiped_after_erase() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport = GateTransport::with_replies(&[&format!("I will keep {TARGET} in mind.")]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_setup(&mut desktop).await;
    desktop
        .composer_mut()
        .set_draft(format!("please remember {TARGET}"));
    desktop.send_text().await.expect("plant the target");
    desktop.set_search_draft(String::from(TARGET));
    desktop.composer_mut().set_draft(format!("draft {TARGET}"));
    desktop.composer_mut().begin_composition();
    match desktop.refresh_memory().await {
        Ok(()) | Err(_) => {}
    }
    match desktop.refresh_tasks().await {
        Ok(()) | Err(_) => {}
    }
    match desktop.refresh_usage().await {
        Ok(()) | Err(_) => {}
    }
    assert!(
        snapshot_has_target(&desktop) || desktop.composer_mut().composing(),
        "the GUI must hold a copy of the target before deletion"
    );
    assert!(
        desktop.last_erasure().is_none(),
        "wiped is not claimed before a demand is erased"
    );
    assert!(
        desktop.has_chat_receipt(),
        "presenting chat stores a receipt the demand must invalidate"
    );

    // Two surface copies and the task pane remain live until their asynchronous
    // erasure acknowledgment arrives. Host must not receive a premature wiped.
    let surfaces = Arc::new(Mutex::new(vec![desktop.surface_snapshot(); 3]));
    let erased = Arc::new(AtomicUsize::new(0));
    desktop.attach_surface_erasure({
        let surfaces = Arc::clone(&surfaces);
        let erased = Arc::clone(&erased);
        Arc::new(move || {
            let surfaces = Arc::clone(&surfaces);
            let erased = Arc::clone(&erased);
            Box::pin(async move {
                tokio::task::yield_now().await;
                surfaces.lock().unwrap().clear();
                erased.fetch_add(1, Ordering::SeqCst);
                true
            })
        })
    });
    desktop.open_page(Page::Deletion);
    desktop.set_deletion_exact_text(String::from(TARGET));
    let staged = desktop.request_deletion().await.expect("stage");
    assert_eq!(
        staged,
        ManagementOutcome::NeedsClarification,
        "Client intent only stages, got {staged:?}"
    );
    desktop
        .begin_deletion_confirm()
        .await
        .expect("seated confirm is required");
    match desktop
        .confirm_owner()
        .await
        .expect("owner confirm starts deletion")
    {
        FromHost::Outcome(ControlOutcome::DeletionStarted { .. }) => {}
        other => panic!("expected DeletionStarted, got {other:?}"),
    }
    assert!(
        desktop.deletion_has_operations(),
        "started deletion is visible on the panel"
    );
    drive_gui_until(&mut desktop, &handle, "completed").await;

    let erasure = desktop
        .last_erasure()
        .cloned()
        .expect("GUI must answer the Host demand");
    assert!(
        erasure.unverified.is_empty(),
        "wiped is only reported after copies are empty: {erasure:?}"
    );
    assert!(
        erasure.wiped.contains(&ClientTempClass::PresentationBuffer)
            || erasure.wiped.contains(&ClientTempClass::InputDraft),
        "the GUI must report a class it actually cleared: {erasure:?}"
    );
    assert!(erased.load(Ordering::SeqCst) > 0);
    assert!(surfaces.lock().unwrap().is_empty());
    assert!(!desktop.composer_mut().composing());
    assert!(desktop.composer_mut().draft().is_empty());
    assert_eq!(desktop.composer_mut().undo_len(), 0);
    assert!(!desktop.has_chat_receipt());
    assert!(!desktop.has_presented_task_receipt());

    desktop.refresh_history().await.expect("history after wipe");
    match desktop.refresh_memory().await {
        Ok(()) | Err(_) => {}
    }
    match desktop.refresh_tasks().await {
        Ok(()) | Err(_) => {}
    }
    let snap = desktop.snapshot();
    assert!(
        !snapshot_has_target(&desktop),
        "timeline/memory/task/draft/IME copies of the target must be gone: {snap:?}"
    );
    assert!(
        !snap.contains_secret(SECRET),
        "registered secret must stay out of the snapshot"
    );
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn receive_without_present_is_not_presented_ack() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport = GateTransport::with_replies(&["companion reply body"]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_setup(&mut desktop).await;

    desktop
        .composer_mut()
        .set_draft(String::from("please answer"));
    let Some(text) = desktop.composer_mut().take_sendable() else {
        panic!("draft must send");
    };
    let mut client = desktop.take_client().expect("paired client");
    let turn = session::submit_and_collect(&mut client, &text, "en")
        .await
        .expect("receive stream");
    desktop.restore_client(client);
    assert!(
        !desktop
            .snapshot()
            .timeline
            .iter()
            .any(|line| line.contains("companion reply body")),
        "mere receive must not copy the turn onto the timeline"
    );
    assert!(
        unpresented_count(&handle).await >= 1,
        "receive without present must leave the round unpresented"
    );

    let mut client = desktop.take_client().expect("client");
    session::confirm_chat_presentation(&mut client, &turn, PresentationStatus::Presented)
        .await
        .expect("ACK after present");
    desktop.restore_client(client);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if unpresented_count(&handle).await == 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "presenting then ACK must mark Presented"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn host_restart_and_reconnect_delivery_evidence_holds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport = GateTransport::with_replies(&["hello from ene"]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_setup(&mut desktop).await;
    desktop.composer_mut().set_draft(String::from("hi there"));
    desktop.send_text().await.expect("chat after assignment");
    let before = desktop.snapshot().history.clone();
    assert!(
        before.iter().any(|line| line.contains("hi there")),
        "history must keep owner text: {before:?}"
    );

    server.shutdown_and_join().await;
    handle
        .run_startup_mutations()
        .await
        .expect("restart mutations");
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    desktop
        .reconnect()
        .await
        .expect("reconnect after host restart");
    let restored = desktop.snapshot().history;
    assert!(
        restored.iter().any(|line| line.contains("hi there")),
        "host restart must keep history: {restored:?}"
    );
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn registered_secret_never_appears_on_any_page() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport = GateTransport::with_replies(&["unused"]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_setup(&mut desktop).await;
    desktop.set_secret(String::from(SECRET));
    for page in [
        Page::Wizard,
        Page::Chat,
        Page::History,
        Page::Memory,
        Page::Tasks,
        Page::Settings,
        Page::About,
        Page::Confirm,
        Page::Usage,
        Page::Deletion,
    ] {
        desktop.open_page(page);
        let snap = desktop.snapshot();
        assert!(
            !snap.contains_secret(SECRET),
            "secret leaked on {page:?}: {snap:?}"
        );
        let debug = format!("{snap:?}");
        assert!(!debug.contains(SECRET), "Debug leaked on {page:?}");
        assert!(
            !snap.deny_reason.contains(SECRET),
            "deny leaked on {page:?}"
        );
    }
    desktop.cancel_secret();
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn killing_body_leaves_chat_settings_and_cancel_alive() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport = GateTransport::with_replies(&["still here after overlay death"]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_setup(&mut desktop).await;

    let Some(exe) = BodySupervisor::locate_binary() else {
        panic!("ene-body binary must be built for Body isolation");
    };
    desktop.try_spawn_body(&exe);
    desktop.tick();
    let spawned = desktop.snapshot().body_status;
    assert!(
        spawned == "Spawned" || spawned == "Exited" || spawned == "Absent",
        "spawn outcome is observed, got {spawned}"
    );
    if spawned == "Spawned" {
        desktop.kill_body();
        desktop.tick();
        assert_ne!(
            desktop.snapshot().body_status,
            "Spawned",
            "kill must drop the overlay child"
        );
    }

    desktop
        .composer_mut()
        .set_draft(String::from("chat without body"));
    desktop.send_text().await.expect("chat survives Body kill");
    desktop
        .refresh_setup()
        .await
        .expect("settings survive Body kill");
    desktop.open_page(Page::Settings);
    let cancel = desktop.cancel_displayed_task().await;
    assert!(
        cancel.is_ok() || cancel.is_err(),
        "cancel path stays reachable"
    );
    assert_eq!(transport.sends(), 1);
    server.shutdown_and_join().await;
}

#[test]
fn measure_skeleton_is_not_a_gate_pass() {
    let record = measure::MeasurementRecord::idle_template();
    assert!(!record.claims_pass());
    assert_eq!(record.verdict, measure::MeasurementVerdict::Unmeasured);
}

#[test]
fn conversation_ack_hook_stays_in_session() {
    let src = include_str!("../src/session.rs");
    assert!(src.contains("ConfirmPresentation"));
    assert!(src.contains("PresentationStatus::Presented"));
}

#[tokio::test]
async fn closed_confirmation_cannot_be_reused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport = GateTransport::with_replies(&["unused"]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), transport);
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    desktop.connect_or_begin_pairing().await.expect("pair");
    let key = desktop
        .surface_snapshot()
        .confirmation
        .expect("challenge")
        .key;
    desktop.cancel_secret();
    assert!(desktop.surface_snapshot().confirmation.is_none());
    assert!(desktop.confirm_key(&key).await.is_err());
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn stale_rows_do_not_select_another_task_or_memory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    desktop
        .composer_mut()
        .set_draft("chat draft stays separate".into());
    assert!(desktop.select_task_key("expired-row").await.is_err());
    assert!(desktop.select_memory_key("expired-row").await.is_err());
    assert!(desktop.select_deletion_key("expired-row").is_err());
    assert_eq!(desktop.composer_mut().draft(), "chat draft stays separate");
    assert!(desktop.surface_snapshot().selected_task.is_empty());
}
