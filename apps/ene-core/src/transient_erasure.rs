//! Host-process and first-party-Client transient erasure participants
//! (Stage 6 A3c; lifecycle §8.1/§9, IPC §17).
//!
//! Two transient holders live in the Host composition and are neither durable
//! masters nor owned by another semantic owner:
//!
//! - `HostTransientParticipant` owns the Host-process transient payloads:
//!   presentation receipts/refs/cursors/subscriptions, the queued Learning
//!   formation transcripts, and the invalidation fence that stops in-flight
//!   dialogue streams and assembled replies from being published or adopted
//!   after a condition became durable.
//! - `ClientIncarnationParticipant` owns one Client incarnation's local
//!   transient world as far as the Host knows it: the connection-owned
//!   presentation state it was handed (swept by the connection lifecycle) and
//!   the local copy the Client itself holds. The Host never projects a target
//!   body onto the wire: the demand names local data classes and the result is
//!   folded back into a [`ParticipantCompletionFact`], which is local
//!   completion — never the system-wide one.
//!
//! The evidence that a Client incarnation may hold a target-bearing local copy
//! is durable (`client_delivery_evidence` in the canonical store), written
//! body-free before the body leaves the Host. Admission therefore names an
//! incarnation that received material in an earlier Host process; a Host
//! restart drops only the in-flight demand plumbing, never the evidence.
//!
//! Both participants are bounded work: the Host-transient demand drops the
//! affected in-memory entries (never durable rows), the Client demand is one
//! wire message with one bounded wait, and an unreachable, disconnecting, or
//! silent Client is an explicit hold, never a completion. A demand the
//! connection loop has not handed to the wire yet yields bounded more-work
//! instead of stalling the pass behind a reachable Client. Disconnect and
//! timeout prove nothing about the Client's local copy, and only a verified
//! full-class local-erasure result supersedes the durable delivery evidence.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use ene_api::v1::deletion::{
    ClientTempClass, DeletionDemand, DeletionDemandWireId, DeletionTargetWire, LocalErasureResult,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{ConnectionWireId, DeletionOperationWireRef};
use ene_learning::ExperienceCandidate;
use ene_preservation::{
    DeletionMaterialOutcome, DeletionOperationId, DeletionOperationRef, DemandLocalErasureCommand,
    ErasureConditionRef, ErasureParticipant, MechanicalDeletionTarget, ParticipantCompletionFact,
    ParticipantCompletionOutcome, ParticipantHoldClass, ParticipantOwnerRef,
    PreservationRepository as _, PreservationTechnicalError, TargetedDeletionTarget,
};
use ene_primitive::{RawId, WallClockWithTz};
use ene_store::{HOST_TRANSIENT_ARRIVAL_PAGE, Store};
use tokio::sync::Notify;
use uuid::Uuid;

use crate::conn::ConnectionTable;
use crate::presentation::PresentationState;
use crate::serve::HostHandle;

/// How long one Client local-erasure demand waits for its result before the
/// participant holds. A wait bound is not a completion proof: a silent or
/// unreachable Client stays a durable hold and a later pass re-demands the
/// same condition idempotently.
const CLIENT_ERASURE_WAIT: Duration = Duration::from_secs(30);

/// The closed demand target set of the current first-party Client surface.
fn current_client_targets() -> Vec<DeletionTargetWire> {
    vec![
        DeletionTargetWire::WipeClass {
            class: ClientTempClass::PresentationBuffer,
        },
        DeletionTargetWire::WipeClass {
            class: ClientTempClass::InputDraft,
        },
    ]
}

/// Whether one Client report objectively supersedes the delivery evidence:
/// every demanded class is reported wiped with no unverified remainder.
///
/// The completion fact itself accepts a narrower report (a local completion
/// with a remainder keeps the operation unfinished instead), but clearing the
/// durable evidence is deliberately stricter: anything less than the whole
/// closed class set leaves the row, so a later pass still demands the copy.
fn verified_full_class_wipe(result: &LocalErasureResult) -> bool {
    if !result.unverified.is_empty() {
        return false;
    }
    current_client_targets().iter().all(|target| {
        let DeletionTargetWire::WipeClass { class } = target;
        result.wiped.contains(class)
    })
}

/// The protected exact mechanical text of one operation target.
fn exact_text(target: &TargetedDeletionTarget) -> &str {
    let MechanicalDeletionTarget::ExactText(material) = &target.mechanical;
    material.expose_for_erasure()
}

/// Whether one queued formation premise can carry the covered target: either
/// its transcript text contains the exact mechanical target, or one of its
/// correlated source bounds is a covered source of the current sweep.
fn experience_covered(
    experience: &ExperienceCandidate,
    exact: &str,
    covered: &[bool],
    identities: &[RawId],
) -> bool {
    debug_assert_eq!(covered.len(), identities.len());
    // The ordered per-message provenance is the exact read set; the coarse
    // range bounds stay checked for a candidate assembled without it.
    if experience
        .sources
        .iter()
        .chain([experience.source.start, experience.source.end].iter())
        .any(|source| {
            identities
                .iter()
                .zip(covered.iter())
                .any(|(identity, hit)| identity == source && *hit)
        })
    {
        return true;
    }
    if exact.is_empty() {
        return false;
    }
    experience
        .transcript
        .iter()
        .any(|turn| turn.text.contains(exact))
}

fn experience_identities(experience: &ExperienceCandidate) -> Vec<RawId> {
    let mut identities = experience.sources.clone();
    identities.push(experience.source.start);
    identities.push(experience.source.end);
    identities
}

/// Invalidation fence for in-flight Host transient payloads.
///
/// The fence holds no deletion state — no condition, no target, no
/// "no-deletion" judgement — so it is not a second currentness registry. It is
/// an invalidation epoch: a dialogue stream or assembled reply that started
/// before the epoch moved can no longer prove its payload is uncovered, and
/// fails closed (aborts the stream, refuses the adoption) instead of
/// publishing. Any publication that starts after the move re-reads the
/// canonical sources under the normal boundaries.
#[derive(Debug, Default)]
pub(crate) struct TransientErasureFence {
    epoch: AtomicU64,
}

impl TransientErasureFence {
    #[must_use]
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// Invalidates every in-flight transient payload; returns the new epoch.
    pub(crate) fn invalidate(&self) -> u64 {
        self.epoch.fetch_add(1, Ordering::SeqCst) + 1
    }
}

/// Caps the Host-transient Learning-queue scan for one demand (lifecycle §9).
/// Remaining entries continue on a later demand via [`ParticipantCompletionStatus::MoreWork`].
pub(crate) const HOST_TRANSIENT_LEARNING_QUEUE_PAGE: usize = 32;

