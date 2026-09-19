//! Targeted Deletion management surface (`Stage 6` A1b).
//!
//! [Targeted Deletion Lifecycle](../../../docs/design/concrete/targeted-deletion-lifecycle.md)
//! §15 and [IPC](../../../docs/design/concrete/host-client-ipc.md) §18 / §18.1
//! split this surface into three paths that must never collapse into one:
//!
//! - **Advisory wire intent** (`RequestDeletionBackupRestoreReset` with a
//!   `deletion:{purpose}:{exact-text}` target): the Host parses the grammar,
//!   re-validates the purpose and the current deletion surface mark, and stages
//!   a durable request. It answers
//!   [`NeedsClarification`](ene_api::v1::management::ManagementOutcome::NeedsClarification)
//!   (the Host PC still has to confirm) or
//!   [`HeldByOperation`](ene_api::v1::management::ManagementOutcome::HeldByOperation)
//!   when an equivalent deletion is already in progress. No wire payload can
//!   reach the admission: the confirmation type has no wire form.
//! - **Bounded status read** (`DeletionStatusRequest` /
//!   `DeletionStatusResponse`): one keyset-paged page of operation phase,
//!   purpose, hold class, current sweep, and participant progress, plus the
//!   surface mark an intent builds on. No target text, no search material, no
//!   credential, and no participant payload beyond the participant-owned
//!   progress tokens.
//! - **Host-local trusted inlet** (IPC §18.1): list staged requests, confirm
//!   one, read the same status page. `confirm_targeted_deletion` is the only
//!   path to a destructive admission, it writes the durable Owner
//!   confirmation, and it runs the canonical preservation producer. The Client
//!   never learns a request identity, so it cannot name — let alone confirm —
//!   one. The confirmation must execute in the serving process: the required
//!   participant snapshot includes every Client incarnation with durable
//!   body-delivery evidence, and the fan-out resolves its reachability from
//!   that process's connection table (lifecycle §8.1). The serving
//!   composition exposes it through [`crate::host_control`], and no offline
//!   path admits a confirmation.
//!
//! The management intent journal never stores the Owner's exact text: the
//! deletion fingerprint names the family and purpose only, and the staged
//! durable request is the single scope record. A reused intent id with a
//! different body therefore observes the first decision instead of adopting
//! new text.

use ene_api::v1::deletion::{
    DELETION_STATUS_CURSOR_PREFIX, DELETION_TARGET_PREFIX, DeletionHoldWire,
    DeletionOperationStatusView, DeletionParticipantReportWire, DeletionParticipantStatusWire,
    DeletionPhaseWire, DeletionPurposeWire, DeletionStatusPage, DeletionStatusRequest,
    DeletionStatusResponse, parse_deletion_target,
};
use ene_api::v1::management::{ManagementIntent, ManagementOutcome};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{DeletionOperationWireRef, DeletionStatusCursorWire, ViewMarkWire};
use ene_api::v1::reject::RejectKind;
use ene_permission::{IntentFingerprint, IntentOutcome};
use ene_plugin_ipc::WireFrame;
use ene_preservation::{
    ConfirmTargetedDeletionOutcome, DeletionLifecycleChange, DeletionLifecycleOutcome,
    DeletionOperationId, DeletionOperationPhase, DeletionOperationRecord, DeletionOperationRef,
    DeletionPurpose, DeletionRequestId, DeletionSearchMaterial, DeletionSweepGeneration,
    MechanicalDeletionTarget, PreservationRepository as _, StageTargetedDeletionRequestCommand,
    StageTargetedDeletionRequestOutcome, StartTargetedDeletionOutcome, TargetedDeletionRequest,
    TargetedDeletionTarget,
};
use ene_primitive::{RawId, WallClockWithTz};

use crate::presentation::{checked_limit, field_reject};
use crate::serve::{CoreError, HostHandle, LiveInput, outgoing_frame, reject_frame};
use crate::setup::outcome_frame;

