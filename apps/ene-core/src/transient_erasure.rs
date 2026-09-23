use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ene_api::v1::deletion::{
    ClientTempClass, DeletionDemand, DeletionDemandWireId, DeletionTargetWire, LocalErasureResult,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{ConnectionWireId, DeletionOperationWireRef};
use ene_learning::{ExperienceCandidate, SourceRangeRef};
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

const CLIENT_ERASURE_WAIT: Duration = Duration::from_secs(30);

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

fn verified_full_class_wipe(result: &LocalErasureResult) -> bool {
    if !result.unverified.is_empty() {
        return false;
    }
    current_client_targets().iter().all(|target| {
        let DeletionTargetWire::WipeClass { class } = target;
        result.wiped.contains(class)
    })
}

/// The wire projection of one operation identity. One definition: the demand
/// and the Client's answer echo must stay byte-identical or every valid answer
/// is refused as a mismatch.
fn operation_wire(operation: DeletionOperationId) -> DeletionOperationWireRef {
    DeletionOperationWireRef(operation.as_raw().as_uuid().as_hyphenated().to_string())
}

/// The protected exact mechanical text of one operation target.
fn exact_text(target: &TargetedDeletionTarget) -> &str {
    let MechanicalDeletionTarget::ExactText(material) = &target.mechanical;
    material.expose_for_erasure()
}

fn experience_covered(
    experience: &ExperienceCandidate,
    exact: &str,
    covered: &[bool],
    identities: &[RawId],
) -> bool {
    debug_assert_eq!(covered.len(), identities.len());
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

#[derive(Debug, Default)]
pub(crate) struct TransientErasureFence {
    epoch: AtomicU64,
}

impl TransientErasureFence {
    #[must_use]
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// Invalidates every in-flight transient payload.
    pub(crate) fn invalidate(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
    }
}

pub(crate) const HOST_TRANSIENT_LEARNING_QUEUE_PAGE: usize = 32;

pub(crate) const LEARNING_FORMATION_QUEUE_CAP: usize = 256;

/// Process-local continuation of one HostTransient Learning-queue sweep.
///
/// This is not a canonical deletion registry. Restart loses it and the next
/// demand starts a new cycle from the live pending queue. `examined` keys the
/// progress by the candidate's stable source range rather than by a
/// positional count: another condition's demand removes its covered entries
/// from the shared deque and records the examined sources, so a positional
/// count no longer identifies an entry and decrementing a slot count would let
/// this sweep report Verified while an unexamined TARGET-bearing entry is
/// still queued. An entry whose source is examined is clean for this
/// condition's target; the generation advance on every queue mutation
/// invalidates the set.
/// Demands for different conditions keep separate entries so one operation's
/// demand cannot restart or overwrite another's cursor.
#[derive(Debug, Clone)]
struct HostTransientLearningSweep {
    queue_generation: u64,
    examined: HashSet<SourceRangeRef>,
}

impl HostTransientLearningSweep {
    fn new(queue_generation: u64) -> Self {
        Self {
            queue_generation,
            examined: HashSet::new(),
        }
    }
}

/// In-memory Learning formation work: the pending queue plus at most one
/// worker-owned candidate that has left the queue but does not yet have a
/// canonical formation identity.
///
/// `mutation_generation` advances on every worker or producer ownership
/// change (enqueue, overflow drop, pending→taken, taken clear). HostTransient
/// may apply a snapshotted page only while this generation still matches, so
/// a `pending → taken` race during the membership await cannot verify from
/// the stale page. HostTransient's own covered drop and examined-source
/// recording do not advance the generation: that is the confirmed apply of
/// the snapshot.
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

    /// Applies one already-selected pending page: every pending candidate
    /// whose source was selected in the snapshot is examined in queue order —
    /// covered premises drop, uncovered ones stay queued and are recorded in
    /// `examined` (they are clean for this condition's target). Candidates
    /// that were not selected keep their queue position untouched. The caller
    /// must have confirmed that [`Self::mutation_generation`] still matches
    /// the snapshot that produced `selected` / `identities` / `covered`. This
    /// does not bump the generation.
    fn apply_examined_page(
        &mut self,
        exact: &str,
        identities: &[RawId],
        covered: &[bool],
        selected: &HashSet<SourceRangeRef>,
        examined: &mut HashSet<SourceRangeRef>,
    ) -> u64 {
        let mut dropped = 0u64;
        let mut retained = VecDeque::with_capacity(self.pending.len());
        while let Some(experience) = self.pending.pop_front() {
            if selected.contains(&experience.source) {
                if experience_covered(&experience, exact, covered, identities) {
                    dropped += 1;
                    continue;
                }
                examined.insert(experience.source);
            }
            retained.push_back(experience);
        }
        self.pending = retained;
        dropped
    }
}

#[derive(Debug)]
pub(crate) struct HostTransientArrival {
    gate: tokio::sync::Mutex<()>,
    inflight_pins: AtomicU64,
    verified_generation: AtomicU64,
    publish: std::sync::Mutex<ArrivalPublishState>,
}

#[derive(Debug, Default)]
struct ArrivalPublishState {
    scan_incomplete: bool,
    owed: HashSet<DeletionOperationId>,
    /// The bounded walk's keyset position: the last unfinished operation the
    /// previous page classified. A new arrival keeps this position and only
    /// marks the walk unfinished again, so the walk rotates to the rest of the
    /// set before wrapping; rewinding to the head here would re-classify the
    /// same page forever while arrivals keep arriving and starve the tail.
    after: Option<DeletionOperationId>,
    /// The queue mutation generation the outstanding page-chain classified
    /// its pages against, recorded where the chain started at the head. A
    /// chain that crosses a live-remainder change cannot prove the head pages
    /// were classified against that remainder and restarts at the head.
    chain_generation: Option<u64>,
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
        }
    }
}

