//! Host-composition Targeted Deletion participant registry and fan-out.
//!
//! `ene-preservation` owns the participant vocabulary; each semantic owner
//! implements [`ErasureParticipant`] in its own crate, and the Host
//! composition (`serve.rs`) constructs each concrete implementation and
//! registers it here (lifecycle §9); this module holds only
//! `Arc<dyn ErasureParticipant>`. The
//! fan-out reads the durable operation and participant snapshot, issues one
//! bounded demand at a time, and records each returned fact through the
//! canonical store — it never invents a second participant registry and never
//! treats a missing implementation, an unreachable holder, or a local
//! completion as global completion.
//!
//! A demand for an owner with no registered implementation is driven as an
//! explicit unsupported participant whose durable hold keeps the operation
//! unfinished.
//!
//! The production entry points over this module are bounded: Host startup
//! restores unfinished operations (resuming a retryable hold once, lifecycle
//! §14), the serving composition runs a periodic tick (one pass plus a
//! backed-off retry of a retryable hold), and a first-party confirmation kicks
//! a bounded drive immediately after admission. None of them decides
//! completion: only the sealed store boundary does.
//!
//! Each bounded walk keeps a durable keyset cursor in the canonical store and
//! starts its page after the last operation the previous pass examined,
//! wrapping to the beginning at the end of the unfinished set. The cursor is
//! a scheduling position only: with more unfinished operations than one pass
//! bound, later passes reach the tail instead of re-reading the head forever,
//! and every completion premise is still re-derived from durable operation
//! and participant state on each visit.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use ene_preservation::{
    DELETION_RECONCILIATION_PAGE_SIZE, DeletionFinalizationOutcome, DeletionLifecycleChange,
    DeletionLifecycleOutcome, DeletionMaterialOutcome, DeletionOperationId,
    DeletionOperationMaterial, DeletionOperationPhase, DeletionOperationRecord,
    DeletionOperationRef, DeletionReconciliationOutcome, DeletionWalk, DemandLocalErasureCommand,
    ErasureParticipant, ParticipantCompletionFact, ParticipantCompletionOutcome,
    ParticipantCompletionStatus, ParticipantDemandOutcome, ParticipantErasureScope,
    ParticipantHoldClass, ParticipantOwnerRef, PreservationRepository as _,
};
use ene_primitive::WallClockWithTz;
use ene_store::Store;

use crate::serve::CoreError;

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

#[derive(Default, Clone)]
pub struct ErasureParticipantRegistry {
    participants: HashMap<ParticipantOwnerRef, Arc<dyn ErasureParticipant>>,
    client_transients: Option<Arc<crate::transient_erasure::ClientTransientRegistry>>,
    host_transient: Option<Arc<crate::transient_erasure::HostTransientParticipant>>,
}

impl ErasureParticipantRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

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

    pub(crate) fn install_client_transients(
        &mut self,
        registry: Arc<crate::transient_erasure::ClientTransientRegistry>,
    ) {
        self.client_transients = Some(registry);
    }

    pub(crate) fn register_host_transient(
        &mut self,
        participant: Arc<crate::transient_erasure::HostTransientParticipant>,
    ) -> Result<(), ParticipantOwnerRef> {
        self.register(participant.clone())?;
        self.host_transient = Some(participant);
        Ok(())
    }

    async fn record_completion(
        &self,
        store: &Store,
        fact: ParticipantCompletionFact,
    ) -> Result<ParticipantCompletionOutcome, ene_preservation::PreservationTechnicalError> {
        if fact.participant() == ParticipantOwnerRef::HostTransient
            && fact.status() == ParticipantCompletionStatus::Verified
            && let Some(host) = &self.host_transient
        {
            return host.commit_verified(fact).await;
        }
        store.record_participant_completion(fact).await
    }

    pub async fn demand(&self, command: DemandLocalErasureCommand) -> ParticipantCompletionFact {
        let owner = command.participant();
        if let Some(participant) = self.participants.get(&owner) {
            return participant.demand_local_erasure(command).await;
        }
        if let (ParticipantOwnerRef::ClientIncarnation(identity), Some(registry)) =
            (owner, &self.client_transients)
        {
            let participant = crate::transient_erasure::ClientIncarnationParticipant::new(
                identity,
                Arc::clone(registry),
            );
            return participant.demand_local_erasure(command).await;
        }
        ParticipantCompletionFact::held(
            command.condition(),
            owner,
            ParticipantHoldClass::Unsupported,
            WallClockWithTz::now(),
        )
    }
}

/// Bounded parameters for one fan-out pass.
///
/// A pass never runs unbounded participant work: each unfinished participant
/// receives at most [`Self::demands_per_participant`] bounded demands, and at
/// most [`Self::operation_limit`] unfinished operations are examined. A
/// participant still reporting more work is left unfinished for the next pass.
///
/// The examined window starts after the fan-out walk's durable cursor and
/// advances past the last operation the pass examined, so an unfinished set
/// larger than one pass is rotated instead of truncated: the operations past
/// the window are reached by later passes when the walk wraps. The cursor is
/// scheduling state only; it never substitutes for the durable phase,
/// participant, and completion premises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetedDeletionPass {
    pub operation_limit: u32,
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
/// for the pass's own continuation decision and tests; they are never a
/// completion decision (the durable participant aggregate plus the
/// system-wide remainder probe are, and only the sealed completion boundary
/// re-derives them).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TargetedDeletionPassOutcome {
    pub operations: u32,
    pub demands: u32,
    pub verified: u32,
    /// Operations that entered the durable `Finalizing` marker during this
    /// pass (including a resumed finalizing operation observed again).
    pub finalizing: u32,
    pub finalized: u32,
    pub remainder_sweeps: u32,
    pub reconciliation_pages: u32,
}

impl TargetedDeletionPassOutcome {
    /// Whether one pass advanced durable work: it demanded a participant,
    /// verified one, moved an operation into `Finalizing`, committed a
    /// completion, opened a remainder sweep, or published a covered-source
    /// reconciliation page.
    ///
    /// Counters only, and deliberately never a completion decision. A recorded
    /// hold is not progress: the operation waits for a resume and the same
    /// bounded driver must not keep hammering it (lifecycle §5.1).
    #[must_use]
    pub(crate) fn progressed(self) -> bool {
        self.demands > 0
            || self.verified > 0
            || self.finalizing > 0
            || self.finalized > 0
            || self.remainder_sweeps > 0
            || self.reconciliation_pages > 0
    }

    pub(crate) fn accumulate(&mut self, other: Self) {
        self.operations = self.operations.saturating_add(other.operations);
        self.demands = self.demands.saturating_add(other.demands);
        self.verified = self.verified.saturating_add(other.verified);
        self.finalizing = self.finalizing.saturating_add(other.finalizing);
        self.finalized = self.finalized.saturating_add(other.finalized);
        self.remainder_sweeps = self.remainder_sweeps.saturating_add(other.remainder_sweeps);
        self.reconciliation_pages = self
            .reconciliation_pages
            .saturating_add(other.reconciliation_pages);
    }
}

pub(crate) const BOUNDED_DRIVE_PASS_BUDGET: u32 = 8;

const HELD_RETRY_MAX_SKIP_SHIFT: u32 = 3;

const RECONCILIATION_PAGES_PER_PASS: u32 = 8;

fn valid_pass(pass: TargetedDeletionPass) -> bool {
    (1..=100).contains(&pass.operation_limit) && pass.demands_per_participant > 0
}

/// Whether one durable unfinished record is a hold this Host may retry.
/// Only `Held(Unavailable)` is retryable: `GenerationExhausted` cannot resume
/// by construction and every non-hold phase is not a hold.
fn is_retryable_hold(
    phase: DeletionOperationPhase,
    hold: Option<ene_preservation::DeletionHoldReason>,
) -> bool {
    phase == DeletionOperationPhase::Held
        && hold == Some(ene_preservation::DeletionHoldReason::Unavailable)
}

fn deletion_error(error: ene_preservation::PreservationTechnicalError) -> CoreError {
    CoreError::Deletion(error.to_string())
}

fn command_scope(
    material: &DeletionOperationMaterial,
    owner: ParticipantOwnerRef,
) -> ParticipantErasureScope {
    if owner.is_incarnation() {
        ParticipantErasureScope::correlation_only()
    } else {
        ParticipantErasureScope::local(material.target().clone())
    }
}

/// A durable keyset rotation over the unfinished-operation set.
///
/// The stored position only chooses where a bounded pass starts reading; it is
/// never completion, verification, or hold truth, and every visit re-derives
/// the operation phase, hold, sweep, and participant status from the canonical
/// rows. The walk advances to the last operation a pass examined, so a restart
/// resumes after it instead of stampeding the head, and it wraps to the
/// beginning when a page reaches the end of the unfinished set (a short page,
/// or an empty page at a non-start position), so the tail is reached within
/// one lap even while the head stays unfinished.
struct UnfinishedWalk {
    walk: DeletionWalk,
    /// The position the next page starts after, in memory.
    after: Option<DeletionOperationId>,
    /// The position last durably written (or read at open). A pass persists
    /// its position only when it moved, so an idle pass performs no durable
    /// mutation.
    persisted: Option<DeletionOperationId>,
}

impl UnfinishedWalk {
    async fn open(store: &Store, walk: DeletionWalk) -> Result<Self, CoreError> {
        let after = store
            .deletion_walk_cursor(walk)
            .await
            .map_err(deletion_error)?;
        Ok(Self {
            walk,
            after,
            persisted: after,
        })
    }

    /// Reads the next bounded page after the current position. An empty page
    /// at a non-start position is the end of the unfinished set: the walk
    /// wraps and returns the page from the beginning in the same pass, so an
    /// emptied tail does not cost a whole idle pass.
    async fn page(
        &mut self,
        store: &Store,
        limit: u32,
    ) -> Result<Vec<DeletionOperationRecord>, CoreError> {
        let page = store
            .unfinished_deletions(self.after, limit)
            .await
            .map_err(deletion_error)?;
        if !page.is_empty() || self.after.is_none() {
            return Ok(page);
        }
        self.after = None;
        store
            .unfinished_deletions(None, limit)
            .await
            .map_err(deletion_error)
    }

    /// Moves the in-memory position past one examined operation.
    fn examined(&mut self, operation: DeletionOperationId) {
        self.after = Some(operation);
    }

    /// Persists the position after a pass. `end` means the pass reached the
    /// end of the unfinished set (a short page), which wraps the next pass to
    /// the head.
    ///
    /// A crash between the last examined operation and this write repeats that
    /// page on the next pass (idempotent work, never a skip); a pass that
    /// fails before this call writes nothing.
    async fn advance(&mut self, store: &Store, end: bool) -> Result<(), CoreError> {
        if end {
            self.after = None;
        }
        if self.after == self.persisted {
            return Ok(());
        }
        store
            .set_deletion_walk_cursor(self.walk, self.after)
            .await
            .map_err(deletion_error)?;
        self.persisted = self.after;
        Ok(())
    }
}

/// Drives one bounded fan-out pass over the durable unfinished operations.
///
/// `Active` operations are driven through their required participants and then
/// through the sealed completion boundary once the durable aggregate says
/// every participant verified. A `Held` operation waits for an explicit resume
/// decision. A `Finalizing` operation is resumed directly at the completion
/// boundary: the durable marker means local erasure and current-sweep
/// verification are complete and only the material wipe / audit / condition
/// closure commit is owed (§12/§14). Participants already `Verified` for the
/// current sweep are never demanded again, so a fan-out that crashed after a
/// participant's semantic effect continues with only the unfinished
/// participants (§14). No caller boolean exists anywhere on this path: the
/// completion premise is re-derived by the store inside its own write
/// transaction.
///
/// The pass reads one bounded window of unfinished operations after the
/// fan-out walk's durable cursor and advances the cursor past the operations
/// it examined (wrapping at the end of the set). The cursor bounds where a
/// pass looks, never what it believes: an unfinished operation on a later page
/// is deferred to the next pass instead of being permanently starved by a
/// stuck head page.
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
    if !valid_pass(pass) {
        return Err(CoreError::Deletion(String::from(
            "invalid targeted deletion pass parameters",
        )));
    }
    if let Some(host) = &registry.host_transient {
        host.publish_owed_arrivals().await;
    }
    let mut outcome = TargetedDeletionPassOutcome::default();
    let mut walk = UnfinishedWalk::open(store, DeletionWalk::FanOut).await?;
    let mut reached_end = false;
    while (outcome.operations as usize) < pass.operation_limit as usize {
        let remaining = pass.operation_limit - outcome.operations;
        let limit = remaining.min(100);
        let page = walk.page(store, limit).await?;
        if page.is_empty() {
            reached_end = true;
            break;
        }
        let page_len = page.len();
        for record in page {
            walk.examined(record.current.operation);
            outcome.operations += 1;
            match record.phase {
                DeletionOperationPhase::Active => {
                    let progressed = drive_operation(
                        store,
                        registry,
                        record.current,
                        pass.demands_per_participant,
                        &mut outcome,
                    )
                    .await?;
                    if !progressed {
                        break;
                    }
                }
                DeletionOperationPhase::Finalizing => {
                    settle_finalizing(store, registry, record.current, &mut outcome).await?;
                }
                DeletionOperationPhase::Held | DeletionOperationPhase::Completed => {}
            }
        }
        if page_len < limit as usize {
            reached_end = true;
            break;
        }
    }
    walk.advance(store, reached_end).await?;
    Ok(outcome)
}

