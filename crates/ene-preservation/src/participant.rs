//! Required-participant contracts for Targeted Deletion (lifecycle §8-§10).
//!
//! `ene-preservation` owns the shared vocabulary and the canonical durable
//! participant snapshot and never depends on a concrete participant crate.
//! Each semantic owner implements [`ErasureParticipant`] in its own crate, and
//! the Host composition registers the implementations and performs the
//! fan-out. The durable snapshot is written in the same short transaction as
//! the operation admission: an operation never exists without the participant
//! set that must complete before it can be finalized.

use std::pin::Pin;

use ene_primitive::{RawId, WallClockWithTz};

use crate::{
    DeletionOperationId, DeletionSweepGeneration, ErasureConditionRef, TargetedDeletionTarget,
};

/// One participant owner in a deletion operation.
///
/// The semantic-owner variants name domains that own durable state
/// (lifecycle §8). [`ParticipantOwnerRef::ClientIncarnation`] additionally
/// carries the Host-minted identity of one Client incarnation that may hold a
/// target-bearing transient copy: a replacement incarnation is a different
/// participant, and the durable snapshot keeps the incarnation that was
/// required at admission (§8.1).
///
/// This crate does not enumerate product capabilities. The Host composition
/// decides which owners the current product surface requires and registers the
/// implementations; an owner with no implementation is driven as an explicit
/// unsupported participant, never silently dropped and never treated as
/// complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParticipantOwnerRef {
    /// Companion-owned durable History, activity records, undelivered
    /// reporting state, and companion metadata (SO §4.3/4.4).
    Companion,
    /// Learning-owned Experience Summary, Memory revisions, and retrieval
    /// state (SO §4.5-4.9).
    Learning,
    /// Task-owned Task records, context entries, delegated results, and
    /// workspace associations (SO §4.10/4.13/4.14).
    Task,
    /// Action-owned attempt and effect bodies (SO §4.12).
    Action,
    /// Inference-owned attempt correlation and provider-session state
    /// (SO §5.2).
    Inference,
    /// Permission-owned consent and management-intent text (SO §4.19).
    Permission,
    /// Credential-owned local metadata; never the external service account
    /// (SO §4.21).
    Credential,
    /// Presence-owned attribution and relocation facts. Verification-only: the
    /// owner proves the absence of target-bearing state instead of creating a
    /// copy to erase (SO §4.15).
    Presence,
    /// Host-process transient state: open rounds, presentation buffers, and
    /// queued formation premises (SO §4.17).
    HostTransient,
    /// One Client incarnation that may hold target-bearing transient copies
    /// (§8.1). The instance identity is Host-minted and durable in the
    /// operation snapshot, so a replacement incarnation never inherits the
    /// required status of the one it replaced.
    ClientIncarnation(RawId),
}

impl ParticipantOwnerRef {
    /// Stable storage name.
    ///
    /// The `client_incarnation:<uuid>` form is a closed composite of the class
    /// name and the incarnation identity; the store round-trips it and never
    /// derives meaning from the text (CI §4.1).
    #[must_use]
    pub fn storage_name(self) -> String {
        match self {
            Self::Companion => String::from("companion"),
            Self::Learning => String::from("learning"),
            Self::Task => String::from("task"),
            Self::Action => String::from("action"),
            Self::Inference => String::from("inference"),
            Self::Permission => String::from("permission"),
            Self::Credential => String::from("credential"),
            Self::Presence => String::from("presence"),
            Self::HostTransient => String::from("host_transient"),
            Self::ClientIncarnation(instance) => {
                format!("client_incarnation:{}", instance.as_uuid().as_hyphenated())
            }
        }
    }

    /// Parses the [`Self::storage_name`] vocabulary, closed world. An unknown
    /// class or a malformed incarnation identity is unreadable stored state,
    /// never a guessed owner.
    #[must_use]
    pub fn from_storage_name(name: &str) -> Option<Self> {
        Some(match name {
            "companion" => Self::Companion,
            "learning" => Self::Learning,
            "task" => Self::Task,
            "action" => Self::Action,
            "inference" => Self::Inference,
            "permission" => Self::Permission,
            "credential" => Self::Credential,
            "presence" => Self::Presence,
            "host_transient" => Self::HostTransient,
            _ => {
                let instance = name.strip_prefix("client_incarnation:")?;
                Self::ClientIncarnation(RawId::from_uuid(instance.parse().ok()?))
            }
        })
    }

