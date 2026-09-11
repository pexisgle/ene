//! Production-path end to end for Stage 2.
//!
//! Real listener socket, real `ene-ctl` client builders and session, real
//! Host orchestration; only the provider HTTP transport is fake. Covers
//! cross-process pairing approval (an INDEPENDENT approval context sharing
//! the file device-auth store), the full challenge/proof handshake, setup,
//! rounds with ordered streaming, restart without re-approval, rotation,
//! tampering, and untrusted-peer denial.
//!
//! Unix-only: the production listener is a Unix socket (Windows uses named
//! pipes in a follow-up).

#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration-test helpers outside #[test] functions need the fixture allowances clippy.toml grants only to test functions"
)]

use std::sync::Arc;
use std::time::Duration;

use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome, RationaleOrigin,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{BaseViewMark, CommandWireId, DeviceWireId, ManagementTargetWire};
use ene_api::v1::round::{PresentationStatus, RoundIntakeOutcomeWire, StreamClose};
use ene_companion::{CompanionRepository, UndeliveredRepository};
use ene_core::conn;
use ene_core::serve::{CredStore, HostHandle};
use ene_credential::{CredentialRef, MemoryCredentialStore};
use ene_ctl::client::Client;
use ene_ctl::cmds;
use ene_ctl::device::{StoredDevice, store_device};
use ene_ctl::errors::CliError;
use ene_inference::RawUsage;
use ene_inference::fake::FakeProviderTransport;
use ene_store::Store;

const DESCRIPTOR: &str = "e2e laptop";
const MODEL: &str = "gpt-slice-test";
const FAKE_TEXT: &str = "hello back over the real socket";

fn memory_store() -> MemoryCredentialStore {
    let store = MemoryCredentialStore::new();
    store.insert(
        CredentialRef::new("openai", "main").expect("valid test fixture"),
        "sk-test-only",
    );
    store
}

fn fake_transport() -> FakeProviderTransport {
    FakeProviderTransport::new(
        String::from(FAKE_TEXT),
        Some(RawUsage {
            input_tokens: 7,
            output_tokens: 3,
        }),
    )
}

async fn open_host(dir: &std::path::Path) -> Option<Arc<HostHandle>> {
    let opened = HostHandle::open_with_cred_store(dir, CredStore::Memory(memory_store())).await;
    assert!(opened.is_ok(), "host must open");
    let Ok(handle) = opened else {
        return None;
    };
    Some(Arc::new(handle))
}

