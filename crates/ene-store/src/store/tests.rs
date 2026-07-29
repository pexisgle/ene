//! Integration tests for [`MemoryStore`].

use super::memory::strip_tags_footer;
use chrono::{DateTime, Utc};

use super::*;

async fn setup_store() -> MemoryStore {
    MemoryStore::open_in_memory(4).await.unwrap()
}

#[test]
fn embedding_bytes_roundtrip() {
    let original = vec![1.0_f32, 0.5, -0.25, 0.0];
    let bytes = embedding_to_bytes(&original);
    let restored = bytes_to_embedding(&bytes);
    for (a, b) in original.iter().zip(restored.iter()) {
        assert!((a - b).abs() < 1e-7, "Mismatch: {a} != {b}");
    }
}

fn new_session_meta(session_id: &str, card_name: &str) -> crate::session::NewSessionMeta {
    crate::session::NewSessionMeta {
        session_id: session_id.to_string(),
        card_name: card_name.to_string(),
        title: format!("{session_id} title"),
    }
}

#[tokio::test]
async fn session_upsert_get_list_archive() {
    let store = setup_store().await;

    let id_a = store
        .upsert_session(&new_session_meta("sess-a", "card"))
        .await
        .unwrap();
    let id_b = store
        .upsert_session(&new_session_meta("sess-b", "card"))
        .await
        .unwrap();
    assert_ne!(id_a, id_b);

    // Upserting an existing session refreshes updated_at but keeps the row.
    let id_a2 = store
        .upsert_session(&new_session_meta("sess-a", "card"))
        .await
        .unwrap();
    assert_eq!(id_a, id_a2);

    // Touch bumps turn_count.
    store.touch_session("sess-a", 5).await.unwrap();
    let meta = store.get_session("sess-a").await.unwrap().unwrap();
    assert_eq!(meta.turn_count, 5);
    assert!(!meta.archived);

    // Listing (newest first) excludes archived by default.
    store.set_session_archived("sess-b", true).await.unwrap();
    let active = store.list_sessions(false, 10).await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active.first().unwrap().session_id, "sess-a");

    let all = store.list_sessions(true, 10).await.unwrap();
    assert_eq!(all.len(), 2);

    // Archiving a missing session reports no update.
    let updated = store.set_session_archived("nope", true).await.unwrap();
    assert!(!updated);
}

#[tokio::test]
async fn message_search_is_case_insensitive_and_paginated() {
    let store = setup_store().await;
    store
        .upsert_session(&new_session_meta("s1", "card"))
        .await
        .unwrap();
    store
        .upsert_session(&new_session_meta("s2", "card"))
        .await
        .unwrap();

    store
        .insert_log("s1", "card", "user", "Hello World")
        .await
        .unwrap();
    store
        .insert_log("s2", "card", "assistant", "hello there")
        .await
        .unwrap();
    store
        .insert_log("s1", "card", "user", "goodbye")
        .await
        .unwrap();

    // Case-insensitive match across sessions.
    let hits = store.search_messages("HELLO", 10, 0).await.unwrap();
    assert_eq!(hits.len(), 2);
    let sessions: std::collections::HashSet<&str> =
        hits.iter().map(|(sid, _)| sid.as_str()).collect();
    assert!(sessions.contains("s1"));
    assert!(sessions.contains("s2"));

    // Pagination.
    let page = store.search_messages("hello", 1, 0).await.unwrap();
    assert_eq!(page.len(), 1);
    let page2 = store.search_messages("hello", 1, 1).await.unwrap();
    assert_eq!(page2.len(), 1);

    // Empty query returns nothing.
    let empty = store.search_messages("", 10, 0).await.unwrap();
    assert!(empty.is_empty());
}

#[tokio::test]
async fn export_import_roundtrip_and_conflict() {
    let store = setup_store().await;
    store
        .upsert_session(&new_session_meta("orig", "card"))
        .await
        .unwrap();
    store
        .insert_log("orig", "card", "user", "my key is sk-secret123 bye")
        .await
        .unwrap();
    store
        .insert_log("orig", "card", "assistant", "noted")
        .await
        .unwrap();

    let export = store.build_export("orig").await.unwrap();
    assert_eq!(export.session.session_id, "orig");
    assert_eq!(export.messages.len(), 2);
    // Secrets in message content are redacted (order-independent).
    let joined: String = export.messages.iter().map(|m| m.content.as_str()).collect();
    assert!(joined.contains("[redacted]"));
    assert!(!joined.contains("sk-secret123"));

    // JSON round-trip.
    let json = export.to_json().unwrap();
    let parsed = crate::export::SessionExport::from_json(&json).unwrap();
    assert_eq!(parsed.session.session_id, "orig");

    // Importing under an existing session_id allocates a new one.
    let new_id = store.import_export(&parsed).await.unwrap();
    let sessions = store.list_sessions(true, 10).await.unwrap();
    assert_eq!(sessions.len(), 2);
    let imported = sessions.iter().find(|s| s.id == new_id).unwrap();
    assert!(imported.session_id.starts_with("imported-"));
    let imported_msgs = store.list_messages(&imported.session_id).await.unwrap();
    assert_eq!(imported_msgs.len(), 2);

    // Importing a brand-new session_id preserves it.
    let mut fresh = parsed.clone();
    fresh.session.session_id = "brand-new".to_string();
    store.import_export(&fresh).await.unwrap();
    assert!(store.get_session("brand-new").await.unwrap().is_some());
}

#[tokio::test]
async fn tool_embedding_field_upsert_overwrites_and_list_filters() {
    let store = setup_store().await;
    let emb = vec![1.0_f32, 0.0, 0.0, 0.0];

    store
        .upsert_tool_embedding_field(
            "web_search",
            "description",
            "",
            "hash-a",
            "",
            &emb,
            "desc text",
        )
        .await
        .unwrap();
    store
        .upsert_tool_embedding_field("web_search", "summary", "", "hash-a", "", &emb, "sum text")
        .await
        .unwrap();
    store
        .upsert_tool_embedding_field("web_search", "negative", "", "hash-a", "", &emb, "neg text")
        .await
        .unwrap();
    store
        .upsert_tool_embedding_field(
            "other_tool",
            "description",
            "",
            "hash-b",
            "",
            &emb,
            "other desc",
        )
        .await
        .unwrap();

    let rows = store.list_tool_embedding_fields().await.unwrap();
    assert_eq!(rows.len(), 4);
    let web_rows: Vec<_> = rows
        .iter()
        .filter(|r| r.tool_name == "web_search")
        .collect();
    assert_eq!(web_rows.len(), 3);
    let fields: std::collections::HashSet<&str> =
        web_rows.iter().map(|r| r.field.as_str()).collect();
    assert!(fields.contains("summary"));
    assert!(fields.contains("description"));
    assert!(fields.contains("negative"));

    // Upsert (replace) on the same (tool_name, field, field_key, model_name) overwrites.
    let emb2 = vec![0.0_f32, 1.0, 0.0, 0.0];
    store
        .upsert_tool_embedding_field(
            "web_search",
            "summary",
            "",
            "hash-a2",
            "",
            &emb2,
            "sum text v2",
        )
        .await
        .unwrap();
    let rows = store.list_tool_embedding_fields().await.unwrap();
    let web_summary = rows
        .iter()
        .find(|r| r.tool_name == "web_search" && r.field == "summary")
        .unwrap();
    assert_eq!(web_summary.version_hash, "hash-a2");
    assert_eq!(web_summary.embedding, emb2);
    assert_eq!(web_summary.source_text, "sum text v2");
}

