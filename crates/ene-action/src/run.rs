//! Orchestration of one Workspace-contained filesystem Action.
//!
//! The order is fixed by K-B.1 and AU5: input bounds and path resolution
//! happen before any durable claim (a refused or malformed request never
//! leaves an attempt row), the permission-owned live decision is taken for
//! exactly the resolved target, the attempt insert is the start linearization
//! point inside the repository, and the filesystem effect runs only after
//! [`ActionStartOutcome::Started`], outside every transaction.
//!
//! The observed effect is always returned once the attempt started, even when
//! recording the fact fails technically: an external effect must never be
//! hidden by a storage failure. Recording failure leaves the durable row at
//! `Unknown`, which is exactly what it means.
//!
//! This orchestration never adopts the effect into a Task, never completes
//! the Task, and never re-executes an unknown outcome.

use ene_permission::{
    ActionAuthorizationDecision, ActionEvaluationTracker, ActionKind, ActionUseCandidate,
    CurrentActionPremise, authorize_action_use,
};
use ene_primitive::{RawId, RevisionInner};

use crate::attempt::{
    ActionAttemptId, ActionAttemptRepository, ActionCertainty, ActionStartOutcome,
    ActionTechnicalError, AttemptCommitPremise, CertaintyUpdateOutcome, OperationKind,
};
use crate::filesystem::{MAX_ACTION_FILE_BYTES, ObservedEffect, TargetRejection, WorkspaceRoot};

/// One start-and-execute request under an existing delegation/workspace
/// premise.
///
/// The correlation values are mapped by the composition root from the Task
/// owner's delegation and current workspace association; `root` is the opened
/// current association folder. `content` is the intended bytes for
/// create/edit and is redacted from [`core::fmt::Debug`].
#[derive(Clone, PartialEq, Eq)]
pub struct WorkspaceActionCommand {
    pub delegation: RawId,
    pub task: RawId,
    pub task_revision: RevisionInner,
    pub workspace: RawId,
    pub root: WorkspaceRoot,
    pub operation: OperationKind,
    pub requested_path: String,
    pub content: Option<Vec<u8>>,
}

impl core::fmt::Debug for WorkspaceActionCommand {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("WorkspaceActionCommand")
            .field("delegation", &self.delegation)
            .field("task", &self.task)
            .field("task_revision", &self.task_revision)
            .field("workspace", &self.workspace)
            .field("root", &self.root)
            .field("operation", &self.operation)
            .field("requested_path", &self.requested_path)
            .field(
                "content",
                &self.content.as_ref().map(|bytes| {
                    if bytes.is_empty() {
                        String::from("<0 bytes>")
                    } else {
                        format!("<{} bytes redacted>", bytes.len())
                    }
                }),
            )
            .finish()
    }
}

/// Why one request never started. Every variant means zero durable writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionNotStarted {
    /// The delegation/task/workspace premise is no longer current.
    StalePremise,
    /// The requested path was refused before any claim.
    Rejected(TargetRejection),
    /// Create/edit arrived without content.
    MissingContent,
    /// List/read arrived with content.
    ContentNotAllowed,
    /// The payload exceeds [`MAX_ACTION_FILE_BYTES`].
    ContentTooLarge,
    /// The permission-owned candidate disagrees with the current premise; the
    /// caller reloads current state and rebuilds the request.
    NeedsRevalidation,
    /// The evaluation id was unknown, already consumed, or bound to a
    /// different candidate fingerprint.
    EvaluationConsumed,
}

/// The result of one start-and-execute request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionRunOutcome {
    /// The attempt started and its effect was observed.
    Completed {
        attempt: ActionAttemptId,
        /// What the executor observed; the [`core::fmt::Debug`] rendering of
        /// [`ObservedEffect`] redacts any read output.
        effect: ObservedEffect,
        /// Whether the observation became durable. `false` means the attempt
        /// row still reads `Unknown` (the effect happened and is reported,
        /// but the fact could not be stored).
        fact_recorded: bool,
    },
    /// Nothing was claimed and nothing was executed.
    NotStarted(ActionNotStarted),
}

