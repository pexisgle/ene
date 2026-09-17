//! Typed wire payloads carried under [`super::envelope::WireEnvelope`].
//!
//! The envelope's `message_type` names one of these variants; unknown names
//! are rejected, never guessed. Adding a message means adding a variant
//! here, never smuggling it through an untyped channel.

use serde::{Deserialize, Serialize};

use super::deletion::{DeletionStatusRequest, DeletionStatusResponse};
use super::handshake::{
    AuthChallenge, AuthProof, AuthResult, CapabilityAdvertise, DisconnectNotice,
    NegotiatedConnection, PairingRequest, PairingResult,
};
use super::management::{
    ManagementIntent, ManagementOutcome, ManagementView, ManagementViewRequest,
};
use super::presence::PresenceAttributionWire;
use super::reject::RejectNotice;
use super::round::{
    ConfirmPresentationWire, HistoryRequest, HistoryResponse, RoundIntakeOutcomeWire,
    SubmitTextInput, TextStreamClose, TextStreamFrameWire, TextStreamOpen,
};
use super::undelivered::{
    GetReportSource, GetTaskReport, ListTasks, ReportSourceResponse, ResumeTask,
    ResumeTaskOutcomeWire, SelectTask, SelectTaskResponse, TaskListResponse, TaskReportResponse,
    UndeliveredAck, UndeliveredAckOutcome, UndeliveredRequest, UndeliveredResponse,
};

/// Externally tagged; unknown variants are rejected at deserialization,
/// never defaulted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WirePayload {
    PairingRequest(PairingRequest),
    PairingResult(PairingResult),
    AuthChallenge(AuthChallenge),
    AuthProof(AuthProof),
    AuthResult(AuthResult),
    CapabilityAdvertise(CapabilityAdvertise),
    NegotiatedConnection(NegotiatedConnection),
    DisconnectNotice(DisconnectNotice),
    SubmitTextInput(SubmitTextInput),
    RoundIntakeOutcome(RoundIntakeOutcomeWire),
    TextStreamOpen(TextStreamOpen),
    TextStreamFrame(TextStreamFrameWire),
    TextStreamClose(TextStreamClose),
    ConfirmPresentation(ConfirmPresentationWire),
    HistoryRequest(HistoryRequest),
    HistoryResponse(HistoryResponse),
    PresenceAttribution(PresenceAttributionWire),
    ManagementIntent(ManagementIntent),
    ManagementOutcome(ManagementOutcome),
    ManagementViewRequest(ManagementViewRequest),
    ManagementView(ManagementView),
    DeletionStatusRequest(DeletionStatusRequest),
    DeletionStatusResponse(DeletionStatusResponse),
    UndeliveredRequest(UndeliveredRequest),
    UndeliveredResponse(UndeliveredResponse),
    UndeliveredAck(UndeliveredAck),
    UndeliveredAckOutcome(UndeliveredAckOutcome),
    ListTasks(ListTasks),
    TaskListResponse(TaskListResponse),
    GetTaskReport(GetTaskReport),
    TaskReportResponse(TaskReportResponse),
    GetReportSource(GetReportSource),
    ReportSourceResponse(ReportSourceResponse),
    SelectTask(SelectTask),
    SelectTaskResponse(SelectTaskResponse),
    ResumeTask(ResumeTask),
    ResumeTaskOutcome(ResumeTaskOutcomeWire),
    Reject(RejectNotice),
}

impl WirePayload {
    /// Canonical name senders put in the envelope; receivers compare the
    /// envelope string against this (instead of trusting it) and reject
    /// mismatches without guessing.
    #[must_use]
    pub fn message_type(&self) -> &'static str {
        match self {
            Self::PairingRequest(_) => "PairingRequest",
            Self::PairingResult(_) => "PairingResult",
            Self::AuthChallenge(_) => "AuthChallenge",
            Self::AuthProof(_) => "AuthProof",
            Self::AuthResult(_) => "AuthResult",
            Self::CapabilityAdvertise(_) => "CapabilityAdvertise",
            Self::NegotiatedConnection(_) => "NegotiatedConnection",
            Self::DisconnectNotice(_) => "DisconnectNotice",
            Self::SubmitTextInput(_) => "SubmitTextInput",
            Self::RoundIntakeOutcome(_) => "RoundIntakeOutcome",
            Self::TextStreamOpen(_) => "TextStreamOpen",
            Self::TextStreamFrame(_) => "TextStreamFrame",
            Self::TextStreamClose(_) => "TextStreamClose",
            Self::ConfirmPresentation(_) => "ConfirmPresentation",
            Self::HistoryRequest(_) => "HistoryRequest",
            Self::HistoryResponse(_) => "HistoryResponse",
            Self::PresenceAttribution(_) => "PresenceAttribution",
            Self::ManagementIntent(_) => "ManagementIntent",
            Self::ManagementOutcome(_) => "ManagementOutcome",
            Self::ManagementViewRequest(_) => "ManagementViewRequest",
            Self::ManagementView(_) => "ManagementView",
            Self::DeletionStatusRequest(_) => "DeletionStatusRequest",
            Self::DeletionStatusResponse(_) => "DeletionStatusResponse",
            Self::UndeliveredRequest(_) => "UndeliveredRequest",
            Self::UndeliveredResponse(_) => "UndeliveredResponse",
            Self::UndeliveredAck(_) => "UndeliveredAck",
            Self::UndeliveredAckOutcome(_) => "UndeliveredAckOutcome",
            Self::ListTasks(_) => "ListTasks",
            Self::TaskListResponse(_) => "TaskListResponse",
            Self::GetTaskReport(_) => "GetTaskReport",
            Self::TaskReportResponse(_) => "TaskReportResponse",
            Self::GetReportSource(_) => "GetReportSource",
            Self::ReportSourceResponse(_) => "ReportSourceResponse",
            Self::SelectTask(_) => "SelectTask",
            Self::SelectTaskResponse(_) => "SelectTaskResponse",
            Self::ResumeTask(_) => "ResumeTask",
            Self::ResumeTaskOutcome(_) => "ResumeTaskOutcome",
            Self::Reject(_) => "Reject",
        }
    }
}