#[tokio::test]
async fn delete_tool_embeddings_cascades_to_fields() {
    let store = setup_store().await;
    let emb = vec![1.0_f32, 0.0, 0.0, 0.0];

    for field in ["summary", "description", "negative"] {
        store
            .upsert_tool_embedding_field("web_search", field, "", "hash", "", &emb, "text")
            .await
            .unwrap();
    }
    store
        .upsert_tool_embedding_field("keep_me", "description", "", "hash", "", &emb, "keep text")
        .await
        .unwrap();

    assert_eq!(store.list_tool_embedding_fields().await.unwrap().len(), 4);
    let deleted = store.delete_tool_embeddings("web_search").await.unwrap();
    assert_eq!(deleted, 3);
    assert_eq!(store.list_tool_embedding_fields().await.unwrap().len(), 1);
}

/// Regression test for #41 (bug 4): embedding insert
/// must reject vectors whose length does not match
/// `embedding_dim` and vectors containing NaN /
/// Infinity, returning a typed `InvalidEmbedding`
/// error rather than letting the row be silently
/// persisted and poisoning later cosine queries.
#[tokio::test]
async fn upsert_memory_embedding_rejects_bad_embedding() {
    let store = setup_store().await;
    let id = store
        .insert_typed_memory(&crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Semantic,
            title: "fact".into(),
            content: "content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::default(),
            salience: crate::MemorySalience::default(),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        })
        .await
        .unwrap();

    let wrong_len = vec![0.1, 0.2, 0.3];
    let err = store
        .upsert_memory_embedding(id, "test-model", "content", &wrong_len)
        .await
        .unwrap_err();
    assert!(
        matches!(err, EneMemoryError::InvalidEmbedding(_)),
        "expected InvalidEmbedding, got {err:?}"
    );

    let with_nan = vec![0.1, f32::NAN, 0.3, 0.4];
    let err = store
        .upsert_memory_embedding(id, "test-model", "content", &with_nan)
        .await
        .unwrap_err();
    assert!(
        matches!(err, EneMemoryError::InvalidEmbedding(_)),
        "expected InvalidEmbedding, got {err:?}"
    );

    let with_inf = vec![0.1, 0.2, f32::INFINITY, 0.4];
    let err = store
        .upsert_memory_embedding(id, "test-model", "content", &with_inf)
        .await
        .unwrap_err();
    assert!(
        matches!(err, EneMemoryError::InvalidEmbedding(_)),
        "expected InvalidEmbedding, got {err:?}"
    );

    let ok = vec![0.1, 0.2, 0.3, 0.4];
    store
        .upsert_memory_embedding(id, "test-model", "content", &ok)
        .await
        .unwrap();
}

/// Regression test for #41 (bug 1): the memory store
/// must apply `foreign_keys=ON` (and the other
/// safety PRAGMAs) on every connection it opens. For
/// an in-memory store `journal_mode=WAL` is a no-op
/// (`SQLite` returns `memory`), but `foreign_keys` and
/// `busy_timeout` are still meaningful.
#[tokio::test]
async fn pragmas_are_applied_on_open() {
    use sea_orm::ConnectionTrait;
    let store = setup_store().await;
    // `execute_unprepared` returns a `ExecResult`
    // whose `rows_affected` field is populated for
    // some statements; for `PRAGMA foreign_keys` it
    // returns the current value of the pragma (0 or
    // 1) as `rows_affected`. This is a pragmatic
    // way to assert the PRAGMA took effect without
    // pulling in a full query API.
    let res = store
        .connection()
        .execute_unprepared("PRAGMA foreign_keys")
        .await
        .unwrap();
    assert_eq!(
        res.rows_affected(),
        1,
        "foreign_keys PRAGMA should report 1 (ON)"
    );
}

#[tokio::test]
async fn insert_conversation_turn_stores_and_retrieves() {
    let store = setup_store().await;
    let session_id = "turn-test-session";

    let ids = store
        .insert_conversation_turn(session_id, "ene", "Hello", "Hi there!")
        .await
        .unwrap();
    let logs = store.get_logs_by_session(session_id).await.unwrap();
    assert_eq!(logs.len(), 2);
    let user_log = logs.first().expect("user log");
    let assistant_log = logs.get(1).expect("assistant log");
    assert_eq!(user_log.role, "user");
    assert_eq!(user_log.content, "Hello");
    assert_eq!(assistant_log.role, "assistant");
    assert_eq!(assistant_log.content, "Hi there!");
    let _ = ids;
}

#[tokio::test]
async fn affect_state_get_upsert() {
    let store = setup_store().await;

    let result = store.get_affect_state("ene").await.unwrap();
    assert!((result.valence - 0.0).abs() < f32::EPSILON);
    assert!((result.arousal - 0.0).abs() < f32::EPSILON);
    assert!(result.discrete_emotions.is_empty());

    let mut state = crate::AffectState {
        character_id: "ene".into(),
        user_id: String::new(),
        valence: 0.5,
        arousal: -0.3,
        dominance: 0.1,
        trust: 0.4,
        affinity: 0.6,
        irritation: 0.0,
        curiosity: 0.7,
        fatigue: 0.1,
        mood_label: String::new(),
        last_expression: String::new(),
        discrete_emotions: vec![
            crate::DiscreteEmotion::new("joy", 0.8),
            crate::DiscreteEmotion::new("surprise", 0.4),
        ],
        updated_at: None,
    };
    store.upsert_affect_state(&state).await.unwrap();

    let loaded = store.get_affect_state("ene").await.unwrap();
    assert!((loaded.valence - 0.5).abs() < f32::EPSILON);
    assert!((loaded.arousal + 0.3).abs() < f32::EPSILON);
    assert_eq!(loaded.discrete_emotions.len(), 2);
    let joy = loaded.discrete_emotions.first().expect("joy emotion");
    assert_eq!(joy.label, "joy");
    assert!((joy.intensity - 0.8).abs() < f32::EPSILON);

    state.valence = -0.2;
    state.discrete_emotions = vec![crate::DiscreteEmotion::new("sadness", 0.6)];
    store.upsert_affect_state(&state).await.unwrap();

    let loaded2 = store.get_affect_state("ene").await.unwrap();
    assert!((loaded2.valence + 0.2).abs() < f32::EPSILON);
    assert_eq!(loaded2.discrete_emotions.len(), 1);
    let sadness = loaded2.discrete_emotions.first().expect("sadness emotion");
    assert_eq!(sadness.label, "sadness");
}