async fn settle_finalizing(
    store: &Store,
    registry: &ErasureParticipantRegistry,
    current: DeletionOperationRef,
    outcome: &mut TargetedDeletionPassOutcome,
) -> Result<(), CoreError> {
    #[cfg(any(test, feature = "test-support"))]
    store.pause_deletion_finalizing_if_armed_for_tests().await;
    let _arrival_gate = if let Some(host) = &registry.host_transient {
        let gate = host.lock_arrival().await;
        host.publish_owed_arrivals_locked().await;
        if host.unpublished_blocks_finalizing(current).await {
            return Ok(());
        }
        Some(gate)
    } else {
        None
    };
    match store
        .begin_deletion_finalizing(current)
        .await
        .map_err(deletion_error)?
    {
        DeletionFinalizationOutcome::Finalizing => {
            outcome.finalizing += 1;
        }
        DeletionFinalizationOutcome::CompletedAlready => return Ok(()),
        DeletionFinalizationOutcome::RemainderCollected(_) => {
            outcome.remainder_sweeps += 1;
            return Ok(());
        }
        DeletionFinalizationOutcome::NotVerified(_)
        | DeletionFinalizationOutcome::ReconciliationIncomplete
        | DeletionFinalizationOutcome::NotFinalizing
        | DeletionFinalizationOutcome::Held(_)
        | DeletionFinalizationOutcome::Missing
        | DeletionFinalizationOutcome::StaleSweep
        | DeletionFinalizationOutcome::UnverifiableMaterial
        | DeletionFinalizationOutcome::Completed => return Ok(()),
    }
    match store
        .complete_deletion_finalizing(current)
        .await
        .map_err(deletion_error)?
    {
        DeletionFinalizationOutcome::Completed => outcome.finalized += 1,
        DeletionFinalizationOutcome::RemainderCollected(_) => outcome.remainder_sweeps += 1,
        DeletionFinalizationOutcome::NotVerified(_)
        | DeletionFinalizationOutcome::ReconciliationIncomplete
        | DeletionFinalizationOutcome::NotFinalizing
        | DeletionFinalizationOutcome::Held(_)
        | DeletionFinalizationOutcome::Missing
        | DeletionFinalizationOutcome::StaleSweep
        | DeletionFinalizationOutcome::UnverifiableMaterial
        | DeletionFinalizationOutcome::CompletedAlready
        | DeletionFinalizationOutcome::Finalizing => {}
    }
    Ok(())
}

async fn drive_operation(
    store: &Store,
    registry: &ErasureParticipantRegistry,
    current: DeletionOperationRef,
    demand_budget: u32,
    outcome: &mut TargetedDeletionPassOutcome,
) -> Result<bool, CoreError> {
    let condition = current.condition();
    let mut advanced_any = false;
    let mut reconciled = false;
    for _ in 0..RECONCILIATION_PAGES_PER_PASS {
        match store
            .reconcile_deletion_sources(current, DELETION_RECONCILIATION_PAGE_SIZE)
            .await
            .map_err(deletion_error)?
        {
            DeletionReconciliationOutcome::Advanced => {
                outcome.reconciliation_pages += 1;
                advanced_any = true;
            }
            DeletionReconciliationOutcome::Complete => {
                reconciled = true;
                break;
            }
            DeletionReconciliationOutcome::Finalizing
            | DeletionReconciliationOutcome::Completed => return Ok(advanced_any),
            DeletionReconciliationOutcome::Missing | DeletionReconciliationOutcome::StaleSweep => {
                return Ok(advanced_any);
            }
        }
    }
    if !reconciled {
        return Ok(advanced_any);
    }
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
                    | ParticipantDemandOutcome::NotRequired => return Ok(false),
                }
                outcome.demands += 1;
                let material = match store
                    .deletion_operation_material(current.operation)
                    .await
                    .map_err(deletion_error)?
                {
                    DeletionMaterialOutcome::Material(material) => material,
                    DeletionMaterialOutcome::Missing | DeletionMaterialOutcome::Destroyed => {
                        return Ok(false);
                    }
                };
                let command = DemandLocalErasureCommand::new(
                    condition,
                    record.participant.owner,
                    command_scope(&material, record.participant.owner),
                );
                let fact = registry.demand(command).await;
                if fact.condition() != condition || fact.participant() != record.participant.owner {
                    return Ok(false);
                }
                let fact_status = fact.status();
                match registry
                    .record_completion(store, fact)
                    .await
                    .map_err(deletion_error)?
                {
                    ParticipantCompletionOutcome::Recorded(progress) => {
                        if progress.is_verified() {
                            verified = true;
                            break;
                        }
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
                    | ParticipantCompletionOutcome::NotRequired => return Ok(false),
                }
            }
            if verified {
                outcome.verified += 1;
            }
            if held_this_participant {
                held = true;
            }
        }
        if page_len < 100 {
            break;
        }
    }
    if held {
        store
            .change_deletion_lifecycle(current, DeletionLifecycleChange::Hold)
            .await
            .map_err(deletion_error)?;
        return Ok(true);
    }
    settle_finalizing(store, registry, current, outcome).await?;
    Ok(true)
}

/// Drives bounded passes until a pass advances no durable work or the budget
/// is exhausted.
///
/// This is the continuation driver behind startup recovery and the post-
/// confirmation kick: each iteration is one [`drive_targeted_deletion`] pass
/// with its own bounded operation/demand limits, and the loop stops as soon as
/// a pass reports no demand, verification, finalizing step, completion,
/// remainder sweep, or published reconciliation page. It never decides
/// completion itself — the sealed store
/// boundary re-derives every premise on each pass.
///
/// # Errors
///
/// Returns [`CoreError::Deletion`] for an invalid pass or budget and when the
/// canonical store refuses.
pub(crate) async fn drive_targeted_deletion_until_settled(
    store: &Store,
    registry: &ErasureParticipantRegistry,
    pass: TargetedDeletionPass,
    pass_budget: u32,
) -> Result<TargetedDeletionPassOutcome, CoreError> {
    if !valid_pass(pass) || pass_budget == 0 {
        return Err(CoreError::Deletion(String::from(
            "invalid targeted deletion drive parameters",
        )));
    }
    let mut total = TargetedDeletionPassOutcome::default();
    for _ in 0..pass_budget {
        let outcome = drive_targeted_deletion(store, registry, pass).await?;
        let progressed = outcome.progressed();
        total.accumulate(outcome);
        if !progressed {
            break;
        }
    }
    Ok(total)
}

pub(crate) async fn recover_targeted_deletions(
    store: &Store,
    registry: &ErasureParticipantRegistry,
    pass: TargetedDeletionPass,
    pass_budget: u32,
) -> Result<TargetedDeletionPassOutcome, CoreError> {
    if !valid_pass(pass) || pass_budget == 0 {
        return Err(CoreError::Deletion(String::from(
            "invalid targeted deletion recovery parameters",
        )));
    }
    // Startup recovery is a fresh schedule: the empty retry map makes every
    // retryable hold eligible, so the page is offered whole.
    resume_hold_page(
        store,
        pass.operation_limit,
        &mut HeldRetrySchedule::new(),
        1,
    )
    .await?;
    drive_targeted_deletion_until_settled(store, registry, pass, pass_budget).await
}

/// Offers one bounded page of retryable-hold resumes.
///
/// Only `Held(Unavailable)` is a resume candidate. `GenerationExhausted`
/// cannot be resumed by construction, every other phase is not a hold, and
/// the canonical store re-checks all of that inside its own write
/// transaction; this read only decides which candidates to offer.
///
/// The page is the next window of the retryable-hold walk (the same durable
/// rotation as the fan-out pass), so a hold page blocked by earlier
/// non-retryable holds or other unfinished operations is still reached on a
/// later call instead of being starved. The walk position is scheduling state
/// only: every offered resume re-derives its premise in the store's write
/// transaction. `schedule`/`tick` are pacing only.
async fn resume_hold_page(
    store: &Store,
    limit: u32,
    schedule: &mut HeldRetrySchedule,
    tick: u64,
) -> Result<(), CoreError> {
    let mut walk = UnfinishedWalk::open(store, DeletionWalk::RetryableHold).await?;
    let page = walk.page(store, limit).await?;
    let page_len = page.len();
    let retryable: HashSet<DeletionOperationId> = page
        .iter()
        .filter(|record| is_retryable_hold(record.phase, record.hold))
        .map(|record| record.current.operation)
        .collect();
    schedule.retain_only(&retryable);
    for record in &page {
        walk.examined(record.current.operation);
        if !retryable.contains(&record.current.operation)
            || !schedule.eligible(record.current.operation, tick)
        {
            continue;
        }
        // The store re-checks the phase, sweep, and hold class inside its
        // write transaction; a refusal (another writer moved the operation, or
        // the durable state cannot resume) leaves it for the bounded drive
        // below without inventing an outcome here.
        match store
            .change_deletion_lifecycle(record.current, DeletionLifecycleChange::Resume)
            .await
            .map_err(deletion_error)?
        {
            DeletionLifecycleOutcome::Applied(_) => {
                schedule.record_resume(record.current.operation, tick);
            }
            // The durable state moved or refuses the resume; the next pass
            // re-reads the phase instead of guessing.
            DeletionLifecycleOutcome::Missing
            | DeletionLifecycleOutcome::StaleSweep
            | DeletionLifecycleOutcome::Completed
            | DeletionLifecycleOutcome::Held(_)
            | DeletionLifecycleOutcome::Finalizing => {}
        }
    }
    walk.advance(store, page_len < limit as usize).await?;
    Ok(())
}

/// In-memory pacing for retrying `Held(Unavailable)` operations from the
/// serving tick.
///
/// The driver retries a retryable hold by resuming it and driving the reopened
/// operation; each consecutive unanswered retry doubles the number of ticks
/// before the next attempt, up to `2^`[`HELD_RETRY_MAX_SKIP_SHIFT`] ticks. The
/// schedule is pacing only and never authority: it decides no phase, stores no
/// deletion condition, converts no hold into a completion, and a restart drops
/// it (startup recovery resumes independently of it). Pruning entries that are
/// no longer held keeps the map bounded by the unfinished-hold page.
#[derive(Default)]
pub(crate) struct HeldRetrySchedule {
    tick: u64,
    retries: HashMap<DeletionOperationId, HoldRetry>,
}

#[derive(Clone, Copy)]
struct HoldRetry {
    attempts: u32,
    next_eligible_tick: u64,
}

impl HeldRetrySchedule {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn next_tick(&mut self) -> u64 {
        self.tick = self.tick.saturating_add(1);
        self.tick
    }

    fn eligible(&self, operation: DeletionOperationId, tick: u64) -> bool {
        self.retries
            .get(&operation)
            .is_none_or(|retry| tick >= retry.next_eligible_tick)
    }

    fn record_resume(&mut self, operation: DeletionOperationId, tick: u64) {
        let attempts = self
            .retries
            .get(&operation)
            .map_or(0, |retry| retry.attempts)
            .saturating_add(1);
        let shift = attempts.saturating_sub(1).min(HELD_RETRY_MAX_SKIP_SHIFT);
        let skip = 1_u64 << shift;
        self.retries.insert(
            operation,
            HoldRetry {
                attempts,
                next_eligible_tick: tick.saturating_add(skip),
            },
        );
    }

    fn retain_only(&mut self, held: &HashSet<DeletionOperationId>) {
        self.retries.retain(|operation, _| held.contains(operation));
    }
}

