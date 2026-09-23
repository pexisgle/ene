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
    pub unfinished: u32,
    pub held: u32,
    pub stale_reports: u32,
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

pub(crate) const BOUNDED_DRIVE_PASS_BUDGET: u32 = 8;

const HELD_RETRY_MAX_SKIP_SHIFT: u32 = 3;

const RECONCILIATION_PAGES_PER_PASS: u32 = 8;

fn valid_pass(pass: TargetedDeletionPass) -> bool {
    (1..=100).contains(&pass.operation_limit) && pass.demands_per_participant > 0
}

fn deletion_error(error: ene_preservation::PreservationTechnicalError) -> CoreError {
    CoreError::Deletion(error.to_string())
}

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
            break;
        }
    }
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
        store
            .change_deletion_lifecycle(current, DeletionLifecycleChange::Hold)
            .await
            .map_err(deletion_error)?;
        return Ok(true);
    }
    settle_finalizing(store, registry, current, outcome).await?;
    Ok(true)
}

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
    resume_retryable_holds(store, pass.operation_limit).await?;
    drive_targeted_deletion_until_settled(store, registry, pass, pass_budget).await
}

async fn resume_retryable_holds(store: &Store, limit: u32) -> Result<(), CoreError> {
    let page = store
        .unfinished_deletions(None, limit)
        .await
        .map_err(deletion_error)?;
    for record in page {
        if record.phase == DeletionOperationPhase::Held
            && record.hold == Some(ene_preservation::DeletionHoldReason::Unavailable)
        {
            store
                .change_deletion_lifecycle(record.current, DeletionLifecycleChange::Resume)
                .await
                .map_err(deletion_error)?;
        }
    }
    Ok(())
}

#[derive(Debug, Default)]
pub(crate) struct HeldRetrySchedule {
    tick: u64,
    retries: HashMap<DeletionOperationId, HoldRetry>,
}

#[derive(Debug, Clone, Copy)]
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
            DeletionLifecycleOutcome::Missing
            | DeletionLifecycleOutcome::StaleSweep
            | DeletionLifecycleOutcome::Completed
            | DeletionLifecycleOutcome::Held(_)
            | DeletionLifecycleOutcome::Finalizing => {}
        }
    }
    drive_targeted_deletion(store, registry, pass).await
}
