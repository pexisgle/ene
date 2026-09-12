//! Action attempt persistence (V19): AU5 start compare, certainty CAS, and
//! fail-closed restart reads.

use super::*;

use ene_action::{
    ActionAttemptId, ActionAttemptRepository, ActionCertainty, ActionStartOutcome,
    ActionTechnicalError, AttemptCommitPremise, CertaintyUpdateOutcome, EffectGrounds,
    OperationKind, RealTargetRef,
};

fn attempt_premise(
    attempt: ActionAttemptId,
    delegation: DelegationId,
    task: TaskRef,
    workspace: WorkspaceAssocId,
    target: &str,
    operation: OperationKind,
) -> AttemptCommitPremise {
    AttemptCommitPremise {
        attempt,
        delegation: delegation.as_raw(),
        task: task.task.as_raw(),
        task_revision: RevisionInner::from_u64(task.revision.as_u64()),
        workspace: workspace.as_raw(),
        real_target: RealTargetRef::from_canonical_path(target.to_owned()),
        operation,
    }
}

/// A platform-absolute fixture target. `Path::is_absolute` requires a Windows
/// prefix, so fixtures cannot hardcode a Unix path.
fn target_path(name: &str) -> String {
    std::env::temp_dir()
        .join(name)
        .to_string_lossy()
        .into_owned()
}

/// Seeds one Task with a confirmed workspace association and one delegation
/// whose copied scope relies on exactly that association.
async fn seed_workspace_delegation(store: &Store) -> (TaskRef, DelegationId, WorkspaceAssocId) {
    let workspace = task_workspace("/srv/workspace/ene", Some("/srv/workspace/ene/out"));
    let assoc = workspace.assoc;
    let created = store
        .create_task(task_premise(Some(workspace)))
        .await
        .expect("the AU2 task must commit");
    let delegation = DelegationId::generate();
    let delegated = store
        .create_delegation(delegation_premise(
            delegation,
            created,
            TaskAgentEphemeralId::generate(),
            delegation_scope(Some(delegated_workspace(
                assoc,
                "/srv/workspace/ene",
                Some("/srv/workspace/ene/out"),
            ))),
        ))
        .await
        .expect("the AU3 delegation must commit");
    assert!(
        matches!(delegated, DelegationOutcome::Delegated(_)),
        "seed delegation commits: {delegated:?}"
    );
    (created, delegation, assoc)
}

#[tokio::test]
async fn action_attempt_records_the_durable_correlation() {
    let store = open_memory().await.unwrap();
    let (created, delegation, assoc) = seed_workspace_delegation(&store).await;
    let attempt = ActionAttemptId::generate();
    let target = target_path("report.md");
    assert_eq!(
        store
            .insert_attempt_if_current(attempt_premise(
                attempt,
                delegation,
                created,
                assoc,
                &target,
                OperationKind::Create,
            ))
            .await,
        Ok(ActionStartOutcome::Started)
    );
    let record = store
        .load_attempt(attempt)
        .await
        .expect("the attempt row must read")
        .expect("the inserted attempt must exist");
    assert_eq!(record.attempt, attempt);
    assert_eq!(record.delegation, delegation.as_raw());
    assert_eq!(record.task, created.task.as_raw());
    assert_eq!(record.task_revision.as_u64(), created.revision.as_u64());
    assert_eq!(record.workspace, assoc.as_raw());
    assert_eq!(record.real_target.as_path(), target.as_str());
    assert_eq!(record.operation, OperationKind::Create);
    assert_eq!(
        record.certainty,
        ActionCertainty::Unknown,
        "a started attempt is unknown until an observation is recorded"
    );
    assert_eq!(record.grounds, None);
    assert_eq!(task_table_count(&store, "action_attempt"), 1);
}

#[tokio::test]
async fn action_attempt_correlation_survives_reopen_without_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("action-restart.db");
    let attempt = ActionAttemptId::generate();
    {
        let store = Store::open(&path).await.unwrap();
        let (created, delegation, assoc) = seed_workspace_delegation(&store).await;
        assert_eq!(
            store
                .insert_attempt_if_current(attempt_premise(
                    attempt,
                    delegation,
                    created,
                    assoc,
                    &target_path("report.md"),
                    OperationKind::Create,
                ))
                .await,
            Ok(ActionStartOutcome::Started)
        );
    }
    let reopened = Store::open(&path).await.expect("reopen must succeed");
    let record = reopened
        .load_attempt(attempt)
        .await
        .expect("the correlation must read after restart")
        .expect("the started attempt survives restart");
    assert_eq!(record.certainty, ActionCertainty::Unknown);
    assert_eq!(
        task_table_count(&reopened, "action_attempt"),
        1,
        "reopen replays nothing"
    );
}

