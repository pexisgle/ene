//! Host adapter from the Task-owned delegation/workspace correspondence to
//! the Action-owned filesystem boundary.
//!
//! Composition only: this module loads the delegation correspondence and the
//! current Task unit, opens the current workspace association's folder, maps
//! the identities into the Action owner's opaque premise, and mirrors the
//! Action outcome without adding behavior. The authoritative premise compare
//! happens inside the Action repository's start transaction, and path
//! containment plus execution happen inside `ene-action`.
//!
//! A precheck here (missing row, moved revision, missing workspace) only
//! shapes the caller-facing domain answer; it is never the concurrency
//! guarantee.

use ene_action::{
    ActionAttemptId, ActionNotStarted, ActionRunOutcome, ActionTechnicalError, ObservedEffect,
    OperationKind, WorkspaceActionCommand, WorkspaceRoot, WorkspaceRootError,
    orchestrate_workspace_action,
};
use ene_primitive::RevisionInner;
use ene_store::Store;
use ene_task::{DelegationId, TaskId, TaskRef, TaskRepository, TaskTechnicalError};

/// The caller-facing outcome of one Host filesystem action request.
///
/// Every variant is a domain answer: no provider-style technical error is
/// folded in, and no variant claims Task completion or result adoption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceActionHostOutcome {
    /// The attempt started and its effect was observed.
    Completed {
        attempt: ActionAttemptId,
        effect: ObservedEffect,
        /// `false` means the observation could not be stored; the durable row
        /// keeps its `Unknown` value and the effect is still reported.
        fact_recorded: bool,
    },
    /// The Action owner refused before or at the start claim.
    NotStarted(ActionNotStarted),
    /// The delegation correspondence does not exist.
    MissingDelegation { delegation: DelegationId },
    /// The delegated Task has no durable state.
    MissingTask { task: TaskId },
    /// The relied Task revision moved; nothing was claimed or executed.
    StaleTaskRevision { current: TaskRef },
    /// The Task has no current workspace association.
    MissingWorkspace { task: TaskId },
    /// The association exists but its folder is currently unusable.
    WorkspaceUnavailable { task: TaskId },
}

/// Technical failure of one Host filesystem action request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceActionHostError {
    #[error("task storage unavailable: {reason}")]
    TaskUnavailable { reason: String },
    #[error("action storage unavailable: {reason}")]
    ActionUnavailable { reason: String },
}

/// Runs one Workspace-contained filesystem action under an existing
/// delegation.
///
/// The current Task revision must still equal the delegation's relied
/// revision, and the current workspace association is the authority (the
/// delegation's copied scope is provenance, compared again inside the start
/// transaction). The folder is opened at request time; a vanished or
/// non-directory folder refuses without an attempt.
///
/// The association's optional `save_target` is not consulted here: it gates
/// the final save confirmation, which the Work owner decides in a later
/// slice. This request only constrains the path to the workspace folder.
pub async fn run_workspace_action(
    store: &Store,
    delegation: DelegationId,
    operation: OperationKind,
    requested_path: String,
    content: Option<Vec<u8>>,
) -> Result<WorkspaceActionHostOutcome, WorkspaceActionHostError> {
    let Some(correspondence) = store
        .load_delegation(delegation)
        .await
        .map_err(task_unavailable)?
    else {
        return Ok(WorkspaceActionHostOutcome::MissingDelegation { delegation });
    };
    let task = correspondence.task.task;
    let Some(record) = store.load_task(task).await.map_err(task_unavailable)? else {
        return Ok(WorkspaceActionHostOutcome::MissingTask { task });
    };
    if record.task.reference != correspondence.task {
        return Ok(WorkspaceActionHostOutcome::StaleTaskRevision {
            current: record.task.reference,
        });
    }
    let Some(workspace) = record.workspace else {
        return Ok(WorkspaceActionHostOutcome::MissingWorkspace { task });
    };
    let root = match WorkspaceRoot::open(&workspace.folder.path) {
        Ok(root) => root,
        Err(WorkspaceRootError::Unavailable | WorkspaceRootError::NotADirectory) => {
            return Ok(WorkspaceActionHostOutcome::WorkspaceUnavailable { task });
        }
    };
    let command = WorkspaceActionCommand {
        delegation: delegation.as_raw(),
        task: task.as_raw(),
        task_revision: RevisionInner::from_u64(correspondence.task.revision.as_u64()),
        workspace: workspace.assoc.as_raw(),
        root,
        operation,
        requested_path,
        content,
    };
    match orchestrate_workspace_action(store, command)
        .await
        .map_err(action_unavailable)?
    {
        ActionRunOutcome::Completed {
            attempt,
            effect,
            fact_recorded,
        } => Ok(WorkspaceActionHostOutcome::Completed {
            attempt,
            effect,
            fact_recorded,
        }),
        ActionRunOutcome::NotStarted(reason) => Ok(WorkspaceActionHostOutcome::NotStarted(reason)),
    }
}

