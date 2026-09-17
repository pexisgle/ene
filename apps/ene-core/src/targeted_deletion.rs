//! Host-composition Targeted Deletion participant registry and fan-out.
//!
//! `ene-preservation` owns the participant vocabulary; each semantic owner
//! implements [`ErasureParticipant`] in its own crate, and this module is the
//! only place that knows the concrete implementations (lifecycle §9). The
//! fan-out reads the durable operation and participant snapshot, issues one
//! bounded demand at a time, and records each returned fact through the
//! canonical store — it never invents a second participant registry and never
//! treats a missing implementation, an unreachable holder, or a local
//! completion as global completion.
//!
//! Local erasure inside a participant is a later slice: until an owner's
//! implementation is registered, the fan-out drives it as an explicit
//! unsupported participant whose durable hold keeps the operation unfinished.

use std::collections::HashMap;
use std::sync::Arc;

use ene_preservation::{
    DeletionLifecycleChange, DeletionMaterialOutcome, DeletionOperationMaterial,
    DeletionOperationPhase, DeletionOperationRef, DemandLocalErasureCommand, ErasureParticipant,
    ParticipantCompletionFact, ParticipantCompletionOutcome, ParticipantCompletionStatus,
    ParticipantDemandOutcome, ParticipantErasureScope, ParticipantHoldClass, ParticipantOwnerRef,
    PreservationRepository as _,
};
use ene_primitive::WallClockWithTz;
use ene_store::Store;

use crate::serve::CoreError;

/// Every participant owner the current product surface requires (lifecycle §8).
///
/// The list contains only owners that actually exist today and can hold or
/// derive target-bearing data; a future capability crate is not registered as
/// a fictitious participant. A3 slices register the implementation for their
/// owner through
/// [`HostHandle::register_deletion_participant`](crate::serve::HostHandle::register_deletion_participant),
/// which is the only extension point needed.
///
/// The Client transient holder is per incarnation rather than a fixed class:
/// once the Client-transient slice tracks which incarnation may hold a
/// target-bearing copy, it appends
/// [`ParticipantOwnerRef::ClientIncarnation`] entries to the required set at
/// admission. No incarnation is listed here while that tracking does not
/// exist, so an operation never claims a Client copy it cannot identify.
#[must_use]
pub fn current_product_surface_owners() -> Vec<ParticipantOwnerRef> {
    vec![
        ParticipantOwnerRef::Companion,
        ParticipantOwnerRef::Learning,
        ParticipantOwnerRef::Task,
        ParticipantOwnerRef::Action,
        ParticipantOwnerRef::Inference,
        ParticipantOwnerRef::Permission,
        ParticipantOwnerRef::Credential,
        ParticipantOwnerRef::Presence,
        ParticipantOwnerRef::HostTransient,
    ]
}

/// One participant implementation per owner.
///
/// `ene-preservation` cannot depend on a concrete participant crate, so the
/// composition keeps the mapping. A demand for an owner without a registered
/// implementation resolves to an explicit unsupported hold: never a fake
/// success, and never a reason to consider the operation complete.
#[derive(Default, Clone)]
pub struct ErasureParticipantRegistry {
    participants: HashMap<ParticipantOwnerRef, Arc<dyn ErasureParticipant>>,
}

impl std::fmt::Debug for ErasureParticipantRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ErasureParticipantRegistry")
            .field("owners", &self.participants.len())
            .finish()
    }
}

impl ErasureParticipantRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one implementation. A duplicate owner would make the fan-out
    /// nondeterministic, so it is refused rather than silently replaced.
    ///
    /// # Errors
    ///
    /// Returns the already-registered owner when one implementation per owner
    /// is already present.
    pub fn register(
        &mut self,
        participant: Arc<dyn ErasureParticipant>,
    ) -> Result<(), ParticipantOwnerRef> {
        let owner = participant.owner();
        if self.participants.contains_key(&owner) {
            return Err(owner);
        }
        self.participants.insert(owner, participant);
        Ok(())
    }

    /// Issues one bounded demand. An owner without an implementation reports
    /// [`ParticipantHoldClass::Unsupported`] for the exact demanded condition,
    /// so the durable snapshot distinguishes "not implemented yet" from
    /// "pending" and from success.
    pub async fn demand(&self, command: DemandLocalErasureCommand) -> ParticipantCompletionFact {
        match self.participants.get(&command.participant()) {
            Some(participant) => participant.demand_local_erasure(command).await,
            None => ParticipantCompletionFact::held(
                command.condition(),
                command.participant(),
                ParticipantHoldClass::Unsupported,
                WallClockWithTz::now(),
            ),
        }
    }
}

/// Bounded parameters for one fan-out pass.
///
/// A pass never runs unbounded participant work: each unfinished participant
/// receives at most [`Self::demands_per_participant`] bounded demands, and at
/// most [`Self::operation_limit`] unfinished operations are examined. A
/// participant still reporting more work is left unfinished for the next pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetedDeletionPass {
    /// Unfinished operations examined per pass, 1..=100 (the canonical read page).
    pub operation_limit: u32,
    /// Bounded demands per unfinished participant per pass, at least 1.
    pub demands_per_participant: u32,
}

impl TargetedDeletionPass {
    #[must_use]
    pub const fn new(operation_limit: u32, demands_per_participant: u32) -> Self {
        Self {
            operation_limit,
            demands_per_participant,
        }
    }
}

impl Default for TargetedDeletionPass {
    fn default() -> Self {
        Self::new(100, 4)
    }
}

/// What one bounded fan-out pass observed. The counts are progress metadata
/// for the status surface and tests; they are never a completion decision
/// (A5 aggregates the durable participant table for that).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TargetedDeletionPassOutcome {
    /// Unfinished operations examined (including non-active phases, which the
    /// pass leaves untouched until an explicit resume or finalization).
    pub operations: u32,
    /// Bounded demands issued to participants.
    pub demands: u32,
    /// Participant sweeps that reached `Verified` during this pass.
    pub verified: u32,
    /// Participant sweeps left incomplete at the end of the pass, including
    /// those held.
    pub unfinished: u32,
    /// Participant holds recorded during this pass.
    pub held: u32,
    /// Demands or facts refused because the operation's sweep had already
    /// moved on, or because its phase ended the work.
    pub stale_reports: u32,
}

fn deletion_error(error: ene_preservation::PreservationTechnicalError) -> CoreError {
    CoreError::Deletion(error.to_string())
}

/// Builds the body-free projection for a Client-bound holder and the local
/// material scope for an in-process owner (lifecycle §8.1).
fn command_scope(
    material: &DeletionOperationMaterial,
    owner: ParticipantOwnerRef,
) -> ParticipantErasureScope {
    let sources = material.sources().to_vec();
    if owner.is_incarnation() {
        ParticipantErasureScope::correlation_only(sources)
    } else {
        ParticipantErasureScope::local(material.target().clone(), sources)
    }
}

