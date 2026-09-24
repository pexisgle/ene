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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalErasurePass {
    NotCurrent,
    Applied { erased: u64, remainder: u64 },
}

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
        Err(_) => held(),
    }
}

pub trait ErasureParticipant: Send + Sync {
    fn owner(&self) -> ParticipantOwnerRef;

    fn demand_local_erasure(
        &self,
        command: DemandLocalErasureCommand,
    ) -> Pin<Box<dyn std::future::Future<Output = ParticipantCompletionFact> + Send + '_>>;
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Waker};

    use super::*;
    use crate::{DeletionSearchMaterial, TargetedDeletionTarget};
    use ene_primitive::RawId;

    const OWNER: ParticipantOwnerRef = ParticipantOwnerRef::Learning;

    fn condition() -> ErasureConditionRef {
        ErasureConditionRef {
            operation: DeletionOperationId::from_raw(RawId::new()),
            sweep: DeletionSweepGeneration::from_u64(2),
        }
    }

    fn local_scope(text: &str) -> ParticipantErasureScope {
        ParticipantErasureScope::local(TargetedDeletionTarget {
            mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                text.to_owned(),
            )),
            semantic_hints: Vec::new(),
        })
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        let mut future = Box::pin(future);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
            std::thread::yield_now();
        }
    }

    fn drive(
        condition: ErasureConditionRef,
        scope: ParticipantErasureScope,
        outcome: Result<LocalErasurePass, ()>,
    ) -> (
        ParticipantCompletionFact,
        Option<(ErasureConditionRef, String)>,
    ) {
        let observed = Arc::new(Mutex::new(None));
        let sink = Arc::clone(&observed);
        let fact = block_on(drive_local_erasure(
            OWNER,
            DemandLocalErasureCommand::new(condition, OWNER, scope),
            move |actual, text| {
                let sink = Arc::clone(&sink);
                let observed_value = (actual, text.to_owned());
                Box::pin(async move {
                    *sink.lock().expect("observation lock") = Some(observed_value);
                    outcome
                })
            },
        ));
        let observed = observed.lock().expect("observation lock").clone();
        (fact, observed)
    }

    fn assert_identity(fact: &ParticipantCompletionFact, condition: ErasureConditionRef) {
        assert_eq!(fact.condition(), condition);
        assert_eq!(fact.participant(), OWNER);
    }

    #[test]
    fn generic_local_erasure_driver_preserves_holds_progress_and_verification() {
        let condition = condition();
        let target = "private target";
        let command = DemandLocalErasureCommand::new(
            condition,
            OWNER,
            ParticipantErasureScope::local(TargetedDeletionTarget {
                mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                    String::from(target),
                )),
                semantic_hints: vec![DeletionSearchMaterial::new(String::from(
                    "private semantic hint",
                ))],
            }),
        );
        let command_debug = format!("{command:?}");
        assert!(!command_debug.contains(target));
        assert!(!command_debug.contains("private semantic hint"));

        for scope in [
            ParticipantErasureScope::correlation_only(),
            local_scope(" "),
        ] {
            let (fact, observed) = drive(
                condition,
                scope,
                Ok(LocalErasurePass::Applied {
                    erased: 0,
                    remainder: 0,
                }),
            );
            assert_eq!(
                fact.status(),
                ParticipantCompletionStatus::Held(ParticipantHoldClass::Failed)
            );
            assert_eq!((fact.erased_count(), fact.remainder_count()), (0, 0));
            assert_identity(&fact, condition);
            assert_eq!(observed, None);
        }

        let (fact, observed) = drive(condition, local_scope(target), Err(()));
        assert_eq!(
            fact.status(),
            ParticipantCompletionStatus::Held(ParticipantHoldClass::Failed)
        );
        assert_eq!(
            observed,
            Some((condition, String::from(target))),
            "a storage failure still preserves its exact condition and target"
        );
        assert_identity(&fact, condition);

        let (fact, observed) = drive(
            condition,
            local_scope(target),
            Ok(LocalErasurePass::NotCurrent),
        );
        assert_eq!(fact.status(), ParticipantCompletionStatus::LocalComplete);
        assert_eq!((fact.erased_count(), fact.remainder_count()), (0, 0));
        assert_eq!(observed, Some((condition, String::from(target))));
        assert_identity(&fact, condition);
        assert!(!matches!(
            fact.status(),
            ParticipantCompletionStatus::Verified
        ));

        let (fact, observed) = drive(
            condition,
            local_scope(target),
            Ok(LocalErasurePass::Applied {
                erased: 0,
                remainder: 0,
            }),
        );
        assert_eq!(fact.status(), ParticipantCompletionStatus::Verified);
        assert_eq!((fact.erased_count(), fact.remainder_count()), (0, 0));
        assert_eq!(observed, Some((condition, String::from(target))));
        assert_identity(&fact, condition);

        let (fact, observed) = drive(
            condition,
            local_scope(target),
            Ok(LocalErasurePass::Applied {
                erased: 1,
                remainder: 2,
            }),
        );
        assert_eq!(fact.status(), ParticipantCompletionStatus::MoreWork);
        assert_eq!((fact.erased_count(), fact.remainder_count()), (1, 2));
        assert_eq!(observed, Some((condition, String::from(target))));
        assert_identity(&fact, condition);
        assert!(!format!("{fact:?}").contains(target));
    }
}