/// Caps the in-memory Learning formation queue. Overflow drops the oldest
/// pending pass (Learning is best-effort). Overflow is a queue mutation, not
/// a scanned-clean proof: the HostTransient sweep cursor rebases when the
/// generation advances.
pub(crate) const LEARNING_FORMATION_QUEUE_CAP: usize = 256;

/// Process-local continuation of one HostTransient Learning-queue sweep.
///
/// This is not a canonical deletion registry. Restart loses it and the next
/// demand starts a new cycle from the live pending queue. `remaining` is the
/// number of pending entries still owed in the current stable generation,
/// never `queue.len() > PAGE`.
#[derive(Debug, Clone, Copy)]
struct HostTransientLearningSweep {
    condition: ErasureConditionRef,
    queue_generation: u64,
    remaining: usize,
}

/// In-memory Learning formation work: the pending queue plus at most one
/// worker-owned candidate that has left the queue but does not yet have a
/// canonical formation identity.
///
/// `mutation_generation` advances on every worker or producer ownership
/// change (enqueue, overflow drop, pending→taken, taken clear). HostTransient
/// may apply a snapshotted page only while this generation still matches, so
/// a `pending → taken` race during the membership await cannot verify from
/// the stale page. HostTransient's own covered drop / uncovered rotate does
/// not advance the generation: that is the confirmed apply of the snapshot.
///
/// HostTransient must not report Verified while `taken` still carries a
/// covered transcript. The worker clears `taken` only after
/// `begin_learning_formation` commits, so the pop→claim window cannot lose
/// deletion provenance. The pending queue itself stays the HostTransient
/// drop surface; `taken` is counted as remainder, never dropped here.
#[derive(Debug, Default)]
pub(crate) struct LearningFormationQueue {
    pending: VecDeque<ExperienceCandidate>,
    taken: Option<ExperienceCandidate>,
    mutation_generation: u64,
}

impl LearningFormationQueue {
    fn bump(&mut self) {
        self.mutation_generation = self.mutation_generation.wrapping_add(1);
    }

    /// Enqueues one candidate, dropping the oldest pending entry when the
    /// queue is already at [`LEARNING_FORMATION_QUEUE_CAP`].
    pub(crate) fn push_back(&mut self, experience: ExperienceCandidate) {
        self.bump();
        while self.pending.len() >= LEARNING_FORMATION_QUEUE_CAP {
            self.pending.pop_front();
        }
        self.pending.push_back(experience);
    }

    pub(crate) fn len(&self) -> usize {
        self.pending.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub(crate) fn mutation_generation(&self) -> u64 {
        self.mutation_generation
    }

    pub(crate) fn iter(&self) -> std::collections::vec_deque::Iter<'_, ExperienceCandidate> {
        self.pending.iter()
    }

    /// Moves the next pending candidate into the worker-owned slot. The
    /// returned clone is the pass body; HostTransient still sees `taken`.
    pub(crate) fn take_pending(&mut self) -> Option<ExperienceCandidate> {
        let next = self.pending.pop_front()?;
        self.taken = Some(next.clone());
        self.bump();
        Some(next)
    }

    pub(crate) fn clear_taken(&mut self) {
        if self.taken.take().is_some() {
            self.bump();
        }
    }

    pub(crate) fn taken(&self) -> Option<&ExperienceCandidate> {
        self.taken.as_ref()
    }

    /// Applies one already-examined pending page: covered premises drop,
    /// uncovered ones rotate to the back. The caller must have confirmed that
    /// [`Self::mutation_generation`] still matches the snapshot that produced
    /// `identities` / `covered`. This does not bump the generation.
    fn apply_examined_page(
        &mut self,
        page_len: usize,
        exact: &str,
        identities: &[RawId],
        covered: &[bool],
    ) -> u64 {
        let mut dropped = 0u64;
        for _ in 0..page_len {
            let Some(experience) = self.pending.pop_front() else {
                break;
            };
            if experience_covered(&experience, exact, covered, identities) {
                dropped += 1;
            } else {
                self.pending.push_back(experience);
            }
        }
        dropped
    }
}

/// Process-local ordering for HostTransient Learning arrivals.
///
/// Canonical deletion authority stays in Store. This gate linearizes
/// process-memory occupancy and queue mutation against HostTransient's
/// Verified-commit and the composition's finalizing attempt. Unpublished
/// bookkeeping records that a body-bearing arrival was accepted and the
/// canonical delayed-arrival has not yet succeeded: it is not a deletion
/// registry. The `std` queue mutex is never held across an await; this tokio
/// mutex may be held across a short Store commit.
#[derive(Debug)]
pub(crate) struct HostTransientArrival {
    gate: tokio::sync::Mutex<()>,
    inflight_pins: AtomicU64,
    verified_generation: AtomicU64,
    publish: std::sync::Mutex<ArrivalPublishState>,
    last_classified: AtomicUsize,
    last_direct_classified: AtomicUsize,
}

/// Fail-closed execution state for one incomplete canonical arrival publish.
#[derive(Debug, Default)]
struct ArrivalPublishState {
    /// The bounded global unfinished-ops walk has not finished a complete
    /// page-chain for the live remainder. This is not "every unfinished
    /// deletion may be related": relatedness is `owed` plus a direct
    /// per-operation classification at Finalizing.
    scan_incomplete: bool,
    /// Operations known to be related whose canonical publish has not
    /// succeeded. Blocks only those operations' finalizing.
    owed: HashSet<DeletionOperationId>,
    after: Option<DeletionOperationId>,
}

impl ArrivalPublishState {
    fn is_clean(&self) -> bool {
        !self.scan_incomplete && self.owed.is_empty()
    }
}

impl Default for HostTransientArrival {
    fn default() -> Self {
        Self {
            gate: tokio::sync::Mutex::new(()),
            inflight_pins: AtomicU64::new(0),
            verified_generation: AtomicU64::new(0),
            publish: std::sync::Mutex::new(ArrivalPublishState::default()),
            last_classified: AtomicUsize::new(0),
            last_direct_classified: AtomicUsize::new(0),
        }
    }
}