/// Why one bounded status read could not answer a page.
///
/// Malformed wire usage stays a rejection or an explicit `Unavailable`
/// response; it is never rendered as an empty page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeletionStatusQueryError {
    InvalidLimit,
    InvalidCursor,
    Unavailable,
}

fn purpose_from_wire(purpose: DeletionPurposeWire) -> DeletionPurpose {
    match purpose {
        DeletionPurposeWire::Privacy => DeletionPurpose::Privacy,
        DeletionPurposeWire::Security => DeletionPurpose::Security,
    }
}

fn purpose_to_wire(purpose: DeletionPurpose) -> DeletionPurposeWire {
    match purpose {
        DeletionPurpose::Privacy => DeletionPurposeWire::Privacy,
        DeletionPurpose::Security => DeletionPurposeWire::Security,
    }
}

fn encode_operation(operation: DeletionOperationId) -> String {
    operation.as_raw().as_uuid().as_hyphenated().to_string()
}

fn parse_request_id(raw: &str) -> Option<DeletionRequestId> {
    let id = uuid::Uuid::parse_str(raw).ok()?;
    Some(DeletionRequestId::from_raw(RawId::from_uuid(id)))
}

fn parse_status_cursor(raw: &str) -> Option<DeletionOperationId> {
    let rest = raw.strip_prefix(DELETION_STATUS_CURSOR_PREFIX)?;
    let id = uuid::Uuid::parse_str(rest).ok()?;
    Some(DeletionOperationId::from_raw(RawId::from_uuid(id)))
}

fn mint_status_cursor(operation: DeletionOperationId) -> DeletionStatusCursorWire {
    DeletionStatusCursorWire(format!(
        "{DELETION_STATUS_CURSOR_PREFIX}{}",
        operation.as_raw().as_uuid().as_hyphenated()
    ))
}

/// Progress token of one durable participant row (lifecycle §8).
///
/// The tokens are the owner's own class names; the view closes no set of its
/// own and never interprets them.
fn progress_token(progress: ene_preservation::ParticipantProgress) -> String {
    match progress {
        ene_preservation::ParticipantProgress::Pending => String::from("pending"),
        ene_preservation::ParticipantProgress::Running { .. } => String::from("running"),
        ene_preservation::ParticipantProgress::LocalComplete { .. } => {
            String::from("local-complete")
        }
        ene_preservation::ParticipantProgress::Verified { .. } => String::from("verified"),
        ene_preservation::ParticipantProgress::Held { reason, .. } => {
            format!("held:{}", reason.as_str())
        }
    }
}

fn status_view(
    record: &DeletionOperationRecord,
    participants: DeletionParticipantReportWire,
) -> DeletionOperationStatusView {
    DeletionOperationStatusView {
        operation: DeletionOperationWireRef(encode_operation(record.current.operation)),
        phase: match record.phase {
            DeletionOperationPhase::Active => DeletionPhaseWire::Active,
            DeletionOperationPhase::Held => DeletionPhaseWire::Held,
            DeletionOperationPhase::Finalizing => DeletionPhaseWire::Finalizing,
            DeletionOperationPhase::Completed => DeletionPhaseWire::Completed,
        },
        purpose: purpose_to_wire(record.purpose),
        started_at: record.started_at.to_rfc3339(),
        sweep: record.current.sweep.as_u64(),
        hold: record.hold.map(|hold| match hold {
            ene_preservation::DeletionHoldReason::Unavailable => DeletionHoldWire::Unavailable,
            ene_preservation::DeletionHoldReason::GenerationExhausted => {
                DeletionHoldWire::GenerationExhausted
            }
        }),
        participants,
    }
}

impl HostHandle {
    /// Journal discriminator for the Targeted Deletion inlet (see
    /// [`Self::deletion_intent_fingerprint`]).
    const INTENT_KIND_DELETION: &str = "deletion-targeted";