/// Drives one bounded fan-out pass over the durable unfinished operations.
///
/// Only `Active` operations are driven: a `Held` operation waits for an
/// explicit resume decision, and a `Finalizing` operation is owned by the
/// completion boundary. Participants already `Verified` for the current sweep
/// are never demanded again, so a fan-out that crashed after a participant's
/// semantic effect continues with only the unfinished participants (§14).
///
/// # Errors
///
/// Returns [`CoreError::Deletion`] for an invalid pass parameter or when the
/// canonical store refuses (including torn participant state, which fails
/// closed instead of reading as an incomplete set).
pub async fn drive_targeted_deletion(
    store: &Store,
    registry: &ErasureParticipantRegistry,
    pass: TargetedDeletionPass,
) -> Result<TargetedDeletionPassOutcome, CoreError> {
    if !(1..=100).contains(&pass.operation_limit) || pass.demands_per_participant == 0 {
        return Err(CoreError::Deletion(String::from(
            "invalid targeted deletion pass parameters",
        )));
    }
    let mut outcome = TargetedDeletionPassOutcome::default();
    let mut after = None;
    while (outcome.operations as usize) < pass.operation_limit as usize {
        let remaining = pass.operation_limit - outcome.operations;
        let limit = remaining.min(100);
        let page = store
            .unfinished_deletions(after, limit)
            .await
            .map_err(deletion_error)?;
        if page.is_empty() {
            break;
        }
        let page_len = page.len();
        for record in page {
            after = Some(record.current.operation);
            outcome.operations += 1;
            if record.phase != DeletionOperationPhase::Active {
                continue;
            }
            let progressed = drive_operation(
                store,
                registry,
                record.current,
                pass.demands_per_participant,
                &mut outcome,
            )
            .await?;
            if !progressed {
                // The sweep or phase moved while this pass was driving: the
                // remaining participants belong to a decision this pass must
                // not make.
                break;
            }
        }
        if page_len < limit as usize {
            break;
        }
    }
    Ok(outcome)
}