impl HostTransientArrival {
    pub(crate) async fn lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.gate.lock().await
    }

    /// Occupies HostTransient remainder for one Learning pin.
    ///
    /// The arrival gate is held only across the counter increment so pin
    /// start linearizes against Finalizing. The guard then releases the
    /// gate; Drop decrements the counter with no await and no Store I/O.
    /// Cancellation, connection drop, and normal return all run Drop.
    pub(crate) async fn acquire_pin(self: &Arc<Self>) -> LearningPinGuard {
        let _gate = self.lock().await;
        self.begin_pin();
        LearningPinGuard {
            arrival: Arc::clone(self),
        }
    }

    fn begin_pin(&self) {
        self.inflight_pins.fetch_add(1, Ordering::SeqCst);
    }

    fn end_pin(&self) {
        self.inflight_pins.fetch_sub(1, Ordering::SeqCst);
    }

    pub(crate) fn inflight_pins(&self) -> u64 {
        self.inflight_pins.load(Ordering::SeqCst)
    }

    pub(crate) fn set_verified_generation(&self, generation: u64) {
        self.verified_generation.store(generation, Ordering::SeqCst);
    }

    pub(crate) fn verified_generation(&self) -> u64 {
        self.verified_generation.load(Ordering::SeqCst)
    }

    /// A body-bearing remainder was accepted into process memory. Canonical
    /// delayed-arrival publication is owed until a later bounded walk proves
    /// it succeeded; this is not a deletion registry.
    pub(crate) fn note_queued_arrival(&self) {
        let mut state = crate::lock_unpoison(&self.publish);
        state.after = None;
        state.scan_incomplete = true;
    }

    /// Whether any TARGET-bearing arrival still owes canonical publication.
    pub(crate) fn has_unpublished(&self) -> bool {
        !crate::lock_unpoison(&self.publish).is_clean()
    }

    fn owes(&self, operation: DeletionOperationId) -> bool {
        crate::lock_unpoison(&self.publish)
            .owed
            .contains(&operation)
    }

    fn mark_owed(&self, operation: DeletionOperationId) {
        let mut state = crate::lock_unpoison(&self.publish);
        state.owed.insert(operation);
        state.scan_incomplete = true;
    }

    #[cfg(test)]
    #[expect(dead_code, reason = "test observation probe")]
    pub(crate) fn last_classified(&self) -> usize {
        self.last_classified.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    #[expect(dead_code, reason = "test observation probe")]
    pub(crate) fn last_direct_classified(&self) -> usize {
        self.last_direct_classified.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    #[expect(dead_code, reason = "test observation probe")]
    pub(crate) fn scan_incomplete(&self) -> bool {
        crate::lock_unpoison(&self.publish).scan_incomplete
    }
}

/// Process-local Learning-pin occupancy. Drop releases `inflight_pins`
/// even when the owning future is cancelled. It never touches unpublished
/// arrival bookkeeping: occupancy and delayed-arrival publication are
/// different remainders.
///
/// Cancellation windows after `acquire_pin`:
/// - before `pin_experience` returns: no queue entry, no unpublished state
/// - after a local candidate exists, before queue push: the body dies with
///   the future; unpublished is not owed
/// - after queue push, before canonical publish: `scan_incomplete` / `owed`
///   stay; Finalizing cannot complete; a later drive retries publication
/// - after canonical publication succeeds: durable next-sweep / invalidation
///   is Store authority; Drop only clears occupancy
#[must_use = "Learning pin occupancy is released when this guard is dropped"]
pub(crate) struct LearningPinGuard {
    arrival: Arc<HostTransientArrival>,
}

impl Drop for LearningPinGuard {
    fn drop(&mut self) {
        self.arrival.end_pin();
    }
}

fn snapshot_learning_remainder(
    queue: &std::sync::Mutex<LearningFormationQueue>,
) -> (Vec<ExperienceCandidate>, u64) {
    let live = crate::lock_unpoison(queue);
    let mut items: Vec<ExperienceCandidate> = live.iter().cloned().collect();
    if let Some(taken) = live.taken() {
        items.push(taken.clone());
    }
    (items, live.mutation_generation())
}

async fn experience_covers_operation(
    store: &Store,
    current: DeletionOperationRef,
    experiences: &[ExperienceCandidate],
) -> Result<bool, PreservationTechnicalError> {
    #[cfg(any(test, feature = "test-support"))]
    if store.host_transient_arrival_classify_fails_for_tests(current.operation) {
        return Err(PreservationTechnicalError::StorageUnavailable);
    }
    let material = match store.deletion_operation_material(current.operation).await? {
        DeletionMaterialOutcome::Material(material) => material,
        DeletionMaterialOutcome::Missing | DeletionMaterialOutcome::Destroyed => return Ok(false),
    };
    let exact = exact_text(material.target()).to_owned();
    for experience in experiences {
        let identities = experience_identities(experience);
        let covered = store
            .erasure_sources_covered(current.condition(), identities.clone())
            .await?;
        if experience_covered(experience, &exact, &covered, &identities) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Publishes a related arrival for one operation. Success drops that
/// identity from `owed`; failure marks it owed so a later drive retries.
/// The global walk cursor is left unchanged: this is not a page of
/// unfinished operations.
async fn publish_current_related_arrival(
    store: &Store,
    arrival: &HostTransientArrival,
    operation: DeletionOperationId,
) {
    match store
        .note_host_transient_learning_arrival(vec![operation])
        .await
    {
        Ok(_) => {
            crate::lock_unpoison(&arrival.publish)
                .owed
                .remove(&operation);
        }
        Err(_) => arrival.mark_owed(operation),
    }
}

/// One bounded page of operation-specific delayed-arrival publication.
///
/// The caller holds the arrival gate. A clean publish state is a no-op:
/// leftover unrelated or already-published queue entries do not restart
/// classification. A technical failure or a truncated unfinished-ops page
/// leaves unpublished bookkeeping set so finalizing cannot treat the
/// remainder as clean.
pub(crate) async fn publish_owed_learning_arrivals(
    store: &Store,
    arrival: &HostTransientArrival,
    queue: &std::sync::Mutex<LearningFormationQueue>,
) {
    if crate::lock_unpoison(&arrival.publish).is_clean() {
        arrival.last_classified.store(0, Ordering::SeqCst);
        return;
    }
    arrival.last_classified.store(0, Ordering::SeqCst);
    let after = crate::lock_unpoison(&arrival.publish).after;
    let page = match store
        .unfinished_deletions(after, HOST_TRANSIENT_ARRIVAL_PAGE)
        .await
    {
        Ok(page) => page,
        Err(_) => {
            crate::lock_unpoison(&arrival.publish).scan_incomplete = true;
            return;
        }
    };
    let (experiences, _) = snapshot_learning_remainder(queue);
    if experiences.is_empty() {
        *crate::lock_unpoison(&arrival.publish) = ArrivalPublishState::default();
        return;
    }
    let page_len = page.len();
    let last = page.last().map(|record| record.current.operation);
    for record in page {
        arrival.last_classified.fetch_add(1, Ordering::SeqCst);
        match experience_covers_operation(store, record.current, &experiences).await {
            Ok(true) => {
                match store
                    .note_host_transient_learning_arrival(vec![record.current.operation])
                    .await
                {
                    Ok(_) => {
                        crate::lock_unpoison(&arrival.publish)
                            .owed
                            .remove(&record.current.operation);
                    }
                    Err(_) => {
                        arrival.mark_owed(record.current.operation);
                        return;
                    }
                }
            }
            Ok(false) => {
                crate::lock_unpoison(&arrival.publish)
                    .owed
                    .remove(&record.current.operation);
            }
            Err(_) => {
                crate::lock_unpoison(&arrival.publish).scan_incomplete = true;
                return;
            }
        }
    }
    let mut state = crate::lock_unpoison(&arrival.publish);
    if page_len < HOST_TRANSIENT_ARRIVAL_PAGE as usize {
        state.after = None;
        state.scan_incomplete = !state.owed.is_empty();
    } else {
        state.scan_incomplete = true;
        state.after = last;
    }
}

/// Whether this operation's sealed completion must wait: a body-bearing pin,
/// a known unpublished related arrival, or a live remainder that this
/// operation cannot be proven unrelated to.
///
/// `scan_incomplete` means only that the bounded global walk has not
/// finished. It does not block every unfinished deletion. Finalizing
/// classifies *this* operation against the live remainder and, when related,
/// publishes the canonical delayed-arrival for that identity alone.
/// Unrelated enqueue may bump `mutation_generation` without blocking this
/// operation. A technical classification failure fail-closes only `current`.
pub(crate) async fn unpublished_blocks_finalizing(
    store: &Store,
    arrival: &HostTransientArrival,
    queue: &std::sync::Mutex<LearningFormationQueue>,
    current: DeletionOperationRef,
) -> bool {
    if arrival.inflight_pins() > 0 {
        return true;
    }
    if arrival.owes(current.operation) {
        publish_current_related_arrival(store, arrival, current.operation).await;
        return true;
    }
    let scan_incomplete = crate::lock_unpoison(&arrival.publish).scan_incomplete;
    let (experiences, live_generation) = snapshot_learning_remainder(queue);
    if experiences.is_empty() {
        return false;
    }
    if !scan_incomplete && live_generation == arrival.verified_generation() {
        return false;
    }
    arrival
        .last_direct_classified
        .fetch_add(1, Ordering::SeqCst);
    match experience_covers_operation(store, current, &experiences).await {
        Ok(true) => {
            publish_current_related_arrival(store, arrival, current.operation).await;
            true
        }
        Ok(false) => false,
        Err(_) => true,
    }
}

/// Host-process transient erasure participant (lifecycle §8, SO §4.17).
pub(crate) struct HostTransientParticipant {
    store: Store,
    fence: Arc<TransientErasureFence>,
    presentations: Arc<std::sync::Mutex<PresentationState>>,
    learning_queue: Arc<std::sync::Mutex<LearningFormationQueue>>,
    arrival: Arc<HostTransientArrival>,
    /// Serializes HostTransient demands. The queue `std` mutex is never held
    /// across an await; this tokio lock only prevents two demands from
    /// overlapping snapshot/apply on the same process-local cursor.
    demand_lock: tokio::sync::Mutex<()>,
    sweep: std::sync::Mutex<Option<HostTransientLearningSweep>>,
}

impl HostTransientParticipant {
    #[must_use]
    pub(crate) fn new(
        store: Store,
        fence: Arc<TransientErasureFence>,
        presentations: Arc<std::sync::Mutex<PresentationState>>,
        learning_queue: Arc<std::sync::Mutex<LearningFormationQueue>>,
        arrival: Arc<HostTransientArrival>,
    ) -> Self {
        Self {
            store,
            fence,
            presentations,
            learning_queue,
            arrival,
            demand_lock: tokio::sync::Mutex::new(()),
            sweep: std::sync::Mutex::new(None),
        }
    }

    pub(crate) async fn lock_arrival(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.arrival.lock().await
    }

    pub(crate) async fn publish_owed_arrivals(&self) {
        let _gate = self.arrival.lock().await;
        publish_owed_learning_arrivals(&self.store, &self.arrival, &self.learning_queue).await;
    }

    pub(crate) async fn publish_owed_arrivals_locked(&self) {
        publish_owed_learning_arrivals(&self.store, &self.arrival, &self.learning_queue).await;
    }

    pub(crate) async fn unpublished_blocks_finalizing(
        &self,
        current: DeletionOperationRef,
    ) -> bool {
        unpublished_blocks_finalizing(&self.store, &self.arrival, &self.learning_queue, current)
            .await
    }

    /// Records a HostTransient Verified fact only while the examined queue
    /// generation is still live, no body-bearing pin is in flight, and no
    /// unpublished TARGET-bearing arrival is owed.
    ///
    /// The park sits *before* the arrival gate so a test can enqueue G+1
    /// while the in-memory fact exists and the durable row is still
    /// `running`. The gate then serializes that enqueue against this commit.
    pub(crate) async fn commit_verified(
        &self,
        fact: ParticipantCompletionFact,
    ) -> Result<ParticipantCompletionOutcome, PreservationTechnicalError> {
        #[cfg(any(test, feature = "test-support"))]
        self.store
            .pause_host_transient_verified_record_if_armed_for_tests()
            .await;
        let _gate = self.arrival.lock().await;
        let inflight = self.arrival.inflight_pins();
        let unpublished = self.arrival.has_unpublished();
        let (live_generation, remainder) = {
            let live = crate::lock_unpoison(&self.learning_queue);
            (
                live.mutation_generation(),
                live.len() as u64
                    + u64::from(live.taken().is_some())
                    + inflight
                    + u64::from(unpublished),
            )
        };
        if inflight > 0 || unpublished || live_generation != self.arrival.verified_generation() {
            return self
                .store
                .record_participant_completion(ParticipantCompletionFact::more_work(
                    fact.condition(),
                    ParticipantOwnerRef::HostTransient,
                    fact.erased_count(),
                    remainder.max(1),
                    WallClockWithTz::now(),
                ))
                .await;
        }
        self.store.record_participant_completion(fact).await
    }

    fn rebase_sweep(&self, condition: ErasureConditionRef, generation: u64, remaining: usize) {
        *crate::lock_unpoison(&self.sweep) = Some(HostTransientLearningSweep {
            condition,
            queue_generation: generation,
            remaining,
        });
    }
}

impl ErasureParticipant for HostTransientParticipant {
    fn owner(&self) -> ParticipantOwnerRef {
        ParticipantOwnerRef::HostTransient
    }

    fn demand_local_erasure(
        &self,
        command: DemandLocalErasureCommand,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ParticipantCompletionFact> + Send + '_>>
    {
        Box::pin(async move {
            // Process-memory mutation cannot share the Immediate writer with
            // the canonical row. The same currentness predicate durable
            // participants re-check inside their erase transaction is read
            // here immediately before any drop: a completed or superseded
            // condition must not discard a fresh post-closure premise.
            // Unreadable currentness fails closed (no mutation, not Verified).
            #[cfg(any(test, feature = "test-support"))]
            self.store.pause_erasure_mutation_if_armed_for_tests().await;
            let current = self
                .store
                .erasure_condition_is_current(command.condition())
                .await
                .unwrap_or(false);
            if !current {
                return ParticipantCompletionFact::local_complete(
                    command.condition(),
                    ParticipantOwnerRef::HostTransient,
                    0,
                    0,
                    WallClockWithTz::now(),
                );
            }
            let _demand = self.demand_lock.lock().await;
            let exact = command
                .scope()
                .target()
                .map(exact_text)
                .unwrap_or_default()
                .to_owned();
            let (identities, page_len, generation) = {
                let queue = crate::lock_unpoison(&self.learning_queue);
                let generation = queue.mutation_generation();
                let mut sweep = crate::lock_unpoison(&self.sweep);
                let restart = !matches!(
                    *sweep,
                    Some(HostTransientLearningSweep {
                        condition,
                        queue_generation,
                        ..
                    }) if condition == command.condition() && queue_generation == generation
                );
                if restart {
                    *sweep = Some(HostTransientLearningSweep {
                        condition: command.condition(),
                        queue_generation: generation,
                        remaining: queue.len(),
                    });
                }
                let remaining = sweep.as_ref().map_or(0, |item| item.remaining);
                let page_len = remaining.min(HOST_TRANSIENT_LEARNING_QUEUE_PAGE);
                let mut identities = queue
                    .iter()
                    .take(page_len)
                    .flat_map(experience_identities)
                    .collect::<Vec<_>>();
                if let Some(taken) = queue.taken() {
                    identities.extend(experience_identities(taken));
                }
                (identities, page_len, generation)
            };
            // Snapshot is complete. The queue std mutex is not held across
            // this await; a worker `take_pending` or producer enqueue bumps
            // generation so the later apply refuses the stale page.
            #[cfg(any(test, feature = "test-support"))]
            self.store
                .pause_host_transient_queue_if_armed_for_tests()
                .await;
            let covered = match self
                .store
                .erasure_sources_covered(command.condition(), identities.clone())
                .await
            {
                Ok(flags) => flags,
                Err(_) => {
                    return ParticipantCompletionFact::local_complete(
                        command.condition(),
                        ParticipantOwnerRef::HostTransient,
                        0,
                        0,
                        WallClockWithTz::now(),
                    );
                }
            };
            // Presentation receipts, carried refs, cursors, subscriptions, and
            // resume slots are all reconstructible from canonical rows; none is
            // provably body-free, so the whole per-connection world is
            // invalidated. Future presentation re-reads the canonical source.
            let dropped_presentation =
                crate::lock_unpoison(&self.presentations).invalidate_for_erasure();
            let (dropped_learning, unfinished, remainder) = {
                let mut queue = crate::lock_unpoison(&self.learning_queue);
                if queue.mutation_generation() != generation {
                    // Worker/producer mutated the queue after the snapshot.
                    // Discard the page; never drop from it and never Verified.
                    self.rebase_sweep(
                        command.condition(),
                        queue.mutation_generation(),
                        queue.len(),
                    );
                    let remainder = queue.len() as u64 + u64::from(queue.taken().is_some());
                    (0, true, remainder.max(1))
                } else {
                    let dropped =
                        queue.apply_examined_page(page_len, &exact, &identities, &covered);
                    let remaining = {
                        let mut sweep = crate::lock_unpoison(&self.sweep);
                        if let Some(item) = sweep.as_mut() {
                            item.remaining = item.remaining.saturating_sub(page_len);
                        }
                        sweep.as_ref().map_or(0, |item| item.remaining)
                    };
                    // `taken` is inspected from the live slot under the still-
                    // matching generation. HostTransient never drops it.
                    let taken_covered = queue.taken().is_some_and(|experience| {
                        experience_covered(experience, &exact, &covered, &identities)
                    });
                    let unfinished = taken_covered || remaining > 0;
                    let remainder = queue.len() as u64 + u64::from(queue.taken().is_some());
                    (dropped, unfinished, remainder.max(1))
                }
            };
            // In-flight streams and assembled replies fail closed from here on;
            // nothing published before the fence is treated as proof of
            // completion (a durable reply is the History owner's to erase).
            self.fence.invalidate();
            // Process memory cannot roll back. A second currentness read
            // after the drop refuses Verified when the operation closed in
            // the window: the durable record will not treat a stale demand as
            // completion, and a later pass of a still-current sweep re-demands.
            let still_current = self
                .store
                .erasure_condition_is_current(command.condition())
                .await
                .unwrap_or(false);
            if !still_current {
                return ParticipantCompletionFact::local_complete(
                    command.condition(),
                    ParticipantOwnerRef::HostTransient,
                    0,
                    0,
                    WallClockWithTz::now(),
                );
            }
            if unfinished {
                return ParticipantCompletionFact::more_work(
                    command.condition(),
                    ParticipantOwnerRef::HostTransient,
                    dropped_presentation + dropped_learning,
                    remainder,
                    WallClockWithTz::now(),
                );
            }
            // The currentness re-check awaits. A producer enqueue or worker
            // take in that window must not complete from the older snapshot:
            // Verified is allowed only while the examined generation is still
            // the live queue generation. A body-bearing pin that has not
            // reached the queue yet is the same remainder.
            let inflight = self.arrival.inflight_pins();
            {
                let queue = crate::lock_unpoison(&self.learning_queue);
                if queue.mutation_generation() != generation || inflight > 0 {
                    if queue.mutation_generation() != generation {
                        self.rebase_sweep(
                            command.condition(),
                            queue.mutation_generation(),
                            queue.len(),
                        );
                    }
                    let remainder =
                        queue.len() as u64 + u64::from(queue.taken().is_some()) + inflight;
                    return ParticipantCompletionFact::more_work(
                        command.condition(),
                        ParticipantOwnerRef::HostTransient,
                        dropped_presentation + dropped_learning,
                        remainder.max(1),
                        WallClockWithTz::now(),
                    );
                }
            }
            self.arrival.set_verified_generation(generation);
            ParticipantCompletionFact::verified(
                command.condition(),
                ParticipantOwnerRef::HostTransient,
                dropped_presentation + dropped_learning,
                WallClockWithTz::now(),
            )
        })
    }
}

/// One outstanding Host → Client local-erasure demand.
struct PendingDemand {
    id: String,
    condition: ErasureConditionRef,
    /// The connection the demand was (or will be) delivered on. A demand is
    /// never re-addressed to another incarnation's connection.
    connection: ConnectionWireId,
    /// The connection that already received this demand, if any. Delivery is
    /// once per live connection; a connection end abandons the demand, so a
    /// later pass re-demands the same condition under a fresh demand id.
    delivered_to: Option<ConnectionWireId>,
    /// Durable delivery-evidence sequence observed when this demand went on
    /// the wire (`None` when no evidence row existed then). The verified
    /// answer may clear the evidence only while the row still carries this
    /// exact sequence: a delivery that raced the wipe advances it and the row
    /// survives.
    evidence_seq: Option<u64>,
    state: PendingState,
}

enum PendingState {
    Awaiting,
    Answered(LocalErasureResult),
}

/// One accepted Client result and the durable evidence premise it carries.
struct AcceptedClientErasure {
    /// The delivery-evidence sequence observed when the demand was handed to
    /// the wire. `None` means no uncleared evidence existed at delivery time,
    /// so a verified answer has nothing to clear.
    evidence_seq: Option<u64>,
}

/// What one bounded wait observed. Every non-answer outcome is a hold.
enum ClientErasureWait {
    Answered(LocalErasureResult),
    /// The connection ended (closed or superseded), the wait bound elapsed, or
    /// the pending demand was replaced. No proof of local erasure.
    Abandoned,
}

/// In-flight local-erasure demand plumbing for Client incarnations that may
/// hold a target-bearing local copy (lifecycle §8.1, IPC §17).
///
/// The admission-time evidence itself is durable and lives in the canonical
/// store (`client_delivery_evidence`); this registry holds only the demand
/// bookkeeping, so a Host restart drops the pending waiters without touching
/// the evidence. Tracking evidence is delivery, not connection: a row exists
/// only after the Host actually handed body-bearing material to an
/// authenticated incarnation (presentation excerpts, history items, Task
/// report source bodies, or text stream deltas). A Client that only sent
/// requests has no copy the Host could erase, and is not claimed as a
/// required participant.
pub(crate) struct ClientTransientRegistry {
    inner: std::sync::Mutex<ClientTransientInner>,
    /// Wakes connection loops to deliver a pending demand.
    delivery_wake: Notify,
    /// Wakes a participant demand waiting for its result.
    result_wake: Notify,
    /// The connection table, installed by the serving composition. Before the
    /// installation (a transport-free handle) no Client is reachable and every
    /// demand holds as unavailable.
    table: OnceLock<Arc<ConnectionTable>>,
    /// Canonical currentness authority for the pre-wire check. The registry
    /// is not a second deletion store; it only refuses to mint a demand when
    /// the operation is already closed.
    store: Store,
    /// Silence bound override. `None` is the production 30s hold wait.
    wait_limit: std::sync::Mutex<Option<Duration>>,
}

#[derive(Default)]
struct ClientTransientInner {
    /// At most one outstanding demand per incarnation.
    pending: HashMap<RawId, PendingDemand>,
}

impl ClientTransientRegistry {
    #[must_use]
    pub(crate) fn new(store: Store) -> Self {
        Self {
            inner: std::sync::Mutex::new(ClientTransientInner::default()),
            delivery_wake: Notify::new(),
            result_wake: Notify::new(),
            table: OnceLock::new(),
            store,
            wait_limit: std::sync::Mutex::new(None),
        }
    }

    pub(crate) fn install_connection_table(&self, table: Arc<ConnectionTable>) {
        // One serving composition owns one table; a second install is ignored
        // rather than replacing the reachability authority.
        if self.table.set(table).is_err() {
            // A previously installed table stays authoritative.
        }
    }

    /// Host-minted participant identity of one Client boot incarnation.
    #[must_use]
    fn identity_for(counter: u64, random: u64) -> RawId {
        RawId::from_uuid(Uuid::from_u64_pair(counter, random))
    }

    /// The current authenticated connection of one tracked incarnation.
    ///
    /// The boot incarnation is recovered from the identity itself, so a
    /// durable participant snapshot resolves after a Host restart even though
    /// the in-flight demand plumbing did not survive it (`transient_erasure`
    /// module docs: identity is Host-minted deterministically from the boot
    /// incarnation). A missing connection is an explicit unreachable hold.
    fn current_connection(&self, identity: RawId) -> Option<ConnectionWireId> {
        let (counter, random) = identity.as_uuid().as_u64_pair();
        let table = self.table.get()?;
        table.current_connection_for_incarnation(counter, random)
    }

    #[must_use]
    pub(crate) fn demand_wakeup(&self) -> &Notify {
        &self.delivery_wake
    }

    /// Begins one bounded demand for an incarnation.
    ///
    /// A demand for the same `(condition, connection)` reuses the outstanding
    /// one instead of minting a new id: a bounded pass that yielded before the
    /// Client answered must still match the Client's answer, and a Client
    /// answer is never orphaned by a later retry of the same condition. Any
    /// older demand with a different condition (or a different connection) is
    /// replaced, and its waiter observes [`ClientErasureWait::Abandoned`]
    /// instead of adopting a foreign answer.
    fn begin(
        &self,
        identity: RawId,
        condition: ErasureConditionRef,
        connection: ConnectionWireId,
    ) -> String {
        let mut inner = crate::lock_unpoison(&self.inner);
        let id = match inner.pending.get(&identity) {
            Some(existing)
                if existing.condition == condition && existing.connection == connection =>
            {
                existing.id.clone()
            }
            _ => {
                let id = Uuid::new_v4().as_hyphenated().to_string();
                inner.pending.insert(
                    identity,
                    PendingDemand {
                        id: id.clone(),
                        condition,
                        connection,
                        delivered_to: None,
                        evidence_seq: None,
                        state: PendingState::Awaiting,
                    },
                );
                id
            }
        };
        drop(inner);
        // Every parked connection loop re-checks, and the stored permit keeps
        // a wakeup that arrived before a loop parked from being lost.
        self.delivery_wake.notify_waiters();
        self.delivery_wake.notify_one();
        id
    }

    /// Whether the outstanding demand for this `(incarnation, condition)` was
    /// already handed to this connection's wire.
    ///
    /// An undelivered demand is bounded-work yield material, never a hold: the
    /// connection loop may simply be inside another frame, and a later pass
    /// re-observes the same pending.
    fn delivered(
        &self,
        identity: RawId,
        condition: ErasureConditionRef,
        connection: ConnectionWireId,
    ) -> bool {
        let inner = crate::lock_unpoison(&self.inner);
        inner.pending.get(&identity).is_some_and(|pending| {
            pending.condition == condition && pending.delivered_to == Some(connection)
        })
    }

    /// The demand this connection may carry now, marked delivered.
    ///
    /// `evidence_seq` is the durable delivery-evidence sequence the caller
    /// read immediately before this handoff; storing it here is what makes the
    /// later verified-answer clear a compare-and-delete against exactly the
    /// evidence the wipe could cover.
    fn take_deliverable(
        &self,
        connection: ConnectionWireId,
        counter: u64,
        random: u64,
        evidence_seq: Option<u64>,
    ) -> Option<DeletionDemand> {
        let identity = Self::identity_for(counter, random);
        let mut inner = crate::lock_unpoison(&self.inner);
        let pending = inner.pending.get_mut(&identity)?;
        if pending.connection != connection {
            return None;
        }
        if pending.delivered_to == Some(connection) {
            return None;
        }
        if matches!(pending.state, PendingState::Answered(_)) {
            return None;
        }
        pending.delivered_to = Some(connection);
        pending.evidence_seq = evidence_seq;
        Some(DeletionDemand {
            demand: DeletionDemandWireId(pending.id.clone()),
            operation: DeletionOperationWireRef(
                pending
                    .condition
                    .operation
                    .as_raw()
                    .as_uuid()
                    .as_hyphenated()
                    .to_string(),
            ),
            sweep: pending.condition.sweep.as_u64(),
            targets: current_client_targets(),
        })
    }

    /// Records one Client result. Returns the accepted evidence premise when
    /// it answered the outstanding demand: a foreign demand id, an
    /// operation/sweep mismatch, or an incarnation that is not the demanded
    /// one is refused without touching the pending state.
    fn accept_result(
        &self,
        connection: ConnectionWireId,
        counter: u64,
        random: u64,
        result: LocalErasureResult,
    ) -> Option<AcceptedClientErasure> {
        let identity = Self::identity_for(counter, random);
        let mut inner = crate::lock_unpoison(&self.inner);
        let pending = inner.pending.get_mut(&identity)?;
        if pending.connection != connection || pending.id != result.demand.0 {
            return None;
        }
        let demanded_operation = pending
            .condition
            .operation
            .as_raw()
            .as_uuid()
            .as_hyphenated()
            .to_string();
        if result.operation.0 != demanded_operation
            || result.sweep != pending.condition.sweep.as_u64()
        {
            return None;
        }
        if matches!(pending.state, PendingState::Answered(_)) {
            return None;
        }
        let accepted = AcceptedClientErasure {
            evidence_seq: pending.evidence_seq,
        };
        pending.state = PendingState::Answered(result);
        drop(inner);
        self.result_wake.notify_waiters();
        self.result_wake.notify_one();
        Some(accepted)
    }

    /// Ends one connection lifetime: any demand addressed to it is abandoned,
    /// so its waiter reports a hold instead of waiting forever. A later pass
    /// may re-demand the same condition when the incarnation is reachable
    /// again.
    pub(crate) fn note_connection_ended(&self, connection: &ConnectionWireId) {
        let mut inner = crate::lock_unpoison(&self.inner);
        let mut abandoned = false;
        inner.pending.retain(|_, pending| {
            if pending.connection == *connection {
                abandoned = true;
                false
            } else {
                true
            }
        });
        drop(inner);
        if abandoned {
            self.result_wake.notify_waiters();
            self.result_wake.notify_one();
        }
    }

    /// Waits for the result of one bounded demand for `(identity, condition)`.
    ///
    /// The adopted answer is removed with the pending demand, so it is
    /// consumed exactly once: a later pass of the same sweep demands the
    /// Client again (a new demand id) instead of replaying an old answer,
    /// which matters for an unverified remainder that must be re-demanded.
    async fn wait(&self, identity: RawId, condition: ErasureConditionRef) -> ClientErasureWait {
        loop {
            let ready = {
                let mut inner = crate::lock_unpoison(&self.inner);
                match inner.pending.get(&identity) {
                    Some(pending) if pending.condition == condition => match &pending.state {
                        PendingState::Answered(_) => inner.pending.remove(&identity),
                        PendingState::Awaiting => None,
                    },
                    // No pending demand for this condition: it was abandoned
                    // (connection ended) or replaced by a later condition.
                    Some(_) | None => return ClientErasureWait::Abandoned,
                }
            };
            if let Some(PendingDemand {
                state: PendingState::Answered(result),
                ..
            }) = ready
            {
                return ClientErasureWait::Answered(result);
            }
            self.result_wake.notified().await;
        }
    }

    fn wait_limit(&self) -> Duration {
        crate::lock_unpoison(&self.wait_limit).unwrap_or(CLIENT_ERASURE_WAIT)
    }

    pub(crate) fn set_wait_limit_for_test(&self, limit: Duration) {
        *crate::lock_unpoison(&self.wait_limit) = Some(limit);
    }
}

/// One Client incarnation's local-erasure participant (lifecycle §8.1).
///
/// The identity is the Host-minted projection of one Client boot; a
/// replacement incarnation is a different owner with its own registration and
/// never inherits the tracked state of the one it replaced.
pub(crate) struct ClientIncarnationParticipant {
    identity: RawId,
    registry: Arc<ClientTransientRegistry>,
}

impl ClientIncarnationParticipant {
    #[must_use]
    pub(crate) fn new(identity: RawId, registry: Arc<ClientTransientRegistry>) -> Self {
        Self { identity, registry }
    }

    fn completion(
        condition: ErasureConditionRef,
        owner: ParticipantOwnerRef,
        result: &LocalErasureResult,
    ) -> ParticipantCompletionFact {
        let wiped = result.wiped.len() as u64;
        if result.unverified.is_empty() {
            ParticipantCompletionFact::verified(condition, owner, wiped, WallClockWithTz::now())
        } else {
            // A reported unverified local class keeps the bounded pass at local
            // completion with a remainder: the client itself could not prove
            // the copy is gone, and the Host never upgrades that to verified.
            ParticipantCompletionFact::local_complete(
                condition,
                owner,
                wiped,
                result.unverified.len() as u64,
                WallClockWithTz::now(),
            )
        }
    }

    async fn await_client_result(
        registry: &ClientTransientRegistry,
        identity: RawId,
        condition: ErasureConditionRef,
        owner: ParticipantOwnerRef,
        connection: ConnectionWireId,
    ) -> ParticipantCompletionFact {
        let limit = registry.wait_limit();
        let wait = registry.wait(identity, condition);
        let outcome = match tokio::time::timeout(limit, wait).await {
            Ok(outcome) => outcome,
            Err(_elapsed) => {
                // The wait bound elapsed: drop the pending demand so a later
                // answer is not adopted against a condition this pass no
                // longer owns.
                registry.note_connection_ended(&connection);
                ClientErasureWait::Abandoned
            }
        };
        match outcome {
            ClientErasureWait::Answered(result) => Self::completion(condition, owner, &result),
            // Disconnect, replacement, or silence is not a local-erasure
            // proof: the participant stays held and re-drivable.
            ClientErasureWait::Abandoned => ParticipantCompletionFact::held(
                condition,
                owner,
                ParticipantHoldClass::Unavailable,
                WallClockWithTz::now(),
            ),
        }
    }
}

impl ErasureParticipant for ClientIncarnationParticipant {
    fn owner(&self) -> ParticipantOwnerRef {
        ParticipantOwnerRef::ClientIncarnation(self.identity)
    }

    fn demand_local_erasure(
        &self,
        command: DemandLocalErasureCommand,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ParticipantCompletionFact> + Send + '_>>
    {
        let condition = command.condition();
        let owner = command.participant();
        // A Client-bound demand never carries the target body or its search
        // material. A scope that does is a composition defect: fail closed
        // instead of projecting it onto the wire or claiming progress.
        let body_free = command.scope().target().is_none();
        Box::pin(async move {
            if !body_free || owner != ParticipantOwnerRef::ClientIncarnation(self.identity) {
                return ParticipantCompletionFact::held(
                    condition,
                    owner,
                    ParticipantHoldClass::Failed,
                    WallClockWithTz::now(),
                );
            }
            #[cfg(any(test, feature = "test-support"))]
            self.registry
                .store
                .pause_client_demand_if_armed_for_tests()
                .await;
            let Some(connection) = self.registry.current_connection(self.identity) else {
                // Unreachable: never presumed erased (§8.1).
                return ParticipantCompletionFact::held(
                    condition,
                    owner,
                    ParticipantHoldClass::Unavailable,
                    WallClockWithTz::now(),
                );
            };
            // A demand already on this connection's wire must reach `wait`
            // without an intervening store await: the retry is the waiter for
            // an already-issued side effect, and a connection end has to land
            // as Abandoned rather than minting a replacement undelivered
            // demand. Currentness is the mint gate, not the wait gate.
            if self
                .registry
                .delivered(self.identity, condition, connection)
            {
                return Self::await_client_result(
                    &self.registry,
                    self.identity,
                    condition,
                    owner,
                    connection,
                )
                .await;
            }
            // The durable Running mark may already exist; the wire demand is
            // the non-rollbackable side effect. Re-read currentness after any
            // park and before minting the in-process/wire demand so a
            // completed operation cannot class-wipe a fresh post-closure
            // Client copy.
            let current = self
                .registry
                .store
                .erasure_condition_is_current(condition)
                .await
                .unwrap_or(false);
            if !current {
                return ParticipantCompletionFact::local_complete(
                    condition,
                    owner,
                    0,
                    0,
                    WallClockWithTz::now(),
                );
            }
            self.registry.begin(self.identity, condition, connection);
            if !self
                .registry
                .delivered(self.identity, condition, connection)
            {
                // The connection loop may be inside another frame and has not
                // handed the demand to the wire yet. Blocking the whole pass
                // would stall every other participant behind a reachable
                // Client, so the demand yields bounded more-work instead: it
                // is neither completion nor hold, and a later pass re-observes
                // the same pending (same demand id) or consumes the answer.
                return ParticipantCompletionFact::more_work(
                    condition,
                    owner,
                    0,
                    0,
                    WallClockWithTz::now(),
                );
            }
            Self::await_client_result(&self.registry, self.identity, condition, owner, connection)
                .await
        })
    }
}

impl HostHandle {
    /// Write-ahead durable evidence that body-bearing material is reaching the
    /// incarnation of the connection behind `live` (lifecycle §8.1).
    ///
    /// Returns whether the caller may hand the body over. The durable row must
    /// exist before the body can leave the Host: a crash after the Client
    /// received a target-bearing copy must not lose the knowledge that the
    /// incarnation may hold it. `false` means the evidence could not be
    /// committed, so the caller withholds the body instead of delivering a
    /// copy the Host cannot account for.
    ///
    /// A delivery outside an authenticated, incarnation-pinned connection (a
    /// transport-free seam) has no Client that could receive it and needs no
    /// evidence; it is not an owner invention because no owner is recorded.
    pub(crate) async fn note_client_body_delivery(&self, live: &crate::serve::LiveInput) -> bool {
        let Some((counter, random)) = live.authority.incarnation_of(&live.connection_id) else {
            return true;
        };
        let identity = ClientTransientRegistry::identity_for(counter, random);
        #[cfg(test)]
        if let Some(gate) = self.delivery_evidence_gate() {
            gate.pause().await;
        }
        self.store
            .note_client_delivery_evidence(identity)
            .await
            .is_ok()
    }

    /// The wake handle the serving connection loop selects on to deliver a
    /// pending Client local-erasure demand.
    #[must_use]
    pub(crate) fn client_demand_wakeup(&self) -> &Notify {
        self.client_transients.demand_wakeup()
    }

    /// The bounded local-erasure demand this connection should carry now, if
    /// any. Delivery is tracked per connection, so a reconnect of the same
    /// incarnation re-delivers rather than losing the demand.
    ///
    /// The durable evidence sequence is read before the demand is handed to
    /// the wire and stored with the pending demand: a delivery that races the
    /// Client's wipe advances the sequence, and the verified answer can then
    /// no longer clear the evidence. A store read failure delivers no demand
    /// (fail closed); a later pass re-demands.
    pub(crate) async fn take_client_demand(
        &self,
        live: &crate::serve::LiveInput,
    ) -> Option<WirePayload> {
        let (counter, random) = live.authority.incarnation_of(&live.connection_id)?;
        let identity = ClientTransientRegistry::identity_for(counter, random);
        let evidence_seq = self
            .store
            .client_delivery_evidence_seq(identity)
            .await
            .ok()?;
        self.client_transients
            .take_deliverable(live.connection_id, counter, random, evidence_seq)
            .map(WirePayload::DeletionDemand)
    }

    /// Records one Client local-erasure result against its outstanding demand.
    ///
    /// A valid verified full-class result supersedes the durable delivery
    /// evidence, but only with the compare-and-delete sequence captured when
    /// the demand went on the wire: a body delivered after that moment leaves
    /// a higher sequence and the evidence survives. A partial, unverified,
    /// stale, or foreign report changes nothing; a failed clear leaves the
    /// evidence (and a later pass re-demands it).
    pub(crate) async fn accept_client_erasure_result(
        &self,
        live: &crate::serve::LiveInput,
        result: &LocalErasureResult,
    ) {
        let Some((counter, random)) = live.authority.incarnation_of(&live.connection_id) else {
            return;
        };
        let identity = ClientTransientRegistry::identity_for(counter, random);
        // An accepted result means the demand was answered; the awaiting
        // participant reads the recorded fact. A refused report is stale or
        // foreign and changes nothing (§17.2).
        let Some(accepted) = self.client_transients.accept_result(
            live.connection_id,
            counter,
            random,
            result.clone(),
        ) else {
            return;
        };
        if !verified_full_class_wipe(result) {
            return;
        }
        if let Some(expected) = accepted.evidence_seq {
            // Ordering: the delete matches the exact sequence observed at
            // delivery. Any later body delivery advanced it, so the row stays
            // and still names the incarnation at the next admission.
            let _cleared = self
                .store
                .clear_client_delivery_evidence(identity, expected)
                .await;
        }
    }
}