#[tokio::test]
async fn typed_memory_crud() {
    let store = setup_store().await;

    let item = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Episodic,
        title: "Greeting".into(),
        content: "The user greeted me this morning".into(),
        source: crate::MemorySource::Conversation,
        source_ref: Some("sess-1/turn-1".into()),
        confidence: crate::MemoryConfidence::new(0.9),
        salience: crate::MemorySalience::new(0.5),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };

    let id = store.insert_typed_memory(&item).await.unwrap();
    assert!(id > 0);

    let loaded = store.get_typed_memory(id).await.unwrap().unwrap();
    assert_eq!(loaded.title, "Greeting");
    assert_eq!(loaded.kind, crate::MemoryKind::Episodic);
    assert!((loaded.confidence.get() - 0.9).abs() < f32::EPSILON);

    let by_char = store
        .get_typed_memories_by_character("ene", None, 10, 0)
        .await
        .unwrap();
    assert!(!by_char.is_empty());

    let count = store
        .count_typed_memories("ene", Some(crate::MemoryKind::Episodic))
        .await
        .unwrap();
    assert!(count > 0);

    let status_ok = store
        .set_memory_status(id, crate::MemoryStatus::Faded)
        .await
        .unwrap();
    assert!(status_ok);

    let access_ok = store.bump_typed_memory_access(id).await.unwrap();
    assert!(access_ok);

    let loaded2 = store.get_typed_memory(id).await.unwrap().unwrap();
    assert_eq!(loaded2.status, crate::MemoryStatus::Faded);
    assert_eq!(loaded2.access_count, 1);

    assert!(store.get_typed_memory(999_999).await.unwrap().is_none());
    assert!(
        !store
            .set_memory_status(999_999, crate::MemoryStatus::Active)
            .await
            .unwrap()
    );
    assert!(!store.bump_typed_memory_access(999_999).await.unwrap());
}

#[tokio::test]
async fn typed_memory_search_with_embedding() {
    let store = setup_store().await;

    let item = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "Test memory".into(),
        content: "The user likes pizza".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::new(0.8),
        salience: crate::MemorySalience::new(0.6),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };

    let id = store.insert_typed_memory(&item).await.unwrap();
    let emb = vec![0.1, 0.2, 0.3, 0.4];
    store
        .upsert_memory_embedding(id, "test-model", "content", &emb)
        .await
        .unwrap();

    let results = store
        .search_typed_memories(&emb, "ene", "test-model", 10, 0.0)
        .await
        .unwrap();
    assert!(!results.is_empty());
    let top = results.first().expect("typed memory search result");
    assert!((top.1 - 1.0).abs() < f32::EPSILON);
    assert_eq!(top.0.title, "Test memory");
}

#[tokio::test]
async fn supersede_typed_memory_links_rows() {
    let store = setup_store().await;

    let old_item = crate::NewMemoryItem {
        scope: crate::MemoryScope::User,
        character_id: "ene".into(),
        user_id: "user1".into(),
        kind: crate::MemoryKind::Preference,
        title: "drink".into(),
        content: "likes coffee".into(),
        source: crate::MemorySource::Inferred,
        source_ref: None,
        confidence: crate::MemoryConfidence::new(0.7),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };
    let old_id = store.insert_typed_memory(&old_item).await.unwrap();

    let new_item = crate::NewMemoryItem {
        content: "likes tea".into(),
        confidence: crate::MemoryConfidence::new(0.9),
        ..old_item
    };
    let new_id = store
        .supersede_typed_memory(&new_item, old_id)
        .await
        .unwrap();

    let old = store.get_typed_memory(old_id).await.unwrap().unwrap();
    assert_eq!(old.status, crate::MemoryStatus::Superseded);
    assert_eq!(old.supersedes_id, None);

    let new_mem = store.get_typed_memory(new_id).await.unwrap().unwrap();
    assert_eq!(new_mem.supersedes_id, Some(old_id));
    assert_eq!(new_mem.status, crate::MemoryStatus::Active);
}

#[tokio::test]
async fn supersede_typed_memory_rejects_terminal_status() {
    let store = setup_store().await;

    let item = crate::NewMemoryItem {
        scope: crate::MemoryScope::User,
        character_id: "ene".into(),
        user_id: "user1".into(),
        kind: crate::MemoryKind::Preference,
        title: "drink".into(),
        content: "likes coffee".into(),
        source: crate::MemorySource::Inferred,
        source_ref: None,
        confidence: crate::MemoryConfidence::new(0.7),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };
    let old_id = store.insert_typed_memory(&item).await.unwrap();
    store
        .set_memory_status(old_id, crate::MemoryStatus::UserDeleted)
        .await
        .unwrap();

    let replacement = crate::NewMemoryItem {
        content: "likes tea".into(),
        ..item
    };
    let err = store
        .supersede_typed_memory(&replacement, old_id)
        .await
        .unwrap_err();
    assert!(
        matches!(err, EneMemoryError::Other(_)),
        "expected Other error, got {err:?}"
    );
}

#[tokio::test]
async fn commitment_crud_and_lifecycle() {
    let store = setup_store().await;

    let id = store
        .insert_commitment(&crate::NewCommitment {
            character_id: "ene".into(),
            user_id: "user1".into(),
            title: "design review".into(),
            description: "Discuss the design next session".into(),
            status: crate::CommitmentStatus::Active,
            due_at: None,
            due_label: Some("next session".into()),
        })
        .await
        .unwrap();

    let loaded = store.get_commitment(id).await.unwrap().unwrap();
    assert_eq!(loaded.title, "design review");

    let active = store
        .list_active_commitments("ene", Some("user1"), 10)
        .await
        .unwrap();
    assert_eq!(active.len(), 1);

    assert!(store.complete_commitment(id).await.unwrap());
    let done = store.get_commitment(id).await.unwrap().unwrap();
    assert_eq!(done.status, crate::CommitmentStatus::Done);
    assert!(done.completed_at.is_some());

    let active_after = store
        .list_active_commitments("ene", None, 10)
        .await
        .unwrap();
    assert!(active_after.is_empty());
}

