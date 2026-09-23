mod companion_learning;
mod remainder;
mod task_action_inference;

pub use companion_learning::{companion_erasure_participant, learning_erasure_participant};
pub(crate) use remainder::system_remainder;
pub use task_action_inference::{
    action_erasure_participant, inference_erasure_participant, task_erasure_participant,
};

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};

use ene_preservation::{
    DemandLocalErasureCommand, ErasureConditionRef, ErasureParticipant, MechanicalDeletionTarget,
    ParticipantCompletionFact, ParticipantHoldClass, ParticipantOwnerRef,
};
use ene_primitive::WallClockWithTz;
use rusqlite::{Transaction, TransactionBehavior};

use crate::Store;
use crate::codec::lock_shared;
use crate::run_blocking;

pub(crate) const ERASED_MARKER: &str = "[erased]";

pub(crate) const ERASURE_BATCH_ROWS: i64 = 500;

pub(crate) const ERASURE_SCAN_ROWS: u32 = 64;

pub(crate) fn erasure_count(value: impl TryInto<u64>) -> Result<u64, String> {
    value
        .try_into()
        .map_err(|_| String::from("count out of range"))
}

pub(crate) fn redact_exact(text: &str, target: &str) -> Option<(String, u64)> {
    if target.is_empty() || !text.contains(target) {
        return None;
    }
    let mut removed = count_occurrences(text, target);
    let mut current = text.replace(target, ERASED_MARKER);
    while current.contains(target) {
        removed += count_occurrences(&current, target);
        current = current.replace(target, "");
    }
    Some((current, removed))
}

fn count_occurrences(text: &str, target: &str) -> u64 {
    text.matches(target).count() as u64
}

#[derive(Debug, Default)]
struct PageOutcome {
    scanned: u32,
    matched: u64,
    deleted: u64,
    last: Option<(String, i64)>,
}

struct PageRequest<'a> {
    target: &'a str,
    after: &'a str,
    after_ordinal: i64,
    limit: i64,
    delete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SweepPhase {
    Erase,
    Verify,
}

#[derive(Debug, Clone)]
struct SweepCursor {
    condition: ErasureConditionRef,
    phase: SweepPhase,
    table: usize,
    after: Option<String>,
    after_ordinal: i64,
    erased: u64,
    remainder: u64,
    verified: bool,
}

impl SweepCursor {
    fn fresh(condition: ErasureConditionRef) -> Self {
        Self {
            condition,
            phase: SweepPhase::Erase,
            table: 0,
            after: None,
            after_ordinal: 0,
            erased: 0,
            remainder: 0,
            verified: false,
        }
    }

    fn reset_position(&mut self) {
        self.after = None;
        self.after_ordinal = 0;
    }

    fn begin_verify(&mut self) {
        self.phase = SweepPhase::Verify;
        self.table = 0;
        self.remainder = 0;
        self.reset_position();
    }

