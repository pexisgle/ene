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
//! Both participants are bounded work: the Host-transient demand drops the
//! affected in-memory entries (never durable rows), the Client demand is one
//! wire message with one bounded wait, and an unreachable, disconnecting, or
//! silent Client is an explicit hold, never a completion. Disconnect and
//! timeout prove nothing about the Client's local copy.

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
use tokio::sync::Notify;
use uuid::Uuid;

use crate::conn::ConnectionTable;
use crate::presentation::PresentationState;
use crate::serve::HostHandle;

/// Bound on tracked Client incarnations. One entry is a boot identity plus its
/// Host-minted projection; the oldest entry drops first, and a dropped
/// incarnation stops being appended to new required sets (an already admitted
/// operation keeps its durable snapshot).
const TRACKED_CLIENT_CAP: usize = 256;

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

/// The protected exact mechanical text of one operation target.
fn exact_text(target: &TargetedDeletionTarget) -> &str {
    let MechanicalDeletionTarget::ExactText(material) = &target.mechanical;
    material.expose_for_erasure()
}

/// Whether one queued formation premise can carry the covered target: either
/// its transcript text contains the exact mechanical target, or one of its
/// correlated source bounds is a covered source.
fn experience_covered(experience: &ExperienceCandidate, exact: &str, sources: &[RawId]) -> bool {
    if sources
        .iter()
        .any(|source| *source == experience.source.start || *source == experience.source.end)
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

/// Host-process transient erasure participant (lifecycle §8, SO §4.17).
pub(crate) struct HostTransientParticipant {
    fence: Arc<TransientErasureFence>,
    presentations: Arc<std::sync::Mutex<PresentationState>>,
    learning_queue: Arc<std::sync::Mutex<VecDeque<ExperienceCandidate>>>,
}

impl HostTransientParticipant {
    #[must_use]
    pub(crate) fn new(
        fence: Arc<TransientErasureFence>,
        presentations: Arc<std::sync::Mutex<PresentationState>>,
        learning_queue: Arc<std::sync::Mutex<VecDeque<ExperienceCandidate>>>,
    ) -> Self {
        Self {
            fence,
            presentations,
            learning_queue,
        }
    }

    /// Drops every queued formation premise that can carry the target and
    /// returns how many were dropped.
    fn prune_learning_queue(&self, exact: &str, sources: &[RawId]) -> u64 {
        let mut queue = crate::lock_unpoison(&self.learning_queue);
        let before = queue.len();
        queue.retain(|experience| !experience_covered(experience, exact, sources));
        (before - queue.len()) as u64
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
            let sources = command.scope().sources().to_vec();
            let exact = command
                .scope()
                .target()
                .map(exact_text)
                .unwrap_or_default()
                .to_owned();
            // Presentation receipts, carried refs, cursors, subscriptions, and
            // resume slots are all reconstructible from canonical rows; none is
            // provably body-free, so the whole per-connection world is
            // invalidated. Future presentation re-reads the canonical source.
            let dropped_presentation =
                crate::lock_unpoison(&self.presentations).invalidate_for_erasure();
            let dropped_learning = self.prune_learning_queue(&exact, &sources);
            // In-flight streams and assembled replies fail closed from here on;
            // nothing published before the fence is treated as proof of
            // completion (a durable reply is the History owner's to erase).
            self.fence.invalidate();
            ParticipantCompletionFact::verified(
                command.condition(),
                ParticipantOwnerRef::HostTransient,
                dropped_presentation + dropped_learning,
                WallClockWithTz::now(),
            )
        })
    }
}

/// One Client incarnation the Host handed body-bearing material to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TrackedIncarnation {
    /// Host-minted participant identity, derived deterministically from the
    /// Client's boot incarnation so the durable participant snapshot survives
    /// a Host restart.
    identity: RawId,
    counter: u64,
    random: u64,
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
    state: PendingState,
}

enum PendingState {
    Awaiting,
    Answered(LocalErasureResult),
}

/// What one bounded wait observed. Every non-answer outcome is a hold.
enum ClientErasureWait {
    Answered(LocalErasureResult),
    /// The connection ended (closed or superseded), the wait bound elapsed, or
    /// the pending demand was replaced. No proof of local erasure.
    Abandoned,
}

