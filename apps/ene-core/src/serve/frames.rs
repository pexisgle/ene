use super::LiveInput;
use ene_api::v1::envelope::{ProtocolVersion, WireEnvelope, WireSender, new_outgoing_envelope};
use ene_api::v1::handshake::DisconnectNotice;
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{DeviceWireId, WireMessageId, WireMessageType};
use ene_api::v1::reject::{RejectKind, RejectNotice};
use ene_plugin_ipc::WireFrame;
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

fn response_sender(frame: &WireFrame, live: &LiveInput, reveal_connection: bool) -> WireSender {
    WireSender {
        device_id: live
            .paired_device
            .as_deref()
            .and_then(|text| Uuid::parse_str(text).ok())
            .map(DeviceWireId),
        incarnation_id: frame.envelope.sender.incarnation_id,
        connection_id: reveal_connection.then_some(live.connection_id),
    }
}

pub(crate) fn outgoing_envelope(
    frame: &WireFrame,
    live: &LiveInput,
    payload: &WirePayload,
    reply_to: Option<WireMessageId>,
) -> WireEnvelope {
    outgoing_envelope_inner(frame, live, payload.message_type(), reply_to, true)
}

fn outgoing_envelope_inner(
    frame: &WireFrame,
    live: &LiveInput,
    message_type: &str,
    reply_to: Option<WireMessageId>,
    reveal_connection: bool,
) -> WireEnvelope {
    let mut envelope = new_outgoing_envelope(
        ProtocolVersion::V1,
        response_sender(frame, live, reveal_connection),
        WireMessageType(message_type.to_string()),
    );
    envelope.correlation.reply_to = reply_to;
    envelope
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
    reject_frame(frame, live, RejectKind::StaleConnection, detail.to_string())
}

pub(crate) fn invalid_phase_reject(frame: &WireFrame, live: &LiveInput, detail: &str) -> WireFrame {
    reject_frame(
        frame,
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
    let mut envelope = outgoing_envelope(frame, live, &payload, None);
    envelope.correlation.reply_to = None;
    WireFrame { envelope, payload }
}

pub(crate) fn reject_frame(
    frame: &WireFrame,
    live: &LiveInput,
    kind: RejectKind,
    detail: String,
) -> WireFrame {
    let payload = WirePayload::Reject(RejectNotice { kind, detail });
    if live.authed {
        outgoing_frame(frame, live, payload)
    } else {
        outgoing_frame_pre_auth(frame, live, payload)
    }
}

pub(crate) fn outgoing_frame_pre_auth(
    frame: &WireFrame,
    live: &LiveInput,
    payload: WirePayload,
) -> WireFrame {
    let envelope = outgoing_envelope_inner(
        frame,
        live,
        payload.message_type(),
        Some(frame.envelope.message_id),
        false,
    );
    WireFrame { envelope, payload }
}
