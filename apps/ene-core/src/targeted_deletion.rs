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
    let limit = pass.operation_limit;
    let page = walk.page(store, limit).await?;
    let page_len = page.len();
    for record in page {
        walk.examined(record.current.operation);
        outcome.operations += 1;
        match record.phase {
            DeletionOperationPhase::Active => {
                drive_operation(
                    store,
                    registry,
                    record.current,
                    pass.demands_per_participant,
                    &mut outcome,
                )
                .await?;
            }
            // A crash between the finalizing marker and the completion
            // commit resumes here; the operation identity, the current
            // condition, and the participant statuses come from durable
            // state, never from memory defaults (§14).
            DeletionOperationPhase::Finalizing => {
                settle_finalizing(store, registry, record.current, &mut outcome).await?;
            }
            DeletionOperationPhase::Held | DeletionOperationPhase::Completed => {}
        }
    }
    walk.advance(store, page_len < limit as usize).await?;
    Ok(outcome)
}

async fn settle_finalizing(
    store: &Store,
    registry: &ErasureParticipantRegistry,
    current: DeletionOperationRef,
    outcome: &mut TargetedDeletionPassOutcome,
) -> Result<(), CoreError> {
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

/// Drives one active operation. A concurrent lifecycle transition (a new sweep
/// or a completion/phase change) ends this operation's work for the pass; the
/// other records on the pass's page are still driven.
async fn drive_operation(
    store: &Store,
    registry: &ErasureParticipantRegistry,
    current: DeletionOperationRef,
    demand_budget: u32,
    outcome: &mut TargetedDeletionPassOutcome,
) -> Result<(), CoreError> {
    let condition = current.condition();
    // Phase 1: exhaust the covered-source reconciliation before the first
    // participant demand. The identity bodies carrying the target are the
    // evidence the already-claimed in-flight-use correspondence is derived
    // from, and the owner sweeps redact them; publishing and associating
    // first is what keeps the correspondence complete regardless of how many
    // covered identities exist. The page budget bounds one pass; the durable
    // cursor resumes the walk on the next pass or after a restart.
    let mut reconciled = false;
    for _ in 0..RECONCILIATION_PAGES_PER_PASS {
        match store
            .reconcile_deletion_sources(current, DELETION_RECONCILIATION_PAGE_SIZE)
            .await
            .map_err(deletion_error)?
        {
            DeletionReconciliationOutcome::Advanced => {
                outcome.reconciliation_pages += 1;
            }
            DeletionReconciliationOutcome::Complete => {
                reconciled = true;
                break;
            }
            DeletionReconciliationOutcome::Finalizing
            | DeletionReconciliationOutcome::Completed => return Ok(()),
            DeletionReconciliationOutcome::Missing | DeletionReconciliationOutcome::StaleSweep => {
                return Ok(());
            }
        }
    }
    if !reconciled {
        return Ok(());
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
                    | ParticipantDemandOutcome::NotRequired => return Ok(()),
                }
                outcome.demands += 1;
                let material = match store
                    .deletion_operation_material(current.operation)
                    .await
                    .map_err(deletion_error)?
                {
                    DeletionMaterialOutcome::Material(material) => material,
                    DeletionMaterialOutcome::Missing | DeletionMaterialOutcome::Destroyed => {
                        return Ok(());
                    }
                };
                let command = DemandLocalErasureCommand::new(
                    condition,
                    record.participant.owner,
                    command_scope(&material, record.participant.owner),
                );
                let fact = registry.demand(command).await;
                if fact.condition() != condition || fact.participant() != record.participant.owner {
                    return Ok(());
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
                    | ParticipantCompletionOutcome::NotRequired => return Ok(()),
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
        return Ok(());
    }
    settle_finalizing(store, registry, current, outcome).await?;
    Ok(())
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
/// Returns [`CoreError::Deletion`] for an invalid budget and when the
/// canonical store refuses.
pub(crate) async fn drive_targeted_deletion_until_settled(
    store: &Store,
    registry: &ErasureParticipantRegistry,
    pass_budget: u32,
) -> Result<TargetedDeletionPassOutcome, CoreError> {
    if pass_budget == 0 {
        return Err(CoreError::Deletion(String::from(
            "invalid targeted deletion drive parameters",
        )));
    }
    let pass = TargetedDeletionPass::default();
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

/// Restores unfinished Targeted Deletion operations at startup (lifecycle
/// §14).
///
/// A `Held(Unavailable)` operation is retryable and a Host restart is a
/// recovery decision: the hold is resumed so the reopened composition can
/// re-drive the stored participant snapshot. This only reads the durable
/// phase and applies the canonical [`DeletionLifecycleChange::Resume`]; a
/// `Held(GenerationExhausted)` operation is left untouched (fail closed — no
/// generation can be reused or invented) and an `Active` / `Finalizing`
/// operation is left for the bounded drive below. Every operation identity,
/// sweep, and participant row comes from durable state; nothing is reset to a
/// memory default, and a restart is never taken as completion evidence.
///
/// # Errors
///
/// Returns [`CoreError::Deletion`] for an invalid budget and when the
/// canonical store refuses.
pub(crate) async fn recover_targeted_deletions(
    store: &Store,
    registry: &ErasureParticipantRegistry,
    pass_budget: u32,
) -> Result<TargetedDeletionPassOutcome, CoreError> {
    let pass = TargetedDeletionPass::default();
    // Startup recovery is a fresh schedule: the empty retry map makes every
    // retryable hold eligible, so the page is offered whole.
    resume_hold_page(
        store,
        pass.operation_limit,
        &mut HeldRetrySchedule::new(),
        1,
    )
    .await?;
    drive_targeted_deletion_until_settled(store, registry, pass_budget).await
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
/// Returns [`CoreError::Deletion`] when the canonical store refuses.
pub(crate) async fn tick_targeted_deletion(
    store: &Store,
    registry: &ErasureParticipantRegistry,
    schedule: &mut HeldRetrySchedule,
) -> Result<TargetedDeletionPassOutcome, CoreError> {
    let pass = TargetedDeletionPass::default();
    let tick = schedule.next_tick();
    resume_hold_page(store, pass.operation_limit, schedule, tick).await?;
    drive_targeted_deletion(store, registry, pass).await
}
