//! Credential failure rendering and rotation at the dialogue admission boundary.

use super::{
    LearningAwareTransport, live_input, memory_handle_with, register_assign_complete, submit_frame,
};
use crate::serve::{CredStore, HostHandle};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::round::RoundIntakeOutcomeWire;
use ene_companion::{CompanionId, CompanionRepository as _, HistoryRepository as _, HistoryRole};
use ene_credential::{CredentialRef, CredentialScrubber, MemoryCredentialStore, SecretScrubber};
use ene_learning::LearningTechnicalError;
use ene_presence::PresenceRepository as _;

/// The one registered secret every assertion scans for.
const SUITE_SECRET: &str = "sk-c2c3-7f31";

/// A second value registered mid-test to model an explicit rotation.
const ROTATED_SECRET: &str = "sk-c2c3-9b57";

fn credential() -> CredentialRef {
    CredentialRef::new("openai", "main").expect("the suite fixture ref is valid")
}

/// Opens the suite Host with one pinned but not yet registered value.
async fn suite_handle(tag: &str, secret: &str) -> (HostHandle, tempfile::TempDir) {
    memory_handle_with(tag, |store| store.insert(credential(), secret))
        .await
        .expect("the suite scratch directory must be creatable")
}

/// Replaces the pinned value the running Host serves, modelling an operator
/// value update; the revision is advanced by the approval write separately.
fn pin(handle: &HostHandle, secret: &str) {
    let CredStore::Memory(store) = &handle.cred_store else {
        panic!("the suite always pins a memory credential store");
    };
    store.insert(credential(), secret);
}

/// Rotates the registered pair through the production approval boundary: the
/// sweep and the revision advance commit atomically, then the running Host
/// pins the new value.
fn rotate_registered(handle: &HostHandle, secret: &str) {
    assert!(
        handle
            .store
            .approve_credential_with_sweep("openai", "main", secret)
            .expect("the rotation commit must run"),
        "the rotated pair stays usable"
    );
    pin(handle, secret);
}

fn expect_absent(label: &str, text: &str, secret: &str) {
    assert!(
        !text.contains(secret),
        "{label} must not contain the registered secret: {text}"
    );
}

fn expect_absent_in_frames(label: &str, frames: &[ene_plugin_ipc::WireFrame], secret: &str) {
    for (index, frame) in frames.iter().enumerate() {
        let encoded = ene_plugin_ipc::encode_frame(frame).expect("the frame must encode");
        expect_absent(
            &format!("{label} frame {index} wire bytes"),
            &String::from_utf8_lossy(&encoded),
            secret,
        );
        expect_absent(
            &format!("{label} frame {index} payload debug"),
            &format!("{:?}", frame.payload),
            secret,
        );
        expect_absent(
            &format!("{label} frame {index} envelope debug"),
            &format!("{:?}", frame.envelope),
            secret,
        );
    }
}

/// Counts exact occurrences of `secret` in every text column of every durable
/// table. This is the backstop for surfaces the explicit list might miss: the
/// value is bound as a parameter and compared with `instr`, so the scan is an
/// exact match on the whole registered value, not a fragment probe.
fn durable_occurrences(db_path: &std::path::Path, secret: &str) -> Vec<String> {
    let conn = rusqlite::Connection::open(db_path).expect("the suite app.db must open");
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .expect("the schema probe must prepare")
        .query_map((), |row| row.get(0))
        .expect("the schema probe must run")
        .collect::<Result<_, _>>()
        .expect("the schema rows must decode");
    let mut hits = Vec::new();
    for table in tables {
        let columns: Vec<String> = conn
            .prepare(&format!("PRAGMA table_info(\"{table}\")"))
            .expect("the column probe must prepare")
            .query_map((), |row| row.get(1))
            .expect("the column probe must run")
            .collect::<Result<_, _>>()
            .expect("the column rows must decode");
        for column in columns {
            let sql = format!(
                "SELECT COUNT(*) FROM \"{table}\" WHERE instr(CAST(\"{column}\" AS TEXT), ?1) > 0"
            );
            let count: i64 = conn
                .query_row(&sql, [secret], |row| row.get(0))
                .expect("the leak count must read");
            if count > 0 {
                hits.push(format!("{table}.{column} x{count}"));
            }
        }
    }
    hits
}

async fn running_companion(handle: &HostHandle) -> CompanionId {
    handle
        .store
        .ensure_running_companion()
        .await
        .expect("the suite companion must resolve")
}

async fn current_generation(handle: &HostHandle) -> ene_presence::PresenceGeneration {
    let companion = running_companion(handle).await;
    handle
        .store
        .load_attribution(companion.as_raw())
        .await
        .expect("the attribution must load")
        .expect("the attribution must exist")
        .generation
}