async fn wait_for_socket(dir: &std::path::Path) -> bool {
    for _ in 0..100 {
        if dir.join("ene.sock").exists() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

async fn ask(client: &mut Client, payload: WirePayload, what: &str) -> Result<WirePayload, String> {
    match tokio::time::timeout(Duration::from_secs(10), client.request(payload)).await {
        Ok(Ok(answer)) => Ok(answer),
        Ok(Err(error)) => Err(format!("{what} errored: {error:?}")),
        Err(_) => Err(format!("{what} timed out")),
    }
}

fn complete_intent(target: &str, mark: &str) -> WirePayload {
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

async fn view_mark(client: &mut Client) -> Result<String, String> {
    let answer = ask(
        client,
        WirePayload::ManagementViewRequest(cmds::setup_view_request()),
        "show",
    )
    .await;
    assert!(
        matches!(&answer, Ok(WirePayload::ManagementView(_))),
        "show must answer a view: {answer:?}"
    );
    let Ok(WirePayload::ManagementView(view)) = answer else {
        return Err(String::from("show answered nothing usable"));
    };
    Ok(view.mark.0.clone())
}

/// Approves through an INDEPENDENT handle (simulating the separate
/// `approve-device` process) and provisions the device file from the
/// one-time secret, like the operator channel would.
async fn approve_and_provision(dir: &std::path::Path, approver: &HostHandle) -> Result<(), String> {
    let approval = approver
        .approve_device(DESCRIPTOR)
        .await
        .map_err(|error| format!("approve failed: {error:?}"))?;
    let Some((record, secret)) = approval else {
        return Err(String::from("approval must pair"));
    };
    store_device(
        dir,
        &StoredDevice::new(
            record
                .wire
                .parse()
                .map(DeviceWireId)
                .map_err(|error| format!("opaque wire must stay UUID text: {error:?}"))?,
            secret,
        ),
    )
    .map_err(|error| format!("device file must store: {error:?}"))?;
    Ok(())
}

async fn setup_flow(client: &mut Client, approver: &HostHandle) -> Result<(), String> {
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
    .await;
    assert!(
        matches!(
            register,
            Ok(WirePayload::ManagementOutcome(
                ManagementOutcome::HeldByOperation
            ))
        ),
        "unapproved register must hold, got {register:?}"
    );
    let credential_approved = approver.approve_credential("openai", "main").await;
    assert!(
        matches!(credential_approved, Ok(true)),
        "host-local credential approval must succeed"
    );
    let mark = view_mark(client).await?;
    let assign = ask(
        client,
        WirePayload::ManagementIntent(cmds::assignment_intent(
            CommandWireId(uuid::Uuid::new_v4()),
            &BaseViewMark(mark),
            cmds::CAPABILITY_DIALOGUE,
            "openai",
            MODEL,
        )),
        "assign",
    )
    .await;
    assert!(
        matches!(
            assign,
            Ok(WirePayload::ManagementOutcome(
                ManagementOutcome::StoredAsRuleView { .. }
            ))
        ),
        "assign must store, got {assign:?}"
    );
    let mark = view_mark(client).await?;
    let complete = ask(
        &mut *client,
        complete_intent("setup:complete", &mark),
        "complete",
    )
    .await;
    assert!(
        matches!(
            complete,
            Ok(WirePayload::ManagementOutcome(
                ManagementOutcome::AppliedAsOneTime
            ))
        ),
        "complete must apply, got {complete:?}"
    );
    Ok(())
}

async fn send_round(
    client: &mut Client,
    text: &str,
) -> Result<(String, Option<ene_api::v1::refs::StreamWireId>, String), String> {
    // The client echoes the companion projection its session learned from
    // presence (public API end to end); scaffolding never hardcodes it.
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
    .await;
    assert!(
        matches!(
            send,
            Ok(WirePayload::RoundIntakeOutcome(
                RoundIntakeOutcomeWire::AcceptedForRound { .. }
            ))
        ),
        "input must be accepted, got {send:?}"
    );
    let Ok(WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { round })) =
        send
    else {
        return Err(String::from("acceptance vanished"));
    };
    let round_wire = round.0.clone();
    let mut opened = false;
    let mut text_out = String::new();
    let mut previous_seq: Option<u64> = None;
    let mut stream_id = None;
    loop {
        let frame = ask_stream(client).await?;
        match frame {
            WirePayload::TextStreamOpen(open) => {
                opened = true;
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
                assert!(
                    close.status == StreamClose::Completed,
                    "stream must complete, got {:?}",
                    close.status
                );
                break;
            }
            other => {
                assert!(
                    matches!(other, WirePayload::TextStreamClose(_)),
                    "unexpected stream payload: {other:?}"
                );
                return Err(String::from("unexpected stream payload"));
            }
        }
    }
    assert!(opened, "stream must open before it closes");
    Ok((round_wire, stream_id, text_out))
}

async fn ask_stream(client: &mut Client) -> Result<WirePayload, String> {
    match tokio::time::timeout(Duration::from_secs(10), client.next_frame()).await {
        Ok(Ok(payload)) => Ok(payload),
        Ok(Err(error)) => Err(format!("stream frame errored: {error:?}")),
        Err(_) => Err(String::from("stream frame timed out")),
    }
}

async fn history_count(client: &mut Client) -> Result<(usize, bool, bool), String> {
    let companion = client.companion_ref();
    let history = ask(
        client,
        WirePayload::HistoryRequest(cmds::history_request(&companion, 50)),
        "history",
    )
    .await;
    assert!(
        matches!(&history, Ok(WirePayload::HistoryView(_))),
        "history must answer, got {history:?}"
    );
    let Ok(WirePayload::HistoryView(view)) = history else {
        return Err(String::from("history answered nothing usable"));
    };
    let mut owner_seen = false;
    let mut companion_seen = false;
    for item in &view.items {
        if item.role == ene_api::v1::round::HistoryRole::Owner {
            owner_seen = true;
        }
        if item.role == ene_api::v1::round::HistoryRole::Companion {
            companion_seen = true;
        }
    }
    Ok((view.items.len(), owner_seen, companion_seen))
}

async fn pending_empty(dir: &std::path::Path) -> bool {
    for _ in 0..100 {
        let Ok(store) = Store::open(&dir.join("app.db")).await else {
            break;
        };
        let Ok(companion) = store.ensure_running_companion().await else {
            break;
        };
        let Ok(pending) = store.list_pending(companion).await else {
            break;
        };
        if pending.is_empty() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn production_path_setup_to_restart() {
    let temp = tempfile::TempDir::new();
    let temp = temp.unwrap();
    let dir = temp.path().to_path_buf();
    let handle = open_host(&dir).await.unwrap();
    let server = tokio::spawn(conn::run(
        dir.clone(),
        Arc::clone(&handle),
        Arc::new(fake_transport()),
    ));
    assert!(wait_for_socket(&dir).await, "listener must bind ene.sock");

    let pending = Client::connect(&dir, DESCRIPTOR, "test").await;
    assert!(
        matches!(pending, Err(CliError::ServerOutcome(_))),
        "first pairing must pend"
    );
    let approver = open_host(&dir).await.unwrap();
    let provisioned = approve_and_provision(&dir, &approver).await;
    assert!(provisioned.is_ok(), "approval must pair: {provisioned:?}");

    let connected = Client::connect(&dir, DESCRIPTOR, "test").await;
    let connected_ok = connected.is_ok();
    assert!(connected_ok, "second connect must succeed");
    let mut client = connected.unwrap();

    let sections = view_sections(&mut client).await;
    let sections = sections.unwrap();
    for expected in ["provider", "model", "consent", "credential"] {
        assert!(
            sections.iter().any(|kind| kind == expected),
            "show must carry the Host sections, got {sections:?}"
        );
    }
    let setup = setup_flow(&mut client, &approver).await;
    assert!(setup.is_ok(), "setup must complete: {setup:?}");

    let sent = send_round(&mut client, "hello companion").await;
    let (round_wire, stream_id, text_out) = sent.unwrap();
    assert!(
        text_out == FAKE_TEXT,
        "stream must carry provider text, got {text_out:?}"
    );

    let notified = tokio::time::timeout(
        Duration::from_secs(10),
        client.notify(WirePayload::ConfirmPresentation(
            ene_api::v1::round::ConfirmPresentationWire {
                round: ene_api::v1::refs::RoundWireId(round_wire.clone()),
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
    assert!(
        pending_empty(&dir).await,
        "confirmed reply must not linger undelivered"
    );

    let counted = history_count(&mut client).await;
    let (before, owner_seen, companion_seen) = counted.unwrap();
    assert!(owner_seen && companion_seen, "history must hold both sides");
    assert!(before >= 2, "history must hold the round, got {before}");

    drop(client);
    server.abort();
    tokio::task::yield_now().await;
    drop(std::fs::remove_file(dir.join("ene.sock")));

    let handle = open_host(&dir).await.unwrap();
    let handle = Arc::new(handle);
    let server = tokio::spawn(conn::run(
        dir.clone(),
        Arc::clone(&handle),
        Arc::new(fake_transport()),
    ));
    assert!(
        wait_for_socket(&dir).await,
        "listener must rebind after restart"
    );

    let reconnected = Client::connect(&dir, DESCRIPTOR, "test").await;
    let reconnected_ok = reconnected.is_ok();
    assert!(
        reconnected_ok,
        "reconnect must succeed on durable auth material"
    );
    let mut client = reconnected.unwrap();
    let counted = history_count(&mut client).await;
    let (after, _, _) = counted.unwrap();
    assert!(
        after == before,
        "restart must preserve history ({before} -> {after})"
    );

    let companion = client.companion_ref();
    let stale = ask(
        &mut client,
        WirePayload::SubmitTextInput(cmds::submit_input(
            &companion,
            Some(round_wire),
            false,
            String::from("old round retry"),
            String::from("en"),
        )),
        "stale probe",
    )
    .await;
    assert!(
        !matches!(
            stale,
            Ok(WirePayload::RoundIntakeOutcome(
                RoundIntakeOutcomeWire::AcceptedForRound { .. }
            ))
        ),
        "pre-restart rounds must not resume, got {stale:?}"
    );
    server.abort();
}

async fn view_sections(client: &mut Client) -> Result<Vec<String>, String> {
    let answer = ask(
        client,
        WirePayload::ManagementViewRequest(cmds::setup_view_request()),
        "show",
    )
    .await;
    assert!(
        matches!(&answer, Ok(WirePayload::ManagementView(_))),
        "show must answer a view: {answer:?}"
    );
    let Ok(WirePayload::ManagementView(view)) = answer else {
        return Err(String::from("show answered nothing usable"));
    };
    Ok(view
        .sections
        .iter()
        .map(|section| section.kind.clone())
        .collect())
}

/// Integration tests run from `target/debug/deps`, so workspace binaries live
/// two levels up. Requires a prior `cargo build` (`cargo test` alone does not
/// link binaries).
fn workspace_binary(name: &str) -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let debug = exe.parent()?.parent()?;
    let candidate = debug.join(name);
    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

/// A spawned child killed on drop, so failing asserts cannot leak a
/// listener holding the test socket.
struct KillOnDrop(Option<std::process::Child>);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            drop(child.kill());
            drop(child.wait());
        }
    }
}

fn escape_json_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

async fn run_cli(
    binary: &std::path::Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
    timeout: Duration,
) -> Option<(i32, String, String)> {
    let mut command = tokio::process::Command::new(binary);
    command.args(args);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let child = command.spawn().ok()?;
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    Some((
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

/// Production-path binary test: the real `ene-core serve` listener plus the
/// real `ene-ctl` command orchestration (arg parsing, builders, session,
/// rendering, exit codes). Provider inference is out of scope here (no
/// network or keys in CI), so the send path stays with the lib-level E2E.
#[tokio::test]
async fn binaries_drive_pairing_setup_and_views() {
    let temp = tempfile::TempDir::new();
    let temp = temp.unwrap();
    let dir = temp.path().to_path_buf();
    let binaries = (workspace_binary("ene-ctl"), workspace_binary("ene-core"));
    assert!(
        binaries.0.is_some() && binaries.1.is_some(),
        "both binaries must be built"
    );
    let (Some(ctl), Some(core)) = binaries else {
        return;
    };
    let config_path = dir.join("ene.json");
    let config = format!(
        "{{\"language\": \"en\", \"data_dir\": \"{}\"}}",
        escape_json_string(&dir.to_string_lossy())
    );
    assert!(
        std::fs::write(&config_path, config).is_ok(),
        "config file must be writable"
    );
    let config = config_path.to_string_lossy().into_owned();

    let mut server = std::process::Command::new(&core);
    // Fake provider key for the SERVER child only (never our own process
    // env): production reads the bearer from its environment, so without it
    // the credential gate would deny assignment. No inference runs here, so
    // no network is touched; the key only satisfies presence checks.
    server.env("ENE_OPENAI_API_KEY", "sk-test-only");
    server.args(["serve", "--config", &config]);
    server.stdout(std::process::Stdio::null());
    server.stderr(std::process::Stdio::null());
    let server = server.spawn();
    let server = server.unwrap();
    let _server = KillOnDrop(Some(server));
    let bound = wait_for_socket(&dir).await;
    let listing: Vec<String> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        bound,
        "listener must bind ene.sock in {dir:?}, has {listing:?}"
    );
    assert!(
        !dir.join("ene.sock").join("ene.sock").exists(),
        "socket path must not double-append"
    );

    let status = run_cli(
        &ctl,
        &["--config", &config, "status"],
        &[],
        Duration::from_secs(10),
    )
    .await;
    assert!(
        matches!(status, Some((2, _, _))),
        "pre-pairing status must pend pairing, got {status:?}"
    );

    let mut list_pending = std::process::Command::new(&core);
    list_pending.args(["approve-device", "--config", &config]);
    list_pending.stdout(std::process::Stdio::piped());
    list_pending.stderr(std::process::Stdio::null());
    let listed = list_pending.output();
    let listed = listed.unwrap();
    assert!(listed.status.success(), "listing pendings must exit 0");
    let pending_out = String::from_utf8_lossy(&listed.stdout).into_owned();
    let descriptor = pending_out
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty());
    assert!(
        descriptor.is_some(),
        "one pending device must list, got {pending_out:?}"
    );
    let descriptor = descriptor.unwrap();
    let mut approve = std::process::Command::new(&core);
    approve.args([
        "approve-device",
        "--descriptor",
        descriptor,
        "--config",
        &config,
    ]);
    approve.stdout(std::process::Stdio::piped());
    approve.stderr(std::process::Stdio::piped());
    let approved = approve.output();
    let approved = approved.unwrap();
    assert!(
        approved.status.success(),
        "approve-device must exit 0: {}",
        String::from_utf8_lossy(&approved.stderr)
    );
    let shown = String::from_utf8_lossy(&approved.stdout).into_owned();
    let secret = shown
        .lines()
        .find_map(|line| line.strip_prefix("pairing secret (show once): "))
        .map(str::to_string);
    assert!(
        secret.is_some(),
        "approve must print the one-time secret, got {shown:?}"
    );
    let secret = secret.unwrap();
    assert!(!secret.trim().is_empty(), "secret must be non-blank");

    let status = run_cli(
        &ctl,
        &["--config", &config, "status"],
        &[("ENE_PAIRING_SECRET", secret.as_str())],
        Duration::from_secs(10),
    )
    .await;
    assert!(
        matches!(status, Some((0, _, _))),
        "paired status must exit 0, got {status:?}"
    );

    let show = run_cli(
        &ctl,
        &["--config", &config, "setup", "--show"],
        &[],
        Duration::from_secs(10),
    )
    .await;
    assert!(
        matches!(show, Some((0, _, _))),
        "setup --show must exit 0, got {show:?}"
    );
    let (_, out, _) = show.unwrap();
    for section in ["provider:", "model:", "consent:", "credential:"] {
        assert!(out.contains(section), "show must render {section}");
    }

    let setup = run_cli(
        &ctl,
        &[
            "--config",
            &config,
            "setup",
            "--provider",
            "openai",
            "--model",
            MODEL,
        ],
        &[],
        Duration::from_secs(10),
    )
    .await;
    assert!(
        matches!(setup, Some((2, _, _))),
        "unapproved setup must hold at exit 2, got {setup:?}"
    );

    let mut approve_cred = std::process::Command::new(&core);
    // The approval process must be able to read the bearer: it sweeps any
    // prior plaintext occurrence before the ref becomes usable.
    approve_cred.env("ENE_OPENAI_API_KEY", "sk-test-only");
    approve_cred.args([
        "approve-credential",
        "--provider",
        "openai",
        "--label",
        "main",
        "--config",
        &config,
    ]);
    approve_cred.stdout(std::process::Stdio::null());
    approve_cred.stderr(std::process::Stdio::piped());
    let credential_approved = approve_cred.output();
    let credential_approved = credential_approved.unwrap();
    assert!(
        credential_approved.status.success(),
        "approve-credential must exit 0: {}",
        String::from_utf8_lossy(&credential_approved.stderr)
    );

    let setup = run_cli(
        &ctl,
        &[
            "--config",
            &config,
            "setup",
            "--provider",
            "openai",
            "--model",
            MODEL,
        ],
        &[],
        Duration::from_secs(10),
    )
    .await;
    let setup_ok = matches!(setup, Some((0, _, _)));
    assert!(setup_ok, "approved setup must exit 0, got {setup:?}");
    let (_, out, _) = setup.unwrap();
    assert!(
        out.contains("setup complete:"),
        "setup must report completion"
    );

    let history = run_cli(
        &ctl,
        &["--config", &config, "history"],
        &[],
        Duration::from_secs(10),
    )
    .await;
    assert!(
        matches!(history, Some((0, _, _))),
        "empty history must exit 0, got {history:?}"
    );

    let memory = run_cli(
        &ctl,
        &["--config", &config, "memory"],
        &[],
        Duration::from_secs(10),
    )
    .await;
    assert!(
        matches!(memory, Some((0, _, _))),
        "the memory read model must exit 0, got {memory:?}"
    );
    let Some((_, memory_out, _)) = memory else {
        return;
    };
    assert!(
        memory_out.contains("memory: Memory"),
        "memory must render its read-only section, got {memory_out:?}"
    );
}

#[tokio::test]
async fn tampered_secret_cannot_authenticate() {
    let temp = tempfile::TempDir::new();
    let temp = temp.unwrap();
    let dir = temp.path().to_path_buf();
    let handle = open_host(&dir).await.unwrap();
    let server = tokio::spawn(conn::run(
        dir.clone(),
        Arc::clone(&handle),
        Arc::new(fake_transport()),
    ));
    assert!(wait_for_socket(&dir).await, "listener must bind");

    let pending = Client::connect(&dir, DESCRIPTOR, "test").await;
    assert!(
        matches!(pending, Err(CliError::ServerOutcome(_))),
        "first pairing must pend"
    );
    let approver = open_host(&dir).await.unwrap();
    let approval = approver.approve_device(DESCRIPTOR).await;
    let (record, _secret) = approval.unwrap().unwrap();
    let wire = record.wire.parse().map(DeviceWireId).unwrap();
    let stored = store_device(
        &dir,
        &StoredDevice::new(wire, String::from("wrong-secret-not-from-approve")),
    );
    assert!(stored.is_ok(), "test device file must store");
    let tampered = Client::connect(&dir, DESCRIPTOR, "test").await;
    assert!(
        matches!(tampered, Err(CliError::ServerOutcome(_))),
        "tampered secret must not authenticate"
    );
    server.abort();
}

#[tokio::test]
async fn rotation_requires_reprovisioning() {
    let temp = tempfile::TempDir::new();
    let temp = temp.unwrap();
    let dir = temp.path().to_path_buf();
    let handle = open_host(&dir).await.unwrap();
    let server = tokio::spawn(conn::run(
        dir.clone(),
        Arc::clone(&handle),
        Arc::new(fake_transport()),
    ));
    assert!(wait_for_socket(&dir).await, "listener must bind");

    let pending = Client::connect(&dir, DESCRIPTOR, "test").await;
    assert!(
        matches!(pending, Err(CliError::ServerOutcome(_))),
        "first pairing must pend"
    );
    let approver = open_host(&dir).await.unwrap();
    let provisioned = approve_and_provision(&dir, &approver).await;
    assert!(provisioned.is_ok(), "approval must pair: {provisioned:?}");
    let connected = Client::connect(&dir, DESCRIPTOR, "test").await;
    assert!(connected.is_ok(), "provisioned connect must succeed");
    drop(connected);

    let reapproved = approver.approve_device(DESCRIPTOR).await;
    assert!(
        reapproved.unwrap().is_some(),
        "re-approval returns the existing record and a fresh secret"
    );
    let stale_file = Client::connect(&dir, DESCRIPTOR, "test").await;
    assert!(
        matches!(stale_file, Err(CliError::ServerOutcome(_))),
        "rotated secret must invalidate the old file"
    );
    server.abort();
}

/// Distinct from the lib-level fake so a crossed wire would show.
const PROD_FAKE_TEXT: &str = "production reply over the real binaries";

/// Minimal fake Responses API over plain HTTP/1.1, answering a fixed
/// non-streaming completion. Hand-rolled to avoid new dependencies: the
/// production transport posts non-streaming JSON to `{base}/v1/responses`, so
/// a `Content-Length` responder suffices, with no external network.
async fn spawn_fake_responses(
    text: &'static str,
) -> Option<(
    std::net::SocketAddr,
    tokio::task::JoinHandle<()>,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
)> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.ok()?;
    let addr = listener.local_addr().ok()?;
    let saw_no_store = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&saw_no_store);
    let handle = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let flag = std::sync::Arc::clone(&flag);
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0_u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if head.len() > 16_384 {
                        return;
                    }
                    let read = stream.read(&mut byte).await.unwrap();
                    if read == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                let head_text = String::from_utf8_lossy(&head).into_owned();
                let mut content_length = 0_usize;
                for line in head_text.lines().skip(1) {
                    let Some((name, value)) = line.split_once(':') else {
                        continue;
                    };
                    if name.trim().eq_ignore_ascii_case("content-length")
                        && let Ok(parsed) = value.trim().parse::<usize>()
                    {
                        content_length = parsed;
                    }
                }
                let mut body = vec![0_u8; content_length.min(1_048_576)];
                let mut filled = 0_usize;
                while filled < body.len() {
                    match stream.read(&mut body[filled..]).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => filled += read,
                    }
                }
                // Lock the privacy boundary end to end: every production
                // request must explicitly disable server-side storage.
                if let Ok(seen) = serde_json::from_slice::<serde_json::Value>(&body)
                    && seen.get("store") == Some(&serde_json::Value::Bool(false))
                {
                    flag.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                let mut payload = String::new();
                // Server-sent events: the production transport requests
                // incremental output, so the fake provider streams two deltas
                // and then completes with usage.
                let (first, second) = text.split_at(text.len() / 2);
                for delta in [first, second] {
                    payload.push_str(&format!(
                        "data: {{\"type\":\"response.output_text.delta\",\"delta\":{}}}\n\n",
                        serde_json::json!(delta)
                    ));
                }
                payload.push_str(
                    "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":7,\"output_tokens\":9}}}\n\n",
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{payload}"
                );
                drop(stream.write_all(response.as_bytes()).await);
                drop(stream.shutdown().await);
            });
        }
    });
    Some((addr, handle, saw_no_store))
}

