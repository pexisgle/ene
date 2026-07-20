//! API v1 contract tests for the ready-handle facade (#111).

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::field_reassign_with_default,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "contract tests use unwrap/expect, fixed indices, and panic on invariant violations"
)]

use ene_config::CharacterCardV3;
use ene_runtime::{CancelError, EneConfig, EneEvent, EneHandle, RunError, TerminalReason, TurnId};

fn test_card() -> CharacterCardV3 {
    let mut card = CharacterCardV3::default();
    card.data.name = "ContractTest".into();
    card.data.system_prompt = "Be brief.".into();
    card
}

fn test_config_memory_off() -> EneConfig {
    let mut config = EneConfig::default();
    let mut store = ene_store::StoreConfig::default();
    store.enabled = false;
    config.set_section(&store).expect("store config merges");
    let mut tools = ene_tool_host::ToolConfig::default();
    tools.enabled = false;
    let _ = config.set_section(&tools);
    let ai = ene_ai::AiConfig::default();
    let _ = config.set_section(&ai);
    config
}

#[tokio::test]
async fn open_returns_ready_handle() {
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");
    let snapshot = handle
        .diagnostics()
        .get_snapshot()
        .await
        .expect("snapshot succeeds");
    assert!(
        snapshot.character_card.is_some(),
        "card must be loaded before open returns"
    );
    assert_eq!(
        snapshot.character_card.as_ref().unwrap().data.name,
        "ContractTest"
    );
    let _ = handle.shutdown(std::time::Duration::from_secs(2)).await;
}

#[tokio::test(flavor = "current_thread")]
async fn second_run_returns_busy() {
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");

    let turn1 = handle.run("hello").expect("first run completes");
    let busy = handle.run("again");
    assert!(
        matches!(busy, Err(RunError::Busy)),
        "expected Busy while turn {turn1} active, got {busy:?}"
    );

    // Free the gate.
    let _ = handle.cancel(&turn1);
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    // After cancel, a new run should be accepted (may immediately Terminal).
    let turn2 = handle.run("third");
    assert!(
        turn2.is_ok() || matches!(turn2, Err(RunError::Busy)),
        "unexpected error: {turn2:?}"
    );
    if let Ok(t) = turn2 {
        let _ = handle.cancel(&t);
    }

    let _ = handle.shutdown(std::time::Duration::from_secs(2)).await;
}

#[tokio::test]
async fn cancel_wrong_turn_returns_mismatch() {
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");
    let wrong = TurnId::new();
    let err = handle.cancel(&wrong);
    assert!(
        matches!(err, Err(CancelError::TurnMismatch)),
        "expected TurnMismatch, got {err:?}"
    );
    let _ = handle.shutdown(std::time::Duration::from_secs(2)).await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_emits_terminal_exactly_once_with_matching_turn() {
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");
    let mut rx = handle.subscribe();
    let turn = handle.run("cancel me").expect("run completes");
    handle.cancel(&turn).expect("cancel targets running turn");

    // Yield so the actor processes Run + Cancel (and any stream race).
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }

    let mut terminals = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let EneEvent::Terminal {
            turn: ref t,
            origin: _,
            reason,
        } = ev
        {
            assert_eq!(t, &turn);
            terminals.push(reason);
        }
    }
    assert_eq!(
        terminals.len(),
        1,
        "expected exactly one Terminal, got {terminals:?}"
    );

    let err = handle.cancel(&turn);
    assert!(matches!(err, Err(CancelError::TurnMismatch)));

    let _ = handle.shutdown(std::time::Duration::from_secs(2)).await;
}

#[tokio::test]
async fn memory_enabled_without_embedder_fails_closed_on_open() {
    let mut config = EneConfig::default();
    let mut store = ene_store::StoreConfig::default();
    store.enabled = true;
    config.set_section(&store).unwrap();
    let mut tools = ene_tool_host::ToolConfig::default();
    tools.enabled = false;
    let _ = config.set_section(&tools);
    // Cloud embedder with no base URL → init fails → open fails closed.
    let ai = ene_ai::AiConfig::default();
    let _ = config.set_section(&ai);

    let err = EneHandle::open(config, test_card()).await;
    assert!(err.is_err(), "expected open to fail closed, got {err:?}");
}

