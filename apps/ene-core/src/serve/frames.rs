//! Outgoing envelope and frame builders shared by every dispatch path.
//!
//! The `message_type` always comes from [`WirePayload::message_type`]; no
//! caller passes it separately, so envelope and body cannot disagree.

use super::LiveInput;
use ene_api::v1::envelope::{ProtocolVersion, WireEnvelope, WireSender, new_outgoing_envelope};
use ene_api::v1::handshake::DisconnectNotice;
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{DeviceWireId, WireMessageId, WireMessageType};
use ene_api::v1::reject::{RejectKind, RejectNotice};
use ene_plugin_ipc::WireFrame;
use uuid::Uuid;

/// Builds the terminal gate frame dropping unauthenticated domain service.
///
/// The connection closes after this frame is written. The `"unpaired"` reason
/// names the gate trip only; a disconnect (rather than a `Reject` denial,
/// which exists for post-auth declines) is the explicit decision, so an
/// unauthenticated peer gets no oracle. The frame hides the connection id: a
/// peer that never completed the challenge must not learn it from the drop.
pub(crate) fn unpaired_close(frame: &WireFrame, live: &LiveInput) -> WireFrame {
    outgoing_frame_pre_auth(
        frame,
        live,
        WirePayload::DisconnectNotice(DisconnectNotice {
            reason: String::from("unpaired"),
        }),
    )
}

/// Builds the Host sender for one response to `frame` under `live`.
///
/// Host-to-Client addressing always echoes the inbound incarnation (so the
/// Client pairs the response with its connection state) and names the paired
/// device target when this connection paired one (the opaque projection the
/// table holds, passed through verbatim; an unparsable entry — never
/// written by this Host — maps to [`None`]); pre-pairing responses carry
/// device [`None`]. The connection id travels only when `reveal_connection`
/// holds: acceptance and later domain responses reveal this connection's
/// table id, while every pre-accept response hides it ([`None`]), so a peer
/// that never completed the challenge never learns the id the gate requires
/// it to echo.
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

/// Builds an outgoing envelope for a `Stage 2` message.
///
/// `reply_to` links the response to its request for transport pairing; domain
/// correspondence travels in the payloads, never here. The sender follows
/// [`response_sender`].
pub(crate) fn outgoing_envelope(
    frame: &WireFrame,
    live: &LiveInput,
    payload: &WirePayload,
    reply_to: Option<WireMessageId>,
) -> WireEnvelope {
    outgoing_envelope_inner(frame, live, payload.message_type(), reply_to, true)
}

pub(crate) fn outgoing_envelope_pre_auth(
    frame: &WireFrame,
    live: &LiveInput,
    payload: &WirePayload,
    reply_to: Option<WireMessageId>,
) -> WireEnvelope {
    outgoing_envelope_inner(frame, live, payload.message_type(), reply_to, false)
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

/// Builds one response frame answering `frame` with `payload`.
///
/// Use only on and after
/// [`Accepted`](ene_api::v1::handshake::AuthResult::Accepted): the acceptance
/// itself, the piggybacked presence fact, and every domain response reveal
/// the connection id.
pub(crate) fn outgoing_frame(
    frame: &WireFrame,
    live: &LiveInput,
    payload: WirePayload,
) -> WireFrame {
    let envelope = outgoing_envelope(frame, live, &payload, Some(frame.envelope.message_id));
    WireFrame { envelope, payload }
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
    let envelope =
        outgoing_envelope_pre_auth(frame, live, &payload, Some(frame.envelope.message_id));
    WireFrame { envelope, payload }
}