    /// Body-free journal target for an inadmissible deletion inlet target.
    ///
    /// It names the inlet family only: even a target the grammar refuses may
    /// carry the Owner's text (an unknown purpose token, an over-long body),
    /// and the journal must never keep it.
    const DELETION_JOURNAL_FAMILY: &str = "deletion-family";

    /// Advisory staging fingerprint: the journal must never keep the Owner's
    /// exact text.
    ///
    /// It names the intent family and the declared purpose, and drops the
    /// rationale quote (the exact text is the scope, and the durable staged
    /// request is its single record). A reused intent id with a different body
    /// therefore observes the first decision instead of adopting new text; a
    /// new target requires a new intent id, exactly like every other command
    /// key.
    pub(crate) fn deletion_intent_fingerprint(
        intent: &ManagementIntent,
        purpose: DeletionPurposeWire,
    ) -> IntentFingerprint {
        Self::deletion_fingerprint_on(
            intent,
            format!("{DELETION_TARGET_PREFIX}{}", purpose.as_str()),
        )
    }

    /// The journal fingerprint of a `RequestDeletionBackupRestoreReset` target
    /// that is not an admissible deletion scope: family only, never the raw
    /// target body, never the rationale quote.
    fn inadmissible_deletion_fingerprint(intent: &ManagementIntent) -> IntentFingerprint {
        Self::deletion_fingerprint_on(intent, String::from(Self::DELETION_JOURNAL_FAMILY))
    }

    fn deletion_fingerprint_on(intent: &ManagementIntent, target: String) -> IntentFingerprint {
        IntentFingerprint {
            intent_id: intent.intent_id.0.as_hyphenated().to_string(),
            kind: Self::INTENT_KIND_DELETION.to_string(),
            target,
            base: intent.base_view.0.clone(),
            rationale_origin: Self::rationale_origin_name(intent.rationale.origin).to_string(),
            rationale_quote: None,
        }
    }

