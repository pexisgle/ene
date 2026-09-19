//! Stage 7 B: first-party desktop acceptance §1 against a real Host.
//!
//! Provider is fake. GUI toolkit is not displayed; the same [`DesktopRuntime`]
//! the Slint window projects is driven here. Empty-seat occupancy is not
//! treated as authenticity. Windows 11 / NixOS 26.11 / IME probes: 未実施.

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

use ene_api::v1::management::ManagementOutcome;
use ene_core::conn;
use ene_core::host_control;
use ene_core::serve::{CoreError, CredStore, HostHandle};
use ene_credential::MemoryVersionedStore;
use ene_desktop::i18n::Locale;
use ene_desktop::ui::{Composer, DesktopRuntime, Page};
use ene_desktop::{DESKTOP_DESCRIPTOR, session};
use ene_inference::{ProviderRequest, ProviderResponse, ProviderTransport};
use ene_local_control::{ControlOutcome, FromConfirmation};

const MODEL: &str = "gpt-slice-test";
const SECRET: &str = "sk-stage7-b-secret-9931";

struct GateTransport {
    replies: Mutex<VecDeque<String>>,
    sends: AtomicUsize,
    park: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

impl GateTransport {
    fn with_replies(replies: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.iter().map(|text| (*text).to_string()).collect()),
            sends: AtomicUsize::new(0),
            park: Mutex::new(None),
        })
    }

    fn sends(&self) -> usize {
        self.sends.load(Ordering::SeqCst)
    }

    fn park_next(&self) -> tokio::sync::oneshot::Sender<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        *self
            .park
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(rx);
        tx
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
            let parked = self
                .park
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            if let Some(rx) = parked {
                match rx.await {
                    Ok(()) | Err(_) => {}
                }
            }
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
    match HostHandle::open_with_cred_store(
        dir,
        CredStore::MemoryVersioned(MemoryVersionedStore::new()),
    )
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

/// Registers this runtime as the GUI the Host spawned, then pairs it.
///
/// The private channel is the seat: the test adopts it through the same
/// registration path the Host uses for its own child, so no test takes a seat
/// from a public endpoint. The Owner's direct gesture is still the test's.
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

#[tokio::test]
async fn register_only_makes_zero_provider_calls() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport = GateTransport::with_replies(&["must-not-run"]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_seat(&mut desktop, &handle).await;
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
    desktop.refresh_setup().await.expect("setup view must read");
    assert!(
        handle.credential_contains_for_tests("openai", "main"),
        "control put must store the credential"
    );
    let snap = desktop.snapshot();
    assert!(
        !snap.setup_ready,
        "register-only must not invent assignment consent"
    );
    assert!(
        !snap.contains_secret(SECRET),
        "registered secret must not appear in GUI state"
    );
    assert_eq!(
        transport.sends(),
        0,
        "register-only makes zero provider calls"
    );
    server.shutdown_and_join().await;
}

