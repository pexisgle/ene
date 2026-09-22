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

    pub(crate) fn invalidate(&self) -> u64 {
        self.epoch.fetch_add(1, Ordering::SeqCst) + 1
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

    #[cfg(test)]
    pub(crate) fn front(&self) -> Option<&ExperienceCandidate> {
        self.pending.front()
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
    last_classified: AtomicUsize,
    last_direct_classified: AtomicUsize,
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

    #[cfg(test)]
    pub(crate) fn last_classified(&self) -> usize {
        self.last_classified.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn last_direct_classified(&self) -> usize {
        self.last_direct_classified.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn scan_incomplete(&self) -> bool {
        crate::lock_unpoison(&self.publish).scan_incomplete
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

pub(crate) struct HostTransientParticipant {
    store: Store,
    fence: Arc<TransientErasureFence>,
    presentations: Arc<std::sync::Mutex<PresentationState>>,
    learning_queue: Arc<std::sync::Mutex<LearningFormationQueue>>,
    arrival: Arc<HostTransientArrival>,
    demand_lock: tokio::sync::Mutex<()>,
    sweep: std::sync::Mutex<HashMap<ErasureConditionRef, HostTransientLearningSweep>>,
    #[cfg(test)]
    last_scanned: std::sync::atomic::AtomicUsize,
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
            #[cfg(test)]
            last_scanned: std::sync::atomic::AtomicUsize::new(0),
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

    #[cfg(test)]
    fn last_scanned(&self) -> usize {
        self.last_scanned.load(Ordering::SeqCst)
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
                #[cfg(test)]
                self.last_scanned.store(page.len(), Ordering::SeqCst);
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
    inner: std::sync::Mutex<ClientTransientInner>,
    delivery_wake: Notify,
    result_wake: Notify,
    table: OnceLock<Arc<ConnectionTable>>,
    store: Store,
    wait_limit: std::sync::Mutex<Option<Duration>>,
}

#[derive(Default)]
struct ClientTransientInner {
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
                        delivered: false,
                        evidence_seq: None,
                        state: PendingState::Awaiting,
                    },
                );
                id
            }
        };
        drop(inner);
        self.delivery_wake.notify_waiters();
        self.delivery_wake.notify_one();
        id
    }

    fn delivered(
        &self,
        identity: RawId,
        condition: ErasureConditionRef,
        connection: ConnectionWireId,
    ) -> bool {
        let inner = crate::lock_unpoison(&self.inner);
        inner.pending.get(&identity).is_some_and(|pending| {
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
        let pending = inner.pending.get_mut(&identity)?;
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
        let pending = inner.pending.get_mut(&identity)?;
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

    async fn wait(&self, identity: RawId, condition: ErasureConditionRef) -> ClientErasureWait {
        loop {
            let ready = {
                let mut inner = crate::lock_unpoison(&self.inner);
                match inner.pending.get(&identity) {
                    Some(pending) if pending.condition == condition => match &pending.state {
                        PendingState::Answered(_) => inner.pending.remove(&identity),
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::serve::{CredStore, LiveInput};
    use crate::targeted_deletion::TargetedDeletionPass;
    use crate::test_support::{authenticate, memory_handle};
    use ene_credential::MemoryCredentialStore;
    use ene_learning::{ExperienceRole, ExperienceSourceKind, ExperienceTurn, SourceRangeRef};
    use ene_preservation::{
        ConfirmTargetedDeletionOutcome, DELETION_RECONCILIATION_PAGE_SIZE,
        DeletionFinalizationOutcome, DeletionOperationPhase, DeletionPurpose,
        DeletionReconciliationOutcome, DeletionSearchMaterial, DemandLocalErasureCommand,
        ParticipantCompletionFact, ParticipantCompletionOutcome, ParticipantCompletionStatus,
        ParticipantErasureScope, ParticipantProgress, StageTargetedDeletionRequestCommand,
        StageTargetedDeletionRequestOutcome,
    };
    use ene_primitive::WallClockWithTz;
    use ene_store::HOST_TRANSIENT_ARRIVAL_PAGE;

    const DEVICE: &str = "client-a3c";

    fn target(text: &str) -> TargetedDeletionTarget {
        TargetedDeletionTarget {
            mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                text.to_owned(),
            )),
            semantic_hints: Vec::new(),
        }
    }

    fn experience(text: &str) -> ExperienceCandidate {
        ExperienceCandidate {
            companion: RawId::new(),
            source: SourceRangeRef {
                kind: ExperienceSourceKind::Dialogue,
                start: RawId::new(),
                end: RawId::new(),
            },
            sources: Vec::new(),
            transcript: vec![ExperienceTurn {
                role: ExperienceRole::Owner,
                text: text.to_owned(),
                at: None,
            }],
            at: WallClockWithTz::now(),
        }
    }

    async fn admit(
        handle: &HostHandle,
        text: &str,
        participants: Vec<ParticipantOwnerRef>,
    ) -> ene_preservation::DeletionOperationRef {
        let staged = handle
            .store
            .stage_targeted_deletion(StageTargetedDeletionRequestCommand::new(
                target(text),
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
            other => panic!("unexpected admission outcome: {other:?}"),
        }
    }

    fn command(
        condition: ErasureConditionRef,
        owner: ParticipantOwnerRef,
        scope: ParticipantErasureScope,
    ) -> DemandLocalErasureCommand {
        DemandLocalErasureCommand::new(condition, owner, scope)
    }

    fn host_transient(handle: &HostHandle) -> HostTransientParticipant {
        HostTransientParticipant::new(
            handle.store.clone(),
            Arc::clone(&handle.transient_fence),
            Arc::clone(&handle.presentations),
            Arc::clone(&handle.learning_queue),
            Arc::clone(&handle.host_transient_arrival),
        )
    }

    fn pending_transcripts(handle: &HostHandle) -> Vec<String> {
        crate::lock_unpoison(&handle.learning_queue)
            .iter()
            .map(|item| item.transcript[0].text.clone())
            .collect()
    }

    fn sorted_texts(mut texts: Vec<String>) -> Vec<String> {
        texts.sort();
        texts
    }

    fn page_demand_bound(queued: usize) -> usize {
        let page = super::HOST_TRANSIENT_LEARNING_QUEUE_PAGE;
        queued.div_ceil(page).max(1)
    }

    async fn deletion_phase(
        handle: &HostHandle,
        operation: ene_preservation::DeletionOperationId,
    ) -> DeletionOperationPhase {
        operation_record(handle, operation).await.phase
    }

    async fn operation_record(
        handle: &HostHandle,
        operation: ene_preservation::DeletionOperationId,
    ) -> ene_preservation::DeletionOperationRecord {
        let mut after = None;
        loop {
            let page = handle
                .store
                .deletion_status(after, 100)
                .await
                .expect("the status must read");
            if page.is_empty() {
                panic!("the operation must stay readable");
            }
            let page_len = page.len();
            if let Some(record) = page
                .iter()
                .find(|record| record.current.operation == operation)
                .cloned()
            {
                return record;
            }
            after = Some(page[page_len - 1].current.operation);
            if page_len < 100 {
                panic!("the operation must stay readable");
            }
        }
    }

    async fn host_transient_is_verified(
        handle: &HostHandle,
        operation: ene_preservation::DeletionOperationId,
    ) -> bool {
        handle
            .store
            .deletion_participants(operation, None, 100)
            .await
            .expect("participant rows read")
            .into_iter()
            .find(|record| record.participant.owner == ParticipantOwnerRef::HostTransient)
            .expect("the HostTransient row exists")
            .progress
            .is_verified()
    }

    async fn drive_until_completed(
        handle: &HostHandle,
        operation: ene_preservation::DeletionOperationId,
    ) {
        for _ in 0..64 {
            let outcome = handle
                .drive_targeted_deletion(TargetedDeletionPass::new(100, 16))
                .await
                .expect("the fan-out must not fail");
            if deletion_phase(handle, operation).await == DeletionOperationPhase::Completed
                && outcome.demands == 0
                && outcome.reconciliation_pages == 0
            {
                return;
            }
        }
        panic!("Targeted Deletion must converge after the arrival is collected");
    }

    fn queued_contains(handle: &HostHandle, needle: &str) -> bool {
        crate::lock_unpoison(&handle.learning_queue)
            .iter()
            .any(|item| item.transcript[0].text.contains(needle))
    }

    async fn reconcile_until_complete(
        handle: &HostHandle,
        current: ene_preservation::DeletionOperationRef,
    ) {
        for _ in 0..16 {
            match handle
                .store
                .reconcile_deletion_sources(current, DELETION_RECONCILIATION_PAGE_SIZE)
                .await
                .expect("reconciliation must read")
            {
                DeletionReconciliationOutcome::Complete
                | DeletionReconciliationOutcome::Finalizing
                | DeletionReconciliationOutcome::Completed => return,
                DeletionReconciliationOutcome::Advanced => continue,
                other => panic!("unexpected reconciliation outcome: {other:?}"),
            }
        }
        panic!("covered-source reconciliation must finish");
    }

    /// Finalizes one operation through the production arrival gate without
    /// demanding sibling operations. A full fan-out would collect a related
    /// TARGET while proving an unrelated sibling can Complete.
    async fn try_finalize_one_operation(
        handle: &HostHandle,
        current: ene_preservation::DeletionOperationRef,
    ) -> DeletionOperationPhase {
        reconcile_until_complete(handle, current).await;
        let _gate = handle.host_transient_arrival.lock().await;
        super::publish_owed_learning_arrivals(
            &handle.store,
            &handle.host_transient_arrival,
            &handle.learning_queue,
        )
        .await;
        if super::unpublished_blocks_finalizing(
            &handle.store,
            &handle.host_transient_arrival,
            &handle.learning_queue,
            current,
        )
        .await
        {
            return deletion_phase(handle, current.operation).await;
        }
        match handle
            .store
            .begin_deletion_finalizing(current)
            .await
            .expect("begin finalizing must not fail technically")
        {
            DeletionFinalizationOutcome::Finalizing
            | DeletionFinalizationOutcome::CompletedAlready => {}
            other => panic!("begin finalizing must enter the sealed boundary: {other:?}"),
        }
        match handle
            .store
            .complete_deletion_finalizing(current)
            .await
            .expect("complete finalizing must not fail technically")
        {
            DeletionFinalizationOutcome::Completed
            | DeletionFinalizationOutcome::CompletedAlready => {}
            other => panic!("complete finalizing must commit or already be sealed: {other:?}"),
        }
        deletion_phase(handle, current.operation).await
    }

    async fn drive_until_operation_completed(
        handle: &HostHandle,
        current: ene_preservation::DeletionOperationRef,
    ) {
        for _ in 0..64 {
            if try_finalize_one_operation(handle, current).await
                == DeletionOperationPhase::Completed
            {
                return;
            }
        }
        panic!(
            "the unrelated operation must Complete without waiting on another classification failure"
        );
    }

    async fn verify_host_transient(
        handle: &HostHandle,
        current: ene_preservation::DeletionOperationRef,
    ) {
        assert_eq!(
            handle
                .store
                .record_participant_completion(ParticipantCompletionFact::verified(
                    current.condition(),
                    ParticipantOwnerRef::HostTransient,
                    0,
                    WallClockWithTz::now(),
                ))
                .await
                .expect("the verified fact must record"),
            ParticipantCompletionOutcome::Recorded(ParticipantProgress::Verified {
                sweep: current.sweep,
            })
        );
        let generation = crate::lock_unpoison(&handle.learning_queue).mutation_generation();
        handle
            .host_transient_arrival
            .set_verified_generation(generation);
    }

    async fn demand_secret(
        participant: &HostTransientParticipant,
        condition: ErasureConditionRef,
    ) -> ParticipantCompletionFact {
        participant
            .demand_local_erasure(command(
                condition,
                ParticipantOwnerRef::HostTransient,
                ParticipantErasureScope::local(target("secret body")),
            ))
            .await
    }

    struct ClientFixture {
        handle: HostHandle,
        _dir: tempfile::TempDir,
        table: Arc<ConnectionTable>,
        connection: ConnectionWireId,
        live: LiveInput,
        identity: RawId,
        condition: ErasureConditionRef,
    }

    async fn client_fixture(tag: &str) -> ClientFixture {
        let (handle, dir) = memory_handle(tag).await.expect("the handle must open");
        let table = Arc::new(ConnectionTable::new());
        let connection = table.note_accept();
        authenticate(&table, &connection, DEVICE);
        assert!(
            table.pin_incarnation_for_tests(&connection, 41, 42),
            "the accepted connection must accept one pinned incarnation"
        );
        handle.install_client_connection_table(Arc::clone(&table));
        let live = table
            .snapshot(&connection)
            .expect("the connection snapshots");
        // Body-bearing material reached this incarnation: the Host records
        // the durable delivery evidence.
        assert!(handle.note_client_body_delivery(&live).await);
        let identity = ClientTransientRegistry::identity_for(41, 42);
        let current = admit(
            &handle,
            "secret body",
            vec![ParticipantOwnerRef::ClientIncarnation(identity)],
        )
        .await;
        ClientFixture {
            handle,
            _dir: dir,
            table,
            connection,
            live,
            identity,
            condition: current.condition(),
        }
    }

    async fn client_row(fixture: &ClientFixture) -> ene_preservation::DeletionParticipantRecord {
        fixture
            .handle
            .store
            .deletion_participants(fixture.condition.operation, None, 100)
            .await
            .expect("participant rows read")
            .into_iter()
            .find(|record| {
                record.participant.owner == ParticipantOwnerRef::ClientIncarnation(fixture.identity)
            })
            .expect("the required client row exists")
    }

    #[tokio::test]
    async fn a_delivered_body_tracks_the_incarnation_for_later_admissions() {
        let (handle, _dir) = memory_handle("a3c-track").await.expect("the handle opens");
        assert!(
            !handle
                .required_deletion_participants()
                .await
                .expect("the required snapshot must read")
                .iter()
                .any(|owner| owner.is_incarnation()),
            "a handle with no delivery claims no Client incarnation"
        );
        let table = Arc::new(ConnectionTable::new());
        let connection = table.note_accept();
        authenticate(&table, &connection, DEVICE);
        assert!(table.pin_incarnation_for_tests(&connection, 41, 42));
        handle.install_client_connection_table(Arc::clone(&table));
        let live = table
            .snapshot(&connection)
            .expect("the connection snapshots");
        assert!(handle.note_client_body_delivery(&live).await);
        let identity = ClientTransientRegistry::identity_for(41, 42);
        let required = handle
            .required_deletion_participants()
            .await
            .expect("the required snapshot must read");
        assert!(
            required.contains(&ParticipantOwnerRef::ClientIncarnation(identity)),
            "the delivered incarnation is required: {required:?}"
        );
        // A replacement boot is a different owner and never inherits it.
        assert!(!required.contains(&ParticipantOwnerRef::ClientIncarnation(
            ClientTransientRegistry::identity_for(41, 43)
        )));
    }

    #[tokio::test]
    async fn a_disconnected_client_is_held_and_never_a_completion() {
        let fixture = client_fixture("a3c-client-held").await;
        // A replacement connection supersedes the demanded one, and the ended
        // connection lifetime abandons any pending demand for it.
        let replacement = fixture.table.note_accept();
        authenticate(&fixture.table, &replacement, DEVICE);
        fixture.handle.on_connection_closed(&fixture.connection);
        let outcome = fixture
            .handle
            .drive_targeted_deletion(TargetedDeletionPass::default())
            .await
            .expect("the pass runs");
        assert_eq!(outcome.verified, 0);
        let row = client_row(&fixture).await;
        assert_eq!(
            row.progress.hold_reason(),
            Some(ParticipantHoldClass::Unavailable),
            "a disconnect is an explicit unreachable hold"
        );
        assert!(!row.progress.is_verified());
        assert_eq!(
            fixture
                .handle
                .store
                .unfinished_deletions(None, 100)
                .await
                .expect("unfinished operations read")
                .len(),
            1,
            "a Client hold keeps the operation retryable-incomplete"
        );
    }

    #[tokio::test]
    async fn a_restarted_host_resolves_a_snapshotted_client_incarnation() {
        let fixture = client_fixture("a3c-client-restart").await;
        // A fresh process over the same directory: it has no in-memory
        // delivery tracking and no registered Client implementation, exactly
        // like a real Host restart. The durable operation snapshot still
        // names the incarnation owner.
        let reopened = HostHandle::open_with_cred_store(
            fixture._dir.path(),
            CredStore::Memory(MemoryCredentialStore::new()),
        )
        .await
        .expect("the restarted handle must open");
        let outcome = reopened
            .drive_targeted_deletion(TargetedDeletionPass::default())
            .await
            .expect("the pass runs");
        assert_eq!(outcome.verified, 0);
        let row = reopened
            .store
            .deletion_participants(fixture.condition.operation, None, 100)
            .await
            .expect("participant rows read")
            .into_iter()
            .find(|record| {
                record.participant.owner == ParticipantOwnerRef::ClientIncarnation(fixture.identity)
            })
            .expect("the snapshotted client row exists");
        assert_eq!(
            row.progress.hold_reason(),
            Some(ParticipantHoldClass::Unavailable),
            "a restart resolves the durable owner to an unreachable hold, never to a composition defect"
        );
    }

    #[tokio::test]
    async fn an_undelivered_client_demand_yields_more_work_instead_of_blocking() {
        let fixture = client_fixture("a3c-client-undelivered").await;
        let participant = ClientIncarnationParticipant::new(
            fixture.identity,
            Arc::clone(&fixture.handle.client_transients),
        );
        // The connection loop has not handed the demand to the wire (it may be
        // inside another frame): bounded work yields instead of stalling the
        // pass behind a reachable Client.
        let fact = participant
            .demand_local_erasure(command(
                fixture.condition,
                ParticipantOwnerRef::ClientIncarnation(fixture.identity),
                ParticipantErasureScope::correlation_only(),
            ))
            .await;
        assert_eq!(
            fact.status(),
            ParticipantCompletionStatus::MoreWork,
            "an undelivered demand is neither completion nor hold"
        );
    }

    #[tokio::test]
    async fn an_in_flight_client_demand_is_abandoned_when_its_connection_ends() {
        let fixture = client_fixture("a3c-client-abandoned").await;
        let participant = ClientIncarnationParticipant::new(
            fixture.identity,
            Arc::clone(&fixture.handle.client_transients),
        );
        // First bounded call yields; the wire handoff is what makes the next
        // call park on the result.
        let fact = participant
            .demand_local_erasure(command(
                fixture.condition,
                ParticipantOwnerRef::ClientIncarnation(fixture.identity),
                ParticipantErasureScope::correlation_only(),
            ))
            .await;
        assert_eq!(fact.status(), ParticipantCompletionStatus::MoreWork);
        let Some(WirePayload::DeletionDemand(_)) =
            fixture.handle.take_client_demand(&fixture.live).await
        else {
            panic!("the pending demand must be deliverable");
        };
        let mut demand = Box::pin(participant.demand_local_erasure(command(
            fixture.condition,
            ParticipantOwnerRef::ClientIncarnation(fixture.identity),
            ParticipantErasureScope::correlation_only(),
        )));
        tokio::select! {
            biased;
            _ = &mut demand => panic!("the demand must not finish before its connection ends"),
            () = tokio::task::yield_now() => {}
        }
        // The connection ends after the demand is in flight: the waiter hears
        // an explicit hold instead of waiting for a proof that cannot arrive.
        fixture.handle.on_connection_closed(&fixture.connection);
        let fact = demand.await;
        assert_eq!(
            fact.status(),
            ParticipantCompletionStatus::Held(ParticipantHoldClass::Unavailable),
            "an ended connection is never read as local erasure"
        );
    }

    #[tokio::test]
    async fn a_connected_client_demand_waits_for_its_result_and_completes_locally() {
        let fixture = client_fixture("a3c-client-answer").await;
        let participant = ClientIncarnationParticipant::new(
            fixture.identity,
            Arc::clone(&fixture.handle.client_transients),
        );
        let owner = ParticipantOwnerRef::ClientIncarnation(fixture.identity);
        // The first bounded call yields because the demand is not on the wire
        // yet; the wire handoff then makes the retry park on the result.
        let yielded = participant
            .demand_local_erasure(command(
                fixture.condition,
                owner,
                ParticipantErasureScope::correlation_only(),
            ))
            .await;
        assert_eq!(yielded.status(), ParticipantCompletionStatus::MoreWork);
        let payload = fixture
            .handle
            .take_client_demand(&fixture.live)
            .await
            .expect("the pending demand must be deliverable");
        let WirePayload::DeletionDemand(demand_payload) = payload else {
            panic!("the delivery is a DeletionDemand");
        };
        assert!(
            !format!("{demand_payload:?}").contains("secret body"),
            "the wire demand never carries the target body"
        );
        let mut demand = Box::pin(participant.demand_local_erasure(command(
            fixture.condition,
            owner,
            ParticipantErasureScope::correlation_only(),
        )));
        // One turn lets the retry register and park on its result.
        tokio::select! {
            biased;
            _ = &mut demand => panic!("the demand must not finish before its result"),
            () = tokio::task::yield_now() => {}
        }
        let result = LocalErasureResult {
            demand: demand_payload.demand.clone(),
            operation: demand_payload.operation.clone(),
            sweep: demand_payload.sweep,
            wiped: vec![
                ClientTempClass::PresentationBuffer,
                ClientTempClass::InputDraft,
            ],
            unverified: Vec::new(),
        };
        fixture
            .handle
            .accept_client_erasure_result(&fixture.live, &result)
            .await;
        // A duplicate report is refused: the demand was answered once.
        fixture
            .handle
            .accept_client_erasure_result(&fixture.live, &result)
            .await;
        let fact = demand.await;
        assert_eq!(
            fact.status(),
            ParticipantCompletionStatus::Verified,
            "the client's local erasure pass is its own completion premise"
        );
        assert_eq!(fact.participant(), owner);
        assert_eq!(fact.condition(), fixture.condition);
        // Local completion is not global completion: the operation is still
        // unfinished and no row is claimed beyond this participant.
        let rows = fixture
            .handle
            .store
            .unfinished_deletions(None, 100)
            .await
            .expect("unfinished operations read");
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn an_unverified_client_class_keeps_the_participant_unfinished() {
        let fixture = client_fixture("a3c-client-unverified").await;
        let participant = ClientIncarnationParticipant::new(
            fixture.identity,
            Arc::clone(&fixture.handle.client_transients),
        );
        let owner = ParticipantOwnerRef::ClientIncarnation(fixture.identity);
        let yielded = participant
            .demand_local_erasure(command(
                fixture.condition,
                owner,
                ParticipantErasureScope::correlation_only(),
            ))
            .await;
        assert_eq!(yielded.status(), ParticipantCompletionStatus::MoreWork);
        let Some(WirePayload::DeletionDemand(demand_payload)) =
            fixture.handle.take_client_demand(&fixture.live).await
        else {
            panic!("the pending demand must be deliverable");
        };
        let mut demand = Box::pin(participant.demand_local_erasure(command(
            fixture.condition,
            owner,
            ParticipantErasureScope::correlation_only(),
        )));
        tokio::select! {
            biased;
            _ = &mut demand => panic!("the demand must not finish before its result"),
            () = tokio::task::yield_now() => {}
        }
        let result = LocalErasureResult {
            demand: demand_payload.demand.clone(),
            operation: demand_payload.operation.clone(),
            sweep: demand_payload.sweep,
            wiped: vec![ClientTempClass::InputDraft],
            unverified: vec![ClientTempClass::PresentationBuffer],
        };
        fixture
            .handle
            .accept_client_erasure_result(&fixture.live, &result)
            .await;
        let fact = demand.await;
        assert_eq!(
            fact.status(),
            ParticipantCompletionStatus::LocalComplete,
            "an unverified local class is never upgraded to verified"
        );
        assert_eq!(fact.remainder_count(), 1);
    }

    /// A narrow report that omits a demanded class is not proof of its
    /// erasure: the participant stays locally complete with a remainder, and
    /// the Host never upgrades it to Verified.
    #[tokio::test]
    async fn a_narrow_client_report_is_never_upgraded_to_verified() {
        let fixture = client_fixture("a3c-client-narrow").await;
        let payload = demand_on_wire(&fixture).await;
        let owner = ParticipantOwnerRef::ClientIncarnation(fixture.identity);
        for (wiped, unverified) in [
            (Vec::new(), Vec::new()),
            (vec![ClientTempClass::InputDraft], Vec::new()),
        ] {
            let result = wipe_result(&payload, wiped, unverified);
            let fact = ClientIncarnationParticipant::completion(fixture.condition, owner, &result);
            assert_eq!(
                fact.status(),
                ParticipantCompletionStatus::LocalComplete,
                "a report missing a demanded class is never a verified wipe"
            );
            assert!(
                fact.remainder_count() >= 1,
                "a demanded class absent from both lists is a remainder"
            );
        }
    }

    #[tokio::test]
    async fn a_delivered_silent_client_demand_holds_after_the_wait_bound() {
        let fixture = client_fixture("a3c-client-timeout").await;
        fixture
            .handle
            .client_transients
            .set_wait_limit_for_test(Duration::from_millis(100));
        let participant = ClientIncarnationParticipant::new(
            fixture.identity,
            Arc::clone(&fixture.handle.client_transients),
        );
        // The demand must be on the wire for silence to mean anything: an
        // undelivered demand yields more work instead of a hold.
        let yielded = participant
            .demand_local_erasure(command(
                fixture.condition,
                ParticipantOwnerRef::ClientIncarnation(fixture.identity),
                ParticipantErasureScope::correlation_only(),
            ))
            .await;
        assert_eq!(yielded.status(), ParticipantCompletionStatus::MoreWork);
        let Some(WirePayload::DeletionDemand(_)) =
            fixture.handle.take_client_demand(&fixture.live).await
        else {
            panic!("the pending demand must be deliverable");
        };
        let fact = participant
            .demand_local_erasure(command(
                fixture.condition,
                ParticipantOwnerRef::ClientIncarnation(fixture.identity),
                ParticipantErasureScope::correlation_only(),
            ))
            .await;
        assert_eq!(
            fact.status(),
            ParticipantCompletionStatus::Held(ParticipantHoldClass::Unavailable),
            "a delivered-but-silent Client is a hold, never a completion"
        );
    }

    #[tokio::test]
    async fn a_client_bound_demand_with_a_body_scope_fails_closed() {
        let fixture = client_fixture("a3c-client-body-scope").await;
        let participant = ClientIncarnationParticipant::new(
            fixture.identity,
            Arc::clone(&fixture.handle.client_transients),
        );
        let fact = participant
            .demand_local_erasure(command(
                fixture.condition,
                ParticipantOwnerRef::ClientIncarnation(fixture.identity),
                ParticipantErasureScope::local(target("secret body")),
            ))
            .await;
        assert_eq!(
            fact.status(),
            ParticipantCompletionStatus::Held(ParticipantHoldClass::Failed),
            "a body-bearing Client scope is a composition defect, never a success"
        );
    }

    // --- M1: durable delivery evidence (lifecycle §8.1) ---------------------

    /// One Client result for a wire demand, exactly as the first-party Client
    /// builds it.
    fn wipe_result(
        payload: &DeletionDemand,
        wiped: Vec<ClientTempClass>,
        unverified: Vec<ClientTempClass>,
    ) -> LocalErasureResult {
        LocalErasureResult {
            demand: payload.demand.clone(),
            operation: payload.operation.clone(),
            sweep: payload.sweep,
            wiped,
            unverified,
        }
    }

    /// A verified report of the whole closed class set.
    fn full_wipe(payload: &DeletionDemand) -> LocalErasureResult {
        wipe_result(
            payload,
            vec![
                ClientTempClass::PresentationBuffer,
                ClientTempClass::InputDraft,
            ],
            Vec::new(),
        )
    }

    /// Begins one demand, hands it to the wire, and returns the payload the
    /// Client would answer. The durable evidence sequence is captured by the
    /// handoff exactly as production does.
    ///
    /// The pending demand is ended first: a delivered demand is not re-handed
    /// to the same connection, so a second cycle stands in for the reconnect
    /// lifetime a real re-demand follows.
    async fn demand_on_wire(fixture: &ClientFixture) -> DeletionDemand {
        fixture
            .handle
            .client_transients
            .note_connection_ended(&fixture.connection);
        let _id = fixture.handle.client_transients.begin(
            fixture.identity,
            fixture.condition,
            fixture.connection,
        );
        let Some(WirePayload::DeletionDemand(payload)) =
            fixture.handle.take_client_demand(&fixture.live).await
        else {
            panic!("the pending demand must be deliverable");
        };
        payload
    }

    async fn evidence_seq(fixture: &ClientFixture) -> Option<u64> {
        fixture
            .handle
            .store
            .client_delivery_evidence_seq(fixture.identity)
            .await
            .expect("the durable evidence must read")
    }

    async fn required_includes(handle: &HostHandle, identity: RawId) -> bool {
        handle
            .required_deletion_participants()
            .await
            .expect("the required snapshot must read")
            .contains(&ParticipantOwnerRef::ClientIncarnation(identity))
    }

    #[tokio::test]
    async fn a_restarted_host_names_a_delivered_incarnation_and_holds_it_unreachable() {
        let (handle, dir) = memory_handle("a3c-evidence-restart")
            .await
            .expect("the handle opens");
        let table = Arc::new(ConnectionTable::new());
        let connection = table.note_accept();
        authenticate(&table, &connection, DEVICE);
        assert!(table.pin_incarnation_for_tests(&connection, 71, 72));
        handle.install_client_connection_table(Arc::clone(&table));
        let live = table
            .snapshot(&connection)
            .expect("the connection snapshots");
        assert!(handle.note_client_body_delivery(&live).await);
        assert_eq!(
            handle
                .store
                .client_delivery_evidence_seq(ClientTransientRegistry::identity_for(71, 72))
                .await
                .expect("the durable evidence must read"),
            Some(1)
        );
        drop(handle);

        // A fresh process over the same directory has no in-memory delivery
        // tracking and no connection; the durable evidence still names the
        // incarnation at admission.
        let reopened = HostHandle::open_with_cred_store(
            dir.path(),
            CredStore::Memory(MemoryCredentialStore::new()),
        )
        .await
        .expect("the restarted handle must open");
        let identity = ClientTransientRegistry::identity_for(71, 72);
        let required = reopened
            .required_deletion_participants()
            .await
            .expect("the required snapshot must read");
        assert!(
            required.contains(&ParticipantOwnerRef::ClientIncarnation(identity)),
            "the restarted Host must still name the delivered incarnation: {required:?}"
        );
        let current = admit(&reopened, "secret body", required).await;
        reopened
            .drive_targeted_deletion(TargetedDeletionPass::default())
            .await
            .expect("the pass runs");
        assert_eq!(
            reopened
                .store
                .unfinished_deletions(None, 100)
                .await
                .expect("unfinished operations read")
                .len(),
            1,
            "the Client hold keeps the operation retryable-incomplete"
        );
        let row = reopened
            .store
            .deletion_participants(current.operation, None, 100)
            .await
            .expect("participant rows read")
            .into_iter()
            .find(|record| {
                record.participant.owner == ParticipantOwnerRef::ClientIncarnation(identity)
            })
            .expect("the durable snapshot names the restarted incarnation");
        assert_eq!(
            row.progress.hold_reason(),
            Some(ParticipantHoldClass::Unavailable),
            "the old incarnation resolves to an unreachable hold, never a composition default"
        );
    }

    #[tokio::test]
    async fn a_verified_full_class_wipe_clears_the_durable_evidence() {
        let fixture = client_fixture("a3c-evidence-clear").await;
        assert_eq!(evidence_seq(&fixture).await, Some(1));
        let payload = demand_on_wire(&fixture).await;
        fixture
            .handle
            .accept_client_erasure_result(&fixture.live, &full_wipe(&payload))
            .await;
        assert_eq!(
            evidence_seq(&fixture).await,
            None,
            "a verified full-class wipe supersedes the delivery evidence"
        );
        assert!(
            !required_includes(&fixture.handle, fixture.identity).await,
            "a cleared incarnation is no longer a required participant"
        );
    }

    #[tokio::test]
    async fn a_delivery_after_the_demand_snapshot_keeps_the_evidence() {
        let fixture = client_fixture("a3c-evidence-race").await;
        let payload = demand_on_wire(&fixture).await;
        // A body reaches the incarnation after the wipe went on the wire: the
        // durable sequence advances and the verified answer must not clear it.
        assert!(
            fixture
                .handle
                .note_client_body_delivery(&fixture.live)
                .await
        );
        fixture
            .handle
            .accept_client_erasure_result(&fixture.live, &full_wipe(&payload))
            .await;
        assert_eq!(
            evidence_seq(&fixture).await,
            Some(2),
            "evidence for a body delivered after the wipe survives the clear"
        );
        assert!(required_includes(&fixture.handle, fixture.identity).await);

        // A later demand observes the new sequence and its verified answer
        // clears exactly that evidence.
        let payload = demand_on_wire(&fixture).await;
        fixture
            .handle
            .accept_client_erasure_result(&fixture.live, &full_wipe(&payload))
            .await;
        assert_eq!(evidence_seq(&fixture).await, None);
    }

    #[tokio::test]
    async fn a_partial_or_unverified_wipe_never_clears_the_evidence() {
        let fixture = client_fixture("a3c-evidence-partial").await;
        let payload = demand_on_wire(&fixture).await;
        // A full-class verified report is required; a narrower class list
        // leaves the evidence even though the completion fact is verified.
        fixture
            .handle
            .accept_client_erasure_result(
                &fixture.live,
                &wipe_result(&payload, vec![ClientTempClass::InputDraft], Vec::new()),
            )
            .await;
        assert_eq!(
            evidence_seq(&fixture).await,
            Some(1),
            "a partial class list never clears the evidence"
        );
        let payload = demand_on_wire(&fixture).await;
        fixture
            .handle
            .accept_client_erasure_result(
                &fixture.live,
                &wipe_result(
                    &payload,
                    vec![ClientTempClass::InputDraft],
                    vec![ClientTempClass::PresentationBuffer],
                ),
            )
            .await;
        assert_eq!(
            evidence_seq(&fixture).await,
            Some(1),
            "an unverified remainder never clears the evidence"
        );
        assert!(required_includes(&fixture.handle, fixture.identity).await);
    }

    #[tokio::test]
    async fn a_disconnect_or_replacement_never_clears_the_evidence() {
        let fixture = client_fixture("a3c-evidence-disconnect").await;
        let _payload = demand_on_wire(&fixture).await;
        // A replacement connection for the same device supersedes the demanded
        // connection; the pending demand is abandoned and the incarnation is
        // unreachable until it answers on a current connection.
        let replacement = fixture.table.note_accept();
        authenticate(&fixture.table, &replacement, DEVICE);
        fixture.handle.on_connection_closed(&fixture.connection);
        assert_eq!(
            evidence_seq(&fixture).await,
            Some(1),
            "disconnect and replacement alone never clear the delivery evidence"
        );
        let outcome = fixture
            .handle
            .drive_targeted_deletion(TargetedDeletionPass::default())
            .await
            .expect("the pass runs");
        assert_eq!(outcome.verified, 0);
        assert_eq!(
            evidence_seq(&fixture).await,
            Some(1),
            "a hold leaves the evidence in place for a later reachable demand"
        );
    }

    #[tokio::test]
    async fn a_new_incarnation_never_inherits_another_incarnations_evidence() {
        let (handle, _dir) = memory_handle("a3c-evidence-incarnation")
            .await
            .expect("the handle opens");
        let table = Arc::new(ConnectionTable::new());
        let first_connection = table.note_accept();
        authenticate(&table, &first_connection, DEVICE);
        assert!(table.pin_incarnation_for_tests(&first_connection, 81, 82));
        let second_connection = table.note_accept();
        authenticate(&table, &second_connection, DEVICE);
        assert!(table.pin_incarnation_for_tests(&second_connection, 81, 83));
        handle.install_client_connection_table(Arc::clone(&table));
        let first = ClientTransientRegistry::identity_for(81, 82);
        let second = ClientTransientRegistry::identity_for(81, 83);
        for connection in [&first_connection, &second_connection] {
            let live = table
                .snapshot(connection)
                .expect("the connection snapshots");
            assert!(handle.note_client_body_delivery(&live).await);
        }
        assert!(required_includes(&handle, first).await);
        assert!(required_includes(&handle, second).await);
        assert!(
            handle
                .store
                .clear_client_delivery_evidence(first, 1)
                .await
                .expect("the clear must commit"),
            "the first incarnation's exact sequence clears"
        );
        assert!(!required_includes(&handle, first).await);
        assert!(
            required_includes(&handle, second).await,
            "a different boot incarnation never inherits another's evidence"
        );
        assert_eq!(
            handle
                .store
                .client_delivery_evidence_seq(second)
                .await
                .expect("the durable evidence must read"),
            Some(1)
        );
    }

    #[tokio::test]
    async fn the_host_transient_demand_drops_covered_premises_and_moves_the_fence() {
        let (handle, _dir) = memory_handle("a3c-host-transient")
            .await
            .expect("the handle opens");
        crate::lock_unpoison(&handle.learning_queue).push_back(experience("contains secret body"));
        crate::lock_unpoison(&handle.learning_queue).push_back(experience("unrelated text"));
        let epoch = handle.transient_fence_epoch();
        let participant = HostTransientParticipant::new(
            handle.store.clone(),
            Arc::clone(&handle.transient_fence),
            Arc::clone(&handle.presentations),
            Arc::clone(&handle.learning_queue),
            Arc::clone(&handle.host_transient_arrival),
        );
        let current = admit(
            &handle,
            "secret body",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        let fact = participant
            .demand_local_erasure(command(
                current.condition(),
                ParticipantOwnerRef::HostTransient,
                ParticipantErasureScope::local(target("secret body")),
            ))
            .await;
        assert_eq!(fact.status(), ParticipantCompletionStatus::Verified);
        assert!(fact.erased_count() >= 1);
        let queue = crate::lock_unpoison(&handle.learning_queue);
        assert_eq!(queue.len(), 1, "only the covered premise drops");
        assert!(
            queue
                .front()
                .expect("one uncovered premise remains")
                .transcript[0]
                .text
                .contains("unrelated text")
        );
        drop(queue);
        assert!(
            handle.transient_fence_epoch() > epoch,
            "in-flight streams and assembled replies fail closed after the demand"
        );
    }

    /// Lifecycle §9: one HostTransient demand examines a bounded Learning-queue
    /// page and continues via MoreWork until every TARGET-bearing entry is
    /// gone. Unrelated premises remain.
    #[tokio::test]
    async fn the_host_transient_learning_queue_demand_is_bounded_and_continues() {
        let (handle, _dir) = memory_handle("a3c-host-transient-bounded")
            .await
            .expect("the handle opens");
        let page = super::HOST_TRANSIENT_LEARNING_QUEUE_PAGE;
        for _ in 0..(page + 5) {
            crate::lock_unpoison(&handle.learning_queue)
                .push_back(experience("contains secret body"));
        }
        crate::lock_unpoison(&handle.learning_queue).push_back(experience("unrelated text"));
        crate::lock_unpoison(&handle.learning_queue).push_back(experience("also unrelated"));
        let queued_before = crate::lock_unpoison(&handle.learning_queue).len();
        let participant = host_transient(&handle);
        let current = admit(
            &handle,
            "secret body",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        let first = demand_secret(&participant, current.condition()).await;
        assert_eq!(first.status(), ParticipantCompletionStatus::MoreWork);
        assert!(
            participant.last_scanned() <= page,
            "one demand must not scan more than the page"
        );
        let remaining_after_first = crate::lock_unpoison(&handle.learning_queue).len();
        let dropped_from_queue = queued_before - remaining_after_first;
        assert!(
            dropped_from_queue <= page,
            "one demand must not drop more than the page: dropped {dropped_from_queue}"
        );
        assert!(
            remaining_after_first > 2,
            "the first page must leave continuation work"
        );

        let max_demands = page_demand_bound(queued_before);
        let mut last = first;
        let mut demands = 1usize;
        while last.status() == ParticipantCompletionStatus::MoreWork {
            assert!(
                demands < max_demands,
                "a stable queue of {queued_before} must verify within {max_demands} demands"
            );
            last = demand_secret(&participant, current.condition()).await;
            assert!(
                participant.last_scanned() <= page,
                "one demand must not scan more than the page"
            );
            demands += 1;
        }
        assert_eq!(last.status(), ParticipantCompletionStatus::Verified);
        let queue = crate::lock_unpoison(&handle.learning_queue);
        assert_eq!(queue.len(), 2, "unrelated premises remain");
        assert!(
            queue
                .iter()
                .all(|item| item.transcript[0].text.contains("unrelated")),
            "TARGET-bearing queue entries must all be gone"
        );
        assert!(
            queue
                .iter()
                .all(|item| !item.transcript[0].text.contains("secret body")),
            "no remaining entry may carry the target"
        );
    }

    /// A second operation's demand must not reset the first's continuation
    /// cursor: with a queue longer than one pass's demand budget, interleaved
    /// demands for two live operations must each keep their own examined set
    /// and both reach Verified.
    #[tokio::test]
    async fn a_second_operation_does_not_reset_the_first_sweep_cursor() {
        let (handle, _dir) = memory_handle("a3c-host-transient-sweep-map")
            .await
            .expect("the handle opens");
        let page = super::HOST_TRANSIENT_LEARNING_QUEUE_PAGE;
        // One more than a whole pass's demand budget (`demands_per_participant
        // * PAGE`), so a single-slot cursor is overwritten before it empties.
        let queued = 4 * page + 1;
        for _ in 0..queued {
            crate::lock_unpoison(&handle.learning_queue).push_back(experience("unrelated text"));
        }
        let participant = host_transient(&handle);
        let a = admit(
            &handle,
            "secret-a",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        let b = admit(
            &handle,
            "secret-b",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        let mut verified_a = false;
        let mut verified_b = false;
        let mut demands = 0usize;
        while !(verified_a && verified_b) {
            demands += 1;
            assert!(
                demands < 64,
                "both operations must settle within a bounded number of demands"
            );
            if !verified_a {
                verified_a = participant
                    .demand_local_erasure(command(
                        a.condition(),
                        ParticipantOwnerRef::HostTransient,
                        ParticipantErasureScope::local(target("secret-a")),
                    ))
                    .await
                    .status()
                    == ParticipantCompletionStatus::Verified;
            }
            if !verified_b {
                verified_b = participant
                    .demand_local_erasure(command(
                        b.condition(),
                        ParticipantOwnerRef::HostTransient,
                        ParticipantErasureScope::local(target("secret-b")),
                    ))
                    .await
                    .status()
                    == ParticipantCompletionStatus::Verified;
            }
        }
        assert_eq!(
            crate::lock_unpoison(&handle.learning_queue).len(),
            queued,
            "unrelated premises are never dropped"
        );
    }

    /// Two interleaved operations share one deque, so each demand removes the
    /// covered entries and shifts the shared deque past the other's positional
    /// window. Progress keyed by the
    /// candidate's source range must still examine every entry: A may not
    /// report Verified while its TARGET is queued ahead of a re-examined slot.
    #[tokio::test]
    async fn an_interleaved_demand_does_not_skip_an_unexamined_target() {
        let (handle, _dir) = memory_handle("a3c-host-transient-interleaved-target")
            .await
            .expect("the handle opens");
        let page = super::HOST_TRANSIENT_LEARNING_QUEUE_PAGE;
        // Not an exact multiple of the demand budget, so a positional count
        // wraps around and re-examines slots instead of covering the queue.
        let queued = 4 * page + 1;
        let target_index = 3 * page + 4;
        for i in 0..queued {
            let text = if i == target_index {
                String::from("contains secret-a")
            } else {
                format!("unrelated-{i}")
            };
            crate::lock_unpoison(&handle.learning_queue).push_back(experience(&text));
        }
        let participant = host_transient(&handle);
        let a = admit(
            &handle,
            "secret-a",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        let b = admit(
            &handle,
            "secret-b",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        let mut verified_a = false;
        let mut verified_b = false;
        let mut demands = 0usize;
        while !(verified_a && verified_b) {
            demands += 1;
            assert!(
                demands < 64,
                "both operations must settle within a bounded number of demands"
            );
            if !verified_a {
                verified_a = participant
                    .demand_local_erasure(command(
                        a.condition(),
                        ParticipantOwnerRef::HostTransient,
                        ParticipantErasureScope::local(target("secret-a")),
                    ))
                    .await
                    .status()
                    == ParticipantCompletionStatus::Verified;
                if verified_a {
                    assert!(
                        pending_transcripts(&handle)
                            .iter()
                            .all(|text| !text.contains("secret-a")),
                        "A must not verify while its TARGET is still queued (demand {demands})"
                    );
                }
            }
            if !verified_b {
                verified_b = participant
                    .demand_local_erasure(command(
                        b.condition(),
                        ParticipantOwnerRef::HostTransient,
                        ParticipantErasureScope::local(target("secret-b")),
                    ))
                    .await
                    .status()
                    == ParticipantCompletionStatus::Verified;
            }
        }
        assert_eq!(
            crate::lock_unpoison(&handle.learning_queue).len(),
            queued - 1,
            "only the covered TARGET drops"
        );
        assert!(
            pending_transcripts(&handle)
                .iter()
                .all(|text| !text.contains("secret-a"))
        );
    }

    /// A candidate taken off the pending queue stays visible to HostTransient
    /// until its formation identity is published, so an empty pending queue is
    /// not a Verified completion while the worker still holds the body.
    #[tokio::test]
    async fn a_taken_learning_candidate_keeps_the_host_transient_demand_unfinished() {
        let (handle, _dir) = memory_handle("a3c-host-transient-taken")
            .await
            .expect("the handle opens");
        {
            let mut queue = crate::lock_unpoison(&handle.learning_queue);
            queue.push_back(experience("contains secret body"));
            assert!(queue.take_pending().is_some());
            assert!(queue.is_empty(), "the pending queue is empty after take");
            assert!(
                queue.taken().is_some(),
                "the worker-owned slot still holds the body"
            );
        }
        let participant = HostTransientParticipant::new(
            handle.store.clone(),
            Arc::clone(&handle.transient_fence),
            Arc::clone(&handle.presentations),
            Arc::clone(&handle.learning_queue),
            Arc::clone(&handle.host_transient_arrival),
        );
        let current = admit(
            &handle,
            "secret body",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        let fact = participant
            .demand_local_erasure(command(
                current.condition(),
                ParticipantOwnerRef::HostTransient,
                ParticipantErasureScope::local(target("secret body")),
            ))
            .await;
        assert_eq!(fact.status(), ParticipantCompletionStatus::MoreWork);
        let queue = crate::lock_unpoison(&handle.learning_queue);
        assert!(
            queue
                .taken()
                .is_some_and(|item| item.transcript[0].text.contains("secret body")),
            "HostTransient must not drop the worker-owned taken slot"
        );
        assert!(queue.is_empty());
    }

    /// P1: a TARGET-bearing candidate that moves pending→taken after the
    /// queue snapshot cannot verify HostTransient from that stale page.
    #[tokio::test]
    async fn a_snapshot_then_take_cannot_verify_host_transient() {
        use ene_inference::fake::FakeProviderTransport;

        let (handle, dir) = memory_handle("a3c-host-transient-snapshot-take")
            .await
            .expect("the handle opens");
        crate::lock_unpoison(&handle.learning_queue).push_back(experience("contains secret body"));
        let handle = Arc::new(handle);
        let current = admit(
            &handle,
            "secret body",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        handle.store.arm_host_transient_queue_park_for_tests();
        handle.store.arm_learning_take_park_for_tests();
        handle.store.arm_learning_formation_park_for_tests();
        let parked = {
            let participant = host_transient(&handle);
            let condition = current.condition();
            tokio::spawn(async move { demand_secret(&participant, condition).await })
        };
        handle
            .store
            .wait_host_transient_queue_park_for_tests()
            .await;
        {
            let queue = crate::lock_unpoison(&handle.learning_queue);
            assert_eq!(queue.len(), 1, "the snapshot still saw the pending TARGET");
            assert!(queue.taken().is_none());
        }
        let worker_handle = Arc::clone(&handle);
        let worker = tokio::spawn(async move {
            worker_handle
                .run_pending_learning(&FakeProviderTransport::new(String::from("noted"), None))
                .await;
        });
        handle.store.wait_learning_take_park_for_tests().await;
        {
            let queue = crate::lock_unpoison(&handle.learning_queue);
            assert!(queue.is_empty(), "the worker emptied pending");
            assert!(
                queue
                    .taken()
                    .is_some_and(|item| item.transcript[0].text.contains("secret body")),
                "the TARGET is in taken with no formation identity yet"
            );
        }
        handle.store.release_host_transient_queue_park_for_tests();
        let fact = parked.await.expect("the parked demand joins");
        assert_eq!(
            fact.status(),
            ParticipantCompletionStatus::MoreWork,
            "a stale snapshot must not verify while taken still holds the TARGET"
        );

        handle
            .drive_targeted_deletion(TargetedDeletionPass::new(100, 1))
            .await
            .expect("the fan-out must not fail");
        let progress = handle
            .store
            .deletion_participants(current.operation, None, 100)
            .await
            .expect("participant rows read")
            .into_iter()
            .find(|record| record.participant.owner == ParticipantOwnerRef::HostTransient)
            .expect("the HostTransient row exists")
            .progress;
        assert!(
            !progress.is_verified(),
            "deletion_participant must not be Verified while taken holds the TARGET, got {progress:?}"
        );
        let phase = handle
            .store
            .deletion_status(None, 100)
            .await
            .expect("the status must read")
            .into_iter()
            .find(|record| record.current.operation == current.operation)
            .expect("the operation must stay readable")
            .phase;
        assert_ne!(
            phase,
            DeletionOperationPhase::Completed,
            "global completion is forbidden while TARGET-bearing taken remains"
        );

        handle.store.release_learning_take_park_for_tests();
        handle.store.wait_learning_formation_park_for_tests().await;
        assert!(
            crate::lock_unpoison(&handle.learning_queue)
                .taken()
                .is_none(),
            "formation publish clears taken"
        );
        let db = dir.path().join("app.db");
        let formations: i64 = {
            let conn = rusqlite::Connection::open(&db).expect("the state database opens");
            conn.query_row("SELECT COUNT(*) FROM learning_formation", [], |row| {
                row.get(0)
            })
            .expect("the formation count must read")
        };
        assert_eq!(
            formations, 1,
            "the worker publishes a body-free formation identity"
        );
        handle.store.release_learning_formation_park_for_tests();
        worker.await.expect("the parked worker joins");

        let mut settled = false;
        for _ in 0..page_demand_bound(1) + 2 {
            let outcome = handle
                .drive_targeted_deletion(TargetedDeletionPass::new(100, 16))
                .await
                .expect("the fan-out must not fail");
            let phase = handle
                .store
                .deletion_status(None, 100)
                .await
                .expect("the status must read")
                .into_iter()
                .find(|record| record.current.operation == current.operation)
                .expect("the operation must stay readable")
                .phase;
            if phase == DeletionOperationPhase::Completed
                && outcome.demands == 0
                && outcome.reconciliation_pages == 0
            {
                settled = true;
                break;
            }
        }
        assert!(
            settled,
            "Targeted Deletion must converge after formation publish"
        );
        let phase = handle
            .store
            .deletion_status(None, 100)
            .await
            .expect("the status must read")
            .into_iter()
            .find(|record| record.current.operation == current.operation)
            .expect("the operation must stay readable")
            .phase;
        assert_eq!(phase, DeletionOperationPhase::Completed);
    }

    /// Unrelated-only queues longer than one page must still Verify in a
    /// fixture-computable number of page-sized demands without dropping.
    #[tokio::test]
    async fn unrelated_learning_candidates_beyond_one_page_verify_in_finite_demands() {
        let (handle, _dir) = memory_handle("a3c-host-transient-unrelated-only")
            .await
            .expect("the handle opens");
        let page = super::HOST_TRANSIENT_LEARNING_QUEUE_PAGE;
        let queued = page * 3 + 5;
        for i in 0..queued {
            crate::lock_unpoison(&handle.learning_queue)
                .push_back(experience(&format!("unrelated-{i}")));
        }
        let before = pending_transcripts(&handle);
        let participant = host_transient(&handle);
        let current = admit(
            &handle,
            "secret body",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        let max_demands = page_demand_bound(queued);
        let mut last = demand_secret(&participant, current.condition()).await;
        let mut demands = 1usize;
        assert!(participant.last_scanned() <= page);
        assert_eq!(
            sorted_texts(pending_transcripts(&handle)),
            sorted_texts(before.clone()),
            "unrelated candidates must not be dropped"
        );
        while last.status() == ParticipantCompletionStatus::MoreWork {
            assert!(
                demands < max_demands,
                "{queued} unrelated entries must verify within {max_demands} demands"
            );
            last = demand_secret(&participant, current.condition()).await;
            assert!(participant.last_scanned() <= page);
            assert_eq!(
                sorted_texts(pending_transcripts(&handle)),
                sorted_texts(before.clone()),
                "unrelated candidates must not be dropped"
            );
            demands += 1;
        }
        assert_eq!(last.status(), ParticipantCompletionStatus::Verified);
        assert_eq!(demands, max_demands);
        assert_eq!(crate::lock_unpoison(&handle.learning_queue).len(), queued);
        assert!(
            crate::lock_unpoison(&handle.learning_queue)
                .taken()
                .is_none()
        );
    }

    /// A mixed queue drops only the TARGET-bearing candidate and still
    /// Verifies in a fixture-computable number of page-sized demands.
    #[tokio::test]
    async fn a_mixed_target_and_unrelated_queue_drops_only_the_target() {
        let (handle, _dir) = memory_handle("a3c-host-transient-mixed")
            .await
            .expect("the handle opens");
        let page = super::HOST_TRANSIENT_LEARNING_QUEUE_PAGE;
        let leading = page + 5;
        let trailing = 5;
        for i in 0..leading {
            crate::lock_unpoison(&handle.learning_queue)
                .push_back(experience(&format!("unrelated-lead-{i}")));
        }
        crate::lock_unpoison(&handle.learning_queue).push_back(experience("contains secret body"));
        for i in 0..trailing {
            crate::lock_unpoison(&handle.learning_queue)
                .push_back(experience(&format!("unrelated-trail-{i}")));
        }
        let unrelated_before: Vec<String> = pending_transcripts(&handle)
            .into_iter()
            .filter(|text| !text.contains("secret body"))
            .collect();
        let queued = crate::lock_unpoison(&handle.learning_queue).len();
        let participant = host_transient(&handle);
        let current = admit(
            &handle,
            "secret body",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        let max_demands = page_demand_bound(queued);
        let mut last = demand_secret(&participant, current.condition()).await;
        let mut demands = 1usize;
        assert!(participant.last_scanned() <= page);
        while last.status() == ParticipantCompletionStatus::MoreWork {
            assert!(
                demands < max_demands,
                "a mixed queue of {queued} must verify within {max_demands} demands"
            );
            last = demand_secret(&participant, current.condition()).await;
            assert!(participant.last_scanned() <= page);
            demands += 1;
        }
        assert_eq!(last.status(), ParticipantCompletionStatus::Verified);
        let remaining = pending_transcripts(&handle);
        assert_eq!(
            sorted_texts(remaining.clone()),
            sorted_texts(unrelated_before)
        );
        assert_eq!(remaining.len(), leading + trailing);
        assert!(remaining.iter().all(|text| !text.contains("secret body")));
        assert!(
            crate::lock_unpoison(&handle.learning_queue)
                .taken()
                .is_none()
        );
    }

    /// An enqueue after the page snapshot invalidates that snapshot: the
    /// demand must not Verified, and a later pass still erases the new TARGET.
    #[tokio::test]
    async fn an_enqueue_during_host_transient_scan_invalidates_the_snapshot() {
        let (handle, _dir) = memory_handle("a3c-host-transient-enqueue-during-scan")
            .await
            .expect("the handle opens");
        crate::lock_unpoison(&handle.learning_queue).push_back(experience("unrelated-stable"));
        let handle = Arc::new(handle);
        let participant = host_transient(&handle);
        let current = admit(
            &handle,
            "secret body",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        handle.store.arm_host_transient_queue_park_for_tests();
        let parked = {
            let condition = current.condition();
            tokio::spawn(async move { demand_secret(&participant, condition).await })
        };
        handle
            .store
            .wait_host_transient_queue_park_for_tests()
            .await;
        crate::lock_unpoison(&handle.learning_queue).push_back(experience("contains secret body"));
        handle.store.release_host_transient_queue_park_for_tests();
        let first = parked.await.expect("the parked demand joins");
        assert_eq!(
            first.status(),
            ParticipantCompletionStatus::MoreWork,
            "a concurrent enqueue must invalidate the snapshot"
        );
        assert_eq!(
            crate::lock_unpoison(&handle.learning_queue).len(),
            2,
            "the stale snapshot must not drop either candidate"
        );

        let participant = host_transient(&handle);
        let max_demands = page_demand_bound(2);
        let mut last = first;
        let mut demands = 1usize;
        while last.status() == ParticipantCompletionStatus::MoreWork {
            assert!(
                demands < max_demands + 1,
                "the new TARGET must be examined within a rebased cycle"
            );
            last = demand_secret(&participant, current.condition()).await;
            assert!(participant.last_scanned() <= super::HOST_TRANSIENT_LEARNING_QUEUE_PAGE);
            demands += 1;
        }
        assert_eq!(last.status(), ParticipantCompletionStatus::Verified);
        let remaining = pending_transcripts(&handle);
        assert_eq!(remaining, vec![String::from("unrelated-stable")]);
        assert!(remaining.iter().all(|text| !text.contains("secret body")));
    }

    /// Overflow drops the oldest pending body and advances generation, so the
    /// sweep cursor cannot treat the dropped entry as scanned-clean.
    #[tokio::test]
    async fn queue_overflow_is_not_treated_as_a_scanned_clean_candidate() {
        let (handle, _dir) = memory_handle("a3c-host-transient-overflow")
            .await
            .expect("the handle opens");
        let page = super::HOST_TRANSIENT_LEARNING_QUEUE_PAGE;
        let cap = super::LEARNING_FORMATION_QUEUE_CAP;
        for i in 0..cap {
            crate::lock_unpoison(&handle.learning_queue)
                .push_back(experience(&format!("unrelated-cap-{i}")));
        }
        let participant = host_transient(&handle);
        let current = admit(
            &handle,
            "secret body",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        let first = demand_secret(&participant, current.condition()).await;
        assert_eq!(first.status(), ParticipantCompletionStatus::MoreWork);
        assert_eq!(participant.last_scanned(), page);
        let generation_after_first =
            crate::lock_unpoison(&handle.learning_queue).mutation_generation();
        crate::lock_unpoison(&handle.learning_queue).push_back(experience("contains secret body"));
        assert_ne!(
            crate::lock_unpoison(&handle.learning_queue).mutation_generation(),
            generation_after_first,
            "overflow enqueue must advance the queue generation"
        );
        assert_eq!(crate::lock_unpoison(&handle.learning_queue).len(), cap);

        let max_demands = 1 + page_demand_bound(cap);
        let mut last = first;
        let mut demands = 1usize;
        while last.status() == ParticipantCompletionStatus::MoreWork {
            assert!(
                demands < max_demands,
                "overflow must rebase the sweep and still verify within {max_demands} demands"
            );
            last = demand_secret(&participant, current.condition()).await;
            assert!(participant.last_scanned() <= page);
            demands += 1;
        }
        assert_eq!(last.status(), ParticipantCompletionStatus::Verified);
        let remaining = pending_transcripts(&handle);
        assert_eq!(
            remaining.len(),
            cap - 1,
            "overflow dropped one unrelated body"
        );
        assert!(remaining.iter().all(|text| !text.contains("secret body")));
        assert!(
            remaining
                .iter()
                .all(|text| text.starts_with("unrelated-cap-"))
        );
    }

    /// P1: a Verified HostTransient fact cannot become durable after a
    /// TARGET-bearing old-origin candidate arrives in the record window.
    #[tokio::test]
    async fn a_verified_fact_cannot_commit_after_a_host_transient_arrival() {
        let (handle, _dir) = memory_handle("a3c-verified-record-arrival")
            .await
            .expect("the handle opens");
        let handle = Arc::new(handle);
        let current = admit(
            &handle,
            "secret body",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        handle
            .store
            .arm_host_transient_verified_record_park_for_tests();
        let parked = {
            let handle = Arc::clone(&handle);
            tokio::spawn(async move {
                handle
                    .drive_targeted_deletion(TargetedDeletionPass::new(100, 1))
                    .await
            })
        };
        handle
            .store
            .wait_host_transient_verified_record_park_for_tests()
            .await;
        assert!(
            crate::lock_unpoison(&handle.learning_queue).is_empty(),
            "the Verified fact examined an empty queue"
        );
        handle
            .queue_learning_formation(experience("contains secret body"))
            .await;
        assert_eq!(crate::lock_unpoison(&handle.learning_queue).len(), 1);
        handle
            .store
            .release_host_transient_verified_record_park_for_tests();
        parked
            .await
            .expect("the parked drive joins")
            .expect("the parked drive must not fail");
        assert_ne!(
            deletion_phase(&handle, current.operation).await,
            DeletionOperationPhase::Completed,
            "stale G must not complete over a G+1 arrival"
        );
        assert!(
            !host_transient_is_verified(&handle, current.operation).await,
            "the durable HostTransient row must not keep the stale Verified fact"
        );
        assert!(
            crate::lock_unpoison(&handle.learning_queue)
                .iter()
                .any(|item| item.transcript[0].text.contains("secret body")),
            "the arrival must still be present for the next HostTransient drive"
        );
        drive_until_completed(&handle, current.operation).await;
        assert!(
            crate::lock_unpoison(&handle.learning_queue)
                .iter()
                .all(|item| !item.transcript[0].text.contains("secret body")),
            "the arrival must be collected before completion"
        );
    }

    /// P1: a durable HostTransient Verified row cannot finalize over a
    /// TARGET-bearing arrival that lands before the sealed completion boundary.
    #[tokio::test]
    async fn a_durable_verified_host_transient_cannot_finalize_over_a_new_arrival() {
        let (handle, _dir) = memory_handle("a3c-verified-finalizing-arrival")
            .await
            .expect("the handle opens");
        let handle = Arc::new(handle);
        let current = admit(
            &handle,
            "secret body",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        handle.store.arm_deletion_finalizing_park_for_tests();
        let parked = {
            let handle = Arc::clone(&handle);
            tokio::spawn(async move {
                handle
                    .drive_targeted_deletion(TargetedDeletionPass::new(100, 16))
                    .await
            })
        };
        handle.store.wait_deletion_finalizing_park_for_tests().await;
        assert!(
            host_transient_is_verified(&handle, current.operation).await,
            "HostTransient must already be durably Verified before finalizing"
        );
        handle
            .queue_learning_formation(experience("contains secret body"))
            .await;
        handle.store.release_deletion_finalizing_park_for_tests();
        parked
            .await
            .expect("the parked drive joins")
            .expect("the parked drive must not fail");
        assert_ne!(
            deletion_phase(&handle, current.operation).await,
            DeletionOperationPhase::Completed,
            "finalizing must not complete over a new HostTransient remainder"
        );
        drive_until_completed(&handle, current.operation).await;
        assert!(
            crate::lock_unpoison(&handle.learning_queue)
                .iter()
                .all(|item| !item.transcript[0].text.contains("secret body")),
            "the arrival must be collected before completion"
        );
    }

    /// P1: a TARGET-bearing enqueue whose canonical delayed-arrival publish
    /// fails must not Complete. Without unpublished bookkeeping, inflight_pins
    /// is 0 after the producer returns and HostTransient is still durably
    /// Verified, so settle_finalizing would Complete over the live queue.
    #[tokio::test]
    async fn a_failed_arrival_publish_cannot_complete_over_a_queued_target() {
        let (handle, _dir) = memory_handle("a3c-arrival-publish-failure")
            .await
            .expect("the handle opens");
        let handle = Arc::new(handle);
        let current = admit(
            &handle,
            "secret body",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        verify_host_transient(&handle, current).await;
        assert!(
            host_transient_is_verified(&handle, current.operation).await,
            "HostTransient must be durably Verified before the arrival"
        );
        let attempts_before = handle.store.host_transient_arrival_attempts_for_tests();
        handle
            .store
            .fail_host_transient_arrivals_until_allow_for_tests();
        handle
            .store
            .arm_host_transient_arrival_publish_park_for_tests();
        let parked = {
            let handle = Arc::clone(&handle);
            tokio::spawn(async move {
                handle
                    .queue_learning_formation(experience("contains secret body"))
                    .await;
            })
        };
        handle
            .store
            .wait_host_transient_arrival_publish_park_for_tests()
            .await;
        assert!(
            crate::lock_unpoison(&handle.learning_queue)
                .iter()
                .any(|item| item.transcript[0].text.contains("secret body")),
            "the candidate is on the queue before publication"
        );
        assert!(
            handle.host_transient_arrival.has_unpublished(),
            "accepting the body-bearing remainder must owe canonical publication"
        );
        handle
            .store
            .release_host_transient_arrival_publish_park_for_tests();
        parked.await.expect("the parked enqueue joins");
        assert_eq!(
            handle.host_transient_arrival.inflight_pins(),
            0,
            "producer occupancy has ended"
        );
        assert!(
            handle.host_transient_arrival.has_unpublished(),
            "a failed canonical publish leaves unpublished bookkeeping"
        );
        assert_ne!(
            crate::lock_unpoison(&handle.learning_queue).mutation_generation(),
            handle.host_transient_arrival.verified_generation(),
            "the live queue generation must not still match the durable Verified generation"
        );
        assert!(
            crate::lock_unpoison(&handle.learning_queue)
                .iter()
                .any(|item| item.transcript[0].text.contains("secret body")),
            "the TARGET candidate remains; that remainder is not clean"
        );
        handle
            .drive_targeted_deletion(TargetedDeletionPass::new(100, 1))
            .await
            .expect("finalizing while publication is still failing must not error");
        assert_ne!(
            deletion_phase(&handle, current.operation).await,
            DeletionOperationPhase::Completed,
            "failed publication must not Complete"
        );
        assert!(
            host_transient_is_verified(&handle, current.operation).await,
            "the durable Verified row stays until a successful publish"
        );
        assert!(
            handle.host_transient_arrival.has_unpublished(),
            "retry that still fails must keep unpublished bookkeeping"
        );
        handle.store.allow_host_transient_arrival_for_tests();
        handle
            .drive_targeted_deletion(TargetedDeletionPass::new(100, 1))
            .await
            .expect("the retry pass runs");
        assert!(
            handle.store.host_transient_arrival_attempts_for_tests() > attempts_before + 1,
            "canonical publication must be retried after the failure"
        );
        let queued_target = crate::lock_unpoison(&handle.learning_queue)
            .iter()
            .any(|item| item.transcript[0].text.contains("secret body"));
        assert!(
            deletion_phase(&handle, current.operation).await != DeletionOperationPhase::Completed
                || !queued_target,
            "a successful publish must not Complete over a still-queued TARGET"
        );
        drive_until_completed(&handle, current.operation).await;
        assert!(
            !handle.host_transient_arrival.has_unpublished(),
            "successful publish clears unpublished bookkeeping"
        );
        assert!(
            crate::lock_unpoison(&handle.learning_queue)
                .iter()
                .all(|item| !item.transcript[0].text.contains("secret body")),
            "the arrival must be collected before completion"
        );
    }

    /// Major: a TARGET_A Learning arrival must not reset an unrelated
    /// TARGET_B operation.
    #[tokio::test]
    async fn a_learning_arrival_invalidates_only_related_operations() {
        let (handle, _dir) = memory_handle("a3c-arrival-opspec-ab")
            .await
            .expect("the handle opens");
        let a = admit(
            &handle,
            "secret-a",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        let b = admit(
            &handle,
            "secret-b",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        verify_host_transient(&handle, a).await;
        verify_host_transient(&handle, b).await;
        let sweep_b = b.sweep;
        handle
            .queue_learning_formation(experience("contains secret-a only"))
            .await;
        assert_ne!(
            operation_record(&handle, a.operation).await.current.sweep,
            a.sweep,
            "A must open the next sweep"
        );
        assert!(
            !host_transient_is_verified(&handle, a.operation).await,
            "A HostTransient verification must reset"
        );
        let record_b = operation_record(&handle, b.operation).await;
        assert_eq!(record_b.current.sweep, sweep_b);
        assert_eq!(record_b.phase, DeletionOperationPhase::Active);
        assert!(
            host_transient_is_verified(&handle, b.operation).await,
            "B HostTransient verification must stay"
        );

        handle
            .queue_learning_formation(experience("contains secret-b only"))
            .await;
        assert!(
            !host_transient_is_verified(&handle, b.operation).await,
            "the reverse arrival must reset only B"
        );
        assert_ne!(
            operation_record(&handle, b.operation).await.current.sweep,
            sweep_b
        );
    }

    /// Major: one producer call examines a bounded unfinished-ops page, then
    /// continuation invalidates only the related operation.
    #[tokio::test]
    async fn a_learning_arrival_pages_unfinished_operations() {
        let (handle, _dir) = memory_handle("a3c-arrival-opspec-page")
            .await
            .expect("the handle opens");
        for extras in 1..=HOST_TRANSIENT_ARRIVAL_PAGE {
            let extra = admit(
                &handle,
                &format!("unrelated-page-{extras}"),
                vec![ParticipantOwnerRef::HostTransient],
            )
            .await;
            verify_host_transient(&handle, extra).await;
        }
        let related = admit(
            &handle,
            "secret-related",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        verify_host_transient(&handle, related).await;
        let mut unrelated = Vec::new();
        let mut after = None;
        loop {
            let page = handle
                .store
                .unfinished_deletions(after, 100)
                .await
                .expect("unfinished operations read");
            if page.is_empty() {
                break;
            }
            let page_len = page.len();
            after = Some(page[page_len - 1].current.operation);
            unrelated.extend(page.into_iter().filter_map(|record| {
                (record.current.operation != related.operation).then_some((
                    record.current.operation,
                    record.current.sweep,
                    record.phase,
                ))
            }));
            if page_len < 100 {
                break;
            }
        }
        assert!(
            unrelated.len() as u32 >= HOST_TRANSIENT_ARRIVAL_PAGE,
            "unfinished operations must fill at least one producer page"
        );
        let first = handle
            .store
            .unfinished_deletions(None, HOST_TRANSIENT_ARRIVAL_PAGE)
            .await
            .expect("the first page reads");
        let related_on_first = first
            .iter()
            .any(|record| record.current.operation == related.operation);
        handle
            .queue_learning_formation(experience("contains secret-related"))
            .await;
        assert!(
            handle.host_transient_arrival.last_classified() <= HOST_TRANSIENT_ARRIVAL_PAGE as usize,
            "one producer call must not scan every unfinished operation"
        );
        if related_on_first {
            assert!(
                !host_transient_is_verified(&handle, related.operation).await,
                "a related operation on the first page is invalidated by that page"
            );
        } else {
            assert!(
                host_transient_is_verified(&handle, related.operation).await,
                "the related operation stays Verified until a later page reaches it"
            );
        }
        let mut passes = 0u32;
        while host_transient_is_verified(&handle, related.operation).await {
            passes += 1;
            assert!(
                passes <= 16,
                "bounded continuation must reach the related operation"
            );
            {
                let _gate = handle.host_transient_arrival.lock().await;
                super::publish_owed_learning_arrivals(
                    &handle.store,
                    &handle.host_transient_arrival,
                    &handle.learning_queue,
                )
                .await;
            }
        }
        for (operation, sweep, phase) in unrelated {
            let record = operation_record(&handle, operation).await;
            assert_eq!(record.current.sweep, sweep);
            assert_eq!(record.phase, phase);
            assert!(
                host_transient_is_verified(&handle, operation).await,
                "unrelated HostTransient verification must stay"
            );
        }
        drive_until_completed(&handle, related.operation).await;
    }

    /// F-3 regression: a new arrival must not rewind the bounded global walk
    /// to the head. With more unfinished operations than one page, successive
    /// arrivals keep advancing the classification window until the last page
    /// completes the lap instead of re-classifying the head page forever.
    #[tokio::test]
    async fn a_new_arrival_continues_the_bounded_walk_instead_of_rewinding_it() {
        let (handle, _dir) = memory_handle("a3c-arrival-rotation")
            .await
            .expect("the handle opens");
        for index in 0..=HOST_TRANSIENT_ARRIVAL_PAGE {
            let current = admit(
                &handle,
                &format!("rotation-unrelated-{index}"),
                vec![ParticipantOwnerRef::HostTransient],
            )
            .await;
            verify_host_transient(&handle, current).await;
        }
        handle
            .queue_learning_formation(experience("contains first-arrival"))
            .await;
        assert_eq!(
            handle.host_transient_arrival.last_classified(),
            HOST_TRANSIENT_ARRIVAL_PAGE as usize,
            "the first walk fills exactly one bounded page"
        );
        assert!(
            handle.host_transient_arrival.has_unpublished(),
            "a full page leaves the global walk unfinished"
        );
        handle
            .queue_learning_formation(experience("contains second-arrival"))
            .await;
        assert_eq!(
            handle.host_transient_arrival.last_classified(),
            1,
            "the arrival must continue after the first page instead of restarting at the head"
        );
        assert!(
            !handle.host_transient_arrival.has_unpublished(),
            "the wrapped page completes the lap over every unfinished operation"
        );
    }

    /// Classification that cannot finish must keep the arrival unpublished
    /// after a related operation already published.
    #[tokio::test]
    async fn an_incomplete_operation_page_keeps_arrival_unpublished() {
        let (handle, _dir) = memory_handle("a3c-arrival-lookup-failure")
            .await
            .expect("the handle opens");
        let a = admit(
            &handle,
            "secret-a",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        let b = admit(
            &handle,
            "secret-b",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        let c = admit(
            &handle,
            "secret-c",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        for current in [a, b, c] {
            verify_host_transient(&handle, current).await;
        }
        handle
            .store
            .fail_deletion_operation_material_for_tests(c.operation);
        handle
            .queue_learning_formation(experience("contains secret-a only"))
            .await;
        assert!(
            handle.host_transient_arrival.has_unpublished(),
            "an incomplete classification is not clean"
        );
        assert_ne!(
            deletion_phase(&handle, a.operation).await,
            DeletionOperationPhase::Completed
        );
        let record_b = operation_record(&handle, b.operation).await;
        assert_eq!(record_b.current.sweep, b.sweep);
        assert!(
            host_transient_is_verified(&handle, b.operation).await,
            "an unrelated operation must stay verified while classification is incomplete"
        );
        assert!(
            handle.host_transient_arrival.scan_incomplete(),
            "C's classification failure leaves the global walk unfinished"
        );
        drive_until_operation_completed(&handle, b).await;
        assert_eq!(
            deletion_phase(&handle, b.operation).await,
            DeletionOperationPhase::Completed,
            "B must Complete while C's classification is still failing"
        );
        let record_b = operation_record(&handle, b.operation).await;
        assert_eq!(
            record_b.current.sweep, b.sweep,
            "B must not open a new sweep"
        );
        assert!(
            queued_contains(&handle, "secret-a"),
            "completing B must not collect A's TARGET"
        );
        assert_ne!(
            deletion_phase(&handle, a.operation).await,
            DeletionOperationPhase::Completed,
            "A must not Complete over a still-queued TARGET"
        );
        assert_ne!(
            deletion_phase(&handle, c.operation).await,
            DeletionOperationPhase::Completed,
            "C must stay fail-closed on its own classification failure"
        );
        assert!(
            handle.host_transient_arrival.scan_incomplete(),
            "completing unrelated B must not pretend the global walk finished"
        );
        handle.store.allow_deletion_operation_material_for_tests();
        drive_until_completed(&handle, a.operation).await;
        assert_eq!(
            deletion_phase(&handle, c.operation).await,
            DeletionOperationPhase::Completed,
            "A and C must converge after C's classification is readable"
        );
        let record_b = operation_record(&handle, b.operation).await;
        assert_eq!(
            record_b.current.sweep, b.sweep,
            "completing A must still leave B's sweep untouched"
        );
    }

    /// Major: Deletion C's classification failure must not stop unrelated
    /// Deletion B from Completing, and must not Complete related A over a
    /// still-queued TARGET.
    #[tokio::test]
    async fn an_unrelated_operation_completes_despite_another_classification_failure() {
        let (handle, _dir) = memory_handle("a3c-scan-incomplete-opspec-b")
            .await
            .expect("the handle opens");
        let a = admit(
            &handle,
            "secret-a",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        let b = admit(
            &handle,
            "secret-b",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        let c = admit(
            &handle,
            "secret-c",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        for current in [a, b, c] {
            verify_host_transient(&handle, current).await;
        }
        handle
            .store
            .fail_deletion_operation_material_for_tests(c.operation);
        handle
            .queue_learning_formation(experience("contains secret-a only"))
            .await;
        let a_after = operation_record(&handle, a.operation).await;
        if a_after.current.sweep != a.sweep {
            assert!(
                !host_transient_is_verified(&handle, a.operation).await,
                "A arrival related: canonical publish opens the next sweep"
            );
        }
        let record_b = operation_record(&handle, b.operation).await;
        assert_eq!(record_b.current.sweep, b.sweep, "B is unrelated");
        assert!(
            host_transient_is_verified(&handle, b.operation).await,
            "B HostTransient verification is maintained"
        );
        let record_c = operation_record(&handle, c.operation).await;
        assert_eq!(
            record_c.current.sweep, c.sweep,
            "C's classification failure must not open a new sweep"
        );
        assert!(handle.host_transient_arrival.scan_incomplete());
        assert!(
            !super::unpublished_blocks_finalizing(
                &handle.store,
                &handle.host_transient_arrival,
                &handle.learning_queue,
                b,
            )
            .await,
            "global scan_incomplete must not by itself block unrelated B"
        );
        drive_until_operation_completed(&handle, b).await;
        assert_eq!(
            deletion_phase(&handle, b.operation).await,
            DeletionOperationPhase::Completed
        );
        assert!(queued_contains(&handle, "secret-a"));
        assert_ne!(
            deletion_phase(&handle, a.operation).await,
            DeletionOperationPhase::Completed
        );
        assert_ne!(
            deletion_phase(&handle, c.operation).await,
            DeletionOperationPhase::Completed
        );
        assert!(handle.host_transient_arrival.scan_incomplete());
    }

    /// An operation admitted while the arrival walk is stuck incomplete must
    /// still verify and Complete through `commit_verified`: the global
    /// `scan_incomplete` flag fail-closes only the operation whose
    /// classification failed.
    #[tokio::test]
    async fn an_unrelated_operation_verifies_through_commit_while_scan_is_incomplete() {
        let (handle, _dir) = memory_handle("a3c-commit-unrelated")
            .await
            .expect("the handle opens");
        let b = admit(
            &handle,
            "secret-b",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        let c = admit(
            &handle,
            "secret-c",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        // Only C is unreadable, so the walk stays globally incomplete. B is
        // never pre-verified: it must reach Verified through the production
        // `commit_verified` path despite the global flag.
        handle
            .store
            .fail_deletion_operation_material_for_tests(c.operation);
        handle
            .queue_learning_formation(experience("contains nothing related"))
            .await;
        assert!(handle.host_transient_arrival.scan_incomplete());
        for _ in 0..32 {
            handle
                .drive_targeted_deletion(TargetedDeletionPass::default())
                .await
                .expect("the fan-out must not fail");
            if deletion_phase(&handle, b.operation).await == DeletionOperationPhase::Completed {
                break;
            }
        }
        assert_eq!(
            deletion_phase(&handle, b.operation).await,
            DeletionOperationPhase::Completed,
            "an unrelated operation must Complete through commit_verified while C's classification fails"
        );
        assert_ne!(
            deletion_phase(&handle, c.operation).await,
            DeletionOperationPhase::Completed,
            "C must stay fail-closed on its own classification failure"
        );
        assert!(handle.host_transient_arrival.scan_incomplete());
    }

    /// Major: a classification StorageUnavailable fail-closes only the
    /// failing operation. Sibling sweeps are not rewritten by that failure.
    #[tokio::test]
    async fn a_classification_failure_blocks_only_the_failing_operation() {
        let (handle, _dir) = memory_handle("a3c-scan-incomplete-opspec-c")
            .await
            .expect("the handle opens");
        let a = admit(
            &handle,
            "secret-a",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        let b = admit(
            &handle,
            "secret-b",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        let c = admit(
            &handle,
            "secret-c",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        for current in [a, b, c] {
            verify_host_transient(&handle, current).await;
        }
        handle
            .store
            .fail_deletion_operation_material_for_tests(c.operation);
        handle
            .queue_learning_formation(experience("contains secret-a only"))
            .await;
        let sweep_a = operation_record(&handle, a.operation).await.current.sweep;
        let sweep_b = operation_record(&handle, b.operation).await.current.sweep;
        assert!(
            super::unpublished_blocks_finalizing(
                &handle.store,
                &handle.host_transient_arrival,
                &handle.learning_queue,
                c,
            )
            .await,
            "C must fail closed on its own classification failure"
        );
        assert_ne!(
            try_finalize_one_operation(&handle, c).await,
            DeletionOperationPhase::Completed,
            "C must not Complete"
        );
        assert_eq!(
            operation_record(&handle, a.operation).await.current.sweep,
            sweep_a,
            "C's failure must not rewrite A's sweep"
        );
        assert_eq!(
            operation_record(&handle, b.operation).await.current.sweep,
            sweep_b,
            "C's failure must not rewrite B's sweep"
        );
        drive_until_operation_completed(&handle, b).await;
        assert_eq!(
            deletion_phase(&handle, b.operation).await,
            DeletionOperationPhase::Completed,
            "B must still be able to progress"
        );
        assert_eq!(
            operation_record(&handle, b.operation).await.current.sweep,
            sweep_b
        );
        assert_ne!(
            deletion_phase(&handle, c.operation).await,
            DeletionOperationPhase::Completed
        );
    }

    /// Major: the Finalizing fallback classifies only the current operation
    /// and does not turn the producer walk unbounded.
    #[tokio::test]
    async fn a_finalizing_fallback_classifies_only_the_current_operation() {
        let (handle, _dir) = memory_handle("a3c-scan-incomplete-opspec-page")
            .await
            .expect("the handle opens");
        let extra_count = HOST_TRANSIENT_ARRIVAL_PAGE * 3 + 5;
        let mut extras = Vec::new();
        for n in 1..=extra_count {
            let extra = admit(
                &handle,
                &format!("unrelated-fallback-{n}"),
                vec![ParticipantOwnerRef::HostTransient],
            )
            .await;
            verify_host_transient(&handle, extra).await;
            extras.push(extra);
        }
        let related = admit(
            &handle,
            "secret-related-fallback",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        verify_host_transient(&handle, related).await;
        handle
            .queue_learning_formation(experience("contains secret-related-fallback"))
            .await;
        assert!(
            handle.host_transient_arrival.last_classified() <= HOST_TRANSIENT_ARRIVAL_PAGE as usize,
            "one normal publication call examines at most PAGE unfinished operations"
        );
        assert!(
            handle.host_transient_arrival.scan_incomplete(),
            "PAGE*3+5 unfinished operations cannot finish in one producer page"
        );
        let classified_after_producer = handle.host_transient_arrival.last_classified();
        let first = handle
            .store
            .unfinished_deletions(None, HOST_TRANSIENT_ARRIVAL_PAGE)
            .await
            .expect("the first page reads");
        let first_ids: std::collections::HashSet<_> = first
            .iter()
            .map(|record| record.current.operation)
            .collect();
        let unrelated = extras
            .iter()
            .copied()
            .find(|extra| !first_ids.contains(&extra.operation))
            .expect("some unrelated operation must sit past the first producer page");
        let direct_before = handle.host_transient_arrival.last_direct_classified();
        assert!(
            !super::unpublished_blocks_finalizing(
                &handle.store,
                &handle.host_transient_arrival,
                &handle.learning_queue,
                unrelated,
            )
            .await,
            "unrelated finalization is independent of the unfinished global walk"
        );
        assert_eq!(
            handle.host_transient_arrival.last_direct_classified(),
            direct_before + 1,
            "Finalizing fallback examines only that one operation"
        );
        assert_eq!(
            handle.host_transient_arrival.last_classified(),
            classified_after_producer,
            "Finalizing must not run the global unfinished-ops walk"
        );
        if host_transient_is_verified(&handle, related.operation).await {
            let direct_before = handle.host_transient_arrival.last_direct_classified();
            assert!(
                super::unpublished_blocks_finalizing(
                    &handle.store,
                    &handle.host_transient_arrival,
                    &handle.learning_queue,
                    related,
                )
                .await,
                "a related operation still on a later page is invalidated by the one-op fallback"
            );
            assert_eq!(
                handle.host_transient_arrival.last_direct_classified(),
                direct_before + 1
            );
            assert_eq!(
                handle.host_transient_arrival.last_classified(),
                classified_after_producer
            );
            assert!(
                !host_transient_is_verified(&handle, related.operation).await,
                "relevant operation is invalidated without a full global scan"
            );
        }
        drive_until_operation_completed(&handle, unrelated).await;
        assert_eq!(
            deletion_phase(&handle, unrelated.operation).await,
            DeletionOperationPhase::Completed
        );
        assert_ne!(
            deletion_phase(&handle, related.operation).await,
            DeletionOperationPhase::Completed,
            "related finalization stays independent of the unrelated sibling"
        );
        assert!(
            handle.host_transient_arrival.last_classified() <= HOST_TRANSIENT_ARRIVAL_PAGE as usize,
            "producer continuation stays bounded by PAGE per call"
        );
    }

    /// A clean publish state must not restart unfinished-operation
    /// classification on the next Targeted Deletion drive. Queue leftovers
    /// that are unrelated or already published are not a new arrival.
    #[tokio::test]
    async fn a_clean_arrival_publish_state_is_a_drive_noop() {
        let (handle, _dir) = memory_handle("a3c-arrival-clean-noop")
            .await
            .expect("the handle opens");
        let ready = admit(
            &handle,
            "secret-clean",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        verify_host_transient(&handle, ready).await;
        handle
            .queue_learning_formation(experience("unrelated-clean-body"))
            .await;
        assert!(
            !handle.host_transient_arrival.has_unpublished(),
            "an unrelated leftover must finish classification as clean"
        );
        let mut extras = Vec::new();
        for n in 1..=HOST_TRANSIENT_ARRIVAL_PAGE {
            let extra = admit(
                &handle,
                &format!("unrelated-clean-page-{n}"),
                vec![ParticipantOwnerRef::HostTransient],
            )
            .await;
            verify_host_transient(&handle, extra).await;
            extras.push((extra.operation, extra.sweep));
        }
        handle
            .drive_targeted_deletion(TargetedDeletionPass::new(100, 1))
            .await
            .expect("the clean drive runs");
        assert_eq!(
            handle.host_transient_arrival.last_classified(),
            0,
            "a clean publish state must not classify unfinished operations"
        );
        assert!(
            !handle.host_transient_arrival.has_unpublished(),
            "a clean drive must not mark scan_incomplete"
        );
        for (operation, sweep) in extras {
            let record = operation_record(&handle, operation).await;
            assert_eq!(
                record.current.sweep, sweep,
                "unrelated extras must not open a new sweep"
            );
        }
        drive_until_completed(&handle, ready.operation).await;

        let late = admit(
            &handle,
            "secret-dirty",
            vec![ParticipantOwnerRef::HostTransient],
        )
        .await;
        verify_host_transient(&handle, late).await;
        handle
            .queue_learning_formation(experience("contains secret-dirty"))
            .await;
        assert!(
            handle.host_transient_arrival.last_classified() > 0,
            "a new body-bearing arrival must restart bounded classification"
        );
        let mut passes = 0u32;
        while host_transient_is_verified(&handle, late.operation).await {
            passes += 1;
            assert!(
                passes <= 16,
                "dirty classification must reach the related operation"
            );
            let _gate = handle.host_transient_arrival.lock().await;
            super::publish_owed_learning_arrivals(
                &handle.store,
                &handle.host_transient_arrival,
                &handle.learning_queue,
            )
            .await;
        }
        drive_until_completed(&handle, late.operation).await;
    }

    /// Blocker 3 remainder: a Client class-wipe demand parked after durable
    /// admission, while another driver Completes and a fresh delivery lands,
    /// must not go on the wire.
    #[tokio::test]
    async fn a_stale_client_class_wipe_does_not_land_after_completion() {
        use ene_preservation::{
            DeletionFinalizationOutcome, DeletionOperationRef, DeletionReconciliationOutcome,
            ParticipantCompletionFact, ParticipantCompletionOutcome, ParticipantProgress,
        };

        let fixture = client_fixture("a3c-stale-class-wipe").await;
        let current = DeletionOperationRef {
            operation: fixture.condition.operation,
            sweep: fixture.condition.sweep,
        };
        fixture.handle.store.arm_client_demand_park_for_tests();
        let parked = {
            let identity = fixture.identity;
            let registry = Arc::clone(&fixture.handle.client_transients);
            let condition = fixture.condition;
            tokio::spawn(async move {
                ClientIncarnationParticipant::new(identity, registry)
                    .demand_local_erasure(command(
                        condition,
                        ParticipantOwnerRef::ClientIncarnation(identity),
                        ParticipantErasureScope::correlation_only(),
                    ))
                    .await
            })
        };
        fixture
            .handle
            .store
            .wait_client_demand_park_for_tests()
            .await;

        loop {
            match fixture
                .handle
                .store
                .reconcile_deletion_sources(current, 64)
                .await
                .expect("reconciliation must answer")
            {
                DeletionReconciliationOutcome::Complete => break,
                DeletionReconciliationOutcome::Advanced => continue,
                other => panic!("the walk must finish, got {other:?}"),
            }
        }
        assert_eq!(
            fixture
                .handle
                .store
                .record_participant_completion(ParticipantCompletionFact::verified(
                    fixture.condition,
                    ParticipantOwnerRef::ClientIncarnation(fixture.identity),
                    0,
                    WallClockWithTz::now(),
                ))
                .await
                .expect("the verified fact must record"),
            ParticipantCompletionOutcome::Recorded(ParticipantProgress::Verified {
                sweep: fixture.condition.sweep,
            })
        );
        assert_eq!(
            fixture
                .handle
                .store
                .begin_deletion_finalizing(current)
                .await
                .expect("the finalizing transition must answer"),
            DeletionFinalizationOutcome::Finalizing
        );
        assert_eq!(
            fixture
                .handle
                .store
                .complete_deletion_finalizing(current)
                .await
                .expect("the completion commit must answer"),
            DeletionFinalizationOutcome::Completed
        );

        assert!(
            fixture
                .handle
                .note_client_body_delivery(&fixture.live)
                .await,
            "a post-completion delivery is a fresh origin"
        );
        assert_eq!(
            evidence_seq(&fixture).await,
            Some(2),
            "the fresh delivery advances the durable evidence"
        );

        fixture.handle.store.release_client_demand_park_for_tests();
        let stale = parked.await.expect("the parked demand joins");
        assert_eq!(
            stale.status(),
            ParticipantCompletionStatus::LocalComplete,
            "a completed condition is NotCurrent, never a wire demand"
        );
        assert!(
            fixture
                .handle
                .take_client_demand(&fixture.live)
                .await
                .is_none(),
            "a stale class-wipe must not reach the wire"
        );
        assert_eq!(
            evidence_seq(&fixture).await,
            Some(2),
            "the fresh delivery evidence must remain"
        );
    }
}
