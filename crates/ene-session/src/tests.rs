use crate::{
    Block, CallId, ClientId, CommitResult, DisplayDepth, EventKind, EventPayload,
    InboxCancelReason, InboxClass, InboxSource, InnerAspect, NewEvent, NewSession, NewUsage,
    ProjectOptions, ProjectedMessage, STORAGE_VERSION, SessionCreatedBy, SessionEndReason,
    SessionError, SessionId, SessionKind, SessionStore, SoulId, ToolStatus, Transaction, TurnId,
    TurnOrigin, TurnOutcome, TurnTrigger, derive_messages, hash_projected, open_turns,
    surface_leaks_inner, unclaimed_inbox, v1,
};
use tempfile::TempDir;

async fn open_tmp() -> (TempDir, SessionStore) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("sessions.db");
    let store = SessionStore::open(&path, "NORMAL").await.unwrap();
    (dir, store)
}

async fn mk_session(store: &SessionStore) -> (SoulId, SessionId) {
    let soul = SoulId::new();
    let id = store
        .create_session(NewSession {
            soul_id: soul,
            body_id: None,
            kind: SessionKind::Conversation,
            delegation_id: None,
            created_by: SessionCreatedBy::Client,
        })
        .await
        .unwrap();
    (soul, id)
}

fn text_user(session: SessionId, turn: TurnId, text: &str) -> NewEvent {
    NewEvent::new(
        session,
        EventKind::UserMessage,
        EventPayload::UserMessage {
            v: v1(),
            turn_id: Some(turn),
            blocks: vec![Block::text(text)],
            input_modality: "text".to_owned(),
            client_id: ClientId::new(),
        },
    )
}

#[tokio::test]
async fn model_visible_projection_isolates_unanswered_tool_call() {
    let (_dir, store) = open_tmp().await;
    let (_soul, session) = mk_session(&store).await;
    let turn = TurnId::new();
    let call_id = CallId::new();
    store
        .commit(Transaction {
            entries: vec![
                text_user(session, turn, "search"),
                NewEvent::new(
                    session,
                    EventKind::ToolCall,
                    EventPayload::ToolCall {
                        v: v1(),
                        turn_id: turn,
                        step_index: 0,
                        call_id,
                        tool_name: "web.search".to_owned(),
                        source: "surface".to_owned(),
                        args: serde_json::json!({"query": "large"}),
                    },
                ),
            ],
            usage: vec![],
        })
        .await
        .unwrap();
    let events = store.load_events(session, 0).unwrap();
    let projected = derive_messages(&events, ProjectOptions::model_visible(8));

    assert!(
        projected
            .messages
            .iter()
            .all(|message| message.role != crate::Role::Tool)
    );
}

#[tokio::test]
async fn model_visible_projection_keeps_complete_tool_exchange() {
    let (_dir, store) = open_tmp().await;
    let (_soul, session) = mk_session(&store).await;
    let turn = TurnId::new();
    let call_id = CallId::new();
    store
        .commit(Transaction {
            entries: vec![
                text_user(session, turn, "search"),
                tool_call_event(session, turn, call_id),
                NewEvent::new(
                    session,
                    EventKind::ToolResult,
                    EventPayload::ToolResult {
                        v: v1(),
                        call_id,
                        status: ToolStatus::Ok,
                        blocks: vec![Block::text("result")],
                        spill_ref: None,
                        error_class: None,
                        duration_ms: 1,
                    },
                ),
            ],
            usage: vec![],
        })
        .await
        .unwrap();
    let events = store.load_events(session, 0).unwrap();
    let projected = derive_messages(&events, ProjectOptions::model_visible(8));

    assert_eq!(
        projected
            .messages
            .iter()
            .filter(|message| message.role == crate::Role::Tool)
            .count(),
        2
    );
}

fn tool_call_event(session: SessionId, turn: TurnId, call_id: CallId) -> NewEvent {
    NewEvent::new(
        session,
        EventKind::ToolCall,
        EventPayload::ToolCall {
            v: v1(),
            turn_id: turn,
            step_index: 0,
            call_id,
            tool_name: "web.search".to_owned(),
            source: "surface".to_owned(),
            args: serde_json::json!({"query": "large"}),
        },
    )
}

