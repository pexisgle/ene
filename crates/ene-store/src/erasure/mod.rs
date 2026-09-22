//! Built-in local-erasure participants for the durable owners `ene-store`
//! serves.
//!
//! `ene-preservation` owns the [`ErasureParticipant`] contract and never
//! depends on a concrete participant; each owner's durable master lives in one
//! `ene-store` domain module, and this crate implements the participant next to
//! that master so a sweep writes only its own owner's rows. The Host
//! composition registers the implementations and performs the fan-out.
//!
//! [`ErasureParticipant`]: ene_preservation::ErasureParticipant
//!
//! `companion_learning` covers the Companion and Learning owners
//! (`history_message`, `activity_record`, the undelivered references, and the
//! Summary / Memory / revision / token-index surfaces). `task_action_inference`
//! covers the Task, Action, and Inference owners. Owners whose domain crate
//! defines a port instead (`ene-permission`, `ene-credential`, `ene-presence`)
//! implement their participants behind those ports.

// The five owner registration names keep the existing `X::new(store)` entry
// point the Host composition calls; each constructs the single shared
// participant implementation rather than itself.
#![expect(
    clippy::new_ret_no_self,
    reason = "owner registration names construct the shared participant"
)]

mod companion_learning;
mod remainder;
mod task_action_inference;

pub use companion_learning::{CompanionErasureParticipant, LearningErasureParticipant};
pub(crate) use remainder::system_remainder;
pub use task_action_inference::{
    ActionErasureParticipant, InferenceErasureParticipant, TaskErasureParticipant,
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

/// The fixed marker a mechanically redacted value keeps in place of the
/// target. It carries no target-derived material and is never treated as a
/// match for a non-overlapping target.
pub(crate) const ERASED_MARKER: &str = "[erased]";

const MARKER_PASS_BOUND: usize = 8;

/// Bounded rows mutated per table in one local erasure pass (lifecycle §9). A
/// pass that hits the bound reports its remainder and the next demand re-scans
/// from the start: erased rows no longer match, so re-scanning is progress and
/// needs no continuation cursor.
pub(crate) const ERASURE_BATCH_ROWS: i64 = 500;

/// Rows one bounded owner demand scans before reporting more work. The value
/// bounds the SQL, the redaction work, and the caller's wait; a longer sweep
/// simply spans more demands. The two local owners share the bound so their
/// sweep budgets cannot diverge.
pub(crate) const ROWS_PER_DEMAND: u32 = 64;

/// One count-width policy for every owner's erasure result (`usize` row
/// counts and `i64` remainders both widen to `u64`): a value that does not fit
/// is a technical error at the call site, never a wrapped count.
pub(crate) fn erasure_count(value: impl TryInto<u64>) -> Result<u64, String> {
    value
        .try_into()
        .map_err(|_| String::from("count out of range"))
}

/// Mechanically removes every occurrence of `target` from `text`.
///
/// Returns [`None`] when the value contains no occurrence (no write needed).
/// A replacement can join the surrounding text into a new occurrence, so the
/// pass repeats until the value is clean; a target that overlaps the marker
/// itself falls back to outright removal, which strictly shortens the value
/// and therefore always reaches a clean fixpoint. The returned count is the
/// number of occurrences removed.
///
/// This is the one mechanical predicate the A3 owner sweeps and the A4
/// acceptance boundaries share: a body an accepting boundary redacts and a
/// body an owner sweep redacts are erased (or refused) by the same rule.
pub(crate) fn redact_exact(text: &str, target: &str) -> Option<(String, u64)> {
    if target.is_empty() || !text.contains(target) {
        return None;
    }
    let mut removed = count_occurrences(text, target);
    let mut current = text.replace(target, ERASED_MARKER);
    for _ in 0..MARKER_PASS_BOUND {
        if !current.contains(target) {
            return Some((current, removed));
        }
        removed += count_occurrences(&current, target);
        current = current.replace(target, ERASED_MARKER);
    }
    while current.contains(target) {
        removed += count_occurrences(&current, target);
        current = current.replace(target, "");
    }
    Some((current, removed))
}

fn count_occurrences(text: &str, target: &str) -> u64 {
    text.matches(target).count() as u64
}

/// The public name of the shared per-demand scanned-row bound.
///
/// The bound is on scanned rows, not matched rows: a table without a match is
/// still walked one page at a time, so no demand performs an unbounded scan
/// and the remainder check is a bounded traversal rather than one query that
/// reads the whole table.
pub const ERASURE_SCAN_ROWS: u32 = ROWS_PER_DEMAND;

/// One scanned page of one owner table or row set.
#[derive(Debug, Default)]
struct PageOutcome {
    /// Rows the page walked.
    scanned: u32,
    /// Rows the mechanical predicate matched.
    matched: u64,
    /// Rows actually deleted (zero on a verification page).
    deleted: u64,
    /// Last scanned key and its ordinal component, when the page was not
    /// empty.
    last: Option<(String, i64)>,
}

/// One bounded page request within a sweep.
struct PageRequest<'a> {
    /// Exact mechanical target.
    target: &'a str,
    /// Last scanned key of the table; the empty string is the head.
    after: &'a str,
    /// Second primary-key component (revision number, or rowid).
    after_ordinal: i64,
    /// Rows the page may scan.
    limit: i64,
    /// `true` erases the page's matches; `false` is the verification walk.
    delete: bool,
}

