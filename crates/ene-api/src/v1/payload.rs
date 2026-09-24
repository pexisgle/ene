use serde::{Deserialize, Serialize};

use super::command::CommandReplayRejectWire;
use super::deletion::{
    DeletionDemand, DeletionStatusRequest, DeletionStatusResponse, LocalErasureResult,
};
use super::handshake::{
    AuthChallenge, AuthProof, AuthResult, CapabilityAdvertise, DisconnectNotice,
    NegotiatedConnection, PairingProvision, PairingRequest, PairingResult,
};
use super::management::{
    ManagementIntent, ManagementOutcome, ManagementView, ManagementViewRequest,
};
use super::presence::PresenceAttributionWire;
use super::reject::{IncompatibleProtocol, RejectNotice};
use super::round::{
    ConfirmPresentationWire, HistoryRequest, HistoryResponse, RoundIntakeOutcomeWire,
    SubmitTextInput, TextStreamClose, TextStreamFrameWire, TextStreamOpen,
};
use super::undelivered::{
    GetReportSource, GetTaskReport, ListTasks, ReportSourceResponse, ResumeTask,
    ResumeTaskOutcomeWire, SelectTask, SelectTaskResponse, TaskListResponse, TaskReportResponse,
    UndeliveredAck, UndeliveredAckOutcome, UndeliveredRequest, UndeliveredResponse,
};
use super::usage::{UsageSummaryRequest, UsageSummaryResponse};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BodyStateHint {
    pub asset_ref: String,
    pub pose_hint: String,
}

macro_rules! wire_payload {
    ($( $variant:ident($type:ty) ),+ $(,)?) => {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        pub enum WirePayload {
            $($variant($type)),+
        }

        impl WirePayload {
            #[must_use]
            pub fn message_type(&self) -> &'static str {
                match self {
                    $(Self::$variant(_) => stringify!($variant)),+
                }
            }

            pub const KNOWN_MESSAGE_TYPES: &'static [&'static str] = &[$(stringify!($variant)),+];
        }
    };
}

wire_payload! {
    PairingRequest(PairingRequest),
    PairingResult(PairingResult),
    PairingProvision(PairingProvision),
    AuthChallenge(AuthChallenge),
    AuthProof(AuthProof),
    AuthResult(AuthResult),
    CapabilityAdvertise(CapabilityAdvertise),
    NegotiatedConnection(NegotiatedConnection),
    DisconnectNotice(DisconnectNotice),
    IncompatibleProtocol(IncompatibleProtocol),
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
    DeletionDemand(DeletionDemand),
    LocalErasureResult(LocalErasureResult),
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
    UsageSummaryRequest(UsageSummaryRequest),
    UsageSummaryResponse(UsageSummaryResponse),
    BodyStateHint(BodyStateHint),
    CommandReplayReject(CommandReplayRejectWire),
    Reject(RejectNotice),
}