fn tool_result_event(session: SessionId, call_id: CallId, text: &str) -> NewEvent {
    NewEvent::new(
        session,
        EventKind::ToolResult,
        EventPayload::ToolResult {
            v: v1(),
            call_id,
            status: ToolStatus::Ok,
            blocks: vec![Block::text(text)],
            spill_ref: None,
            error_class: None,
            duration_ms: 1,
        },
    )
}

#[tokio::test]
async fn model_visible_projection_keeps_multiple_calls_as_one_exchange() {
    let (_dir, store) = open_tmp().await;
    let (_soul, session) = mk_session(&store).await;
    let turn = TurnId::new();
    let first = CallId::new();
    let second = CallId::new();
    store
        .commit(Transaction {
            entries: vec![
                text_user(session, turn, "search both"),
                tool_call_event(session, turn, first),
                tool_call_event(session, turn, second),
                tool_result_event(session, first, "first"),
                tool_result_event(session, second, "second"),
            ],
            usage: vec![],
        })
        .await
        .unwrap();

    let events = store.load_events(session, 0).unwrap();
    let projected = derive_messages(&events, ProjectOptions::model_visible(8));
    let tool_messages: Vec<_> = projected
        .messages
        .iter()
        .filter(|message| message.role == crate::Role::Tool)
        .collect();
    assert_eq!(tool_messages.len(), 4);
    assert!(
        tool_messages
            .iter()
            .all(|message| message.tool_call_id.is_some())
    );
}

#[tokio::test]
async fn model_visible_projection_drops_incomplete_multi_call_exchange() {
    let (_dir, store) = open_tmp().await;
    let (_soul, session) = mk_session(&store).await;
    let turn = TurnId::new();
    let first = CallId::new();
    let second = CallId::new();
    let next_turn = TurnId::new();
    store
        .commit(Transaction {
            entries: vec![
                text_user(session, turn, "search both"),
                tool_call_event(session, turn, first),
                tool_call_event(session, turn, second),
                tool_result_event(session, first, "first"),
                text_user(session, next_turn, "continue"),
            ],
            usage: vec![],
        })
        .await
        .unwrap();

    let events = store.load_events(session, 0).unwrap();
    let projected = derive_messages(&events, ProjectOptions::model_visible(8));
    assert!(
        projected
            .messages
            .iter()
            .all(|message| message.role != crate::Role::Tool)
    );
    assert!(
        projected
            .messages
            .iter()
            .any(|message| message.text() == "continue")
    );
}

#[tokio::test]
async fn turn_roundtrip_projects_history() {
    let (_dir, store) = open_tmp().await;
    let (_soul, session) = mk_session(&store).await;
    let turn = TurnId::new();
    store
        .commit(Transaction {
            entries: vec![
                NewEvent::new(
                    session,
                    EventKind::TurnStart,
                    EventPayload::TurnStart {
                        v: v1(),
                        turn_id: turn,
                        lane: "dialogue".to_owned(),
                        origin: TurnOrigin::User,
                        delegation_id: None,
                        trigger: TurnTrigger::Text,
                    },
                ),
                text_user(session, turn, "hello"),
                NewEvent::new(
                    session,
                    EventKind::AssistantMessage,
                    EventPayload::AssistantMessage {
                        v: v1(),
                        turn_id: turn,
                        step_index: 0,
                        blocks: vec![Block::text("hi there")],
                        finish_reason: "stop".to_owned(),
                        token_count: Some(4),
                    },
                ),
                NewEvent::new(
                    session,
                    EventKind::TurnEnd,
                    EventPayload::TurnEnd {
                        v: v1(),
                        turn_id: turn,
                        outcome: TurnOutcome::Completed,
                        error_class: None,
                        error_detail: None,
                    },
                ),
            ],
            usage: vec![],
        })
        .await
        .unwrap();
    let events = store.load_events(session, 0).unwrap();
    let history = derive_messages(&events, ProjectOptions::for_depth(DisplayDepth::Surface, 8));
    let texts: Vec<String> = history
        .messages
        .iter()
        .map(ProjectedMessage::text)
        .collect();
    assert!(texts.iter().any(|t| t == "hello"));
    assert!(texts.iter().any(|t| t == "hi there"));
}