fn write_test_config(dir: &std::path::Path) -> Option<String> {
    let config_path = dir.join("ene.json");
    let config = format!(
        "{{\"language\": \"en\", \"data_dir\": \"{}\"}}",
        escape_json_string(&dir.to_string_lossy())
    );
    if std::fs::write(&config_path, config).is_err() {
        return None;
    }
    Some(config_path.to_string_lossy().into_owned())
}

/// `extra_env` goes to the child only; our own process env is never touched.
fn spawn_serve_binary(
    core: &std::path::Path,
    config: &str,
    extra_env: &[(&str, &str)],
) -> Option<KillOnDrop> {
    let mut server = std::process::Command::new(core);
    for (key, value) in extra_env {
        server.env(key, value);
    }
    server.args(["serve", "--config", config]);
    server.stdout(std::process::Stdio::null());
    server.stderr(std::process::Stdio::null());
    server.spawn().ok().map(|child| KillOnDrop(Some(child)))
}

async fn pair_via_binaries(
    ctl: &std::path::Path,
    core: &std::path::Path,
    config: &str,
) -> Option<String> {
    let status = run_cli(
        ctl,
        &["--config", config, "status"],
        &[],
        Duration::from_secs(10),
    )
    .await;
    assert!(
        matches!(status, Some((2, _, _))),
        "pre-pairing status must pend pairing, got {status:?}"
    );
    // The real client pairs under its platform descriptor, so approve
    // whatever it actually requested (like the operator channel would).
    let listed = std::process::Command::new(core)
        .args(["approve-device", "--config", config])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok();
    let descriptor = listed
        .as_ref()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .and_then(|out| {
            out.lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(str::to_string)
        });
    let descriptor = descriptor?;
    let mut approve = std::process::Command::new(core);
    approve.args([
        "approve-device",
        "--descriptor",
        descriptor.as_str(),
        "--config",
        config,
    ]);
    approve.stdout(std::process::Stdio::piped());
    approve.stderr(std::process::Stdio::piped());
    let approved = approve.output().ok()?;
    assert!(
        approved.status.success(),
        "approve-device must exit 0: {}",
        String::from_utf8_lossy(&approved.stderr)
    );
    let shown = String::from_utf8_lossy(&approved.stdout).into_owned();
    let secret = shown
        .lines()
        .find_map(|line| line.strip_prefix("pairing secret (show once): "))
        .map(str::to_string)?;
    assert!(!secret.trim().is_empty(), "secret must be non-blank");
    let status = run_cli(
        ctl,
        &["--config", config, "status"],
        &[("ENE_PAIRING_SECRET", secret.as_str())],
        Duration::from_secs(10),
    )
    .await;
    assert!(
        matches!(status, Some((0, _, _))),
        "paired status must exit 0, got {status:?}"
    );
    Some(secret)
}

