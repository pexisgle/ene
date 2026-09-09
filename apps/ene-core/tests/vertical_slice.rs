//! Production-path end to end for Stage 2.
//!
//! Real listener socket, real `ene-ctl` client builders and session, real
//! Host orchestration; only the provider HTTP transport is fake. Covers:
//! socket placement, pairing approval with the one-time secret, the full
//! challenge/proof/accepted handshake, setup register/assign/complete, text
//! round with ordered streaming, presentation confirmation draining
//! undelivered, history, restart restore with stale old rounds, tampered
//! secrets, and untrusted-peer denial.
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

async fn recv(client: &mut Client, what: &str) -> Result<WirePayload, String> {
    match tokio::time::timeout(Duration::from_secs(10), client.next_frame()).await {
        Ok(Ok(payload)) => Ok(payload),
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

/// Runs pairing approval and provisions the device file, returning the
/// connected client. Fails loudly at whichever step breaks.
async fn paired_client(dir: &std::path::Path, handle: &HostHandle) -> Result<Client, String> {
    let pending = Client::connect(dir, DESCRIPTOR, "test").await;
    assert!(
        matches!(pending, Err(CliError::ServerOutcome(_))),
        "first pairing must pend"
    );
    let approved = handle
        .approve_device(DESCRIPTOR)
        .await
        .map_err(|error| format!("approve failed: {error:?}"))?;
    let Some((record, secret)) = approved else {
        return Err(String::from("approval must pair"));
    };
    store_device(
        dir,
        &StoredDevice::new(DeviceWireId(record.id.0.as_uuid()), secret),
    )
    .map_err(|error| format!("device file must store: {error:?}"))?;
    let connected = Client::connect(dir, DESCRIPTOR, "test").await;
    match connected {
        Ok(client) => Ok(client),
        Err(error) => {
            assert!(
                format!("{error:?}").is_empty(),
                "second connect must succeed: {error:?}"
            );
            Err(String::from("second connect failed"))
        }
    }
}

async fn setup_step(handle: &HostHandle, client: &mut Client) -> Result<(), String> {
    let _ = handle;
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
                ManagementOutcome::AppliedAsOneTime
            ))
        ),
        "register must apply, got {register:?}"
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
        let frame = recv(client, "stream frame").await?;
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

#[tokio::test]
async fn production_path_setup_to_restart() {
    let temp = tempfile::TempDir::new();
    assert!(temp.is_ok(), "tempdir must create");
    let Ok(temp) = temp else {
        return;
    };
    let dir = temp.path().to_path_buf();
    let transport = Arc::new(fake_transport());
    let opened = HostHandle::open_with_cred_store(&dir, CredStore::Memory(memory_store())).await;
    assert!(opened.is_ok(), "host must open");
    let Ok(handle) = opened else {
        return;
    };
    let handle = Arc::new(handle);
    let server = tokio::spawn(conn::run(
        dir.clone(),
        Arc::clone(&handle),
        Arc::clone(&transport),
    ));
    assert!(wait_for_socket(&dir).await, "listener must bind ene.sock");
    assert!(
        !dir.join("ene.sock").join("ene.sock").exists(),
        "socket path must not double-append"
    );

    let client = paired_client(&dir, &handle).await;
    assert!(client.is_ok(), "pairing flow must connect");
    let Ok(mut client) = client else {
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
    let setup = setup_step(&handle, &mut client).await;
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

    let db = dir.join("app.db");
    let mut drained = false;
    for _ in 0..100 {
        let Ok(store) = Store::open(&db).await else {
            break;
        };
        let Ok(companion) = store.ensure_running_companion().await else {
            break;
        };
        let Ok(pending) = store.list_pending(companion).await else {
            break;
        };
        if pending.is_empty() {
            drained = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(drained, "confirmed reply must not linger undelivered");

    let history = ask(
        &mut client,
        WirePayload::HistoryRequest(cmds::history_request(50)),
        "history",
    )
    .await;
    assert!(
        matches!(&history, Ok(WirePayload::HistoryView(_))),
        "history must answer, got {history:?}"
    );
    let Ok(WirePayload::HistoryView(view)) = history else {
        return;
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
    assert!(owner_seen && companion_seen, "history must hold both sides");
    let before = view.items.len();
    assert!(before >= 2, "history must hold the round, got {before}");

    drop(client);
    server.abort();
    tokio::task::yield_now().await;
    drop(std::fs::remove_file(dir.join("ene.sock")));
    let memory = memory_store();
    let reopened = HostHandle::open_with_cred_store(&dir, CredStore::Memory(memory)).await;
    assert!(reopened.is_ok(), "reopen must succeed");
    let Ok(handle) = reopened else {
        return;
    };
    let handle = Arc::new(handle);
    let transport = Arc::new(fake_transport());
    let server = tokio::spawn(conn::run(
        dir.clone(),
        Arc::clone(&handle),
        Arc::clone(&transport),
    ));
    assert!(
        wait_for_socket(&dir).await,
        "listener must rebind after restart"
    );

    // Restart drops the transient device-auth store (Group K, E): the Owner
    // re-approves (rotation) and re-provisions, exactly the real flow.
    // History and pairing records survive (durable); live secrets do not.
    let reapproved = handle.approve_device(DESCRIPTOR).await;
    assert!(reapproved.is_ok(), "re-approval must succeed");
    let Ok(Some((record, secret))) = reapproved else {
        return;
    };
    let stored = store_device(
        &dir,
        &StoredDevice::new(DeviceWireId(record.id.0.as_uuid()), secret),
    );
    assert!(stored.is_ok(), "device file must re-provision");

    let reconnected = Client::connect(&dir, DESCRIPTOR, "test").await;
    let mut client = match reconnected {
        Ok(client) => client,
        Err(error) => {
            assert!(
                format!("{error:?}").is_empty(),
                "reconnect must succeed: {error:?}"
            );
            return;
        }
    };
    let history = ask(
        &mut client,
        WirePayload::HistoryRequest(cmds::history_request(50)),
        "history",
    )
    .await;
    assert!(
        matches!(&history, Ok(WirePayload::HistoryView(_))),
        "history must answer after restart, got {history:?}"
    );
    let Ok(WirePayload::HistoryView(view)) = history else {
        return;
    };
    assert!(
        view.items.len() == before,
        "restart must preserve history ({} -> {})",
        before,
        view.items.len()
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

#[tokio::test]
async fn tampered_secret_cannot_authenticate() {
    let temp = tempfile::TempDir::new();
    assert!(temp.is_ok(), "tempdir must create");
    let Ok(temp) = temp else {
        return;
    };
    let dir = temp.path().to_path_buf();
    let transport = Arc::new(fake_transport());
    let opened = HostHandle::open_with_cred_store(&dir, CredStore::Memory(memory_store())).await;
    assert!(opened.is_ok(), "host must open");
    let Ok(handle) = opened else {
        return;
    };
    let handle = Arc::new(handle);
    let server = tokio::spawn(conn::run(
        dir.clone(),
        Arc::clone(&handle),
        Arc::clone(&transport),
    ));
    assert!(wait_for_socket(&dir).await, "listener must bind");

    let pending = Client::connect(&dir, DESCRIPTOR, "test").await;
    assert!(
        matches!(pending, Err(CliError::ServerOutcome(_))),
        "first pairing must pend"
    );
    let approved = handle.approve_device(DESCRIPTOR).await;
    assert!(approved.is_ok(), "approve must succeed");
    let Ok(Some((record, _secret))) = approved else {
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
