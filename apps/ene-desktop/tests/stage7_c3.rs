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
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ene_api::v1::deletion::{DeletionPhaseWire, DeletionStatusResponse};
use ene_api::v1::management::ManagementOutcome;
use ene_core::serve::HostHandle;
use ene_desktop::ui::{DesktopRuntime, Page};
use ene_inference::{ProviderRequest, ProviderResponse, ProviderTransport, RawUsage};
use ene_local_control::{ControlOutcome, DeletionOutcome, FromConfirmation};

mod common;

use common::{ServingTask, drive_gui_until, open_host, wait_for_control};

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
            let response = ProviderResponse { text: reply, usage };
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
    pair_and_setup(&mut desktop, &handle).await;

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
    pair_and_setup(&mut desktop, &handle).await;
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
    assert_eq!(desktop.snapshot().page, "Confirm");
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
async fn finalizing_is_distinct_from_completed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport =
        GateTransport::with_script(&[(&format!("I will keep {TARGET} in mind."), None)]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_setup(&mut desktop, &handle).await;
    desktop
        .composer_mut()
        .set_draft(format!("please remember {TARGET}"));
    desktop.send_text().await.expect("plant the target");
    desktop.set_deletion_exact_text(String::from(TARGET));
    desktop.request_deletion().await.expect("stage");
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
        Ok(FromConfirmation::Outcome(ControlOutcome::Deletion(DeletionOutcome::Started {
            ..
        }))) => {}
        other => panic!("expected DeletionStarted, got {other:?}"),
    }
    let mut desktop = Arc::try_unwrap(desktop)
        .unwrap_or_else(|_| panic!("confirm task must drop its runtime clone"))
        .into_inner();
    drive_gui_until(&mut desktop, &handle, "completed").await;
    assert_eq!(desktop.deletion_phase_token(), Some("completed"));
    server.shutdown_and_join().await;
}
