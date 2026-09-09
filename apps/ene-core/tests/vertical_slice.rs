//! Production-path end to end for Stage 2.
//!
//! Real listener socket, real `ene-ctl` client builders and session, real
//! Host orchestration; only the provider HTTP transport is fake. Covers:
//! socket placement, pairing approval through an INDEPENDENT approval
//! context (proving cross-process sharing via the file device-auth store),
//! the full challenge/proof/accepted handshake, setup register/assign/
//! complete, text round with ordered streaming, presentation confirmation
//! draining undelivered, history, restart restore WITHOUT re-approval,
//! stale old rounds, secret rotation, tampered secrets, and untrusted-peer
//! denial.
//!
//! Unix-only: the production listener is a Unix socket (Windows uses named
//! pipes in a follow-up).

#![cfg(unix)]

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
        CredentialRef {
            id: String::from("openai:main"),
            provider: String::from("openai"),
            label: String::from("main"),
        },
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
        &StoredDevice::new(DeviceWireId(record.id.0.as_uuid()), secret),
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
    let send = ask(
        client,
        WirePayload::SubmitTextInput(cmds::submit_input(
            None,
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
    let history = ask(
        client,
        WirePayload::HistoryRequest(cmds::history_request(50)),
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
    assert!(temp.is_ok(), "tempdir must create");
    let Ok(temp) = temp else {
        return;
    };
    let dir = temp.path().to_path_buf();
    let Some(handle) = open_host(&dir).await else {
        return;
    };
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
    let Some(approver) = open_host(&dir).await else {
        return;
    };
    let provisioned = approve_and_provision(&dir, &approver).await;
    assert!(provisioned.is_ok(), "approval must pair: {provisioned:?}");

    let connected = Client::connect(&dir, DESCRIPTOR, "test").await;
    let connected_ok = connected.is_ok();
    assert!(connected_ok, "second connect must succeed");
    let Ok(mut client) = connected else {
        return;
    };

    let sections = view_sections(&mut client).await;
    assert!(sections.is_ok(), "show must answer");
    let Ok(sections) = sections else {
        return;
    };
    for expected in ["provider", "model", "consent", "credential"] {
        assert!(
            sections.iter().any(|kind| kind == expected),
            "show must carry the Host sections, got {sections:?}"
        );
    }
    let setup = setup_flow(&mut client, &approver).await;
    assert!(setup.is_ok(), "setup must complete: {setup:?}");

    let sent = send_round(&mut client, "hello companion").await;
    assert!(sent.is_ok(), "round must stream: {sent:?}");
    let Ok((round_wire, stream_id, text_out)) = sent else {
        return;
    };
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
    assert!(counted.is_ok(), "history must answer");
    let Ok((before, owner_seen, companion_seen)) = counted else {
        return;
    };
    assert!(owner_seen && companion_seen, "history must hold both sides");
    assert!(before >= 2, "history must hold the round, got {before}");

    drop(client);
    server.abort();
    tokio::task::yield_now().await;
    drop(std::fs::remove_file(dir.join("ene.sock")));

    let Some(handle) = open_host(&dir).await else {
        return;
    };
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
    let Ok(mut client) = reconnected else {
        return;
    };
    let counted = history_count(&mut client).await;
    assert!(counted.is_ok(), "history must answer after restart");
    let Ok((after, _, _)) = counted else {
        return;
    };
    assert!(
        after == before,
        "restart must preserve history ({before} -> {after})"
    );

    let stale = ask(
        &mut client,
        WirePayload::SubmitTextInput(cmds::submit_input(
            Some(round_wire),
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

/// Locates a sibling binary built by the workspace: integration tests run
/// from `target/debug/deps`, so the binaries live two levels up. Requires
/// a prior `cargo build` (`cargo test` alone does not link binaries); CI
/// builds the workspace before testing for exactly this reason.
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
/// network or keys in CI): the send path stays with the lib-level E2E, and
/// this test proves pairing approval, setup, views, and graceful
/// degradation end to end through both binaries.
#[tokio::test]
async fn binaries_drive_pairing_setup_and_views() {
    let temp = tempfile::TempDir::new();
    assert!(temp.is_ok(), "tempdir must create");
    let Ok(temp) = temp else {
        return;
    };
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
    // Fake provider key for the SERVER child only (setting a child env is
    // safe; our own process env is never touched): production reads the
    // bearer from its environment, so without this the credential gate
    // would deny assignment. No inference runs in these flows, hence no
    // network is ever touched — the key only satisfies presence checks.
    server.env("ENE_OPENAI_API_KEY", "sk-test-only");
    server.args(["serve", "--config", &config]);
    server.stdout(std::process::Stdio::null());
    server.stderr(std::process::Stdio::null());
    let server = server.spawn();
    assert!(server.is_ok(), "serve must spawn");
    let Ok(server) = server else {
        return;
    };
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
    assert!(listed.is_ok(), "approve-device list must spawn");
    let Ok(listed) = listed else {
        return;
    };
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
    let Some(descriptor) = descriptor else {
        return;
    };
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
    assert!(approved.is_ok(), "approve must spawn");
    let Ok(approved) = approved else {
        return;
    };
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
    let Some(secret) = secret else {
        return;
    };
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
    let Some((_, out, _)) = show else {
        return;
    };
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
    assert!(credential_approved.is_ok(), "credential approve must spawn");
    let Ok(credential_approved) = credential_approved else {
        return;
    };
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
    let Some((_, out, _)) = setup else {
        return;
    };
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
}

#[tokio::test]
async fn tampered_secret_cannot_authenticate() {
    let temp = tempfile::TempDir::new();
    assert!(temp.is_ok(), "tempdir must create");
    let Ok(temp) = temp else {
        return;
    };
    let dir = temp.path().to_path_buf();
    let Some(handle) = open_host(&dir).await else {
        return;
    };
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
    let Some(approver) = open_host(&dir).await else {
        return;
    };
    let approval = approver.approve_device(DESCRIPTOR).await;
    assert!(approval.is_ok(), "approve must succeed");
    let Ok(Some((record, _secret))) = approval else {
        return;
    };
    let stored = store_device(
        &dir,
        &StoredDevice::new(
            DeviceWireId(record.id.0.as_uuid()),
            String::from("wrong-secret-not-from-approve"),
        ),
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
    assert!(temp.is_ok(), "tempdir must create");
    let Ok(temp) = temp else {
        return;
    };
    let dir = temp.path().to_path_buf();
    let Some(handle) = open_host(&dir).await else {
        return;
    };
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
    let Some(approver) = open_host(&dir).await else {
        return;
    };
    let provisioned = approve_and_provision(&dir, &approver).await;
    assert!(provisioned.is_ok(), "approval must pair: {provisioned:?}");
    let connected = Client::connect(&dir, DESCRIPTOR, "test").await;
    assert!(connected.is_ok(), "provisioned connect must succeed");
    drop(connected);

    let reapproved = approver.approve_device(DESCRIPTOR).await;
    assert!(reapproved.is_ok(), "re-approval must succeed");
    let Ok(Some(_)) = reapproved else {
        return;
    };
    let stale_file = Client::connect(&dir, DESCRIPTOR, "test").await;
    assert!(
        matches!(stale_file, Err(CliError::ServerOutcome(_))),
        "rotated secret must invalidate the old file"
    );
    server.abort();
}