#[tokio::test]
async fn failed_turn_projects_status_not_assistant() {
    let (_dir, store) = open_tmp().await;
    let (_soul, session) = mk_session(&store).await;
    let turn = TurnId::new();
    store
        .commit(Transaction {
            entries: vec![
                text_user(session, turn, "hello"),
                NewEvent::new(
                    session,
                    EventKind::TurnEnd,
                    EventPayload::TurnEnd {
                        v: v1(),
                        turn_id: turn,
                        outcome: TurnOutcome::Failed,
                        error_class: Some("model".to_owned()),
                        error_detail: Some("chat model is not configured".to_owned()),
                    },
                ),
            ],
            usage: vec![],
        })
        .await
        .unwrap();
    let events = store.load_events(session, 0).unwrap();
    let surface = derive_messages(&events, ProjectOptions::for_depth(DisplayDepth::Surface, 8));
    assert!(
        surface
            .messages
            .iter()
            .any(|message| message.role == crate::Role::Status
                && message.text().contains("not configured"))
    );
    assert!(
        surface
            .messages
            .iter()
            .all(|message| message.role != crate::Role::Assistant)
    );
    let for_model = derive_messages(&events, ProjectOptions::model_visible(8));
    assert!(
        for_model
            .messages
            .iter()
            .all(|message| message.role != crate::Role::Status)
    );
}

#[tokio::test]
async fn model_visible_hash_matches_projection() {
    let (_dir, store) = open_tmp().await;
    let (_soul, session) = mk_session(&store).await;
    let turn = TurnId::new();
    store
        .commit(Transaction {
            entries: vec![
                NewEvent::new(
                    session,
                    EventKind::ContextSystemMessage,
                    EventPayload::ContextSystemMessage {
                        v: v1(),
                        blocks: vec![Block::text("you are ene")],
                        source_key: "platform_contract".to_owned(),
                    },
                ),
                text_user(session, turn, "ping"),
                NewEvent::new(
                    session,
                    EventKind::AssistantMessage,
                    EventPayload::AssistantMessage {
                        v: v1(),
                        turn_id: turn,
                        step_index: 0,
                        blocks: vec![Block::text("pong")],
                        finish_reason: "stop".to_owned(),
                        token_count: None,
                    },
                ),
            ],
            usage: vec![],
        })
        .await
        .unwrap();
    let events = store.load_events(session, 0).unwrap();
    let history = derive_messages(&events, ProjectOptions::model_visible(8));
    let from_log = hash_projected(&history).unwrap();
    let replayed = derive_messages(&events, ProjectOptions::model_visible(8));
    assert_eq!(from_log, hash_projected(&replayed).unwrap());
    assert_eq!(history.messages.len(), 3);
}

#[tokio::test]
async fn seq_is_monotonic_without_gaps() {
    let (_dir, store) = open_tmp().await;
    let (_soul, session) = mk_session(&store).await;
    let CommitResult { seqs, .. } = store
        .commit(Transaction {
            entries: vec![
                NewEvent::new(
                    session,
                    EventKind::SessionTitle,
                    EventPayload::SessionTitle {
                        v: v1(),
                        title: "t".to_owned(),
                    },
                ),
                NewEvent::new(
                    session,
                    EventKind::SessionTitle,
                    EventPayload::SessionTitle {
                        v: v1(),
                        title: "t2".to_owned(),
                    },
                ),
            ],
            usage: vec![],
        })
        .await
        .unwrap();
    assert_eq!(seqs, vec![2, 3]);
    let events = store.load_events(session, 0).unwrap();
    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, vec![1, 2, 3]);
}

