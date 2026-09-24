use ene_api::codec::WireFrame;
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
        encode_operation(operation)
    ))
}

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
    const INTENT_KIND_DELETION: &str = "deletion-targeted";

    pub(crate) const DELETION_JOURNAL_FAMILY: &str = "deletion-family";

    pub(crate) fn deletion_intent_fingerprint(
        intent: &ManagementIntent,
        purpose: DeletionPurposeWire,
    ) -> IntentFingerprint {
        IntentFingerprint {
            target: format!("{DELETION_TARGET_PREFIX}{}", purpose.as_str()),
            ..Self::intent_fingerprint(intent, Self::INTENT_KIND_DELETION)
        }
    }

    pub(crate) async fn targeted_deletion_intent(
        &self,
        frame: &WireFrame,
        intent: &ManagementIntent,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        let Some(request) = parse_deletion_target(&intent.target) else {
            return vec![outcome_frame(
                frame,
                live,
                intent,
                self.record_decided(
                    Self::intent_fingerprint(intent, Self::INTENT_KIND_DELETION),
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
            Err(_) => vec![outcome_frame(
                frame,
                live,
                intent,
                ManagementOutcome::HeldByOperation,
            )],
        }
    }

    pub(crate) async fn deletion_status_wire(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        query: &DeletionStatusRequest,
    ) -> Vec<WireFrame> {
        match self
            .read_deletion_status(
                query.cursor.as_ref().map(|cursor| cursor.0.as_str()),
                query.limit,
            )
            .await
        {
            Ok(page) => vec![outgoing_frame(
                frame,
                live,
                WirePayload::DeletionStatusResponse(DeletionStatusResponse::Page(page)),
            )],
            Err(DeletionStatusQueryError::InvalidLimit) => {
                vec![field_reject(frame, live, "query limit must be 1..=50")]
            }
            Err(DeletionStatusQueryError::InvalidCursor) => vec![reject_frame(
                &frame.envelope,
                live,
                RejectKind::UnsupportedFieldValue,
                String::from("cursor is not a deletion-status cursor"),
            )],
            Err(DeletionStatusQueryError::Unavailable) => vec![outgoing_frame(
                frame,
                live,
                WirePayload::DeletionStatusResponse(DeletionStatusResponse::Unavailable),
            )],
        }
    }

    async fn participant_report(
        &self,
        record: &DeletionOperationRecord,
    ) -> Result<DeletionParticipantReportWire, CoreError> {
        let mut participants = Vec::new();
        let mut after = None;
        loop {
            let page = self
                .store
                .deletion_participants(record.current.operation, after, 100)
                .await
                .map_err(|error| CoreError::Deletion(error.to_string()))?;
            if page.is_empty() {
                break;
            }
            let short = page.len() < 100;
            after = page.last().map(|entry| entry.participant.owner);
            participants.extend(page);
            if short {
                break;
            }
        }
        Ok(DeletionParticipantReportWire::Reported(
            participants
                .iter()
                .map(|participant| DeletionParticipantStatusWire {
                    owner: participant.participant.owner.storage_name(),
                    progress: progress_token(participant.progress),
                    sweep: participant
                        .progress
                        .sweep()
                        .map_or(record.current.sweep.as_u64(), |sweep| sweep.as_u64()),
                })
                .collect(),
        ))
    }

    async fn read_deletion_status(
        &self,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<DeletionStatusPage, DeletionStatusQueryError> {
        let limit = checked_limit(limit).ok_or(DeletionStatusQueryError::InvalidLimit)?;
        let after = match cursor {
            None => None,
            Some(cursor) => {
                Some(parse_status_cursor(cursor).ok_or(DeletionStatusQueryError::InvalidCursor)?)
            }
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
        Ok(DeletionStatusPage {
            mark: ViewMarkWire(mark.as_str().to_string()),
            operations,
            next_cursor,
        })
    }

    pub async fn deletion_status_page(
        &self,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<DeletionStatusPage, CoreError> {
        self.read_deletion_status(cursor, Some(limit))
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

    pub async fn confirm_targeted_deletion(
        &self,
        request: &str,
    ) -> Result<ConfirmTargetedDeletionOutcome, CoreError> {
        let Some(request) = parse_request_id(request) else {
            return Ok(ConfirmTargetedDeletionOutcome::Missing);
        };
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
}
