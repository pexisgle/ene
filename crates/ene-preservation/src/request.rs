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
//!   site of [`TrustedOwnerConfirmationRef`]; it needs both, and the
//!   confirmation binds to the request identity, so it can never be
//!   transferred to another target or purpose. The store re-reads the
//!   confirmation row inside the same transaction that commits the operation.
//!
//! No Client payload, LLM output, or Task Agent text can construct any of
//! these: there is no `Deserialize`, no public literal constructor, and no
//! caller boolean anywhere on the path.

use ene_primitive::{RawId, WallClockWithTz};

use crate::{
    DeletionOperationRef, DeletionPurpose, MechanicalDeletionTarget, ParticipantOwnerRef,
    StartTargetedDeletionCommand, TargetedDeletionTarget,
};

/// Identity of one staged Targeted Deletion request.
///
/// Minted by the preservation store when the request is staged. It never
/// travels the wire, so a Client cannot name — and therefore cannot confirm —
/// a request.
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

/// One staged request awaiting the Host-local trusted confirmation.
///
/// The target stays protected search material: it never renders through
/// `Debug` (see [`DeletionSearchMaterial`](crate::DeletionSearchMaterial)),
/// and it never enters an audit record, a management view, or a participant
/// fact. The Host-local confirmation inlet reads it to show the Owner the
/// concrete target, and the admission commit consumes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetedDeletionRequest {
    request: DeletionRequestId,
    target: TargetedDeletionTarget,
    purpose: DeletionPurpose,
    requested_at: WallClockWithTz,
}

impl TargetedDeletionRequest {
    /// Store-read construction for one durable request row.
    ///
    /// There is no wire constructor: the caller must have read this exact row
    /// from the request journal.
    #[must_use]
    pub fn from_durable(
        request: DeletionRequestId,
        target: TargetedDeletionTarget,
        purpose: DeletionPurpose,
        requested_at: WallClockWithTz,
    ) -> Self {
        Self {
            request,
            target,
            purpose,
            requested_at,
        }
    }

    #[must_use]
    pub fn request(&self) -> DeletionRequestId {
        self.request
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

    /// Protected target text for the Host-local Owner review surface (IPC
    /// §18.1 preview): the trusted console shows exactly what would be
    /// deleted before the Owner confirms. Never a log, `Debug`, or wire
    /// representation.
    #[must_use]
    pub fn owner_review_text(&self) -> &str {
        let MechanicalDeletionTarget::ExactText(material) = &self.target.mechanical;
        material.expose_for_owner_review()
    }

    /// The admission command this staged request becomes once the Owner
    /// confirmed it on the Host-local trusted surface.
    ///
    /// Returns [`None`] when `confirmation` names a different request: a
    /// confirmation never transfers to another target, purpose, or request
    /// identity. `admitted_at` is the commit-time premise the operation and
    /// its current erasure condition open with; the earlier request time is
    /// journal metadata and is never backdated into the condition.
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
            self.request.as_raw(),
            self.target,
            self.purpose,
            admitted_at,
            required_participants,
        ))
    }
}

/// One durable Owner confirmation, as read from the Host-local confirmation
/// journal.
///
/// Private fields and no wire representation: the only production producer is
/// the preservation store's confirmation read, written by the Host-local
/// trusted inlet (IPC §18.1). A Client payload, an LLM output, or a Task Agent
/// can neither name nor construct one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerConfirmationFact {
    request: DeletionRequestId,
    confirmed_at: WallClockWithTz,
}

impl OwnerConfirmationFact {
    /// Store-read construction for one durable confirmation row.
    ///
    /// Callers must have read this exact row from the confirmation journal;
    /// the admission transaction re-reads and re-checks it before any
    /// operation row is committed.
    #[must_use]
    pub fn from_durable(request: DeletionRequestId, confirmed_at: WallClockWithTz) -> Self {
        Self {
            request,
            confirmed_at,
        }
    }

    #[must_use]
    pub fn request(self) -> DeletionRequestId {
        self.request
    }

    #[must_use]
    pub fn confirmed_at(self) -> WallClockWithTz {
        self.confirmed_at
    }
}

/// Advisory staging input.
///
/// The mechanical target comes from the Host's parse of the wire grammar. The
/// request identity is minted by the store; source correlations and semantic
/// hints are never Client-supplied (lifecycle §4), so this command carries the
/// mechanical target only.
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
    /// A new staged request is durable, awaiting the trusted confirmation.
    /// This is not an admission: no condition is published.
    Staged(DeletionRequestId),
    /// The identical request (same mechanical target and purpose) was already
    /// staged; the same durable row is returned and nothing was written.
    AlreadyStaged(DeletionRequestId),
    /// This request is staged and its Owner confirmation is already durable,
    /// but no operation is committed yet (a crash between the confirmation
    /// and the admission). The caller runs the canonical confirmed admission;
    /// staging itself still starts nothing.
    Confirmed(DeletionRequestId),
    /// An unfinished operation with the same mechanical target already covers
    /// the request scope. Never a claim that the deletion completed.
    AlreadyCoveredBy(DeletionOperationRef),
    /// An unfinished operation with the same mechanical target exists but
    /// does not cover this request scope: a duplicate never silently widens a
    /// confirmed operation.
    HeldByOperation(DeletionOperationRef),
    /// The mechanical target is blank: not an admissible request scope.
    NeedsClarification,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmTargetedDeletionOutcome {
    /// The Owner's confirmation is durable and this request's canonical
    /// operation started. The current phase is read through the status view,
    /// never guessed here.
    Started(DeletionOperationRef),
    /// The Owner's confirmation is durable and no second operation exists:
    /// this request's single-use operation already exists, or an equivalent
    /// unfinished operation already covers its scope. This is an idempotency
    /// result, never a claim that the deletion completed — the status view
    /// carries the phase.
    AlreadyCoveredBy(DeletionOperationRef),
    /// The Owner's confirmation is durable and the canonical admission held.
    HeldByOperation(DeletionOperationRef),
    /// No staged request with this identity exists.
    Missing,
    /// The staged target is not admissible.
    NeedsClarification,
}

/// Display-revision mark of the deletion management surface (lifecycle §15,
/// IPC §18.2).
///
/// Derived from canonical rows on every read and never stored: staging a
/// request or admitting an operation changes it, as does the newest
/// operation's phase or sweep. Management intents compare their `base_view`
/// against it; a mismatch is re-read, never silently adopted. It is a
/// comparison mark, never authority.
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

impl core::fmt::Display for DeletionSurfaceMark {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.0)
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
            WallClockWithTz::now(),
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
        let fact = OwnerConfirmationFact::from_durable(identity, WallClockWithTz::now());
        assert_eq!(fact.request(), identity);
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
        assert!(
            command.is_confirmed(),
            "the minted command carries the sealed confirmation premise"
        );
        assert_eq!(command.purpose(), DeletionPurpose::Privacy);

        let foreign = OwnerConfirmationFact::from_durable(
            DeletionRequestId::from_raw(RawId::new()),
            WallClockWithTz::now(),
        );
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
