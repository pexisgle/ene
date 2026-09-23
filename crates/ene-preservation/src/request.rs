//! Targeted Deletion first-party request / confirmation / status surface.
//!
//! [Targeted Deletion Lifecycle](../../../docs/design/concrete/targeted-deletion-lifecycle.md)
//! §15 puts the wire intent, the trusted Host-local confirmation, and the
//! bounded status view in one first-party management boundary; §4 fixes the
//! admission premise: an explicit privacy/security purpose **and** a trusted
//! Owner confirmation.
//!
//! Types here keep those two apart:
//!
//! - [`TargetedDeletionRequest`] is a staged, still harmless request. The wire
//!   can legitimately produce one; it can start nothing.
//! - [`OwnerConfirmationFact`] is the durable Owner decision read from the
//!   Host-local confirmation journal (IPC §18.1). It has no wire form.
//! - [`TargetedDeletionRequest::into_command`] is the only production mint
//!   site of the admission command; it needs both the staged request and its
//!   durable confirmation fact, and the confirmation binds to the request
//!   identity, so it can never be transferred to another target or purpose.
//!   The store re-reads the confirmation row inside the same transaction that
//!   commits the operation.
//!
//! No Client payload, LLM output, or Task Agent text can construct any of
//! these: there is no `Deserialize`, no public literal constructor, and no
//! caller boolean anywhere on the path.

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

    /// Protected target text for the Host-local Owner review surface (IPC
    /// §18.1 preview): the trusted console shows exactly what would be
    /// deleted before the Owner confirms. Never a log, `Debug`, or wire
    /// representation.
    #[must_use]
    pub fn owner_review_text(&self) -> &str {
        let MechanicalDeletionTarget::ExactText(material) = &self.target.mechanical;
        material.expose_for_erasure()
    }

    /// The admission command this staged request becomes once the Owner
    /// confirmed it on the Host-local trusted surface.
    ///
    /// Returns [`None`] when `confirmation` names a different request: a
    /// confirmation never transfers to another target, purpose, or request
    /// identity. `admitted_at` is the commit-time premise the operation and
    /// its current erasure condition open with.
    /// `required_participants` is the current product surface's owner set the
    /// Host composition decided on (lifecycle §8); the admission transaction
    /// snapshots it durably with the operation.
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
    /// Store-read construction for one durable confirmation row.
    ///
    /// Callers must have read and validated this exact row from the
    /// confirmation journal; the admission transaction re-reads and re-checks
    /// it before any operation row is committed.
    #[must_use]
    pub fn from_durable(request: DeletionRequestId) -> Self {
        Self { request }
    }
}

/// Advisory staging input.
///
/// The mechanical target comes from the Host's parse of the wire grammar. The
/// request identity is minted by the store; source correlations are
/// Host-derived, never Client-supplied (lifecycle §4). A `semantic_hints` entry
/// is usable only for the staging duplicate-scope decision, and the staged
/// request journal re-derives the mechanical target and keeps no hints.
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
    use crate::{DeletionPurpose, MechanicalDeletionTarget, TargetedDeletionTarget};

    fn target(text: &str) -> TargetedDeletionTarget {
        TargetedDeletionTarget {
            mechanical: MechanicalDeletionTarget::ExactText(crate::DeletionSearchMaterial::new(
                text.to_owned(),
            )),
            semantic_hints: Vec::new(),
        }
    }

    fn staged() -> TargetedDeletionRequest {
        TargetedDeletionRequest::from_durable(
            DeletionRequestId::from_raw(RawId::new()),
            target("private target"),
            DeletionPurpose::Privacy,
        )
    }

    #[test]
    fn staged_request_debug_never_renders_the_target_body() {
        let request = staged();
        let rendered = format!("{request:?}");
        assert!(
            !rendered.contains("private target"),
            "the staged target never renders through Debug: {rendered}"
        );
    }

    #[test]
    fn confirmation_binds_to_its_own_request_identity() {
        let request = staged();
        let identity = request.request();
        let fact = OwnerConfirmationFact::from_durable(identity);
        let command = request
            .clone()
            .into_command(
                fact,
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
    fn staging_command_exposes_no_confirmation() {
        let command = StageTargetedDeletionRequestCommand::new(
            target("secret"),
            DeletionPurpose::Security,
            WallClockWithTz::now(),
        );
        assert_eq!(command.purpose(), DeletionPurpose::Security);
        assert!(
            !format!("{command:?}").contains("secret"),
            "the staging input is as protected as the request"
        );
    }
}