    /// Advisory Targeted Deletion request inlet (lifecycle §4, §15).
    ///
    /// The Client intent is a proposal: the Host re-parses the target grammar,
    /// re-checks the surface mark the intent was built on, and stages at most
    /// one durable request per scope. Nothing destructive can happen on this
    /// path — the Owner's final confirmation is a Host-local act (IPC §18.1).
    pub(crate) async fn targeted_deletion_intent(
        &self,
        frame: &WireFrame,
        intent: &ManagementIntent,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        let Some(request) = parse_deletion_target(&intent.target) else {
            // Backup / restore / reset targets have no producer in this
            // slice. Clarify, recorded under the inlet's discriminator so a
            // retried id observes one answer, instead of borrowing the
            // deletion grammar's authority. The recorded fingerprint is
            // body-free even here: an inadmissible target may still carry the
            // Owner's text.
            return vec![outcome_frame(
                frame,
                live,
                intent,
                self.record_decided(
                    Self::inadmissible_deletion_fingerprint(intent),
                    IntentOutcome::NeedsClarification,
                )
                .await,
            )];
        };
        let fingerprint = Self::deletion_intent_fingerprint(intent, request.purpose());
        if let Some(answer) = self
            .replay_or_hold(frame, live, intent, fingerprint.clone())
            .await
        {
            return answer;
        }
        // Currentness of the advisory premise: the Client must have built the
        // intent on the live deletion surface. A stale mark decides nothing
        // (and is not recorded), so the Client re-reads and retries.
        let mark = match self.store.deletion_surface_mark().await {
            Ok(mark) => mark,
            Err(_) => {
                return vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    ManagementOutcome::HeldByOperation,
                )];
            }
        };
        if intent.base_view.0 != mark.as_str() {
            return vec![outcome_frame(
                frame,
                live,
                intent,
                ManagementOutcome::StaleBaseView {
                    current: ViewMarkWire(mark.as_str().to_string()),
                },
            )];
        }
        let command = StageTargetedDeletionRequestCommand::new(
            TargetedDeletionTarget {
                mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                    request.exact_text().to_string(),
                )),
                // Exploration hints are an owner-side search aid with no wire
                // grammar in this slice; a Client can never widen the
                // confirmed scope through staging.
                semantic_hints: Vec::new(),
            },
            purpose_from_wire(request.purpose()),
            WallClockWithTz::now(),
        );
        match self.store.stage_targeted_deletion(command).await {
            Ok(
                StageTargetedDeletionRequestOutcome::Staged(_)
                | StageTargetedDeletionRequestOutcome::AlreadyStaged(_),
            ) => vec![outcome_frame(
                frame,
                live,
                intent,
                self.record_decided(fingerprint, IntentOutcome::NeedsClarification)
                    .await,
            )],
            Ok(StageTargetedDeletionRequestOutcome::Confirmed(request)) => {
                // The Owner already confirmed this durable request (a crash
                // between confirmation and admission). The intent adds no
                // authority; it only lets the canonical admission finish. An
                // unreadable evidence snapshot admits nothing: the intent
                // records no outcome so a later retry can still admit.
                let required = match self.required_deletion_participants().await {
                    Ok(required) => required,
                    Err(_) => {
                        return vec![outcome_frame(
                            frame,
                            live,
                            intent,
                            ManagementOutcome::HeldByOperation,
                        )];
                    }
                };
                match self
                    .store
                    .start_confirmed_targeted_deletion(request, required)
                    .await
                {
                    Ok(StartTargetedDeletionOutcome::Started(_)) => {
                        // The durable Owner confirmation already admitted this
                        // operation (a crash between confirmation and
                        // admission): the bounded kick starts the fan-out now
                        // instead of waiting for the next serving tick. It is
                        // best-effort and never a completion claim.
                        self.kick_targeted_deletion().await;
                        vec![outcome_frame(
                            frame,
                            live,
                            intent,
                            self.record_decided(fingerprint, IntentOutcome::AppliedAsOneTime)
                                .await,
                        )]
                    }
                    Ok(StartTargetedDeletionOutcome::NeedsClarification) => vec![outcome_frame(
                        frame,
                        live,
                        intent,
                        self.record_decided(fingerprint, IntentOutcome::NeedsClarification)
                            .await,
                    )],
                    Ok(
                        StartTargetedDeletionOutcome::AlreadyCoveredBy(_)
                        | StartTargetedDeletionOutcome::HeldByOperation(_),
                    ) => vec![outcome_frame(
                        frame,
                        live,
                        intent,
                        self.record_decided(fingerprint, IntentOutcome::HeldByOperation)
                            .await,
                    )],
                    // Not decided: a missing durable premise (or an
                    // unreadable store) records nothing, so a later retry can
                    // still admit.
                    Ok(StartTargetedDeletionOutcome::ConfirmationRequired) | Err(_) => {
                        vec![outcome_frame(
                            frame,
                            live,
                            intent,
                            ManagementOutcome::HeldByOperation,
                        )]
                    }
                }
            }
            Ok(
                StageTargetedDeletionRequestOutcome::AlreadyCoveredBy(_)
                | StageTargetedDeletionRequestOutcome::HeldByOperation(_),
            ) => vec![outcome_frame(
                frame,
                live,
                intent,
                // An equivalent deletion is already in progress. This answer
                // never means completed: the covering operation's phase is
                // read through the status view.
                self.record_decided(fingerprint, IntentOutcome::HeldByOperation)
                    .await,
            )],
            Ok(StageTargetedDeletionRequestOutcome::NeedsClarification) => vec![outcome_frame(
                frame,
                live,
                intent,
                self.record_decided(fingerprint, IntentOutcome::NeedsClarification)
                    .await,
            )],
            // Nothing was decided, so a retry is safe.
            Err(_) => vec![outcome_frame(
                frame,
                live,
                intent,
                ManagementOutcome::HeldByOperation,
            )],
        }
    }

    /// Wire dispatch entry for the bounded deletion status read.
    pub(crate) async fn deletion_status_wire(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        query: &DeletionStatusRequest,
    ) -> Vec<WireFrame> {
        match self
            .read_deletion_status(query.cursor.as_ref(), query.limit)
            .await
        {
            Ok(response) => vec![outgoing_frame(
                frame,
                live,
                WirePayload::DeletionStatusResponse(response),
            )],
            Err(DeletionStatusQueryError::InvalidLimit) => {
                vec![field_reject(frame, live, "query limit must be 1..=50")]
            }
            Err(DeletionStatusQueryError::InvalidCursor) => vec![reject_frame(
                frame,
                live,
                RejectKind::UnsupportedFieldValue,
                String::from("cursor is not a deletion-status cursor"),
            )],
            // A torn or unreadable surface is explicit, never "no operations".
            Err(DeletionStatusQueryError::Unavailable) => vec![outgoing_frame(
                frame,
                live,
                WirePayload::DeletionStatusResponse(DeletionStatusResponse::Unavailable),
            )],
        }
    }

    /// Bounded participant progress for one operation's status view (A2 §8).
    ///
    /// The durable `deletion_participant` registry is the only source: the
    /// view never fabricates progress and a failed read fails the whole page
    /// closed instead of rendering as `NotReported`. Every operation carries a
    /// non-empty snapshot from admission, so a `Reported` page always names
    /// the required owners.
    async fn participant_report(
        &self,
        record: &DeletionOperationRecord,
    ) -> Result<DeletionParticipantReportWire, CoreError> {
        let participants = self
            .store
            .deletion_participants(record.current.operation, None, 100)
            .await
            .map_err(|error| CoreError::Deletion(error.to_string()))?;
        Ok(DeletionParticipantReportWire::Reported(
            participants
                .iter()
                .map(|participant| DeletionParticipantStatusWire {
                    owner: participant.participant.owner.storage_name(),
                    progress: progress_token(participant.progress),
                    // Pending rows carry no progress sweep in the vocabulary;
                    // the durable row tracks the operation's current sweep
                    // (validate refuses any other shape), so the operation's
                    // sweep is the row's premise, never a guess.
                    sweep: participant
                        .progress
                        .sweep()
                        .map_or(record.current.sweep.as_u64(), |sweep| sweep.as_u64()),
                })
                .collect(),
        ))
    }

    /// One bounded page of the deletion surface plus its live mark.
    ///
    /// Pure read: no startup, repair, re-evaluation, or presented-update runs
    /// here. The cursor is a Host-issued keyset position bound to this query
    /// by its prefix; any other value is rejected.
    async fn read_deletion_status(
        &self,
        cursor: Option<&DeletionStatusCursorWire>,
        limit: Option<u32>,
    ) -> Result<DeletionStatusResponse, DeletionStatusQueryError> {
        let limit = checked_limit(limit).ok_or(DeletionStatusQueryError::InvalidLimit)?;
        let after = match cursor {
            None => None,
            Some(cursor) => Some(
                parse_status_cursor(&cursor.0).ok_or(DeletionStatusQueryError::InvalidCursor)?,
            ),
        };
        let records = self
            .store
            .deletion_status(after, limit)
            .await
            .map_err(|_| DeletionStatusQueryError::Unavailable)?;
        let mark = self
            .store
            .deletion_surface_mark()
            .await
            .map_err(|_| DeletionStatusQueryError::Unavailable)?;
        let next_cursor = if records.len() as u32 == limit {
            records
                .last()
                .map(|record| mint_status_cursor(record.current.operation))
        } else {
            None
        };
        let mut operations = Vec::with_capacity(records.len());
        for record in &records {
            let participants = self
                .participant_report(record)
                .await
                .map_err(|_| DeletionStatusQueryError::Unavailable)?;
            operations.push(status_view(record, participants));
        }
        Ok(DeletionStatusResponse::Page(DeletionStatusPage {
            mark: ViewMarkWire(mark.as_str().to_string()),
            operations,
            next_cursor,
        }))
    }

    /// Host-local read of the exact bounded page the wire view renders: the
    /// Owner's trusted console and the Client must never see different
    /// deletion status.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Deletion`] for a malformed cursor or limit, and an
    /// unreadable surface, so the caller can distinguish "nothing to show" from
    /// "cannot show".
    pub async fn deletion_status_page(
        &self,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<DeletionStatusResponse, CoreError> {
        let cursor = cursor.map(|raw| DeletionStatusCursorWire(raw.to_string()));
        self.read_deletion_status(cursor.as_ref(), Some(limit))
            .await
            .map_err(|error| match error {
                DeletionStatusQueryError::InvalidLimit => {
                    CoreError::Deletion(String::from("status limit must be 1..=50"))
                }
                DeletionStatusQueryError::InvalidCursor => {
                    CoreError::Deletion(String::from("cursor is not a deletion-status cursor"))
                }
                DeletionStatusQueryError::Unavailable => {
                    CoreError::Deletion(String::from("deletion status is unavailable"))
                }
            })
    }

    /// Host-local trusted inlet (IPC §18.1): every staged request still
    /// awaiting the Owner's final confirmation, keyset-paged by canonical
    /// request identity (`limit` is 1..=100).
    ///
    /// Each entry carries its protected mechanical target so the Owner can
    /// review exactly what would be deleted before confirming; the target
    /// never travels further than that console.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Store`] when the durable request journal is
    /// unreadable.
    pub async fn pending_targeted_deletions(
        &self,
        after: Option<DeletionRequestId>,
        limit: u32,
    ) -> Result<Vec<TargetedDeletionRequest>, CoreError> {
        self.store
            .pending_targeted_deletions(after, limit)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))
    }

    /// Host-local trusted inlet (IPC §18.1): record the Owner's final
    /// confirmation for one staged request and run the canonical admission.
    ///
    /// This is the only path from a request to a destructive operation, and
    /// it must run in the serving composition: the Client-incarnation demand
    /// resolves reachability from that process's connection table, and the
    /// Host-local trusted inlet is the only path that may record the Owner's
    /// confirmation (IPC §18.1). The delivery evidence the snapshot reads is
    /// durable, so a restart still names every incarnation that may hold a
    /// target-bearing copy (lifecycle §8.1). The Owner reaches it through
    /// [`crate::host_control`]; an offline
    /// CLI refusal is deliberate, never a fallback. An unknown or malformed
    /// identity answers
    /// [`Missing`](ConfirmTargetedDeletionOutcome::Missing) and changes
    /// nothing; a duplicate confirmation observes the same single operation.
    /// When the admission starts the operation, a bounded fan-out drive runs
    /// immediately, so erasure begins at the confirmation instead of waiting
    /// for the next serving tick; the drive is best-effort and never turns the
    /// confirmation into a completion claim.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Store`] when the durable journals are unreadable.
    pub async fn confirm_targeted_deletion(
        &self,
        request: &str,
    ) -> Result<ConfirmTargetedDeletionOutcome, CoreError> {
        let Some(request) = parse_request_id(request) else {
            return Ok(ConfirmTargetedDeletionOutcome::Missing);
        };
        // An unreadable evidence snapshot fails the admission before any
        // confirmation row is written: a destructive operation never starts
        // with an incomplete required-participant set.
        let required = self.required_deletion_participants().await?;
        let outcome = self
            .store
            .confirm_targeted_deletion(request, required)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        if matches!(outcome, ConfirmTargetedDeletionOutcome::Started(_)) {
            self.kick_targeted_deletion().await;
        }
        Ok(outcome)
    }

    /// Owner-initiated resume of a Held Targeted Deletion operation.
    ///
    /// The operation was already admitted. This reopens a retryable
    /// `Held(Unavailable)` hold and kicks fan-out; `GenerationExhausted`
    /// stays held. A stale sweep or completed operation is refused without
    /// rewriting durable state.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Store`] when the durable journals are unreadable.
    pub async fn resume_targeted_deletion(
        &self,
        operation: &str,
        sweep: u64,
    ) -> Result<DeletionLifecycleOutcome, CoreError> {
        let Ok(uuid) = uuid::Uuid::parse_str(operation) else {
            return Ok(DeletionLifecycleOutcome::Missing);
        };
        let current = DeletionOperationRef {
            operation: DeletionOperationId::from_raw(RawId::from_uuid(uuid)),
            sweep: DeletionSweepGeneration::from_u64(sweep),
        };
        let outcome = self
            .store
            .change_deletion_lifecycle(current, DeletionLifecycleChange::Resume)
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        if matches!(outcome, DeletionLifecycleOutcome::Applied(_)) {
            self.kick_targeted_deletion().await;
        }
        Ok(outcome)
    }

    /// Parks the sealed finalizing boundary so a GUI test can observe
    /// `Finalizing` as distinct from `Completed`.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_deletion_finalizing_park_for_tests(&self) {
        self.store.arm_deletion_finalizing_park_for_tests();
    }

    /// Waits until the armed deletion-finalizing park has a waiter.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_deletion_finalizing_park_for_tests(&self) {
        self.store.wait_deletion_finalizing_park_for_tests().await;
    }

    /// Releases the parked deletion-finalizing attempt.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_deletion_finalizing_park_for_tests(&self) {
        self.store.release_deletion_finalizing_park_for_tests();
    }
}