#[tokio::test]
async fn marker_tokens_become_performance_not_text() {
    // Unit-level: special token splitter used by stream path.
    let mut carry = String::new();
    let (text, tokens) =
        ene_mind::split_text_and_special_tokens(&mut carry, "Hi <|perf:expr=happy|> there");
    assert_eq!(text.join(""), "Hi  there");
    assert_eq!(tokens.len(), 1);
    let cue = ene_mind::parse_performance_marker(&tokens[0]).expect("perf marker");
    assert_eq!(cue.name, "happy");
    assert_eq!(cue.kind, ene_mind::PerfKind::Expression);
}

#[tokio::test(flavor = "current_thread")]
async fn store_off_run_emits_terminal() {
    // Without a live LLM the turn fails, but it must still Terminal with
    // Done or Failed (chat is allowed with store.enabled=false). Cancel must
    // not be used as a success path for this contract.
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");
    let mut rx = handle.subscribe();
    let turn = handle.run("hello with memory off").expect("run completes");

    let mut saw_terminal = false;
    for _ in 0..200 {
        while let Ok(ev) = rx.try_recv() {
            match ev {
                EneEvent::Terminal {
                    turn: ref t,
                    origin: _,
                    reason: TerminalReason::Done | TerminalReason::Failed { .. },
                } if t == &turn => {
                    saw_terminal = true;
                }
                EneEvent::Terminal {
                    turn: ref t,
                    origin: _,
                    reason: TerminalReason::Cancelled,
                } if t == &turn => {
                    panic!("store-off contract must not rely on Cancel; got Cancelled for {t}");
                }
                _ => {}
            }
        }
        if saw_terminal {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert!(
        saw_terminal,
        "store.enabled=false must complete a turn with Terminal Done or Failed"
    );
    let _ = handle.shutdown(std::time::Duration::from_secs(2)).await;
}

#[tokio::test]
async fn snapshot_history_is_history_entry() {
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");
    let snapshot = handle
        .diagnostics()
        .get_snapshot()
        .await
        .expect("snapshot succeeds");
    // Type-level: Vec<HistoryEntry> — compile-time check via annotation.
    let _: &Vec<ene_mind::HistoryEntry> = &snapshot.history;
    assert!(snapshot.history.is_empty());
    let _ = handle.shutdown(std::time::Duration::from_secs(2)).await;
}

#[tokio::test]
async fn open_accepts_default_mind_compression() {
    // `mind.context` is code-default only (not public settings); compression stays on.
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle with default compression");
    let _ = handle.shutdown(std::time::Duration::from_secs(2)).await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_frees_gate_for_next_run() {
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");
    let turn1 = handle.run("first").expect("run completes");
    handle.cancel(&turn1).expect("cancel targets running turn");
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    // Gate must be free; Busy is only acceptable briefly while cancel drains.
    let mut accepted = false;
    for _ in 0..20 {
        match handle.run("second") {
            Ok(t) => {
                accepted = true;
                let _ = handle.cancel(&t);
                break;
            }
            Err(RunError::Busy) => {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            Err(e) => panic!("unexpected run error: {e}"),
        }
    }
    assert!(
        accepted,
        "cancel must free the turn gate for a subsequent run"
    );
    let _ = handle.shutdown(std::time::Duration::from_secs(2)).await;
}

#[tokio::test]
async fn diagnostics_search_tools_returns_empty_when_no_tools() {
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");
    let result = handle
        .diagnostics()
        .search_tools("nonexistent".to_string())
        .await
        .expect("search tools succeeds");
    assert!(result.is_empty(), "expected empty list, got {result:?}");
    let _ = handle.shutdown(std::time::Duration::from_secs(2)).await;
}
