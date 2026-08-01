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
    let mut tools = ene_plugin_host::PluginConfig::default();
    tools.enabled = false;
    drop(config.set_section(&tools));
    let ai = ene_ai::AiConfig::default();
    drop(config.set_section(&ai));
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
    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
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
    drop(handle.cancel(&turn1));
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    // After cancel, a new run should be accepted (may immediately Terminal).
    let turn2 = handle.run("third");
    assert!(
        turn2.is_ok() || matches!(turn2, Err(RunError::Busy)),
        "unexpected error: {turn2:?}"
    );
    if let Ok(t) = turn2 {
        drop(handle.cancel(&t));
    }

    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
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
    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
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

    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
}

#[tokio::test]
async fn memory_enabled_without_embedder_fails_closed_on_open() {
    let mut config = EneConfig::default();
    let mut store = ene_store::StoreConfig::default();
    store.enabled = true;
    config.set_section(&store).unwrap();
    let mut tools = ene_plugin_host::PluginConfig::default();
    tools.enabled = false;
    drop(config.set_section(&tools));
    // Cloud embedder with no base URL → init fails → open fails closed.
    let ai = ene_ai::AiConfig::default();
    drop(config.set_section(&ai));

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
    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
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
    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
}

#[tokio::test]
async fn open_accepts_default_mind_compression() {
    // `mind.context` is code-default only (not public settings); compression stays on.
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle with default compression");
    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
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
                drop(handle.cancel(&t));
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
    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
}

#[tokio::test]
async fn tools_search_returns_empty_when_no_tools() {
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");
    let result = handle
        .tools()
        .search_tools("nonexistent".to_string())
        .await
        .expect("search tools succeeds");
    assert!(result.is_empty(), "expected empty list, got {result:?}");
    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
}

#[tokio::test]
async fn tools_list_includes_system_search_tool() {
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");
    let result = handle
        .tools()
        .list_tools()
        .await
        .expect("list tools succeeds");
    assert!(
        result
            .iter()
            .any(|t| t.name.as_str() == "system.search_tools"),
        "expected system.search_tools in tool list"
    );
    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
}

#[tokio::test]
async fn tools_call_intercepts_system_search_tool() {
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");
    let result = handle
        .tools()
        .call_tool(
            "system.search_tools".to_string(),
            serde_json::json!({ "query": "filesystem" }).to_string(),
        )
        .await
        .expect("call system.search_tools succeeds");
    assert!(
        result.contains("No matching tools found."),
        "expected search tool response, got: {result}"
    );
    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
}

/// `EneHandle::set_character` (#406) is a public control method (moved off
/// the diagnostics facade): the card swap must round-trip through the actor
/// and be observable via the next snapshot.
#[tokio::test]
async fn set_character_round_trips_through_public_handle() {
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");

    let mut replacement = test_card();
    replacement.data.name = "ReplacementCard".into();
    replacement.data.system_prompt = "You are the replacement.".into();
    handle
        .set_character(replacement.clone())
        .await
        .expect("set_character succeeds through the public handle");

    let snapshot = handle
        .diagnostics()
        .get_snapshot()
        .await
        .expect("snapshot succeeds");
    let card = snapshot
        .character_card
        .expect("replacement card must be loaded");
    assert_eq!(card.data.name, "ReplacementCard");
    assert_eq!(card.data.system_prompt, "You are the replacement.");
    assert_eq!(snapshot.card_name.as_str(), "ReplacementCard");

    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
}

/// `EneHandle::compress_context` (#406) is a public control method (moved
/// off the diagnostics facade and renamed from `manual_split`). With no
/// conversation history the actor's compression path deterministically
/// returns `SplitNotNeeded` — proving the command round-trips through the
/// actor (a dropped channel or dead actor would surface as
/// `ChannelClosed` / `ActorDead` instead), and that the reply carries the
/// actor-side error, not a facade-local one.
#[tokio::test]
async fn compress_context_routes_through_actor_when_history_empty() {
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");

    let err = handle
        .compress_context()
        .await
        .expect_err("empty history must be rejected by the actor's compression path");
    assert!(
        matches!(err, ene_runtime::EneRuntimeError::Mind(_)),
        "expected the actor's SplitNotNeeded Mind error, got {err:?}"
    );

    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
}