#[tokio::test]
async fn mark_stale_commitments_past_due() {
    let store = setup_store().await;
    let past = Utc::now() - chrono::Duration::days(1);
    let future = Utc::now() + chrono::Duration::days(1);

    let stale_target = store
        .insert_commitment(&crate::NewCommitment {
            character_id: "ene".into(),
            user_id: String::new(),
            title: "overdue".into(),
            description: "was due yesterday".into(),
            status: crate::CommitmentStatus::Active,
            due_at: Some(past),
            due_label: None,
        })
        .await
        .unwrap();
    let _still_active = store
        .insert_commitment(&crate::NewCommitment {
            character_id: "ene".into(),
            user_id: String::new(),
            title: "upcoming".into(),
            description: "due tomorrow".into(),
            status: crate::CommitmentStatus::Active,
            due_at: Some(future),
            due_label: None,
        })
        .await
        .unwrap();

    let updated = store.mark_stale_commitments(Utc::now()).await.unwrap();
    assert_eq!(updated, 1);

    let stale = store.get_commitment(stale_target).await.unwrap().unwrap();
    assert_eq!(stale.status, crate::CommitmentStatus::Stale);
}

#[tokio::test]
async fn list_active_commitments_orders_dated_before_undated() {
    let store = setup_store().await;
    let now = Utc::now();

    let later_id = store
        .insert_commitment(&crate::NewCommitment {
            character_id: "ene".into(),
            user_id: "user1".into(),
            title: "later".into(),
            description: "due in two days".into(),
            status: crate::CommitmentStatus::Active,
            due_at: Some(now + chrono::Duration::days(2)),
            due_label: None,
        })
        .await
        .unwrap();
    let sooner_id = store
        .insert_commitment(&crate::NewCommitment {
            character_id: "ene".into(),
            user_id: "user1".into(),
            title: "sooner".into(),
            description: "due tomorrow".into(),
            status: crate::CommitmentStatus::Active,
            due_at: Some(now + chrono::Duration::days(1)),
            due_label: None,
        })
        .await
        .unwrap();
    let undated_id = store
        .insert_commitment(&crate::NewCommitment {
            character_id: "ene".into(),
            user_id: "user1".into(),
            title: "undated".into(),
            description: "no due date".into(),
            status: crate::CommitmentStatus::Active,
            due_at: None,
            due_label: Some("next time".into()),
        })
        .await
        .unwrap();

    let active = store
        .list_active_commitments("ene", Some("user1"), 10)
        .await
        .unwrap();
    let ids: Vec<i64> = active.iter().map(|c| c.id.unwrap()).collect();
    assert_eq!(ids, vec![sooner_id, later_id, undated_id]);
}

#[tokio::test]
async fn terminal_commitment_status_is_not_overwritten() {
    let store = setup_store().await;
    let past = Utc::now() - chrono::Duration::days(1);

    let done_id = store
        .insert_commitment(&crate::NewCommitment {
            character_id: "ene".into(),
            user_id: "user1".into(),
            title: "completed".into(),
            description: "already done".into(),
            status: crate::CommitmentStatus::Active,
            due_at: Some(past),
            due_label: None,
        })
        .await
        .unwrap();
    assert!(store.complete_commitment(done_id).await.unwrap());

    let updated = store.mark_stale_commitments(Utc::now()).await.unwrap();
    assert_eq!(updated, 0);

    let done = store.get_commitment(done_id).await.unwrap().unwrap();
    assert_eq!(done.status, crate::CommitmentStatus::Done);
    assert!(done.completed_at.is_some());

    assert!(!store.cancel_commitment(done_id).await.unwrap());
    assert!(
        !store
            .update_commitment_status(done_id, crate::CommitmentStatus::Stale)
            .await
            .unwrap()
    );
}

fn hybrid_search_options<'a>(
    query_text: &'a str,
    query_embedding: &'a [f32],
    now: DateTime<Utc>,
) -> crate::Query<'a> {
    crate::Query {
        query_text,
        embedding: Some(query_embedding),
        character_id: "ene",
        user_id: None,
        model_name: "test-model",
        limit: 10,
        similarity_threshold: 0.0,
        candidate_pool_size: 50,
        query_affect: None,
        weights: crate::HybridSearchWeights::default(),
        decay_half_life_days: 30.0,
        now,
        time_range: None,
        min_score: 0.0,
        commitment_boost: 0.25,
        recent_fallback_limit: 5,
    }
}

async fn insert_memory_with_embedding(
    store: &MemoryStore,
    item: &crate::NewMemoryItem,
    embedding: &[f32],
) -> i64 {
    let id = store.insert_typed_memory(item).await.unwrap();
    store
        .upsert_memory_embedding(id, "test-model", "content", embedding)
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn hybrid_search_ranks_by_salience_and_recency_not_vector_alone() {
    let store = setup_store().await;
    let now = Utc::now();
    let query_emb = vec![1.0, 0.0, 0.0, 0.0];

    let low_salience = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "distant topic".into(),
        content: "unrelated content".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::new(0.5),
        salience: crate::MemorySalience::new(0.2),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };
    let high_salience = crate::NewMemoryItem {
        salience: crate::MemorySalience::new(0.95),
        confidence: crate::MemoryConfidence::new(0.9),
        title: "important fact".into(),
        content: "user preference about music".into(),
        ..low_salience.clone()
    };

    insert_memory_with_embedding(&store, &low_salience, &query_emb).await;
    insert_memory_with_embedding(&store, &high_salience, &query_emb).await;

    let options = hybrid_search_options("music preference", &query_emb, now);
    let results = store.search(&options).await.unwrap();
    assert_eq!(results.len(), 2);
    let top = results.first().expect("top hybrid result");
    let second = results.get(1).expect("second hybrid result");
    assert_eq!(top.item.title, "important fact");
    assert!(top.breakdown.total >= second.breakdown.total);
}

#[tokio::test]
async fn hybrid_search_lexical_component_for_matching_query() {
    let store = setup_store().await;
    let now = Utc::now();
    let orthogonal = vec![0.0, 1.0, 0.0, 0.0];

    let item = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "favorite drink".into(),
        content: "The user loves matcha latte".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::default(),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };
    insert_memory_with_embedding(&store, &item, &orthogonal).await;

    let options = hybrid_search_options("matcha latte", &orthogonal, now);
    let results = store.search(&options).await.unwrap();
    assert_eq!(results.len(), 1);
    let result = results.first().expect("lexical match result");
    assert!(result.breakdown.lexical_score > 0.0);
    assert!(
        result
            .sources
            .contains(&crate::MemoryCandidateSource::Lexical)
    );
}