#[tokio::test]
async fn action_attempt_is_stale_after_a_steering_forward() {
    let store = open_memory().await.unwrap();
    let (created, delegation, assoc) = seed_workspace_delegation(&store).await;
    let advanced = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: Some(task_purpose_adoption("moved before the action")),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await
        .unwrap();
    assert!(matches!(advanced, TaskCommitOutcome::CommittedAs(_)));
    assert_eq!(
        store
            .insert_attempt_if_current(attempt_premise(
                ActionAttemptId::generate(),
                delegation,
                created,
                assoc,
                &target_path("report.md"),
                OperationKind::Create,
            ))
            .await,
        Ok(ActionStartOutcome::StalePremise),
        "a moved task revision refuses the start"
    );
    assert_eq!(task_table_count(&store, "action_attempt"), 0);
}

#[tokio::test]
async fn action_attempt_is_stale_when_the_delegation_is_missing() {
    let store = open_memory().await.unwrap();
    let (created, _delegation, assoc) = seed_workspace_delegation(&store).await;
    assert_eq!(
        store
            .insert_attempt_if_current(attempt_premise(
                ActionAttemptId::generate(),
                DelegationId::generate(),
                created,
                assoc,
                &target_path("report.md"),
                OperationKind::Create,
            ))
            .await,
        Ok(ActionStartOutcome::StalePremise)
    );
    assert_eq!(task_table_count(&store, "action_attempt"), 0);
}

#[tokio::test]
async fn action_attempt_is_stale_without_a_workspace_association() {
    let store = open_memory().await.unwrap();
    let created = store.create_task(task_premise(None)).await.unwrap();
    let delegation = DelegationId::generate();
    let delegated = store
        .create_delegation(delegation_premise(
            delegation,
            created,
            TaskAgentEphemeralId::generate(),
            delegation_scope(None),
        ))
        .await
        .unwrap();
    assert!(matches!(delegated, DelegationOutcome::Delegated(_)));
    assert_eq!(
        store
            .insert_attempt_if_current(attempt_premise(
                ActionAttemptId::generate(),
                delegation,
                created,
                WorkspaceAssocId::generate(),
                &target_path("report.md"),
                OperationKind::Create,
            ))
            .await,
        Ok(ActionStartOutcome::StalePremise)
    );
    assert_eq!(task_table_count(&store, "action_attempt"), 0);
}

#[tokio::test]
async fn action_attempt_is_stale_when_the_delegated_scope_is_wider_than_the_association() {
    let store = open_memory().await.unwrap();
    let workspace = task_workspace("/srv/workspace/ene", None);
    let assoc = workspace.assoc;
    let created = store
        .create_task(task_premise(Some(workspace)))
        .await
        .unwrap();
    let delegation = DelegationId::generate();
    let delegated = store
        .create_delegation(delegation_premise(
            delegation,
            created,
            TaskAgentEphemeralId::generate(),
            delegation_scope(Some(delegated_workspace(
                WorkspaceAssocId::generate(),
                "/srv/workspace/other",
                None,
            ))),
        ))
        .await
        .unwrap();
    assert!(matches!(delegated, DelegationOutcome::Delegated(_)));
    assert_eq!(
        store
            .insert_attempt_if_current(attempt_premise(
                ActionAttemptId::generate(),
                delegation,
                created,
                assoc,
                &target_path("report.md"),
                OperationKind::Create,
            ))
            .await,
        Ok(ActionStartOutcome::StalePremise),
        "a delegation whose copy relies on another boundary cannot witness this one"
    );
    assert_eq!(task_table_count(&store, "action_attempt"), 0);
}

#[tokio::test]
async fn action_attempt_premise_disagreement_is_a_technical_error() {
    let store = open_memory().await.unwrap();
    let (created, delegation, assoc) = seed_workspace_delegation(&store).await;
    let mut premise = attempt_premise(
        ActionAttemptId::generate(),
        delegation,
        created,
        assoc,
        &target_path("report.md"),
        OperationKind::Create,
    );
    premise.task_revision = RevisionInner::from_u64(created.revision.as_u64() + 7);
    let outcome = store.insert_attempt_if_current(premise).await;
    assert!(
        matches!(
            outcome,
            Err(ActionTechnicalError::StorageUnavailable { .. })
        ),
        "a delegation/premise disagreement is a technical error, got {outcome:?}"
    );
    assert_eq!(task_table_count(&store, "action_attempt"), 0);
}

