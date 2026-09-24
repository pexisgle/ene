#![cfg(any(unix, windows))]

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use ene_api::v1::management::ManagementOutcome;
use ene_api::v1::round::PresentationStatus;
use ene_companion::{CompanionRepository as _, UNDELIVERED_PAGE_MAX, UndeliveredRepository as _};
use ene_core::host_control;
use ene_core::serve::HostHandle;
use ene_desktop::body_supervise::BodySupervisor;
use ene_desktop::session::{self, ChatSendReport, ChatSessionOutcome, ChatStreamEnd};
use ene_desktop::ui::{DesktopRuntime, Page};
use ene_inference::{
    InferenceTechnicalError, ProviderRequest, ProviderResponse, ProviderTransport,
};
use ene_local_control::{ControlOutcome, DeletionOutcome, FromConfirmation};

mod common;

use common::{ServingTask, drive_gui_until, open_host, wait_for_control};

const MODEL: &str = "gpt-slice-test";
const SECRET: &str = "sk-stage7-e-secret-5519";
const TARGET: &str = "stage7-e-keyword-omega";

struct GateTransport {
    replies: Mutex<VecDeque<String>>,
    sends: AtomicUsize,
    fail_next_response: AtomicBool,
}

impl GateTransport {
    fn with_replies(replies: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.iter().map(|text| (*text).to_string()).collect()),
            sends: AtomicUsize::new(0),
            fail_next_response: AtomicBool::new(false),
        })
    }

    fn sends(&self) -> usize {
        self.sends.load(Ordering::SeqCst)
    }

    fn fail_next_response(&self) {
        self.fail_next_response.store(true, Ordering::SeqCst);
    }
}

impl ProviderTransport for GateTransport {
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
        let _ = req;
        Box::pin(async move {
            self.sends.fetch_add(1, Ordering::SeqCst);
            if self.fail_next_response.swap(false, Ordering::SeqCst) {
                return Err(InferenceTechnicalError::ProviderTransportFailed(
                    String::from("fixture provider response was lost"),
                ));
            }
            let reply = self
                .replies
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop_front()
                .unwrap_or_else(|| String::from("ok"));
            let response = ProviderResponse {
                text: reply,
                usage: None,
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
}

async fn pair_and_setup(desktop: &mut DesktopRuntime, handle: &Arc<HostHandle>) {
    common::pair_and_setup(desktop, handle, SECRET, MODEL).await;
}

#[expect(clippy::expect_used, reason = "test fixture helper")]
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

fn snapshot_has_target(desktop: &DesktopRuntime) -> bool {
    let snap = desktop.snapshot();
    snap.contains_secret(TARGET)
        || snap.timeline.iter().any(|line| line.contains(TARGET))
        || snap.history.iter().any(|line| line.contains(TARGET))
        || snap.draft.contains(TARGET)
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
    pair_and_setup(&mut desktop, &handle).await;
    desktop
        .composer_mut()
        .set_draft(format!("please remember {TARGET}"));
    desktop.send_text().await.expect("plant the target");
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
        desktop.has_chat_receipt(),
        "presenting chat stores a receipt the demand must invalidate"
    );

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
        .refresh_deletion_requests()
        .await
        .expect("the staged request must list");
    let staged = handle
        .pending_targeted_deletions(None, 10)
        .await
        .expect("the staged request must read");
    let request_key = format!(
        "request:{}",
        staged[0].request().as_raw().as_uuid().as_hyphenated()
    );
    desktop
        .select_deletion_key(&request_key)
        .expect("the staged request must select");
    desktop
        .begin_deletion_confirm()
        .await
        .expect("seated confirm is required");
    match desktop
        .confirm_owner()
        .await
        .expect("owner confirm starts deletion")
    {
        FromConfirmation::Outcome(ControlOutcome::Deletion(DeletionOutcome::Started {
            ..
        })) => {}
        other => panic!("expected DeletionStarted, got {other:?}"),
    }
    drive_gui_until(&mut desktop, &handle, "completed").await;

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
    pair_and_setup(&mut desktop, &handle).await;

    desktop
        .composer_mut()
        .set_draft(String::from("please answer"));
    let Some(text) = desktop.composer_mut().take_sendable() else {
        panic!("draft must send");
    };
    let mut client = desktop.take_client().expect("paired client");
    let outcome = session::submit_and_collect(&mut client, &text, "en")
        .await
        .expect("receive stream");
    let ChatSessionOutcome::Completed(turn) = outcome else {
        panic!("the fixture stream must complete: {outcome:?}");
    };
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
async fn registered_secret_never_appears_on_any_page() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport = GateTransport::with_replies(&["unused"]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_setup(&mut desktop, &handle).await;
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
    let asset = dir.path().join(ene_desktop::BUNDLED_SAMPLE_ASSET);
    std::fs::create_dir_all(asset.parent().expect("asset parent")).expect("asset directory");
    std::fs::write(&asset, b"invalid isolation fixture").expect("asset fixture");
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_setup(&mut desktop, &handle).await;

    let Some(exe) = BodySupervisor::locate_binary() else {
        panic!("ene-body binary must be built for Body isolation");
    };
    desktop.try_spawn_body(&exe);
    desktop.tick();
    let spawned = desktop.snapshot().body_status;
    assert_ne!(
        spawned, "Absent",
        "the Body binary must launch rather than be missing: {spawned}"
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
        matches!(
            cancel,
            Err(ene_desktop::ui::DesktopError::Protocol(ref message))
                if message.contains("displayed task")
        ),
        "cancel path stays reachable and refuses without a displayed task: {cancel:?}"
    );
    assert_eq!(transport.sends(), 1);
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn send_text_presents_its_collected_turn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport = GateTransport::with_replies(&["ack me"]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_setup(&mut desktop, &handle).await;

    desktop.composer_mut().set_draft(String::from("ack me"));
    desktop.send_text().await.expect("send_text");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if unpresented_count(&handle).await == 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "send_text must present its collected turn"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn interrupted_provider_stream_keeps_the_typed_outcome_and_discards_partial_reply() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport = GateTransport::with_replies(&["partial reply"]);
    transport.fail_next_response();
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_setup(&mut desktop, &handle).await;

    desktop
        .composer_mut()
        .set_draft(String::from("do not lose this input"));
    let result = desktop.send_text().await;
    assert!(
        matches!(
            result,
            Ok(ChatSendReport::StreamEnded(ChatStreamEnd::Interrupted))
        ),
        "an interrupted provider stream must retain its close reason: {result:?}"
    );
    let snapshot = desktop.snapshot();
    assert!(
        snapshot
            .timeline
            .iter()
            .any(|line| line == "[owner] do not lose this input")
    );
    assert!(
        !snapshot
            .timeline
            .iter()
            .any(|line| line.contains("partial reply"))
    );
    assert_eq!(transport.sends(), 1);
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn closed_confirmation_cannot_be_reused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport = GateTransport::with_replies(&["unused"]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), transport);
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    let channel = host_control::seat_test_gui_for_tests(&handle).expect("private channel");
    desktop
        .attach_confirmation(channel)
        .expect("the private channel is the seat");
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