/// Host-memory registry of Client incarnations that may hold a target-bearing
/// local copy, plus the in-flight demand plumbing (lifecycle §8.1, IPC §17).
///
/// Tracking evidence is delivery, not connection: an incarnation enters the
/// registry only when the Host actually handed it body-bearing material
/// (presentation excerpts, history items, Task report source bodies, or text
/// stream deltas). A Client that only sent requests has no copy the Host
/// could erase, and is not claimed as a required participant.
#[derive(Default)]
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
    /// Test-only wait bound override.
    #[cfg(test)]
    wait_limit: std::sync::Mutex<Option<Duration>>,
}

#[derive(Default)]
struct ClientTransientInner {
    /// Delivery order, oldest first.
    tracked: VecDeque<TrackedIncarnation>,
    /// At most one outstanding demand per incarnation.
    pending: HashMap<RawId, PendingDemand>,
}

impl ClientTransientRegistry {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
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

    /// Records that body-bearing material reached this incarnation. Returns
    /// the identity when the incarnation was newly tracked, so the caller can
    /// register the matching participant implementation exactly once.
    pub(crate) fn note_body_delivery(&self, counter: u64, random: u64) -> Option<RawId> {
        let identity = Self::identity_for(counter, random);
        let mut inner = crate::lock_unpoison(&self.inner);
        if inner
            .tracked
            .iter()
            .any(|tracked| tracked.identity == identity)
        {
            return None;
        }
        inner.tracked.push_back(TrackedIncarnation {
            identity,
            counter,
            random,
        });
        while inner.tracked.len() > TRACKED_CLIENT_CAP {
            inner.tracked.pop_front();
        }
        Some(identity)
    }

    /// Identities that may hold a target-bearing copy, in delivery order.
    #[must_use]
    pub(crate) fn tracked_incarnations(&self) -> Vec<RawId> {
        crate::lock_unpoison(&self.inner)
            .tracked
            .iter()
            .map(|tracked| tracked.identity)
            .collect()
    }

    /// The current authenticated connection of one tracked incarnation.
    fn current_connection(&self, identity: RawId) -> Option<ConnectionWireId> {
        let (counter, random) = {
            let inner = crate::lock_unpoison(&self.inner);
            inner
                .tracked
                .iter()
                .find(|tracked| tracked.identity == identity)
                .map(|tracked| (tracked.counter, tracked.random))?
        };
        let table = self.table.get()?;
        table.current_connection_for_incarnation(counter, random)
    }

    #[must_use]
    pub(crate) fn demand_wakeup(&self) -> &Notify {
        &self.delivery_wake
    }

    /// Begins one bounded demand for an incarnation, replacing any older
    /// outstanding demand for it. The older condition's waiter observes
    /// [`ClientErasureWait::Abandoned`] instead of adopting a foreign answer.
    fn begin(
        &self,
        identity: RawId,
        condition: ErasureConditionRef,
        connection: ConnectionWireId,
    ) -> String {
        let id = Uuid::new_v4().as_hyphenated().to_string();
        let mut inner = crate::lock_unpoison(&self.inner);
        inner.pending.insert(
            identity,
            PendingDemand {
                id: id.clone(),
                condition,
                connection,
                delivered_to: None,
                state: PendingState::Awaiting,
            },
        );
        drop(inner);
        // Every parked connection loop re-checks, and the stored permit keeps
        // a wakeup that arrived before a loop parked from being lost.
        self.delivery_wake.notify_waiters();
        self.delivery_wake.notify_one();
        id
    }

