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
//!
//! The production entry points over this module are bounded: Host startup
//! restores unfinished operations (resuming a retryable hold once, lifecycle
//! §14), the serving composition runs a periodic tick (one pass plus a
//! backed-off retry of a retryable hold), and a first-party confirmation kicks
//! a bounded drive immediately after admission. None of them decides
//! completion: only the sealed store boundary does.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use ene_preservation::{
    DELETION_RECONCILIATION_PAGE_SIZE, DeletionFinalizationOutcome, DeletionLifecycleChange,
    DeletionLifecycleOutcome, DeletionMaterialOutcome, DeletionOperationId,
    DeletionOperationMaterial, DeletionOperationPhase, DeletionOperationRef,
    DeletionReconciliationOutcome, DemandLocalErasureCommand, ErasureParticipant,
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
/// the composition appends one [`ParticipantOwnerRef::ClientIncarnation`] per
/// incarnation with durable body-delivery evidence when it builds the
/// admission snapshot (lifecycle §8.1). No incarnation is listed here: a
/// Client that received no body is never claimed as a copy holder.
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
    /// The composition's Client-transient plumbing. A `ClientIncarnation` owner
    /// snapshotted by an operation must stay resolvable after a Host restart:
    /// the demand plumbing does not survive it, but the owner identity is the
    /// deterministic projection of the Client boot incarnation the connection
    /// table is keyed by, so the participant can be reconstructed for the
    /// exact durable owner (lifecycle §8.1, §14).
    client_transients: Option<Arc<crate::transient_erasure::ClientTransientRegistry>>,
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

    /// Installs the composition's Client-transient plumbing, so a
    /// `ClientIncarnation` owner in a durable participant snapshot resolves
    /// through the current connection table after a restart (lifecycle §8.1).
    pub(crate) fn install_client_transients(
        &mut self,
        registry: Arc<crate::transient_erasure::ClientTransientRegistry>,
    ) {
        self.client_transients = Some(registry);
    }

    /// Issues one bounded demand. An owner without an implementation reports
    /// [`ParticipantHoldClass::Unsupported`] for the exact demanded condition,
    /// so the durable snapshot distinguishes "not implemented yet" from
    /// "pending" and from success. A `ClientIncarnation` owner is resolved
    /// against the current connection table instead: a reachable incarnation
    /// is demanded, and an unreachable one holds as `Unavailable` — never as a
    /// composition defect.
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
/// (the durable participant aggregate plus the system-wide remainder probe
/// are, and only the sealed completion boundary re-derives them).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TargetedDeletionPassOutcome {
    /// Unfinished operations examined.
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
    /// Operations that entered the durable `Finalizing` marker during this
    /// pass (including a resumed finalizing operation observed again).
    pub finalizing: u32,
    /// Operations whose global completion commit ran during this pass.
    pub finalized: u32,
    /// Operations that found collected target data in the §12 step-1
    /// re-check and returned to `Active` on a new sweep.
    pub remainder_sweeps: u32,
    /// Bounded covered-source reconciliation pages published during this pass
    /// (lifecycle §4.1 point 4). A page is work, never a completion decision.
    pub reconciliation_pages: u32,
}

impl TargetedDeletionPassOutcome {
    /// Whether one pass advanced durable work: it demanded a participant,
    /// verified one, moved an operation into `Finalizing`, committed a
    /// completion, or opened a remainder sweep.
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

    /// Folds another pass's counters into this one.
    pub(crate) fn accumulate(&mut self, other: Self) {
        self.operations = self.operations.saturating_add(other.operations);
        self.demands = self.demands.saturating_add(other.demands);
        self.verified = self.verified.saturating_add(other.verified);
        self.unfinished = self.unfinished.saturating_add(other.unfinished);
        self.held = self.held.saturating_add(other.held);
        self.stale_reports = self.stale_reports.saturating_add(other.stale_reports);
        self.finalizing = self.finalizing.saturating_add(other.finalizing);
        self.finalized = self.finalized.saturating_add(other.finalized);
        self.remainder_sweeps = self.remainder_sweeps.saturating_add(other.remainder_sweeps);
        self.reconciliation_pages = self
            .reconciliation_pages
            .saturating_add(other.reconciliation_pages);
    }
}