impl HostTransientArrival {
    pub(crate) async fn lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.gate.lock().await
    }

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
    ///
    /// The walk's position is not rewound: the live remainder changed, so the
    /// walk must classify every unfinished operation again, and the rotation
    /// reaches the remaining pages before wrapping to the head. Restarting at
    /// the head instead would re-classify that page on every arrival and never
    /// reach the tail. This remains scheduling only: finalizing re-derives
    /// relatedness for its own operation directly.
    pub(crate) fn note_queued_arrival(&self) {
        let mut state = crate::lock_unpoison(&self.publish);
        state.scan_incomplete = true;
    }

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
}

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
    let identities: Vec<RawId> = experiences.iter().flat_map(experience_identities).collect();
    let covered = store
        .erasure_sources_covered(current.condition(), identities.clone())
        .await?;
    if experiences
        .iter()
        .any(|experience| experience_covered(experience, &exact, &covered, &identities))
    {
        return Ok(true);
    }
    Ok(false)
}

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
///
/// The page starts after the walk's keyset position and the position advances
/// past the page, wrapping to the head when the unfinished set ends, so
/// successive calls classify the whole set without materializing it and
/// without re-reading the head. The position is scheduling only: a related
/// operation is also classified directly when its own finalizing needs it.
pub(crate) async fn publish_owed_learning_arrivals(
    store: &Store,
    arrival: &HostTransientArrival,
    queue: &std::sync::Mutex<LearningFormationQueue>,
) {
    if crate::lock_unpoison(&arrival.publish).is_clean() {
        return;
    }
    let after = crate::lock_unpoison(&arrival.publish).after;
    let started_at_head = after.is_none();
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
    let (experiences, generation) = snapshot_learning_remainder(queue);
    if experiences.is_empty() {
        *crate::lock_unpoison(&arrival.publish) = ArrivalPublishState::default();
        return;
    }
    let page_len = page.len();
    let last = page.last().map(|record| record.current.operation);
    for record in page {
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
        // Only a chain whose head page saw this live remainder can read
        // clean; a chain resumed past the head that observed a different
        // generation restarts at the head instead of clearing.
        let one_remainder = match state.chain_generation {
            Some(chain_generation) => chain_generation == generation,
            None => started_at_head,
        };
        state.after = None;
        state.chain_generation = None;
        state.scan_incomplete = !state.owed.is_empty() || !one_remainder;
    } else {
        if started_at_head {
            state.chain_generation = Some(generation);
        }
        state.scan_incomplete = true;
        state.after = last;
    }
}

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
    match experience_covers_operation(store, current, &experiences).await {
        Ok(true) => {
            publish_current_related_arrival(store, arrival, current.operation).await;
            true
        }
        Ok(false) => false,
        Err(_) => true,
    }
}

