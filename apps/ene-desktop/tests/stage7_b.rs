//! Stage 7 B: first-party desktop acceptance §1 against a real Host.
//!
//! Provider is fake. GUI toolkit is not displayed; the same [`DesktopRuntime`]
//! the Slint window projects is driven here. The confirmation channel uses the
//! Host registration path; the requester listener cannot acquire an empty
//! seat. Windows 11 / NixOS 26.11 / IME probes: 未実施.

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

use ene_core::host_control;
use ene_credential::{CredentialPublicationRepository as _, CredentialSetRepository as _};
use ene_desktop::ui::{Composer, DesktopRuntime, Page};
use ene_inference::{ProviderRequest, ProviderResponse, ProviderTransport};
use ene_local_control::{ControlOutcome, FromConfirmation};

mod common;

use common::{ServingTask, open_host, pair_and_seat, wait_for_control};

const SECRET: &str = "sk-stage7-b-secret-9931";

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
    let revision = handle
        .store_for_tests()
        .current_set_revision()
        .await
        .expect("publication revision");
    let active = handle
        .store_for_tests()
        .active_credential_version("openai", "main")
        .await
        .expect("active publication record");
    assert!(
        revision.as_u64() > 0 && active.is_some(),
        "CredentialStored is legal only after sweep, revision commit, active ref, and snapshot publication"
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
    first
        .refresh_setup()
        .await
        .expect("the first GUI reads current Host setup state");
    let before_restart = first.surface_snapshot();
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
    let after_restart = restarted.surface_snapshot();
    assert_eq!(after_restart.credential, before_restart.credential);
    assert_eq!(after_restart.consent, before_restart.consent);
    assert_eq!(after_restart.ready, before_restart.ready);
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
    assert_eq!(desktop.snapshot().page, "About");
    desktop.try_spawn_body(&dir.path().join("ene-body-absent"));
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
