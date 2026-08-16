#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "integration tests use unwrap/expect for concise assertions"
)]

mod common;

use ene_card::CharacterCardV3;
use ene_runtime::{EneConfig, EneHandle, LifecycleEvent, MemoryLedgerChange};
use ene_store::{
    AffectAnnotation, CommitmentStatus, MemoryConfidence, MemoryEdit, MemoryJournalListOptions,
    MemoryKind, MemorySalience, MemoryScope, MemorySource, MemoryStatus, MemoryStore,
    NewCommitment, NewMemoryItem,
};
use std::sync::Arc;

/// Removes the test's character directory on drop, so a panicked test cannot
/// leave `assets/characters/<name>` behind.
struct TestCharacterDir(&'static str);

impl Drop for TestCharacterDir {
    fn drop(&mut self) {
        drop(std::fs::remove_dir_all(ene_config::paths::character_dir(
            self.0,
        )));
    }
}

fn test_card() -> CharacterCardV3 {
    common::test_card("LedgerTest")
}

fn test_memory(title: &str, kind: MemoryKind, status: MemoryStatus) -> NewMemoryItem {
    NewMemoryItem {
        scope: MemoryScope::Character,
        character_id: "LedgerTest".into(),
        user_id: String::new(),
        kind,
        title: title.into(),
        content: format!("content for {title}"),
        source: MemorySource::Conversation,
        source_ref: None,
        confidence: MemoryConfidence::new(0.8),
        salience: MemorySalience::new(0.4),
        affect: AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    }
}

/// Opens a handle backed by a temp-file store, then seeds it through a second
/// connection on the same file so the ledger handle can read the rows.
///
/// The database lives under the debug-build assets dir at
/// `assets/characters/{character}/memory.db`; callers must pass a unique
/// character name per test and clean the directory up afterwards.
async fn open_seeded_handle(character: &str) -> (EneHandle, Arc<MemoryStore>) {
    let mut config = EneConfig {
        character: character.to_string(),
        ..Default::default()
    };
    let store = ene_store::StoreConfig {
        enabled: true,
        ..Default::default()
    };
    config.set_section(&store).expect("store config merges");
    let tools = ene_plugin_host::PluginConfig {
        enabled: false,
        ..Default::default()
    };
    drop(config.set_section(&tools));

    let handle = EneHandle::open(config, test_card())
        .await
        .expect("open initializes handle");
    let db_path = ene_config::paths::character_dir(character).join("memory.db");
    let seeder = Arc::new(MemoryStore::open(&db_path, 4).await.expect("seeder store"));
    (handle, seeder)
}

#[tokio::test]
async fn ledger_lists_memories_and_commitments_across_statuses() {
    let _cleanup = TestCharacterDir("LedgerListTest");
    let (handle, seeder) = open_seeded_handle("LedgerListTest").await;

    let memory_id = seeder
        .insert_typed_memory(&test_memory(
            "coffee",
            MemoryKind::Preference,
            MemoryStatus::Active,
        ))
        .await
        .unwrap();
    seeder
        .insert_typed_memory(&test_memory(
            "old note",
            MemoryKind::Episodic,
            MemoryStatus::UserDeleted,
        ))
        .await
        .unwrap();
    let active_commitment = seeder
        .insert_commitment(&NewCommitment {
            character_id: "LedgerTest".into(),
            user_id: String::new(),
            title: "send report".into(),
            description: String::new(),
            status: CommitmentStatus::Active,
            due_at: None,
            due_label: None,
        })
        .await
        .unwrap();
    seeder
        .insert_commitment(&NewCommitment {
            character_id: "LedgerTest".into(),
            user_id: String::new(),
            title: "old task".into(),
            description: String::new(),
            status: CommitmentStatus::Done,
            due_at: None,
            due_label: None,
        })
        .await
        .unwrap();
    drop(seeder);

    let ledger = handle.memory_ledger();
    let character_id = handle.card_name();

    let options = MemoryJournalListOptions {
        character_id: &character_id,
        user_id: None,
        include_archived: true,
        include_superseded: true,
        include_user_deleted: true,
        kind: None,
        limit: 50,
        offset: 0,
    };
    let memories = ledger.list_memories(&options).await.expect("list memories");
    assert_eq!(memories.len(), 2);
    assert!(memories.iter().any(|m| m.id == Some(memory_id)));

    let inspected = ledger
        .inspect_memory(memory_id)
        .await
        .expect("inspect memory")
        .expect("seeded memory exists");
    assert_eq!(inspected.title, "coffee");
    assert_eq!(inspected.kind, MemoryKind::Preference);

    let commitments = ledger
        .list_commitments(None, None, 50)
        .await
        .expect("list commitments");
    assert_eq!(commitments.len(), 2, "all statuses must be listable");
    let done = ledger
        .list_commitments(None, Some(CommitmentStatus::Done), 50)
        .await
        .expect("filtered commitments");
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].title, "old task");

    // Deletion reuses the existing MemoryHandle surface.
    let memory = handle.diagnostics().memory().clone();
    assert!(memory.user_forget_typed_memory(memory_id).await.unwrap());
    let after_delete = ledger
        .list_memories(&options)
        .await
        .expect("list after delete");
    assert_eq!(after_delete.len(), 2);
    assert!(
        after_delete
            .iter()
            .any(|m| m.id == Some(memory_id) && m.status == MemoryStatus::UserDeleted)
    );

    // Commitment lifecycle reuses the existing MemoryHandle surface.
    assert!(memory.complete_commitment(active_commitment).await.unwrap());
    let after_complete = ledger
        .list_commitments(None, Some(CommitmentStatus::Done), 50)
        .await
        .expect("done after complete");
    assert!(
        after_complete
            .iter()
            .any(|c| c.id == Some(active_commitment))
    );

    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
}