fn task_unavailable(error: TaskTechnicalError) -> WorkspaceActionHostError {
    match error {
        TaskTechnicalError::StorageUnavailable { reason } => {
            WorkspaceActionHostError::TaskUnavailable { reason }
        }
    }
}

fn action_unavailable(error: ActionTechnicalError) -> WorkspaceActionHostError {
    match error {
        ActionTechnicalError::StorageUnavailable { reason } => {
            WorkspaceActionHostError::ActionUnavailable { reason }
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{WorkspaceActionHostOutcome, run_workspace_action};
    use ene_action::{
        ActionAttemptRepository, ActionCertainty, EffectGrounds, OperationKind, TargetRejection,
    };
    use ene_primitive::{RawId, WallClockWithTz};
    use ene_store::Store;
    use ene_task::{
        AssigneeRef, DelegatedWorkspace, DelegationCreationPremise, DelegationId,
        DelegationOutcome, DelegationScope, TaskAgentEphemeralId, TaskCommitOutcome,
        TaskCommitPremise, TaskContextEntryId, TaskContextOrigin, TaskContextOriginKind,
        TaskCreationPremise, TaskId, TaskPurpose, TaskRef, TaskRepository, WorkspaceAssocId,
        WorkspaceAssociationPremise, WorkspaceFolderRef, WorkspaceNeedRef,
    };

    struct Host {
        _store_dir: tempfile::TempDir,
        store: Store,
        workspace: tempfile::TempDir,
        task: TaskRef,
        delegation: DelegationId,
    }

    async fn host() -> Host {
        let store_dir = tempdir().expect("store directory");
        let store = Store::open(&store_dir.path().join("action.db"))
            .await
            .expect("store opens");
        let workspace = tempdir().expect("workspace directory");
        let assoc = WorkspaceAssocId::generate();
        let folder = WorkspaceFolderRef {
            path: workspace.path().to_string_lossy().into_owned(),
        };
        let task = store
            .create_task(TaskCreationPremise {
                task: TaskId::generate(),
                purpose: TaskPurpose {
                    text: String::from("read the input and write a report"),
                },
                entry: TaskContextEntryId::generate(),
                origin: TaskContextOrigin {
                    kind: TaskContextOriginKind::OwnerConversation,
                    source: RawId::new(),
                },
                acquired_at: WallClockWithTz::now(),
                assignee: AssigneeRef {
                    companion: RawId::new(),
                },
                workspace: Some(WorkspaceAssociationPremise {
                    assoc,
                    need: WorkspaceNeedRef {
                        folder: folder.clone(),
                        save_target: None,
                    },
                }),
            })
            .await
            .expect("task creation commits");
        let delegation = DelegationId::generate();
        let created = store
            .create_delegation(DelegationCreationPremise {
                delegation,
                task,
                agent: TaskAgentEphemeralId::generate(),
                scope_copy: DelegationScope {
                    workspace: Some(DelegatedWorkspace {
                        assoc,
                        folder,
                        save_target: None,
                    }),
                },
            })
            .await
            .expect("delegation creation commits");
        assert!(matches!(created, DelegationOutcome::Delegated(_)));
        Host {
            _store_dir: store_dir,
            store,
            workspace,
            task,
            delegation,
        }
    }

    #[tokio::test]
    async fn reads_then_creates_then_edits_end_to_end() {
        let host = host().await;
        std::fs::write(host.workspace.path().join("input.txt"), b"notes").expect("input fixture");

        let read = run_workspace_action(
            &host.store,
            host.delegation,
            OperationKind::Read,
            String::from("input.txt"),
            None,
        )
        .await
        .expect("a read answers a domain outcome");
        let WorkspaceActionHostOutcome::Completed {
            effect,
            fact_recorded,
            attempt,
        } = read
        else {
            panic!("the read must complete, got {read:?}");
        };
        assert_eq!(effect.certainty, ActionCertainty::ConfirmedSuccess);
        assert_eq!(effect.grounds, EffectGrounds::ObservedAtTarget);
        assert_eq!(effect.output.as_deref(), Some(&b"notes"[..]));
        assert!(fact_recorded);
        let record = host
            .store
            .load_attempt(attempt)
            .await
            .unwrap()
            .expect("the read attempt is durable");
        assert_eq!(record.task, host.task.task.as_raw());
        assert_eq!(record.operation, OperationKind::Read);

        let created = run_workspace_action(
            &host.store,
            host.delegation,
            OperationKind::Create,
            String::from("report.md"),
            Some(b"# report".to_vec()),
        )
        .await
        .expect("a create answers a domain outcome");
        let WorkspaceActionHostOutcome::Completed {
            attempt: create_attempt,
            fact_recorded: create_recorded,
            ..
        } = created
        else {
            panic!("the create must complete, got {created:?}");
        };
        assert!(create_recorded);
        assert_eq!(
            std::fs::read(host.workspace.path().join("report.md")).expect("report exists"),
            b"# report"
        );

        let edited = run_workspace_action(
            &host.store,
            host.delegation,
            OperationKind::Edit,
            String::from("report.md"),
            Some(b"# edited report".to_vec()),
        )
        .await
        .expect("an edit answers a domain outcome");
        let WorkspaceActionHostOutcome::Completed {
            attempt: edit_attempt,
            fact_recorded: edit_recorded,
            ..
        } = edited
        else {
            panic!("the edit must complete, got {edited:?}");
        };
        assert!(edit_recorded);
        assert_eq!(
            std::fs::read(host.workspace.path().join("report.md")).expect("report exists"),
            b"# edited report"
        );

        // Each executed action kept its own attempted identity and correlation.
        for expected in [attempt, create_attempt, edit_attempt] {
            let record = host
                .store
                .load_attempt(expected)
                .await
                .unwrap()
                .expect("every executed action kept its attempt");
            assert_eq!(record.attempt, expected);
            assert_eq!(record.task, host.task.task.as_raw());
            assert_eq!(record.delegation, host.delegation.as_raw());
            assert_eq!(record.certainty, ActionCertainty::ConfirmedSuccess);
        }
        assert_ne!(attempt, create_attempt);
        assert_ne!(create_attempt, edit_attempt);
    }

    #[tokio::test]
    async fn a_missing_read_is_refused_before_the_claim() {
        let host = host().await;
        let read = run_workspace_action(
            &host.store,
            host.delegation,
            OperationKind::Read,
            String::from("missing.txt"),
            None,
        )
        .await
        .expect("a missing file is a domain outcome");
        assert_eq!(
            read,
            WorkspaceActionHostOutcome::NotStarted(ene_action::ActionNotStarted::Rejected(
                TargetRejection::MissingTarget
            )),
            "a path that cannot resolve never becomes a claimed failure"
        );
    }

    #[tokio::test]
    async fn escaped_or_absent_targets_never_claim() {
        let host = host().await;
        for (path, expected) in [
            ("../escape.txt", TargetRejection::MalformedPath),
            ("/etc/passwd", TargetRejection::MalformedPath),
            ("missing.txt", TargetRejection::MissingTarget),
        ] {
            let outcome = run_workspace_action(
                &host.store,
                host.delegation,
                OperationKind::Read,
                String::from(path),
                None,
            )
            .await
            .expect("a rejection is a domain outcome");
            assert_eq!(
                outcome,
                WorkspaceActionHostOutcome::NotStarted(ene_action::ActionNotStarted::Rejected(
                    expected
                )),
                "path {path} must be refused before any claim"
            );
        }
    }

    #[tokio::test]
    async fn steering_between_load_and_start_is_stale_with_no_effect() {
        let host = host().await;
        let advanced = host
            .store
            .forward_steering(TaskCommitPremise {
                expected: host.task,
                new_purpose: Some(ene_task::TaskPurposeAdoptionPremise {
                    purpose: TaskPurpose {
                        text: String::from("new direction"),
                    },
                    origin: TaskContextOrigin {
                        kind: TaskContextOriginKind::OwnerConversation,
                        source: RawId::new(),
                    },
                    acquired_at: WallClockWithTz::now(),
                }),
                adopted_purpose_entry: TaskContextEntryId::generate(),
                adopted_instruction: None,
            })
            .await
            .unwrap();
        assert!(matches!(advanced, TaskCommitOutcome::CommittedAs(_)));
        let outcome = run_workspace_action(
            &host.store,
            host.delegation,
            OperationKind::Create,
            String::from("report.md"),
            Some(b"# report".to_vec()),
        )
        .await
        .expect("a stale revision is a domain outcome");
        assert!(matches!(
            outcome,
            WorkspaceActionHostOutcome::StaleTaskRevision { .. }
        ));
        assert!(!host.workspace.path().join("report.md").exists());
    }

    #[tokio::test]
    async fn missing_delegation_workspace_and_folder_are_distinct_domain_answers() {
        let host = host().await;
        let missing = run_workspace_action(
            &host.store,
            DelegationId::generate(),
            OperationKind::Read,
            String::from("input.txt"),
            None,
        )
        .await
        .expect("a missing delegation is a domain outcome");
        assert!(matches!(
            missing,
            WorkspaceActionHostOutcome::MissingDelegation { .. }
        ));

        // A Task with no workspace association at all.
        let no_workspace_task = host
            .store
            .create_task(TaskCreationPremise {
                task: TaskId::generate(),
                purpose: TaskPurpose {
                    text: String::from("no workspace"),
                },
                entry: TaskContextEntryId::generate(),
                origin: TaskContextOrigin {
                    kind: TaskContextOriginKind::OwnerConversation,
                    source: RawId::new(),
                },
                acquired_at: WallClockWithTz::now(),
                assignee: AssigneeRef {
                    companion: RawId::new(),
                },
                workspace: None,
            })
            .await
            .unwrap();
        let no_workspace_delegation = DelegationId::generate();
        host.store
            .create_delegation(DelegationCreationPremise {
                delegation: no_workspace_delegation,
                task: no_workspace_task,
                agent: TaskAgentEphemeralId::generate(),
                scope_copy: DelegationScope { workspace: None },
            })
            .await
            .unwrap();
        let no_workspace = run_workspace_action(
            &host.store,
            no_workspace_delegation,
            OperationKind::Read,
            String::from("input.txt"),
            None,
        )
        .await
        .expect("a missing workspace is a domain outcome");
        assert!(matches!(
            no_workspace,
            WorkspaceActionHostOutcome::MissingWorkspace { .. }
        ));

        // The association exists but its folder was removed.
        drop(host.workspace);
        let unavailable = run_workspace_action(
            &host.store,
            host.delegation,
            OperationKind::Read,
            String::from("input.txt"),
            None,
        )
        .await
        .expect("an unavailable folder is a domain outcome");
        assert!(
            matches!(
                unavailable,
                WorkspaceActionHostOutcome::WorkspaceUnavailable { .. }
            ),
            "the removed folder refuses without a claim: {unavailable:?}"
        );
    }
}