/// Bounded passes one startup/confirmation drive runs before it stops and
/// leaves the rest to the serving tick.
///
/// Each pass is itself bounded by [`TargetedDeletionPass`]; this budget caps
/// how many continuation passes a single caller runs, so a participant that
/// always reports more work can never make startup or a confirmation
/// unbounded.
pub(crate) const BOUNDED_DRIVE_PASS_BUDGET: u32 = 8;

/// Upper bound on the held-retry skip shift: the skip doubles per consecutive
/// unanswered retry up to `2^3` ticks and then stays there.
const HELD_RETRY_MAX_SKIP_SHIFT: u32 = 3;

/// Bounded covered-source reconciliation pages one operation may publish in a
/// single fan-out pass.
///
/// Reconciliation must finish before the operation's participants erase (the
/// identity bodies are the evidence the in-flight-use correspondence is
/// derived from) and before completion; the per-pass budget keeps one pass
/// bounded while the durable cursor makes the walk resumable across passes,
/// ticks, and restarts. The budget is work pacing only: it never decides
/// completion, and a larger covered set simply takes more passes.
const RECONCILIATION_PAGES_PER_PASS: u32 = 8;

/// Whether the bounded pass parameters are inside their contract.
fn valid_pass(pass: TargetedDeletionPass) -> bool {
    (1..=100).contains(&pass.operation_limit) && pass.demands_per_participant > 0
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
                        // The sweep or phase moved while this pass was driving:
                        // the remaining participants belong to a decision this
                        // pass must not make.
                        break;
                    }
                }
                // A crash between the finalizing marker and the completion
                // commit resumes here; the operation identity, the current
                // condition, and the participant statuses come from durable
                // state, never from memory defaults (§14).
                DeletionOperationPhase::Finalizing => {
                    settle_finalizing(store, record.current, &mut outcome).await?;
                }
                DeletionOperationPhase::Held | DeletionOperationPhase::Completed => {}
            }
        }
        if page_len < limit as usize {
            break;
        }
    }
    Ok(outcome)
}

/// Runs the sealed completion boundary for one operation whose participant
/// work is finished.
///
/// Both steps re-derive every premise from durable state: the begin step
/// refuses unless the durable aggregate says every required participant is
/// `Verified` for the current sweep, and the completion step re-runs the
/// system-wide remainder probe inside its commit transaction. A remainder
/// returns the operation to `Active` on a new sweep without destroying
/// material, so this pass never completes over collected data.
async fn settle_finalizing(
    store: &Store,
    current: DeletionOperationRef,
    outcome: &mut TargetedDeletionPassOutcome,
) -> Result<(), CoreError> {
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
        // Not every participant is verified yet, the current sweep's covered
        // source walk is still incomplete, or the operation left `Active`
        // meanwhile: nothing to finalize.
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
        // The completion commit re-checks every premise; anything else leaves
        // the operation unfinished for a later pass.
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
    // Phase 1: exhaust the covered-source reconciliation before the first
    // participant demand. The identity bodies carrying the target are the
    // evidence the already-claimed in-flight-use correspondence is derived
    // from, and the owner sweeps redact them; publishing and associating
    // first is what keeps the correspondence complete regardless of how many
    // covered identities exist. The page budget bounds one pass; the durable
    // cursor resumes the walk on the next pass or after a restart.
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
            // A concurrent lifecycle advance moved the operation past the
            // walk; this pass must not drive its participants.
            DeletionReconciliationOutcome::Finalizing
            | DeletionReconciliationOutcome::Completed => return Ok(advanced_any),
            DeletionReconciliationOutcome::Missing | DeletionReconciliationOutcome::StaleSweep => {
                outcome.stale_reports += 1;
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
                // Currentness boundary for this demand's source set: the
                // canonical coverage is re-read only after reconciliation
                // reported Complete for `current`, and again immediately
                // before this demand so a source this pass just published
                // (or one another writer published during an earlier
                // participant) is in the scope Learning pin-correlation
                // walks. A handle taken before the walk would omit
                // identities past the admission page; Missing/Destroyed
                // here means a concurrent lifecycle transition ended this
                // pass's authority and the pass must not mutate with a
                // stale snapshot. The page size is a work bound, never a
                // coverage bound. Each participant's actual erase
                // transaction re-checks `condition_is_current` on the same
                // Immediate writer; this read only chooses the demand
                // scope, it is not the mutation gate.
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
        return Ok(true);
    }
    // The participant work is (now) complete: try the sealed completion
    // boundary. It re-reads the durable aggregate and the system-wide
    // remainder itself, so this call is safe to repeat and safe to skip.
    settle_finalizing(store, current, outcome).await?;
    Ok(true)
}