#[tokio::test]
async fn binaries_drive_send_stream_history_and_restart() {
    let temp = tempfile::TempDir::new();
    let temp = temp.unwrap();
    let dir = temp.path().to_path_buf();
    let binaries = (workspace_binary("ene-ctl"), workspace_binary("ene-core"));
    assert!(
        binaries.0.is_some() && binaries.1.is_some(),
        "both binaries must be built"
    );
    let (Some(ctl), Some(core)) = binaries else {
        return;
    };
    let config = write_test_config(&dir).unwrap();
    let (fake_addr, fake, saw_no_store) = spawn_fake_responses(PROD_FAKE_TEXT).await.unwrap();
    let base_url = format!("http://{fake_addr}");
    let server_env = [
        ("ENE_OPENAI_API_KEY", "sk-test-only"),
        ("ENE_OPENAI_BASE_URL", base_url.as_str()),
    ];

    let server = spawn_serve_binary(&core, &config, &server_env);
    assert!(server.is_some(), "serve must spawn");
    let Some(server) = server else {
        fake.abort();
        return;
    };
    assert!(wait_for_socket(&dir).await, "listener must bind ene.sock");

    let secret = pair_via_binaries(&ctl, &core, &config).await;
    assert!(secret.is_some(), "binary pairing must complete");
    let setup_args = [
        "--config",
        &config,
        "setup",
        "--provider",
        "openai",
        "--model",
        MODEL,
    ];
    let setup = run_cli(&ctl, &setup_args, &[], Duration::from_secs(10)).await;
    assert!(
        matches!(setup, Some((2, _, _))),
        "unapproved setup must hold at exit 2, got {setup:?}"
    );
    let approve_cred = std::process::Command::new(&core)
        .env("ENE_OPENAI_API_KEY", "sk-test-only")
        .args([
            "approve-credential",
            "--provider",
            "openai",
            "--label",
            "main",
            "--config",
            &config,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    assert!(
        matches!(&approve_cred, Ok(output) if output.status.success()),
        "approve-credential must exit 0"
    );
    let setup = run_cli(&ctl, &setup_args, &[], Duration::from_secs(10)).await;
    assert!(
        matches!(setup, Some((0, _, _))),
        "approved setup must exit 0, got {setup:?}"
    );

    let send = run_cli(
        &ctl,
        &["--config", &config, "send", "hello production"],
        &[],
        Duration::from_secs(30),
    )
    .await;
    assert!(
        matches!(send, Some((0, _, _))),
        "real send must exit 0, got {send:?}"
    );
    let Some((_, send_out, _)) = send else {
        fake.abort();
        return;
    };
    assert!(
        send_out.contains("AcceptedForRound"),
        "send must print its accept, got {send_out:?}"
    );
    assert!(
        send_out.contains(PROD_FAKE_TEXT),
        "send must stream provider text, got {send_out:?}"
    );
    assert!(
        saw_no_store.load(std::sync::atomic::Ordering::Relaxed),
        "the production request must disable server-side storage"
    );
    assert!(
        pending_empty(&dir).await,
        "the presented reply must not linger undelivered"
    );

    let history = run_cli(
        &ctl,
        &["--config", &config, "history"],
        &[],
        Duration::from_secs(10),
    )
    .await;
    assert!(
        matches!(history, Some((0, _, _))),
        "history must exit 0, got {history:?}"
    );
    let Some((_, history_out, _)) = history else {
        fake.abort();
        return;
    };
    assert!(
        history_out.contains("hello production"),
        "history must hold the sent text, got {history_out:?}"
    );
    assert!(
        history_out.contains(PROD_FAKE_TEXT),
        "history must hold the reply text, got {history_out:?}"
    );

    // Drop the whole guard: `KillOnDrop` kills the child on drop, while
    // dropping a bare `std::process::Child` would leak the old server
    // (still holding the unlinked socket) past the restart.
    drop(server);
    drop(std::fs::remove_file(dir.join("ene.sock")));
    let server = spawn_serve_binary(&core, &config, &server_env);
    assert!(server.is_some(), "serve must respawn after restart");
    let Some(server) = server else {
        fake.abort();
        return;
    };
    assert!(
        wait_for_socket(&dir).await,
        "listener must rebind after restart"
    );
    let again = run_cli(
        &ctl,
        &["--config", &config, "history"],
        &[],
        Duration::from_secs(10),
    )
    .await;
    let Some((exit_code, history_again, _)) = again else {
        fake.abort();
        return;
    };
    assert!(
        exit_code == 0 && history_again == history_out,
        "restart must preserve history byte-for-byte, got code {exit_code} and {history_again:?}"
    );
    let resend = run_cli(
        &ctl,
        &["--config", &config, "send", "after restart"],
        &[],
        Duration::from_secs(30),
    )
    .await;
    assert!(
        matches!(&resend, Some((0, out, _)) if out.contains(PROD_FAKE_TEXT)),
        "the restarted server must serve new sends, got {resend:?}"
    );
    let memory = run_cli(
        &ctl,
        &["--config", &config, "memory"],
        &[],
        Duration::from_secs(10),
    )
    .await;
    assert!(
        matches!(&memory, Some((0, out, _)) if out.contains("memory: Memory")),
        "the real binaries must render the memory read model, got {memory:?}"
    );
    drop(server);
    fake.abort();
}

/// Scripted provider for the Stage 3 acceptance path.
///
/// Dialogue calls always answer `reply`; learning calls pop the next scripted
/// answer, or a no-value answer when the queue is empty. Every input is
/// recorded so recall and credential non-exposure can be inspected on the
/// production path.
struct Stage3Transport {
    reply: String,
    learning: std::sync::Mutex<std::collections::VecDeque<String>>,
    inputs: std::sync::Mutex<Vec<String>>,
    /// One permit per completed learning provider call, so the test can wait
    /// for post-response formation instead of racing it.
    learning_calls: std::sync::Arc<tokio::sync::Semaphore>,
}

impl Stage3Transport {
    fn new(reply: &str) -> Self {
        Self {
            reply: reply.to_owned(),
            learning: std::sync::Mutex::new(std::collections::VecDeque::new()),
            inputs: std::sync::Mutex::new(Vec::new()),
            learning_calls: std::sync::Arc::new(tokio::sync::Semaphore::new(0)),
        }
    }

    fn push_learning(&self, answer: &str) {
        self.learning
            .lock()
            .expect("learning script lock")
            .push_back(answer.to_owned());
    }

    /// Waits for one completed learning provider call.
    async fn wait_learning(&self) {
        let permit = self
            .learning_calls
            .acquire()
            .await
            .expect("the learning-call semaphore stays open");
        permit.forget();
    }

    /// Every provider input, dialogue and learning, in order.
    fn all_inputs(&self) -> Vec<String> {
        self.inputs.lock().expect("provider input lock").clone()
    }

    fn dialogue_inputs(&self) -> Vec<String> {
        self.all_inputs()
            .into_iter()
            .filter(|input| !input.contains("learning formation pass"))
            .collect()
    }

    fn last_dialogue_input(&self) -> Option<String> {
        self.dialogue_inputs().last().cloned()
    }
}

impl ene_inference::ProviderTransport for Stage3Transport {
    fn complete(
        &self,
        req: ene_inference::ProviderRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        ene_inference::ProviderResponse,
                        ene_inference::InferenceTechnicalError,
                    >,
                > + Send
                + '_,
        >,
    > {
        self.inputs
            .lock()
            .expect("provider input lock")
            .push(req.input.clone());
        let text = if req.input.contains("learning formation pass") {
            let answer = self
                .learning
                .lock()
                .expect("learning script lock")
                .pop_front()
                .unwrap_or_else(|| {
                    String::from(r#"{"summary": "Nothing worth keeping.", "memories": []}"#)
                });
            self.learning_calls.add_permits(1);
            answer
        } else {
            self.reply.clone()
        };
        Box::pin(async move { Ok(ene_inference::ProviderResponse { text, usage: None }) })
    }
}

/// The read-only memory section body from one management view answer.
async fn memory_view(client: &mut Client) -> String {
    memory_view_after(client, None).await
}

/// The memory section body after a page cursor.
async fn memory_view_after(client: &mut Client, after: Option<&str>) -> String {
    let answer = ask(
        client,
        WirePayload::ManagementViewRequest(cmds::memory_view_request(after)),
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

/// The `next:` page cursor of a memory section body, when older memories
/// remain.
fn next_memory_cursor(body: &str) -> Option<String> {
    body.lines()
        .find_map(|line| line.strip_prefix("next: ").map(str::to_owned))
}

fn memory_count(body: &str) -> usize {
    body.lines()
        .filter(|line| line.starts_with("memory "))
        .count()
}

/// The recall section of one assembled dialogue prompt.
fn recall_section(input: &str) -> String {
    let Some(start) = input.find("Relevant memories:") else {
        return String::new();
    };
    let rest = &input[start..];
    let end = rest.find("Recent conversation:").unwrap_or(rest.len());
    rest[..end].to_owned()
}

/// A fresh production-path client for one operation. Each CLI process is a
/// fresh connection that learns the current presence generation on connect,
/// so multi-turn fixtures reconnect like separate `ene-ctl` invocations.
async fn stage3_client(dir: &std::path::Path) -> Client {
    match Client::connect(dir, DESCRIPTOR, "test").await {
        Ok(client) => client,
        Err(error) => panic!("client must connect: {error:?}"),
    }
}

async fn stage3_send(
    dir: &std::path::Path,
    text: &str,
) -> Result<(String, Option<ene_api::v1::refs::StreamWireId>, String), String> {
    let mut client = stage3_client(dir).await;
    send_round(&mut client, text).await
}

async fn stage3_view(dir: &std::path::Path) -> String {
    let mut client = stage3_client(dir).await;
    memory_view(&mut client).await
}

async fn stage3_view_after(dir: &std::path::Path, after: Option<&str>) -> String {
    let mut client = stage3_client(dir).await;
    memory_view_after(&mut client, after).await
}

/// Assigns the learning capability explicitly on the production path.
async fn stage3_assign_learning(dir: &std::path::Path) -> Result<(), String> {
    let mut client = stage3_client(dir).await;
    let mark = view_mark(&mut client).await?;
    let answer = ask(
        &mut client,
        WirePayload::ManagementIntent(cmds::assignment_intent(
            CommandWireId(uuid::Uuid::new_v4()),
            &BaseViewMark(mark),
            cmds::CAPABILITY_LEARNING,
            "openai",
            MODEL,
        )),
        "learning assign",
    )
    .await?;
    match answer {
        WirePayload::ManagementOutcome(ManagementOutcome::StoredAsRuleView { .. }) => Ok(()),
        other => Err(format!("learning assignment must store, got {other:?}")),
    }
}

/// The rendered Conversation History over the production path.
async fn stage3_history(dir: &std::path::Path) -> String {
    let mut client = stage3_client(dir).await;
    let companion = client.companion_ref();
    let answer = ask(
        &mut client,
        WirePayload::HistoryRequest(cmds::history_request(&companion, 100)),
        "history",
    )
    .await;
    let Ok(WirePayload::HistoryView(view)) = answer else {
        panic!("history must answer a view: {answer:?}");
    };
    cmds::render_history(&view)
}

/// Polls the read-only Memory view until `expected` appears.
///
/// Formation is post-response work, so the test waits for the durable result
/// instead of assuming it completed with the stream.
async fn stage3_wait_for_memory(dir: &std::path::Path, expected: &str) -> String {
    for _ in 0..100 {
        let view = stage3_view(dir).await;
        if view.contains(expected) {
            return view;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let view = stage3_view(dir).await;
    panic!("the memory view never showed {expected:?}: {view}");
}

#[tokio::test]
async fn stage3_conversation_formation_restart_and_recall() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let handle = open_host(&dir).await.unwrap();
    let transport = Arc::new(Stage3Transport::new(FAKE_TEXT));
    let server = tokio::spawn(conn::run(
        dir.clone(),
        Arc::clone(&handle),
        Arc::clone(&transport),
    ));
    assert!(wait_for_socket(&dir).await, "listener must bind ene.sock");

    let pending = Client::connect(&dir, DESCRIPTOR, "test").await;
    assert!(
        matches!(pending, Err(CliError::ServerOutcome(_))),
        "first pairing must pend"
    );
    let approver = open_host(&dir).await.unwrap();
    let provisioned = approve_and_provision(&dir, &approver).await;
    assert!(provisioned.is_ok(), "approval must pair: {provisioned:?}");
    let mut client = stage3_client(&dir).await;
    let setup = setup_flow(&mut client, &approver).await;
    assert!(setup.is_ok(), "setup must complete: {setup:?}");
    drop(client);
    // The dialogue assignment above never authorizes learning: the Owner
    // assigns the same route to the learning capability explicitly.
    let assigned = stage3_assign_learning(&dir).await;
    assert!(
        assigned.is_ok(),
        "learning assignment must store: {assigned:?}"
    );

    // (1) One conversation forms one compressed Memory grounded on a Summary.
    transport.push_learning(
        r#"{"summary": "The owner prefers jasmine tea in the morning.", "memories": [{"action": "create", "content": "The owner prefers jasmine tea in the morning.", "importance": 4, "temporal": "enduring"}]}"#,
    );
    let sent = stage3_send(&dir, "remember that I prefer jasmine tea in the morning").await;
    assert!(sent.is_ok(), "the memory turn must complete: {sent:?}");
    transport.wait_learning().await;
    let view = stage3_wait_for_memory(&dir, "The owner prefers jasmine tea in the morning.").await;
    assert_eq!(memory_count(&view), 1, "one memory, not one per message");
    assert!(view.contains("scope=companion"), "companion scope: {view}");
    assert!(view.contains("importance=4"));
    assert!(view.contains("temporal=enduring"));
    assert!(view.contains("recall=active"));
    assert!(view.contains("grounds summary"), "summary grounds: {view}");
    assert!(view.contains("rev1 initial"), "first revision: {view}");

    // (2) An experience with no lasting value is not stored.
    transport.push_learning(r#"{"summary": "Small talk about the weather.", "memories": []}"#);
    let sent = stage3_send(&dir, "nice weather today").await;
    assert!(sent.is_ok());
    transport.wait_learning().await;
    let view = stage3_view(&dir).await;
    assert_eq!(
        memory_count(&view),
        1,
        "a declined experience stores nothing"
    );

    // (3) Repeating information reinforces the existing Memory.
    transport.push_learning(
        r#"{"summary": "The owner mentioned tea again.", "memories": [{"action": "update", "target": 1, "change": "reinforced", "content": "The owner prefers jasmine tea in the morning."}]}"#,
    );
    let sent = stage3_send(&dir, "I still love jasmine tea").await;
    assert!(sent.is_ok());
    transport.wait_learning().await;
    let view = stage3_wait_for_memory(&dir, "rev2 reinforced").await;
    assert_eq!(memory_count(&view), 1, "no duplicate memory");

    // (4) A correction and a temporal change stay distinguishable.
    transport.push_learning(
        r#"{"summary": "The owner corrected the earlier memory.", "memories": [{"action": "update", "target": 1, "change": "corrected_initially_wrong", "content": "The owner never liked jasmine tea."}]}"#,
    );
    let sent = stage3_send(&dir, "actually I never liked jasmine tea").await;
    assert!(sent.is_ok());
    transport.wait_learning().await;
    transport.push_learning(
        r#"{"summary": "The preference changed over time.", "memories": [{"action": "update", "target": 1, "change": "changed_since", "content": "The owner prefers coffee now."}]}"#,
    );
    let sent = stage3_send(&dir, "I moved on to coffee last month").await;
    assert!(sent.is_ok());
    transport.wait_learning().await;
    let view = stage3_wait_for_memory(&dir, "rev4 changed-since").await;
    assert!(view.contains("rev3 corrected-initially-wrong"), "{view}");
    assert!(
        view.contains("rev1 initial"),
        "earlier revisions are kept: {view}"
    );
    assert!(view.contains("The owner never liked jasmine tea."));
    assert!(view.contains("The owner prefers coffee now."));

    // (5) A registered credential never reaches Summary, Memory, History, or
    // either provider prompt, even when the owner asks to remember it and
    // the answers echo it.
    transport.push_learning(
        r#"{"summary": "The owner shared a key: sk-test-only.", "memories": [{"action": "create", "content": "The owner's key is sk-test-only.", "importance": 5, "temporal": "enduring"}]}"#,
    );
    let sent = stage3_send(&dir, "remember my key sk-test-only").await;
    assert!(sent.is_ok());
    transport.wait_learning().await;
    let view = stage3_wait_for_memory(&dir, "[credential]").await;
    assert_eq!(memory_count(&view), 2, "the redacted memory still forms");
    assert!(
        !view.contains("sk-test-only"),
        "a registered credential never reaches Memory or Summary: {view}"
    );
    let history = stage3_history(&dir).await;
    assert!(
        !history.contains("sk-test-only"),
        "a registered credential never reaches Conversation History: {history}"
    );
    assert!(
        history.contains("[credential]"),
        "the History occurrence is visibly redacted: {history}"
    );
    for (position, input) in transport.all_inputs().iter().enumerate() {
        assert!(
            !input.contains("sk-test-only"),
            "provider input {position} must not carry the credential"
        );
    }

    // (6) Restart keeps Memory, revisions, and grounds.
    server.abort();
    tokio::task::yield_now().await;
    drop(std::fs::remove_file(dir.join("ene.sock")));
    let handle = open_host(&dir).await.unwrap();
    let server = tokio::spawn(conn::run(
        dir.clone(),
        Arc::clone(&handle),
        Arc::clone(&transport),
    ));
    assert!(wait_for_socket(&dir).await, "listener must rebind");
    let view = stage3_view(&dir).await;
    assert_eq!(memory_count(&view), 2, "restart keeps formed memories");
    assert!(view.contains("rev1 initial"));
    assert!(view.contains("rev3 corrected-initially-wrong"));
    assert!(view.contains("rev4 changed-since"));
    assert!(view.contains("grounds summary"));

    // (7) A related conversation after restart recalls the current Memory.
    transport.push_learning(r#"{"summary": "Nothing new.", "memories": []}"#);
    let sent = stage3_send(&dir, "what do I drink now?").await;
    assert!(sent.is_ok());
    transport.wait_learning().await;
    let input = transport
        .last_dialogue_input()
        .expect("the dialogue input is recorded");
    let recalled = recall_section(&input);
    assert!(
        recalled.contains("coffee"),
        "the current recognition is recalled after restart: {recalled}"
    );

    // (8) Normal forgetting suppresses recall without deleting anything.
    transport.push_learning(
        r#"{"summary": "The owner asked to let the drink topic rest.", "memories": [{"action": "forget", "target": 2}]}"#,
    );
    let sent = stage3_send(&dir, "forget about my drink preference").await;
    assert!(sent.is_ok());
    transport.wait_learning().await;
    let view = stage3_wait_for_memory(&dir, "rev5 forgotten").await;
    assert!(view.contains("recall=suppressed"), "{view}");
    assert!(
        view.contains("content: The owner prefers coffee now."),
        "content is kept: {view}"
    );
    assert!(
        view.contains("rev1 initial"),
        "all revisions are kept: {view}"
    );

    transport.push_learning(r#"{"summary": "Nothing new.", "memories": []}"#);
    let sent = stage3_send(&dir, "what do I drink now?").await;
    assert!(sent.is_ok());
    transport.wait_learning().await;
    let input = transport
        .last_dialogue_input()
        .expect("the dialogue input is recorded");
    let recalled = recall_section(&input);
    assert!(
        !recalled.contains("coffee"),
        "suppressed memory is not recalled: {recalled}"
    );

    server.abort();
}

/// Stage 3 management read model: a companion with more memories than one
/// page must remain fully reachable. Five formations of five creates each
/// produce twenty-five memories; the view pages twenty at a time and ends a
/// page with the cursor that reads the next one, so an old memory's current
/// recognition, revisions, and grounds stay reachable from the management
/// surface alone.
#[tokio::test]
async fn stage3_management_view_reaches_memories_beyond_the_first_page() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let handle = open_host(&dir).await.unwrap();
    let transport = Arc::new(Stage3Transport::new(FAKE_TEXT));
    let server = tokio::spawn(conn::run(
        dir.clone(),
        Arc::clone(&handle),
        Arc::clone(&transport),
    ));
    assert!(wait_for_socket(&dir).await, "listener must bind ene.sock");

    let pending = Client::connect(&dir, DESCRIPTOR, "test").await;
    assert!(
        matches!(pending, Err(CliError::ServerOutcome(_))),
        "first pairing must pend"
    );
    let approver = open_host(&dir).await.unwrap();
    let provisioned = approve_and_provision(&dir, &approver).await;
    assert!(provisioned.is_ok(), "approval must pair: {provisioned:?}");
    let mut client = stage3_client(&dir).await;
    let setup = setup_flow(&mut client, &approver).await;
    assert!(setup.is_ok(), "setup must complete: {setup:?}");
    drop(client);
    let assigned = stage3_assign_learning(&dir).await;
    assert!(
        assigned.is_ok(),
        "learning assignment must store: {assigned:?}"
    );

    // Twenty-five memories, five per formation pass.
    for batch in 0..5 {
        let creates: Vec<String> = (0..5)
            .map(|index| {
                format!(
                    "{{\"action\": \"create\", \"content\": \"batch {batch} memory {index}\", \"importance\": 3, \"temporal\": \"enduring\"}}"
                )
            })
            .collect();
        transport.push_learning(&format!(
            "{{\"summary\": \"Batch {batch} of durable memories.\", \"memories\": [{}]}}",
            creates.join(", ")
        ));
        let sent = stage3_send(&dir, &format!("remember batch {batch}")).await;
        assert!(sent.is_ok(), "batch {batch} must complete: {sent:?}");
        transport.wait_learning().await;
    }
    // One formation commits its changes in order, so the last memory of the
    // last batch becoming durable proves every earlier commit landed.
    let _ = stage3_wait_for_memory(&dir, "batch 4 memory 4").await;

    let first = stage3_view_after(&dir, None).await;
    assert_eq!(memory_count(&first), 20, "the first page is capped");
    let cursor = next_memory_cursor(&first).expect("older memories remain");
    let second = stage3_view_after(&dir, Some(&cursor)).await;
    assert_eq!(
        memory_count(&second),
        5,
        "the older memories fill the next page"
    );
    assert!(
        next_memory_cursor(&second).is_none(),
        "the last page offers no cursor"
    );
    assert!(
        second.contains("batch 0 memory 0"),
        "the oldest memory is reachable: {second}"
    );
    assert_eq!(
        second.matches("rev1 initial").count(),
        5,
        "every older memory shows its first revision: {second}"
    );
    assert_eq!(
        second.matches("grounds summary").count(),
        5,
        "every older memory shows its grounds: {second}"
    );
    for batch in 0..5 {
        for index in 0..5 {
            let content = format!("batch {batch} memory {index}");
            assert!(
                first.contains(&content) || second.contains(&content),
                "every memory is reachable: {content}"
            );
        }
    }

    server.abort();
}