#[cfg(test)]
mod tests {
    use ene_api::v1::deletion::{
        DeletionPhaseWire, DeletionStatusRequest, DeletionStatusResponse, deletion_target,
    };
    use ene_api::v1::payload::WirePayload;
    use ene_preservation::{DeletionOperationId, DeletionOperationPhase, DeletionOperationRecord};
    use ene_primitive::{RawId, WallClockWithTz};

    use super::{
        DeletionStatusQueryError, mint_status_cursor, parse_request_id, parse_status_cursor,
        progress_token, status_view,
    };
    use ene_api::v1::deletion::{DeletionParticipantReportWire, DeletionParticipantStatusWire};

    fn record() -> DeletionOperationRecord {
        DeletionOperationRecord {
            current: ene_preservation::DeletionOperationRef {
                operation: DeletionOperationId::from_raw(RawId::new()),
                sweep: ene_preservation::DeletionSweepGeneration::from_u64(2),
            },
            phase: DeletionOperationPhase::Active,
            purpose: ene_preservation::DeletionPurpose::Privacy,
            started_at: WallClockWithTz::now(),
            hold: None,
        }
    }

    #[test]
    fn status_cursor_roundtrips_and_rejects_other_text() {
        let operation = record().current.operation;
        let cursor = mint_status_cursor(operation);
        assert_eq!(parse_status_cursor(&cursor.0), Some(operation));
        for raw in [
            "deletion-status:",
            "deletion-status:not-a-uuid",
            "task-list:some-id",
            "",
        ] {
            assert_eq!(parse_status_cursor(raw), None, "rejects {raw:?}");
        }
    }