#[tokio::test]
async fn multiple_workspace_associations_fail_closed_at_start() {
    let store = open_memory().await.unwrap();
    let (created, delegation, assoc) = seed_workspace_delegation(&store).await;
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "INSERT INTO workspace_assoc (assoc_id, task_id, folder, save_target) VALUES (?1, ?2, ?3, NULL)",
                params![
                    crate::codec::encode_id(RawId::new()),
                    crate::codec::encode_id(created.task.as_raw()),
                    "/srv/workspace/duplicate",
                ],
            )
            .expect("the duplicate association must seed");
    }
    let outcome = store
        .insert_attempt_if_current(attempt_premise(
            ActionAttemptId::generate(),
            delegation,
            created,
            assoc,
            &target_path("report.md"),
            OperationKind::Create,
        ))
        .await;
    assert!(
        matches!(
            outcome,
            Err(ActionTechnicalError::StorageUnavailable { .. })
        ),
        "a duplicate association is a violated invariant, got {outcome:?}"
    );
    assert_eq!(task_table_count(&store, "action_attempt"), 0);
    assert!(
        matches!(
            store.load_task(created.task).await,
            Err(TaskTechnicalError::StorageUnavailable { .. })
        ),
        "the task read rejects the duplicate association instead of picking the first"
    );
}

#[tokio::test]
async fn duplicate_attempt_identity_is_a_technical_error() {
    let store = open_memory().await.unwrap();
    let (created, delegation, assoc) = seed_workspace_delegation(&store).await;
    let attempt = ActionAttemptId::generate();
    let premise = attempt_premise(
        attempt,
        delegation,
        created,
        assoc,
        &target_path("report.md"),
        OperationKind::Create,
    );
    assert_eq!(
        store.insert_attempt_if_current(premise.clone()).await,
        Ok(ActionStartOutcome::Started)
    );
    let duplicate = store.insert_attempt_if_current(premise).await;
    assert!(
        matches!(
            duplicate,
            Err(ActionTechnicalError::StorageUnavailable { .. })
        ),
        "attempt identities are single-use, got {duplicate:?}"
    );
    assert_eq!(task_table_count(&store, "action_attempt"), 1);
}

#[tokio::test]
async fn certainty_cas_only_moves_unknown_forward() {
    let store = open_memory().await.unwrap();
    let (created, delegation, assoc) = seed_workspace_delegation(&store).await;
    let attempt = ActionAttemptId::generate();
    assert_eq!(
        store
            .insert_attempt_if_current(attempt_premise(
                attempt,
                delegation,
                created,
                assoc,
                &target_path("report.md"),
                OperationKind::Create,
            ))
            .await,
        Ok(ActionStartOutcome::Started)
    );
    assert_eq!(
        store
            .compare_and_set_certainty(
                attempt,
                ActionCertainty::Unknown,
                ActionCertainty::ConfirmedSuccess,
                EffectGrounds::ObservedAtTarget,
            )
            .await,
        Ok(CertaintyUpdateOutcome::Updated)
    );
    let confirmed = store.load_attempt(attempt).await.unwrap().unwrap();
    assert_eq!(confirmed.certainty, ActionCertainty::ConfirmedSuccess);
    assert_eq!(confirmed.grounds, Some(EffectGrounds::ObservedAtTarget));

    // A confirmed certainty is never rewritten.
    assert_eq!(
        store
            .compare_and_set_certainty(
                attempt,
                ActionCertainty::Unknown,
                ActionCertainty::ConfirmedFailure,
                EffectGrounds::RefusedBeforeEffect,
            )
            .await,
        Ok(CertaintyUpdateOutcome::StaleCurrent {
            current: ActionCertainty::ConfirmedSuccess,
        })
    );
    assert!(
        store
            .compare_and_set_certainty(
                attempt,
                ActionCertainty::ConfirmedSuccess,
                ActionCertainty::ConfirmedSuccess,
                EffectGrounds::ObservedAtTarget,
            )
            .await
            .is_err(),
        "expected must be the started Unknown value"
    );
    assert!(
        store
            .compare_and_set_certainty(
                attempt,
                ActionCertainty::Unknown,
                ActionCertainty::ConfirmedSuccess,
                EffectGrounds::OutcomeUnverified,
            )
            .await
            .is_err(),
        "a confirmed certainty requires its matching ground"
    );
}