    /// Class-level label for display and audit. The incarnation identity stays
    /// separate so a view can distinguish holders without parsing the class.
    #[must_use]
    pub const fn class_name(self) -> &'static str {
        match self {
            Self::Companion => "companion",
            Self::Learning => "learning",
            Self::Task => "task",
            Self::Action => "action",
            Self::Inference => "inference",
            Self::Permission => "permission",
            Self::Credential => "credential",
            Self::Presence => "presence",
            Self::HostTransient => "host_transient",
            Self::ClientIncarnation(_) => "client_incarnation",
        }
    }

    /// Whether this owner names several live holders of one class. Only such a
    /// class can carry an incarnation identity.
    #[must_use]
    pub const fn is_incarnation(self) -> bool {
        matches!(self, Self::ClientIncarnation(_))
    }
}

/// One required participant of one deletion operation (§8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeletionParticipantRef {
    pub operation: DeletionOperationId,
    pub owner: ParticipantOwnerRef,
}

/// Why a participant is currently held (§9). A hold is a durable incomplete
/// outcome, never a completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParticipantHoldClass {
    /// The owner cannot run now: an unreachable Client incarnation, a
    /// disconnected transport, or a temporarily unavailable local store.
    Unavailable,
    /// No local-erasure implementation is registered for this owner. Explicit
    /// unsupported outcome: never a fake success and never a global completion.
    Unsupported,
    /// The owner attempted its bounded work and needs recovery before the
    /// current sweep can be verified.
    Failed,
}

impl ParticipantHoldClass {
    /// Stable storage name; one owner for the vocabulary.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Unsupported => "unsupported",
            Self::Failed => "failed",
        }
    }

    /// Parses the [`Self::as_str`] vocabulary, closed world.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "unavailable" => Self::Unavailable,
            "unsupported" => Self::Unsupported,
            "failed" => Self::Failed,
            _ => return None,
        })
    }
}

/// Durable progress of one required participant (§8).
///
/// Every non-[`Self::Pending`] variant names the sweep it refers to, so a
/// report from an older generation is distinguishable from the current state
/// and can never advance it (§6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantProgress {
    Pending,
    Running {
        sweep: DeletionSweepGeneration,
    },
    LocalComplete {
        sweep: DeletionSweepGeneration,
    },
    Verified {
        sweep: DeletionSweepGeneration,
    },
    Held {
        sweep: DeletionSweepGeneration,
        reason: ParticipantHoldClass,
    },
}

impl ParticipantProgress {
    /// The sweep this progress refers to, or [`None`] while pending.
    #[must_use]
    pub const fn sweep(self) -> Option<DeletionSweepGeneration> {
        match self {
            Self::Pending => None,
            Self::Running { sweep }
            | Self::LocalComplete { sweep }
            | Self::Verified { sweep }
            | Self::Held { sweep, .. } => Some(sweep),
        }
    }

    /// The hold reason, if held.
    #[must_use]
    pub const fn hold_reason(self) -> Option<ParticipantHoldClass> {
        match self {
            Self::Held { reason, .. } => Some(reason),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_verified(self) -> bool {
        matches!(self, Self::Verified { .. })
    }
}

/// Bounded work scope for one participant demand (§9).
///
/// The scope is bounded by construction: it carries, for a local owner, the
/// operation's protected exact-text material. Exhaustive covered-source
/// identities stay in the canonical `(operation, sweep, source)` primary key;
/// the command does not materialize that set. The material is never rendered
/// into logs, never copied into a completion fact, and is absent from a
/// correlation-only scope — the shape a Client-bound projection must use,
/// because the wire never carries the target body or search material
/// (§8.1, IPC §23).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantErasureScope {
    target: Option<TargetedDeletionTarget>,
    sources: Vec<RawId>,
}

impl ParticipantErasureScope {
    /// In-process scope for a local semantic owner: the owner may hold the
    /// body, so it receives the protected material. Covered-source membership
    /// is the canonical indexed table, not a snapshot copied into this value.
    #[must_use]
    pub fn local(target: TargetedDeletionTarget, sources: Vec<RawId>) -> Self {
        Self {
            target: Some(target),
            sources,
        }
    }

    /// Body-free scope for a holder whose transport must not carry target
    /// bodies or search material (a Client-partition demand, §8.1). The Host
    /// maps its own state to these minimal correlation identities.
    #[must_use]
    pub fn correlation_only(sources: Vec<RawId>) -> Self {
        Self {
            target: None,
            sources,
        }
    }

    /// Protected material, absent for a correlation-only scope.
    #[must_use]
    pub fn target(&self) -> Option<&TargetedDeletionTarget> {
        self.target.as_ref()
    }

