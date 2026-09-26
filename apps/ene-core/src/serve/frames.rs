use super::LiveInput;
use ene_api::codec::{UnsupportedReason, WireFrame};
use ene_api::v1::envelope::{ProtocolVersion, WireEnvelope, WireSender, new_outgoing_envelope};
use ene_api::v1::handshake::DisconnectNotice;
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{DeviceWireId, WireMessageId, WireMessageType};
use ene_api::v1::reject::{IncompatibleProtocol, RejectKind, RejectNotice};
use uuid::Uuid;

pub(crate) fn unpaired_close(frame: &WireFrame, live: &LiveInput) -> WireFrame {
    outgoing_frame_pre_auth(
        frame,
        live,
        WirePayload::DisconnectNotice(DisconnectNotice {
            reason: String::from("unpaired"),
        }),
    )
}

fn response_sender(
    envelope: &WireEnvelope,
    live: &LiveInput,
    reveal_connection: bool,
) -> WireSender {
    WireSender {
        device_id: live
            .paired_device
            .as_deref()
            .and_then(|text| Uuid::parse_str(text).ok())
            .map(DeviceWireId),
        incarnation_id: envelope.sender.incarnation_id,
        connection_id: reveal_connection.then_some(live.connection_id),
    }
}

pub(crate) fn outgoing_envelope(
    frame: &WireFrame,
    live: &LiveInput,
    payload: &WirePayload,
    reply_to: Option<WireMessageId>,
) -> WireEnvelope {
    outgoing_envelope_inner(&frame.envelope, live, payload, reply_to, true)
}

fn outgoing_envelope_inner(
    envelope: &WireEnvelope,
    live: &LiveInput,
    payload: &WirePayload,
    reply_to: Option<WireMessageId>,
    reveal_connection: bool,
) -> WireEnvelope {
    let mut outgoing = new_outgoing_envelope(
        ProtocolVersion::V1,
        response_sender(envelope, live, reveal_connection),
        WireMessageType(payload.message_type().to_string()),
    );
    outgoing.correlation.reply_to = reply_to;
    outgoing
}

pub(crate) fn outgoing_frame(
    frame: &WireFrame,
    live: &LiveInput,
    payload: WirePayload,
) -> WireFrame {
    let envelope = outgoing_envelope(frame, live, &payload, Some(frame.envelope.message_id));
    WireFrame { envelope, payload }
}

pub(crate) fn stale_reject(frame: &WireFrame, live: &LiveInput, detail: &str) -> WireFrame {
    reject_frame(
        &frame.envelope,
        live,
        RejectKind::StaleConnection,
        detail.to_string(),
    )
}

pub(crate) fn invalid_phase_reject(frame: &WireFrame, live: &LiveInput, detail: &str) -> WireFrame {
    reject_frame(
        &frame.envelope,
        live,
        RejectKind::InvalidHandshakePhase,
        detail.to_string(),
    )
}

pub(crate) fn outgoing_fact(
    frame: &WireFrame,
    live: &LiveInput,
    payload: WirePayload,
) -> WireFrame {
    let envelope = outgoing_envelope(frame, live, &payload, None);
    WireFrame { envelope, payload }
}

pub(crate) fn incompatible_protocol(
    envelope: &WireEnvelope,
    live: &LiveInput,
    client_max: ProtocolVersion,
) -> WireFrame {
    let host_max = ProtocolVersion::V1;
    outgoing_frame_from_envelope(
        envelope,
        live,
        WirePayload::IncompatibleProtocol(IncompatibleProtocol {
            host_max,
            client_max,
            hint: upgrade_hint(host_max),
        }),
    )
}

fn upgrade_hint(host_max: ProtocolVersion) -> String {
    format!(
        "use a client release matching the host's protocol {}.{}",
        host_max.major, host_max.minor
    )
}

pub(crate) fn reject_frame(
    envelope: &WireEnvelope,
    live: &LiveInput,
    kind: RejectKind,
    detail: String,
) -> WireFrame {
    let payload = WirePayload::Reject(RejectNotice { kind, detail });
    outgoing_frame_from_envelope(envelope, live, payload)
}

fn outgoing_frame_from_envelope(
    envelope: &WireEnvelope,
    live: &LiveInput,
    payload: WirePayload,
) -> WireFrame {
    let reply = outgoing_envelope_inner(
        envelope,
        live,
        &payload,
        Some(envelope.message_id),
        live.authed,
    );
    WireFrame {
        envelope: reply,
        payload,
    }
}

pub(crate) fn unsupported_reject(
    envelope: &WireEnvelope,
    live: &LiveInput,
    reason: &UnsupportedReason,
) -> WireFrame {
    outgoing_frame_from_envelope(envelope, live, WirePayload::Reject(reason.notice()))
}

pub(crate) fn outgoing_frame_pre_auth(
    frame: &WireFrame,
    live: &LiveInput,
    payload: WirePayload,
) -> WireFrame {
    let envelope = outgoing_envelope_inner(
        &frame.envelope,
        live,
        &payload,
        Some(frame.envelope.message_id),
        false,
    );
    WireFrame { envelope, payload }
}