/// One bounded serving tick: at most one backed-off resume per retryable hold
/// plus exactly one fan-out pass.
///
/// Holds are read from the durable phase; the schedule bounds how often a hold
/// is retried, and a resume is offered only to `Held(Unavailable)` operations.
/// `GenerationExhausted` is never retried (fail closed). The pass itself is
/// [`drive_targeted_deletion`], so a tick can never run unbounded participant
/// work and can never complete an operation without the sealed store
/// boundary re-deriving every premise.
///
/// The hold page is the next window of the retryable-hold walk, so a tick
/// examines a bounded page while successive ticks rotate through a hold set
/// larger than one page instead of re-reading the head. Because the in-memory
/// schedule only sees the window read by its tick, its backoff bounds
/// consecutive attempts within a lap; a hold is still offered at most one
/// resume per lap while the set exceeds one page, and the schedule is pacing
/// only, never authority.
///
/// # Errors
///
/// Returns [`CoreError::Deletion`] for an invalid pass and when the canonical
/// store refuses.
pub(crate) async fn tick_targeted_deletion(
    store: &Store,
    registry: &ErasureParticipantRegistry,
    pass: TargetedDeletionPass,
    schedule: &mut HeldRetrySchedule,
) -> Result<TargetedDeletionPassOutcome, CoreError> {
    if !valid_pass(pass) {
        return Err(CoreError::Deletion(String::from(
            "invalid targeted deletion pass parameters",
        )));
    }
    let tick = schedule.next_tick();
    resume_hold_page(store, pass.operation_limit, schedule, tick).await?;
    drive_targeted_deletion(store, registry, pass).await
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
        ConfirmTargetedDeletionOutcome, DeletionFinalizationOutcome, DeletionHoldReason,
        DeletionMaterialOutcome, DeletionOperationId, DeletionOperationPhase,
        DeletionOperationRecord, DeletionOperationRef, DeletionPurpose, DeletionSearchMaterial,
        DeletionSweepGeneration, ErasureParticipant, MechanicalDeletionTarget,
        ParticipantCompletionFact, ParticipantCompletionStatus, ParticipantHoldClass,
        ParticipantOwnerRef, ParticipantProgress, StageTargetedDeletionRequestCommand,
        StageTargetedDeletionRequestOutcome, TargetedDeletionTarget,
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
        orchestrate_task_agent_turn,
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

    /// Test fixture admission: the production stage/confirm path, then the
    /// current sweep's bounded source walk driven to completion, matching the
    /// durable shape the deleted direct admission seam committed. Tests that
    /// pin the admission page itself use [`admit_first_party`] instead.
    async fn admit(
        handle: &HostHandle,
        text: &str,
        participants: Vec<ParticipantOwnerRef>,
    ) -> DeletionOperationRef {
        let current = admit_first_party(handle, text, participants).await;
        for _ in 0..64 {
            match handle
                .store
                .reconcile_deletion_sources(current, DELETION_RECONCILIATION_PAGE_SIZE)
                .await
                .expect("a reconciliation step must answer")
            {
                DeletionReconciliationOutcome::Advanced => {}
                DeletionReconciliationOutcome::Complete
                | DeletionReconciliationOutcome::Finalizing
                | DeletionReconciliationOutcome::Completed => break,
                other => panic!("unexpected reconciliation step: {other:?}"),
            }
        }
        current
    }

    /// First-party request/confirmation: admission publishes only the first
    /// bounded identity page and leaves the durable reconciliation cursor
    /// incomplete, so page-late coverage stays observable.
    async fn admit_first_party(
        handle: &HostHandle,
        text: &str,
        participants: Vec<ParticipantOwnerRef>,
    ) -> DeletionOperationRef {
        let staged = handle
            .store
            .stage_targeted_deletion(StageTargetedDeletionRequestCommand::new(
                TargetedDeletionTarget {
                    mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                        text.to_owned(),
                    )),
                    semantic_hints: Vec::new(),
                },
                DeletionPurpose::Privacy,
                WallClockWithTz::now(),
            ))
            .await
            .expect("staging must answer");
        let request = match staged {
            StageTargetedDeletionRequestOutcome::Staged(request) => request,
            other => panic!("the scope must stage, got {other:?}"),
        };
        match handle
            .store
            .confirm_targeted_deletion(request, participants)
            .await
            .expect("the confirmation must answer")
        {
            ConfirmTargetedDeletionOutcome::Started(current) => current,
            other => panic!("the confirmation must start, got {other:?}"),
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
                claim: None,
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

    /// Runs bounded fan-out passes until nothing is left to demand,
    /// accumulating the counters.
    ///
    /// A global completion runs inside the pass that verified the last
    /// participant, so the pass that settles (zero demands) may itself be
    /// empty; the accumulation keeps the completion observable.
    async fn drive_until_settled(handle: &HostHandle) -> TargetedDeletionPassOutcome {
        let mut total = TargetedDeletionPassOutcome::default();
        for _ in 0..64 {
            let outcome = handle
                .drive_targeted_deletion(TargetedDeletionPass::new(100, 16))
                .await
                .expect("the fan-out must not fail");
            let settled = outcome.demands == 0 && outcome.reconciliation_pages == 0;
            total.accumulate(outcome);
            if settled {
                return total;
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
        drive_until_settled(&handle).await;
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

    /// Blocker 2: a paraphrase Summary whose History pin sits past the
    /// admission/reconciliation page must still be erased by the same sweep.
    /// Page size is a work bound. The driver re-reads canonical coverage
    /// after reconciliation Complete before Learning fan-out, so semantic
    /// derived data cannot survive because the pin's source was published
    /// after the admission page.
    #[tokio::test]
    async fn reconciliation_then_fan_out_erases_paraphrase_pinned_past_the_page() {
        use ene_companion::{CompanionRepository as _, HistoryRepository as _, HistoryRole};
        use ene_learning::{ChangeKind, LearningRepository as _, MemoryId, MemoryTarget};
        use ene_presence::PresenceRepository as _;

        let Some((handle, _dir)) = memory_handle("targeted-deletion-page-pin").await else {
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
        let target = "page-late-pin-canary";
        let mut sources = Vec::new();
        for index in 0..(DELETION_RECONCILIATION_PAGE_SIZE + 6) {
            sources.push(
                append_history(
                    &handle,
                    companion,
                    generation,
                    HistoryRole::Owner,
                    &format!("note {index} carries {target}"),
                )
                .await,
            );
        }
        let late = *sources
            .iter()
            .max_by_key(|source| source.as_uuid())
            .expect("the fixture has sources");
        let paraphrase = summary_of(
            companion.as_raw(),
            String::from("The owner keeps a private launch credential."),
            late,
            late,
        );
        let memory = MemoryId::generate();
        commit_learning(
            &handle,
            &paraphrase,
            MemoryTarget::New { id: memory },
            "The owner keeps a private launch credential.",
            ChangeKind::Initial,
        )
        .await;
        let unrelated = append_history(
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
            unrelated,
            unrelated,
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

        let current = admit_first_party(
            &handle,
            target,
            vec![
                ParticipantOwnerRef::Companion,
                ParticipantOwnerRef::Learning,
            ],
        )
        .await;
        let DeletionMaterialOutcome::Material(_) = handle
            .store
            .deletion_operation_material(current.operation)
            .await
            .expect("the protected material must read")
        else {
            panic!("an unfinished operation keeps its material");
        };
        let outcome = drive_until_settled(&handle).await;
        assert!(
            outcome.reconciliation_pages >= 1,
            "the walk needed a continuation page: {outcome:?}"
        );
        assert_eq!(
            handle
                .store
                .count_exact_text_remainder_for_tests(target)
                .await
                .unwrap(),
            0
        );

        let timeline = handle
            .store
            .load_timeline(companion, None, None, 200)
            .await
            .expect("the timeline must load");
        assert!(timeline.iter().all(|item| !item.text.contains(target)));
        assert!(
            timeline.iter().any(|item| item.id == unrelated),
            "unrelated History stays"
        );
        assert!(
            timeline.iter().all(|item| item.id != late),
            "the late covered History source is erased"
        );
        let summaries = handle
            .store
            .load_summaries(&[paraphrase.id, unrelated_summary.id])
            .await
            .expect("summaries must load");
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, unrelated_summary.id);
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
                .is_empty()
        );
        let recalled = handle
            .store
            .recall_candidates(companion.as_raw(), &[String::from("launch")], 50)
            .await
            .expect("recall must answer");
        assert!(recalled.iter().all(|item| item.id != memory));
        assert_eq!(
            operation_record(&handle, current.operation).await.phase,
            DeletionOperationPhase::Completed
        );

        let fresh = append_history(
            &handle,
            companion,
            generation,
            HistoryRole::Owner,
            &format!("a fresh note about {target}"),
        )
        .await;
        let timeline = handle
            .store
            .load_timeline(companion, None, None, 200)
            .await
            .expect("the timeline must load");
        assert!(
            timeline
                .iter()
                .any(|item| item.id == fresh && item.text.contains(target)),
            "a post-completion Owner origin of the same string is accepted"
        );
    }

    /// Blocker 3: a demand parked after it was current, while another driver
    /// completes the operation and the Owner re-provides the same string,
    /// must return NotCurrent and leave the fresh body byte-identical.
    #[tokio::test]
    async fn a_stale_hosted_erase_does_not_mutate_fresh_post_completion_history() {
        use ene_companion::{CompanionRepository as _, HistoryRepository as _, HistoryRole};
        use ene_presence::PresenceRepository as _;
        use ene_store::CompanionErasureParticipant;

        let Some((handle, _dir)) = memory_handle("targeted-deletion-stale-erase").await else {
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
        let target = "hosted-stale-erase-canary";
        let old = append_history(
            &handle,
            companion,
            generation,
            HistoryRole::Owner,
            &format!("old copy of {target}"),
        )
        .await;
        let current = admit(&handle, target, vec![ParticipantOwnerRef::Companion]).await;

        handle.store.arm_erasure_mutation_park_for_tests();
        let parked = {
            let store = handle.store.clone();
            let condition = current.condition();
            tokio::spawn(async move {
                let material = match store
                    .deletion_operation_material(current.operation)
                    .await
                    .expect("the material must read")
                {
                    DeletionMaterialOutcome::Material(material) => material,
                    other => panic!("the protected material must read, got {other:?}"),
                };
                CompanionErasureParticipant::new(store)
                    .demand_local_erasure(DemandLocalErasureCommand::new(
                        condition,
                        ParticipantOwnerRef::Companion,
                        command_scope(&material, ParticipantOwnerRef::Companion),
                    ))
                    .await
            })
        };
        handle.store.wait_erasure_mutation_park_for_tests().await;

        drive_until_settled(&handle).await;
        assert_eq!(
            handle
                .store
                .count_exact_text_remainder_for_tests(target)
                .await
                .unwrap(),
            0
        );

        let fresh_body = format!("fresh origin of {target}");
        let fresh = append_history(
            &handle,
            companion,
            generation,
            HistoryRole::Owner,
            &fresh_body,
        )
        .await;

        handle.store.release_erasure_mutation_park_for_tests();
        let stale = parked.await.expect("the parked demand joins");
        assert_eq!(
            stale.status(),
            ParticipantCompletionStatus::LocalComplete,
            "a completed condition is NotCurrent, never Verified"
        );
        assert_eq!(stale.erased_count(), 0);

        let timeline = handle
            .store
            .load_timeline(companion, None, None, 100)
            .await
            .expect("the timeline must load");
        let fresh_row = timeline
            .iter()
            .find(|item| item.id == fresh)
            .expect("the fresh origin must remain");
        assert_eq!(fresh_row.text, fresh_body);
        assert!(timeline.iter().all(|item| item.id != old));
        assert_eq!(
            operation_record(&handle, current.operation).await.phase,
            DeletionOperationPhase::Completed
        );
    }

    /// Blocker 3 remainder: Host-transient process memory cannot share the
    /// Immediate writer. A demand parked after currentness, while another
    /// driver Completes and the Owner re-queues the same string, must not
    /// drop the fresh premise.
    #[tokio::test]
    async fn a_stale_host_transient_demand_does_not_drop_fresh_post_completion_premises() {
        use crate::transient_erasure::HostTransientParticipant;
        use ene_learning::{
            ExperienceCandidate, ExperienceRole, ExperienceSourceKind, ExperienceTurn,
            SourceRangeRef,
        };

        let Some((handle, _dir)) = memory_handle("targeted-deletion-stale-transient").await else {
            panic!("the host must open");
        };
        let target = "host-transient-stale-canary";
        let old = ExperienceCandidate {
            companion: RawId::new(),
            source: SourceRangeRef {
                kind: ExperienceSourceKind::Dialogue,
                start: RawId::new(),
                end: RawId::new(),
            },
            sources: Vec::new(),
            transcript: vec![ExperienceTurn {
                role: ExperienceRole::Owner,
                text: format!("old copy of {target}"),
                at: None,
            }],
            at: WallClockWithTz::now(),
        };
        crate::lock_unpoison(&handle.learning_queue).push_back(old);

        let current = admit(&handle, target, vec![ParticipantOwnerRef::HostTransient]).await;
        handle.store.arm_erasure_mutation_park_for_tests();
        let parked = {
            let store = handle.store.clone();
            let fence = Arc::clone(&handle.transient_fence);
            let presentations = Arc::clone(&handle.presentations);
            let queue = Arc::clone(&handle.learning_queue);
            let arrival = Arc::clone(&handle.host_transient_arrival);
            let condition = current.condition();
            tokio::spawn(async move {
                let material = match store
                    .deletion_operation_material(current.operation)
                    .await
                    .expect("the material must read")
                {
                    DeletionMaterialOutcome::Material(material) => material,
                    other => panic!("the protected material must read, got {other:?}"),
                };
                HostTransientParticipant::new(store, fence, presentations, queue, arrival)
                    .demand_local_erasure(DemandLocalErasureCommand::new(
                        condition,
                        ParticipantOwnerRef::HostTransient,
                        command_scope(&material, ParticipantOwnerRef::HostTransient),
                    ))
                    .await
            })
        };
        handle.store.wait_erasure_mutation_park_for_tests().await;

        drive_until_settled(&handle).await;
        assert_eq!(
            operation_record(&handle, current.operation).await.phase,
            DeletionOperationPhase::Completed
        );

        let fresh_text = format!("fresh origin of {target}");
        let fresh = ExperienceCandidate {
            companion: RawId::new(),
            source: SourceRangeRef {
                kind: ExperienceSourceKind::Dialogue,
                start: RawId::new(),
                end: RawId::new(),
            },
            sources: Vec::new(),
            transcript: vec![ExperienceTurn {
                role: ExperienceRole::Owner,
                text: fresh_text.clone(),
                at: None,
            }],
            at: WallClockWithTz::now(),
        };
        crate::lock_unpoison(&handle.learning_queue).push_back(fresh);

        handle.store.release_erasure_mutation_park_for_tests();
        let stale = parked.await.expect("the parked demand joins");
        assert_eq!(
            stale.status(),
            ParticipantCompletionStatus::LocalComplete,
            "a completed condition is NotCurrent, never Verified"
        );

        let queue = crate::lock_unpoison(&handle.learning_queue);
        assert_eq!(queue.len(), 1, "the fresh premise must remain");
        assert_eq!(
            queue
                .front()
                .expect("the fresh premise must remain")
                .transcript[0]
                .text,
            fresh_text
        );
    }

    /// Blocker 3 remainder: the credential device-auth file cannot share the
    /// metadata transaction. A demand parked after the DB pass, while another
    /// driver Completes and the Owner saves a fresh entry of the same string,
    /// must leave that entry byte-for-byte.
    #[tokio::test]
    async fn a_stale_credential_file_erase_does_not_drop_fresh_device_auth() {
        use ene_credential::{CredentialErasureParticipant, DeviceId};

        let Some((handle, _dir)) = memory_handle("targeted-deletion-stale-device-auth").await
        else {
            panic!("the host must open");
        };
        let target = "device-auth-stale-canary";
        let old_device = DeviceId(RawId::new());
        handle
            .auth_store
            .save_secret(&old_device, &format!("phone {target}"), "old-secret")
            .expect("the old device-auth entry saves");

        let current = admit(&handle, target, vec![ParticipantOwnerRef::Credential]).await;
        handle.store.arm_device_auth_file_park_for_tests();
        let parked = {
            let store = handle.store.clone();
            let auth = std::sync::Arc::new(handle.auth_store.clone());
            let condition = current.condition();
            tokio::spawn(async move {
                let material = match store
                    .deletion_operation_material(current.operation)
                    .await
                    .expect("the material must read")
                {
                    DeletionMaterialOutcome::Material(material) => material,
                    other => panic!("the protected material must read, got {other:?}"),
                };
                CredentialErasureParticipant::new(std::sync::Arc::new(store), auth)
                    .demand_local_erasure(DemandLocalErasureCommand::new(
                        condition,
                        ParticipantOwnerRef::Credential,
                        command_scope(&material, ParticipantOwnerRef::Credential),
                    ))
                    .await
            })
        };
        handle.store.wait_device_auth_file_park_for_tests().await;

        drive_until_settled(&handle).await;
        assert_eq!(
            operation_record(&handle, current.operation).await.phase,
            DeletionOperationPhase::Completed
        );

        let fresh_device = DeviceId(RawId::new());
        let fresh_descriptor = format!("laptop {target}");
        handle
            .auth_store
            .save_secret(&fresh_device, &fresh_descriptor, "fresh-secret")
            .expect("the fresh device-auth entry saves");

        handle.store.release_device_auth_file_park_for_tests();
        let stale = parked.await.expect("the parked demand joins");
        assert_eq!(
            stale.status(),
            ParticipantCompletionStatus::LocalComplete,
            "a completed condition is NotCurrent, never Verified"
        );
        assert_eq!(
            handle
                .auth_store
                .count_target_text(target)
                .expect("the file remainder must read"),
            1,
            "the fresh device-auth entry must remain"
        );
        assert!(
            handle
                .auth_store
                .has_secret(&fresh_device)
                .expect("the fresh entry must load"),
            "the fresh device-auth secret must remain"
        );
    }

    /// Coverage stays exhaustive without materializing every source into one
    /// command: a paraphrase pin past many covered identities is still erased,
    /// and the material read stays empty of source ids.
    #[tokio::test]
    async fn a_large_covered_source_set_does_not_unbounded_one_demand() {
        use ene_companion::{CompanionRepository as _, HistoryRole};
        use ene_learning::{ChangeKind, LearningRepository as _, MemoryId, MemoryTarget};
        use ene_presence::PresenceRepository as _;

        let Some((handle, _dir)) = memory_handle("targeted-deletion-many-sources").await else {
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
        let target = "many-sources-pin-canary";
        let mut sources = Vec::new();
        for index in 0..128 {
            sources.push(
                append_history(
                    &handle,
                    companion,
                    generation,
                    HistoryRole::Owner,
                    &format!("note {index} carries {target}"),
                )
                .await,
            );
        }
        let late = *sources
            .iter()
            .max_by_key(|source| source.as_uuid())
            .expect("the fixture has sources");
        let paraphrase = summary_of(
            companion.as_raw(),
            String::from("The owner keeps a private launch credential."),
            late,
            late,
        );
        let memory = MemoryId::generate();
        commit_learning(
            &handle,
            &paraphrase,
            MemoryTarget::New { id: memory },
            "The owner keeps a private launch credential.",
            ChangeKind::Initial,
        )
        .await;

        let current = admit_first_party(
            &handle,
            target,
            vec![
                ParticipantOwnerRef::Companion,
                ParticipantOwnerRef::Learning,
            ],
        )
        .await;
        let DeletionMaterialOutcome::Material(_) = handle
            .store
            .deletion_operation_material(current.operation)
            .await
            .expect("the protected material must read")
        else {
            panic!("an unfinished operation keeps its material");
        };

        drive_until_settled(&handle).await;
        assert_eq!(
            handle
                .store
                .count_exact_text_remainder_for_tests(target)
                .await
                .unwrap(),
            0
        );
        let summaries = handle
            .store
            .load_summaries(&[paraphrase.id])
            .await
            .expect("summaries must load");
        assert!(
            summaries.is_empty(),
            "the paraphrase pin past the large set is still erased"
        );
        let memories = handle
            .store
            .list_current_memories(companion.as_raw(), None, 100)
            .await
            .expect("memories must list");
        assert!(memories.iter().all(|item| item.id != memory));
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
        handle
            .drive_targeted_deletion(TargetedDeletionPass::new(100, 1))
            .await
            .expect("the fan-out must run");

        // Restart: the durable operation and participant snapshot survive, the
        // in-memory continuation cursors do not, and the reopened composition
        // re-registers the built-in implementations.
        drop(handle);
        let reopened = reopen(dir.path()).await;
        drive_until_settled(&reopened).await;
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
        let required = handle
            .required_deletion_participants()
            .await
            .expect("the required snapshot must read");
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
        assert_eq!(
            reopened
                .required_deletion_participants()
                .await
                .expect("the reopened snapshot must read"),
            required
        );
    }

    #[tokio::test]
    async fn an_unimplemented_owner_holds_the_operation_and_is_never_a_completion() {
        let Some((handle, _dir)) = memory_handle("targeted-deletion-unsupported").await else {
            panic!("the host must open");
        };
        // The demanded owner is one no composition registers: the registry is
        // cleared, and with no Client plumbing installed a Client-incarnation
        // identity has no implementation either. This test's premise — an
        // owner with no implementation is held, never completed — must not
        // depend on which classes the built-in composition happens to serve.
        handle.reset_deletion_participants_for_tests();
        let required = vec![ParticipantOwnerRef::ClientIncarnation(RawId::new())];
        let current = admit(&handle, "unsupported-target", required.clone()).await;
        let outcome = handle
            .drive_targeted_deletion(TargetedDeletionPass::default())
            .await
            .unwrap();
        assert_eq!(outcome.verified, 0);
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
        assert_eq!(first.verified, 1);
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
        assert!(
            rows.is_empty(),
            "every required participant verified: the pass completes the operation"
        );
        let status = reopened.store.deletion_status(None, 100).await.unwrap();
        let record = status
            .iter()
            .find(|record| record.current.operation == current.operation)
            .expect("the completed operation keeps its identity");
        assert_eq!(record.current, current);
        assert_eq!(record.phase, DeletionOperationPhase::Completed);
        let audit = reopened
            .store
            .deletion_completion_audit(current.operation)
            .await
            .unwrap()
            .expect("the completion audit is durable");
        assert_eq!(audit.sweep_count, 1);
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
    async fn an_empty_registry_reports_an_unsupported_hold_for_the_exact_condition() {
        let registry = ErasureParticipantRegistry::new();
        let condition = ene_preservation::ErasureConditionRef {
            operation: ene_preservation::DeletionOperationId::from_raw(RawId::new()),
            sweep: DeletionSweepGeneration::from_u64(1),
        };
        let fact = registry
            .demand(DemandLocalErasureCommand::new(
                condition,
                ParticipantOwnerRef::Task,
                ParticipantErasureScope::correlation_only(),
            ))
            .await;
        assert_eq!(fact.condition(), condition);
        assert_eq!(fact.participant(), ParticipantOwnerRef::Task);
        assert_eq!(
            fact.status(),
            ParticipantCompletionStatus::Held(ParticipantHoldClass::Unsupported),
            "a missing implementation is an explicit hold, never a completion"
        );
    }

    /// Stage 6 A3d end to end through the real fan-out: the permission,
    /// credential, and presence owners erase their own target-bearing state,
    /// survive a Host restart without a second mutation, and never let the
    /// target text remain in any of their rows (lifecycle §6-§10).
    #[tokio::test]
    async fn a3d_owners_erase_through_the_fan_out_and_restart_without_double_mutation() {
        use ene_companion::CompanionRepository as _;
        use ene_credential::{
            CredentialIntentRepository as _, CredentialRefRepository as _,
            CredentialSetRepository as _, DevicePairingRepository as _, RegistrationFingerprint,
        };
        use ene_permission::{
            IntentFingerprint, IntentOutcome, IntentOutcomeRecord, IntentOutcomeRepository as _,
        };
        use ene_presence::{
            ClientId, ConfirmTransitionOutcome, LiveReachabilityRef, MoveDecision,
            PresenceCheckRef, PresenceRepository as _, PresenceState, ThinMoveReason,
        };
        use rusqlite::params;

        let Some((handle, dir)) = memory_handle("targeted-deletion-a3d").await else {
            panic!("the host must open");
        };
        // One target string exercises all three owners: it is the client
        // identity presence references, and plain text for the control and
        // credential metadata.
        let target_raw = RawId::new();
        let target = target_raw.as_uuid().to_string();
        let client = ClientId::from_raw(target_raw);

        // Permission: a decided intent whose quoted rationale carries it.
        let intent_id = RawId::new().as_uuid().to_string();
        handle
            .store
            .record_intent_outcome(IntentOutcomeRecord {
                fingerprint: IntentFingerprint {
                    intent_id: intent_id.clone(),
                    kind: String::from("assign"),
                    target: String::from("consent:dialogue"),
                    base: String::from("consent-dialogue-none"),
                    rationale_origin: String::from("conversation"),
                    rationale_quote: Some(target.clone()),
                },
                outcome: IntentOutcome::NeedsClarification,
            })
            .await
            .unwrap();
        // Credential: a usable ref, a pending registration, and a paired
        // device, all naming the target.
        let registration_id = RawId::new().as_uuid().to_string();
        let registration_target = format!("credential:acme:{target}");
        let registration = |intent_id: String| RegistrationFingerprint {
            intent_id,
            kind: String::from("register"),
            target: registration_target.clone(),
            base: String::from("consent-dialogue-none"),
            rationale_origin: String::from("management-surface"),
            rationale_quote: None,
        };
        handle
            .store
            .request_registration_with_intent(
                String::from("acme"),
                target.clone(),
                registration(registration_id),
            )
            .await
            .unwrap();
        assert!(
            handle
                .store
                .approve_credential_with_sweep("acme", &target, "a3d-bearer")
                .unwrap(),
            "the fixture ref must become usable"
        );
        assert!(
            handle
                .store
                .request_registration_with_intent(
                    String::from("acme"),
                    String::from("pending-target"),
                    registration(RawId::new().as_uuid().to_string()),
                )
                .await
                .is_ok()
        );
        let pairing = handle
            .store
            .request_pairing(format!("{target} phone"), String::from("conn-a3d"))
            .await
            .unwrap();
        assert!(
            handle
                .store
                .approve_pending(&pairing.pending_id, "conn-a3d")
                .await
                .unwrap()
                .is_some()
        );
        let revision_before = handle.store.current_set_revision().await.unwrap();
        // Presence: Present on the target client through the production path.
        let companion = handle.store.ensure_running_companion().await.unwrap();
        let attribution = handle
            .store
            .load_attribution(companion.as_raw())
            .await
            .unwrap()
            .unwrap();
        let MoveDecision::TransitioningToNew { generation } = handle
            .store
            .compare_and_begin_transition(
                companion.as_raw(),
                PresenceCheckRef {
                    expected_generation: attribution.generation,
                    expected_state: attribution.state,
                    expected_active: attribution.active_client,
                },
                Some(client),
                ThinMoveReason::InitialAttach,
            )
            .await
            .unwrap()
        else {
            panic!("the presence fixture must begin a transition");
        };
        assert!(matches!(
            handle
                .store
                .confirm_transition(
                    companion.as_raw(),
                    generation,
                    LiveReachabilityRef {
                        client,
                        connection_live: true,
                    },
                )
                .await
                .unwrap(),
            ConfirmTransitionOutcome::Confirmed(_)
        ));

        let required = vec![
            ParticipantOwnerRef::Permission,
            ParticipantOwnerRef::Credential,
            ParticipantOwnerRef::Presence,
        ];
        let current = admit(&handle, &target, required).await;
        let first = handle
            .drive_targeted_deletion(TargetedDeletionPass::default())
            .await
            .unwrap();
        assert_eq!(
            (first.demands, first.verified),
            (3, 3),
            "all three owners complete their bounded pass in one demand"
        );
        assert_eq!(
            first.finalized, 1,
            "every required owner verified: the sealed completion boundary runs"
        );

        // Owner-local remainder verification: no targeted row keeps the text.
        let db_path = dir.path().join("app.db");
        let leaked = |path: &std::path::Path| {
            let conn = rusqlite::Connection::open(path).unwrap();
            conn.query_row(
                "SELECT
                   (SELECT COUNT(*) FROM management_intent
                    WHERE instr(target, ?1) > 0 OR instr(COALESCE(rationale_quote, ''), ?1) > 0)
                 + (SELECT COUNT(*) FROM credential_ref
                    WHERE instr(id, ?1) > 0 OR instr(provider, ?1) > 0 OR instr(label, ?1) > 0)
                 + (SELECT COUNT(*) FROM credential_pending
                    WHERE instr(provider, ?1) > 0 OR instr(label, ?1) > 0)
                 + (SELECT COUNT(*) FROM paired_device
                    WHERE instr(device_id, ?1) > 0 OR instr(descriptor, ?1) > 0)
                 + (SELECT COUNT(*) FROM presence_attribution
                    WHERE instr(companion_id, ?1) > 0
                       OR instr(COALESCE(active_client, ''), ?1) > 0)
                 + (SELECT COUNT(*) FROM relocation_hint
                    WHERE instr(companion_id, ?1) > 0
                       OR instr(COALESCE(last_client, ''), ?1) > 0
                       OR instr(COALESCE(recovery_destination, ''), ?1) > 0)",
                params![target],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
        };
        assert_eq!(leaked(&db_path), 0, "the fan-out leaves no remainder");
        assert_eq!(
            handle.store.current_set_revision().await.unwrap().as_u64(),
            revision_before.as_u64() + 1,
            "the erased ref advanced the usable-set revision once"
        );
        let stored = handle
            .store
            .lookup_intent_outcome(&intent_id)
            .await
            .unwrap()
            .expect("the decided intent survives");
        assert_eq!(stored.outcome, IntentOutcome::NeedsClarification);
        assert!(
            !stored
                .fingerprint
                .rationale_quote
                .as_deref()
                .unwrap_or_default()
                .contains(&target)
        );
        let attribution = handle
            .store
            .load_attribution(companion.as_raw())
            .await
            .unwrap()
            .expect("the companion keeps an attribution");
        assert_eq!(attribution.state, PresenceState::Stopped);
        assert_eq!(attribution.active_client, None);

        // Restart: the composition re-registers its implementations, the
        // durable sweep is already verified, and nothing is demanded or
        // mutated a second time.
        drop(handle);
        let reopened = reopen(dir.path()).await;
        let second = reopened
            .drive_targeted_deletion(TargetedDeletionPass::default())
            .await
            .unwrap();
        assert_eq!(second.demands, 0, "a verified sweep is never re-driven");
        assert_eq!(leaked(&db_path), 0);
        assert_eq!(
            reopened
                .store
                .current_set_revision()
                .await
                .unwrap()
                .as_u64(),
            revision_before.as_u64() + 1
        );
        assert!(reopened.store.list_refs().await.unwrap().is_empty());
        assert_eq!(
            reopened
                .store
                .load_attribution(companion.as_raw())
                .await
                .unwrap()
                .map(|current| current.state),
            Some(PresenceState::Stopped)
        );

        // A completed operation is terminal: a repeated pass demands nothing,
        // a generic generation advance is refused, and the control state is
        // not mutated a second time.
        let third = reopened
            .drive_targeted_deletion(TargetedDeletionPass::default())
            .await
            .unwrap();
        assert_eq!((third.demands, third.finalized), (0, 0));
        assert_eq!(
            reopened
                .store
                .change_deletion_lifecycle(
                    current,
                    crate::targeted_deletion::DeletionLifecycleChange::NextSweep,
                )
                .await
                .unwrap(),
            ene_preservation::DeletionLifecycleOutcome::Completed,
            "a terminal operation is never restarted by a generation advance"
        );
        assert_eq!(
            reopened
                .store
                .current_set_revision()
                .await
                .unwrap()
                .as_u64(),
            revision_before.as_u64() + 1,
            "a repeated pass never advances the revision again"
        );
        assert!(reopened.store.list_refs().await.unwrap().is_empty());
        assert_eq!(leaked(&db_path), 0);
        let audit = reopened
            .store
            .deletion_completion_audit(current.operation)
            .await
            .unwrap()
            .expect("the completion audit is durable");
        assert_eq!(audit.sweep_count, current.sweep.as_u64());
        assert_eq!(
            audit.erased_count,
            audit
                .participants
                .iter()
                .map(|entry| entry.erased_count)
                .sum::<u64>(),
            "the audit's erased count is the durable participant total"
        );
    }

    /// Stage 6 A5 end to end through the real fan-out and real participants:
    /// every required owner verifies, the sealed completion boundary commits
    /// the audit and closes the condition, the target is gone system-wide, the
    /// audit carries only objective metadata, and the same string provided
    /// afterwards is a fresh origin.
    #[tokio::test]
    async fn a5_a_full_sweep_completes_and_leaves_a_body_free_audit() {
        use ene_companion::{CompanionRepository as _, HistoryRepository as _, HistoryRole};
        use ene_presence::PresenceRepository as _;

        let Some((handle, dir)) = memory_handle("targeted-deletion-a5-complete").await else {
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
        let target = "a5-e2e-target";
        append_history(
            &handle,
            companion,
            generation,
            HistoryRole::Owner,
            &format!("my private note is {target}"),
        )
        .await;
        let unrelated = append_history(
            &handle,
            companion,
            generation,
            HistoryRole::Owner,
            "the weather is nice today",
        )
        .await;
        assert!(
            handle
                .store
                .count_exact_text_remainder_for_tests(target)
                .await
                .unwrap()
                > 0
        );

        let required = vec![
            ParticipantOwnerRef::Companion,
            ParticipantOwnerRef::Learning,
        ];
        let current = admit(&handle, target, required.clone()).await;
        let settled = drive_until_settled(&handle).await;
        assert_eq!(settled.finalized, 1, "the operation completes once");
        assert_eq!(
            handle
                .store
                .count_exact_text_remainder_for_tests(target)
                .await
                .unwrap(),
            0,
            "completion requires a system-wide zero remainder"
        );
        assert!(
            handle
                .store
                .unfinished_deletions(None, 100)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            handle
                .store
                .current_erasure_conditions(None, 100)
                .await
                .unwrap()
                .is_empty(),
            "a completed operation leaves the current-condition set"
        );
        let status = handle
            .store
            .deletion_status(None, 100)
            .await
            .unwrap()
            .into_iter()
            .find(|record| record.current.operation == current.operation)
            .expect("the terminal operation keeps its identity");
        assert_eq!(status.current, current);
        assert_eq!(status.phase, DeletionOperationPhase::Completed);
        assert!(!format!("{status:?}").contains(target));

        let audit = handle
            .store
            .deletion_completion_audit(current.operation)
            .await
            .unwrap()
            .expect("completion writes the durable audit");
        assert_eq!(audit.sweep_count, 1);
        assert_eq!(
            audit.erased_count, 1,
            "the collected History row is counted"
        );

        // The audit and its participant rows carry no target text: scan every
        // audit column in the durable database.
        let db_path = dir.path().join("app.db");
        let audit_leak = |needle: &str| -> i64 {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.query_row(
                "SELECT
                   (SELECT COUNT(*) FROM deletion_completion_audit
                    WHERE instr(operation_id, ?1) > 0 OR instr(purpose, ?1) > 0
                       OR instr(started_at, ?1) > 0 OR instr(completed_at, ?1) > 0)
                 + (SELECT COUNT(*) FROM deletion_audit_participant
                    WHERE instr(participant_owner, ?1) > 0 OR instr(final_state, ?1) > 0)",
                rusqlite::params![needle],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(
            audit_leak(target),
            0,
            "no audit column may carry the target"
        );
        let timeline = handle
            .store
            .load_timeline(companion, None, None, 1000)
            .await
            .expect("the timeline must load");
        assert!(timeline.iter().all(|item| !item.text.contains(target)));
        assert!(timeline.iter().any(|item| item.id == unrelated));

        // Restart after the commit: completion is durable and terminal.
        drop(handle);
        let reopened = reopen(dir.path()).await;
        assert!(
            reopened
                .store
                .unfinished_deletions(None, 100)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            reopened
                .store
                .current_erasure_conditions(None, 100)
                .await
                .unwrap()
                .is_empty()
        );

        // The same string provided after completion is a new origin: the
        // append commits and a fresh admission starts a second operation.
        let (companion, generation) = {
            let companion = reopened
                .store
                .ensure_running_companion()
                .await
                .expect("the companion must resolve");
            let generation = reopened
                .store
                .load_attribution(companion.as_raw())
                .await
                .expect("attribution must load")
                .expect("attribution must exist")
                .generation;
            (companion, generation)
        };
        append_history(
            &reopened,
            companion,
            generation,
            HistoryRole::Owner,
            &format!("I am saying {target} again"),
        )
        .await;
        let fresh = admit(&reopened, target, required).await;
        assert_ne!(fresh.operation, current.operation);
    }

    /// A crash between the finalizing marker and the completion commit:
    /// restart resumes the remaining completion steps without re-demanding any
    /// participant and without releasing the current condition early.
    #[tokio::test]
    async fn a5_restart_in_finalizing_resumes_from_the_durable_marker() {
        let Some((handle, dir)) = memory_handle("targeted-deletion-a5-finalize").await else {
            panic!("the host must open");
        };
        let target = "a5-finalizing-target";
        let required = vec![
            ParticipantOwnerRef::Companion,
            ParticipantOwnerRef::Learning,
        ];
        let current = admit(&handle, target, required).await;
        // Establish the durable all-verified premise through the canonical
        // participant API, then enter Finalizing: this is the durable shape a
        // crashed pass leaves behind.
        mark_all_participants_verified(&handle, current).await;
        assert_eq!(
            handle
                .store
                .begin_deletion_finalizing(current)
                .await
                .unwrap(),
            DeletionFinalizationOutcome::Finalizing
        );
        drop(handle);

        let reopened = reopen(dir.path()).await;
        let conditions = reopened
            .store
            .current_erasure_conditions(None, 100)
            .await
            .unwrap();
        assert_eq!(conditions.len(), 1, "the condition survives the crash");
        assert_eq!(conditions[0].condition, current.condition());
        let outcome = reopened
            .drive_targeted_deletion(TargetedDeletionPass::default())
            .await
            .expect("the resume pass must run");
        assert_eq!(outcome.demands, 0, "no participant is demanded again");
        assert_eq!(outcome.finalized, 1, "only the remaining steps run");
        assert!(
            reopened
                .store
                .current_erasure_conditions(None, 100)
                .await
                .unwrap()
                .is_empty()
        );
        let audit = reopened
            .store
            .deletion_completion_audit(current.operation)
            .await
            .unwrap()
            .expect("the resumed completion writes the audit");
        assert_eq!(audit.sweep_count, 1);
    }

    /// Verification followed by a delayed arrival: the pass refuses to
    /// complete, opens a new sweep, and only the real owner sweep of that
    /// generation completes the operation.
    #[tokio::test]
    async fn a5_a_remainder_opens_a_new_sweep_and_the_real_owner_completes_it() {
        use ene_companion::{CompanionRepository as _, HistoryRole};
        use ene_presence::PresenceRepository as _;

        let Some((handle, _dir)) = memory_handle("targeted-deletion-a5-remainder").await else {
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
        let target = "a5-delayed-arrival";
        append_history(
            &handle,
            companion,
            generation,
            HistoryRole::Owner,
            &format!("carrying {target}"),
        )
        .await;
        let current = admit(&handle, target, vec![ParticipantOwnerRef::Companion]).await;

        // A participant that claims verification without erasing: the
        // system-wide probe must refuse to complete over the stored body.
        handle.reset_deletion_participants_for_tests();
        handle
            .register_deletion_participant(Arc::new(TestParticipant::new(
                ParticipantOwnerRef::Companion,
                ParticipantCompletionStatus::Verified,
            )))
            .unwrap();
        let first = handle
            .drive_targeted_deletion(TargetedDeletionPass::default())
            .await
            .unwrap();
        assert_eq!(first.verified, 1);
        assert_eq!(first.finalized, 0);
        assert_eq!(first.remainder_sweeps, 1);
        let rows = handle.store.unfinished_deletions(None, 100).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].current.sweep, DeletionSweepGeneration::from_u64(2));
        assert_eq!(rows[0].phase, DeletionOperationPhase::Active);
        assert_eq!(
            participant_for(
                &handle.store,
                current.operation,
                ParticipantOwnerRef::Companion
            )
            .await
            .progress,
            ParticipantProgress::Pending,
            "a new sweep never inherits the old verification"
        );

        // The real owner sweep collects the remainder and the operation
        // completes on the new generation.
        handle.reset_deletion_participants_for_tests();
        handle
            .register_deletion_participant(Arc::new(ene_store::CompanionErasureParticipant::new(
                handle.store.clone(),
            )))
            .unwrap();
        let settled = drive_until_settled(&handle).await;
        assert_eq!(settled.remainder_sweeps, 0);
        assert_eq!(settled.finalized, 1);
        assert_eq!(
            handle
                .store
                .count_exact_text_remainder_for_tests(target)
                .await
                .unwrap(),
            0
        );
        let audit = handle
            .store
            .deletion_completion_audit(current.operation)
            .await
            .unwrap()
            .expect("the second generation completes");
        assert_eq!(audit.sweep_count, 2);
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
                    data_use: vec![source],
                    task_agent: Some(TaskAgentAttemptPremise {
                        delegation: delegation.as_raw(),
                        task: current.task.as_raw(),
                        task_revision: RevisionInner::from_u64(current.revision.as_u64()),
                        data_use: vec![source],
                    }),
                    pricing: None,
                    usage_estimate: None,
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
        let result = crate::test_support::record_result(
            &handle.store,
            delegation,
            &format!("final report mentions {target}"),
        )
        .await;
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
        let mut total = TargetedDeletionPassOutcome::default();
        for _ in 0..8 {
            let outcome = handle
                .drive_targeted_deletion(TargetedDeletionPass::default())
                .await
                .unwrap();
            total.accumulate(outcome);
            if total.verified == expected {
                return total;
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

        let _ = admit(
            &handle,
            target,
            vec![ParticipantOwnerRef::Task, ParticipantOwnerRef::Action],
        )
        .await;
        let outcome = drive_until_local_owners_verify(&handle, 2).await;
        assert_eq!(outcome.verified, 2);

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

    // --- Stage 6 F1: production driver and startup recovery ----------------

    /// The operation record as the durable status surface reports it.
    async fn operation_record(
        handle: &HostHandle,
        operation: DeletionOperationId,
    ) -> DeletionOperationRecord {
        handle
            .store
            .deletion_status(None, 100)
            .await
            .expect("the status must read")
            .into_iter()
            .find(|record| record.current.operation == operation)
            .expect("the operation must stay readable")
    }

    /// Establishes the durable finalizing premise through the canonical
    /// participant API: every required participant verified for the current
    /// sweep.
    async fn mark_all_participants_verified(handle: &HostHandle, current: DeletionOperationRef) {
        let mut after = None;
        loop {
            let page = handle
                .store
                .deletion_participants(current.operation, after, 100)
                .await
                .expect("the participants must read");
            if page.is_empty() {
                break;
            }
            let page_len = page.len();
            for record in page {
                after = Some(record.participant.owner);
                handle
                    .store
                    .record_participant_completion(ParticipantCompletionFact::verified(
                        current.condition(),
                        record.participant.owner,
                        0,
                        WallClockWithTz::now(),
                    ))
                    .await
                    .expect("the verification fact must record");
            }
            if page_len < 100 {
                break;
            }
        }
    }

    /// F1: the Host-local confirmation alone drives the operation. This test
    /// never calls `drive_targeted_deletion`, a tick, or startup recovery: the
    /// production confirm path must run the bounded fan-out and reach the
    /// sealed completion boundary on its own.
    #[tokio::test]
    async fn confirming_a_staged_request_drives_the_operation_to_completion() {
        let Some((handle, _dir)) = memory_handle("targeted-deletion-driver-confirm").await else {
            panic!("the host must open");
        };
        let target = "driver-confirm-body";
        let command = StageTargetedDeletionRequestCommand::new(
            TargetedDeletionTarget {
                mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                    target.into(),
                )),
                semantic_hints: vec![],
            },
            DeletionPurpose::Privacy,
            WallClockWithTz::now(),
        );
        let StageTargetedDeletionRequestOutcome::Staged(request) =
            handle.store.stage_targeted_deletion(command).await.unwrap()
        else {
            panic!("a fresh request must stage");
        };
        let request_text = request.as_raw().as_uuid().as_hyphenated().to_string();
        let outcome = handle
            .confirm_targeted_deletion(&request_text)
            .await
            .expect("the confirmation must answer");
        assert!(
            !format!("{outcome:?}").contains(target),
            "the confirmation outcome never carries the target"
        );
        let ConfirmTargetedDeletionOutcome::Started(current) = outcome else {
            panic!("the Owner confirmation must start the operation");
        };
        let record = operation_record(&handle, current.operation).await;
        assert_eq!(
            record.phase,
            DeletionOperationPhase::Completed,
            "the confirmation kick must drive the operation through the sealed boundary"
        );
        assert!(
            handle
                .store
                .deletion_completion_audit(current.operation)
                .await
                .unwrap()
                .is_some(),
            "the completion audit is durable"
        );
        assert!(
            handle
                .store
                .unfinished_deletions(None, 100)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            handle
                .store
                .current_erasure_conditions(None, 100)
                .await
                .unwrap()
                .is_empty(),
            "the completion boundary closes the current condition"
        );
    }

    /// F1 startup recovery: an `Active` operation left by a crashed process is
    /// driven to completion by the restart, with the target mechanically gone.
    #[tokio::test]
    async fn startup_recovery_drives_an_active_operation_to_completion() {
        use ene_companion::{CompanionRepository as _, HistoryRole};
        use ene_presence::PresenceRepository as _;

        let Some((handle, dir)) = memory_handle("targeted-deletion-startup-active").await else {
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
        let target = "startup-active-target";
        append_history(
            &handle,
            companion,
            generation,
            HistoryRole::Owner,
            &format!("my private note is {target}"),
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
        // Crash before any drive: the durable operation stays Active.
        drop(handle);

        let reopened = reopen(dir.path()).await;
        assert_eq!(
            operation_record(&reopened, current.operation).await.phase,
            DeletionOperationPhase::Active,
            "the crash must leave the operation Active"
        );
        reopened
            .run_startup_mutations()
            .await
            .expect("startup recovery must run");
        assert_eq!(
            operation_record(&reopened, current.operation).await.phase,
            DeletionOperationPhase::Completed,
            "startup must drive an Active operation through the sealed boundary"
        );
        assert_eq!(
            reopened
                .store
                .count_exact_text_remainder_for_tests(target)
                .await
                .unwrap(),
            0,
            "the startup drive erases the stored target"
        );
    }

    /// F1 startup recovery: a durable `Held(Unavailable)` operation is a
    /// retryable hold, so the restart resumes it and the composition re-drives
    /// the stored participant snapshot instead of leaving it stopped.
    #[tokio::test]
    async fn startup_recovery_resumes_a_held_unavailable_operation() {
        let Some((handle, dir)) = memory_handle("targeted-deletion-startup-held").await else {
            panic!("the host must open");
        };
        // Scripted holders create the durable hold; the reopened production
        // composition is what has to advance it after the resume.
        handle.reset_deletion_participants_for_tests();
        let required = vec![
            ParticipantOwnerRef::Companion,
            ParticipantOwnerRef::Learning,
        ];
        let current = admit(&handle, "startup-held-target", required.clone()).await;
        for owner in &required {
            handle
                .register_deletion_participant(Arc::new(TestParticipant::new(
                    *owner,
                    ParticipantCompletionStatus::Held(ParticipantHoldClass::Unavailable),
                )))
                .unwrap();
        }
        handle.run_targeted_deletion_tick().await.unwrap();
        drop(handle);

        let reopened = reopen(dir.path()).await;
        let before = operation_record(&reopened, current.operation).await;
        assert_eq!(before.phase, DeletionOperationPhase::Held);
        assert_eq!(before.hold, Some(DeletionHoldReason::Unavailable));
        reopened
            .run_startup_mutations()
            .await
            .expect("startup recovery must run");
        let after = operation_record(&reopened, current.operation).await;
        assert_eq!(
            after.current, current,
            "the recovery never regenerates the operation identity or sweep"
        );
        assert_eq!(
            after.phase,
            DeletionOperationPhase::Completed,
            "the resumed operation is re-driven by the reopened composition"
        );
        assert!(
            reopened
                .store
                .deletion_participants(current.operation, None, 100)
                .await
                .unwrap()
                .iter()
                .all(|record| record.progress.is_verified()),
            "every required participant verified for the current sweep"
        );
    }

    /// F1 startup recovery: a `Finalizing` operation resumes only the
    /// remaining completion steps from its durable marker; no participant is
    /// demanded again.
    #[tokio::test]
    async fn startup_recovery_resumes_a_finalizing_operation() {
        let Some((handle, dir)) = memory_handle("targeted-deletion-startup-finalizing").await
        else {
            panic!("the host must open");
        };
        let current = admit(
            &handle,
            "startup-finalizing-target",
            vec![
                ParticipantOwnerRef::Companion,
                ParticipantOwnerRef::Learning,
            ],
        )
        .await;
        mark_all_participants_verified(&handle, current).await;
        assert_eq!(
            handle
                .store
                .begin_deletion_finalizing(current)
                .await
                .unwrap(),
            DeletionFinalizationOutcome::Finalizing
        );
        drop(handle);

        let reopened = reopen(dir.path()).await;
        assert_eq!(
            operation_record(&reopened, current.operation).await.phase,
            DeletionOperationPhase::Finalizing,
            "the durable marker survives the crash"
        );
        reopened
            .run_startup_mutations()
            .await
            .expect("startup recovery must run");
        assert_eq!(
            operation_record(&reopened, current.operation).await.phase,
            DeletionOperationPhase::Completed,
            "the restart finishes the remaining completion steps"
        );
        assert!(
            reopened
                .store
                .deletion_completion_audit(current.operation)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            reopened
                .store
                .current_erasure_conditions(None, 100)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// F1 fail closed: `Held(GenerationExhausted)` can never resume, so
    /// startup recovery and a serving tick must both leave it held with its
    /// condition active and no participant progress fabricated.
    #[tokio::test]
    async fn startup_recovery_leaves_a_generation_exhausted_hold_untouched() {
        let Some((handle, dir)) = memory_handle("targeted-deletion-startup-exhausted").await else {
            panic!("the host must open");
        };
        let current = admit(
            &handle,
            "exhausted-target",
            vec![
                ParticipantOwnerRef::Companion,
                ParticipantOwnerRef::Learning,
            ],
        )
        .await;
        handle
            .store
            .hold_generation_exhausted_for_tests(current.operation)
            .await
            .expect("the fixture must enter the exhaustion hold");
        drop(handle);

        let reopened = reopen(dir.path()).await;
        reopened
            .run_startup_mutations()
            .await
            .expect("startup recovery must run");
        let after = operation_record(&reopened, current.operation).await;
        assert_eq!(after.phase, DeletionOperationPhase::Held);
        assert_eq!(after.hold, Some(DeletionHoldReason::GenerationExhausted));
        assert!(
            reopened
                .store
                .deletion_participants(current.operation, None, 100)
                .await
                .unwrap()
                .iter()
                .all(|record| !record.progress.is_verified()),
            "no verification is fabricated for an exhausted generation"
        );
        assert!(
            reopened
                .store
                .deletion_completion_audit(current.operation)
                .await
                .unwrap()
                .is_none(),
            "an exhausted hold is never a completion"
        );
        assert!(
            reopened
                .store
                .current_erasure_conditions(None, 100)
                .await
                .unwrap()
                .iter()
                .any(|condition| condition.condition == after.current.condition()),
            "the current condition stays active while the operation is held"
        );

        // A serving tick does not move it either, and never demands work for it.
        let tick = reopened.run_targeted_deletion_tick().await.unwrap();
        assert_eq!(tick.demands, 0);
        assert_eq!(
            operation_record(&reopened, current.operation).await.phase,
            DeletionOperationPhase::Held
        );
    }

    /// F1 serving tick: a retryable hold is retried, but with a bounded
    /// backoff (the skip doubles per consecutive attempt) and never as an
    /// inferred completion.
    #[tokio::test]
    async fn serving_ticks_retry_a_held_operation_with_bounded_backoff() {
        let target = "backoff-target";
        let Some((handle, _dir)) = memory_handle("targeted-deletion-tick-backoff").await else {
            panic!("the host must open");
        };
        handle.reset_deletion_participants_for_tests();
        let owner = ParticipantOwnerRef::Companion;
        let current = admit(&handle, target, vec![owner]).await;
        let participant = Arc::new(TestParticipant::new(
            owner,
            ParticipantCompletionStatus::Held(ParticipantHoldClass::Unavailable),
        ));
        handle
            .register_deletion_participant(participant.clone())
            .unwrap();

        // Tick 1 drives the Active operation into the durable hold.
        handle.run_targeted_deletion_tick().await.unwrap();
        assert_eq!(participant.calls(), 1);
        // Ticks 2 and 3 retry immediately after the first retry (skip 1 then
        // 2), then tick 4 must be skipped by the doubled backoff.
        handle.run_targeted_deletion_tick().await.unwrap();
        assert_eq!(participant.calls(), 2);
        handle.run_targeted_deletion_tick().await.unwrap();
        assert_eq!(participant.calls(), 3);
        let fourth = handle.run_targeted_deletion_tick().await.unwrap();
        assert_eq!(
            fourth.operations, 1,
            "the held operation is still examined every tick"
        );
        assert_eq!(fourth.demands, 0, "the backoff skips this retry");
        assert_eq!(participant.calls(), 3);
        // Tick 5 retries once more; ticks 6-8 are then inside the doubled
        // skip, and tick 9 retries again.
        handle.run_targeted_deletion_tick().await.unwrap();
        assert_eq!(participant.calls(), 4);
        for _ in 0..3 {
            handle.run_targeted_deletion_tick().await.unwrap();
        }
        assert_eq!(
            participant.calls(),
            4,
            "the skip doubles between consecutive retry attempts"
        );
        let ninth = handle.run_targeted_deletion_tick().await.unwrap();
        assert_eq!(participant.calls(), 5);
        assert!(
            !format!("{ninth:?}").contains(target),
            "the tick outcome never carries the target"
        );

        // Not a completion: the durable phase stays held with its condition
        // open and no audit, no matter how often the tick retried.
        let after = operation_record(&handle, current.operation).await;
        assert_eq!(after.phase, DeletionOperationPhase::Held);
        assert_eq!(after.hold, Some(DeletionHoldReason::Unavailable));
        assert!(
            handle
                .store
                .deletion_completion_audit(current.operation)
                .await
                .unwrap()
                .is_none(),
            "a held operation is never completed by estimation"
        );
        assert!(
            handle
                .store
                .current_erasure_conditions(None, 100)
                .await
                .unwrap()
                .iter()
                .any(|condition| condition.condition == current.condition()),
            "the condition stays active while the operation is held"
        );
    }

    /// F1 concurrent ticks: two overlapping ticks must be idempotent. The
    /// retry schedule serializes them and the canonical store re-derives every
    /// premise, so the operation completes exactly once with one audit.
    #[tokio::test]
    async fn concurrent_serving_ticks_complete_once_and_stay_idempotent() {
        use ene_companion::{CompanionRepository as _, HistoryRole};
        use ene_presence::PresenceRepository as _;

        let Some((handle, _dir)) = memory_handle("targeted-deletion-tick-concurrent").await else {
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
        let target = "concurrent-tick-target";
        append_history(
            &handle,
            companion,
            generation,
            HistoryRole::Owner,
            &format!("the concurrent note is {target}"),
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
        let (first, second) = tokio::join!(
            handle.run_targeted_deletion_tick(),
            handle.run_targeted_deletion_tick()
        );
        first.expect("the first concurrent tick must run");
        second.expect("the second concurrent tick must run");
        let after = operation_record(&handle, current.operation).await;
        assert_eq!(after.phase, DeletionOperationPhase::Completed);
        assert_eq!(
            handle
                .store
                .count_exact_text_remainder_for_tests(target)
                .await
                .unwrap(),
            0
        );
        let audit = handle
            .store
            .deletion_completion_audit(current.operation)
            .await
            .unwrap()
            .expect("completion writes one audit");
        assert_eq!(audit.sweep_count, 1);
        // A later tick over the completed operation demands nothing and can
        // never reopen it.
        let idle = handle.run_targeted_deletion_tick().await.unwrap();
        assert_eq!((idle.demands, idle.finalized), (0, 0));
        assert_eq!(
            operation_record(&handle, current.operation).await.phase,
            DeletionOperationPhase::Completed
        );
    }

    /// F-3 regression: an unfinished set larger than `operation_limit` is
    /// rotated across passes instead of truncated to the first page. The
    /// rotation is scheduling only: every operation stays unfinished on its
    /// own durable evidence.
    #[tokio::test]
    async fn a_pass_rotates_through_more_unfinished_operations_than_the_limit() {
        let Some((handle, _dir)) = memory_handle("targeted-deletion-rotation").await else {
            panic!("the host must open");
        };
        // Distinct owners per operation so "visited" is observable per
        // identity instead of being conflated in one shared participant.
        handle.reset_deletion_participants_for_tests();
        let mut participants = Vec::new();
        for (index, owner) in [
            ParticipantOwnerRef::Companion,
            ParticipantOwnerRef::Learning,
            ParticipantOwnerRef::Task,
        ]
        .into_iter()
        .enumerate()
        {
            let participant = Arc::new(TestParticipant::new(
                owner,
                ParticipantCompletionStatus::MoreWork,
            ));
            handle
                .register_deletion_participant(participant.clone())
                .unwrap();
            admit(&handle, &format!("rotation-target-{index}"), vec![owner]).await;
            participants.push(participant);
        }
        let visited = |participants: &[Arc<TestParticipant>]| {
            participants
                .iter()
                .filter(|participant| participant.calls() > 0)
                .count()
        };
        let pass = TargetedDeletionPass::new(2, 1);
        let first = handle.drive_targeted_deletion(pass).await.unwrap();
        assert_eq!(first.operations, 2, "one pass stays inside its bound");
        assert_eq!(
            visited(&participants),
            2,
            "the first pass examines only the head page"
        );
        let second = handle.drive_targeted_deletion(pass).await.unwrap();
        assert_eq!(
            visited(&participants),
            3,
            "the next pass starts after the cursor and reaches the tail operation"
        );
        assert_eq!(second.operations, 1, "the rotated pass examines the tail");
        assert_eq!(
            handle
                .store
                .unfinished_deletions(None, 100)
                .await
                .unwrap()
                .len(),
            3,
            "rotation alone completes no operation"
        );
    }

    /// F-3 regression: an operation admitted while the walk is mid-lap is still
    /// collected. The id it lands on relative to the cursor is not known (ids
    /// are not admission-ordered), so the rotation must reach it from either
    /// side of the cursor within a bounded number of laps.
    #[tokio::test]
    async fn an_operation_admitted_mid_lap_is_collected_by_the_wrap() {
        let Some((handle, _dir)) = memory_handle("targeted-deletion-mid-lap").await else {
            panic!("the host must open");
        };
        handle.reset_deletion_participants_for_tests();
        for (index, owner) in [
            ParticipantOwnerRef::Companion,
            ParticipantOwnerRef::Learning,
        ]
        .into_iter()
        .enumerate()
        {
            let participant = Arc::new(TestParticipant::new(
                owner,
                ParticipantCompletionStatus::MoreWork,
            ));
            handle.register_deletion_participant(participant).unwrap();
            admit(&handle, &format!("mid-lap-{index}"), vec![owner]).await;
        }
        let pass = TargetedDeletionPass::new(1, 1);
        handle.drive_targeted_deletion(pass).await.unwrap();
        // The arrival is admitted while the walk sits between finished and
        // unfinished ids.
        let late_owner = ParticipantOwnerRef::Task;
        let late = Arc::new(TestParticipant::new(
            late_owner,
            ParticipantCompletionStatus::MoreWork,
        ));
        handle.register_deletion_participant(late.clone()).unwrap();
        admit(&handle, "mid-lap-late", vec![late_owner]).await;
        for _ in 0..4 {
            handle.drive_targeted_deletion(pass).await.unwrap();
            if late.calls() > 0 {
                break;
            }
        }
        assert!(
            late.calls() > 0,
            "the mid-lap arrival is reached by the rotated walk"
        );
    }

    /// F-3 regression: a completed head operation leaves the unfinished set
    /// without stopping the walk at the operations after it.
    #[tokio::test]
    async fn a_completed_operation_does_not_stop_the_walk_at_the_later_operations() {
        let Some((handle, _dir)) = memory_handle("targeted-deletion-completed-head").await else {
            panic!("the host must open");
        };
        handle.reset_deletion_participants_for_tests();
        let mut registered = Vec::new();
        for (index, owner) in [
            ParticipantOwnerRef::Companion,
            ParticipantOwnerRef::Learning,
            ParticipantOwnerRef::Task,
        ]
        .into_iter()
        .enumerate()
        {
            let participant = Arc::new(TestParticipant::new(
                owner,
                ParticipantCompletionStatus::MoreWork,
            ));
            handle
                .register_deletion_participant(participant.clone())
                .unwrap();
            let current = admit(&handle, &format!("completed-head-{index}"), vec![owner]).await;
            registered.push((current.operation, participant));
        }
        // The head is derived from the durable order, not from admission
        // order: operation ids are not admission-ordered, and only the head
        // verifies.
        let order = handle.store.unfinished_deletions(None, 100).await.unwrap();
        assert_eq!(order.len(), 3);
        let head = order[0].current.operation;
        registered
            .iter()
            .find(|(operation, _)| *operation == head)
            .expect("the head operation is one of the admitted ones")
            .1
            .set_status(ParticipantCompletionStatus::Verified);

        let pass = TargetedDeletionPass::new(1, 1);
        let first = handle.drive_targeted_deletion(pass).await.unwrap();
        assert_eq!(first.finalized, 1, "the verified head operation completes");
        assert_eq!(
            operation_record(&handle, head).await.phase,
            DeletionOperationPhase::Completed
        );
        // The completed operation left the unfinished set and the cursor moved
        // past it; the following passes must still reach the two operations
        // after it instead of stopping at the vanished head.
        handle.drive_targeted_deletion(pass).await.unwrap();
        handle.drive_targeted_deletion(pass).await.unwrap();
        for (operation, participant) in &registered {
            if *operation == head {
                continue;
            }
            assert!(
                participant.calls() > 0,
                "the operation after the completed head must be demanded"
            );
            assert_ne!(
                operation_record(&handle, *operation).await.phase,
                DeletionOperationPhase::Completed,
                "an unfinished operation is not completed by a walk position"
            );
        }
        assert_eq!(
            handle
                .store
                .unfinished_deletions(None, 100)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    /// F-3 regression: the walk cursor is a scheduling position and never
    /// completion truth. A pass that starts past a completion-ready operation
    /// neither completes it unseen nor invents completion for the operation it
    /// did examine; when the rotation reaches it, the same durable premise
    /// completes it.
    #[tokio::test]
    async fn the_walk_cursor_is_a_position_and_never_completion_truth() {
        let Some((handle, _dir)) = memory_handle("targeted-deletion-cursor-truth").await else {
            panic!("the host must open");
        };
        handle.reset_deletion_participants_for_tests();
        let mut registered = Vec::new();
        for (index, owner) in [
            ParticipantOwnerRef::Companion,
            ParticipantOwnerRef::Learning,
            ParticipantOwnerRef::Task,
        ]
        .into_iter()
        .enumerate()
        {
            let participant = Arc::new(TestParticipant::new(
                owner,
                ParticipantCompletionStatus::MoreWork,
            ));
            handle
                .register_deletion_participant(participant.clone())
                .unwrap();
            let current = admit(&handle, &format!("cursor-truth-{index}"), vec![owner]).await;
            registered.push((current, participant));
        }
        let order = handle.store.unfinished_deletions(None, 100).await.unwrap();
        assert_eq!(order.len(), 3);
        let head = registered
            .iter()
            .find(|(current, _)| current.operation == order[0].current.operation)
            .expect("the head is one of the admitted operations")
            .0;
        let head_participant = Arc::clone(
            &registered
                .iter()
                .find(|(current, _)| current.operation == head.operation)
                .expect("the head is one of the admitted operations")
                .1,
        );
        // Durable verification of the head is the only completion premise.
        mark_all_participants_verified(&handle, head).await;
        // The cursor starts after the head, so the pass cannot see it yet.
        handle
            .store
            .set_deletion_walk_cursor(DeletionWalk::FanOut, Some(order[1].current.operation))
            .await
            .unwrap();
        let pass = TargetedDeletionPass::new(1, 1);
        let first = handle.drive_targeted_deletion(pass).await.unwrap();
        assert_eq!(first.operations, 1);
        assert_eq!(
            first.finalized, 0,
            "a position never completes an operation this pass did not settle"
        );
        assert_eq!(
            operation_record(&handle, head.operation).await.phase,
            DeletionOperationPhase::Active,
            "the skipped completion-ready operation stays unfinished"
        );
        assert_eq!(
            head_participant.calls(),
            0,
            "the skipped operation's participant was not demanded"
        );
        // The rotation wraps to it and completes it from the durable premise.
        let mut completed = false;
        for _ in 0..4 {
            handle.drive_targeted_deletion(pass).await.unwrap();
            if operation_record(&handle, head.operation).await.phase
                == DeletionOperationPhase::Completed
            {
                completed = true;
                break;
            }
        }
        assert!(
            completed,
            "the rotation must reach and complete the delayed head"
        );
        assert!(
            handle
                .store
                .deletion_completion_audit(head.operation)
                .await
                .unwrap()
                .is_some(),
            "the delayed completion re-derives and commits the audit"
        );
    }

    /// F-3 regression: the retryable-hold scan rotates too, so holds past the
    /// first page are offered a resume instead of being starved by the head.
    #[tokio::test]
    async fn the_retryable_hold_scan_rotates_through_more_holds_than_the_limit() {
        let Some((handle, _dir)) = memory_handle("targeted-deletion-hold-rotation").await else {
            panic!("the host must open");
        };
        handle.reset_deletion_participants_for_tests();
        let mut participants = Vec::new();
        for (index, owner) in [
            ParticipantOwnerRef::Companion,
            ParticipantOwnerRef::Learning,
            ParticipantOwnerRef::Task,
        ]
        .into_iter()
        .enumerate()
        {
            let participant = Arc::new(TestParticipant::new(
                owner,
                ParticipantCompletionStatus::Held(ParticipantHoldClass::Unavailable),
            ));
            handle
                .register_deletion_participant(participant.clone())
                .unwrap();
            admit(&handle, &format!("hold-rotation-{index}"), vec![owner]).await;
            participants.push(participant);
        }
        handle
            .drive_targeted_deletion(TargetedDeletionPass::new(3, 1))
            .await
            .unwrap();
        assert!(
            participants
                .iter()
                .all(|participant| participant.calls() == 1)
        );

        let registry = crate::lock_unpoison(&handle.targeted_deletion).clone();
        let mut schedule = HeldRetrySchedule::new();
        let pass = TargetedDeletionPass::new(2, 1);
        for _ in 0..3 {
            tick_targeted_deletion(&handle.store, &registry, pass, &mut schedule)
                .await
                .unwrap();
        }
        assert!(
            participants
                .iter()
                .all(|participant| participant.calls() >= 2),
            "every hold must be resumed and re-driven by the rotating scan"
        );
        assert_eq!(
            handle
                .store
                .unfinished_deletions(None, 100)
                .await
                .unwrap()
                .len(),
            3,
            "a resumed hold stays unfinished until its participant verifies"
        );
    }

    /// M3 driver regression: strictly more covered identities than one
    /// reconciliation page, with the claim's source on the last page.
    ///
    /// The production fan-out must walk every page before demanding any
    /// participant (the identity bodies are the evidence the correspondence
    /// is derived from), refuse `Finalizing` while the walk is incomplete,
    /// complete only after it, and then refuse the delayed result after
    /// completion while accepting a fresh origin.
    #[tokio::test]
    async fn exhaustive_reconciliation_holds_the_last_source_before_completion() {
        use ene_companion::{CompanionRepository as _, HistoryRole};
        use ene_learning::{
            ChangeKind, Importance, LearningClaimRef, LearningRepository as _, LearningScope,
            MemoryChange, MemoryChangeCommit, MemoryId, MemoryTarget, TemporalMeaning,
        };
        use ene_permission::{ConsentRecord, ConsentRevision};
        use ene_presence::PresenceRepository as _;
        use ene_preservation::{
            ConfirmTargetedDeletionOutcome, DeletionPurpose, DeletionSearchMaterial,
            MechanicalDeletionTarget, StageTargetedDeletionRequestCommand,
            StageTargetedDeletionRequestOutcome, TargetedDeletionTarget,
        };

        let Some((handle, dir)) = memory_handle("targeted-deletion-m3-reconcile").await else {
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
        let target = "m3-driver-canary-beyond-the-admission-page";
        // Strictly more than one reconciliation page of covered identities,
        // all committed through the production append path.
        let mut sources = Vec::new();
        for index in 0..(DELETION_RECONCILIATION_PAGE_SIZE + 6) {
            sources.push(
                append_history(
                    &handle,
                    companion,
                    generation,
                    HistoryRole::Owner,
                    &format!("note {index} carries {target}"),
                )
                .await,
            );
        }
        // The canonical last identity is the lexical maximum of the encoded
        // keys, which the encoded UUID order preserves.
        let last = *sources
            .iter()
            .max_by_key(|source| source.as_uuid())
            .expect("the fixture has sources");

        // R2 use-first: the formation claim naming the last covered source
        // commits before the deletion condition.
        let fingerprint = IntentFingerprint {
            intent_id: RawId::new().as_uuid().to_string(),
            kind: String::from("assign"),
            target: String::from("consent:test-seed"),
            base: String::from("consent-test-seed"),
            rationale_origin: String::from("management-surface"),
            rationale_quote: None,
        };
        let saved = handle
            .store
            .assign_with_intent(
                None,
                ConsentRecord {
                    capability: CapabilityKind::Learning,
                    id: String::from("consent-learning"),
                    rev: ConsentRevision::from_u64(1),
                    provider: String::from("openai"),
                    model: String::from("dialogue-1"),
                    credential_id: String::from("cred-1"),
                },
                fingerprint,
            )
            .await
            .expect("the consent must answer");
        assert!(
            matches!(
                saved,
                IntentResolution::Decided(ConsentCommitOutcome::Committed { .. })
            ),
            "the Learning consent must commit: {saved:?}"
        );
        let ticket = InferenceTicketId(RawId::new());
        let claimed = handle
            .store
            .begin_inference_attempt(InferenceAttempt {
                ticket,
                consumer: ConsumerKind::CompanionLearning,
                capability: CapabilityKind::Learning,
                purpose: PurposeKind::MemoryFormation,
                expected_consent: (
                    String::from("consent-learning"),
                    ConsentRevision::from_u64(1),
                ),
                expected_credential_set: CredentialSetRevision::initial(),
                provider: String::from("openai"),
                model: String::from("dialogue-1"),
                task_agent: None,
                data_use: vec![last],
                pricing: None,
                usage_estimate: None,
            })
            .await
            .expect("the formation claim must answer");
        assert_eq!(claimed, AttemptBeginOutcome::Started);

        // First-party admission through the production request/confirmation
        // path, so the bounded admission page publishes only the first page.
        let staged = handle
            .store
            .stage_targeted_deletion(StageTargetedDeletionRequestCommand::new(
                TargetedDeletionTarget {
                    mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                        target.to_owned(),
                    )),
                    semantic_hints: Vec::new(),
                },
                DeletionPurpose::Privacy,
                WallClockWithTz::now(),
            ))
            .await
            .expect("staging must answer");
        let request = match staged {
            StageTargetedDeletionRequestOutcome::Staged(request) => request,
            other => panic!("the scope must stage, got {other:?}"),
        };
        let current = match handle
            .store
            .confirm_targeted_deletion(request, current_product_surface_owners())
            .await
            .expect("the confirmation must answer")
        {
            ConfirmTargetedDeletionOutcome::Started(current) => current,
            other => panic!("the confirmation must start, got {other:?}"),
        };

        // The production fan-out walks every reconciliation page before the
        // participant sweeps, then completes.
        let outcome = drive_until_settled(&handle).await;
        assert!(
            outcome.reconciliation_pages >= 1,
            "the walk needed at least one continuation page: {outcome:?}"
        );
        assert_eq!(outcome.finalized, 1, "the operation completes once");
        assert_eq!(
            operation_record(&handle, current.operation).await.phase,
            DeletionOperationPhase::Completed
        );
        assert_eq!(
            handle
                .store
                .count_exact_text_remainder_for_tests(target)
                .await
                .unwrap(),
            0,
            "no durable surface keeps the exact target"
        );

        // The durable hold for the claim exists even though its source fell
        // past the admission page.
        let ticket_text = ticket.0.as_uuid().as_hyphenated().to_string();
        let held: i64 = {
            let conn = rusqlite::Connection::open(dir.path().join("app.db"))
                .expect("the state database opens for inspection");
            conn.query_row(
                "SELECT COUNT(*) FROM erasure_use_hold WHERE use_kind='inference_attempt' AND use_id=?1",
                [&ticket_text],
                |row| row.get(0),
            )
            .expect("the hold probe must answer")
        };
        assert_eq!(held, 1, "the claim is durably associated");

        // The delayed formation arrives after completion and is refused by the
        // claim correspondence, never by a permanent keyword ban.
        let delayed = MemoryChangeCommit {
            summary: Some(summary_of(
                companion.as_raw(),
                String::from("a clean paraphrase"),
                RawId::new(),
                RawId::new(),
            )),
            secret_premise: None,
            claim: Some(LearningClaimRef::from_raw(ticket.0)),
            change: MemoryChange {
                target: MemoryTarget::New {
                    id: MemoryId::generate(),
                },
                scope: LearningScope::companion(companion.as_raw()),
                content: String::from("a clean recall"),
                importance: Importance::default(),
                temporal: TemporalMeaning::Enduring,
                change: ChangeKind::Initial,
                recall_suppressed: false,
                at: WallClockWithTz::now(),
            },
        };
        assert_eq!(
            handle
                .store
                .commit_memory_change(delayed)
                .await
                .expect("the delayed commit must answer"),
            ene_learning::MemoryChangeOutcome::HeldForErasure
        );

        // A fresh claim from a fresh source after completion is a new origin.
        let fresh_ticket = InferenceTicketId(RawId::new());
        assert_eq!(
            handle
                .store
                .begin_inference_attempt(InferenceAttempt {
                    ticket: fresh_ticket,
                    consumer: ConsumerKind::CompanionLearning,
                    capability: CapabilityKind::Learning,
                    purpose: PurposeKind::MemoryFormation,
                    expected_consent: (
                        String::from("consent-learning"),
                        ConsentRevision::from_u64(1),
                    ),
                    expected_credential_set: CredentialSetRevision::initial(),
                    provider: String::from("openai"),
                    model: String::from("dialogue-1"),
                    task_agent: None,
                    data_use: vec![RawId::new()],
                    pricing: None,
                    usage_estimate: None,
                })
                .await
                .expect("the fresh claim must answer"),
            AttemptBeginOutcome::Started
        );
        let fresh = MemoryChangeCommit {
            summary: Some(summary_of(
                companion.as_raw(),
                String::from("a fresh note"),
                RawId::new(),
                RawId::new(),
            )),
            secret_premise: None,
            claim: Some(LearningClaimRef::from_raw(fresh_ticket.0)),
            change: MemoryChange {
                target: MemoryTarget::New {
                    id: MemoryId::generate(),
                },
                scope: LearningScope::companion(companion.as_raw()),
                content: String::from("a fresh recognition"),
                importance: Importance::default(),
                temporal: TemporalMeaning::Enduring,
                change: ChangeKind::Initial,
                recall_suppressed: false,
                at: WallClockWithTz::now(),
            },
        };
        assert!(
            matches!(
                handle
                    .store
                    .commit_memory_change(fresh)
                    .await
                    .expect("the fresh commit must answer"),
                ene_learning::MemoryChangeOutcome::Committed { .. }
            ),
            "a post-completion origin is accepted"
        );
    }

    /// M3 restart regression: the process stops with the covered-source walk
    /// mid-flight; the reopened Host resumes from the durable cursor through
    /// the production startup recovery, finishes the walk, and completes. No
    /// page state lives in memory and no generation is reused.
    #[tokio::test]
    async fn restart_mid_reconciliation_resumes_the_walk_and_completes() {
        use ene_companion::{CompanionRepository as _, HistoryRole};
        use ene_presence::PresenceRepository as _;
        use ene_preservation::{
            ConfirmTargetedDeletionOutcome, DeletionPurpose, DeletionSearchMaterial,
            MechanicalDeletionTarget, StageTargetedDeletionRequestCommand,
            StageTargetedDeletionRequestOutcome, TargetedDeletionTarget,
        };

        let Some((handle, dir)) = memory_handle("targeted-deletion-m3-restart").await else {
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
        let target = "m3-restart-canary-beyond-the-admission-page";
        for index in 0..(DELETION_RECONCILIATION_PAGE_SIZE + 6) {
            append_history(
                &handle,
                companion,
                generation,
                HistoryRole::Owner,
                &format!("note {index} carries {target}"),
            )
            .await;
        }
        let staged = handle
            .store
            .stage_targeted_deletion(StageTargetedDeletionRequestCommand::new(
                TargetedDeletionTarget {
                    mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                        target.to_owned(),
                    )),
                    semantic_hints: Vec::new(),
                },
                DeletionPurpose::Privacy,
                WallClockWithTz::now(),
            ))
            .await
            .expect("staging must answer");
        let request = match staged {
            StageTargetedDeletionRequestOutcome::Staged(request) => request,
            other => panic!("the scope must stage, got {other:?}"),
        };
        let current = match handle
            .store
            .confirm_targeted_deletion(request, current_product_surface_owners())
            .await
            .expect("the confirmation must answer")
        {
            ConfirmTargetedDeletionOutcome::Started(current) => current,
            other => panic!("the confirmation must start, got {other:?}"),
        };
        // One bounded continuation page commits, then the process stops with
        // the walk still incomplete.
        assert_eq!(
            handle
                .store
                .reconcile_deletion_sources(current, DELETION_RECONCILIATION_PAGE_SIZE)
                .await
                .expect("the continuation page must answer"),
            DeletionReconciliationOutcome::Advanced
        );
        drop(handle);

        let reopened = reopen(dir.path()).await;
        // The production restart path resumes and finishes the walk from the
        // durable cursor; the completion is observable right after recovery.
        reopened
            .run_startup_mutations()
            .await
            .expect("the restart recovery must complete");
        assert_eq!(
            operation_record(&reopened, current.operation).await.phase,
            DeletionOperationPhase::Completed,
            "the startup recovery resumes the durable walk and completes"
        );
        assert_eq!(
            reopened
                .store
                .count_exact_text_remainder_for_tests(target)
                .await
                .unwrap(),
            0,
            "the resumed operation erases every identity"
        );
    }
}