#[tokio::test]
async fn hybrid_search_surfaces_active_commitment_with_low_vector_similarity() {
    let store = setup_store().await;
    let now = Utc::now();
    let query_emb = vec![1.0, 0.0, 0.0, 0.0];

    let commitment_id = store
        .insert_commitment(&crate::NewCommitment {
            character_id: "ene".into(),
            user_id: "user1".into(),
            title: "follow up".into(),
            description: "Review the architecture document".into(),
            status: crate::CommitmentStatus::Active,
            due_at: None,
            due_label: Some("next time".into()),
        })
        .await
        .unwrap();

    let options = hybrid_search_options("unrelated query", &query_emb, now);
    let results = store.search(&options).await.unwrap();
    assert!(
        results.iter().any(|r| {
            r.item.commitment_id == Some(commitment_id)
                && r.breakdown.commitment_boost > 0.0
                && r.sources
                    .contains(&crate::MemoryCandidateSource::Commitment)
        }),
        "active ledger commitment should be recalled despite low vector similarity"
    );
}

#[tokio::test]
async fn hybrid_search_excludes_archived_superseded_and_user_deleted() {
    let store = setup_store().await;
    let now = Utc::now();
    let emb = vec![0.5, 0.5, 0.5, 0.5];

    let base = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "memory".into(),
        content: "shared content".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::default(),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };

    for status in [
        crate::MemoryStatus::Archived,
        crate::MemoryStatus::Superseded,
        crate::MemoryStatus::UserDeleted,
    ] {
        let mut item = base.clone();
        item.title = format!("{status:?}");
        item.status = status;
        insert_memory_with_embedding(&store, &item, &emb).await;
    }

    let options = hybrid_search_options("shared content", &emb, now);
    let results = store.search(&options).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn hybrid_search_faded_memory_has_stale_penalty() {
    let store = setup_store().await;
    let now = Utc::now();
    let emb = vec![1.0, 0.0, 0.0, 0.0];

    let item = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "old fact".into(),
        content: "faded memory content".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::default(),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Faded,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };
    insert_memory_with_embedding(&store, &item, &emb).await;

    let options = hybrid_search_options("faded memory", &emb, now);
    let results = store.search(&options).await.unwrap();
    assert_eq!(results.len(), 1);
    let result = results.first().expect("faded memory result");
    assert!(result.breakdown.vector_similarity > 0.0);
    assert!(result.breakdown.stale_penalty > 0.0);
}

#[tokio::test]
async fn hybrid_search_finds_old_lexical_match_outside_recent_pool() {
    let store = setup_store().await;
    let now = Utc::now();
    let query_emb = vec![0.0, 0.0, 1.0, 0.0];
    let orthogonal = vec![0.0, 1.0, 0.0, 0.0];

    let old_item = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "ancient dragon recipe".into(),
        content: "A very old note about ancient dragon recipe".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::default(),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };
    let old_id = store.insert_typed_memory(&old_item).await.unwrap();
    store
        .upsert_memory_embedding(old_id, "test-model", "content", &orthogonal)
        .await
        .unwrap();

    for i in 0..10 {
        let filler = crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Semantic,
            title: format!("recent filler {i}"),
            content: "unrelated filler content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::default(),
            salience: crate::MemorySalience::new(0.95),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        };
        insert_memory_with_embedding(&store, &filler, &orthogonal).await;
    }

    let mut options = hybrid_search_options("ancient dragon recipe", &query_emb, now);
    options.recent_fallback_limit = 0;
    options.similarity_threshold = 0.8;
    let results = store.search(&options).await.unwrap();
    assert!(
        results
            .iter()
            .any(|r| r.item.id == Some(old_id) && r.breakdown.lexical_score > 0.0),
        "old lexical match should be found outside the recent pool"
    );
}

#[tokio::test]
async fn hybrid_search_excludes_unrelated_recent_without_fallback() {
    let store = setup_store().await;
    let now = Utc::now();
    let query_emb = vec![1.0, 0.0, 0.0, 0.0];
    let orthogonal = vec![0.0, 1.0, 0.0, 0.0];

    let unrelated = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "fresh but unrelated".into(),
        content: "nothing to do with the query".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::default(),
        salience: crate::MemorySalience::new(0.99),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };
    insert_memory_with_embedding(&store, &unrelated, &orthogonal).await;

    let mut options = hybrid_search_options("completely different topic", &query_emb, now);
    options.recent_fallback_limit = 0;
    options.similarity_threshold = 0.8;
    let results = store.search(&options).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn hybrid_search_ranks_higher_confidence_when_other_signals_match() {
    let store = setup_store().await;
    let now = Utc::now();
    let query_emb = vec![1.0, 0.0, 0.0, 0.0];

    let base = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "shared topic".into(),
        content: "shared topic content".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::new(0.2),
        salience: crate::MemorySalience::new(0.5),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };
    let low_confidence = base.clone();
    let high_confidence = crate::NewMemoryItem {
        title: "shared topic high".into(),
        confidence: crate::MemoryConfidence::new(0.95),
        ..base
    };

    insert_memory_with_embedding(&store, &low_confidence, &query_emb).await;
    insert_memory_with_embedding(&store, &high_confidence, &query_emb).await;

    let options = hybrid_search_options("shared topic", &query_emb, now);
    let results = store.search(&options).await.unwrap();
    assert_eq!(results.len(), 2);
    let top = results.first().expect("top confidence result");
    let second = results.get(1).expect("second confidence result");
    assert_eq!(top.item.title, "shared topic high");
    assert!(top.breakdown.confidence > second.breakdown.confidence);
}

#[tokio::test]
async fn hybrid_search_respects_user_id_scope() {
    let store = setup_store().await;
    let now = Utc::now();
    let query_emb = vec![1.0, 0.0, 0.0, 0.0];

    let base = crate::NewMemoryItem {
        scope: crate::MemoryScope::User,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "scoped memory".into(),
        content: "user scoped content".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::default(),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };

    let mut user1_item = base.clone();
    user1_item.user_id = "user1".into();
    let mut user2_item = base;
    user2_item.user_id = "user2".into();

    insert_memory_with_embedding(&store, &user1_item, &query_emb).await;
    insert_memory_with_embedding(&store, &user2_item, &query_emb).await;

    let mut options = hybrid_search_options("scoped memory", &query_emb, now);
    options.user_id = Some("user1");
    let results = store.search(&options).await.unwrap();
    assert_eq!(results.len(), 1);
    let result = results.first().expect("scoped search result");
    assert_eq!(result.item.user_id, "user1");
}