#[tokio::test]
async fn ledger_edit_and_salience_persist_and_emit_audit_events() {
    let _cleanup = TestCharacterDir("LedgerEditTest");
    let (handle, seeder) = open_seeded_handle("LedgerEditTest").await;

    let memory_id = seeder
        .insert_typed_memory(&test_memory(
            "coffee",
            MemoryKind::Preference,
            MemoryStatus::Active,
        ))
        .await
        .unwrap();
    drop(seeder);

    let ledger = handle.memory_ledger();
    let mut lifecycle = handle.subscribe_lifecycle();

    ledger
        .edit_memory(
            memory_id,
            MemoryEdit {
                title: "tea".into(),
                content: "prefers tea over coffee".into(),
                kind: MemoryKind::Preference,
                confidence: MemoryConfidence::new(0.9),
            },
            None,
        )
        .await
        .expect("edit succeeds");
    ledger
        .set_memory_salience(memory_id, 0.85, None)
        .await
        .expect("salience set succeeds");

    let edited = ledger
        .inspect_memory(memory_id)
        .await
        .expect("inspect")
        .expect("memory exists");
    assert_eq!(edited.title, "tea");
    assert_eq!(edited.content, "prefers tea over coffee");
    assert!((edited.confidence.get() - 0.9).abs() < f32::EPSILON);
    assert!((edited.salience.get() - 0.85).abs() < f32::EPSILON);
    assert_eq!(
        edited.scope,
        MemoryScope::User,
        "Preference kind must carry the canonical User scope"
    );
    assert_eq!(
        edited.user_id, "User",
        "User-scope rows must move to the editing user (config user_name)"
    );

    let mut saw_edited = false;
    let mut saw_salience = false;
    for _ in 0..4 {
        let Ok(event) = lifecycle.try_recv() else {
            break;
        };
        if let LifecycleEvent::MemoryLedgerChanged { id, action, turn } = event {
            assert_eq!(id, memory_id);
            assert!(turn.is_none());
            match action {
                MemoryLedgerChange::Edited => {
                    saw_edited = true;
                }
                MemoryLedgerChange::SalienceAdjusted => {
                    saw_salience = true;
                }
            }
        }
    }
    assert!(
        saw_edited,
        "edit must emit a MemoryLedgerChanged audit event"
    );
    assert!(
        saw_salience,
        "salience adjustment must emit a MemoryLedgerChanged audit event"
    );

    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
}

#[tokio::test]
async fn ledger_mutations_report_missing_rows_without_events() {
    let _cleanup = TestCharacterDir("LedgerMissingTest");
    let (handle, seeder) = open_seeded_handle("LedgerMissingTest").await;
    drop(seeder);

    let ledger = handle.memory_ledger();
    let mut lifecycle = handle.subscribe_lifecycle();

    let edit_result = ledger
        .edit_memory(
            999_999,
            MemoryEdit {
                title: "ghost".into(),
                content: "no such row".into(),
                kind: MemoryKind::Semantic,
                confidence: MemoryConfidence::new(0.5),
            },
            None,
        )
        .await;
    assert!(matches!(
        edit_result,
        Err(ene_runtime::PublicApiError::NotFound { .. })
    ));
    assert!(
        ledger
            .set_memory_salience(999_999, 0.5, None)
            .await
            .is_err(),
        "missing row must fail closed"
    );
    assert!(
        lifecycle.try_recv().is_err(),
        "failed mutations must not emit audit events"
    );

    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
}

#[tokio::test]
async fn ledger_invalid_edit_maps_to_invalid_error() {
    let _cleanup = TestCharacterDir("LedgerInvalidTest");
    let (handle, seeder) = open_seeded_handle("LedgerInvalidTest").await;
    let memory_id = seeder
        .insert_typed_memory(&test_memory(
            "coffee",
            MemoryKind::Preference,
            MemoryStatus::Active,
        ))
        .await
        .unwrap();
    drop(seeder);

    let ledger = handle.memory_ledger();
    let result = ledger
        .edit_memory(
            memory_id,
            MemoryEdit {
                title: "   ".into(),
                content: "content".into(),
                kind: MemoryKind::Preference,
                confidence: MemoryConfidence::new(0.5),
            },
            None,
        )
        .await;
    assert!(
        matches!(result, Err(ene_runtime::PublicApiError::Invalid { .. })),
        "blank-title edits must surface as Invalid, got {result:?}"
    );

    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
}