    /// Optional caller-supplied correlation identities. Production fan-out
    /// leaves this empty: owners probe `erasure_condition_source` by primary
    /// key for each candidate they actually inspect.
    #[must_use]
    pub fn sources(&self) -> &[RawId] {
        &self.sources
    }
}

/// One bounded local-erasure demand from the Host fan-out to one participant
/// (§9). The command is idempotent for its `(condition, participant)` pair: a
/// participant that crashed after erasing but before reporting can be demanded
/// again in the same sweep without a second semantic effect (§9.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemandLocalErasureCommand {
    condition: ErasureConditionRef,
    participant: ParticipantOwnerRef,
    scope: ParticipantErasureScope,
}

impl DemandLocalErasureCommand {
    #[must_use]
    pub fn new(
        condition: ErasureConditionRef,
        participant: ParticipantOwnerRef,
        scope: ParticipantErasureScope,
    ) -> Self {
        Self {
            condition,
            participant,
            scope,
        }
    }

    /// Operation and sweep the demand belongs to.
    #[must_use]
    pub fn condition(&self) -> ErasureConditionRef {
        self.condition
    }

    #[must_use]
    pub fn participant(&self) -> ParticipantOwnerRef {
        self.participant
    }

    #[must_use]
    pub fn scope(&self) -> &ParticipantErasureScope {
        &self.scope
    }
}

/// Participant completion status (§9). [`Self::LocalComplete`] is the bounded
/// erase pass being finished for this participant; [`Self::Verified`] is a
/// zero-remainder confirmation for the same sweep. Neither is global
/// completion (§10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantCompletionStatus {
    MoreWork,
    LocalComplete,
    Verified,
    Held(ParticipantHoldClass),
}

/// One participant completion report (§9).
///
/// Fields are sealed behind constructors so an invalid fact cannot be
/// assembled: [`ParticipantCompletionFact::verified`] always reports zero
/// remainder, and a held or partial report carries a hold class or a partial
/// count instead of a verification claim. The fact never carries target
/// bodies, search material, credential values, or prompt/output text (§9.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantCompletionFact {
    condition: ErasureConditionRef,
    participant: ParticipantOwnerRef,
    status: ParticipantCompletionStatus,
    erased_count: u64,
    remainder_count: u64,
    observed_at: WallClockWithTz,
}

impl ParticipantCompletionFact {
    /// Bounded work remains after this call; the participant owns its
    /// continuation and reports counts observed so far.
    #[must_use]
    pub fn more_work(
        condition: ErasureConditionRef,
        participant: ParticipantOwnerRef,
        erased_count: u64,
        remainder_count: u64,
        observed_at: WallClockWithTz,
    ) -> Self {
        Self {
            condition,
            participant,
            status: ParticipantCompletionStatus::MoreWork,
            erased_count,
            remainder_count,
            observed_at,
        }
    }

    /// The participant finished its bounded erase pass for this sweep; the
    /// remainder check is still outstanding.
    #[must_use]
    pub fn local_complete(
        condition: ErasureConditionRef,
        participant: ParticipantOwnerRef,
        erased_count: u64,
        remainder_count: u64,
        observed_at: WallClockWithTz,
    ) -> Self {
        Self {
            condition,
            participant,
            status: ParticipantCompletionStatus::LocalComplete,
            erased_count,
            remainder_count,
            observed_at,
        }
    }

    /// The participant's bounded remainder check found nothing for this sweep.
    /// A verified fact always reports zero remainder.
    #[must_use]
    pub fn verified(
        condition: ErasureConditionRef,
        participant: ParticipantOwnerRef,
        erased_count: u64,
        observed_at: WallClockWithTz,
    ) -> Self {
        Self {
            condition,
            participant,
            status: ParticipantCompletionStatus::Verified,
            erased_count,
            remainder_count: 0,
            observed_at,
        }
    }

    /// The participant cannot currently complete, with a durable hold class.
    /// A hold carries no usable counts, so the durable row keeps its last
    /// reported counts instead of erasing them.
    #[must_use]
    pub fn held(
        condition: ErasureConditionRef,
        participant: ParticipantOwnerRef,
        reason: ParticipantHoldClass,
        observed_at: WallClockWithTz,
    ) -> Self {
        Self {
            condition,
            participant,
            status: ParticipantCompletionStatus::Held(reason),
            erased_count: 0,
            remainder_count: 0,
            observed_at,
        }
    }