#[tokio::test]
async fn hybrid_search_dedupes_multi_source_candidates() {
    let store = setup_store().await;
    let now = Utc::now();
    let emb = vec![1.0, 0.0, 0.0, 0.0];

    let item = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "pizza night".into(),
        content: "Friday pizza tradition".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::default(),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };
    insert_memory_with_embedding(&store, &item, &emb).await;

    let options = hybrid_search_options("pizza tradition", &emb, now);
    let results = store.search(&options).await.unwrap();
    assert_eq!(results.len(), 1);
    let result = results.first().expect("deduped search result");
    assert!(result.sources.len() >= 2);
}

#[tokio::test]
async fn set_memory_status_rejects_invalid_edge() {
    let store = setup_store().await;
    let id = store
        .insert_typed_memory(&crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Semantic,
            title: "fact".into(),
            content: "content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::default(),
            salience: crate::MemorySalience::default(),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Faded,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        })
        .await
        .unwrap();

    let err = store
        .set_memory_status(id, crate::MemoryStatus::Active)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        EneMemoryError::InvalidTransition {
            from: crate::MemoryStatus::Faded,
            to: crate::MemoryStatus::Active,
        }
    ));
}

#[tokio::test]
async fn apply_natural_decay_batch_fades_and_archives() {
    let store = setup_store().await;
    let now = Utc::now();

    let active_id = store
        .insert_typed_memory(&crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind: crate::MemoryKind::Semantic,
            title: "old active".into(),
            content: "old content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::new(0.2),
            salience: crate::MemorySalience::new(0.1),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        })
        .await
        .unwrap();
    store
        .test_backdate_typed_memory(active_id, 120)
        .await
        .unwrap();

    let faded_id = store
        .insert_typed_memory(&crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind: crate::MemoryKind::Semantic,
            title: "old faded".into(),
            content: "very old content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::new(0.1),
            salience: crate::MemorySalience::new(0.1),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Faded,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        })
        .await
        .unwrap();
    store
        .test_backdate_typed_memory(faded_id, 365)
        .await
        .unwrap();

    let report = store
        .apply_natural_decay_batch("ene", Some("user1"), now, 30.0, 64)
        .await
        .unwrap();
    assert!(report.faded_count >= 1);
    assert!(report.archived_count >= 1);

    let active_loaded = store.get_typed_memory(active_id).await.unwrap().unwrap();
    assert_eq!(active_loaded.status, crate::MemoryStatus::Faded);

    let faded_loaded = store.get_typed_memory(faded_id).await.unwrap().unwrap();
    assert_eq!(faded_loaded.status, crate::MemoryStatus::Archived);
}

#[tokio::test]
async fn pin_typed_memory_excludes_from_natural_decay() {
    let store = setup_store().await;
    let id = store
        .insert_typed_memory(&crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: crate::MemoryKind::Semantic,
            title: "pinned".into(),
            content: "pinned content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::new(0.1),
            salience: crate::MemorySalience::new(0.1),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Active,
            supersedes_id: None,
            pinned: true,
            created_at: None,
            commitment_id: None,
        })
        .await
        .unwrap();
    store.test_backdate_typed_memory(id, 200).await.unwrap();

    let report = store
        .apply_natural_decay_batch("ene", None, Utc::now(), 30.0, 64)
        .await
        .unwrap();
    assert_eq!(report.faded_count, 0);

    let loaded = store.get_typed_memory(id).await.unwrap().unwrap();
    assert_eq!(loaded.status, crate::MemoryStatus::Active);
    assert!(loaded.pinned);
}

#[tokio::test]
async fn transition_active_to_faded_sets_faded_at_from_decay_anchor() {
    let store = setup_store().await;
    let id = store
        .insert_typed_memory(&crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind: crate::MemoryKind::Semantic,
            title: "anchor test".into(),
            content: "anchor content".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::default(),
            salience: crate::MemorySalience::default(),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        })
        .await
        .unwrap();
    store.test_backdate_typed_memory(id, 45).await.unwrap();

    let before = store.get_typed_memory(id).await.unwrap().unwrap();
    assert!(
        store
            .set_memory_status(id, crate::MemoryStatus::Faded)
            .await
            .unwrap()
    );

    let after = store.get_typed_memory(id).await.unwrap().unwrap();
    assert_eq!(after.status, crate::MemoryStatus::Faded);
    assert_eq!(after.faded_at, Some(before.updated_at));
}

#[tokio::test]
async fn single_row_natural_decay_reaches_archived_in_two_passes() {
    let store = setup_store().await;
    let now = Utc::now();

    let id = store
        .insert_typed_memory(&crate::NewMemoryItem {
            scope: crate::MemoryScope::Character,
            character_id: "ene".into(),
            user_id: "user1".into(),
            kind: crate::MemoryKind::Semantic,
            title: "ancient".into(),
            content: "very old fact".into(),
            source: crate::MemorySource::Conversation,
            source_ref: None,
            confidence: crate::MemoryConfidence::new(0.1),
            salience: crate::MemorySalience::new(0.1),
            affect: crate::AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: crate::MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        })
        .await
        .unwrap();
    store.test_backdate_typed_memory(id, 365).await.unwrap();

    let first = store
        .apply_natural_decay_batch("ene", Some("user1"), now, 30.0, 64)
        .await
        .unwrap();
    assert_eq!(first.faded_count, 1);
    assert_eq!(first.archived_count, 0);

    let faded = store.get_typed_memory(id).await.unwrap().unwrap();
    assert_eq!(faded.status, crate::MemoryStatus::Faded);
    assert!(faded.faded_at.is_some());

    let second = store
        .apply_natural_decay_batch("ene", Some("user1"), now, 30.0, 64)
        .await
        .unwrap();
    assert_eq!(second.archived_count, 1);

    let archived = store.get_typed_memory(id).await.unwrap().unwrap();
    assert_eq!(archived.status, crate::MemoryStatus::Archived);
}

#[tokio::test]
async fn hybrid_search_preserves_pinned_flag() {
    let store = setup_store().await;
    let now = Utc::now();
    let emb = vec![1.0, 0.0, 0.0, 0.0];

    let item = crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: "pinned fact".into(),
        content: "pinned vector content".into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::default(),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: true,
        created_at: None,
        commitment_id: None,
    };
    insert_memory_with_embedding(&store, &item, &emb).await;

    let options = hybrid_search_options("pinned vector", &emb, now);
    let results = store.search(&options).await.unwrap();
    assert_eq!(results.len(), 1);
    let result = results.first().expect("pinned search result");
    assert!(result.item.pinned);
}