#[tokio::test]
async fn fork_copies_prefix_and_leaves_source_intact() {
    let (_dir, store) = open_tmp().await;
    let (_soul, session) = mk_session(&store).await;
    store
        .commit(Transaction {
            entries: vec![
                NewEvent::new(
                    session,
                    EventKind::SessionTitle,
                    EventPayload::SessionTitle {
                        v: v1(),
                        title: "one".to_owned(),
                    },
                ),
                NewEvent::new(
                    session,
                    EventKind::SessionTitle,
                    EventPayload::SessionTitle {
                        v: v1(),
                        title: "two".to_owned(),
                    },
                ),
            ],
            usage: vec![],
        })
        .await
        .unwrap();
    let fork = store.fork(session, 2).await.unwrap();
    store
        .commit(Transaction {
            entries: vec![NewEvent::new(
                session,
                EventKind::SessionTitle,
                EventPayload::SessionTitle {
                    v: v1(),
                    title: "three".to_owned(),
                },
            )],
            usage: vec![],
        })
        .await
        .unwrap();
    let source = store.load_events(session, 0).unwrap();
    let forked = store.load_events(fork, 0).unwrap();
    assert!(source.iter().any(|e| matches!(
        e.payload,
        EventPayload::SessionTitle { ref title, .. } if title == "three"
    )));
    assert!(!forked.iter().any(|e| matches!(
        e.payload,
        EventPayload::SessionTitle { ref title, .. } if title == "three"
    )));
    assert!(matches!(forked[0].kind, EventKind::ForkPoint));
    assert!(forked.iter().all(|e| e.seq <= 3));
}

#[tokio::test]
async fn recover_closes_open_turn_and_abandons_inbox() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("sessions.db");
    let soul;
    let session;
    let turn = TurnId::new();
    {
        let store = SessionStore::open(&path, "NORMAL").await.unwrap();
        soul = SoulId::new();
        session = store
            .create_session(NewSession {
                soul_id: soul,
                body_id: None,
                kind: SessionKind::Conversation,
                delegation_id: None,
                created_by: SessionCreatedBy::Client,
            })
            .await
            .unwrap();
        store
            .commit(Transaction {
                entries: vec![
                    NewEvent::new(
                        session,
                        EventKind::TurnStart,
                        EventPayload::TurnStart {
                            v: v1(),
                            turn_id: turn,
                            lane: "dialogue".to_owned(),
                            origin: TurnOrigin::User,
                            delegation_id: None,
                            trigger: TurnTrigger::Text,
                        },
                    ),
                    text_user(session, turn, "half said"),
                    NewEvent::new(
                        session,
                        EventKind::InboxEnqueued,
                        EventPayload::InboxEnqueued {
                            v: v1(),
                            lane: "dialogue".to_owned(),
                            class: InboxClass::Wake,
                            source: InboxSource::User,
                            ref_seq: Some(2),
                        },
                    ),
                ],
                usage: vec![],
            })
            .await
            .unwrap();
        drop(store);
    }
    let store = SessionStore::open(&path, "NORMAL").await.unwrap();
    let events = store.load_events(session, 0).unwrap();
    assert!(!open_turns(&events).is_empty());
    assert!(!unclaimed_inbox(&events).is_empty());
    let reports = store.recover_interrupted().await.unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].interrupted_turns[0].turn_id, turn);
    let events = store.load_events(session, 0).unwrap();
    assert!(open_turns(&events).is_empty());
    assert!(unclaimed_inbox(&events).is_empty());
    assert!(events.iter().any(|e| matches!(
        e.payload,
        EventPayload::TurnEnd {
            outcome: TurnOutcome::Interrupted,
            ..
        }
    )));
    assert!(events.iter().any(|e| matches!(
        e.payload,
        EventPayload::InboxCancelled {
            reason: InboxCancelReason::AbandonedInterrupt,
            ..
        }
    )));
}