/// A restarted GUI reconnects as the device it already paired, instead of
/// reopening a pairing request the Host would have to approve again.
///
/// This is the reported P1: the restart used to dial, succeed, and turn that
/// success into the error "pairing was expected to pend", leaving the window on
/// the language page with `connecting / unknown` forever. A fresh runtime on
/// the same data directory is what the product restart is.
#[tokio::test]
async fn a_restarted_gui_reconnects_instead_of_reopening_pairing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport = GateTransport::with_replies(&["still here"]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);

    // First run: pair, so a device file exists for the restart to use.
    let mut first = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_seat(&mut first, &handle).await;
    assert!(
        first.surface_snapshot().connected,
        "the paired GUI must be connected"
    );
    drop(first);

    // Restart: a new process with the same data directory, which is what the
    // user sees when they close and reopen the GUI while the Host keeps
    // serving.
    let channel = host_control::seat_test_gui_for_tests(&handle).expect("private channel");
    let mut restarted = DesktopRuntime::new(dir.path().to_path_buf());
    restarted
        .attach_confirmation(channel)
        .expect("the private channel is the seat");
    restarted
        .connect_or_begin_pairing()
        .await
        .expect("a paired GUI must reconnect");
    assert!(
        restarted.surface_snapshot().connected,
        "the restart must present the Host's current state, not a pending pairing"
    );
    assert_ne!(
        restarted.snapshot().page,
        "Confirm",
        "an already-paired GUI must not ask the Owner to confirm pairing again"
    );
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn assignment_then_chat_and_locale_and_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport = GateTransport::with_replies(&["hello from ene"]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_seat(&mut desktop, &handle).await;
    desktop.set_secret(String::from(SECRET));
    desktop.begin_credential_put().await.expect("put");
    desktop.confirm_owner().await.expect("store");
    desktop.set_model(String::from(MODEL));
    let assigned = desktop.assign_model().await.expect("assign");
    assert!(
        matches!(assigned, ManagementOutcome::StoredAsRuleView { .. }),
        "assignment must store, got {assigned:?}"
    );
    assert!(desktop.snapshot().setup_ready);
    desktop.composer_mut().set_draft(String::from("hi there"));
    desktop.send_text().await.expect("chat after assignment");
    assert_eq!(transport.sends(), 1);
    let before = desktop.snapshot().history.clone();
    assert!(
        before.iter().any(|line| line.contains("hi there")),
        "history must keep owner text: {before:?}"
    );
    desktop.set_locale(Locale::En);
    desktop
        .refresh_history()
        .await
        .expect("history after locale");
    let after = desktop.snapshot().history.clone();
    assert_eq!(
        before, after,
        "JA/EN must not rewrite history or domain state"
    );
    assert_eq!(desktop.snapshot().locale, "en");

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
async fn secrets_stay_out_of_gui_state_and_seated_confirm_denies_client_true() {
    let dir = tempfile::tempdir().expect("tempdir");
    let transport = GateTransport::with_replies(&["unused"]);
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_seat(&mut desktop, &handle).await;
    desktop.set_secret(String::from(SECRET));
    let snap = desktop.snapshot();
    assert!(
        !snap.contains_secret(SECRET),
        "C1 buffer must not appear in the GUI snapshot"
    );
    desktop.begin_credential_put().await.expect("put");
    let snap = desktop.snapshot();
    assert!(
        !snap.contains_secret(SECRET),
        "control challenge must not copy the secret into the snapshot"
    );
    desktop.confirm_owner().await.expect("store");
    desktop.cancel_secret();
    let snap = desktop.snapshot();
    assert!(!snap.contains_secret(SECRET));
    let debug = format!("{snap:?}");
    assert!(!debug.contains(SECRET), "Debug of snapshot must not leak");

    let denied = desktop
        .client_confirmed_true()
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
        matches!(control_denied, FromConfirmation::DeniedByBoundary),
        "session-less ConfirmedTrue must deny"
    );
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn gui_event_loop_is_not_blocked_on_connect_or_provider_wait() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    let connecting = tokio::spawn({
        let dir = dir.path().to_path_buf();
        async move {
            loop {
                match session::connect(&dir, DESKTOP_DESCRIPTOR, None).await {
                    Ok(client) => return Ok(client),
                    Err(ene_client::error::ClientError::Transport(_)) => {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    });
    for _ in 0..12 {
        desktop.tick();
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    assert!(
        desktop.ui_ticks() >= 12,
        "ticks must advance while connect waits for Host"
    );

    let transport = GateTransport::with_replies(&["parked-reply"]);
    let release = transport.park_next();
    let handle = open_host(dir.path()).await;
    let server = ServingTask::start(dir.path(), Arc::clone(&handle), Arc::clone(&transport));
    assert!(wait_for_control(dir.path()).await);
    connecting.abort();

    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    pair_and_seat(&mut desktop, &handle).await;
    desktop.set_secret(String::from(SECRET));
    desktop.begin_credential_put().await.expect("put");
    desktop.confirm_owner().await.expect("store");
    desktop.set_model(String::from(MODEL));
    let assigned = desktop.assign_model().await.expect("assign");
    assert!(
        matches!(assigned, ManagementOutcome::StoredAsRuleView { .. }),
        "parked chat requires assignment, got {assigned:?}"
    );
    desktop
        .composer_mut()
        .set_draft(String::from("while parked"));
    let Some(text) = desktop.composer_mut().take_sendable() else {
        panic!("draft must send");
    };
    let mut client = desktop.take_client().expect("paired client");
    let send_task = tokio::spawn(async move {
        let result = session::submit_and_collect(&mut client, &text, "en").await;
        (client, result)
    });
    for _ in 0..10 {
        desktop.tick();
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    assert!(
        desktop.ui_ticks() >= 10,
        "ticks must advance while the provider is parked"
    );
    release.send(()).expect("release provider");
    let (client, result) = send_task.await.expect("send task joins");
    result.expect("parked chat completes");
    desktop.restore_client(client);
    desktop
        .refresh_management_without_body()
        .await
        .expect("settings work without avatar");
    assert_eq!(desktop.snapshot().body_status, "Absent");
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn ime_and_about_slint_and_missing_body() {
    let mut composer = Composer::default();
    composer.begin_composition();
    composer.set_draft(String::from("should-not-stick"));
    assert!(composer.take_sendable().is_none());
    composer.end_composition(Some(String::from("done")));
    assert_eq!(composer.take_sendable().as_deref(), Some("done"));

    let dir = tempfile::tempdir().expect("tempdir");
    let mut desktop = DesktopRuntime::new(dir.path().to_path_buf());
    desktop.open_page(Page::About);
    assert!(desktop.snapshot().about_slint);
    desktop.try_spawn_body(&desktop.bundled_ene_asset());
    assert_eq!(desktop.snapshot().body_status, "Absent");
    desktop.wizard_next();
    desktop.wizard_next();
    desktop.wizard_next();
    assert!(
        desktop.snapshot().secret_visible,
        "credential step is the secret field"
    );
    assert!(
        !desktop.snapshot().setup_ready,
        "wizard steps must not invent consent"
    );
}

#[test]
fn the_seat_comes_from_the_host_spawn_not_from_a_connection() {
    // Compile-time reminder: a public connection can no longer take a seat.
    // The spawn-derived seat and its generation rules are covered by
    // stage7_a1; B reuses the Host-spawned registration path.
    let _ = host_control::seat_test_gui_for_tests;
}
