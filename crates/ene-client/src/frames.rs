use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
use ene_api::v1::handshake::{AuthProof, CapabilityAdvertise, PairingRequest};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{
    ClientIncarnationId, CommandWireId, DeviceWireId, RequestWireId, WireMessageType,
};
use ene_plugin_ipc::WireFrame;

pub fn proof_frame(
    proof: &str,
    incarnation: ClientIncarnationId,
    device_id: DeviceWireId,
) -> WireFrame {
    frame_for(
        WirePayload::AuthProof(AuthProof {
            proof: String::from(proof),
        }),
        WireSender {
            device_id: Some(device_id),
            incarnation_id: incarnation,
            connection_id: None,
        },
    )
}

#[must_use]
pub fn missing_secret_guidance() -> String {
    String::from("no pairing secret stored for this device; start a fresh pairing request")
}

#[must_use]
pub fn unreadable_device_file_guidance() -> String {
    String::from(
        "the stored client device file is unreadable or malformed; remove it and start a fresh pairing request",
    )
}

#[must_use]
pub fn auth_rejected_guidance(reason: &str) -> String {
    format!(
        "authentication rejected: {reason}; remove the stored client device and start a fresh pairing request"
    )
}

/// A logical send prepared before I/O: the payload plus the command identity a
/// transport attempt must reuse. Prepare through [`super::Client::prepare`] and
/// keep the handle; [`super::Client::execute`]
/// sends it without rebuilding the identity.
///
/// The command identity is [`None`] for a pure request/response payload
/// (`HistoryRequest`, `ManagementViewRequest`): those pair by `request_id` and
/// `reply_to` only and have no command saga to replay.
///
/// A prepared command is bound to the sender incarnation that prepared it.
/// The Host keys command idempotency on the authenticated sender epoch, so
/// after a reconnect (new incarnation) the same handle can no longer be
/// replayed — re-prepare under the new incarnation instead of retrying.
pub struct PreparedRequest {
    command_id: Option<CommandWireId>,
    payload: WirePayload,
}

impl PreparedRequest {
    #[must_use]
    pub fn new(payload: WirePayload) -> Self {
        let command_id = match &payload {
            WirePayload::ManagementIntent(intent) => Some(intent.intent_id),
            WirePayload::SubmitTextInput(_) | WirePayload::ResumeTask(_) => {
                Some(CommandWireId(uuid::Uuid::new_v4()))
            }
            _ => None,
        };
        Self {
            command_id,
            payload,
        }
    }

    pub(super) fn frame(&self, sender: WireSender, generation: Option<u64>) -> WireFrame {
        let mut frame = frame_for_session(self.payload.clone(), sender, generation);
        frame.envelope.correlation.command_id = self.command_id;
        frame.envelope.correlation.request_id = Some(RequestWireId(uuid::Uuid::new_v4()));
        frame
    }
}

pub fn frame_for_session(
    payload: WirePayload,
    sender: WireSender,
    generation: Option<u64>,
) -> WireFrame {
    let mut frame = frame_for(payload, sender);
    if matches!(frame.payload, WirePayload::SubmitTextInput(_)) {
        frame.envelope.observed.presence_generation_view = generation;
    }
    frame
}

pub fn observed_frame(
    payload: WirePayload,
    sender: WireSender,
    generation: Option<u64>,
    round: Option<ene_api::v1::refs::RoundWireId>,
) -> WireFrame {
    let mut frame = frame_for(payload, sender);
    frame.envelope.observed.presence_generation_view = generation;
    frame.envelope.observed.round_view = round;
    frame
}

/// Pre-pairing sender: the Host issues the device ID and secret on this same
/// connection after Owner confirmation.
///
/// The frame carries a Client-minted `request_id` so a retry of the same
/// logical request can be told from a conflicting reuse: the Host answers the
/// live pending for the same id and body, and refuses an id reused with a
/// different body (IPC §9.3).
pub fn pairing_frame(descriptor: &str, incarnation: ClientIncarnationId) -> WireFrame {
    let mut frame = frame_for(
        WirePayload::PairingRequest(PairingRequest {
            device_descriptor: String::from(descriptor),
        }),
        WireSender {
            device_id: None,
            incarnation_id: incarnation,
            connection_id: None,
        },
    );
    frame.envelope.correlation.request_id = Some(RequestWireId(uuid::Uuid::new_v4()));
    frame
}

pub fn capability_frame(
    platform: &str,
    incarnation: ClientIncarnationId,
    device_id: Option<DeviceWireId>,
) -> WireFrame {
    frame_for(
        WirePayload::CapabilityAdvertise(CapabilityAdvertise {
            supported_protocol: vec![ProtocolVersion::V1],
            platform: String::from(platform),
        }),
        WireSender {
            device_id,
            incarnation_id: incarnation,
            connection_id: None,
        },
    )
}

pub fn frame_for(payload: WirePayload, sender: WireSender) -> WireFrame {
    let message_type = WireMessageType(String::from(payload.message_type()));
    let envelope = new_outgoing_envelope(ProtocolVersion::V1, sender, message_type);
    WireFrame { envelope, payload }
}