#[tokio::test]
async fn certainty_cas_keeps_an_unverifiable_outcome_unknown() {
    let store = open_memory().await.unwrap();
    let (created, delegation, assoc) = seed_workspace_delegation(&store).await;
    let attempt = ActionAttemptId::generate();
    store
        .insert_attempt_if_current(attempt_premise(
            attempt,
            delegation,
            created,
            assoc,
            &target_path("report.md"),
            OperationKind::Edit,
        ))
        .await
        .unwrap();
    assert_eq!(
        store
            .compare_and_set_certainty(
                attempt,
                ActionCertainty::Unknown,
                ActionCertainty::Unknown,
                EffectGrounds::OutcomeUnverified,
            )
            .await,
        Ok(CertaintyUpdateOutcome::Updated)
    );
    let record = store.load_attempt(attempt).await.unwrap().unwrap();
    assert_eq!(record.certainty, ActionCertainty::Unknown);
    assert_eq!(record.grounds, Some(EffectGrounds::OutcomeUnverified));

    assert_eq!(
        store
            .compare_and_set_certainty(
                ActionAttemptId::generate(),
                ActionCertainty::Unknown,
                ActionCertainty::ConfirmedSuccess,
                EffectGrounds::ObservedAtTarget,
            )
            .await,
        Ok(CertaintyUpdateOutcome::MissingAttempt)
    );
}

#[tokio::test]
async fn action_attempt_reads_fail_closed_on_corrupt_rows() {
    let store = open_memory().await.unwrap();
    let (created, delegation, assoc) = seed_workspace_delegation(&store).await;
    let cases = [
        "UPDATE action_attempt SET operation = 'delete';",
        "UPDATE action_attempt SET operation = '';",
        "UPDATE action_attempt SET certainty = 'confirmed';",
        "UPDATE action_attempt SET certainty = 'unknown', grounds = 'observed_at_target';",
        "UPDATE action_attempt SET certainty = 'confirmed_success', grounds = NULL;",
        "UPDATE action_attempt SET grounds = 'agent_reported_success';",
        "UPDATE action_attempt SET real_target = 'relative/path.md';",
        "UPDATE action_attempt SET task_revision = -1;",
        "UPDATE action_attempt SET started_at = 'not a clock';",
    ];
    for (index, sql) in cases.iter().enumerate() {
        let attempt = ActionAttemptId::generate();
        assert_eq!(
            store
                .insert_attempt_if_current(attempt_premise(
                    attempt,
                    delegation,
                    created,
                    assoc,
                    &target_path("report.md"),
                    OperationKind::Read,
                ))
                .await,
            Ok(ActionStartOutcome::Started),
            "corruption case {index} must seed a fresh attempt"
        );
        {
            let guard = match store.conn.lock() {
                Ok(locked) => locked,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard
                .execute_batch(&format!(
                    "{} WHERE attempt_id = '{}';",
                    sql.trim_end_matches(';'),
                    crate::codec::encode_id(attempt.as_raw())
                ))
                .expect("the corruption must apply");
        }
        assert!(
            matches!(
                store.load_attempt(attempt).await,
                Err(ActionTechnicalError::StorageUnavailable { .. })
            ),
            "a corrupt attempt row must fail closed after: {sql}"
        );
    }
}

#[tokio::test]
async fn action_attempt_migration_creates_the_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("action-migration.db");
    {
        let store = Store::open(&path).await.expect("a fresh store must open");
        assert_eq!(read_schema_version(&path), Some(19));
        drop(store);
    }
    {
        let conn = rusqlite::Connection::open(&path).expect("the rewind must open");
        conn.execute_batch(
            "DROP TABLE action_attempt;
             PRAGMA user_version = 18;",
        )
        .expect("the version-18 rewind must apply");
    }
    assert_eq!(read_schema_version(&path), Some(18));
    assert!(
        !table_columns(&path, "action_attempt").contains(&String::from("grounds")),
        "the rewind drops the action attempt table"
    );
    let reopened = Store::open(&path)
        .await
        .expect("the V19 migration must succeed");
    assert_eq!(read_schema_version(&path), Some(19));
    let columns = table_columns(&path, "action_attempt");
    for column in [
        "attempt_id",
        "task_id",
        "task_revision",
        "delegation_id",
        "workspace_assoc_id",
        "real_target",
        "operation",
        "certainty",
        "grounds",
        "started_at",
    ] {
        assert!(
            columns.contains(&String::from(column)),
            "the migrated action_attempt table has {column}"
        );
    }
    drop(reopened);
}