#[tokio::test]
async fn surface_projection_hides_inner_and_thinking() {
    let (_dir, store) = open_tmp().await;
    let (_soul, session) = mk_session(&store).await;
    let turn = TurnId::new();
    store
        .commit(Transaction {
            entries: vec![
                NewEvent::new(
                    session,
                    EventKind::AssistantThinking,
                    EventPayload::AssistantThinking {
                        v: v1(),
                        turn_id: turn,
                        step_index: 0,
                        blocks: vec![Block::text("secret thought")],
                        model_id: "stub".to_owned(),
                    },
                ),
                NewEvent::new(
                    session,
                    EventKind::InnerMessage,
                    EventPayload::InnerMessage {
                        v: v1(),
                        turn_id: Some(turn),
                        step_index: Some(0),
                        aspects: vec![InnerAspect::Thought],
                        blocks: vec![Block::text("inner voice")],
                        model_visible: true,
                    },
                ),
                NewEvent::new(
                    session,
                    EventKind::AssistantMessage,
                    EventPayload::AssistantMessage {
                        v: v1(),
                        turn_id: turn,
                        step_index: 0,
                        blocks: vec![Block::text("hello")],
                        finish_reason: "stop".to_owned(),
                        token_count: None,
                    },
                ),
            ],
            usage: vec![],
        })
        .await
        .unwrap();
    let events = store.load_events(session, 0).unwrap();
    let surface = derive_messages(&events, ProjectOptions::for_depth(DisplayDepth::Surface, 8));
    assert!(!surface.messages.iter().any(|m| m.text().contains("secret")));
    assert!(!surface.messages.iter().any(|m| m.text().contains("inner")));
    assert!(!surface_leaks_inner(&surface));
    let detail = derive_messages(&events, ProjectOptions::for_depth(DisplayDepth::Detail, 8));
    assert!(detail.messages.iter().any(|m| m.text().contains("secret")));
    assert!(detail.messages.iter().any(|m| m.text().contains("inner")));
}

#[tokio::test]
async fn usage_ledger_rows_are_append_only() {
    let (_dir, store) = open_tmp().await;
    let (soul, session) = mk_session(&store).await;
    store
        .commit(Transaction {
            entries: vec![NewEvent::new(
                session,
                EventKind::AssistantMessage,
                EventPayload::AssistantMessage {
                    v: v1(),
                    turn_id: TurnId::new(),
                    step_index: 0,
                    blocks: vec![Block::text("ok")],
                    finish_reason: "stop".to_owned(),
                    token_count: Some(2),
                },
            )],
            usage: vec![NewUsage {
                session_id: session,
                soul_id: soul,
                lane: "dialogue".to_owned(),
                task: "chat".to_owned(),
                provider: "stub".to_owned(),
                model: "echo".to_owned(),
                entry_seq: Some(2),
                input_tokens: 3,
                output_tokens: 2,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_micro_usd: None,
                adjustment: false,
            }],
        })
        .await
        .unwrap();
    let totals = store.usage_totals(session).unwrap();
    assert_eq!(totals.input_tokens, 3);
    assert_eq!(totals.output_tokens, 2);
    assert_eq!(totals.rows, 1);
}

#[tokio::test]
async fn compaction_replaces_range_in_projection() {
    let (_dir, store) = open_tmp().await;
    let (_soul, session) = mk_session(&store).await;
    store
        .commit(Transaction {
            entries: vec![
                text_user(session, TurnId::new(), "old"),
                NewEvent::new(
                    session,
                    EventKind::SessionSummary,
                    EventPayload::SessionSummary {
                        v: v1(),
                        scope: "compaction_ref".to_owned(),
                        summary: "summary of old".to_owned(),
                    },
                ),
                NewEvent::new(
                    session,
                    EventKind::CompactionApplied,
                    EventPayload::CompactionApplied {
                        v: v1(),
                        from_seq: 2,
                        to_seq: 3,
                        summary_event_seq: 3,
                    },
                ),
                text_user(session, TurnId::new(), "new"),
            ],
            usage: vec![],
        })
        .await
        .unwrap();
    let events = store.load_events(session, 0).unwrap();
    let history = derive_messages(&events, ProjectOptions::for_depth(DisplayDepth::Surface, 8));
    let texts: Vec<String> = history
        .messages
        .iter()
        .map(ProjectedMessage::text)
        .collect();
    assert!(!texts.iter().any(|t| t == "old"));
    assert!(texts.iter().any(|t| t.contains("summary")));
    assert!(texts.iter().any(|t| t == "new"));
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, EventKind::UserMessage) && e.seq == 2)
    );
}

#[tokio::test]
async fn storage_too_new_is_rejected() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("sessions.db");
    {
        let _store = SessionStore::open(&path, "NORMAL").await.unwrap();
    }
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE meta SET value = '99' WHERE key = 'storage_version'",
            [],
        )
        .unwrap();
    }
    let Err(err) = SessionStore::open(&path, "NORMAL").await else {
        panic!("expected storage too new")
    };
    assert!(matches!(err, SessionError::StorageTooNew { found: 99, .. }));
}

