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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TargetedDeletionPassOutcome {
    pub operations: u32,
    pub demands: u32,
    pub verified: u32,
    pub finalizing: u32,
    pub finalized: u32,
    pub remainder_sweeps: u32,
    pub reconciliation_pages: u32,
}

impl TargetedDeletionPassOutcome {
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

struct UnfinishedWalk {
    walk: DeletionWalk,
    after: Option<DeletionOperationId>,
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

    fn examined(&mut self, operation: DeletionOperationId) {
        self.after = Some(operation);
    }

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

async fn drive_operation(
    store: &Store,
    registry: &ErasureParticipantRegistry,
    current: DeletionOperationRef,
    demand_budget: u32,
    outcome: &mut TargetedDeletionPassOutcome,
) -> Result<(), CoreError> {
    let condition = current.condition();
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

pub(crate) async fn recover_targeted_deletions(
    store: &Store,
    registry: &ErasureParticipantRegistry,
    pass_budget: u32,
) -> Result<TargetedDeletionPassOutcome, CoreError> {
    let pass = TargetedDeletionPass::default();
    resume_hold_page(
        store,
        pass.operation_limit,
        &mut HeldRetrySchedule::new(),
        1,
    )
    .await?;
    drive_targeted_deletion_until_settled(store, registry, pass_budget).await
}

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
        match store
            .change_deletion_lifecycle(record.current, DeletionLifecycleChange::Resume)
            .await
            .map_err(deletion_error)?
        {
            DeletionLifecycleOutcome::Applied(_) => {
                schedule.record_resume(record.current.operation, tick);
            }
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
