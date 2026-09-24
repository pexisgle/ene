use ene_permission::{
    ActionAuthorizationDecision, ActionDenyCode, ActionEvaluationTracker, ActionKind,
    ActionUseCandidate, CurrentActionPremise, authorize_action_use,
};
use ene_primitive::{RawId, RevisionInner};

use crate::attempt::{
    ActionAttemptId, ActionAttemptRepository, ActionCertainty, ActionStartOutcome,
    ActionTechnicalError, AttemptCommitPremise, CertaintyUpdateOutcome, OperationKind,
    RealTargetRef,
};
use crate::filesystem::{ObservedEffect, TargetRejection, WorkspaceRoot};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionNotStarted {
    StalePremise,
    TaskTerminal,
    ExecutionSealed,
    DataUseHeld,
    Rejected(TargetRejection),
    MissingContent,
    ContentNotAllowed,
    Denied(ActionDenyCode),
    NeedsRevalidation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionRunOutcome {
    Completed {
        attempt: ActionAttemptId,
        effect: ObservedEffect,
        fact_recorded: bool,
    },
    NotStarted(ActionNotStarted),
}

pub enum ActionClaimOutcome {
    Started(StartedWorkspaceAction),
    NotStarted(ActionNotStarted),
}

pub struct StartedWorkspaceAction {
    attempt: ActionAttemptId,
    root: WorkspaceRoot,
    target: RealTargetRef,
    operation: OperationKind,
    content: Option<Vec<u8>>,
}

impl StartedWorkspaceAction {
    #[must_use]
    pub fn attempt(&self) -> ActionAttemptId {
        self.attempt
    }

    #[must_use]
    pub fn root(&self) -> &WorkspaceRoot {
        &self.root
    }

    #[must_use]
    pub fn target(&self) -> &RealTargetRef {
        &self.target
    }

    #[must_use]
    pub fn operation(&self) -> OperationKind {
        self.operation
    }

    #[must_use]
    pub fn content(&self) -> Option<&[u8]> {
        self.content.as_deref()
    }

    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn for_test(
        root: WorkspaceRoot,
        target: RealTargetRef,
        operation: OperationKind,
        content: Option<Vec<u8>>,
    ) -> Self {
        Self {
            attempt: ActionAttemptId::generate(),
            root,
            target,
            operation,
            content,
        }
    }
}

pub async fn orchestrate_workspace_action(
    repository: &impl ActionAttemptRepository,
    tracker: &mut ActionEvaluationTracker,
    command: WorkspaceActionCommand,
) -> Result<ActionRunOutcome, ActionTechnicalError> {
    let started = match start_workspace_action(repository, tracker, command).await? {
        ActionClaimOutcome::Started(started) => started,
        ActionClaimOutcome::NotStarted(reason) => {
            return Ok(ActionRunOutcome::NotStarted(reason));
        }
    };
    let attempt = started.attempt;
    let effect = started.root.execute(
        &started.target,
        started.operation,
        started.content.as_deref(),
    );
    settle_workspace_effect(repository, attempt, effect).await
}

pub async fn start_workspace_action(
    repository: &impl ActionAttemptRepository,
    tracker: &mut ActionEvaluationTracker,
    command: WorkspaceActionCommand,
) -> Result<ActionClaimOutcome, ActionTechnicalError> {
    if let Some(not_started) = input_check(&command) {
        return Ok(ActionClaimOutcome::NotStarted(not_started));
    }
    let target = match command
        .root
        .resolve(&command.requested_path, command.operation)
    {
        Ok(target) => target,
        Err(rejection) => {
            return Ok(ActionClaimOutcome::NotStarted(ActionNotStarted::Rejected(
                rejection,
            )));
        }
    };
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
        ActionAuthorizationDecision::Deny(code) => {
            return Ok(ActionClaimOutcome::NotStarted(ActionNotStarted::Denied(
                code,
            )));
        }
        ActionAuthorizationDecision::NeedsRevalidation => {
            return Ok(ActionClaimOutcome::NotStarted(
                ActionNotStarted::NeedsRevalidation,
            ));
        }
    };
    tracker.consume(&evaluation, &candidate);
    let premise = attempt_premise(&command, &target, evaluation.as_raw());
    let attempt = premise.attempt;
    match repository.insert_attempt_if_current(premise).await? {
        ActionStartOutcome::Started => Ok(ActionClaimOutcome::Started(StartedWorkspaceAction {
            attempt,
            root: command.root,
            target,
            operation: command.operation,
            content: command.content,
        })),
        ActionStartOutcome::StalePremise => Ok(ActionClaimOutcome::NotStarted(
            ActionNotStarted::StalePremise,
        )),
        ActionStartOutcome::TaskTerminal => Ok(ActionClaimOutcome::NotStarted(
            ActionNotStarted::TaskTerminal,
        )),
        ActionStartOutcome::ExecutionSealed => Ok(ActionClaimOutcome::NotStarted(
            ActionNotStarted::ExecutionSealed,
        )),
        ActionStartOutcome::HeldForErasure => Ok(ActionClaimOutcome::NotStarted(
            ActionNotStarted::DataUseHeld,
        )),
    }
}

