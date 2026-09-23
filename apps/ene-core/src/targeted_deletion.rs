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
    /// Typed HostTransient handle so Verified-commit and finalizing can share
    /// the process-local arrival gate with Learning enqueue. The same `Arc`
    /// is also in `participants`.
    host_transient: Option<Arc<crate::transient_erasure::HostTransientParticipant>>,
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

    /// Registers the typed HostTransient participant so Verified-commit and
    /// finalizing share its process-local arrival gate.
    pub(crate) fn register_host_transient(
        &mut self,
        participant: Arc<crate::transient_erasure::HostTransientParticipant>,
    ) -> Result<(), ParticipantOwnerRef> {
        if self
            .participants
            .contains_key(&ParticipantOwnerRef::HostTransient)
        {
            return Err(ParticipantOwnerRef::HostTransient);
        }
        self.host_transient = Some(Arc::clone(&participant));
        self.participants
            .insert(ParticipantOwnerRef::HostTransient, participant);
        Ok(())
    }

    /// Records one completion fact. HostTransient Verified facts go through
    /// the arrival gate so a concurrent Learning enqueue cannot land between
    /// the in-memory fact and the durable participant row.
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
///
/// Covered-source identities are not copied into the command: Learning and
/// other semantic owners probe the canonical `(operation, sweep, source)`
/// primary key for each candidate they inspect, so one demand stays bounded
/// in its own page rather than in the whole sweep.
fn command_scope(
    material: &DeletionOperationMaterial,
    owner: ParticipantOwnerRef,
) -> ParticipantErasureScope {
    if owner.is_incarnation() {
        ParticipantErasureScope::correlation_only(Vec::new())
    } else {
        ParticipantErasureScope::local(material.target().clone(), Vec::new())
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
    if let Some(host) = &registry.host_transient {
        host.publish_owed_arrivals().await;
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
                    settle_finalizing(store, registry, record.current, &mut outcome).await?;
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
        // Finalizing proceeds only when inflight_pins == 0, this operation
        // does not owe an unpublished HostTransient arrival, and either the
        // bounded global walk is complete with a matching generation or a
        // direct classification proves this operation is unrelated to the
        // live remainder. An incomplete global walk does not block every
        // unfinished deletion. Unrelated enqueue may bump
        // `mutation_generation` without resetting this participant.
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
                // Currentness boundary for this demand's protected target: the
                // canonical material is re-read only after reconciliation
                // reported Complete for `current`, and again immediately
                // before this demand so a concurrent wipe cannot hand a
                // destroyed target to a participant. Covered-source
                // membership is not snapshotted here; the owner probes the
                // `(operation, sweep, source)` primary key for each
                // candidate it inspects. Missing/Destroyed means a
                // concurrent lifecycle transition ended this pass's
                // authority. The page size is a work bound, never a
                // coverage bound. Each participant's actual erase
                // transaction re-checks `condition_is_current` on the same
                // Immediate writer; this read only chooses the demand
                // target, it is not the mutation gate.
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
    settle_finalizing(store, registry, current, outcome).await?;
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