pub(crate) struct HostTransientParticipant {
    store: Store,
    fence: Arc<TransientErasureFence>,
    presentations: Arc<std::sync::Mutex<PresentationState>>,
    learning_queue: Arc<std::sync::Mutex<LearningFormationQueue>>,
    arrival: Arc<HostTransientArrival>,
    demand_lock: tokio::sync::Mutex<()>,
    sweep: std::sync::Mutex<HashMap<ErasureConditionRef, HostTransientLearningSweep>>,
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
            sweep: std::sync::Mutex::new(HashMap::new()),
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
    /// generation is still live, no body-bearing pin is in flight, and the
    /// fact's own operation has no unpublished TARGET-bearing arrival owed.
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
        let blocked = inflight > 0
            || live_generation != self.arrival.verified_generation()
            || (unpublished
                && self
                    .unpublished_blocks_finalizing(DeletionOperationRef {
                        operation: fact.condition().operation,
                        sweep: fact.condition().sweep,
                    })
                    .await);
        if blocked {
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

    fn rebase_sweep(&self, condition: ErasureConditionRef, generation: u64) {
        crate::lock_unpoison(&self.sweep)
            .insert(condition, HostTransientLearningSweep::new(generation));
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
            // the canonical row. The currentness predicate durable
            // participants re-check inside their erase transaction is read
            // before the classification snapshot and before any drop, so a
            // condition already closed here does not discard a fresh
            // post-closure premise. A condition that closes during the awaits
            // below is caught by the post-drop read, which refuses Verified
            // but cannot roll back the drop. Unreadable currentness fails
            // closed (no mutation, not Verified).
            #[cfg(any(test, feature = "test-support"))]
            self.store.pause_erasure_mutation_if_armed_for_tests().await;
            let current = match self
                .store
                .erasure_condition_is_current(command.condition())
                .await
            {
                Ok(current) => current,
                Err(_) => {
                    return ParticipantCompletionFact::held(
                        command.condition(),
                        ParticipantOwnerRef::HostTransient,
                        ParticipantHoldClass::Unavailable,
                        WallClockWithTz::now(),
                    );
                }
            };
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
            let (identities, selected, generation) = {
                let queue = crate::lock_unpoison(&self.learning_queue);
                let generation = queue.mutation_generation();
                let mut sweep = crate::lock_unpoison(&self.sweep);
                let examined = match sweep.get(&command.condition()).cloned() {
                    Some(entry) if entry.queue_generation == generation => entry.examined,
                    _ => {
                        sweep.insert(
                            command.condition(),
                            HostTransientLearningSweep::new(generation),
                        );
                        HashSet::new()
                    }
                };
                // Select the page by entry identity, in queue order: another
                // condition's demand reorders the shared deque, so the
                // progress cannot be a positional count.
                let page = queue
                    .iter()
                    .filter(|experience| !examined.contains(&experience.source))
                    .take(HOST_TRANSIENT_LEARNING_QUEUE_PAGE)
                    .collect::<Vec<_>>();
                let selected = page
                    .iter()
                    .map(|experience| experience.source)
                    .collect::<HashSet<_>>();
                let mut identities = page
                    .iter()
                    .flat_map(|experience| experience_identities(experience))
                    .collect::<Vec<_>>();
                if let Some(taken) = queue.taken() {
                    identities.extend(experience_identities(taken));
                }
                (identities, selected, generation)
            };
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
                    return ParticipantCompletionFact::held(
                        command.condition(),
                        ParticipantOwnerRef::HostTransient,
                        ParticipantHoldClass::Unavailable,
                        WallClockWithTz::now(),
                    );
                }
            };
            let dropped_presentation =
                crate::lock_unpoison(&self.presentations).invalidate_for_erasure();
            let (dropped_learning, unfinished, remainder) = {
                let mut queue = crate::lock_unpoison(&self.learning_queue);
                if queue.mutation_generation() != generation {
                    // Worker/producer mutated the queue after the snapshot.
                    // Discard the page; never drop from it and never Verified.
                    let live_generation = queue.mutation_generation();
                    self.rebase_sweep(command.condition(), live_generation);
                    let remainder = queue.len() as u64 + u64::from(queue.taken().is_some());
                    (0, true, remainder.max(1))
                } else {
                    let mut sweep = crate::lock_unpoison(&self.sweep);
                    let examined = &mut sweep
                        .entry(command.condition())
                        .or_insert_with(|| HostTransientLearningSweep::new(generation))
                        .examined;
                    let dropped = queue.apply_examined_page(
                        &exact,
                        &identities,
                        &covered,
                        &selected,
                        examined,
                    );
                    // `taken` is inspected from the live slot under the still-
                    // matching generation. HostTransient never drops it.
                    let taken_covered = queue.taken().is_some_and(|experience| {
                        experience_covered(experience, &exact, &covered, &identities)
                    });
                    // Verified requires that no queued candidate is left
                    // unexamined for this condition's target.
                    let unfinished = taken_covered
                        || queue
                            .iter()
                            .any(|experience| !examined.contains(&experience.source));
                    let remainder = queue.len() as u64 + u64::from(queue.taken().is_some());
                    if !unfinished {
                        // Every live pending source is examined for this
                        // condition: the sweep is finished. Drop the
                        // process-local cursor so the singleton participant's
                        // map does not grow for the Host's lifetime; a later
                        // demand for the same condition rebases from empty.
                        sweep.remove(&command.condition());
                    }
                    (dropped, unfinished, remainder.max(1))
                }
            };
            self.fence.invalidate();
            // Process memory cannot roll back. A second currentness read
            // after the drop refuses Verified when the operation closed in
            // the window: the durable record will not treat a stale demand as
            // completion, and a later pass of a still-current sweep re-demands.
            let still_current = match self
                .store
                .erasure_condition_is_current(command.condition())
                .await
            {
                Ok(still_current) => still_current,
                Err(_) => {
                    return ParticipantCompletionFact::held(
                        command.condition(),
                        ParticipantOwnerRef::HostTransient,
                        ParticipantHoldClass::Unavailable,
                        WallClockWithTz::now(),
                    );
                }
            };
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
            let inflight = self.arrival.inflight_pins();
            {
                let queue = crate::lock_unpoison(&self.learning_queue);
                if queue.mutation_generation() != generation || inflight > 0 {
                    if queue.mutation_generation() != generation {
                        let live_generation = queue.mutation_generation();
                        self.rebase_sweep(command.condition(), live_generation);
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

struct PendingDemand {
    id: String,
    condition: ErasureConditionRef,
    connection: ConnectionWireId,
    /// Whether this demand already went on the wire. Delivery is once per
    /// live connection; a connection end abandons the demand, so a later pass
    /// re-demands the same condition under a fresh demand id.
    delivered: bool,
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

struct AcceptedClientErasure {
    evidence_seq: Option<u64>,
}

enum ClientErasureWait {
    Answered(LocalErasureResult),
    Abandoned,
}

pub(crate) struct ClientTransientRegistry {
    /// At most one outstanding demand per incarnation.
    inner: std::sync::Mutex<HashMap<RawId, PendingDemand>>,
    /// Wakes connection loops to deliver a pending demand.
    delivery_wake: Notify,
    result_wake: Notify,
    table: OnceLock<Arc<ConnectionTable>>,
    store: Store,
    wait_limit: std::sync::Mutex<Option<Duration>>,
}

impl ClientTransientRegistry {
    #[must_use]
    pub(crate) fn new(store: Store) -> Self {
        Self {
            inner: std::sync::Mutex::new(HashMap::new()),
            delivery_wake: Notify::new(),
            result_wake: Notify::new(),
            table: OnceLock::new(),
            store,
            wait_limit: std::sync::Mutex::new(None),
        }
    }

    pub(crate) fn install_connection_table(&self, table: Arc<ConnectionTable>) {
        if self.table.set(table).is_err() {
            // A previously installed table stays authoritative.
        }
    }

    #[must_use]
    fn identity_for(counter: u64, random: u64) -> RawId {
        RawId::from_uuid(Uuid::from_u64_pair(counter, random))
    }

    /// The current authenticated connection of one tracked incarnation.
    ///
    /// The boot incarnation is recovered from the identity itself, so a
    /// durable participant snapshot resolves after a Host restart even though
    /// the in-flight demand plumbing did not survive it (`Self::identity_for`:
    /// the identity is the deterministic projection of the boot incarnation,
    /// so the incarnation is recovered from the identity itself). A missing
    /// connection is an explicit unreachable hold.
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
    fn begin(&self, identity: RawId, condition: ErasureConditionRef, connection: ConnectionWireId) {
        let mut inner = crate::lock_unpoison(&self.inner);
        match inner.get(&identity) {
            Some(existing)
                if existing.condition == condition && existing.connection == connection => {}
            _ => {
                let id = Uuid::new_v4().as_hyphenated().to_string();
                inner.insert(
                    identity,
                    PendingDemand {
                        id,
                        condition,
                        connection,
                        delivered: false,
                        evidence_seq: None,
                        state: PendingState::Awaiting,
                    },
                );
            }
        }
        drop(inner);
        self.delivery_wake.notify_waiters();
        self.delivery_wake.notify_one();
    }

    fn delivered(
        &self,
        identity: RawId,
        condition: ErasureConditionRef,
        connection: ConnectionWireId,
    ) -> bool {
        let inner = crate::lock_unpoison(&self.inner);
        inner.get(&identity).is_some_and(|pending| {
            pending.connection == connection && pending.condition == condition && pending.delivered
        })
    }

    fn take_deliverable(
        &self,
        connection: ConnectionWireId,
        counter: u64,
        random: u64,
        evidence_seq: Option<u64>,
    ) -> Option<DeletionDemand> {
        let identity = Self::identity_for(counter, random);
        let mut inner = crate::lock_unpoison(&self.inner);
        let pending = inner.get_mut(&identity)?;
        if pending.connection != connection {
            return None;
        }
        if pending.delivered {
            return None;
        }
        if matches!(pending.state, PendingState::Answered(_)) {
            return None;
        }
        pending.delivered = true;
        pending.evidence_seq = evidence_seq;
        Some(DeletionDemand {
            demand: DeletionDemandWireId(pending.id.clone()),
            operation: operation_wire(pending.condition.operation),
            sweep: pending.condition.sweep.as_u64(),
            targets: current_client_targets(),
        })
    }

    fn accept_result(
        &self,
        connection: ConnectionWireId,
        counter: u64,
        random: u64,
        result: LocalErasureResult,
    ) -> Option<AcceptedClientErasure> {
        let identity = Self::identity_for(counter, random);
        let mut inner = crate::lock_unpoison(&self.inner);
        let pending = inner.get_mut(&identity)?;
        if pending.connection != connection || pending.id != result.demand.0 {
            return None;
        }
        if result.operation != operation_wire(pending.condition.operation)
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

    pub(crate) fn note_connection_ended(&self, connection: &ConnectionWireId) {
        let mut inner = crate::lock_unpoison(&self.inner);
        let mut abandoned = false;
        inner.retain(|_, pending| {
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

    async fn wait(&self, identity: RawId, condition: ErasureConditionRef) -> ClientErasureWait {
        loop {
            let ready = {
                let mut inner = crate::lock_unpoison(&self.inner);
                match inner.get(&identity) {
                    Some(pending) if pending.condition == condition => match &pending.state {
                        PendingState::Answered(_) => inner.remove(&identity),
                        PendingState::Awaiting => None,
                    },
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
        if verified_full_class_wipe(result) {
            ParticipantCompletionFact::verified(condition, owner, wiped, WallClockWithTz::now())
        } else {
            // A report that does not cover the whole demanded class set keeps
            // the bounded pass at local completion with a remainder: a class
            // absent from both lists is as unproven as a reported unverified
            // one, and the Host never upgrades either to verified.
            let missing = current_client_targets()
                .iter()
                .filter(|target| {
                    let DeletionTargetWire::WipeClass { class } = target;
                    !result.wiped.contains(class) && !result.unverified.contains(class)
                })
                .count() as u64;
            ParticipantCompletionFact::local_complete(
                condition,
                owner,
                wiped,
                result.unverified.len() as u64 + missing,
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
                registry.note_connection_ended(&connection);
                ClientErasureWait::Abandoned
            }
        };
        match outcome {
            ClientErasureWait::Answered(result) => Self::completion(condition, owner, &result),
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
                return ParticipantCompletionFact::held(
                    condition,
                    owner,
                    ParticipantHoldClass::Unavailable,
                    WallClockWithTz::now(),
                );
            };
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
            let current = match self
                .registry
                .store
                .erasure_condition_is_current(condition)
                .await
            {
                Ok(current) => current,
                Err(_) => {
                    return ParticipantCompletionFact::held(
                        condition,
                        owner,
                        ParticipantHoldClass::Unavailable,
                        WallClockWithTz::now(),
                    );
                }
            };
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

    #[must_use]
    pub(crate) fn client_demand_wakeup(&self) -> &Notify {
        self.client_transients.demand_wakeup()
    }

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

    pub(crate) async fn accept_client_erasure_result(
        &self,
        live: &crate::serve::LiveInput,
        result: &LocalErasureResult,
    ) {
        let Some((counter, random)) = live.authority.incarnation_of(&live.connection_id) else {
            return;
        };
        let identity = ClientTransientRegistry::identity_for(counter, random);
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
            let _cleared = self
                .store
                .clear_client_delivery_evidence(identity, expected)
                .await;
        }
    }
}
