//! Stage 7 C3: usage / cap and Targeted Deletion management GUI.
//!
//! Provider is fake. The same [`DesktopRuntime`] the Slint window projects is
//! driven here against a real Host. GUI never reads `app.db`. Windows 11 /
//! NixOS 26.11 / IME probes: 未実施.

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

use ene_api::v1::deletion::{DeletionPhaseWire, DeletionStatusResponse};
use ene_api::v1::management::ManagementOutcome;
use ene_core::conn;
use ene_core::host_control;
use ene_core::serve::{CoreError, CredStore, HostHandle};
use ene_credential::MemoryCredentialStore;
use ene_desktop::ui::{DesktopRuntime, Page};
use ene_inference::{ProviderRequest, ProviderResponse, ProviderTransport, RawUsage};
use ene_local_control::{ControlOutcome, FromHost};

const MODEL: &str = "gpt-4o-mini";
const SECRET: &str = "sk-stage7-c3-secret-4402";
const TARGET: &str = "stage7-c3-keyword-alpha";

struct GateTransport {
    replies: Mutex<VecDeque<String>>,
    usages: Mutex<VecDeque<Option<RawUsage>>>,
    sends: AtomicUsize,
}

impl GateTransport {
    fn with_script(items: &[(&str, Option<RawUsage>)]) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(items.iter().map(|(text, _)| (*text).to_string()).collect()),
            usages: Mutex::new(items.iter().map(|(_, usage)| *usage).collect()),
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
            let usage = self
                .usages
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop_front()
                .flatten();
            Ok(ProviderResponse { text: reply, usage })
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
        if ene_core::host_control::ControlClient::connect(dir)
            .await
            .is_ok()
        {
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
        .begin_pairing()
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

fn reported_usage() -> RawUsage {
    RawUsage {
        input_tokens: 1_000_000,
        cached_input_tokens: 0,
        output_tokens: 1_000_000,
    }
}

async fn local_deletion_page(handle: &HostHandle) -> ene_api::v1::deletion::DeletionStatusPage {
    match handle
        .deletion_status_page(None, 20)
        .await
        .expect("the local status must answer")
    {
        DeletionStatusResponse::Page(page) => page,
        other => panic!("the local status must answer a page: {other:?}"),
    }
}

async fn wait_handle_phase(handle: &HostHandle, wanted: DeletionPhaseWire) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(150);
    loop {
        let page = local_deletion_page(handle).await;
        if page.operations.first().map(|view| view.phase) == Some(wanted) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the deletion operation did not reach {}: {page:?}",
            wanted.as_str()
        );
        match handle.run_targeted_deletion_tick().await {
            Ok(_) | Err(_) => {}
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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

#[tokio::test]
async fn unknown_cost_is_not_yen_zero_and_stale_cap_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport = GateTransport::with_script(&[
        ("unknown-reply", None),
        ("reported-reply", Some(reported_usage())),
    ]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_setup(&mut desktop).await;

    desktop.open_page(Page::Usage);
    desktop.set_usage_period(None, None);
    desktop.set_usage_attribution(
        Some(String::from("openai")),
        Some(String::from(MODEL)),
        Some(String::from("companion_dialogue")),
        Some(String::from("dialogue_response")),
    );
    desktop.set_usage_cap_slot(
        String::from("system"),
        None,
        String::from("daily_utc"),
        String::from("USD"),
    );
    desktop.set_usage_status_filter(None);
    desktop
        .composer_mut()
        .set_draft(String::from("first usage turn"));
    desktop.send_text().await.expect("unknown-cost chat");
    desktop
        .composer_mut()
        .set_draft(String::from("second usage turn"));
    desktop.send_text().await.expect("reported-cost chat");
    assert_eq!(transport.sends(), 2);

    desktop.refresh_usage().await.expect("usage query");
    desktop
        .next_usage_page()
        .await
        .expect("next page is a no-op without a cursor");
    let snap = desktop.snapshot();
    assert!(
        snap.usage_body.contains("unknown"),
        "unknown cost must stay visible: {}",
        snap.usage_body
    );
    assert!(
        !snap.usage_body.contains("¥0") && !snap.usage_body.contains("¥ 0"),
        "unknown must never display as yen zero: {}",
        snap.usage_body
    );
    assert!(
        desktop.usage_has_unknown_cost(),
        "a missing provider report is unknown, not zero"
    );
    assert!(
        snap.usage_body.contains("USD:750000") || snap.usage_body.contains("750000"),
        "reported gpt-4o-mini cost must appear as micros, not yen: {}",
        snap.usage_body
    );
    assert!(
        snap.usage_body.contains("not admit authority"),
        "remaining is display-only: {}",
        snap.usage_body
    );
    assert!(
        snap.usage_body.contains("companion_dialogue")
            && snap.usage_body.contains("dialogue_response")
            && snap.usage_body.contains(MODEL),
        "attribution must name provider/model/consumer/purpose: {}",
        snap.usage_body
    );

    desktop.set_usage_cap_limit_micros(2_000_000);
    let first = desktop.apply_usage_cap().await.expect("cap apply");
    assert!(
        matches!(first, ManagementOutcome::StoredAsRuleView { .. }),
        "the first cap write must store, got {first:?}"
    );
    let stale = desktop
        .apply_usage_cap()
        .await
        .expect("stale cap is a domain outcome");
    assert!(
        matches!(stale, ManagementOutcome::StaleBaseView { .. }),
        "a stale cap view must be refused, got {stale:?}"
    );
    let snap = desktop.snapshot();
    assert!(
        snap.deny_reason.contains("stale")
            || snap.deny_reason.contains("Stale")
            || snap.deny_reason.contains("古い"),
        "the GUI must surface the stale rejection: {}",
        snap.deny_reason
    );
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn secrets_stay_out_and_client_confirmed_true_cannot_delete() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport = GateTransport::with_script(&[("unused", None)]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_setup(&mut desktop).await;

    desktop.open_page(Page::Deletion);
    desktop.set_deletion_purpose(ene_api::v1::deletion::DeletionPurposeWire::Privacy);
    desktop.set_deletion_exact_text(String::from(TARGET));
    let snap = desktop.snapshot();
    assert!(
        !snap.contains_secret(SECRET),
        "registered secret must not appear in the GUI snapshot"
    );
    assert!(
        !snap.contains_secret(TARGET),
        "deletion exact text must not appear in the GUI snapshot"
    );
    assert!(
        !snap.deletion_body.contains(TARGET),
        "status projection must omit the target body: {}",
        snap.deletion_body
    );
    let debug = format!("{snap:?}");
    assert!(!debug.contains(SECRET), "Debug of snapshot must not leak");
    assert!(
        !debug.contains(TARGET),
        "Debug of snapshot must not carry the deletion body"
    );

    let denied = desktop
        .deletion_confirmed_true()
        .await
        .expect("client confirmed=true is a domain outcome");
    assert!(
        matches!(denied, ManagementOutcome::DeniedByBoundary),
        "Client confirmed=true must deny, got {denied:?}"
    );
    let control_denied = desktop
        .send_confirmed_true_on_control()
        .await
        .expect("control ConfirmedTrue");
    assert!(
        matches!(control_denied, FromHost::DeniedByBoundary),
        "session-less ConfirmedTrue must deny"
    );
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn seated_confirm_completes_targeted_deletion() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport =
        GateTransport::with_script(&[(&format!("I will keep {TARGET} in mind."), None)]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_setup(&mut desktop).await;
    desktop
        .composer_mut()
        .set_draft(format!("please remember {TARGET}"));
    desktop.send_text().await.expect("plant the target");

    desktop.open_page(Page::Deletion);
    desktop.set_deletion_exact_text(String::from(TARGET));
    let staged = desktop.request_deletion().await.expect("stage");
    assert_eq!(
        staged,
        ManagementOutcome::NeedsClarification,
        "Client intent only stages, got {staged:?}"
    );
    let snap = desktop.snapshot();
    assert!(
        !snap.deletion_body.contains(TARGET),
        "the typed target is wiped from the panel after the request: {}",
        snap.deletion_body
    );
    assert!(
        snap.deletion_body
            .contains("distinct from conversational forget"),
        "Targeted Deletion stays a separate surface: {}",
        snap.deletion_body
    );

    desktop
        .begin_deletion_confirm()
        .await
        .expect("seated confirm is required");
    assert_eq!(desktop.snapshot().page, "Confirm");
    match desktop
        .confirm_owner()
        .await
        .expect("owner confirm starts deletion")
    {
        FromHost::Outcome(ControlOutcome::DeletionStarted { .. }) => {}
        other => panic!("expected DeletionStarted, got {other:?}"),
    }
    drive_gui_until(&mut desktop, &handle, "completed").await;
    let snap = desktop.snapshot();
    assert!(
        snap.deletion_body.contains("completed"),
        "GUI must show Completed: {}",
        snap.deletion_body
    );
    assert_ne!(
        desktop.deletion_phase_token(),
        Some("held"),
        "a pumped Client is not an unreachable hold"
    );
    assert!(
        !snap.contains_secret(TARGET),
        "completed deletion must not leave the target in the snapshot"
    );
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn unreachable_client_is_not_completion_and_resume_is_explicit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport =
        GateTransport::with_script(&[(&format!("I will keep {TARGET} in mind."), None)]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_setup(&mut desktop).await;
    desktop
        .composer_mut()
        .set_draft(format!("please remember {TARGET}"));
    desktop.send_text().await.expect("plant the target");

    desktop.set_deletion_exact_text(String::from(TARGET));
    let staged = desktop.request_deletion().await.expect("stage");
    assert_eq!(staged, ManagementOutcome::NeedsClarification);

    drop(desktop.take_client());
    desktop
        .begin_deletion_confirm()
        .await
        .expect("confirm is Host-local");
    match desktop
        .confirm_owner()
        .await
        .expect("admission still starts")
    {
        FromHost::Outcome(ControlOutcome::DeletionStarted { .. }) => {}
        other => panic!("expected DeletionStarted, got {other:?}"),
    }
    wait_handle_phase(&handle, DeletionPhaseWire::Held).await;
    let page = local_deletion_page(&handle).await;
    assert_ne!(
        page.operations[0].phase,
        DeletionPhaseWire::Completed,
        "an unreachable Client must never be presumed erased"
    );
    let snap = desktop.snapshot();
    assert!(
        !snap.deletion_body.contains("completed"),
        "GUI must not treat disconnect as completion: {}",
        snap.deletion_body
    );

    desktop.reconnect().await.expect("replacement connection");
    desktop
        .refresh_deletion()
        .await
        .expect("status after reconnect");
    if desktop.deletion_phase_token() == Some("held") {
        match desktop.resume_deletion().await.expect("explicit resume") {
            FromHost::Outcome(ControlOutcome::DeletionResumed { .. }) => {}
            other => panic!("expected DeletionResumed, got {other:?}"),
        }
        assert!(
            desktop.snapshot().deletion_body.contains("resumed")
                || desktop.deletion_phase_token() != Some("held"),
            "explicit resume must move a Held(Unavailable) operation: {}",
            desktop.snapshot().deletion_body
        );
    }
    drive_gui_until(&mut desktop, &handle, "completed").await;
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn finalizing_is_distinct_from_completed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport =
        GateTransport::with_script(&[(&format!("I will keep {TARGET} in mind."), None)]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_setup(&mut desktop).await;
    desktop
        .composer_mut()
        .set_draft(format!("please remember {TARGET}"));
    desktop.send_text().await.expect("plant the target");
    desktop.set_deletion_exact_text(String::from(TARGET));
    desktop.request_deletion().await.expect("stage");
    desktop.begin_deletion_confirm().await.expect("challenge");

    handle.arm_deletion_finalizing_park_for_tests();
    let desktop = Arc::new(tokio::sync::Mutex::new(desktop));
    let task_desktop = Arc::clone(&desktop);
    let confirm = tokio::spawn(async move {
        let mut guard = task_desktop.lock().await;
        guard.confirm_owner().await
    });
    tokio::time::timeout(
        Duration::from_secs(150),
        handle.wait_deletion_finalizing_park_for_tests(),
    )
    .await
    .expect("finalizing park must be entered");
    let page = local_deletion_page(&handle).await;
    assert_ne!(
        page.operations[0].phase,
        DeletionPhaseWire::Completed,
        "the sealed boundary must not complete while parked: {page:?}"
    );
    handle
        .begin_deletion_finalizing_for_tests(
            &page.operations[0].operation.0,
            page.operations[0].sweep,
        )
        .await
        .expect("the Finalizing marker must commit");
    let page = local_deletion_page(&handle).await;
    assert_eq!(
        page.operations[0].phase,
        DeletionPhaseWire::Finalizing,
        "the sealed boundary is Finalizing, not Completed: {page:?}"
    );
    assert_ne!(page.operations[0].phase, DeletionPhaseWire::Completed);
    handle.release_deletion_finalizing_park_for_tests();
    match confirm.await.expect("confirm task joins") {
        Ok(FromHost::Outcome(ControlOutcome::DeletionStarted { .. })) => {}
        other => panic!("expected DeletionStarted, got {other:?}"),
    }
    let mut desktop = Arc::try_unwrap(desktop)
        .unwrap_or_else(|_| panic!("confirm task must drop its runtime clone"))
        .into_inner();
    drive_gui_until(&mut desktop, &handle, "completed").await;
    assert_eq!(desktop.deletion_phase_token(), Some("completed"));
    server.shutdown_and_join().await;
}