#[test]
fn api_version_constant_is_one() {
    assert_eq!(ene_runtime::API_VERSION, "1");
}

#[tokio::test]
async fn list_sessions_with_store_disabled_returns_public_api_error() {
    // #269: EneHandle::list_sessions returns Result<Vec<PublicSessionMeta>,
    // PublicApiError> — a single Result of API v1 DTO types, not
    // Result<Result<Vec<ene_store::SessionMeta>, ene_store::EneMemoryError>,
    // PublicApiError>. The type annotation below is a compile-time proof of
    // the contract; the disabled-store case is also a realistic error path.
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");
    let result: Result<Vec<ene_runtime::PublicSessionMeta>, ene_runtime::PublicApiError> =
        handle.sessions().list(false, 10).await;
    let err = result.expect_err("memory store is disabled in this config");
    assert!(
        matches!(err, ene_runtime::PublicApiError::Internal { .. }),
        "expected Internal for a disabled store, got {err:?}"
    );
    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
}

#[tokio::test]
async fn archive_session_with_store_disabled_returns_public_api_error() {
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");
    let result: Result<bool, ene_runtime::PublicApiError> = handle
        .sessions()
        .set_archived("missing-session", true)
        .await;
    assert!(result.is_err(), "expected an error with memory disabled");
    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
}

#[test]
fn public_chat_event_mirrors_terminal_done() {
    let turn = TurnId::new();
    let event = EneEvent::Terminal {
        turn: turn.clone(),
        origin: ene_runtime::TurnOrigin::User,
        reason: TerminalReason::Done,
    };
    let public = ene_runtime::PublicChatEvent::from_ene_event(&event);
    let value = serde_json::to_value(&public).expect("serializable");
    assert_eq!(value["type"], "terminal");
    assert_eq!(value["reason"], "done");
    assert_eq!(value["origin"], "user");
    assert_eq!(value["turn"], turn.to_string());
}

// ── #407: mailbox-free lightweight state reads ──

/// The synchronous accessors ([`EneHandle::card_name`],
/// [`EneHandle::session_id`], [`EneHandle::session_started_at`],
/// [`EneHandle::turn_count`], [`EneHandle::config`]) must agree with the
/// mailbox-based snapshot at open: the shared slots are initialized from the
/// same bootstrap config/session the actor owns.
#[tokio::test]
async fn sync_accessors_match_snapshot_at_open() {
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");
    let snapshot = handle
        .diagnostics()
        .get_snapshot()
        .await
        .expect("snapshot succeeds");

    assert_eq!(handle.card_name(), snapshot.card_name.as_str());
    assert_eq!(handle.session_id(), snapshot.session_id);
    assert_eq!(handle.session_started_at(), snapshot.session_started_at);
    assert_eq!(handle.turn_count(), snapshot.current_turn_count);
    assert_eq!(handle.config().user_name, snapshot.config.user_name);
    assert_eq!(handle.config().character, snapshot.config.character);

    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
}

/// `EneHandle::card_name()` / `character_card()` are mailbox-free reads of
/// the shared slots the actor keeps in sync: after `set_character` both must
/// reflect the replacement card immediately (no mailbox round-trip to
/// observe).
#[tokio::test]
async fn set_character_updates_shared_sync_state() {
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");

    assert_eq!(handle.card_name(), "ContractTest");
    let initial = handle.character_card().expect("bootstrap card is loaded");
    assert_eq!(initial.data.name, "ContractTest");

    let mut replacement = test_card();
    replacement.data.name = "ReplacementCard".into();
    replacement.data.system_prompt = "You are the replacement.".into();
    handle
        .set_character(replacement.clone())
        .await
        .expect("set_character succeeds through the public handle");

    assert_eq!(handle.card_name(), "ReplacementCard");
    let card = handle
        .character_card()
        .expect("replacement card must be loaded");
    assert_eq!(card.data.name, "ReplacementCard");
    assert_eq!(card.data.system_prompt, "You are the replacement.");

    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
}