/// Error and debug renderings of the credential boundary itself never carry
/// the value: an unreadable registered bearer fails closed with a reason that
/// names the credential, not its secret.
#[tokio::test]
async fn credential_failures_render_without_the_secret() {
    let live = live_input("client-credential-errors");
    let transport = LearningAwareTransport::new("noted", None);
    let (handle, _dir) = suite_handle("dlg-credential-errors", SUITE_SECRET).await;
    assert!(
        register_assign_complete(&handle, &live, &transport).await,
        "the production registration and dialogue setup must complete"
    );

    // The registered ref is readable, but the pinned value is gone: the
    // scrubber must fail closed and the error must name the class only.
    let empty = MemoryCredentialStore::new();
    let scrubber = CredentialScrubber {
        refs: &handle.store,
        store: &empty,
    };
    let error = scrubber
        .scrub(&format!("the key is {SUITE_SECRET}"))
        .await
        .expect_err("an unreadable registered bearer cannot prove absence");
    expect_absent("scrub error display", &error.to_string(), SUITE_SECRET);
    expect_absent("scrub error debug", &format!("{error:?}"), SUITE_SECRET);
    expect_absent(
        "learning error display",
        &LearningTechnicalError::SecretBoundaryUnavailable {
            reason: error.to_string(),
        }
        .to_string(),
        SUITE_SECRET,
    );
    expect_absent(
        "credential store debug",
        &format!("{:?}", handle.cred_store),
        SUITE_SECRET,
    );
}

/// A credential rotation that lands between the dialogue input scrub and the
/// owner History append must refuse the append (the prepared input may carry
/// the newly registered value); only a fresh submit, re-scrubbed under the new
/// revision, may commit. Both the refusal frames and the durable rows stay
/// free of the value.
#[tokio::test]
async fn rotation_between_dialogue_scrub_and_history_append_refuses_then_rescrubbed_retry_commits()
{
    let live = live_input("client-credential-history-race");
    let transport = LearningAwareTransport::new("noted", None);
    let (handle, dir) = suite_handle("dlg-credential-history-race", SUITE_SECRET).await;
    let setup = register_assign_complete(&handle, &live, &transport).await;
    assert!(
        setup,
        "the production registration and dialogue setup must complete"
    );
    let handle = std::sync::Arc::new(handle);
    let text = format!("the new api key is {ROTATED_SECRET}");

    let gate = handle.arm_submit_accept_gate();
    let submit = {
        let handle = std::sync::Arc::clone(&handle);
        let live = live.clone();
        let text = text.clone();
        tokio::spawn(async move {
            let transport = LearningAwareTransport::new("noted", None);
            handle
                .handle_frame(
                    submit_frame(
                        handle.companion_wire(),
                        Some(0),
                        None,
                        "local-history-race",
                        &text,
                        live.connection_id,
                    ),
                    live,
                    &transport,
                )
                .await
        })
    };
    gate.wait_entered().await;
    rotate_registered(&handle, ROTATED_SECRET);
    handle.disarm_submit_accept_gate();
    gate.release();
    let refused = submit.await.expect("the raced submit must join");

    assert!(
        refused.iter().any(|frame| matches!(
            &frame.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::HeldForTransition)
        )),
        "the stale credential premise must hold the append, got {refused:?}"
    );
    expect_absent_in_frames("refused submit", &refused, ROTATED_SECRET);
    let timeline = handle
        .store
        .load_timeline(running_companion(&handle).await, None, None, 50)
        .await
        .expect("the timeline must load");
    assert!(
        timeline
            .iter()
            .all(|item| !item.text.contains(ROTATED_SECRET)),
        "no stale append may land raw text: {timeline:?}"
    );

    // Retry: the same text is re-scrubbed under the new revision and commits
    // only in its redacted form.
    let retry = handle
        .handle_frame(
            submit_frame(
                handle.companion_wire(),
                Some(current_generation(&handle).await.as_u64()),
                None,
                "local-history-race-retry",
                &text,
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert!(
        retry.iter().any(|frame| matches!(
            &frame.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
        )),
        "the re-scrubbed retry must be accepted, got {retry:?}"
    );
    expect_absent_in_frames("retry submit", &retry, ROTATED_SECRET);
    let owner = handle
        .store
        .load_timeline(running_companion(&handle).await, None, None, 50)
        .await
        .expect("the timeline must load")
        .into_iter()
        .find(|item| item.role == HistoryRole::Owner)
        .expect("the retried Owner row commits");
    assert!(
        !owner.text.contains(ROTATED_SECRET) && owner.text.contains("[credential]"),
        "the retried row carries the redaction marker only: {}",
        owner.text
    );
    for input in transport.inputs() {
        expect_absent("provider capture", &input, ROTATED_SECRET);
    }
    assert!(
        durable_occurrences(&dir.path().join("app.db"), ROTATED_SECRET).is_empty(),
        "the rotated value must never land durable"
    );
}