#[tokio::test]
async fn file_backup_restore_and_integrity_roundtrip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path, 4).await.expect("open");
    store
        .insert_log("s1", "card", "user", "hello")
        .await
        .expect("insert");
    store.check_integrity().await.expect("integrity");
    let backup = store.backup().await.expect("backup");
    drop(store);

    // Corrupt the live DB, then restore from the backup.
    std::fs::write(&path, b"not-a-sqlite-database").expect("corrupt");
    crate::backup::restore_database(&backup, &path).expect("restore");
    let store = MemoryStore::open(&path, 4).await.expect("reopen");
    store
        .check_integrity()
        .await
        .expect("integrity after restore");
    let logs = store.get_logs_by_session("s1").await.expect("logs");
    assert_eq!(logs.len(), 1);
}

#[tokio::test]
async fn schema_too_new_is_reported() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path, 4).await.expect("open");
    // Inject an unknown migration name into seaql_migrations.
    store
        .connection()
        .execute_unprepared(
            "INSERT INTO seaql_migrations (version, applied_at) VALUES ('m20990101_future', 0)",
        )
        .await
        .expect("inject");
    drop(store);

    let err = MemoryStore::open(&path, 4)
        .await
        .err()
        .expect("expected SchemaTooNew");
    assert!(
        matches!(err, EneMemoryError::SchemaTooNew { .. }),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn restore_after_simulated_migration_failure_keeps_db_usable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path, 4).await.expect("open");
    store
        .insert_log("s1", "card", "user", "keep-me")
        .await
        .expect("insert");
    let backup = store.backup().await.expect("backup");
    drop(store);

    // Simulate a half-applied migration by wiping the live file, then
    // restoring the backup the way open_with_options does on failure.
    std::fs::write(&path, b"").expect("wipe");
    crate::backup::restore_database(&backup, &path).expect("restore");
    let store = MemoryStore::open(&path, 4).await.expect("reopen");
    let logs = store.get_logs_by_session("s1").await.expect("logs");
    assert_eq!(logs.len(), 1);
    assert_eq!(
        logs.first().map(|entry| entry.content.as_str()),
        Some("keep-me")
    );
}

#[tokio::test]
async fn pending_memory_write_queue_roundtrip() {
    let store = setup_store().await;
    let id = store
        .enqueue_pending_memory_write("ene", "user", r#"{"character_id":"ene"}"#, "boom")
        .await
        .expect("enqueue");
    let (pending, permanent) = store
        .count_pending_memory_writes("ene")
        .await
        .expect("count");
    assert_eq!(pending, 1);
    assert_eq!(permanent, 0);

    // Force due by setting next_retry_at to the past via fail/complete path:
    // take_due only returns rows with next_retry_at <= now; freshly enqueued
    // rows wait 30s, so mark due by failing with attempts that keep pending
    // and a zero delay — instead, complete and re-enqueue is enough to prove CRUD.
    let listed = store
        .list_pending_memory_writes("ene", 10)
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed.first().map(|r| r.id), Some(id));

    store
        .complete_pending_memory_write(id)
        .await
        .expect("complete");
    let (pending, _) = store
        .count_pending_memory_writes("ene")
        .await
        .expect("count after");
    assert_eq!(pending, 0);
}

#[test]
fn strip_tags_footer_removes_trailing_footer() {
    let content = "User likes coffee.\n\n<!-- ene:tags {\"tags\":[\"interrupted\"]} -->";
    assert_eq!(strip_tags_footer(content), "User likes coffee.");
}

#[test]
fn strip_tags_footer_leaves_content_without_footer() {
    let content = "User likes coffee.";
    assert_eq!(strip_tags_footer(content), "User likes coffee.");
}

#[test]
fn strip_tags_footer_handles_empty_content() {
    assert_eq!(strip_tags_footer(""), "");
}

#[test]
fn strip_tags_footer_preserves_mid_content_marker() {
    // "ene:tags" appearing mid-content (without the `\n\n<!-- ` prefix)
    // is not a footer and must be preserved.
    let content = "Discussed ene:tags format in the meeting.";
    assert_eq!(
        strip_tags_footer(content),
        "Discussed ene:tags format in the meeting."
    );
}

#[test]
fn strip_tags_footer_multiline_content() {
    let content = "Line one.\nLine two.\n\n<!-- ene:tags {\"tags\":[\"a\",\"b\"]} -->";
    assert_eq!(strip_tags_footer(content), "Line one.\nLine two.");
}

fn sample_pending_candidate(character_id: &str, title: &str) -> PendingCandidate {
    PendingCandidate {
        id: 0,
        character_id: character_id.to_string(),
        user_id: "user1".to_string(),
        title: title.to_string(),
        content: format!("{title} content"),
        kind: crate::MemoryKind::Preference,
        confidence: 0.8,
        reason_detail: "extracted from conversation".to_string(),
        existing_memory_title: None,
        source_quote: "I like tea".to_string(),
        status: PendingCandidateStatus::Pending,
    }
}

#[tokio::test]
async fn pending_candidate_insert_list_approve_persists_typed_memory() {
    let store = setup_store().await;

    let id = store
        .insert_pending_candidate(sample_pending_candidate("ene", "likes tea"))
        .expect("insert");
    assert_eq!(id, 1);

    let listed = store
        .list_pending_candidates("ene", Some(PendingCandidateStatus::Pending))
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "likes tea");
    assert_eq!(listed[0].status, PendingCandidateStatus::Pending);

    // Approving persists the candidate to typed memory.
    let memory_id = store.approve_pending_candidate(id).await.expect("approve");
    let memory = store
        .get_typed_memory(memory_id)
        .await
        .expect("get memory")
        .expect("memory exists");
    assert_eq!(memory.title, "likes tea");
    assert_eq!(memory.kind, crate::MemoryKind::Preference);
    assert_eq!(memory.status, crate::MemoryStatus::Active);

    // The candidate is now approved and no longer pending.
    let pending = store
        .list_pending_candidates("ene", Some(PendingCandidateStatus::Pending))
        .expect("list pending");
    assert!(pending.is_empty());
    let approved = store
        .list_pending_candidates("ene", Some(PendingCandidateStatus::Approved))
        .expect("list approved");
    assert_eq!(approved.len(), 1);

    // Approving again fails because it is already resolved.
    assert!(store.approve_pending_candidate(id).await.is_err());
}