/// Starts and executes one Workspace-contained filesystem action.
///
/// Input bounds and path resolution are checked first, so a refused request
/// leaves no attempt row. The resolved target then goes through the
/// permission-owned [`authorize_action_use`], whose single-use evaluation is
/// consumed before the durable claim; the repository's claim decides start
/// versus [`ActionNotStarted::StalePremise`], and only `Started` executes. The
/// effect is observed from the executor's own operation (read-back for
/// writes), never from an agent self-report, and its certainty is recorded
/// with a compare-and-set from `Unknown`.
pub async fn orchestrate_workspace_action(
    repository: &impl ActionAttemptRepository,
    tracker: &mut ActionEvaluationTracker,
    command: WorkspaceActionCommand,
) -> Result<ActionRunOutcome, ActionTechnicalError> {
    if let Some(not_started) = input_check(&command) {
        return Ok(ActionRunOutcome::NotStarted(not_started));
    }
    let target = match command
        .root
        .resolve(&command.requested_path, command.operation)
    {
        Ok(target) => target,
        Err(rejection) => {
            return Ok(ActionRunOutcome::NotStarted(ActionNotStarted::Rejected(
                rejection,
            )));
        }
    };
    // The live decision binds this operation to the exact resolved target and
    // the current premise; only its single-use evaluation may start the
    // attempt.
    let candidate = ActionUseCandidate {
        delegation: command.delegation,
        task: command.task,
        task_revision: command.task_revision,
        workspace: command.workspace,
        operation: action_kind(command.operation),
        resolved_target: target.as_path().to_owned(),
    };
    let current = CurrentActionPremise {
        delegation: command.delegation,
        task: command.task,
        task_revision: command.task_revision,
        workspace: command.workspace,
    };
    let evaluation = match authorize_action_use(&candidate, &current, tracker) {
        ActionAuthorizationDecision::AllowForThisUse(evaluation) => evaluation,
        ActionAuthorizationDecision::NeedsRevalidation => {
            return Ok(ActionRunOutcome::NotStarted(
                ActionNotStarted::NeedsRevalidation,
            ));
        }
    };
    if !tracker.consume(&evaluation, &candidate) {
        return Ok(ActionRunOutcome::NotStarted(
            ActionNotStarted::EvaluationConsumed,
        ));
    }
    let attempt = ActionAttemptId::generate();
    let outcome = repository
        .insert_attempt_if_current(AttemptCommitPremise {
            attempt,
            delegation: command.delegation,
            task: command.task,
            task_revision: command.task_revision,
            workspace: command.workspace,
            real_target: target.clone(),
            operation: command.operation,
            relied_evaluation: evaluation,
        })
        .await?;
    if outcome == ActionStartOutcome::StalePremise {
        return Ok(ActionRunOutcome::NotStarted(ActionNotStarted::StalePremise));
    }
    // The attempt is durable: execute outside every transaction, exactly the
    // premise that was compared, then observe the effect.
    let effect = command
        .root
        .execute(&target, command.operation, command.content.as_deref());
    let fact_recorded = repository
        .compare_and_set_certainty(
            attempt,
            ActionCertainty::Unknown,
            effect.certainty,
            effect.grounds,
        )
        .await
        .map(|outcome| outcome == CertaintyUpdateOutcome::Updated)
        .unwrap_or(false);
    Ok(ActionRunOutcome::Completed {
        attempt,
        effect,
        fact_recorded,
    })
}

/// Total mapping from the Action-owned operation kind to the permission-owned
/// capability vocabulary; the closed worlds must grow together.
fn action_kind(operation: OperationKind) -> ActionKind {
    match operation {
        OperationKind::List => ActionKind::List,
        OperationKind::Read => ActionKind::Read,
        OperationKind::Create => ActionKind::Create,
        OperationKind::Edit => ActionKind::Edit,
    }
}

