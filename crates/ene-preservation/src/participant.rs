use std::pin::Pin;

use ene_primitive::{RawId, WallClockWithTz};

use crate::{
    DeletionOperationId, DeletionSweepGeneration, ErasureConditionRef, TargetedDeletionTarget,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParticipantOwnerRef {
    Companion,
    Learning,
    Task,
    Action,
    Inference,
    Permission,
    Credential,
    Presence,
    HostTransient,
    ClientIncarnation(RawId),
}

impl ParticipantOwnerRef {
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

    #[must_use]
    pub const fn is_incarnation(self) -> bool {
        matches!(self, Self::ClientIncarnation(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeletionParticipantRef {
    pub operation: DeletionOperationId,
    pub owner: ParticipantOwnerRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParticipantHoldClass {
    Unavailable,
    Unsupported,
    Failed,
}

impl ParticipantHoldClass {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Unsupported => "unsupported",
            Self::Failed => "failed",
        }
    }

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantErasureScope {
    target: Option<TargetedDeletionTarget>,
    sources: Vec<RawId>,
}

impl ParticipantErasureScope {
    #[must_use]
    pub fn local(target: TargetedDeletionTarget, sources: Vec<RawId>) -> Self {
        Self {
            target: Some(target),
            sources,
        }
    }

    #[must_use]
    pub fn correlation_only(sources: Vec<RawId>) -> Self {
        Self {
            target: None,
            sources,
        }
    }

    #[must_use]
    pub fn target(&self) -> Option<&TargetedDeletionTarget> {
        self.target.as_ref()
    }

    #[must_use]
    pub fn sources(&self) -> &[RawId] {
        &self.sources
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantCompletionStatus {
    MoreWork,
    LocalComplete,
    Verified,
    Held(ParticipantHoldClass),
}

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

    #[must_use]
    pub fn target(&self) -> &TargetedDeletionTarget {
        &self.target
    }

    #[must_use]
    pub fn sources(&self) -> &[RawId] {
        &self.sources
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeletionMaterialOutcome {
    Material(DeletionOperationMaterial),
    Missing,
    Destroyed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionParticipantRecord {
    pub participant: DeletionParticipantRef,
    pub progress: ParticipantProgress,
    pub erased_count: u64,
    pub remainder_count: u64,
    pub reported_at: Option<WallClockWithTz>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantDemandOutcome {
    Marked(ParticipantProgress),
    AlreadyVerified,
    Missing,
    Completed,
    NotRequired,
    StaleSweep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantCompletionOutcome {
    Recorded(ParticipantProgress),
    AlreadyVerified,
    Missing,
    Completed,
    NotRequired,
    StaleSweep,
}

pub trait ErasureParticipant: Send + Sync {
    fn owner(&self) -> ParticipantOwnerRef;

    fn demand_local_erasure(
        &self,
        command: DemandLocalErasureCommand,
    ) -> Pin<Box<dyn std::future::Future<Output = ParticipantCompletionFact> + Send + '_>>;
}
