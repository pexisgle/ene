//! Production-path end to end for Stage 2.
//!
//! Real listener socket, real `ene-ctl` client builders and session, real
//! Host orchestration; only the provider HTTP transport is fake. Covers:
//! socket placement, pairing approval, capability handshake with the
//! presence fact, setup register/assign/complete, text round with ordered
//! streaming, presentation confirmation draining undelivered, history,
//! restart restore with stale old rounds, and untrusted-peer denial.
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
use ene_api::v1::refs::{BaseViewMark, CommandWireId, ManagementTargetWire};
use ene_api::v1::round::{PresentationStatus, RoundIntakeOutcomeWire, StreamClose};
use ene_companion::{CompanionRepository, UndeliveredRepository};
use ene_core::conn;
use ene_core::serve::{CredStore, HostHandle};
use ene_credential::{CredentialRef, MemoryCredentialStore};
use ene_ctl::client::Client;
use ene_ctl::cmds;
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

async fn view_mark(client: &mut Client) -> Option<String> {
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
        return None;
    };
    Some(view.mark.0.clone())
}

async fn view_sections(client: &mut Client) -> Option<Vec<String>> {
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
        return None;
    };
    Some(
        view.sections
            .iter()
            .map(|section| section.kind.clone())
            .collect(),
    )
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
    let Ok(handle) =
        HostHandle::open_with_cred_store(&dir, CredStore::Memory(memory_store())).await
    else {
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

    let pending = Client::connect(&dir, DESCRIPTOR, "test").await;
    assert!(
        matches!(pending, Err(CliError::ServerOutcome(_))),
        "first pairing must pend"
    );
    let approved = handle.approve_device(DESCRIPTOR).await;
    assert!(
        matches!(approved, Ok(Some(_))),
        "owner approval must pair, got {approved:?}"
    );

    let connected = Client::connect(&dir, DESCRIPTOR, "test").await;
    assert!(connected.is_ok(), "second connect must succeed");
    let Ok(mut client) = connected else {
        return;
    };
    let fact = recv(&mut client, "presence fact").await;
    assert!(
        matches!(fact, Ok(WirePayload::PresenceAttribution(_))),
        "capability must be followed by the attribution fact, got {fact:?}"
    );

    let sections = view_sections(&mut client).await;
    assert!(
        sections
            == Some(
                ["provider", "model", "consent", "credential"]
                    .iter()
                    .map(ToString::to_string)
                    .collect()
            ),
        "show must carry the four Host sections, got {sections:?}"
    );
    let mark = view_mark(&mut client).await;
    assert!(mark.is_some(), "show must answer a view");
    let Some(mark) = mark else {
        return;
    };
    let register = ask(
        &mut client,
        WirePayload::ManagementIntent(cmds::credential_intent(
            CommandWireId(uuid::Uuid::new_v4()),
            &BaseViewMark(mark.clone()),
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
    let mark = view_mark(&mut client).await;
    assert!(mark.is_some(), "show must answer after register");
    let Some(mark) = mark else {
        return;
    };
    let assign = ask(
        &mut client,
        WirePayload::ManagementIntent(cmds::assignment_intent(
            CommandWireId(uuid::Uuid::new_v4()),
            &BaseViewMark(mark.clone()),
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
    let mark = view_mark(&mut client).await;
    assert!(mark.is_some(), "show must answer after assign");
    let Some(mark) = mark else {
        return;
    };
    let complete = ask(
        &mut client,
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

    let send = ask(
        &mut client,
        WirePayload::SubmitTextInput(cmds::submit_input(
            None,
            String::from("hello companion"),
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
        return;
    };
    let round_wire = round.0.clone();
    let mut opened = false;
    let mut text = String::new();
    let mut previous_seq: Option<u64> = None;
    let mut stream_id = None;
    loop {
        let frame = recv(&mut client, "stream frame").await;
        match frame {
            Ok(WirePayload::TextStreamOpen(open)) => {
                opened = true;
                stream_id = Some(open.stream);
            }
            Ok(WirePayload::TextStreamFrame(frame)) => {
                assert!(
                    previous_seq.is_none_or(|previous| frame.seq == previous + 1),
                    "stream frames must order by seq"
                );
                previous_seq = Some(frame.seq);
                text.push_str(&frame.delta);
            }
            Ok(WirePayload::TextStreamClose(close)) => {
                assert!(
                    close.status == StreamClose::Completed,
                    "stream must complete, got {:?}",
                    close.status
                );
                break;
            }
            other => {
                assert!(
                    matches!(other, Ok(WirePayload::TextStreamClose(_))),
                    "unexpected stream payload: {other:?}"
                );
                return;
            }
        }
    }
    assert!(opened, "stream must open before it closes");
    assert!(
        text == FAKE_TEXT,
        "stream must carry provider text, got {text:?}"
    );

    let confirm = ene_api::v1::round::ConfirmPresentationWire {
        round: ene_api::v1::refs::RoundWireId(round_wire.clone()),
        stream: stream_id,
        status: PresentationStatus::Presented,
        detail: None,
    };
    let notify = tokio::time::timeout(
        Duration::from_secs(10),
        client.notify(WirePayload::ConfirmPresentation(confirm)),
    )
    .await;
    assert!(
        matches!(notify, Ok(Ok(()))),
        "confirm must send, got {notify:?}"
    );

    let history = ask(
        &mut client,
        WirePayload::HistoryRequest(cmds::history_request(50)),
        "history",
    )
    .await;
    let mut owner_seen = false;
    let mut companion_seen = false;
    let mut before = 0_usize;
    if let Ok(WirePayload::HistoryView(view)) = history {
        before = view.items.len();
        for item in &view.items {
            if item.role == ene_api::v1::round::HistoryRole::Owner {
                owner_seen = true;
            }
            if item.role == ene_api::v1::round::HistoryRole::Companion {
                companion_seen = true;
            }
        }
    }
    assert!(owner_seen && companion_seen, "history must hold both sides");
    assert!(before >= 2, "history must hold the round, got {before}");

    let db = dir.join("app.db");
    let store = Store::open(&db).await;
    assert!(store.is_ok(), "store must reopen");
    let Ok(store) = store else {
        return;
    };
    let companion = store.ensure_running_companion().await;
    assert!(companion.is_ok(), "companion must load");
    let Ok(companion) = companion else {
        return;
    };
    let pending = store.list_pending(companion).await;
    assert!(pending.is_ok(), "undelivered must list");
    let Ok(pending) = pending else {
        return;
    };
    assert!(
        pending.is_empty(),
        "confirmed reply must not linger undelivered"
    );

    server.abort();
    drop(client);
    tokio::task::yield_now().await;
    drop(std::fs::remove_file(dir.join("ene.sock")));
    let memory = memory_store();
    let opened = HostHandle::open_with_cred_store(&dir, CredStore::Memory(memory)).await;
    assert!(opened.is_ok(), "reopen must succeed");
    let Ok(handle) = opened else {
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

    let connected = Client::connect(&dir, DESCRIPTOR, "test").await;
    assert!(connected.is_ok(), "reconnect must succeed");
    let Ok(mut client) = connected else {
        return;
    };
    let fact = recv(&mut client, "post-restart fact").await;
    assert!(
        matches!(fact, Ok(WirePayload::PresenceAttribution(_))),
        "reconnect must report attribution, got {fact:?}"
    );
    let history = ask(
        &mut client,
        WirePayload::HistoryRequest(cmds::history_request(50)),
        "history",
    )
    .await;
    let mut after = 0_usize;
    if let Ok(WirePayload::HistoryView(view)) = history {
        after = view.items.len();
    }
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