#[tokio::test]
async fn older_storage_migrates_and_interrupts_open_work() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("sessions.db");
    let turn = TurnId::new();
    let (soul, session) = {
        let store = SessionStore::open(&path, "NORMAL").await.unwrap();
        let (soul, session) = mk_session(&store).await;
        store
            .commit(Transaction {
                entries: vec![
                    NewEvent::new(
                        session,
                        EventKind::TurnStart,
                        EventPayload::TurnStart {
                            v: v1(),
                            turn_id: turn,
                            lane: "dialogue".to_owned(),
                            origin: TurnOrigin::User,
                            delegation_id: None,
                            trigger: TurnTrigger::Text,
                        },
                    ),
                    text_user(session, turn, "remember this"),
                ],
                usage: vec![],
            })
            .await
            .unwrap();
        (soul, session)
    };
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE meta SET value = '0' WHERE key = 'storage_version'",
            [],
        )
        .unwrap();
    }
    let store = SessionStore::open(&path, "NORMAL").await.unwrap();
    let version: String = {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.query_row(
            "SELECT value FROM meta WHERE key = 'storage_version'",
            [],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(version, STORAGE_VERSION.to_string());
    let meta = store.get_session(session).unwrap();
    assert_eq!(meta.soul_id, soul);
    let events = store.load_events(session, 0).unwrap();
    assert!(!open_turns(&events).is_empty());
    let reports = store.recover_interrupted().await.unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].interrupted_turns[0].turn_id, turn);
    let events = store.load_events(session, 0).unwrap();
    assert!(open_turns(&events).is_empty());
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::TurnEnd {
            outcome: TurnOutcome::Interrupted,
            ..
        }
    )));
}

#[tokio::test]
async fn session_end_and_surface_search() {
    let (_dir, store) = open_tmp().await;
    let (soul, session) = mk_session(&store).await;
    store
        .commit(Transaction {
            entries: vec![
                NewEvent::new(
                    session,
                    EventKind::SessionTitle,
                    EventPayload::SessionTitle {
                        v: v1(),
                        title: "picnic plans".to_owned(),
                    },
                ),
                text_user(session, TurnId::new(), "bring sandwiches"),
            ],
            usage: vec![],
        })
        .await
        .unwrap();
    let events = store.load_events(session, 0).unwrap();
    let haystack: String = events
        .iter()
        .map(|event| event.payload.surface_search_text())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(haystack.to_ascii_lowercase().contains("picnic"));
    assert!(haystack.to_ascii_lowercase().contains("sandwiches"));
    assert!(!haystack.to_ascii_lowercase().contains("thought"));
    store
        .commit(Transaction {
            entries: vec![NewEvent::new(
                session,
                EventKind::SessionEnd,
                EventPayload::SessionEnd {
                    v: v1(),
                    reason: SessionEndReason::Explicit,
                    summary_ref: None,
                },
            )],
            usage: vec![],
        })
        .await
        .unwrap();
    let meta = store.get_session(session).unwrap();
    assert_eq!(meta.soul_id, soul);
    assert!(meta.ended_at.is_some());
    assert_eq!(meta.end_reason.as_deref(), Some("explicit"));
    assert!(store.last_event_ts(session).unwrap().is_some());
}

#[tokio::test]
async fn put_spill_round_trips_bytes_by_sha256() {
    let (_dir, store) = open_tmp().await;
    let bytes = b"screenshot-png";
    let id = store.put_spill(bytes, Some("image/png")).await.unwrap();
    assert_eq!(id.len(), 64);
    let loaded = store.get_spill(&id).unwrap().unwrap();
    assert_eq!(loaded.bytes, bytes);
    assert_eq!(loaded.mime.as_deref(), Some("image/png"));
    assert!(store.get_spill("not-a-valid-spill-id").is_err());
    assert!(store.get_spill(&"0".repeat(64)).unwrap().is_none());
}

#[tokio::test]
async fn open_applies_full_synchronous_pragma() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("sessions.db");
    let store = SessionStore::open(&path, "FULL").await.unwrap();
    assert_eq!(store.reader_synchronous().unwrap(), "FULL");
    drop(store);
    let store = SessionStore::open(&path, "NORMAL").await.unwrap();
    assert_eq!(store.reader_synchronous().unwrap(), "NORMAL");
}
