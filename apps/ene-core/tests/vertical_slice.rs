//! Production-path end to end for Stage 2.
//!
//! Real listener socket, real `ene-ctl` client builders and session, real
//! Host orchestration; only the provider HTTP transport is fake. Covers
//! cross-process pairing approval (an INDEPENDENT approval context sharing
//! the file device-auth store), the full challenge/proof handshake, setup,
//! rounds with ordered streaming, restart without re-approval, lost-origin
//! recovery, tampering, and untrusted-peer denial.
//!
//! Unix-only: these production-path tests drive the Unix socket listener.
//! The Windows named-pipe listener shares the same handshake and phase path;
//! its transport subset runs in
//! [`stage5_windows_pipe_e2e.rs`](stage5_windows_pipe_e2e.rs) on Windows.

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
use ene_api::v1::round::{
    HistoryResponse, PresentationStatus, RoundIntakeOutcomeWire, StreamClose,
};
use ene_companion::{CompanionRepository, UndeliveredRepository};
use ene_core::conn;
use ene_core::host_control;
use ene_core::serve::{CredStore, HostHandle};
use ene_credential::MemoryVersionedStore;
use ene_credential::{CredentialRef, MemoryCredentialStore};
use ene_ctl::client::{Client, ClientError};
use ene_ctl::cmds;
use ene_ctl::device::{StoredDevice, store_device};
use ene_inference::RawUsage;
use ene_inference::fake::FakeProviderTransport;
use ene_local_control::{ControlOp, FromConfirmation, ToConfirmation};
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
            cached_input_tokens: 1,
            output_tokens: 3,
        }),
    )
}

/// One durable usage fact read straight from the Host database:
/// `(source, input, cached, output)`.
type UsageFactRow = (String, Option<i64>, Option<i64>, Option<i64>);

/// Durable token accounting read straight from the Host database.
fn usage_fact_rows(dir: &std::path::Path) -> Vec<UsageFactRow> {
    let conn = rusqlite::Connection::open(dir.join("app.db"))
        .expect("the store file must open for the probe");
    let mut statement = conn
        .prepare("SELECT source, input_tokens, cached_input_tokens, output_tokens FROM usage_fact ORDER BY ticket")
        .expect("the usage probe statement must prepare");
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })
        .expect("the usage probe must run");
    rows.collect::<Result<Vec<_>, _>>()
        .expect("the usage probe rows must decode")
}