    #[test]
    fn request_identity_parses_only_canonical_uuids() {
        let id = RawId::new();
        let parsed = parse_request_id(&id.as_uuid().as_hyphenated().to_string());
        assert_eq!(parsed.map(|request| request.as_raw()), Some(id));
        assert_eq!(parse_request_id("not-a-uuid"), None);
        assert_eq!(parse_request_id(""), None);
    }

    #[test]
    fn status_view_reports_phase_and_never_a_body() {
        let record = record();
        let view = status_view(&record, reported_progress());
        assert_eq!(view.sweep, 2);
        assert_eq!(
            view.operation.0,
            record
                .current
                .operation
                .as_raw()
                .as_uuid()
                .as_hyphenated()
                .to_string()
        );
        let rendered = format!("{view:?}");
        assert!(
            !rendered.contains("deletion:") && !rendered.contains(DELETION_BODY),
            "the status view never renders target material: {rendered}"
        );
        assert!(rendered.contains("Active"), "the phase is displayable");
    }

    #[test]
    fn status_view_reports_every_lifecycle_phase() {
        for (phase, expected) in [
            (DeletionOperationPhase::Active, DeletionPhaseWire::Active),
            (DeletionOperationPhase::Held, DeletionPhaseWire::Held),
            (
                DeletionOperationPhase::Finalizing,
                DeletionPhaseWire::Finalizing,
            ),
            (
                DeletionOperationPhase::Completed,
                DeletionPhaseWire::Completed,
            ),
        ] {
            let mut record = record();
            record.phase = phase;
            assert_eq!(status_view(&record, reported_progress()).phase, expected);
        }
    }