/// Which stage of one sweep the participant is in. `Verify` re-walks exactly
/// the same rows as `Erase`; only a completed `Verify` walk with zero matches
/// produces a verified fact (§10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SweepPhase {
    Erase,
    Verify,
}

/// Participant-owned continuation state for one sweep (§9).
///
/// Keyed by the condition it belongs to. A demand for another
/// `(operation, sweep)` starts a fresh cursor, so an older generation's
/// position never advances a newer sweep and a completion is never mixed
/// across generations. The cursor is deliberately not durable: a restart
/// restarts the sweep from its head, which is safe because every erase action
/// is idempotent and the durable rows carry no cursor state to corrupt.
#[derive(Debug, Clone)]
struct SweepCursor {
    condition: ErasureConditionRef,
    phase: SweepPhase,
    /// Index into the owner's table order; one past the end means the walk
    /// finished its current phase.
    table: usize,
    /// Last scanned primary-key component in the current table. The empty
    /// string is the smallest stored identity text, so it is the head.
    after: Option<String>,
    /// Second primary-key component for the composite revision key (the
    /// revision number), and the rowid keyset for the derived token table.
    after_ordinal: i64,
    /// Rows erased so far in this sweep, including cascaded rows.
    erased: u64,
    /// Rows a verification page found before returning to erasing.
    remainder: u64,
    /// The verify walk completed with zero matches for this sweep.
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

/// Failure of one bounded page. [`Self::Storage`] and
/// [`Self::Unrepresentable`] become explicit holds; [`Self::NotCurrent`]
/// mutates nothing, drops the cursor, and returns `LocalComplete`, so the
/// canonical record refuses the stale generation. No variant is a silent
/// success and none carries row content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErasurePageError {
    /// The store could not read or write the page.
    Storage,
    /// The owner cannot store a target-free value its own read path accepts
    /// (the redacted path would not be a readable locator and the fixed
    /// marker itself contains the target). Fail closed instead of writing an
    /// unreadable row or a value that still contains the target.
    Unrepresentable,
    /// The demanded condition is no longer the operation's current unfinished
    /// condition. The page mutates nothing.
    NotCurrent,
}

impl From<rusqlite::Error> for ErasurePageError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Storage
    }
}

/// The step function one owner applies to a bounded budget of scanned rows.
type LocalStep = fn(
    tx: &Transaction<'_>,
    cursor: &mut SweepCursor,
    target: &str,
) -> Result<(), ErasurePageError>;

