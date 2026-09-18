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

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ene_api::v1::deletion::{
    ClientTempClass, DeletionDemand, DeletionDemandWireId, DeletionTargetWire, LocalErasureResult,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{ConnectionWireId, DeletionOperationWireRef};
use ene_learning::ExperienceCandidate;
use ene_preservation::{
    DemandLocalErasureCommand, ErasureConditionRef, ErasureParticipant, MechanicalDeletionTarget,
    ParticipantCompletionFact, ParticipantHoldClass, ParticipantOwnerRef, TargetedDeletionTarget,
};
use ene_primitive::{RawId, WallClockWithTz};
use ene_store::Store;
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

/// Host-process transient erasure participant (lifecycle §8, SO §4.17).
pub(crate) struct HostTransientParticipant {
    store: Store,
    fence: Arc<TransientErasureFence>,
    presentations: Arc<std::sync::Mutex<PresentationState>>,
    learning_queue: Arc<std::sync::Mutex<LearningFormationQueue>>,
    /// Serializes HostTransient demands. The queue `std` mutex is never held
    /// across an await; this tokio lock only prevents two demands from
    /// overlapping snapshot/apply on the same process-local cursor.
    demand_lock: tokio::sync::Mutex<()>,
    sweep: std::sync::Mutex<Option<HostTransientLearningSweep>>,
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
    ) -> Self {
        Self {
            store,
            fence,
            presentations,
            learning_queue,
            demand_lock: tokio::sync::Mutex::new(()),
            sweep: std::sync::Mutex::new(None),
            #[cfg(test)]
            last_scanned: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    #[cfg(test)]
    fn last_scanned(&self) -> usize {
        self.last_scanned.load(Ordering::SeqCst)
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
                #[cfg(test)]
                self.last_scanned.store(page_len, Ordering::SeqCst);
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
            // the live queue generation.
            {
                let queue = crate::lock_unpoison(&self.learning_queue);
                if queue.mutation_generation() != generation {
                    self.rebase_sweep(
                        command.condition(),
                        queue.mutation_generation(),
                        queue.len(),
                    );
                    let remainder = queue.len() as u64 + u64::from(queue.taken().is_some());
                    return ParticipantCompletionFact::more_work(
                        command.condition(),
                        ParticipantOwnerRef::HostTransient,
                        dropped_presentation + dropped_learning,
                        remainder.max(1),
                        WallClockWithTz::now(),
                    );
                }
            }
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
    /// Test-only wait bound override.
    #[cfg(test)]
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
            #[cfg(test)]
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

    #[cfg(test)]
    fn wait_limit(&self) -> Duration {
        crate::lock_unpoison(&self.wait_limit).unwrap_or(CLIENT_ERASURE_WAIT)
    }

    #[cfg(test)]
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
        #[cfg(test)]
        let limit = registry.wait_limit();
        #[cfg(not(test))]
        let limit = CLIENT_ERASURE_WAIT;
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
        DeletionOperationPhase, DeletionPurpose, DeletionSearchMaterial, DemandLocalErasureCommand,
        ParticipantCompletionStatus, ParticipantErasureScope, PreservationRepository as _,
        StartTargetedDeletionCommand, StartTargetedDeletionOutcome,
    };
    use ene_primitive::WallClockWithTz;

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
        match handle
            .store
            .start_targeted_deletion(
                StartTargetedDeletionCommand::new(
                    target(text),
                    DeletionPurpose::Privacy,
                    WallClockWithTz::now(),
                    Vec::new(),
                    participants,
                )
                .confirmed_for_tests(),
            )
            .await
            .expect("admission must commit")
        {
            StartTargetedDeletionOutcome::Started(current) => current,
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

    async fn demand_secret(
        participant: &HostTransientParticipant,
        condition: ErasureConditionRef,
    ) -> ParticipantCompletionFact {
        participant
            .demand_local_erasure(command(
                condition,
                ParticipantOwnerRef::HostTransient,
                ParticipantErasureScope::local(target("secret body"), Vec::new()),
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
        assert!(outcome.held >= 1);
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
        assert!(outcome.held >= 1, "the pass reports a hold: {outcome:?}");
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
                ParticipantErasureScope::correlation_only(Vec::new()),
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
                ParticipantErasureScope::correlation_only(Vec::new()),
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
            ParticipantErasureScope::correlation_only(Vec::new()),
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
                ParticipantErasureScope::correlation_only(Vec::new()),
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
            ParticipantErasureScope::correlation_only(Vec::new()),
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
                ParticipantErasureScope::correlation_only(Vec::new()),
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
            ParticipantErasureScope::correlation_only(Vec::new()),
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
                ParticipantErasureScope::correlation_only(Vec::new()),
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
                ParticipantErasureScope::correlation_only(Vec::new()),
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
                ParticipantErasureScope::local(target("secret body"), Vec::new()),
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
        let outcome = reopened
            .drive_targeted_deletion(TargetedDeletionPass::default())
            .await
            .expect("the pass runs");
        assert!(
            outcome.held >= 1,
            "an unreachable old incarnation is an explicit hold: {outcome:?}"
        );
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
        assert!(
            outcome.held >= 1,
            "an unreachable holder is never a completion"
        );
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
                ParticipantErasureScope::local(target("secret body"), Vec::new()),
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
                ParticipantErasureScope::local(target("secret body"), Vec::new()),
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

        let drive = handle
            .drive_targeted_deletion(TargetedDeletionPass::new(100, 1))
            .await
            .expect("the fan-out must not fail");
        assert!(
            drive.unfinished > 0,
            "the durable HostTransient row must stay unfinished"
        );
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
                && outcome.held == 0
                && outcome.unfinished == 0
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
                        ParticipantErasureScope::correlation_only(Vec::new()),
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