    /// The demand this connection may carry now, marked delivered.
    fn take_deliverable(
        &self,
        connection: ConnectionWireId,
        counter: u64,
        random: u64,
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

    /// Records one Client result. Returns whether it answered the outstanding
    /// demand: a foreign demand id, an operation/sweep mismatch, or an
    /// incarnation that is not the demanded one is refused without touching
    /// the pending state.
    fn accept_result(
        &self,
        connection: ConnectionWireId,
        counter: u64,
        random: u64,
        result: LocalErasureResult,
    ) -> bool {
        let identity = Self::identity_for(counter, random);
        let mut inner = crate::lock_unpoison(&self.inner);
        let Some(pending) = inner.pending.get_mut(&identity) else {
            return false;
        };
        if pending.connection != connection || pending.id != result.demand.0 {
            return false;
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
            return false;
        }
        if matches!(pending.state, PendingState::Answered(_)) {
            return false;
        }
        pending.state = PendingState::Answered(result);
        drop(inner);
        self.result_wake.notify_waiters();
        self.result_wake.notify_one();
        true
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
    async fn wait(&self, identity: RawId, condition: ErasureConditionRef) -> ClientErasureWait {
        loop {
            {
                let inner = crate::lock_unpoison(&self.inner);
                match inner.pending.get(&identity) {
                    Some(pending) if pending.condition == condition => match &pending.state {
                        PendingState::Answered(result) => {
                            return ClientErasureWait::Answered(result.clone());
                        }
                        PendingState::Awaiting => {}
                    },
                    // No pending demand for this condition: it was abandoned
                    // (connection ended) or replaced by a later condition.
                    Some(_) | None => return ClientErasureWait::Abandoned,
                }
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
            let Some(connection) = self.registry.current_connection(self.identity) else {
                // Unreachable: never presumed erased (§8.1).
                return ParticipantCompletionFact::held(
                    condition,
                    owner,
                    ParticipantHoldClass::Unavailable,
                    WallClockWithTz::now(),
                );
            };
            self.registry.begin(self.identity, condition, connection);
            #[cfg(test)]
            let limit = self.registry.wait_limit();
            #[cfg(not(test))]
            let limit = CLIENT_ERASURE_WAIT;
            let wait = self.registry.wait(self.identity, condition);
            let outcome = match tokio::time::timeout(limit, wait).await {
                Ok(outcome) => outcome,
                Err(_elapsed) => {
                    // The wait bound elapsed: drop the pending demand so a later
                    // answer is not adopted against a condition this pass no
                    // longer owns.
                    self.registry.note_connection_ended(&connection);
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
        })
    }
}

impl HostHandle {
    /// Records that body-bearing material reached the incarnation of the
    /// connection behind `live`, tracking it as a possible target-bearing copy
    /// holder and registering its participant implementation once.
    pub(crate) fn note_client_body_delivery(&self, live: &crate::serve::LiveInput) {
        let Some((counter, random)) = live.authority.incarnation_of(&live.connection_id) else {
            // No pinned incarnation (an unauthenticated frame, or a
            // transport-free seam): nothing was handed to a known client.
            return;
        };
        let Some(identity) = self.client_transients.note_body_delivery(counter, random) else {
            return;
        };
        let participant = Arc::new(ClientIncarnationParticipant::new(
            identity,
            Arc::clone(&self.client_transients),
        ));
        match crate::lock_unpoison(&self.targeted_deletion).register(participant) {
            Ok(()) => {}
            Err(_owner) => {
                // One implementation per owner; a re-observed incarnation
                // reuses the registered one.
            }
        }
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
    pub(crate) fn take_client_demand(&self, live: &crate::serve::LiveInput) -> Option<WirePayload> {
        let (counter, random) = live.authority.incarnation_of(&live.connection_id)?;
        self.client_transients
            .take_deliverable(live.connection_id, counter, random)
            .map(WirePayload::DeletionDemand)
    }

    /// Records one Client local-erasure result against its outstanding demand.
    pub(crate) fn accept_client_erasure_result(
        &self,
        live: &crate::serve::LiveInput,
        result: &LocalErasureResult,
    ) {
        let Some((counter, random)) = live.authority.incarnation_of(&live.connection_id) else {
            return;
        };
        // A true return means the demand was answered; the awaiting participant
        // reads the recorded fact. A false return is a stale or foreign report
        // and changes nothing (§17.2).
        let _answered = self.client_transients.accept_result(
            live.connection_id,
            counter,
            random,
            result.clone(),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::serve::LiveInput;
    use crate::targeted_deletion::TargetedDeletionPass;
    use crate::test_support::{authenticate, memory_handle};
    use ene_learning::{ExperienceRole, ExperienceSourceKind, ExperienceTurn, SourceRangeRef};
    use ene_preservation::{
        DeletionPurpose, DeletionSearchMaterial, DeletionSweepGeneration,
        DemandLocalErasureCommand, ParticipantCompletionStatus, ParticipantErasureScope,
        PreservationRepository as _, StartTargetedDeletionCommand, StartTargetedDeletionOutcome,
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
        // Body-bearing material reached this incarnation: the Host tracks it
        // and registers its participant implementation.
        handle.note_client_body_delivery(&live);
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
        handle.note_client_body_delivery(&live);
        let identity = ClientTransientRegistry::identity_for(41, 42);
        let required = handle.required_deletion_participants();
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
    async fn an_in_flight_client_demand_is_abandoned_when_its_connection_ends() {
        let fixture = client_fixture("a3c-client-abandoned").await;
        let participant = ClientIncarnationParticipant::new(
            fixture.identity,
            Arc::clone(&fixture.handle.client_transients),
        );
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
        let mut demand = Box::pin(participant.demand_local_erasure(command(
            fixture.condition,
            owner,
            ParticipantErasureScope::correlation_only(Vec::new()),
        )));
        // One turn lets the demand register and park on its result.
        tokio::select! {
            biased;
            _ = &mut demand => panic!("the demand must not finish before its result"),
            () = tokio::task::yield_now() => {}
        }
        let payload = fixture
            .handle
            .take_client_demand(&fixture.live)
            .expect("the pending demand must be deliverable");
        let WirePayload::DeletionDemand(demand_payload) = payload else {
            panic!("the delivery is a DeletionDemand");
        };
        assert!(
            !format!("{demand_payload:?}").contains("secret body"),
            "the wire demand never carries the target body"
        );
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
            .accept_client_erasure_result(&fixture.live, &result);
        // A duplicate report is refused: the demand was answered once.
        fixture
            .handle
            .accept_client_erasure_result(&fixture.live, &result);
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
        let Some(WirePayload::DeletionDemand(demand_payload)) =
            fixture.handle.take_client_demand(&fixture.live)
        else {
            panic!("the pending demand must be deliverable");
        };
        let result = LocalErasureResult {
            demand: demand_payload.demand.clone(),
            operation: demand_payload.operation.clone(),
            sweep: demand_payload.sweep,
            wiped: vec![ClientTempClass::InputDraft],
            unverified: vec![ClientTempClass::PresentationBuffer],
        };
        fixture
            .handle
            .accept_client_erasure_result(&fixture.live, &result);
        let fact = demand.await;
        assert_eq!(
            fact.status(),
            ParticipantCompletionStatus::LocalComplete,
            "an unverified local class is never upgraded to verified"
        );
        assert_eq!(fact.remainder_count(), 1);
    }

    #[tokio::test]
    async fn a_silent_client_demand_holds_after_the_wait_bound() {
        let fixture = client_fixture("a3c-client-timeout").await;
        fixture
            .handle
            .client_transients
            .set_wait_limit_for_test(Duration::from_millis(100));
        let participant = ClientIncarnationParticipant::new(
            fixture.identity,
            Arc::clone(&fixture.handle.client_transients),
        );
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
            "silence is a hold, never a completion"
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

    #[tokio::test]
    async fn the_host_transient_demand_drops_covered_premises_and_moves_the_fence() {
        let (handle, _dir) = memory_handle("a3c-host-transient")
            .await
            .expect("the handle opens");
        crate::lock_unpoison(&handle.learning_queue).push_back(experience("contains secret body"));
        crate::lock_unpoison(&handle.learning_queue).push_back(experience("unrelated text"));
        let epoch = handle.transient_fence_epoch();
        let participant = HostTransientParticipant::new(
            Arc::clone(&handle.transient_fence),
            Arc::clone(&handle.presentations),
            Arc::clone(&handle.learning_queue),
        );
        let condition = ErasureConditionRef {
            operation: ene_preservation::DeletionOperationId::from_raw(RawId::new()),
            sweep: DeletionSweepGeneration::from_u64(1),
        };
        let fact = participant
            .demand_local_erasure(command(
                condition,
                ParticipantOwnerRef::HostTransient,
                ParticipantErasureScope::local(target("secret body"), Vec::new()),
            ))
            .await;
        assert_eq!(fact.status(), ParticipantCompletionStatus::Verified);
        assert!(fact.erased_count() >= 1);
        let queue = crate::lock_unpoison(&handle.learning_queue);
        assert_eq!(queue.len(), 1, "only the covered premise drops");
        assert!(queue[0].transcript[0].text.contains("unrelated text"));
        drop(queue);
        assert!(
            handle.transient_fence_epoch() > epoch,
            "in-flight streams and assembled replies fail closed after the demand"
        );
    }
}
