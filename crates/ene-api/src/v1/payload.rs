//! Typed wire payloads carried under [`super::envelope::WireEnvelope`].
//!
//! The envelope's `message_type` names one of these variants for routing;
//! unknown names are rejected, never guessed. Each variant is one of the
//! module DTOs; adding a message means adding a variant here, never
//! smuggling it through an untyped channel.

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

/// Typed body of a wire message. Externally tagged; unknown variants are
/// rejected at deserialization, never defaulted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WirePayload {
    /// Pairing request (pre-pairing; envelope carries no device ID).
    PairingRequest(PairingRequest),
    /// Pairing outcome.
    PairingResult(PairingResult),
    /// Authentication challenge.
    AuthChallenge(AuthChallenge),
    /// Authentication proof (auth frames only).
    AuthProof(AuthProof),
    /// Authentication outcome with the connection key.
    AuthResult(AuthResult),
    /// Capability advertisement.
    CapabilityAdvertise(CapabilityAdvertise),
    /// Negotiated connection terms.
    NegotiatedConnection(NegotiatedConnection),
    /// Reconnect declaration with fresh authentication.
    ReconnectHello(ReconnectHello),
    /// Post-restart recovery invitation.
    RecoveryInvite(RecoveryInvite),
    /// Disconnection observation.
    DisconnectNotice(DisconnectNotice),
    /// Owner text input candidate.
    SubmitTextInput(SubmitTextInput),
    /// Intake outcome for one input.
    RoundIntakeOutcome(RoundIntakeOutcomeWire),
    /// Response stream opening.
    TextStreamOpen(TextStreamOpen),
    /// One stream frame.
    TextStreamFrame(TextStreamFrameWire),
    /// Stream close record.
    TextStreamClose(TextStreamClose),
    /// Presentation confirmation observation.
    ConfirmPresentation(ConfirmPresentationWire),
    /// Timeline restore request.
    HistoryRequest(HistoryRequest),
    /// Timeline restore answer.
    HistoryView(HistoryView),
    /// Authoritative presence attribution fact.
    PresenceAttribution(PresenceAttributionWire),
    /// Management intent.
    ManagementIntent(ManagementIntent),
    /// Management outcome.
    ManagementOutcome(ManagementOutcome),
    /// Filtered view request.
    ManagementViewRequest(ManagementViewRequest),
    /// Filtered management view.
    ManagementView(ManagementView),
    /// Typed wire rejection: no side effects, no retry signal.
    Reject(RejectNotice),
}