#[tokio::test]
async fn dialogue_and_learning_calls_share_one_ticket_accounting_path() {
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

    let pending = begin_pairing(&dir).await.expect("first pairing must pend");
    let mut client = approve_and_complete(pending, &handle)
        .await
        .expect("approval must pair");
    let approver = open_host(&dir).await.unwrap();
    let setup = setup_flow(&mut client, &approver).await;
    assert!(setup.is_ok(), "setup must complete: {setup:?}");
    drop(client);
    let assigned = stage3_assign_learning(&dir).await;
    assert!(
        assigned.is_ok(),
        "learning assignment must store: {assigned:?}"
    );

    // One dialogue turn plus its formation pass: both go through the shared
    // dispatch, so both settle exactly one durable usage fact per ticket.
    transport.push_learning(
        r#"{"summary": "The owner prefers jasmine tea in the morning.", "memories": [{"action": "create", "content": "The owner prefers jasmine tea in the morning.", "importance": 4, "temporal": "enduring"}]}"#,
    );
    let sent = stage3_send(&dir, "remember that I prefer jasmine tea in the morning").await;
    assert!(sent.is_ok(), "the memory turn must complete: {sent:?}");
    transport.wait_learning().await;
    stage3_wait_for_memory(&dir, "The owner prefers jasmine tea in the morning.").await;

    let rows = usage_fact_rows(&dir);
    assert_eq!(
        rows.len(),
        2,
        "one dialogue call and one learning call, one fact each: {rows:?}"
    );
    for (source, input, cached, output) in &rows {
        assert_eq!(source, "reported", "the transport reports complete counts");
        assert_eq!((*input, *cached, *output), (Some(7), Some(1), Some(3)));
        assert!(*cached <= *input, "cached input stays a subset of input");
    }

    // Restart must not re-account or lose the durable facts.
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
    assert_eq!(
        usage_fact_rows(&dir),
        rows,
        "the accounting rows survive restart unchanged"
    );
    server.abort();
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

/// Waits until a listener actually accepts on the socket path: a stopped
/// Host leaves the path behind, so existence alone is not readiness.
async fn wait_for_listener(dir: &std::path::Path) -> bool {
    for _ in 0..100 {
        if tokio::net::UnixStream::connect(dir.join("ene.sock"))
            .await
            .is_ok()
        {
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
        confirmed: false,
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

async fn begin_pairing(
    dir: &std::path::Path,
) -> Result<ene_ctl::client::PendingPairingClient, String> {
    match Client::begin_connect(dir, DESCRIPTOR, "test")
        .await
        .map_err(|error| format!("pairing connect failed: {error:?}"))?
    {
        ene_ctl::client::ConnectProgress::Pending(pending) => Ok(pending),
        ene_ctl::client::ConnectProgress::Connected(_) => {
            Err(String::from("first pairing unexpectedly authenticated"))
        }
    }
}

async fn approve_and_complete(
    pending: ene_ctl::client::PendingPairingClient,
    approver: &HostHandle,
) -> Result<Client, String> {
    let pending_id = pending.pending_id().to_owned();
    let approved = approver
        .approve_device(&pending_id)
        .await
        .map_err(|error| format!("approve failed: {error:?}"))?;
    if approved.is_none() {
        return Err(String::from("approval must pair"));
    }
    pending
        .complete()
        .await
        .map_err(|error| format!("provision completion failed: {error:?}"))
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
        matches!(
            &history,
            Ok(WirePayload::HistoryResponse(HistoryResponse::Items(_)))
        ),
        "history must answer, got {history:?}"
    );
    let Ok(WirePayload::HistoryResponse(HistoryResponse::Items(items))) = history else {
        return Err(String::from("history answered nothing usable"));
    };
    let mut owner_seen = false;
    let mut companion_seen = false;
    for item in &items {
        if item.role == ene_api::v1::round::HistoryRole::Owner {
            owner_seen = true;
        }
        if item.role == ene_api::v1::round::HistoryRole::Companion {
            companion_seen = true;
        }
    }
    Ok((items.len(), owner_seen, companion_seen))
}

async fn pending_empty(dir: &std::path::Path) -> bool {
    for _ in 0..100 {
        let Ok(store) = Store::open(&dir.join("app.db")).await else {
            break;
        };
        let Ok(companion) = store.ensure_running_companion().await else {
            break;
        };
        let Ok(page) = store
            .list_unpresented(companion, None, ene_companion::UNDELIVERED_PAGE_MAX)
            .await
        else {
            break;
        };
        if page.entries.is_empty() {
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

    let pending = begin_pairing(&dir).await.expect("first pairing must pend");
    let mut client = approve_and_complete(pending, &handle)
        .await
        .expect("approval must pair");
    let approver = open_host(&dir).await.unwrap();

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

    let serving = ServingHost::start(&dir, Arc::new(fake_transport())).await;
    let bound = true;
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

    let mut status_command = tokio::process::Command::new(&ctl);
    status_command
        .args(["--config", &config, "status"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let status_process = status_command.spawn().expect("status client must start");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let pending_id = loop {
        let listed = std::process::Command::new(&core)
            .args(["approve-device", "--config", &config])
            .output()
            .expect("listing pendings must run");
        assert!(listed.status.success(), "listing pendings must exit 0");
        let pending_out = String::from_utf8_lossy(&listed.stdout);
        if let Some(id) = pending_out
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .and_then(|line| line.split_whitespace().next().map(str::to_string))
        {
            break id;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "pending never listed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    // The console asks as a requester; the Owner confirms on the private
    // channel the Host issued to its GUI, and the one-time provision comes
    // back there rather than to the console. The list names opaque pending IDs
    // first (the descriptor after it is display-only); approval names the ID.
    let gui = host_control::seat_test_gui_for_tests(serving.handle()).expect("private channel");
    let mut approve = tokio::process::Command::new(&core);
    approve
        .args([
            "approve-device",
            "--pending",
            pending_id.as_str(),
            "--config",
            &config,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let approve = approve.spawn().expect("the requester must start");
    let (session_id, nonce) = read_challenge(&gui, ControlOp::DeviceApprove).await;
    owner_send(&gui, ToConfirmation::SessionComplete { session_id, nonce }).await;
    let reply = owner_recv(&gui).await;
    let FromConfirmation::Outcome(ene_local_control::ControlOutcome::DeviceApproved { .. }) = reply
    else {
        panic!("the Owner's confirmation must approve the pending, got {reply:?}");
    };
    let approved = approve
        .wait_with_output()
        .await
        .expect("the requester exits");
    assert!(
        approved.status.success(),
        "approve-device must exit 0: {}",
        String::from_utf8_lossy(&approved.stderr)
    );
    let shown = String::from_utf8_lossy(&approved.stdout).into_owned();
    assert!(!shown.contains("pairing_secret"));

    let status_output =
        tokio::time::timeout(Duration::from_secs(10), status_process.wait_with_output())
            .await
            .expect("originating status client must finish")
            .expect("status output must collect");
    let status = Some((
        status_output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&status_output.stdout).into_owned(),
        String::from_utf8_lossy(&status_output.stderr).into_owned(),
    ));
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

    // Credential registration is the same shape: the console asks as a
    // requester, the Owner types the value on the private channel, and the
    // Host publishes it as a version before answering.
    let gui = host_control::seat_test_gui_for_tests(serving.handle()).expect("private channel");
    let mut approve_cred = tokio::process::Command::new(&core);
    approve_cred
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
        .stderr(std::process::Stdio::piped());
    let approve_cred = approve_cred.spawn().expect("the requester must start");
    let (session_id, nonce) = read_challenge(&gui, ControlOp::CredentialPut).await;
    owner_send(
        &gui,
        ToConfirmation::CredentialSecret {
            session_id,
            nonce: nonce.clone(),
            provider: String::from("openai"),
            label: String::from("main"),
            secret: ene_local_control::RedactedSecret::new("sk-test-only"),
        },
    )
    .await;
    let staged = owner_recv(&gui).await;
    assert!(
        matches!(
            staged,
            FromConfirmation::Outcome(ene_local_control::ControlOutcome::CredentialStaged { .. })
        ),
        "intake must stage the value before the completion, got {staged:?}"
    );
    owner_send(&gui, ToConfirmation::SessionComplete { session_id, nonce }).await;
    let published = owner_recv(&gui).await;
    assert!(
        matches!(
            published,
            FromConfirmation::Outcome(ene_local_control::ControlOutcome::CredentialStored { .. })
        ),
        "the Owner's completion must publish the credential, got {published:?}"
    );
    let credential_approved = approve_cred
        .wait_with_output()
        .await
        .expect("the requester exits");
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
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let handle = open_host(&dir).await.unwrap();
    let server = tokio::spawn(conn::run(
        dir.clone(),
        Arc::clone(&handle),
        Arc::new(fake_transport()),
    ));
    assert!(wait_for_socket(&dir).await, "listener must bind");

    let pending = begin_pairing(&dir).await.expect("first pairing must pend");
    let client = approve_and_complete(pending, &handle)
        .await
        .expect("approval must authenticate");
    drop(client);
    let stored = ene_ctl::device::load_stored_device(&dir);
    let ene_ctl::device::DeviceFileState::Loaded(stored) = stored else {
        panic!("accepted pairing must persist a device file");
    };
    store_device(
        &dir,
        &StoredDevice::new(
            stored.device_id,
            String::from("wrong-secret-not-from-approve"),
        ),
    )
    .expect("test device file must store");
    let tampered = Client::connect(&dir, DESCRIPTOR, "test").await;
    assert!(matches!(tampered, Err(ClientError::ServerOutcome(_))));
    server.abort();
}

#[tokio::test]
async fn lost_origin_is_not_redelivered_and_fresh_pairing_succeeds() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let handle = open_host(&dir).await.unwrap();
    let server = tokio::spawn(conn::run(
        dir.clone(),
        Arc::clone(&handle),
        Arc::new(fake_transport()),
    ));
    assert!(wait_for_socket(&dir).await, "listener must bind");

    let lost = begin_pairing(&dir).await.expect("first pairing must pend");
    let lost_id = lost.pending_id().to_owned();
    drop(lost);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let still_pending = handle
            .pending_devices()
            .await
            .unwrap()
            .iter()
            .any(|entry| entry.pending_id == lost_id);
        if !still_pending {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "disconnect cleanup timed out"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(handle.approve_device(&lost_id).await.unwrap().is_none());

    let fresh = begin_pairing(&dir).await.expect("fresh pairing must pend");
    assert_ne!(fresh.pending_id(), lost_id);
    let client = approve_and_complete(fresh, &handle)
        .await
        .expect("fresh origin must authenticate");
    drop(client);
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
                    "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":7,\"output_tokens\":9,\"input_tokens_details\":{\"cached_tokens\":2}}}}\n\n",
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

/// One in-process serving Host for the binary-level suites.
///
/// The Owner's confirmation surface is a process the Host itself spawned and
/// handed the private channel to. A headless test cannot produce that through a
/// spawned Host binary, so these suites serve in process and drive the Owner's
/// channel through the same registration path the Host uses for its own child.
/// The client side stays the real binaries.
struct ServingHost {
    dir: std::path::PathBuf,
    handle: Arc<HostHandle>,
    transport: Arc<FakeProviderTransport>,
    shutdown: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<Result<(), ene_core::serve::CoreError>>,
    /// The single-writer lock the production `serve` takes. The console's
    /// `approve-*` commands probe it to tell a serving Host from a stopped one,
    /// so an in-process Host must hold it exactly as the binary does.
    _lock: ene_core::host_lock::HostLock,
}

impl ServingHost {
    async fn start(dir: &std::path::Path, transport: Arc<FakeProviderTransport>) -> Self {
        let lock = ene_core::host_lock::HostLock::acquire(dir)
            .expect("the data directory must be free to serve");
        let handle = Arc::new(
            HostHandle::open_with_cred_store(
                dir,
                CredStore::MemoryVersioned(MemoryVersionedStore::new()),
            )
            .await
            .expect("the Host state must open"),
        );
        // The production serving boundary runs its startup mutations before
        // the listener binds; the in-process Host keeps that order.
        handle
            .run_startup_mutations()
            .await
            .expect("the startup mutations must complete");
        let mut serving = Self {
            dir: dir.to_path_buf(),
            handle,
            transport,
            shutdown: tokio::sync::watch::channel(false).0,
            task: tokio::spawn(async { Ok(()) }),
            _lock: lock,
        };
        serving.spawn_task();
        assert!(
            wait_for_listener(&serving.dir).await,
            "listener must bind ene.sock"
        );
        serving
    }

    fn spawn_task(&mut self) {
        let (shutdown, rx) = tokio::sync::watch::channel(false);
        self.shutdown = shutdown;
        self.task = tokio::spawn(conn::run_until_shutdown(
            self.dir.clone(),
            Arc::clone(&self.handle),
            Arc::clone(&self.transport),
            rx,
        ));
    }

    fn handle(&self) -> &Arc<HostHandle> {
        &self.handle
    }

    async fn stop(&mut self) {
        self.shutdown.send_replace(true);
        tokio::time::timeout(Duration::from_secs(30), &mut self.task)
            .await
            .expect("the in-process Host must stop within the timeout")
            .expect("the in-process Host task must join")
            .expect("the in-process Host must stop cleanly");
    }
}

/// Reads one challenge from the private channel the Host issued to its GUI.
async fn read_challenge(
    gui: &ene_local_control::GuiChannel,
    expected: ControlOp,
) -> (uuid::Uuid, String) {
    let mut channel = gui.try_clone().expect("the private channel clones");
    let frame = tokio::task::spawn_blocking(move || channel.recv())
        .await
        .expect("the reader joins")
        .expect("the channel reads")
        .expect("a live channel carries the challenge");
    match frame {
        FromConfirmation::ConfirmationChallenge {
            session_id,
            op,
            nonce,
            ..
        } => {
            assert_eq!(
                op, expected,
                "the challenge must name the operation asked for"
            );
            (session_id, nonce)
        }
        other => panic!("expected a challenge, got {other:?}"),
    }
}

/// Sends one Owner frame on the private channel.
async fn owner_send(gui: &ene_local_control::GuiChannel, frame: ToConfirmation) {
    let mut channel = gui.try_clone().expect("the private channel clones");
    tokio::task::spawn_blocking(move || channel.send(&frame))
        .await
        .expect("the writer joins")
        .expect("the channel writes");
}

/// Reads one answer on the private channel.
async fn owner_recv(gui: &ene_local_control::GuiChannel) -> FromConfirmation {
    let mut channel = gui.try_clone().expect("the private channel clones");
    tokio::task::spawn_blocking(move || channel.recv())
        .await
        .expect("the reader joins")
        .expect("the channel reads")
        .expect("a live channel carries the answer")
}

async fn pair_via_binaries(
    ctl: &std::path::Path,
    core: &std::path::Path,
    config: &str,
    serving: &ServingHost,
) -> Option<()> {
    let mut status_command = tokio::process::Command::new(ctl);
    status_command
        .args(["--config", config, "status"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let status_process = status_command.spawn().ok()?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let pending_id = loop {
        let listed = std::process::Command::new(core)
            .args(["approve-device", "--config", config])
            .output()
            .ok()?;
        let out = String::from_utf8_lossy(&listed.stdout);
        if let Some(id) = out
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .and_then(|line| line.split_whitespace().next().map(str::to_string))
        {
            break id;
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let gui = host_control::seat_test_gui_for_tests(serving.handle()).ok()?;
    let mut approve = tokio::process::Command::new(core);
    approve
        .args([
            "approve-device",
            "--pending",
            &pending_id,
            "--config",
            config,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let approve = approve.spawn().ok()?;
    let (session_id, nonce) = read_challenge(&gui, ControlOp::DeviceApprove).await;
    owner_send(&gui, ToConfirmation::SessionComplete { session_id, nonce }).await;
    let reply = owner_recv(&gui).await;
    assert!(matches!(
        reply,
        FromConfirmation::Outcome(ene_local_control::ControlOutcome::DeviceApproved { .. })
    ));
    let approved = approve.wait_with_output().await.ok()?;
    assert!(approved.status.success());
    assert!(!String::from_utf8_lossy(&approved.stdout).contains("pairing_secret"));
    let status = tokio::time::timeout(Duration::from_secs(10), status_process.wait_with_output())
        .await
        .ok()?
        .ok()?;
    assert!(
        status.status.success(),
        "originating client must authenticate"
    );
    Some(())
}

/// The client device file is commit-on-acceptance and atomically replaced:
/// Provisions one paired device into a data directory no Host is serving, as a
/// completed first pairing leaves it.
///
/// The first pairing itself needs the Owner's confirmation surface, which the
/// in-process suites drive directly; a suite that must run the real Host
/// binary (to exercise the production HTTP transport, for example) starts from
/// the state pairing leaves behind instead of re-opening the flow.
async fn provision_paired_device(
    dir: &std::path::Path,
    descriptor: &str,
) -> (DeviceWireId, String) {
    use ene_credential::{DevicePairingRepository as _, FileDeviceAuthStore};
    let store = Store::open(&dir.join("app.db"))
        .await
        .expect("the store must open offline");
    let origin = String::from("fixture-origin");
    let pending = store
        .request_pairing(descriptor.to_string(), origin.clone())
        .await
        .expect("a pairing request must record");
    let (record, secret) = store
        .approve_pending(&pending.pending_id, &origin)
        .await
        .expect("the approval must run")
        .expect("the pending must pair");
    let auth = FileDeviceAuthStore::open(&dir.join("device-auth.json"))
        .expect("the device-auth store must open");
    auth.save_secret(&record.id, &record.descriptor, secret.expose_secret())
        .expect("the pairing secret must be custodied");
    let wire = record.wire.parse().expect("the wire id must be UUID text");
    let secret = secret.into_inner();
    store_device(dir, &StoredDevice::new(DeviceWireId(wire), secret.clone()))
        .expect("the client device file must store");
    (DeviceWireId(wire), secret)
}

/// Creates one usable credential ref, as a completed Owner registration leaves
/// it: the pending pair is staged and then approved with its sweep.
async fn provision_credential(dir: &std::path::Path, provider: &str, label: &str, bearer: &str) {
    use ene_credential::{
        CredentialIntentRepository as _, RegistrationApply, RegistrationFingerprint,
        RegistrationState,
    };
    let store = Store::open(&dir.join("app.db"))
        .await
        .expect("the store must open offline");
    let fingerprint = RegistrationFingerprint {
        intent_id: uuid::Uuid::new_v4().as_hyphenated().to_string(),
        kind: String::from("register"),
        target: format!("credential:{provider}:{label}"),
        base: String::from("fixture"),
        rationale_origin: String::from("management-surface"),
        rationale_quote: None,
    };
    let staged = store
        .request_registration_with_intent(provider.to_string(), label.to_string(), fingerprint)
        .await
        .expect("the registration must stage");
    assert!(
        matches!(
            staged,
            RegistrationApply::Decided(RegistrationState::HeldByOperation)
        ),
        "a fresh pair must pend Owner approval, got {staged:?}"
    );
    assert!(
        store
            .approve_credential_with_sweep(provider, label, bearer)
            .expect("the approval must commit"),
        "the approval makes the pair usable"
    );
}
/// A malformed credential file is never repaired through an out-of-band
/// secret; deleting it and completing a fresh live pairing publishes a new
/// file only after accepted authentication.
#[tokio::test]
async fn binaries_keep_the_device_file_until_a_proof_is_accepted() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let binaries = (workspace_binary("ene-ctl"), workspace_binary("ene-core"));
    assert!(binaries.0.is_some() && binaries.1.is_some());
    let (Some(ctl), Some(core)) = binaries else {
        return;
    };
    let Some(config) = write_test_config(&dir) else {
        return;
    };
    let mut serving = ServingHost::start(&dir, Arc::new(fake_transport())).await;

    assert!(
        pair_via_binaries(&ctl, &core, &config, &serving)
            .await
            .is_some()
    );
    let device_file = ene_ctl::device::device_file_path(&dir);
    let provisioned = std::fs::read(&device_file).unwrap_or_default();
    assert!(!provisioned.is_empty());
    let valid = run_cli(
        &ctl,
        &["--config", &config, "status"],
        &[],
        Duration::from_secs(10),
    )
    .await;
    assert!(matches!(valid, Some((0, _, _))));

    std::fs::write(&device_file, b"{not json").expect("malformed fixture must write");
    let malformed = run_cli(
        &ctl,
        &["--config", &config, "status"],
        &[],
        Duration::from_secs(10),
    )
    .await;
    let Some((2, _, stderr)) = malformed else {
        panic!("malformed file must fail");
    };
    assert!(stderr.contains("unreadable or malformed"));
    assert!(stderr.contains("fresh pairing request"));
    assert_eq!(
        std::fs::read(&device_file).unwrap_or_default(),
        b"{not json"
    );

    std::fs::remove_file(&device_file).expect("owner reset must remove degraded file");
    assert!(
        pair_via_binaries(&ctl, &core, &config, &serving)
            .await
            .is_some()
    );
    let repaired = std::fs::read(&device_file).unwrap_or_default();
    assert!(!repaired.is_empty());
    assert_ne!(
        repaired, provisioned,
        "fresh pairing must publish new credentials"
    );
    serving.stop().await;
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

    // This suite runs the real Host binary to exercise the production HTTP
    // transport, which an in-process Host cannot provide. Pairing and the
    // credential registration need the Owner's confirmation surface, so they
    // are provisioned as the state those flows leave behind; the flows
    // themselves are covered where the surface exists.
    provision_paired_device(&dir, "production transport laptop").await;
    provision_credential(&dir, "openai", "main", "sk-test-only").await;

    let server = spawn_serve_binary(&core, &config, &server_env);
    assert!(server.is_some(), "serve must spawn");
    let Some(server) = server else {
        fake.abort();
        return;
    };
    assert!(wait_for_socket(&dir).await, "listener must bind ene.sock");

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
        matches!(setup, Some((0, _, _))),
        "setup with a registered credential must exit 0, got {setup:?}"
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
    // Every production send settled exactly one usage fact with the cached
    // subset decoded from the provider's SSE completion, and restart kept
    // the first settlement terminal.
    let rows = usage_fact_rows(&dir);
    assert_eq!(rows.len(), 2, "two sends, two durable facts: {rows:?}");
    for (source, input, cached, output) in &rows {
        assert_eq!(
            (source.as_str(), *input, *cached, *output),
            ("reported", Some(7), Some(2), Some(9)),
            "the production SSE completion carries input, cached subset, and output: {rows:?}"
        );
    }
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
        Box::pin(async move {
            Ok(ene_inference::ProviderResponse {
                text,
                usage: Some(ene_inference::RawUsage {
                    input_tokens: 7,
                    cached_input_tokens: 1,
                    output_tokens: 3,
                }),
            })
        })
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
        WirePayload::ManagementViewRequest(cmds::memory_view_request(after, None, None)),
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

/// The first addressable Memory id named by a rendered memory list body.
fn first_memory_id(view: &str) -> String {
    view.lines()
        .find(|line| line.starts_with("memory "))
        .and_then(|line| line.split_whitespace().nth(1))
        .expect("the list names a Memory id")
        .to_owned()
}

/// One bounded page of one Memory's revisions and grounds.
async fn stage3_memory_revisions(
    dir: &std::path::Path,
    memory: &str,
    after_revision: Option<u64>,
) -> String {
    let mut client = stage3_client(dir).await;
    let answer = ask(
        &mut client,
        WirePayload::ManagementViewRequest(cmds::memory_view_request(
            None,
            Some(memory),
            after_revision,
        )),
        "memory revisions",
    )
    .await;
    let Ok(WirePayload::ManagementView(view)) = answer else {
        panic!("memory revisions must answer a view: {answer:?}");
    };
    view.sections
        .iter()
        .find(|section| section.kind == "memory")
        .map(|section| section.body.clone())
        .unwrap_or_default()
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
    let Ok(WirePayload::HistoryResponse(HistoryResponse::Items(items))) = answer else {
        panic!("history must answer items: {answer:?}");
    };
    cmds::render_history(&items)
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

/// Polls one Memory's revision page until `expected` appears.
async fn stage3_wait_for_revisions(dir: &std::path::Path, expected: &str) -> String {
    for _ in 0..100 {
        let view = stage3_view(dir).await;
        for memory_id in view
            .lines()
            .filter_map(|line| line.strip_prefix("memory "))
            .filter_map(|rest| rest.split_whitespace().next())
        {
            let revisions = stage3_memory_revisions(dir, memory_id, None).await;
            if revisions.contains(expected) {
                return revisions;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the revision page never showed {expected:?}");
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

    let pending = begin_pairing(&dir).await.expect("first pairing must pend");
    let mut client = approve_and_complete(pending, &handle)
        .await
        .expect("approval must pair");
    let approver = open_host(&dir).await.unwrap();
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
    assert!(
        !view.contains("grounds summary") && !view.contains("rev1"),
        "the list page stays current recognition plus metadata: {view}"
    );
    let memory_id = first_memory_id(&view);
    let revisions = stage3_memory_revisions(&dir, &memory_id, None).await;
    assert!(
        revisions.contains("grounds summary"),
        "summary grounds: {revisions}"
    );
    assert!(
        revisions.contains("rev1 initial"),
        "first revision: {revisions}"
    );

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
    let view = stage3_wait_for_revisions(&dir, "rev2 reinforced").await;
    assert_eq!(
        memory_count(&stage3_view(&dir).await),
        1,
        "no duplicate memory"
    );
    let _ = view;

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
    let view = stage3_wait_for_revisions(&dir, "rev4 changed-since").await;
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
    assert!(
        !view.contains("rev1") && !view.contains("grounds summary"),
        "the restarted list still stays current recognition only: {view}"
    );
    let lines: Vec<&str> = view.lines().collect();
    let content_line = lines
        .iter()
        .position(|line| line.starts_with("content: The owner prefers coffee now."))
        .expect("the current recognition is listed");
    let memory_id = lines[..content_line]
        .iter()
        .rev()
        .find(|line| line.starts_with("memory "))
        .and_then(|line| line.split_whitespace().nth(1))
        .expect("the listed content belongs to a Memory");
    let revisions = stage3_memory_revisions(&dir, memory_id, None).await;
    assert!(revisions.contains("rev1 initial"), "{revisions}");
    assert!(
        revisions.contains("rev3 corrected-initially-wrong"),
        "{revisions}"
    );
    assert!(revisions.contains("rev4 changed-since"), "{revisions}");
    assert!(revisions.contains("grounds summary"), "{revisions}");

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
    let view = stage3_wait_for_memory(&dir, "recall=suppressed").await;
    assert!(
        view.contains("content: The owner prefers coffee now."),
        "content is kept: {view}"
    );
    let revisions = stage3_wait_for_revisions(&dir, "rev5 forgotten").await;
    assert!(
        revisions.contains("rev1 initial"),
        "all revisions are kept: {revisions}"
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

    let pending = begin_pairing(&dir).await.expect("first pairing must pend");
    let mut client = approve_and_complete(pending, &handle)
        .await
        .expect("approval must pair");
    let approver = open_host(&dir).await.unwrap();
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
    assert!(
        !second.contains("rev1") && !second.contains("grounds summary"),
        "list pages never expand revisions: {second}"
    );
    let memory_id = first_memory_id(&second);
    let revisions = stage3_memory_revisions(&dir, &memory_id, None).await;
    assert_eq!(
        revisions.matches("rev1 initial").count(),
        1,
        "the requested memory shows its first revision: {revisions}"
    );
    assert!(
        revisions.contains("grounds summary"),
        "the requested memory shows its grounds: {revisions}"
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