    fn fact(&self, owner: ParticipantOwnerRef, at: WallClockWithTz) -> ParticipantCompletionFact {
        if self.verified {
            ParticipantCompletionFact::verified(self.condition, owner, self.erased, at)
        } else if self.phase == SweepPhase::Verify {
            ParticipantCompletionFact::local_complete(self.condition, owner, self.erased, 0, at)
        } else {
            ParticipantCompletionFact::more_work(
                self.condition,
                owner,
                self.erased,
                self.remainder,
                at,
            )
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErasurePageError {
    Storage,
    Unrepresentable,
    NotCurrent,
}

impl From<rusqlite::Error> for ErasurePageError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Storage
    }
}

type LocalStep = fn(
    tx: &Transaction<'_>,
    cursor: &mut SweepCursor,
    target: &str,
) -> Result<(), ErasurePageError>;

fn bounded_step(
    tx: &Transaction<'_>,
    cursor: &mut SweepCursor,
    target: &str,
    table_count: usize,
    mut page: impl FnMut(
        &Transaction<'_>,
        usize,
        &PageRequest<'_>,
    ) -> Result<PageOutcome, ErasurePageError>,
) -> Result<(), ErasurePageError> {
    let mut budget = ERASURE_SCAN_ROWS;
    while budget > 0 {
        if cursor.table >= table_count {
            if cursor.phase == SweepPhase::Erase {
                cursor.begin_verify();
                continue;
            }
            cursor.verified = true;
            break;
        }
        let limit = i64::from(budget);
        let after = cursor.after.clone().unwrap_or_default();
        let erasing = cursor.phase == SweepPhase::Erase;
        let request = PageRequest {
            target,
            after: &after,
            after_ordinal: cursor.after_ordinal,
            limit,
            delete: erasing,
        };
        let outcome = page(tx, cursor.table, &request)?;
        budget = budget.saturating_sub(outcome.scanned);
        if outcome.matched > 0 && !erasing {
            cursor.remainder = outcome.matched;
            cursor.phase = SweepPhase::Erase;
            cursor.reset_position();
            continue;
        }
        if erasing {
            cursor.erased += outcome.deleted;
        }
        if outcome.scanned < u32::try_from(limit).unwrap_or(u32::MAX) {
            cursor.table += 1;
            cursor.reset_position();
        } else if let Some((last, ordinal)) = outcome.last {
            cursor.after = Some(last);
            cursor.after_ordinal = ordinal;
        }
    }
    Ok(())
}

fn lock_cursors(
    cursors: &Mutex<HashMap<ErasureConditionRef, SweepCursor>>,
) -> MutexGuard<'_, HashMap<ErasureConditionRef, SweepCursor>> {
    match cursors.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn exact_text(command: &DemandLocalErasureCommand) -> Option<String> {
    let target = command.scope().target()?;
    let MechanicalDeletionTarget::ExactText(material) = &target.mechanical;
    let text = material.expose_for_erasure();
    if text.is_empty() {
        return None;
    }
    Some(text.to_owned())
}

fn run_local_demand(
    store: &Store,
    cursors: &Mutex<HashMap<ErasureConditionRef, SweepCursor>>,
    condition: ErasureConditionRef,
    owner: ParticipantOwnerRef,
    target: &str,
    step: LocalStep,
) -> Result<ParticipantCompletionFact, ErasurePageError> {
    let mut guard = lock_shared(&store.conn);
    let tx = guard.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let at = WallClockWithTz::now();
    if !crate::preservation::condition_is_current(&tx, condition)? {
        lock_cursors(cursors).remove(&condition);
        return Err(ErasurePageError::NotCurrent);
    }
    let mut cursor = lock_cursors(cursors)
        .get(&condition)
        .cloned()
        .unwrap_or_else(|| SweepCursor::fresh(condition));
    step(&tx, &mut cursor, target)?;
    tx.commit()?;
    let fact = cursor.fact(owner, at);
    let mut cursors = lock_cursors(cursors);
    if cursor.verified {
        cursors.remove(&condition);
    } else {
        cursors.retain(|existing, _| {
            existing.operation != condition.operation || existing.sweep == condition.sweep
        });
        cursors.insert(condition, cursor);
    }
    Ok(fact)
}

async fn local_demand(
    store: Store,
    sweep: Arc<Mutex<HashMap<ErasureConditionRef, SweepCursor>>>,
    command: DemandLocalErasureCommand,
    step: LocalStep,
) -> ParticipantCompletionFact {
    #[cfg(any(test, feature = "test-support"))]
    store.test_parks.erasure_mutation.pause_if_armed().await;
    let condition = command.condition();
    let owner = command.participant();
    let held =
        |reason| ParticipantCompletionFact::held(condition, owner, reason, WallClockWithTz::now());
    let Some(target) = exact_text(&command) else {
        return held(ParticipantHoldClass::Failed);
    };
    match run_blocking(move || run_local_demand(&store, &sweep, condition, owner, &target, step))
        .await
    {
        Ok(fact) => fact,
        Err(ErasurePageError::Unrepresentable) => held(ParticipantHoldClass::Failed),
        Err(ErasurePageError::Storage) => held(ParticipantHoldClass::Unavailable),
        Err(ErasurePageError::NotCurrent) => ParticipantCompletionFact::local_complete(
            condition,
            owner,
            0,
            0,
            WallClockWithTz::now(),
        ),
    }
}

pub struct LocalErasureParticipant {
    owner: ParticipantOwnerRef,
    store: Store,
    step: LocalStep,
    sweep: Arc<Mutex<HashMap<ErasureConditionRef, SweepCursor>>>,
}

impl LocalErasureParticipant {
    fn new(owner: ParticipantOwnerRef, step: LocalStep, store: Store) -> Self {
        Self {
            owner,
            store,
            step,
            sweep: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl ErasureParticipant for LocalErasureParticipant {
    fn owner(&self) -> ParticipantOwnerRef {
        self.owner
    }

    fn demand_local_erasure(
        &self,
        command: DemandLocalErasureCommand,
    ) -> Pin<Box<dyn std::future::Future<Output = ParticipantCompletionFact> + Send + '_>> {
        Box::pin(local_demand(
            self.store.clone(),
            Arc::clone(&self.sweep),
            command,
            self.step,
        ))
    }
}
