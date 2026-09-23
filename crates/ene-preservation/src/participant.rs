use std::pin::Pin;

use ene_primitive::{RawId, WallClockWithTz};

use crate::{
    DeletionOperationId, DeletionSweepGeneration, ErasureConditionRef, MechanicalDeletionTarget,
    TargetedDeletionTarget,
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
            Self::ClientIncarnation(instance) => format!(
                "{}:{}",
                self.class_name(),
                instance.as_uuid().as_hyphenated()
            ),
            _ => self.class_name().to_owned(),
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
    pub const fn is_verified(self) -> bool {
        matches!(self, Self::Verified { .. })
    }

    /// Rebuilds one progress row from its storage state token, sweep, and hold
    /// class. The hold class is present exactly for `held`; any other
    /// combination is outside the closed vocabulary and answers [`None`].
    #[must_use]
    pub fn from_state_name(
        state: &str,
        sweep: DeletionSweepGeneration,
        hold: Option<ParticipantHoldClass>,
    ) -> Option<Self> {
        Some(match (state, hold) {
            ("pending", None) => Self::Pending,
            ("running", None) => Self::Running { sweep },
            ("local_complete", None) => Self::LocalComplete { sweep },
            ("verified", None) => Self::Verified { sweep },
            ("held", Some(reason)) => Self::Held { sweep, reason },
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantErasureScope {
    target: Option<TargetedDeletionTarget>,
}

impl ParticipantErasureScope {
    #[must_use]
    pub fn local(target: TargetedDeletionTarget) -> Self {
        Self {
            target: Some(target),
        }
    }

    /// Body-free scope for a holder whose transport must not carry target
    /// bodies or search material (a Client-partition demand, §8.1). The Host
    /// maps its own state to this body-free shape.
    #[must_use]
    pub fn correlation_only() -> Self {
        Self { target: None }
    }

    #[must_use]
    pub fn target(&self) -> Option<&TargetedDeletionTarget> {
        self.target.as_ref()
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
}

impl DeletionOperationMaterial {
    #[must_use]
    pub fn new(target: TargetedDeletionTarget) -> Self {
        Self { target }
    }

    #[must_use]
    pub fn target(&self) -> &TargetedDeletionTarget {
        &self.target
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

/// Outcome of one bounded owner-local erasure pass (§9).
///
/// Each participant maps its own storage result onto this vocabulary;
/// [`drive_local_erasure`] turns it into the completion fact the durable
/// participant record accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalErasurePass {
    /// The condition is not the operation's current unfinished condition (a
    /// superseded sweep or a completed operation): nothing was read or
    /// mutated, and no verification is claimed for it.
    NotCurrent,
    /// The bounded pass ran: `erased` target-bearing values were removed,
    /// redacted, or invalidated, and `remainder` target-bearing values remain.
    Applied { erased: u64, remainder: u64 },
}

/// Drives one bounded owner-local erasure demand into a completion fact.
///
/// Text-owning participants share this envelope: a correlation-only or blank
/// demand is an explicit hold rather than a verification, and only a
/// zero-remainder [`LocalErasurePass::Applied`] is a verification claim (§9).
/// The `erase` callback runs the owner's own bounded pass over the protected
/// exact-text material; it must be idempotent for the same
/// `(condition, target)` and must not mutate anything when the condition is
/// not current.
pub async fn drive_local_erasure<F, E>(
    owner: ParticipantOwnerRef,
    command: DemandLocalErasureCommand,
    erase: F,
) -> ParticipantCompletionFact
where
    F: for<'a> FnOnce(
            ErasureConditionRef,
            &'a str,
        ) -> Pin<
            Box<dyn std::future::Future<Output = Result<LocalErasurePass, E>> + Send + 'a>,
        > + Send,
    E: 'static,
{
    let condition = command.condition();
    let held = || {
        ParticipantCompletionFact::held(
            condition,
            owner,
            ParticipantHoldClass::Failed,
            WallClockWithTz::now(),
        )
    };
    let Some(target) = command.scope().target() else {
        return held();
    };
    let MechanicalDeletionTarget::ExactText(material) = &target.mechanical;
    let text = material.expose_for_erasure();
    if text.trim().is_empty() {
        return held();
    }
    match erase(condition, text).await {
        // Not-current work mutates nothing and cannot verify the demanded
        // condition; the durable record refuses it as stale or completed, so
        // the pass ends without a false completion.
        Ok(LocalErasurePass::NotCurrent) => ParticipantCompletionFact::local_complete(
            condition,
            owner,
            0,
            0,
            WallClockWithTz::now(),
        ),
        Ok(LocalErasurePass::Applied {
            erased,
            remainder: 0,
        }) => ParticipantCompletionFact::verified(condition, owner, erased, WallClockWithTz::now()),
        Ok(LocalErasurePass::Applied { erased, remainder }) => {
            ParticipantCompletionFact::more_work(
                condition,
                owner,
                erased,
                remainder,
                WallClockWithTz::now(),
            )
        }
        // A storage failure is a hold, never a completion: the residual
        // target-bearing state is unproven, so the operation stays
        // retryable-incomplete.
        Err(_) => held(),
    }
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
    fn owner(&self) -> ParticipantOwnerRef;

    fn demand_local_erasure(
        &self,
        command: DemandLocalErasureCommand,
    ) -> Pin<Box<dyn std::future::Future<Output = ParticipantCompletionFact> + Send + '_>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DeletionSearchMaterial, MechanicalDeletionTarget};
    use ene_primitive::RawId;

    fn condition() -> ErasureConditionRef {
        ErasureConditionRef {
            operation: DeletionOperationId::from_raw(RawId::new()),
            sweep: DeletionSweepGeneration::from_u64(2),
        }
    }

    #[test]
    fn owner_storage_names_round_trip_including_incarnations() {
        let owners = [
            ParticipantOwnerRef::Companion,
            ParticipantOwnerRef::Learning,
            ParticipantOwnerRef::Task,
            ParticipantOwnerRef::Action,
            ParticipantOwnerRef::Inference,
            ParticipantOwnerRef::Permission,
            ParticipantOwnerRef::Credential,
            ParticipantOwnerRef::Presence,
            ParticipantOwnerRef::HostTransient,
            ParticipantOwnerRef::ClientIncarnation(RawId::new()),
        ];
        for owner in owners {
            let stored = owner.storage_name();
            assert_eq!(
                ParticipantOwnerRef::from_storage_name(&stored),
                Some(owner),
                "{stored} must round-trip"
            );
        }
        let client = ParticipantOwnerRef::ClientIncarnation(RawId::new());
        assert!(client.is_incarnation());
        assert_eq!(client.class_name(), "client_incarnation");
        assert!(!ParticipantOwnerRef::Companion.is_incarnation());
        assert_eq!(
            ParticipantOwnerRef::from_storage_name("client_incarnation:not-a-uuid"),
            None,
            "a malformed incarnation identity is unreadable stored state"
        );
        assert_eq!(
            ParticipantOwnerRef::from_storage_name("unknown_owner"),
            None
        );
    }

    #[test]
    fn a_verified_fact_always_reports_zero_remainder() {
        let condition = condition();
        let participant = ParticipantOwnerRef::Companion;
        let at = WallClockWithTz::now();
        let verified = ParticipantCompletionFact::verified(condition, participant, 7, at);
        assert_eq!(verified.status(), ParticipantCompletionStatus::Verified);
        assert_eq!(verified.erased_count(), 7);
        assert_eq!(verified.remainder_count(), 0);
        assert_eq!(verified.condition(), condition);
        assert_eq!(verified.participant(), participant);
        assert_eq!(verified.observed_at(), at);
        let partial = ParticipantCompletionFact::local_complete(condition, participant, 3, 4, at);
        assert_eq!(partial.status(), ParticipantCompletionStatus::LocalComplete);
        assert_eq!(partial.erased_count(), 3);
        assert_eq!(partial.remainder_count(), 4);
        let held = ParticipantCompletionFact::held(
            condition,
            participant,
            ParticipantHoldClass::Unavailable,
            at,
        );
        assert_eq!(
            held.status(),
            ParticipantCompletionStatus::Held(ParticipantHoldClass::Unavailable)
        );
    }

    #[test]
    fn hold_classes_and_progress_keep_their_sweep() {
        assert_eq!(
            ParticipantHoldClass::from_name("unsupported"),
            Some(ParticipantHoldClass::Unsupported)
        );
        assert_eq!(ParticipantHoldClass::from_name("other"), None);
        assert_eq!(ParticipantProgress::Pending.sweep(), None);
        let sweep = DeletionSweepGeneration::from_u64(3);
        let held = ParticipantProgress::Held {
            sweep,
            reason: ParticipantHoldClass::Failed,
        };
        assert_eq!(held.sweep(), Some(sweep));
        assert!(!held.is_verified());
        assert!(ParticipantProgress::Verified { sweep }.is_verified());
    }

    #[test]
    fn scope_debug_never_renders_the_target_body() {
        let material = TargetedDeletionTarget {
            mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                "private-target".into(),
            )),
            semantic_hints: vec![DeletionSearchMaterial::new("private-hint".into())],
        };
        let command = DemandLocalErasureCommand::new(
            condition(),
            ParticipantOwnerRef::Learning,
            ParticipantErasureScope::local(material),
        );
        let rendered = format!("{command:?}");
        assert!(!rendered.contains("private-target"));
        assert!(!rendered.contains("private-hint"));
        let correlation_only = ParticipantErasureScope::correlation_only();
        assert!(correlation_only.target().is_none());
    }
}