#[tokio::test]
async fn pending_candidate_reject_does_not_persist() {
    let store = setup_store().await;

    let id = store
        .insert_pending_candidate(sample_pending_candidate("ene", "rejected fact"))
        .expect("insert");
    store
        .resolve_pending_candidate(id, false)
        .await
        .expect("reject");

    let rejected = store
        .list_pending_candidates("ene", Some(PendingCandidateStatus::Rejected))
        .expect("list rejected");
    assert_eq!(rejected.len(), 1);

    // No typed memory was created for a rejected candidate.
    let count = store
        .count_typed_memories("ene", None)
        .await
        .expect("count");
    assert_eq!(count, 0);
}

#[tokio::test]
async fn pending_candidates_are_isolated_per_store_instance() {
    let store_a = setup_store().await;
    let store_b = setup_store().await;

    store_a
        .insert_pending_candidate(sample_pending_candidate("ene", "only in A"))
        .expect("insert A");

    let in_a = store_a
        .list_pending_candidates("ene", None)
        .expect("list A");
    assert_eq!(in_a.len(), 1);

    // The second store instance must not see store A's candidates.
    let in_b = store_b
        .list_pending_candidates("ene", None)
        .expect("list B");
    assert!(in_b.is_empty());
}

// ── #419: PRAGMAs must reach every pooled connection ──

/// Regression test for #419: the per-connection PRAGMAs must set
/// `foreign_keys=ON` (and the other safety PRAGMAs) on **every**
/// connection in the pool, not just the first one. A file-backed
/// store uses a pool of eight connections; we deterministically
/// exercise all of them by opening eight concurrent transactions
/// (each holds a distinct pool connection) behind a barrier so
/// they are all live simultaneously before checking the PRAGMA.
#[tokio::test]
async fn pragmas_apply_to_all_pool_connections() {
    use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
    use std::sync::Arc;
    use tokio::sync::Barrier;

    const POOL_SIZE: usize = 8;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path, 4).await.expect("open");

    let barrier = Arc::new(Barrier::new(POOL_SIZE));
    let mut handles = Vec::with_capacity(POOL_SIZE);
    for _ in 0..POOL_SIZE {
        let barrier = Arc::clone(&barrier);
        let db = store.connection().clone();
        handles.push(tokio::spawn(async move {
            let txn = db.begin().await.expect("begin txn");
            barrier.wait().await;
            let row = txn
                .query_one_raw(Statement::from_string(
                    DbBackend::Sqlite,
                    "PRAGMA foreign_keys".to_string(),
                ))
                .await
                .expect("query PRAGMA foreign_keys")
                .expect("row");
            let fk_on: i32 = row.try_get_by_index(0).expect("value");
            txn.commit().await.expect("commit");
            fk_on
        }));
    }

    for handle in handles {
        let fk_on = handle.await.expect("task panicked");
        assert_eq!(
            fk_on, 1,
            "foreign_keys must report 1 (ON) for every pool connection"
        );
    }
}

// ── #421: FK cascade and unique index correctness ──

/// Builds a minimal [`crate::NewMemoryItem`] for test use.
fn test_memory_item(title: &str, content: &str) -> crate::NewMemoryItem {
    crate::NewMemoryItem {
        scope: crate::MemoryScope::Character,
        character_id: "ene".into(),
        user_id: String::new(),
        kind: crate::MemoryKind::Semantic,
        title: title.into(),
        content: content.into(),
        source: crate::MemorySource::Conversation,
        source_ref: None,
        confidence: crate::MemoryConfidence::default(),
        salience: crate::MemorySalience::default(),
        affect: crate::AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: crate::MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    }
}

/// Deleting a `typed_memories` row must cascade-delete its
/// `memory_embeddings` rows now that `foreign_keys=ON` is
/// enforced on every pooled connection (#419).
#[tokio::test]
async fn delete_typed_memory_cascades_to_embeddings() {
    use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter};

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path, 4).await.expect("open");

    let item = test_memory_item("cascade test", "content to embed");
    let id = store.insert_typed_memory(&item).await.expect("insert");
    store
        .upsert_memory_embedding(id, "test-model", "content", &[0.1, 0.2, 0.3, 0.4])
        .await
        .expect("upsert embedding");

    let count_before = crate::entities::memory_embeddings::Entity::find()
        .filter(crate::entities::memory_embeddings::Column::MemoryItemId.eq(id))
        .count(store.connection())
        .await
        .expect("count before");
    assert_eq!(count_before, 1, "embedding must exist before delete");

    // Delete the parent row directly; the FK ON DELETE CASCADE must fire.
    store
        .connection()
        .execute_unprepared(&format!("DELETE FROM typed_memories WHERE id = {id}"))
        .await
        .expect("delete parent");

    let count_after = crate::entities::memory_embeddings::Entity::find()
        .filter(crate::entities::memory_embeddings::Column::MemoryItemId.eq(id))
        .count(store.connection())
        .await
        .expect("count after");
    assert_eq!(count_after, 0, "embedding must be cascade-deleted");
}

/// The unique index `uniq_memory_embedding` must prevent duplicate
/// rows for the same `(memory_item_id, model_name, field)` triple.
/// A second upsert must update the existing row, not insert a new one.
#[tokio::test]
async fn upsert_memory_embedding_does_not_create_duplicates() {
    use sea_orm::{DbBackend, Statement};

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path, 4).await.expect("open");

    let item = test_memory_item("dedup test", "content to embed");
    let id = store.insert_typed_memory(&item).await.expect("insert");

    let emb1: Vec<f32> = vec![0.1, 0.2, 0.3, 0.4];
    store
        .upsert_memory_embedding(id, "test-model", "content", &emb1)
        .await
        .expect("first upsert");

    let emb2: Vec<f32> = vec![0.5, 0.6, 0.7, 0.8];
    store
        .upsert_memory_embedding(id, "test-model", "content", &emb2)
        .await
        .expect("second upsert (update, not insert)");

    let rows = store
        .connection()
        .query_all_raw(Statement::from_string(
            DbBackend::Sqlite,
            format!(
                "SELECT embedding FROM memory_embeddings \
                 WHERE memory_item_id = {id} AND model_name = 'test-model' AND field = 'content'"
            ),
        ))
        .await
        .expect("query embeddings");

    assert_eq!(
        rows.len(),
        1,
        "unique index must prevent duplicate (memory_item_id, model_name, field)"
    );

    let stored_bytes: Vec<u8> = rows[0].try_get_by_index(0).expect("embedding bytes");
    assert_eq!(
        stored_bytes,
        embedding_to_bytes(&emb2),
        "stored embedding must be the second (updated) value"
    );
}