/// `EneHandle::turn_count()` reads an `Arc<AtomicU32>` the actor publishes
/// after `record_user_input` — deterministically observable while the turn
/// is still in flight against a hanging provider (which never lets the
/// stream complete, so the assistant-side increment never happens).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turn_count_reflects_in_flight_turn() {
    let (config, _hanging) = hanging_provider_config().await;
    let handle = EneHandle::open(config, test_card())
        .await
        .expect("open initializes handle");
    assert_eq!(handle.turn_count(), 0);

    let turn = handle.run("hello").expect("run claims the gate");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while handle.turn_count() == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        handle.turn_count(),
        1,
        "record_user_input must publish the incremented turn count"
    );

    drop(handle.cancel(&turn));
    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
}

/// `EneHandle::config()` returns the shared `Arc<EneConfig>` the actor swaps
/// under a write lock on `UpdateFeatureSettings` (#407) — the read-only
/// sharing side of the #325/#327/#331 write-path fixes.
#[tokio::test]
async fn shared_config_reflects_feature_settings_update() {
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");
    let initial_timeout = handle
        .config()
        .get_section::<ene_mind::MindConfig>()
        .expect("mind section present")
        .session
        .session_timeout_minutes;

    let mut mind = ene_mind::MindConfig::default();
    mind.session.session_timeout_minutes = 42;
    let store = ene_store::StoreConfig {
        enabled: false,
        ..Default::default()
    };
    let plugins = ene_plugin_host::PluginConfig {
        enabled: false,
        ..Default::default()
    };
    let rag = ene_rag::ToolRagConfig::default();
    handle
        .update_feature_settings(ene_runtime::FeatureSettingsUpdate {
            mind,
            store,
            plugins,
            rag,
        })
        .expect("update accepted by the mailbox");

    // Fire-and-forget update: poll the shared slot until the actor applies it.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut seen = None;
    while tokio::time::Instant::now() < deadline {
        let timeout = handle
            .config()
            .get_section::<ene_mind::MindConfig>()
            .expect("mind section present")
            .session
            .session_timeout_minutes;
        if timeout != initial_timeout {
            seen = Some(timeout);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(seen, Some(42), "config() must observe the applied update");

    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
}

/// `EneHandle::history()` is the dedicated mailbox-based history read (#407):
/// large payload, so it stays off the mailbox-free shared state, but it must
/// still round-trip through the actor and return the session's entries.
#[tokio::test]
async fn history_returns_actor_history() {
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");
    let history = handle.history().await.expect("history succeeds");
    assert!(
        history.is_empty(),
        "a fresh session has no history before any turn"
    );

    let snapshot = handle
        .diagnostics()
        .get_snapshot()
        .await
        .expect("snapshot succeeds");
    assert_eq!(history.len(), snapshot.history.len());

    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
}

/// Config whose chat provider points at a local listener that completes the
/// TCP handshake (via the kernel backlog) but never writes a response, so a
/// started turn hangs on the per-request timeout instead of completing fast
/// and releasing the single-flight gate on its own. The caller must hold the
/// returned listener for as long as the turn should stay in flight.
async fn hanging_provider_config() -> (EneConfig, tokio::net::TcpListener) {
    let hanging = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local hanging listener");
    let addr = hanging.local_addr().expect("listener has a local address");
    let mut config = test_config_memory_off();
    let mut ai = ene_ai::AiConfig::default();
    if let Some(provider) = ai.providers.get_mut("default") {
        provider.base_url = format!("http://{addr}");
    }
    drop(config.set_section(&ai));
    (config, hanging)
}