/// Drives one active operation; returns `false` when a concurrent lifecycle
/// transition (a new sweep or a completion/phase change) ended this pass's
/// authority over the operation.
async fn drive_operation(
    store: &Store,
    registry: &ErasureParticipantRegistry,
    current: DeletionOperationRef,
    demand_budget: u32,
    outcome: &mut TargetedDeletionPassOutcome,
) -> Result<bool, CoreError> {
    let condition = current.condition();
    let material = match store
        .deletion_operation_material(current.operation)
        .await
        .map_err(deletion_error)?
    {
        DeletionMaterialOutcome::Material(material) => material,
        // Missing means the operation vanished (nothing to drive); Destroyed
        // means completion started and this pass must not fan out erasure.
        DeletionMaterialOutcome::Missing | DeletionMaterialOutcome::Destroyed => return Ok(false),
    };
    let mut held = false;
    let mut after = None;
    loop {
        let page = store
            .deletion_participants(current.operation, after, 100)
            .await
            .map_err(deletion_error)?;
        if page.is_empty() {
            break;
        }
        let page_len = page.len();
        for record in page {
            after = Some(record.participant.owner);
            if record.progress.is_verified() {
                continue;
            }
            let mut verified = false;
            let mut held_this_participant = false;
            for _ in 0..demand_budget {
                // Durable-before-effect: the Running mark commits before the
                // participant may erase anything, so a crash leaves the
                // participant re-drivable instead of silently pending.
                match store
                    .begin_participant_demand(condition, record.participant.owner)
                    .await
                    .map_err(deletion_error)?
                {
                    ParticipantDemandOutcome::Marked(_) => {}
                    ParticipantDemandOutcome::AlreadyVerified => {
                        verified = true;
                        break;
                    }
                    ParticipantDemandOutcome::StaleSweep
                    | ParticipantDemandOutcome::Missing
                    | ParticipantDemandOutcome::Completed
                    | ParticipantDemandOutcome::NotRequired => {
                        outcome.stale_reports += 1;
                        return Ok(false);
                    }
                }
                outcome.demands += 1;
                let command = DemandLocalErasureCommand::new(
                    condition,
                    record.participant.owner,
                    command_scope(&material, record.participant.owner),
                );
                let fact = registry.demand(command).await;
                // The composition boundary is authoritative: a fact that does
                // not answer the exact demand is never recorded.
                if fact.condition() != condition || fact.participant() != record.participant.owner {
                    outcome.stale_reports += 1;
                    return Ok(false);
                }
                let fact_status = fact.status();
                match store
                    .record_participant_completion(fact)
                    .await
                    .map_err(deletion_error)?
                {
                    ParticipantCompletionOutcome::Recorded(progress) => {
                        if progress.is_verified() {
                            verified = true;
                            break;
                        }
                        // Any hold class means the participant cannot finish
                        // now; stop demanding it in this pass and carry the
                        // hold to the operation below.
                        if matches!(fact_status, ParticipantCompletionStatus::Held(_)) {
                            held_this_participant = true;
                            break;
                        }
                    }
                    ParticipantCompletionOutcome::AlreadyVerified => {
                        verified = true;
                        break;
                    }
                    ParticipantCompletionOutcome::StaleSweep
                    | ParticipantCompletionOutcome::Missing
                    | ParticipantCompletionOutcome::Completed
                    | ParticipantCompletionOutcome::NotRequired => {
                        outcome.stale_reports += 1;
                        return Ok(false);
                    }
                }
            }
            if verified {
                outcome.verified += 1;
            } else {
                outcome.unfinished += 1;
            }
            if held_this_participant {
                outcome.held += 1;
                held = true;
            }
        }
        if page_len < 100 {
            break;
        }
    }
    if held {
        // A held required participant makes the operation retryable-incomplete
        // (§5.1): the hold is durable and later passes skip the operation
        // until an explicit resume reopens it.
        store
            .change_deletion_lifecycle(current, DeletionLifecycleChange::Hold)
            .await
            .map_err(deletion_error)?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    use super::*;
    use crate::serve::{CredStore, HostHandle};
    use crate::test_support::memory_handle;
    use ene_action::{
        ActionAttemptId, ActionAttemptRepository as _, ActionCertainty, ActionStartOutcome,
        AttemptCommitPremise, CertaintyUpdateOutcome, EffectGrounds, OperationKind, RealTargetRef,
    };
    use ene_companion::{TaskFact, UndeliveredSource};
    use ene_credential::{CredentialScrubber, CredentialSetRevision, MemoryCredentialStore};
    use ene_inference::{
        AttemptBeginOutcome, InferenceAttempt, InferenceAttemptRepository as _, InferenceTicketId,
        TaskAgentAttemptPremise, UsageFact, UsageRepository as _, UsageSource,
    };
    use ene_permission::{
        CapabilityKind, ConsentCommitOutcome, ConsentRecord, ConsentRevision, ConsumerKind,
        IntentFingerprint, IntentOutcomeRepository as _, IntentResolution, PurposeKind,
    };
    use ene_preservation::{
        DeletionPurpose, DeletionSearchMaterial, DeletionSweepGeneration, ErasureParticipant,
        MechanicalDeletionTarget, ParticipantCompletionFact, ParticipantCompletionStatus,
        ParticipantHoldClass, ParticipantOwnerRef, ParticipantProgress,
        StartTargetedDeletionCommand, StartTargetedDeletionOutcome, TargetedDeletionTarget,
    };
    use ene_primitive::{RawId, RevisionInner, WallClockWithTz};
    use ene_task::{
        AssigneeRef, DelegatedWorkspace, DelegationCreationPremise, DelegationId,
        DelegationOutcome, DelegationScope, TaskAgentEphemeralId, TaskAgentInference,
        TaskAgentInferenceError, TaskAgentInferenceOutcome, TaskAgentInferencePremise,
        TaskAgentOutput, TaskAgentTurnOutcome, TaskAgentTurnPremise, TaskCommitOutcome,
        TaskCommitPremise, TaskContextEntryId, TaskContextOrigin, TaskContextOriginKind,
        TaskCreationPremise, TaskId, TaskInstructionSource, TaskInstructionSourceError,
        TaskInstructionSourceRecord, TaskPurpose, TaskPurposeAdoptionPremise, TaskRef,
        TaskReportSourceRef, TaskRepository as _, TaskResultId, WorkspaceAssocId,
        WorkspaceAssociationPremise, WorkspaceFolderRef, WorkspaceNeedRef,
        orchestrate_result_arrival, orchestrate_task_agent_turn,
    };

    /// One scripted participant that records how often it was demanded and
    /// whether its demand scope was body-free. A test may change the answer
    /// between passes (for example after an explicit resume).
    struct TestParticipant {
        owner: ParticipantOwnerRef,
        status: StdMutex<ParticipantCompletionStatus>,
        calls: AtomicUsize,
        correlation_only_observed: AtomicBool,
    }

    impl TestParticipant {
        fn new(owner: ParticipantOwnerRef, status: ParticipantCompletionStatus) -> Self {
            Self {
                owner,
                status: StdMutex::new(status),
                calls: AtomicUsize::new(0),
                correlation_only_observed: AtomicBool::new(false),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn set_status(&self, status: ParticipantCompletionStatus) {
            *self.status.lock().unwrap() = status;
        }

        fn correlation_only_observed(&self) -> bool {
            self.correlation_only_observed.load(Ordering::SeqCst)
        }
    }

    impl ErasureParticipant for TestParticipant {
        fn owner(&self) -> ParticipantOwnerRef {
            self.owner
        }

        fn demand_local_erasure(
            &self,
            command: DemandLocalErasureCommand,
        ) -> Pin<Box<dyn std::future::Future<Output = ParticipantCompletionFact> + Send + '_>>
        {
            Box::pin(async move {
                let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
                if command.scope().target().is_none() {
                    self.correlation_only_observed.store(true, Ordering::SeqCst);
                }
                let status = *self.status.lock().unwrap();
                let at = WallClockWithTz::now();
                match status {
                    ParticipantCompletionStatus::Verified => ParticipantCompletionFact::verified(
                        command.condition(),
                        command.participant(),
                        call as u64,
                        at,
                    ),
                    ParticipantCompletionStatus::LocalComplete => {
                        ParticipantCompletionFact::local_complete(
                            command.condition(),
                            command.participant(),
                            call as u64,
                            0,
                            at,
                        )
                    }
                    ParticipantCompletionStatus::MoreWork => ParticipantCompletionFact::more_work(
                        command.condition(),
                        command.participant(),
                        call as u64,
                        1,
                        at,
                    ),
                    ParticipantCompletionStatus::Held(reason) => ParticipantCompletionFact::held(
                        command.condition(),
                        command.participant(),
                        reason,
                        at,
                    ),
                }
            })
        }
    }

    /// A participant that advances the operation to a new sweep while it is
    /// working, then reports a completion minted against the superseded
    /// generation. The report must never update the new sweep's row.
    struct SweepAdvancingParticipant {
        owner: ParticipantOwnerRef,
        store: Store,
        current: DeletionOperationRef,
    }

    impl ErasureParticipant for SweepAdvancingParticipant {
        fn owner(&self) -> ParticipantOwnerRef {
            self.owner
        }

        fn demand_local_erasure(
            &self,
            command: DemandLocalErasureCommand,
        ) -> Pin<Box<dyn std::future::Future<Output = ParticipantCompletionFact> + Send + '_>>
        {
            Box::pin(async move {
                self.store
                    .change_deletion_lifecycle(
                        self.current,
                        crate::targeted_deletion::DeletionLifecycleChange::NextSweep,
                    )
                    .await
                    .expect("the sweep advance must apply");
                ParticipantCompletionFact::verified(
                    command.condition(),
                    command.participant(),
                    1,
                    WallClockWithTz::now(),
                )
            })
        }
    }

    fn admission(
        text: &str,
        participants: Vec<ParticipantOwnerRef>,
    ) -> StartTargetedDeletionCommand {
        StartTargetedDeletionCommand::new(
            TargetedDeletionTarget {
                mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                    text.into(),
                )),
                semantic_hints: vec![],
            },
            DeletionPurpose::Privacy,
            WallClockWithTz::now(),
            vec![],
            participants,
        )
        .confirmed_for_tests()
    }

    async fn admit(
        handle: &HostHandle,
        text: &str,
        participants: Vec<ParticipantOwnerRef>,
    ) -> DeletionOperationRef {
        match handle
            .store
            .start_targeted_deletion(admission(text, participants))
            .await
            .unwrap()
        {
            StartTargetedDeletionOutcome::Started(current) => current,
            other => panic!("unexpected admission: {other:?}"),
        }
    }

    async fn reopen(dir: &std::path::Path) -> HostHandle {
        HostHandle::open_with_cred_store(dir, CredStore::Memory(MemoryCredentialStore::new()))
            .await
            .expect("the host must reopen")
    }

    async fn participant_for(
        store: &Store,
        operation: ene_preservation::DeletionOperationId,
        owner: ParticipantOwnerRef,
    ) -> ene_preservation::DeletionParticipantRecord {
        store
            .deletion_participants(operation, None, 100)
            .await
            .unwrap()
            .into_iter()
            .find(|record| record.participant.owner == owner)
            .expect("the required participant row must exist")
    }

    /// One durable History append for the acceptance fixture.
    async fn append_history(
        handle: &HostHandle,
        companion: ene_companion::CompanionId,
        generation: ene_presence::PresenceGeneration,
        role: ene_companion::HistoryRole,
        text: &str,
    ) -> RawId {
        use ene_companion::HistoryRepository as _;
        match handle
            .store
            .append_message(ene_companion::AppendHistoryCommand {
                companion,
                round: RawId::new(),
                role,
                text: text.to_owned(),
                lang: String::from("en"),
                at: WallClockWithTz::now(),
                expected_generation: generation,
                expected_consent: None,
                expected_credential_set: None,
                expected_owner_message: None,
                command_id: None,
                round_wire: Some(RawId::new().as_uuid().as_hyphenated().to_string()),
                round_intent: None,
                incarnation: None,
                local_id: None,
            })
            .await
            .expect("the history append must commit")
        {
            ene_companion::HistoryAppendOutcome::CommittedAs { message } => message,
            other => panic!("the append must commit: {other:?}"),
        }
    }

    /// Commits one Memory change grounded on `summary`, as formation does.
    async fn commit_learning(
        handle: &HostHandle,
        summary: &ene_learning::SummaryRecord,
        target: ene_learning::MemoryTarget,
        content: &str,
        change: ene_learning::ChangeKind,
    ) {
        use ene_learning::LearningRepository as _;
        let outcome = handle
            .store
            .commit_memory_change(ene_learning::MemoryChangeCommit {
                summary: Some(summary.clone()),
                secret_premise: None,
                change: ene_learning::MemoryChange {
                    target,
                    scope: ene_learning::LearningScope::companion(summary.scope.companion_id()),
                    content: content.to_owned(),
                    importance: ene_learning::Importance::default(),
                    temporal: ene_learning::TemporalMeaning::Enduring,
                    change,
                    recall_suppressed: false,
                    at: WallClockWithTz::now(),
                },
            })
            .await
            .expect("the memory change must commit");
        assert!(
            matches!(outcome, ene_learning::MemoryChangeOutcome::Committed { .. }),
            "the fixture change must commit: {outcome:?}"
        );
    }

    fn summary_of(
        companion: RawId,
        content: String,
        start: RawId,
        end: RawId,
    ) -> ene_learning::SummaryRecord {
        ene_learning::SummaryRecord {
            id: ene_learning::SummaryId::generate(),
            scope: ene_learning::LearningScope::companion(companion),
            content,
            source: ene_learning::SourceRangeRef {
                kind: ene_learning::ExperienceSourceKind::Dialogue,
                start,
                end,
            },
            formed_at: WallClockWithTz::now(),
        }
    }

    /// Runs bounded fan-out passes until nothing is left to demand.
    async fn drive_until_settled(handle: &HostHandle) -> TargetedDeletionPassOutcome {
        for _ in 0..64 {
            let outcome = handle
                .drive_targeted_deletion(TargetedDeletionPass::new(100, 16))
                .await
                .expect("the fan-out must not fail");
            if outcome.held == 0 && outcome.unfinished == 0 && outcome.demands == 0 {
                return outcome;
            }
        }
        panic!("the bounded fan-out must settle");
    }

    #[tokio::test]
    async fn history_to_summary_to_memory_erasure_leaves_no_exact_remainder() {
        use ene_companion::{CompanionRepository as _, HistoryRepository as _, HistoryRole};
        use ene_learning::{
            ChangeKind, LearningRepository as _, MemoryId, MemoryRevision, MemoryTarget,
        };
        use ene_presence::PresenceRepository as _;

        let Some((handle, _dir)) = memory_handle("targeted-deletion-a3a").await else {
            panic!("the host must open");
        };
        let companion = handle
            .store
            .ensure_running_companion()
            .await
            .expect("the companion must resolve");
        let generation = handle
            .store
            .load_attribution(companion.as_raw())
            .await
            .expect("attribution must load")
            .expect("attribution must exist")
            .generation;
        let target = "swordfish-a3a-e2e";

        // Conversation -> Summary -> Memory update history, with the target
        // in every layer.
        let source_start = append_history(
            &handle,
            companion,
            generation,
            HistoryRole::Owner,
            &format!("my launch code is {target}"),
        )
        .await;
        let source_end = append_history(
            &handle,
            companion,
            generation,
            HistoryRole::Companion,
            "understood, I will remember",
        )
        .await;
        let summary = summary_of(
            companion.as_raw(),
            format!("The owner shared a private launch code: {target}."),
            source_start,
            source_end,
        );
        let memory = MemoryId::generate();
        commit_learning(
            &handle,
            &summary,
            MemoryTarget::New { id: memory },
            &format!("The owner's launch code is {target}."),
            ChangeKind::Initial,
        )
        .await;
        commit_learning(
            &handle,
            &summary,
            MemoryTarget::Existing {
                id: memory,
                expected_revision: MemoryRevision::initial(),
            },
            &format!("The owner's launch code is {target}, still current."),
            ChangeKind::Refined,
        )
        .await;

        // Positive control: unrelated History, Summary, and Memory stay.
        let unrelated_message = append_history(
            &handle,
            companion,
            generation,
            HistoryRole::Owner,
            "the weather is nice today",
        )
        .await;
        let unrelated_summary = summary_of(
            companion.as_raw(),
            String::from("The owner likes jasmine tea."),
            RawId::new(),
            RawId::new(),
        );
        let unrelated_memory = MemoryId::generate();
        commit_learning(
            &handle,
            &unrelated_summary,
            MemoryTarget::New {
                id: unrelated_memory,
            },
            "The owner likes jasmine tea.",
            ChangeKind::Initial,
        )
        .await;
        assert!(
            handle
                .store
                .count_exact_text_remainder_for_tests(target)
                .await
                .unwrap()
                > 0,
            "the fixture must place the target before the sweep"
        );

        let required = vec![
            ParticipantOwnerRef::Companion,
            ParticipantOwnerRef::Learning,
        ];
        let current = admit(&handle, target, required.clone()).await;
        let outcome = drive_until_settled(&handle).await;
        assert_eq!(outcome.held, 0);
        assert_eq!(outcome.unfinished, 0);
        assert_eq!(
            handle
                .store
                .count_exact_text_remainder_for_tests(target)
                .await
                .unwrap(),
            0,
            "no durable surface may keep the exact target"
        );

        // The usual reads can no longer reach the target.
        let timeline = handle
            .store
            .load_timeline(companion, None, None, 100)
            .await
            .expect("the timeline must load");
        assert!(timeline.iter().all(|item| !item.text.contains(target)));
        assert!(
            timeline.iter().any(|item| item.id == unrelated_message),
            "unrelated History stays"
        );
        let recalled = handle
            .store
            .recall_candidates(companion.as_raw(), &[String::from("swordfish")], 50)
            .await
            .expect("recall must answer");
        assert!(
            recalled
                .iter()
                .all(|memory| !memory.content.contains(target))
        );
        let memories = handle
            .store
            .list_current_memories(companion.as_raw(), None, 100)
            .await
            .expect("memories must list");
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].id, unrelated_memory);
        assert!(
            handle
                .store
                .list_memory_revisions(memory, None, 100)
                .await
                .expect("revisions must list")
                .is_empty(),
            "no past revision may keep the erased body"
        );
        let summaries = handle
            .store
            .load_summaries(&[summary.id, unrelated_summary.id])
            .await
            .expect("summaries must load");
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, unrelated_summary.id);

        // Every required participant verified for the current sweep, and the
        // status view never renders the target.
        let participants = handle
            .store
            .deletion_participants(current.operation, None, 100)
            .await
            .expect("participants must read");
        assert!(
            participants
                .iter()
                .all(|record| record.progress.is_verified())
        );
        let page = handle
            .deletion_status_page(None, 50)
            .await
            .expect("the status page must read");
        assert!(!format!("{page:?}").contains(target));
    }

    #[tokio::test]
    async fn a_multi_page_erasure_survives_a_host_restart() {
        use ene_companion::{CompanionRepository as _, HistoryRepository as _, HistoryRole};
        use ene_presence::PresenceRepository as _;

        let Some((handle, dir)) = memory_handle("targeted-deletion-a3a-pages").await else {
            panic!("the host must open");
        };
        let companion = handle
            .store
            .ensure_running_companion()
            .await
            .expect("the companion must resolve");
        let generation = handle
            .store
            .load_attribution(companion.as_raw())
            .await
            .expect("attribution must load")
            .expect("attribution must exist")
            .generation;
        let target = "swordfish-a3a-pages";
        let rows = ene_store::ERASURE_SCAN_ROWS * 3;
        for index in 0..rows {
            append_history(
                &handle,
                companion,
                generation,
                HistoryRole::Owner,
                &format!("{target} note {index}"),
            )
            .await;
        }
        let surviving = append_history(
            &handle,
            companion,
            generation,
            HistoryRole::Owner,
            "an unrelated note",
        )
        .await;

        let current = admit(
            &handle,
            target,
            vec![
                ParticipantOwnerRef::Companion,
                ParticipantOwnerRef::Learning,
            ],
        )
        .await;
        // One bounded demand per participant: the sweep cannot finish yet.
        let first = handle
            .drive_targeted_deletion(TargetedDeletionPass::new(100, 1))
            .await
            .expect("the fan-out must run");
        assert!(first.unfinished > 0, "the sweep must still have work");

        // Restart: the durable operation and participant snapshot survive, the
        // in-memory continuation cursors do not, and the reopened composition
        // re-registers the built-in implementations.
        drop(handle);
        let reopened = reopen(dir.path()).await;
        let outcome = drive_until_settled(&reopened).await;
        assert_eq!(outcome.held, 0);
        assert_eq!(outcome.unfinished, 0);
        assert_eq!(
            reopened
                .store
                .count_exact_text_remainder_for_tests(target)
                .await
                .unwrap(),
            0
        );
        let record = reopened
            .store
            .deletion_status(None, 100)
            .await
            .expect("the status must read")
            .into_iter()
            .find(|record| record.current == current)
            .expect("the operation identity survives the restart");
        assert_eq!(record.current, current);
        let timeline = reopened
            .store
            .load_timeline(companion, None, None, 1000)
            .await
            .expect("the timeline must load");
        assert_eq!(timeline.len(), 1);
        assert_eq!(timeline[0].id, surviving);
    }

    #[tokio::test]
    async fn admission_snapshots_the_current_product_surface_and_restart_keeps_it() {
        let Some((handle, dir)) = memory_handle("targeted-deletion-snapshot").await else {
            panic!("the host must open");
        };
        let required = handle.required_deletion_participants();
        assert!(
            !required.is_empty(),
            "the current product surface always requires participants"
        );
        assert!(required.contains(&ParticipantOwnerRef::Companion));
        assert!(required.contains(&ParticipantOwnerRef::HostTransient));
        assert!(
            !required.iter().any(|owner| owner.is_incarnation()),
            "no Client incarnation is claimed while no copy tracking exists"
        );
        let current = admit(&handle, "snapshot-target", required.clone()).await;
        let before = handle
            .store
            .deletion_participants(current.operation, None, 100)
            .await
            .unwrap();
        assert_eq!(before.len(), required.len());
        drop(handle);
        let reopened = reopen(dir.path()).await;
        let after = reopened
            .store
            .deletion_participants(current.operation, None, 100)
            .await
            .unwrap();
        assert_eq!(
            after, before,
            "the participant snapshot survives restart unchanged"
        );
        assert_eq!(reopened.required_deletion_participants(), required);
    }

    #[tokio::test]
    async fn an_unimplemented_owner_holds_the_operation_and_is_never_a_completion() {
        let Some((handle, _dir)) = memory_handle("targeted-deletion-unsupported").await else {
            panic!("the host must open");
        };
        // The semantic owners the current composition implements are
        // registered at open, so the unregistered-owner contract is pinned
        // with a holder-driven participant that no composition registers
        // implicitly: a Client incarnation. The operation must still require
        // it, hold on it, and never read the hold as completion.
        let required = vec![ParticipantOwnerRef::ClientIncarnation(RawId::new())];
        let current = admit(&handle, "unsupported-target", required.clone()).await;
        let outcome = handle
            .drive_targeted_deletion(TargetedDeletionPass::default())
            .await
            .unwrap();
        assert_eq!(outcome.verified, 0);
        assert_eq!(outcome.held as usize, required.len());
        assert_eq!(outcome.unfinished as usize, required.len());
        let rows = handle.store.unfinished_deletions(None, 100).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].current, current,
            "the operation identity is never regenerated"
        );
        assert_eq!(
            rows[0].phase,
            DeletionOperationPhase::Held,
            "a required hold keeps the operation retryable-incomplete"
        );
        let participants = handle
            .store
            .deletion_participants(current.operation, None, 100)
            .await
            .unwrap();
        assert_eq!(participants.len(), required.len());
        assert!(
            participants
                .iter()
                .all(|record| record.progress.hold_reason()
                    == Some(ParticipantHoldClass::Unsupported)),
            "an unregistered owner is an explicit unsupported hold"
        );
        assert!(
            !participants
                .iter()
                .all(|record| record.progress.is_verified()),
            "held is never a global completion candidate"
        );
        // A held operation waits for an explicit resume instead of being
        // hammered by the next pass.
        let again = handle
            .drive_targeted_deletion(TargetedDeletionPass::default())
            .await
            .unwrap();
        assert_eq!(again.demands, 0);
    }

    #[tokio::test]
    async fn a_crashed_fan_out_re_drives_only_the_unfinished_participants() {
        let Some((handle, dir)) = memory_handle("targeted-deletion-crash").await else {
            panic!("the host must open");
        };
        // Scripted participants for owners the built-in composition also
        // serves: the registry is cleared so the scripted set is the whole
        // composition.
        handle.reset_deletion_participants_for_tests();
        let required = vec![
            ParticipantOwnerRef::Companion,
            ParticipantOwnerRef::Learning,
        ];
        let current = admit(&handle, "crash-target", required).await;
        let companion = Arc::new(TestParticipant::new(
            ParticipantOwnerRef::Companion,
            ParticipantCompletionStatus::Verified,
        ));
        let learning = Arc::new(TestParticipant::new(
            ParticipantOwnerRef::Learning,
            ParticipantCompletionStatus::MoreWork,
        ));
        handle
            .register_deletion_participant(companion.clone())
            .unwrap();
        handle
            .register_deletion_participant(learning.clone())
            .unwrap();
        let first = handle
            .drive_targeted_deletion(TargetedDeletionPass::new(100, 1))
            .await
            .unwrap();
        assert_eq!((first.verified, first.unfinished), (1, 1));
        assert_eq!(companion.calls(), 1);
        assert_eq!(learning.calls(), 1);
        // Crash: the handle (and the in-memory registry) drops; the durable
        // snapshot and progress do not.
        drop(handle);
        let reopened = reopen(dir.path()).await;
        reopened.reset_deletion_participants_for_tests();
        let companion_after = Arc::new(TestParticipant::new(
            ParticipantOwnerRef::Companion,
            ParticipantCompletionStatus::Verified,
        ));
        let learning_after = Arc::new(TestParticipant::new(
            ParticipantOwnerRef::Learning,
            ParticipantCompletionStatus::Verified,
        ));
        reopened
            .register_deletion_participant(companion_after.clone())
            .unwrap();
        reopened
            .register_deletion_participant(learning_after.clone())
            .unwrap();
        let second = reopened
            .drive_targeted_deletion(TargetedDeletionPass::new(100, 2))
            .await
            .unwrap();
        assert_eq!(second.verified, 1);
        assert_eq!(
            companion_after.calls(),
            0,
            "a participant verified for the sweep is never re-driven"
        );
        assert_eq!(
            learning_after.calls(),
            1,
            "only the unfinished participant continues"
        );
        let rows = reopened
            .store
            .unfinished_deletions(None, 100)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].current, current);
        let participants = reopened
            .store
            .deletion_participants(current.operation, None, 100)
            .await
            .unwrap();
        assert!(
            participants
                .iter()
                .all(|record| record.progress.is_verified())
        );
    }

    #[tokio::test]
    async fn a_disconnected_client_incarnation_is_held_and_not_completion() {
        let Some((handle, _dir)) = memory_handle("targeted-deletion-client").await else {
            panic!("the host must open");
        };
        handle.reset_deletion_participants_for_tests();
        let incarnation = ParticipantOwnerRef::ClientIncarnation(RawId::new());
        let required = vec![ParticipantOwnerRef::Companion, incarnation];
        let current = admit(&handle, "client-target", required).await;
        let companion = Arc::new(TestParticipant::new(
            ParticipantOwnerRef::Companion,
            ParticipantCompletionStatus::Verified,
        ));
        let client = Arc::new(TestParticipant::new(
            incarnation,
            ParticipantCompletionStatus::Held(ParticipantHoldClass::Unavailable),
        ));
        handle
            .register_deletion_participant(companion.clone())
            .unwrap();
        handle
            .register_deletion_participant(client.clone())
            .unwrap();
        let outcome = handle
            .drive_targeted_deletion(TargetedDeletionPass::default())
            .await
            .unwrap();
        assert_eq!(outcome.verified, 1);
        assert_eq!(outcome.held, 1);
        assert!(
            client.correlation_only_observed(),
            "a Client-bound demand never carries the target body"
        );
        let rows = handle.store.unfinished_deletions(None, 100).await.unwrap();
        assert_eq!(
            rows[0].phase,
            DeletionOperationPhase::Held,
            "a disconnect is never a global completion"
        );
        let client_row = participant_for(&handle.store, current.operation, incarnation).await;
        assert_eq!(
            client_row.progress.hold_reason(),
            Some(ParticipantHoldClass::Unavailable)
        );
        assert!(!client_row.progress.is_verified());
        // Once the holder is reachable again, an explicit resume reopens the
        // operation and the next pass re-drives the previously held participant.
        let held = rows[0].current;
        handle
            .store
            .change_deletion_lifecycle(held, DeletionLifecycleChange::Resume)
            .await
            .unwrap();
        client.set_status(ParticipantCompletionStatus::Verified);
        let resumed = handle
            .drive_targeted_deletion(TargetedDeletionPass::default())
            .await
            .unwrap();
        assert_eq!(resumed.verified, 1);
        assert!(client.calls() >= 2);
        let verified = participant_for(&handle.store, current.operation, incarnation).await;
        assert!(verified.progress.is_verified());
    }

    #[tokio::test]
    async fn a_completion_from_a_superseded_sweep_never_updates_the_current_row() {
        let Some((handle, _dir)) = memory_handle("targeted-deletion-stale").await else {
            panic!("the host must open");
        };
        handle.reset_deletion_participants_for_tests();
        let current = admit(
            &handle,
            "stale-target",
            vec![ParticipantOwnerRef::Companion],
        )
        .await;
        let participant = Arc::new(SweepAdvancingParticipant {
            owner: ParticipantOwnerRef::Companion,
            store: handle.store.clone(),
            current,
        });
        handle.register_deletion_participant(participant).unwrap();
        let outcome = handle
            .drive_targeted_deletion(TargetedDeletionPass::default())
            .await
            .unwrap();
        assert_eq!(outcome.stale_reports, 1);
        assert_eq!(outcome.verified, 0);
        let rows = handle.store.unfinished_deletions(None, 100).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].current.sweep, DeletionSweepGeneration::from_u64(2));
        assert_eq!(rows[0].phase, DeletionOperationPhase::Active);
        let participants = handle
            .store
            .deletion_participants(rows[0].current.operation, None, 100)
            .await
            .unwrap();
        assert_eq!(
            participants[0].progress,
            ParticipantProgress::Pending,
            "the stale fact must not verify the new sweep"
        );
    }

    #[tokio::test]
    async fn duplicate_participant_registration_is_refused() {
        let Some((handle, _dir)) = memory_handle("targeted-deletion-duplicate").await else {
            panic!("the host must open");
        };
        handle.reset_deletion_participants_for_tests();
        handle
            .register_deletion_participant(Arc::new(TestParticipant::new(
                ParticipantOwnerRef::Companion,
                ParticipantCompletionStatus::Verified,
            )))
            .unwrap();
        assert!(matches!(
            handle.register_deletion_participant(Arc::new(TestParticipant::new(
                ParticipantOwnerRef::Companion,
                ParticipantCompletionStatus::Verified,
            ))),
            Err(CoreError::Deletion(_))
        ));
    }

    #[tokio::test]
    async fn invalid_pass_parameters_are_refused_without_touching_state() {
        let Some((handle, _dir)) = memory_handle("targeted-deletion-pass").await else {
            panic!("the host must open");
        };
        for pass in [
            TargetedDeletionPass::new(0, 1),
            TargetedDeletionPass::new(101, 1),
            TargetedDeletionPass::new(1, 0),
        ] {
            assert!(matches!(
                handle.drive_targeted_deletion(pass).await,
                Err(CoreError::Deletion(_))
            ));
        }
    }

    // --- Stage 6 A3b: the real Task / Action / Inference participants ------

    /// One host-side fixture carrying the same target in every current
    /// production surface of the three owners.
    struct TargetSurface {
        task: TaskRef,
        delegation: DelegationId,
        result: TaskResultId,
        done: ActionAttemptId,
        unknown: ActionAttemptId,
        ticket: InferenceTicketId,
    }

    fn target_folder(target: &str) -> String {
        std::env::temp_dir()
            .join(format!("ene-a3b-{target}"))
            .to_string_lossy()
            .into_owned()
    }

    /// Seeds the whole Task / Action / Inference surface through the
    /// canonical producers, in execution order: attempts (and the inference
    /// claim) before the result arrival seals the delegation.
    async fn seed_target_surface(handle: &HostHandle, target: &str) -> TargetSurface {
        let folder = target_folder(target);
        let save_target = format!("{folder}/out");
        let workspace = WorkspaceAssociationPremise {
            assoc: WorkspaceAssocId::generate(),
            need: WorkspaceNeedRef {
                folder: WorkspaceFolderRef {
                    path: folder.clone(),
                },
                save_target: Some(WorkspaceFolderRef {
                    path: save_target.clone(),
                }),
            },
        };
        let assoc = workspace.assoc;
        let source = RawId::new();
        let created = handle
            .store
            .create_task(TaskCreationPremise {
                task: TaskId::generate(),
                purpose: TaskPurpose {
                    text: format!("keep {target} private"),
                },
                entry: TaskContextEntryId::generate(),
                origin: TaskContextOrigin {
                    kind: TaskContextOriginKind::OwnerConversation,
                    source,
                },
                acquired_at: WallClockWithTz::now(),
                assignee: AssigneeRef {
                    companion: RawId::new(),
                },
                workspace: Some(workspace),
            })
            .await
            .expect("the task must commit");
        let advanced = handle
            .store
            .forward_steering(TaskCommitPremise {
                expected: created,
                new_purpose: Some(TaskPurposeAdoptionPremise {
                    purpose: TaskPurpose {
                        text: format!("now {target} is adopted"),
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
            .expect("the steering must commit");
        let TaskCommitOutcome::CommittedAs(current) = advanced else {
            panic!("the steering must commit, got {advanced:?}");
        };
        let delegation = DelegationId::generate();
        assert!(matches!(
            handle
                .store
                .create_delegation(DelegationCreationPremise {
                    delegation,
                    task: current,
                    agent: TaskAgentEphemeralId::generate(),
                    scope_copy: DelegationScope {
                        workspace: Some(DelegatedWorkspace {
                            assoc,
                            folder: WorkspaceFolderRef {
                                path: folder.clone(),
                            },
                            save_target: Some(WorkspaceFolderRef {
                                path: save_target.clone(),
                            }),
                        }),
                    },
                })
                .await
                .expect("the delegation must commit"),
            DelegationOutcome::Delegated(_)
        ));
        let done = ActionAttemptId::generate();
        assert_eq!(
            handle
                .store
                .insert_attempt_if_current(AttemptCommitPremise {
                    attempt: done,
                    delegation: delegation.as_raw(),
                    task: current.task.as_raw(),
                    task_revision: RevisionInner::from_u64(current.revision.as_u64()),
                    workspace: assoc.as_raw(),
                    real_target: RealTargetRef::from_canonical_path(format!(
                        "{folder}/{target}.md"
                    )),
                    operation: OperationKind::Create,
                    relied_evaluation: RawId::new(),
                })
                .await
                .unwrap(),
            ActionStartOutcome::Started
        );
        assert_eq!(
            handle
                .store
                .compare_and_set_certainty(
                    done,
                    ActionCertainty::Unknown,
                    ActionCertainty::ConfirmedSuccess,
                    EffectGrounds::ObservedAtTarget,
                )
                .await
                .unwrap(),
            CertaintyUpdateOutcome::Updated
        );
        let unknown = ActionAttemptId::generate();
        assert_eq!(
            handle
                .store
                .insert_attempt_if_current(AttemptCommitPremise {
                    attempt: unknown,
                    delegation: delegation.as_raw(),
                    task: current.task.as_raw(),
                    task_revision: RevisionInner::from_u64(current.revision.as_u64()),
                    workspace: assoc.as_raw(),
                    real_target: RealTargetRef::from_canonical_path(format!(
                        "{folder}/{target}-open.md"
                    )),
                    operation: OperationKind::Read,
                    relied_evaluation: RawId::new(),
                })
                .await
                .unwrap(),
            ActionStartOutcome::Started
        );
        let assigned = handle
            .store
            .assign_with_intent(
                None,
                ConsentRecord {
                    capability: CapabilityKind::Dialogue,
                    id: String::from("consent-1"),
                    rev: ConsentRevision::from_u64(1),
                    provider: String::from("openai"),
                    model: String::from("dialogue-1"),
                    credential_id: String::from("openai:main"),
                },
                IntentFingerprint {
                    intent_id: RawId::new().as_uuid().to_string(),
                    kind: String::from("assign"),
                    target: String::from("consent:a3b-fixture"),
                    base: String::from("consent-a3b-fixture"),
                    rationale_origin: String::from("management-surface"),
                    rationale_quote: None,
                },
            )
            .await
            .expect("the consent fixture must commit");
        assert!(matches!(
            assigned,
            IntentResolution::Decided(ConsentCommitOutcome::Committed { .. })
        ));
        let ticket = InferenceTicketId(RawId::new());
        assert_eq!(
            handle
                .store
                .begin_inference_attempt(InferenceAttempt {
                    ticket,
                    consumer: ConsumerKind::TaskAgent,
                    capability: CapabilityKind::Dialogue,
                    purpose: PurposeKind::TaskAgentTurn,
                    expected_consent: (String::from("consent-1"), ConsentRevision::from_u64(1)),
                    expected_credential_set: CredentialSetRevision::initial(),
                    provider: String::from("openai"),
                    model: String::from("dialogue-1"),
                    task_agent: Some(TaskAgentAttemptPremise {
                        delegation: delegation.as_raw(),
                        task: current.task.as_raw(),
                        task_revision: RevisionInner::from_u64(current.revision.as_u64()),
                        data_use: vec![source],
                    }),
                    pricing: None,
                })
                .await,
            Ok(AttemptBeginOutcome::Started)
        );
        handle
            .store
            .record_usage(UsageFact {
                ticket,
                provider: String::from("openai"),
                model: String::from("dialogue-1"),
                input_tokens: Some(200),
                cached_input_tokens: Some(50),
                output_tokens: Some(80),
                source: UsageSource::Reported,
            })
            .await
            .expect("the usage settlement must commit");
        let result = orchestrate_result_arrival(
            &handle.store,
            delegation,
            TaskAgentOutput::new(format!("final report mentions {target}")),
        )
        .await
        .expect("the result arrival must commit");
        TargetSurface {
            task: current,
            delegation,
            result: result.result,
            done,
            unknown,
            ticket,
        }
    }

    /// Counts stored values of one owner column that still contain the exact
    /// target. This is the mechanical remainder check on the real database
    /// file, independent of any repository read or cache.
    fn matching_cells(dir: &std::path::Path, table: &str, column: &str, target: &str) -> i64 {
        let conn = rusqlite::Connection::open(dir.join("app.db"))
            .expect("the store file must open for the remainder probe");
        conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM {table} WHERE {column} IS NOT NULL AND instr({column}, ?1) > 0"
            ),
            rusqlite::params![target],
            |row| row.get(0),
        )
        .expect("the remainder probe must read")
    }

    /// Drives the fan-out until the local owners of the fixture verify.
    async fn drive_until_local_owners_verify(
        handle: &HostHandle,
        expected: u32,
    ) -> TargetedDeletionPassOutcome {
        for _ in 0..8 {
            let outcome = handle
                .drive_targeted_deletion(TargetedDeletionPass::default())
                .await
                .unwrap();
            if outcome.verified == expected {
                return outcome;
            }
        }
        panic!("the bounded participants must settle");
    }

    /// A port that records the assembled logical input instead of dispatching.
    struct RecordingPort {
        prompt: StdMutex<Option<String>>,
    }

    impl RecordingPort {
        fn new() -> Self {
            Self {
                prompt: StdMutex::new(None),
            }
        }

        fn prompt(&self) -> String {
            self.prompt
                .lock()
                .unwrap()
                .clone()
                .expect("the port must have been called")
        }
    }

    impl TaskAgentInference for RecordingPort {
        fn input_budget(&self) -> usize {
            ene_inference::MAX_INPUT_CHARS
        }

        async fn infer(
            &self,
            premise: TaskAgentInferencePremise,
        ) -> Result<TaskAgentInferenceOutcome, TaskAgentInferenceError> {
            *self.prompt.lock().unwrap() = Some(premise.prompt.text().to_owned());
            Ok(TaskAgentInferenceOutcome::Produced {
                output: TaskAgentOutput::new(String::from("captured")),
                adoption_consent_current: true,
            })
        }
    }

    /// The fixture Tasks adopt no instruction entry, so no body is read; a
    /// call would mean the fixture drifted.
    struct NoInstructions;

    impl TaskInstructionSource for NoInstructions {
        async fn load_owner_instruction(
            &self,
            _origin: TaskContextOrigin,
        ) -> Result<Option<TaskInstructionSourceRecord>, TaskInstructionSourceError> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn local_owners_erase_the_current_surface_and_keep_objective_facts() {
        let Some((handle, dir)) = memory_handle("targeted-deletion-a3b-e2e").await else {
            panic!("the host must open");
        };
        let target = "e2e-target-1587";
        let fixture = seed_target_surface(&handle, target).await;
        let before_attempt = handle
            .store
            .load_inference_attempt(fixture.ticket)
            .await
            .unwrap()
            .expect("the claimed attempt exists");
        let before_usage = handle
            .store
            .load_usage_cost(fixture.ticket)
            .await
            .unwrap()
            .expect("the settled usage exists");

        // Positive control: every body-bearing owner column carries the
        // target before the sweep.
        let columns = [
            ("task", "purpose_text"),
            ("task_revision", "purpose_text"),
            ("task_result", "body"),
            ("workspace_assoc", "folder"),
            ("workspace_assoc", "save_target"),
            ("delegation", "scope_folder"),
            ("delegation", "scope_save_target"),
            ("action_attempt", "real_target"),
        ];
        for (table, column) in columns {
            assert!(
                matching_cells(dir.path(), table, column, target) > 0,
                "{table}.{column} must carry the target before erasure"
            );
        }

        let current = admit(
            &handle,
            target,
            vec![
                ParticipantOwnerRef::Task,
                ParticipantOwnerRef::Action,
                ParticipantOwnerRef::Inference,
            ],
        )
        .await;
        let outcome = drive_until_local_owners_verify(&handle, 3).await;
        assert_eq!(outcome.verified, 3);
        assert_eq!(outcome.held, 0);
        for owner in [
            ParticipantOwnerRef::Task,
            ParticipantOwnerRef::Action,
            ParticipantOwnerRef::Inference,
        ] {
            let record = participant_for(&handle.store, current.operation, owner).await;
            assert!(record.progress.is_verified(), "{owner:?} must verify");
            assert_eq!(record.remainder_count, 0);
        }

        // Mechanical remainder: zero on the real database file.
        for (table, column) in columns {
            assert_eq!(
                matching_cells(dir.path(), table, column, target),
                0,
                "{table}.{column} must carry no remainder"
            );
        }

        // Objective facts survive: the execution fact, the certainty, the
        // Task lifecycle, and the usage attribution.
        let done = handle
            .store
            .load_attempt(fixture.done)
            .await
            .unwrap()
            .expect("the completed attempt stays");
        assert_eq!(done.certainty, ActionCertainty::ConfirmedSuccess);
        assert_eq!(done.grounds, Some(EffectGrounds::ObservedAtTarget));
        assert!(
            std::path::Path::new(done.real_target.as_path()).is_absolute(),
            "the erased locator stays a readable absolute path"
        );
        let unknown = handle
            .store
            .load_attempt(fixture.unknown)
            .await
            .unwrap()
            .expect("the unknown attempt stays");
        assert_eq!(
            unknown.certainty,
            ActionCertainty::Unknown,
            "deletion never downgrades an unknown effect to not executed"
        );
        let task = handle
            .store
            .load_task(fixture.task.task)
            .await
            .unwrap()
            .expect("the task stays");
        assert_eq!(task.task.reference, fixture.task);
        assert_eq!(task.task.progress, ene_task::TaskProgress::InProgress);
        assert!(!task.revision.purpose_text.text.contains(target));
        assert_eq!(
            handle
                .store
                .load_inference_attempt(fixture.ticket)
                .await
                .unwrap(),
            Some(before_attempt),
            "the ticket correlation is untouched"
        );
        assert_eq!(
            handle.store.load_usage_cost(fixture.ticket).await.unwrap(),
            Some(before_usage),
            "settled usage is neither zeroed nor double-counted"
        );

        // The report readers and the derived presentation excerpt read the
        // erased owner rows, never a stale snapshot.
        let result_body = handle
            .store
            .load_report_source_bounded(TaskReportSourceRef::ResultBody(fixture.result), 0, 4096)
            .await
            .unwrap()
            .expect("the recorded result stays");
        assert!(!result_body.text.contains(target));
        let excerpt = handle
            .store
            .load_undelivered_excerpt(
                UndeliveredSource::TaskRecord {
                    task: fixture.task.task.as_raw(),
                    fact: TaskFact::ActionAttempt {
                        attempt: fixture.done.as_raw(),
                        certainty: ene_companion::ActionCertaintyWire::ConfirmedSuccess,
                    },
                },
                4096,
            )
            .await
            .unwrap()
            .expect("the attempt derives a presentation excerpt");
        assert!(
            !excerpt.text.contains(target),
            "the excerpt derives from the erased attempt row: {}",
            excerpt.text
        );
    }

    #[tokio::test]
    async fn the_task_agent_prompt_is_rebuilt_from_erased_rows() {
        let Some((handle, _dir)) = memory_handle("targeted-deletion-a3b-prompt").await else {
            panic!("the host must open");
        };
        let target = "e2e-prompt-target-1587";
        let fixture = seed_target_surface(&handle, target).await;
        let port = RecordingPort::new();
        let scrubber = CredentialScrubber {
            refs: &handle.store,
            store: &handle.cred_store,
        };
        let before = orchestrate_task_agent_turn(
            &handle.store,
            &NoInstructions,
            &port,
            &scrubber,
            TaskAgentTurnPremise {
                delegation: fixture.delegation,
                exchanges: Vec::new(),
            },
        )
        .await
        .expect("the turn must answer");
        assert!(matches!(before, TaskAgentTurnOutcome::Produced(_)));
        assert!(
            port.prompt().contains(target),
            "positive control: the assembled prompt carries the Task-owned purpose"
        );

        let current = admit(
            &handle,
            target,
            vec![ParticipantOwnerRef::Task, ParticipantOwnerRef::Action],
        )
        .await;
        let outcome = drive_until_local_owners_verify(&handle, 2).await;
        assert_eq!(outcome.verified, 2);
        let _ = current;

        let after = orchestrate_task_agent_turn(
            &handle.store,
            &NoInstructions,
            &port,
            &scrubber,
            TaskAgentTurnPremise {
                delegation: fixture.delegation,
                exchanges: Vec::new(),
            },
        )
        .await
        .expect("the post-erasure turn must answer");
        assert!(matches!(after, TaskAgentTurnOutcome::Produced(_)));
        let prompt = port.prompt();
        assert!(
            !prompt.contains(target),
            "the prompt is assembled from the erased rows, got {prompt}"
        );
        assert!(prompt.contains("[erased]"));
    }
}
