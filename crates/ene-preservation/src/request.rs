use ene_primitive::{RawId, WallClockWithTz};

use crate::{
    DeletionOperationRef, DeletionPurpose, MechanicalDeletionTarget, ParticipantOwnerRef,
    StartTargetedDeletionCommand, TargetedDeletionTarget,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeletionRequestId(RawId);

impl DeletionRequestId {
    #[must_use]
    pub fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetedDeletionRequest {
    request: DeletionRequestId,
    target: TargetedDeletionTarget,
    purpose: DeletionPurpose,
}

impl TargetedDeletionRequest {
    #[must_use]
    pub fn from_durable(
        request: DeletionRequestId,
        target: TargetedDeletionTarget,
        purpose: DeletionPurpose,
    ) -> Self {
        Self {
            request,
            target,
            purpose,
        }
    }

    #[must_use]
    pub fn request(&self) -> DeletionRequestId {
        self.request
    }

    #[must_use]
    pub fn purpose(&self) -> DeletionPurpose {
        self.purpose
    }

    #[must_use]
    pub fn owner_review_text(&self) -> &str {
        let MechanicalDeletionTarget::ExactText(material) = &self.target.mechanical;
        material.expose_for_erasure()
    }

    #[must_use]
    pub fn into_command(
        self,
        confirmation: OwnerConfirmationFact,
        admitted_at: WallClockWithTz,
        required_participants: Vec<ParticipantOwnerRef>,
    ) -> Option<StartTargetedDeletionCommand> {
        if confirmation.request != self.request {
            return None;
        }
        Some(StartTargetedDeletionCommand::confirmed(
            self.target,
            self.purpose,
            admitted_at,
            required_participants,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerConfirmationFact {
    request: DeletionRequestId,
}

impl OwnerConfirmationFact {
    #[must_use]
    pub fn from_durable(request: DeletionRequestId) -> Self {
        Self { request }
    }
}

#[derive(Debug, Clone)]
pub struct StageTargetedDeletionRequestCommand {
    target: TargetedDeletionTarget,
    purpose: DeletionPurpose,
    requested_at: WallClockWithTz,
}

impl StageTargetedDeletionRequestCommand {
    #[must_use]
    pub fn new(
        target: TargetedDeletionTarget,
        purpose: DeletionPurpose,
        requested_at: WallClockWithTz,
    ) -> Self {
        Self {
            target,
            purpose,
            requested_at,
        }
    }

    #[must_use]
    pub fn target(&self) -> &TargetedDeletionTarget {
        &self.target
    }

    #[must_use]
    pub fn purpose(&self) -> DeletionPurpose {
        self.purpose
    }

    #[must_use]
    pub fn requested_at(&self) -> WallClockWithTz {
        self.requested_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageTargetedDeletionRequestOutcome {
    Staged(DeletionRequestId),
    AlreadyStaged(DeletionRequestId),
    Confirmed(DeletionRequestId),
    AlreadyCoveredBy(DeletionOperationRef),
    HeldByOperation(DeletionOperationRef),
    NeedsClarification,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmTargetedDeletionOutcome {
    Started(DeletionOperationRef),
    AlreadyCoveredBy(DeletionOperationRef),
    HeldByOperation(DeletionOperationRef),
    Missing,
    NeedsClarification,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionSurfaceMark(String);

impl DeletionSurfaceMark {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use ene_primitive::{RawId, WallClockWithTz};

    use super::{
        DeletionRequestId, OwnerConfirmationFact, ParticipantOwnerRef,
        StageTargetedDeletionRequestCommand, TargetedDeletionRequest,
    };
    use crate::{
        DeletionPurpose, DeletionSearchMaterial, MechanicalDeletionTarget, TargetedDeletionTarget,
    };

    fn target(text: &str, hint: &str) -> TargetedDeletionTarget {
        TargetedDeletionTarget {
            mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                text.to_owned(),
            )),
            semantic_hints: vec![DeletionSearchMaterial::new(hint.to_owned())],
        }
    }

    fn staged() -> TargetedDeletionRequest {
        TargetedDeletionRequest::from_durable(
            DeletionRequestId::from_raw(RawId::new()),
            target("private target", "private hint"),
            DeletionPurpose::Privacy,
        )
    }

    #[test]
    fn confirmation_is_bound_to_its_own_request_identity() {
        let request = staged();
        let identity = request.request();
        let command = request
            .clone()
            .into_command(
                OwnerConfirmationFact::from_durable(identity),
                WallClockWithTz::now(),
                vec![
                    ParticipantOwnerRef::Companion,
                    ParticipantOwnerRef::Learning,
                ],
            )
            .expect("the matching confirmation must mint the command");
        assert_eq!(command.purpose(), DeletionPurpose::Privacy);

        let foreign =
            OwnerConfirmationFact::from_durable(DeletionRequestId::from_raw(RawId::new()));
        assert!(
            request
                .into_command(foreign, WallClockWithTz::now(), Vec::new())
                .is_none(),
            "a confirmation never transfers to another request"
        );
    }

    #[test]
    fn staged_scope_and_command_material_are_absent_from_debug() {
        let request = staged();
        let request_debug = format!("{request:?}");
        assert!(!request_debug.contains("private target"));
        assert!(!request_debug.contains("private hint"));

        let command = StageTargetedDeletionRequestCommand::new(
            target("secret staging body", "secret staging hint"),
            DeletionPurpose::Security,
            WallClockWithTz::now(),
        );
        assert_eq!(command.purpose(), DeletionPurpose::Security);
        let command_debug = format!("{command:?}");
        assert!(!command_debug.contains("secret staging body"));
        assert!(!command_debug.contains("secret staging hint"));
    }
}
