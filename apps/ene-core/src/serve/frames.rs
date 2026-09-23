use super::LiveInput;
use ene_api::v1::envelope::{ProtocolVersion, WireEnvelope, WireSender, new_outgoing_envelope};
use ene_api::v1::handshake::DisconnectNotice;
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{DeviceWireId, WireMessageId, WireMessageType};
use ene_api::v1::reject::{IncompatibleProtocol, RejectKind, RejectNotice};
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

/// Builds one typed `StaleConnection` rejection answering `frame`.
///
/// A superseded connection keeps its socket (IPC §11.3): the frame's
/// attribution was verifiable, so the honest typed outcome is sent instead of
/// a drop. Whether the rejection reveals the connection id follows
/// [`reject_frame`]'s rule: a rejection built from a pre-auth or
/// superseded-phase snapshot hides it, while one built from an authenticated
/// intake snapshot keeps it. It is never a retry signal.
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
    let envelope = outgoing_envelope(frame, live, &payload, None);
    WireFrame { envelope, payload }
}

/// Builds the terminal negotiation rejection for a major mismatch (IPC §7.2).
///
/// The Host names its own highest version, the Client's highest advertised
/// version, and upgrade guidance; `client_max` is the caller's projection of
/// the Client's claim. The frame hides the connection id like every pre-accept
/// answer — the peer never negotiated, so it never learns the id the ingress
/// gate requires it to echo — and the connection closes after it (see
/// [`crate::conn`]).
pub(crate) fn incompatible_protocol(
    frame: &WireFrame,
    live: &LiveInput,
    client_max: ProtocolVersion,
) -> WireFrame {
    let host_max = ProtocolVersion::V1;
    outgoing_frame_pre_auth(
        frame,
        live,
        WirePayload::IncompatibleProtocol(IncompatibleProtocol {
            host_max,
            client_max,
            hint: upgrade_hint(host_max),
        }),
    )
}

/// Upgrade guidance for a major mismatch: the protocol major the Client must
/// move to. One message serves both directions (an older Client updates, a
/// newer Client downgrades), matching IPC §7.2 and V-11.
fn upgrade_hint(host_max: ProtocolVersion) -> String {
    format!(
        "use a client release sharing the host's protocol major {}",
        host_max.major
    )
}

/// Builds one typed wire rejection answering `frame`.
///
/// Per IPC §5, a rejection on an authenticated connection (`live.authed`)
/// carries this connection's table id; a pre-auth rejection hides it.
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