/// Runs one bounded budget of scanned rows over an owner's table order: the
/// shared erase→verify walk every local owner drives. The per-owner dispatch
/// decides what one page of a table means; the budget, transition, and
/// continuation update stay identical so the sweeps cannot drift.
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
            // Verification found rows the erase walk must remove: re-walk
            // from the head of the table that still holds them.
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

/// Protected exact-text material of one demand, or [`None`] when the demand
/// carries no usable mechanical target (a correlation-only scope or an empty
/// text). A local owner can never erase mechanically without it.
fn exact_text(command: &DemandLocalErasureCommand) -> Option<String> {
    let target = command.scope().target()?;
    let MechanicalDeletionTarget::ExactText(material) = &target.mechanical;
    let text = material.expose_for_erasure();
    if text.is_empty() {
        return None;
    }
    Some(text.to_owned())
}

/// Runs one bounded demand inside a single `Immediate` transaction: either
/// the whole demand's erase actions commit, or none of them does, so a
/// re-driven demand repeats only idempotent work. The in-memory cursor is
/// updated only after the commit: a rollback leaves the continuation where it
/// was, and a crash that loses it restarts the sweep from the head.
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
    // Every store connection raises `secure_delete` at open, so deleted cells
    // are zeroed instead of being left recoverable in freed pages.
    let at = WallClockWithTz::now();
    // The demand was admitted against a then-current condition. Actual
    // mutation re-reads canonical currentness in this same Immediate
    // transaction: a completed operation or a superseded sweep must not
    // change a byte of target-bearing state, including a fresh origin the
    // Owner provided after closure.
    if !crate::preservation::condition_is_current(&tx, condition)? {
        // The sweep is not finishing; drop its continuation so a later demand
        // for the same condition starts from a clean head.
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
    // One continuation per unfinished condition: a verified sweep is finished
    // and leaves no entry, and a demand for condition B never overwrites the
    // position of an unfinished condition A.
    let mut cursors = lock_cursors(cursors);
    if cursor.verified {
        cursors.remove(&condition);
    } else {
        cursors.insert(condition, cursor);
    }
    Ok(fact)
}

/// One local-owner demand: the shared parking, protected-material, hold, and
/// blocking-section envelope every local participant applies. Only the owner's
/// step function differs, so every owner's currentness/cursor contract has one
/// implementation.
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
        // A local owner must receive the protected material. Without it no
        // mechanical sweep exists, so this is an explicit retryable hold,
        // never a fabricated completion.
        return held(ParticipantHoldClass::Failed);
    };
    match run_blocking(move || run_local_demand(&store, &sweep, condition, owner, &target, step))
        .await
    {
        Ok(fact) => fact,
        Err(ErasurePageError::Unrepresentable) => held(ParticipantHoldClass::Failed),
        Err(ErasurePageError::Storage) => held(ParticipantHoldClass::Unavailable),
        // The condition was completed or superseded; the sweep mutates
        // nothing and drops its continuation. `LocalComplete` is what makes
        // the canonical record refuse it as stale, never Verified.
        Err(ErasurePageError::NotCurrent) => ParticipantCompletionFact::local_complete(
            condition,
            owner,
            0,
            0,
            WallClockWithTz::now(),
        ),
    }
}

/// The single local-erasure participant: one bounded erase→verify sweep over
/// the owner's declared page order.
///
/// Every local owner differs only in the step that interprets one page, so
/// the parking, protected-material, cursor, currentness, and completion-fact
/// envelope has one implementation.
pub struct LocalErasureParticipant {
    owner: ParticipantOwnerRef,
    store: Store,
    step: LocalStep,
    /// One continuation per unfinished condition, so concurrent operations do
    /// not restart each other's in-progress sweep from the head.
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

#[cfg(test)]
pub(crate) use task_action_inference::ERASED_LOCATOR;

/// Test-support alias of the system-wide mechanical probe (see
/// [`remainder::system_remainder`]). Tests assert `0` after an erasure pass
/// instead of re-implementing the canonical content-surface list.
#[cfg(any(test, feature = "test-support"))]
pub(crate) use remainder::system_remainder as exact_remainder_probe;
