//! Typed wire payloads carried under [`super::envelope::WireEnvelope`].
//!
//! The envelope's `message_type` names one of these variants; unknown names
//! are rejected, never guessed. Adding a message means adding a variant
//! here, never smuggling it through an untyped channel.

use serde::{Deserialize, Serialize};

use super::handshake::{
    AuthChallenge, AuthProof, AuthResult, CapabilityAdvertise, DisconnectNotice,
    NegotiatedConnection, PairingRequest, PairingResult, ReconnectHello, RecoveryInvite,
};
use super::management::{
    ManagementIntent, ManagementOutcome, ManagementView, ManagementViewRequest,
};
use super::presence::PresenceAttributionWire;
use super::reject::RejectNotice;
use super::round::{
    ConfirmPresentationWire, HistoryRequest, HistoryView, RoundIntakeOutcomeWire, SubmitTextInput,
    TextStreamClose, TextStreamFrameWire, TextStreamOpen,
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
    ReconnectHello(ReconnectHello),
    RecoveryInvite(RecoveryInvite),
    DisconnectNotice(DisconnectNotice),
    SubmitTextInput(SubmitTextInput),
    RoundIntakeOutcome(RoundIntakeOutcomeWire),
    TextStreamOpen(TextStreamOpen),
    TextStreamFrame(TextStreamFrameWire),
    TextStreamClose(TextStreamClose),
    ConfirmPresentation(ConfirmPresentationWire),
    HistoryRequest(HistoryRequest),
    HistoryView(HistoryView),
    PresenceAttribution(PresenceAttributionWire),
    ManagementIntent(ManagementIntent),
    ManagementOutcome(ManagementOutcome),
    ManagementViewRequest(ManagementViewRequest),
    ManagementView(ManagementView),
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
            Self::ReconnectHello(_) => "ReconnectHello",
            Self::RecoveryInvite(_) => "RecoveryInvite",
            Self::DisconnectNotice(_) => "DisconnectNotice",
            Self::SubmitTextInput(_) => "SubmitTextInput",
            Self::RoundIntakeOutcome(_) => "RoundIntakeOutcome",
            Self::TextStreamOpen(_) => "TextStreamOpen",
            Self::TextStreamFrame(_) => "TextStreamFrame",
            Self::TextStreamClose(_) => "TextStreamClose",
            Self::ConfirmPresentation(_) => "ConfirmPresentation",
            Self::HistoryRequest(_) => "HistoryRequest",
            Self::HistoryView(_) => "HistoryView",
            Self::PresenceAttribution(_) => "PresenceAttribution",
            Self::ManagementIntent(_) => "ManagementIntent",
            Self::ManagementOutcome(_) => "ManagementOutcome",
            Self::ManagementViewRequest(_) => "ManagementViewRequest",
            Self::ManagementView(_) => "ManagementView",
            Self::Reject(_) => "Reject",
        }
    }
}