fn input_check(command: &WorkspaceActionCommand) -> Option<ActionNotStarted> {
    match command.operation {
        OperationKind::List | OperationKind::Read => command
            .content
            .is_some()
            .then_some(ActionNotStarted::ContentNotAllowed),
        OperationKind::Create | OperationKind::Edit => match &command.content {
            None => Some(ActionNotStarted::MissingContent),
            Some(content) if content.len() > MAX_ACTION_FILE_BYTES => {
                Some(ActionNotStarted::ContentTooLarge)
            }
            Some(_) => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tempfile::tempdir;

    use super::{
        ActionNotStarted, ActionRunOutcome, WorkspaceActionCommand, orchestrate_workspace_action,
    };
    use crate::attempt::{
        ActionAttemptId, ActionAttemptRecord, ActionCertainty, ActionStartOutcome,
        ActionTechnicalError, AttemptCommitPremise, CertaintyUpdateOutcome, EffectGrounds,
        OperationKind, RealTargetRef,
    };
    use crate::filesystem::{TargetRejection, WorkspaceRoot};

    /// Captures every claim and answers configured domain outcomes.
    #[derive(Default)]
    struct FakeAttempts {
        starts: Mutex<Vec<AttemptCommitPremise>>,
        updates: Mutex<Vec<(ActionAttemptId, ActionCertainty, EffectGrounds)>>,
        start_outcome: Mutex<Option<ActionStartOutcome>>,
        update_outcome: Mutex<Option<CertaintyUpdateOutcome>>,
    }

    impl FakeAttempts {
        fn starts(&self) -> Vec<AttemptCommitPremise> {
            self.starts.lock().expect("start capture lock").clone()
        }

        fn updates(&self) -> Vec<(ActionAttemptId, ActionCertainty, EffectGrounds)> {
            self.updates.lock().expect("update capture lock").clone()
        }

        fn set_start(&self, outcome: ActionStartOutcome) {
            *self.start_outcome.lock().expect("start outcome lock") = Some(outcome);
        }

        fn set_update(&self, outcome: CertaintyUpdateOutcome) {
            *self.update_outcome.lock().expect("update outcome lock") = Some(outcome);
        }
    }

    impl crate::attempt::ActionAttemptRepository for FakeAttempts {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn insert_attempt_if_current(
            &self,
            premise: AttemptCommitPremise,
        ) -> Result<ActionStartOutcome, ActionTechnicalError> {
            self.starts
                .lock()
                .expect("start capture lock")
                .push(premise);
            Ok(self
                .start_outcome
                .lock()
                .expect("start outcome lock")
                .unwrap_or(ActionStartOutcome::Started))
        }

        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn compare_and_set_certainty(
            &self,
            attempt: ActionAttemptId,
            _expected: ActionCertainty,
            new: ActionCertainty,
            grounds: EffectGrounds,
        ) -> Result<CertaintyUpdateOutcome, ActionTechnicalError> {
            self.updates
                .lock()
                .expect("update capture lock")
                .push((attempt, new, grounds));
            Ok(self
                .update_outcome
                .lock()
                .expect("update outcome lock")
                .unwrap_or(CertaintyUpdateOutcome::Updated))
        }

        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn load_attempt(
            &self,
            _attempt: ActionAttemptId,
        ) -> Result<Option<ActionAttemptRecord>, ActionTechnicalError> {
            Ok(None)
        }
    }

    fn workspace() -> (tempfile::TempDir, WorkspaceRoot) {
        let directory = tempdir().expect("test workspace directory");
        let root = WorkspaceRoot::open(&directory.path().to_string_lossy())
            .expect("an existing directory opens");
        (directory, root)
    }

    fn command(
        root: WorkspaceRoot,
        operation: OperationKind,
        path: &str,
    ) -> WorkspaceActionCommand {
        WorkspaceActionCommand {
            delegation: ene_primitive::RawId::new(),
            task: ene_primitive::RawId::new(),
            task_revision: ene_primitive::RevisionInner::from_u64(3),
            workspace: ene_primitive::RawId::new(),
            root,
            operation,
            requested_path: String::from(path),
            content: match operation {
                OperationKind::List | OperationKind::Read => None,
                OperationKind::Create | OperationKind::Edit => Some(Vec::new()),
            },
        }
    }

    async fn run(
        attempts: &FakeAttempts,
        command: WorkspaceActionCommand,
    ) -> Result<ActionRunOutcome, ActionTechnicalError> {
        let mut tracker = ene_permission::ActionEvaluationTracker::new();
        orchestrate_workspace_action(attempts, &mut tracker, command).await
    }

    #[tokio::test]
    async fn create_starts_then_writes_and_records_the_observation() {
        let (directory, root) = workspace();
        let attempts = FakeAttempts::default();
        let mut command = command(root, OperationKind::Create, "report.md");
        command.content = Some(b"# report".to_vec());
        let outcome = run(&attempts, command)
            .await
            .expect("domain outcomes are not technical errors");
        let ActionRunOutcome::Completed {
            effect,
            fact_recorded,
            ..
        } = outcome
        else {
            panic!("the configured claim started");
        };
        assert_eq!(effect.certainty, ActionCertainty::ConfirmedSuccess);
        assert_eq!(effect.grounds, EffectGrounds::ObservedAtTarget);
        assert!(fact_recorded);
        assert_eq!(
            std::fs::read(directory.path().join("report.md")).expect("created file"),
            b"# report"
        );
        let starts = attempts.starts();
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].operation, OperationKind::Create);
        assert!(starts[0].real_target.as_path().ends_with("report.md"));
        assert_ne!(
            starts[0].relied_evaluation.as_raw(),
            ene_primitive::RawId::new(),
            "the claim carries the permission-owned evaluation id"
        );
        let updates = attempts.updates();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].1, ActionCertainty::ConfirmedSuccess);
        assert_eq!(updates[0].2, EffectGrounds::ObservedAtTarget);
    }

    #[tokio::test]
    async fn list_starts_then_observes_the_directory() {
        let (directory, root) = workspace();
        std::fs::write(directory.path().join("a.txt"), b"a").expect("fixture write");
        let attempts = FakeAttempts::default();
        let outcome = run(&attempts, command(root, OperationKind::List, ""))
            .await
            .expect("a listing answers a domain outcome");
        let ActionRunOutcome::Completed {
            effect,
            fact_recorded,
            ..
        } = outcome
        else {
            panic!("the listing must complete on the started attempt");
        };
        assert!(fact_recorded);
        assert_eq!(effect.certainty, ActionCertainty::ConfirmedSuccess);
        assert!(matches!(
            effect.output,
            Some(crate::filesystem::ActionOutput::Listing(_))
        ));
    }

    #[tokio::test]
    async fn stale_premise_writes_nothing_and_executes_nothing() {
        let (directory, root) = workspace();
        let attempts = FakeAttempts::default();
        attempts.set_start(ActionStartOutcome::StalePremise);
        let mut command = command(root, OperationKind::Create, "report.md");
        command.content = Some(b"# report".to_vec());
        let outcome = run(&attempts, command)
            .await
            .expect("stale is a domain outcome");
        assert_eq!(
            outcome,
            ActionRunOutcome::NotStarted(ActionNotStarted::StalePremise)
        );
        assert!(!directory.path().join("report.md").exists());
        assert!(attempts.updates().is_empty());
    }

    #[tokio::test]
    async fn escaped_paths_never_claim_and_never_execute() {
        let (_directory, root) = workspace();
        let attempts = FakeAttempts::default();
        let mut command = command(root, OperationKind::Read, "../escape.txt");
        command.content = None;
        let outcome = run(&attempts, command)
            .await
            .expect("a rejection is a domain outcome");
        assert_eq!(
            outcome,
            ActionRunOutcome::NotStarted(ActionNotStarted::Rejected(
                TargetRejection::MalformedPath
            ))
        );
        assert!(attempts.starts().is_empty(), "a rejection never claims");
        assert!(attempts.updates().is_empty());
    }

    #[tokio::test]
    async fn input_bounds_are_refused_before_any_claim() {
        let (_directory, root) = workspace();
        let attempts = FakeAttempts::default();

        let read_with_content = WorkspaceActionCommand {
            content: Some(b"unexpected".to_vec()),
            ..command(root.clone(), OperationKind::Read, "input.txt")
        };
        assert_eq!(
            run(&attempts, read_with_content)
                .await
                .expect("domain outcome"),
            ActionRunOutcome::NotStarted(ActionNotStarted::ContentNotAllowed)
        );

        let mut write_without_content = command(root.clone(), OperationKind::Edit, "input.txt");
        write_without_content.content = None;
        assert_eq!(
            run(&attempts, write_without_content)
                .await
                .expect("domain outcome"),
            ActionRunOutcome::NotStarted(ActionNotStarted::MissingContent)
        );

        let mut oversized = command(root, OperationKind::Create, "report.md");
        oversized.content = Some(vec![b'x'; crate::filesystem::MAX_ACTION_FILE_BYTES + 1]);
        assert_eq!(
            run(&attempts, oversized).await.expect("domain outcome"),
            ActionRunOutcome::NotStarted(ActionNotStarted::ContentTooLarge)
        );
        assert!(attempts.starts().is_empty());
    }

    #[tokio::test]
    async fn the_effect_is_returned_even_when_recording_it_fails() {
        let (directory, root) = workspace();
        let attempts = FakeAttempts::default();
        attempts.set_update(CertaintyUpdateOutcome::MissingAttempt);
        let mut command = command(root, OperationKind::Create, "report.md");
        command.content = Some(b"# report".to_vec());
        let outcome = run(&attempts, command)
            .await
            .expect("a missing attempt row is a domain outcome");
        let ActionRunOutcome::Completed {
            effect,
            fact_recorded,
            ..
        } = outcome
        else {
            panic!("the configured claim started");
        };
        assert_eq!(effect.certainty, ActionCertainty::ConfirmedSuccess);
        assert!(!fact_recorded, "the durable row keeps its Unknown value");
        assert_eq!(
            std::fs::read(directory.path().join("report.md")).expect("created file"),
            b"# report",
            "the external effect is never hidden by the recording failure"
        );
    }

    #[tokio::test]
    async fn command_debug_redacts_content() {
        let (_directory, root) = workspace();
        let mut command = command(root, OperationKind::Create, "report.md");
        command.content = Some(b"secret file body".to_vec());
        let rendered = format!("{command:?}");
        assert!(!rendered.contains("secret file body"));
        assert!(rendered.contains("bytes redacted"));
    }

    #[test]
    fn real_target_debug_never_loses_the_path() {
        let target = RealTargetRef::from_canonical_path(String::from("/tmp/workspace/report.md"));
        assert_eq!(
            format!("{target:?}"),
            "RealTargetRef(\"/tmp/workspace/report.md\")"
        );
    }
}