    /// One explicit durable-progress report, as the participant read returns.
    fn reported_progress() -> DeletionParticipantReportWire {
        DeletionParticipantReportWire::Reported(vec![
            DeletionParticipantStatusWire {
                owner: String::from("companion"),
                progress: String::from("local-complete"),
                sweep: 2,
            },
            DeletionParticipantStatusWire {
                owner: String::from("learning"),
                progress: String::from("held:unsupported"),
                sweep: 2,
            },
        ])
    }

    #[test]
    fn participant_progress_tokens_map_from_the_durable_vocabulary() {
        use ene_preservation::{
            DeletionSweepGeneration, ParticipantHoldClass, ParticipantProgress,
        };

        let sweep = DeletionSweepGeneration::from_u64(3);
        assert_eq!(progress_token(ParticipantProgress::Pending), "pending");
        assert_eq!(
            progress_token(ParticipantProgress::Running { sweep }),
            "running"
        );
        assert_eq!(
            progress_token(ParticipantProgress::LocalComplete { sweep }),
            "local-complete"
        );
        assert_eq!(
            progress_token(ParticipantProgress::Verified { sweep }),
            "verified"
        );
        assert_eq!(
            progress_token(ParticipantProgress::Held {
                sweep,
                reason: ParticipantHoldClass::Unsupported,
            }),
            "held:unsupported"
        );
    }

