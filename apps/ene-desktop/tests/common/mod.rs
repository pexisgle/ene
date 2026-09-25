#![allow(
    dead_code,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "each integration-test binary compiles this module alone; helpers outside #[test] functions need the fixture allowances clippy.toml grants only to test functions"
)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ene_api::v1::management::ManagementOutcome;
use ene_core::conn;
use ene_core::host_control;
use ene_core::serve::{CoreError, CredStore, HostHandle};
use ene_credential::MemoryVersionedStore;
use ene_desktop::ui::DesktopRuntime;
use ene_inference::ProviderTransport;
use ene_local_control::{ControlOutcome, FromConfirmation};

pub struct ServingTask {
    shutdown: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<Result<(), CoreError>>,
}

impl ServingTask {
    pub fn start<T>(dir: &Path, handle: Arc<HostHandle>, transport: Arc<T>) -> Self
    where
        T: ProviderTransport + Send + Sync + 'static,
    {
        let (shutdown, rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(conn::run_until_shutdown(
            dir.to_path_buf(),
            handle,
            transport,
            rx,
        ));
        Self { shutdown, task }
    }

    pub async fn shutdown_and_join(self) {
        self.shutdown.send_replace(true);
        tokio::time::timeout(Duration::from_secs(30), self.task)
            .await
            .expect("serving shutdown must drain")
            .expect("serving task must join")
            .expect("serving shutdown must succeed");
    }
}

pub async fn open_host(dir: &Path) -> Arc<HostHandle> {
    match HostHandle::open_with_cred_store(
        dir,
        CredStore::MemoryVersioned(MemoryVersionedStore::new()),
    )
    .await
    {
        Ok(handle) => {
            handle.set_client_erasure_wait_for_tests(Duration::from_millis(200));
            Arc::new(handle)
        }
        Err(error) => panic!("host must open: {error}"),
    }
}

pub async fn wait_for_control(dir: &Path) -> bool {
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

pub async fn pair_and_seat(desktop: &mut DesktopRuntime, handle: &Arc<HostHandle>) {
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

pub async fn say(desktop: &mut DesktopRuntime, text: &str) {
    desktop.composer_mut().set_draft(text.to_owned());
    desktop.send_text().await.expect("chat turn must complete");
}

pub async fn complete_setup(desktop: &mut DesktopRuntime, secret: &str, model: &str) {
    desktop.set_secret(secret.to_string());
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
    desktop.set_model(model.to_string());
    let assigned = desktop.assign_model().await.expect("dialogue assign");
    assert!(
        matches!(assigned, ManagementOutcome::StoredAsRuleView { .. }),
        "dialogue assignment must store, got {assigned:?}"
    );
}

pub async fn pair_and_setup(
    desktop: &mut DesktopRuntime,
    handle: &Arc<HostHandle>,
    secret: &str,
    model: &str,
) {
    pair_and_seat(desktop, handle).await;
    complete_setup(desktop, secret, model).await;
}

pub async fn drive_gui_until(desktop: &mut DesktopRuntime, handle: &HostHandle, needle: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(150);
    loop {
        match desktop.refresh_deletion().await {
            Ok(()) | Err(_) => {}
        }
        if desktop.snapshot().deletion_body.contains(needle) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "GUI deletion body never contained {needle}: {}",
            desktop.snapshot().deletion_body
        );
        handle.wake_deletion_driver_for_tests();
        let mut drive = std::pin::pin!(handle.run_targeted_deletion_tick());
        loop {
            tokio::select! {
                driven = &mut drive => {
                    match driven {
                        Ok(_) | Err(_) => {}
                    }
                    break;
                }
                () = tokio::time::sleep(Duration::from_millis(5)) => {
                    if tokio::time::Instant::now() >= deadline {
                        break;
                    }
                    match desktop.refresh_deletion().await {
                        Ok(()) | Err(_) => {}
                    }
                    if desktop.snapshot().deletion_body.contains(needle) {
                        return;
                    }
                }
            }
        }
    }
}