pub async fn settle_workspace_effect(
    repository: &impl ActionAttemptRepository,
    attempt: ActionAttemptId,
    effect: ObservedEffect,
) -> Result<ActionRunOutcome, ActionTechnicalError> {
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

fn attempt_premise(
    command: &WorkspaceActionCommand,
    target: &RealTargetRef,
    evaluation: RawId,
) -> AttemptCommitPremise {
    AttemptCommitPremise {
        attempt: ActionAttemptId::generate(),
        delegation: command.delegation,
        task: command.task,
        task_revision: command.task_revision,
        workspace: command.workspace,
        real_target: target.clone(),
        operation: command.operation,
        relied_evaluation: evaluation,
    }
}

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
        OperationKind::Create | OperationKind::Edit => command
            .content
            .is_none()
            .then_some(ActionNotStarted::MissingContent),
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Mutex};

    use tempfile::tempdir;

    use super::{
        ActionNotStarted, ActionRunOutcome, WorkspaceActionCommand, orchestrate_workspace_action,
    };
    use crate::attempt::{
        ActionAttemptId, ActionAttemptRecord, ActionCertainty, ActionStartOutcome,
        ActionTechnicalError, AttemptCommitPremise, CertaintyUpdateOutcome, EffectGrounds,
        OperationKind, certainty_grounds_pair_is_valid,
    };
    use crate::filesystem::{ActionOutput, ObservedEffect, TargetRejection, WorkspaceRoot};
    use ene_permission::ActionEvaluationTracker;

    #[derive(Default)]
    struct FakeAttempts {
        starts: Mutex<Vec<AttemptCommitPremise>>,
        updates: Mutex<
            Vec<(
                ActionAttemptId,
                ActionCertainty,
                ActionCertainty,
                EffectGrounds,
            )>,
        >,
        start_outcome: Mutex<Option<ActionStartOutcome>>,
        update_outcome: Mutex<Option<CertaintyUpdateOutcome>>,
    }

    impl FakeAttempts {
        fn starts(&self) -> Vec<AttemptCommitPremise> {
            self.starts.lock().expect("start capture lock").clone()
        }

        fn updates(
            &self,
        ) -> Vec<(
            ActionAttemptId,
            ActionCertainty,
            ActionCertainty,
            EffectGrounds,
        )> {
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
            expected: ActionCertainty,
            new: ActionCertainty,
            grounds: EffectGrounds,
        ) -> Result<CertaintyUpdateOutcome, ActionTechnicalError> {
            self.updates
                .lock()
                .expect("update capture lock")
                .push((attempt, expected, new, grounds));
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
        let mut tracker = ActionEvaluationTracker::new();
        orchestrate_workspace_action(attempts, &mut tracker, command).await
    }

    #[tokio::test]
    async fn successful_create_returns_the_effect_and_claims_its_exact_premise() {
        let (directory, root) = workspace();
        let attempts = FakeAttempts::default();
        let mut command = command(root.clone(), OperationKind::Create, "report.md");
        command.content = Some(b"# report".to_vec());
        let expected_target = root
            .resolve("report.md", OperationKind::Create)
            .expect("create target");
        let expected = command.clone();

        let outcome = run(&attempts, command)
            .await
            .expect("domain outcomes are not technical errors");
        let ActionRunOutcome::Completed {
            attempt,
            effect,
            fact_recorded,
        } = outcome
        else {
            panic!("the configured claim started");
        };
        assert_eq!(effect.certainty, ActionCertainty::ConfirmedSuccess);
        assert_eq!(effect.grounds, EffectGrounds::ObservedAtTarget);
        assert!(certainty_grounds_pair_is_valid(
            effect.certainty,
            effect.grounds
        ));
        assert_eq!(
            effect.output,
            Some(ActionOutput::Created {
                target: expected_target.clone()
            })
        );
        assert!(fact_recorded);
        assert_eq!(
            fs::read(directory.path().join("report.md")).expect("created file"),
            b"# report"
        );

        let starts = attempts.starts();
        assert_eq!(starts.len(), 1);
        let premise = &starts[0];
        assert_eq!(premise.attempt, attempt);
        assert_eq!(premise.delegation, expected.delegation);
        assert_eq!(premise.task, expected.task);
        assert_eq!(premise.task_revision, expected.task_revision);
        assert_eq!(premise.workspace, expected.workspace);
        assert_eq!(premise.real_target, expected_target);
        assert_eq!(premise.operation, OperationKind::Create);
        assert!(!premise.relied_evaluation.as_uuid().is_nil());

        assert_eq!(
            attempts.updates(),
            vec![(
                attempt,
                ActionCertainty::Unknown,
                ActionCertainty::ConfirmedSuccess,
                EffectGrounds::ObservedAtTarget,
            )]
        );
    }

    #[tokio::test]
    async fn stale_premise_has_no_effect_and_no_certainty_update() {
        let (directory, root) = workspace();
        let attempts = FakeAttempts::default();
        attempts.set_start(ActionStartOutcome::StalePremise);
        let mut command = command(root, OperationKind::Create, "report.md");
        command.content = Some(b"# report".to_vec());

        assert_eq!(
            run(&attempts, command)
                .await
                .expect("stale is a domain outcome"),
            ActionRunOutcome::NotStarted(ActionNotStarted::StalePremise)
        );
        assert!(!directory.path().join("report.md").exists());
        assert_eq!(attempts.starts().len(), 1);
        assert!(attempts.updates().is_empty());
    }

    #[tokio::test]
    async fn rejected_target_never_claims_or_produces_an_effect() {
        let (directory, root) = workspace();
        let attempts = FakeAttempts::default();
        for (path, expected) in [
            ("../escape.txt", TargetRejection::MalformedPath),
            ("missing.txt", TargetRejection::MissingTarget),
        ] {
            assert_eq!(
                run(&attempts, command(root.clone(), OperationKind::Read, path))
                    .await
                    .expect("a rejection is a domain outcome"),
                ActionRunOutcome::NotStarted(ActionNotStarted::Rejected(expected))
            );
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let outside = tempdir().expect("outside directory");
            let sub = directory.path().join("sub");
            fs::create_dir(&sub).expect("inside subdirectory");
            symlink(outside.path(), directory.path().join("out")).expect("outside symlink");
            symlink(&sub, outside.path().join("back")).expect("back symlink");
            let mut create = command(root.clone(), OperationKind::Create, "out/back/new.txt");
            create.content = Some(b"# new".to_vec());
            assert_eq!(
                run(&attempts, create)
                    .await
                    .expect("a rejection is a domain outcome"),
                ActionRunOutcome::NotStarted(ActionNotStarted::Rejected(
                    TargetRejection::OutsideWorkspace
                ))
            );
            assert!(!sub.join("new.txt").exists());
        }

        #[cfg(windows)]
        {
            use std::os::windows::fs::symlink_dir;

            let outside = tempdir().expect("outside directory");
            let sub = directory.path().join("sub");
            fs::create_dir(&sub).expect("inside subdirectory");
            if symlink_dir(outside.path(), directory.path().join("out")).is_ok()
                && symlink_dir(&sub, outside.path().join("back")).is_ok()
            {
                let mut create = command(root.clone(), OperationKind::Create, "out/back/new.txt");
                create.content = Some(b"# new".to_vec());
                assert_eq!(
                    run(&attempts, create)
                        .await
                        .expect("a rejection is a domain outcome"),
                    ActionRunOutcome::NotStarted(ActionNotStarted::Rejected(
                        TargetRejection::OutsideWorkspace
                    ))
                );
                assert!(!sub.join("new.txt").exists());
            }
        }

        assert!(attempts.starts().is_empty());
        assert!(attempts.updates().is_empty());
    }

    #[tokio::test]
    async fn observed_effect_is_returned_when_fact_recording_does_not_update() {
        let (directory, root) = workspace();
        let attempts = FakeAttempts::default();
        attempts.set_update(CertaintyUpdateOutcome::MissingAttempt);
        let mut command = command(root, OperationKind::Create, "report.md");
        command.content = Some(b"# report".to_vec());

        let outcome = run(&attempts, command)
            .await
            .expect("a missing attempt row is a domain outcome");
        let ActionRunOutcome::Completed {
            attempt,
            effect,
            fact_recorded,
        } = outcome
        else {
            panic!("the configured claim started");
        };
        assert_eq!(effect.certainty, ActionCertainty::ConfirmedSuccess);
        assert!(!fact_recorded);
        assert_eq!(
            attempts.updates(),
            vec![(
                attempt,
                ActionCertainty::Unknown,
                ActionCertainty::ConfirmedSuccess,
                EffectGrounds::ObservedAtTarget,
            )]
        );
        assert_eq!(
            fs::read(directory.path().join("report.md")).expect("created file"),
            b"# report",
            "a failed fact write never hides the already observed effect"
        );
    }

    #[tokio::test]
    async fn command_and_effect_debug_bodies_are_absent() {
        let (_directory, root) = workspace();
        let mut command = command(root, OperationKind::Create, "report.md");
        command.content = Some(b"secret command body".to_vec());
        let command_debug = format!("{command:?}");
        assert!(!command_debug.contains("secret command body"));

        let effect = ObservedEffect {
            certainty: ActionCertainty::ConfirmedSuccess,
            grounds: EffectGrounds::ObservedAtTarget,
            output: Some(ActionOutput::Bytes(b"secret effect body".to_vec())),
        };
        let effect_debug = format!("{effect:?}");
        assert!(!effect_debug.contains("secret effect body"));
    }
}