    #[test]
    fn not_reported_participants_are_explicit() {
        let view = status_view(&record(), DeletionParticipantReportWire::NotReported);
        assert_eq!(
            view.participants,
            DeletionParticipantReportWire::NotReported,
            "an absent report is distinct from reported progress"
        );
        assert_ne!(
            DeletionParticipantReportWire::Reported(Vec::new()),
            DeletionParticipantReportWire::NotReported
        );
    }

    #[test]
    fn wire_payloads_roundtrip_without_a_target() {
        let response = DeletionStatusResponse::Page(ene_api::v1::deletion::DeletionStatusPage {
            mark: ene_api::v1::refs::ViewMarkWire(String::from("deletion-view/0/-/0/-")),
            operations: vec![status_view(&record(), reported_progress())],
            next_cursor: None,
        });
        let payload = WirePayload::DeletionStatusResponse(response.clone());
        assert_eq!(payload.message_type(), "DeletionStatusResponse");
        let json = serde_json::to_string(&payload).expect("the status payload must serialize");
        assert!(
            !json.contains(DELETION_BODY),
            "no target body reaches a status payload"
        );
        let request = DeletionStatusRequest {
            cursor: None,
            limit: Some(10),
        };
        assert_eq!(
            WirePayload::DeletionStatusRequest(request).message_type(),
            "DeletionStatusRequest"
        );
    }

    #[test]
    fn parse_errors_are_typed() {
        assert_ne!(
            DeletionStatusQueryError::InvalidLimit,
            DeletionStatusQueryError::Unavailable
        );
        let target = deletion_target(
            ene_api::v1::deletion::DeletionPurposeWire::Privacy,
            DELETION_BODY,
        );
        let parsed = ene_api::v1::deletion::parse_deletion_target(&target)
            .expect("the fixture target must parse");
        assert!(!format!("{parsed:?}").contains(DELETION_BODY));
    }

    const DELETION_BODY: &str = "fixture-body-that-must-never-render";
}