/// Drives bounded passes until a pass advances no durable work or the budget
/// is exhausted.
///
/// This is the continuation driver behind startup recovery and the post-
/// confirmation kick: each iteration is one [`drive_targeted_deletion`] pass
/// with its own bounded operation/demand limits, and the loop stops as soon as
/// a pass reports no demand, verification, finalizing step, completion, or
/// remainder sweep. It never decides completion itself — the sealed store
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
/// Returns [`CoreError::Deletion`] for an invalid pass or budget and when the
/// canonical store refuses.
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
    resume_retryable_holds(store, pass.operation_limit).await?;
    drive_targeted_deletion_until_settled(store, registry, pass, pass_budget).await
}

/// Resumes the retryable holds inside one bounded operation page.
///
/// Only `Held(Unavailable)` is a resume candidate. `GenerationExhausted`
/// cannot be resumed by construction, every other phase is not a hold, and
/// the canonical store re-checks all of that inside its own write
/// transaction; this read only decides which candidates to offer.
async fn resume_retryable_holds(store: &Store, limit: u32) -> Result<(), CoreError> {
    let page = store
        .unfinished_deletions(None, limit)
        .await
        .map_err(deletion_error)?;
    for record in page {
        if record.phase == DeletionOperationPhase::Held
            && record.hold == Some(ene_preservation::DeletionHoldReason::Unavailable)
        {
            // The store re-checks the phase, sweep, and hold class inside its
            // write transaction; a refusal (another writer moved the
            // operation, or the durable state cannot resume) leaves it for the
            // bounded drive below without inventing an outcome here.
            store
                .change_deletion_lifecycle(record.current, DeletionLifecycleChange::Resume)
                .await
                .map_err(deletion_error)?;
        }
    }
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
#[derive(Debug, Default)]
pub(crate) struct HeldRetrySchedule {
    tick: u64,
    retries: HashMap<DeletionOperationId, HoldRetry>,
}

#[derive(Debug, Clone, Copy)]
struct HoldRetry {
    /// Consecutive resume attempts that have not answered.
    attempts: u32,
    /// First tick at which the next resume is allowed.
    next_eligible_tick: u64,
}

impl HeldRetrySchedule {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Advances the schedule by one tick.
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

    /// Drops scheduling state for operations that are no longer retryable
    /// holds.
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
    let page = store
        .unfinished_deletions(None, pass.operation_limit)
        .await
        .map_err(deletion_error)?;
    let retryable: HashSet<DeletionOperationId> = page
        .iter()
        .filter(|record| {
            record.phase == DeletionOperationPhase::Held
                && record.hold == Some(ene_preservation::DeletionHoldReason::Unavailable)
        })
        .map(|record| record.current.operation)
        .collect();
    schedule.retain_only(&retryable);
    for record in &page {
        if !retryable.contains(&record.current.operation)
            || !schedule.eligible(record.current.operation, tick)
        {
            continue;
        }
        match store
            .change_deletion_lifecycle(record.current, DeletionLifecycleChange::Resume)
            .await
            .map_err(deletion_error)?
        {
            DeletionLifecycleOutcome::Applied(_) => {
                schedule.record_resume(record.current.operation, tick);
            }
            // The durable state moved or refuses the resume; the next tick
            // re-reads the phase instead of guessing.
            DeletionLifecycleOutcome::Missing
            | DeletionLifecycleOutcome::StaleSweep
            | DeletionLifecycleOutcome::Completed
            | DeletionLifecycleOutcome::Held(_)
            | DeletionLifecycleOutcome::Finalizing => {}
        }
    }
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
        DeletionOperationId, DeletionOperationPhase, DeletionOperationRecord, DeletionOperationRef,
        DeletionPurpose, DeletionSearchMaterial, DeletionSweepGeneration, ErasureParticipant,
        MechanicalDeletionTarget, ParticipantCompletionFact, ParticipantCompletionStatus,
        ParticipantHoldClass, ParticipantOwnerRef, ParticipantProgress,
        StageTargetedDeletionRequestCommand, StageTargetedDeletionRequestOutcome,
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
            total.operations += outcome.operations;
            total.demands += outcome.demands;
            total.verified += outcome.verified;
            total.unfinished += outcome.unfinished;
            total.held += outcome.held;
            total.stale_reports += outcome.stale_reports;
            total.finalizing += outcome.finalizing;
            total.finalized += outcome.finalized;
            total.remainder_sweeps += outcome.remainder_sweeps;
            total.reconciliation_pages += outcome.reconciliation_pages;
            if outcome.held == 0
                && outcome.unfinished == 0
                && outcome.demands == 0
                && outcome.reconciliation_pages == 0
            {
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

        let current = admit(
            &handle,
            target,
            vec![
                ParticipantOwnerRef::Companion,
                ParticipantOwnerRef::Learning,
            ],
        )
        .await;
        let outcome = drive_until_settled(&handle).await;
        assert_eq!(outcome.held, 0, "semantic derived data must not hold");
        assert_eq!(outcome.unfinished, 0);
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

        let outcome = drive_until_settled(&handle).await;
        assert_eq!(outcome.held, 0);
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
        assert_eq!(audit.verified_count(), 2);
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
                ParticipantErasureScope::correlation_only(vec![]),
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
            CredentialSetRepository as _, DevicePairingRepository as _, DevicePairingStatus,
            RegistrationFingerprint,
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
                    base: String::from("consent-none"),
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
            base: String::from("consent-none"),
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
            .request_pairing(format!("{target} phone"), String::from("conn-a3d"), None)
            .await
            .unwrap();
        let DevicePairingStatus::Pending { pending } = pairing else {
            panic!("a fresh pairing request must be pending");
        };
        assert!(
            handle
                .store
                .approve_pending(&pending.pending_id, "conn-a3d")
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
            (first.demands, first.verified, first.unfinished, first.held),
            (3, 3, 0, 0),
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
        assert_eq!(settled.held, 0);
        assert_eq!(settled.unfinished, 0);
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
        assert_eq!(audit.verified_count(), required.len() as u64);
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
        let mut after = None;
        loop {
            let page = handle
                .store
                .deletion_participants(current.operation, after, 100)
                .await
                .unwrap();
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
                    .unwrap();
            }
            if page_len < 100 {
                break;
            }
        }
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
        let held = handle.run_targeted_deletion_tick().await.unwrap();
        assert_eq!(held.held as usize, required.len());
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
        let first = handle.run_targeted_deletion_tick().await.unwrap();
        assert_eq!(first.held, 1);
        assert_eq!(participant.calls(), 1);
        // Ticks 2 and 3 retry immediately after the first retry (skip 1 then
        // 2), then tick 4 must be skipped by the doubled backoff.
        let second = handle.run_targeted_deletion_tick().await.unwrap();
        assert_eq!(second.held, 1);
        assert_eq!(participant.calls(), 2);
        let third = handle.run_targeted_deletion_tick().await.unwrap();
        assert_eq!(third.held, 1);
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
        let fifth = handle.run_targeted_deletion_tick().await.unwrap();
        assert_eq!(fifth.held, 1);
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
        assert_eq!(ninth.held, 1);
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
        assert_eq!(audit.verified_count(), 2);
        // A later tick over the completed operation demands nothing and can
        // never reopen it.
        let idle = handle.run_targeted_deletion_tick().await.unwrap();
        assert_eq!((idle.demands, idle.finalized), (0, 0));
        assert_eq!(
            operation_record(&handle, current.operation).await.phase,
            DeletionOperationPhase::Completed
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