    #[must_use]
    pub fn condition(&self) -> ErasureConditionRef {
        self.condition
    }

    #[must_use]
    pub fn participant(&self) -> ParticipantOwnerRef {
        self.participant
    }

    #[must_use]
    pub fn status(&self) -> ParticipantCompletionStatus {
        self.status
    }

    #[must_use]
    pub fn erased_count(&self) -> u64 {
        self.erased_count
    }

    #[must_use]
    pub fn remainder_count(&self) -> u64 {
        self.remainder_count
    }

    #[must_use]
    pub fn observed_at(&self) -> WallClockWithTz {
        self.observed_at
    }
}

/// Protected operation-lifetime material for one fan-out pass (§3.1).
///
/// The material exists only while the operation is unfinished and is destroyed
/// before global completion; it is never persisted by a participant and never
/// copied into a completion fact or audit record. Covered-source identities
/// are not loaded into this value: the current sweep's
/// `erasure_condition_source` primary key is the membership authority, so a
/// material read stays bounded in the target/hint rows rather than in the
/// whole covered-source set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionOperationMaterial {
    target: TargetedDeletionTarget,
    sources: Vec<RawId>,
}

impl DeletionOperationMaterial {
    #[must_use]
    pub fn new(target: TargetedDeletionTarget, sources: Vec<RawId>) -> Self {
        Self { target, sources }
    }

    /// Protected exact-text and semantic-hint material.
    #[must_use]
    pub fn target(&self) -> &TargetedDeletionTarget {
        &self.target
    }

    /// Optional correlation identities. The canonical store's material read
    /// does not populate this: membership is an indexed probe per candidate.
    #[must_use]
    pub fn sources(&self) -> &[RawId] {
        &self.sources
    }
}

/// Outcome of the protected material read used to build fan-out commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeletionMaterialOutcome {
    /// Current-sweep material; the body never leaves the Host process.
    Material(DeletionOperationMaterial),
    /// No such operation.
    Missing,
    /// The operation's protected material was already destroyed: the
    /// finalizing wipe or the completed commit ran (§3.1/§12). A protected
    /// read never resurrects it.
    Destroyed,
}

/// One durable participant row: the required snapshot entry plus its current
/// progress (§8, PR Group J).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionParticipantRecord {
    pub participant: DeletionParticipantRef,
    pub progress: ParticipantProgress,
    pub erased_count: u64,
    pub remainder_count: u64,
    pub reported_at: Option<WallClockWithTz>,
}

/// Durable outcome of one bounded participant demand (the command side).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantDemandOutcome {
    /// The participant is now `Running` for the operation's current sweep.
    Marked(ParticipantProgress),
    /// The participant is already `Verified` for the current sweep; the
    /// command is an idempotent no-op.
    AlreadyVerified,
    /// No such operation.
    Missing,
    /// The operation already completed; no participant work remains.
    Completed,
    /// The owner is not part of this operation's required snapshot. A demand
    /// never registers a participant lazily.
    NotRequired,
    /// The operation's sweep has moved on since the command was minted; the
    /// report must not update current state.
    StaleSweep,
}

/// Durable outcome of one participant completion fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantCompletionOutcome {
    /// The fact was applied to the current-sweep row.
    Recorded(ParticipantProgress),
    /// The row already reached `Verified` for the current sweep, so a
    /// downgrading report cannot change it (§10: verification is terminal for
    /// the sweep).
    AlreadyVerified,
    Missing,
    Completed,
    NotRequired,
    StaleSweep,
}

/// Cross-cutting participant boundary owned by preservation; each semantic
/// owner implements it and the Host composition registers it (§9).
///
/// Contracts:
///
/// - The implementation erases and verifies only its own state and never
///   updates another domain's rows.
/// - Calls are bounded work. Continuation state (including any cursor) belongs
///   to the participant, so the same `(condition, participant)` demand may be
///   repeated after a crash without a second semantic effect (§9.1).
/// - The returned fact carries metadata only.
///
/// The method returns a boxed future (not an `async fn`) so the trait stays
/// object-safe: the composition holds heterogeneous implementations behind
/// `Arc<dyn ErasureParticipant>`.
pub trait ErasureParticipant: Send + Sync {
    /// The exact owner this implementation serves. One owner has at most one
    /// implementation per composition.
    fn owner(&self) -> ParticipantOwnerRef;

    fn demand_local_erasure(
        &self,
        command: DemandLocalErasureCommand,
    ) -> Pin<Box<dyn std::future::Future<Output = ParticipantCompletionFact> + Send + '_>>;
}
