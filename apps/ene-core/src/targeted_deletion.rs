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
    use ene_credential::MemoryCredentialStore;
    use ene_preservation::{
        DeletionPurpose, DeletionSearchMaterial, DeletionSweepGeneration, ErasureParticipant,
        MechanicalDeletionTarget, ParticipantCompletionFact, ParticipantCompletionStatus,
        ParticipantHoldClass, ParticipantOwnerRef, ParticipantProgress,
        StartTargetedDeletionCommand, StartTargetedDeletionOutcome, TargetedDeletionTarget,
    };
    use ene_primitive::{RawId, WallClockWithTz};

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
        let required = handle.required_deletion_participants();
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
}
