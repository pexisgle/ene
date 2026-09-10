//! Pure outbound frame builders: pairing, capability, auth proof, requests.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
use ene_api::v1::handshake::{AuthProof, CapabilityAdvertise, PairingRequest};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{
    ClientIncarnationId, CommandWireId, DeviceWireId, RequestWireId, WireMessageId, WireMessageType,
};
use ene_plugin_ipc::WireFrame;

use crate::device;

/// Per-process incarnation sequence backing [`new_incarnation`]: `std` only,
/// process pid plus a process-local monotonic counter plus start-time
/// nanoseconds (see the function docs).
static INCARNATION_SEQ: AtomicU64 = AtomicU64::new(0);

/// Start-time nanoseconds memo for [`new_incarnation`], captured once.
static INCARNATION_START: OnceLock<u64> = OnceLock::new();

/// Mints this process's incarnation: `counter` is the process pid (unique per
/// boot per device for distinct processes), `random` folds a process-local
/// monotonic sequence into the process start-time nanoseconds.
///
/// Uniqueness needs are modest — disambiguating restarts of one device —
/// and a collision only risks a duplicate-suppression alias, never a
/// privilege change, so clock-plus-counter randomness from `std` only
/// (no OS RNG dependency) documented here is enough. This never collapses
/// with connection identity or presence generation: the three stay separate
/// envelope dimensions.
pub fn new_incarnation() -> ClientIncarnationId {
    let start = *INCARNATION_START.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos() as u64)
    });
    let seq = INCARNATION_SEQ.fetch_add(1, Ordering::Relaxed);
    ClientIncarnationId {
        counter: u64::from(std::process::id()),
        random: start.wrapping_add(seq),
    }
}

/// Builds the proof frame: the wire sender names the paired device under
/// proof (the Host attributes through its connection table and never trusts
/// the claim, but the paired-sender contract carries it), echoes the
/// caller incarnation, and hides the connection id (still undisclosed
/// pre-accept). The proof itself is the pairing-secret HMAC over the
/// single-use challenge nonce.
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

/// Guidance for a still-pending pairing: approve on the Host-local trusted
/// surface, then re-run once with the shown secret in the environment so it
/// reaches the `0600` device file.
#[must_use]
pub fn pending_guidance() -> String {
    format!(
        "pairing is pending owner confirmation; approve the device on the \
         Host-local trusted surface, then re-run ene-ctl once with {} set \
         to the shown secret (it is stored to the 0600 client device file)",
        device::BOOTSTRAP_SECRET_ENV,
    )
}

/// Guidance for a challenge that arrived with no secret to prove with: the
/// operator must approve and provision before authentication can run.
#[must_use]
pub fn missing_secret_guidance() -> String {
    format!(
        "no pairing secret stored for this device; approve the device on \
         the Host-local trusted surface, then re-run ene-ctl once with {} \
         set to the shown secret",
        device::BOOTSTRAP_SECRET_ENV,
    )
}

/// Guidance for a rejected proof: the Host reason plus the re-provisioning
/// step. The reason is operational by DTO contract, so echoing it is safe.
#[must_use]
pub fn auth_rejected_guidance(reason: &str) -> String {
    format!(
        "authentication rejected: {reason}; approve the device again on the \
         Host-local trusted surface and re-run ene-ctl once with {} set to \
         the fresh secret",
        device::BOOTSTRAP_SECRET_ENV,
    )
}

/// Builds a retry frame: the caller's command id travels unchanged while
/// message and request ids go fresh for this attempt only. Same incarnation
/// only (see [`Client::retry`]): the sender, generation view, and payload
/// are reused untouched. Pure: the transport pairing in [`Client::retry`]
/// moves it unchanged.
pub fn retry_frame(
    payload: WirePayload,
    sender: WireSender,
    generation: Option<u64>,
    command: CommandWireId,
) -> WireFrame {
    let mut frame = frame_for_session(payload, sender, generation);
    frame.envelope.correlation.command_id = Some(command);
    frame.envelope.correlation.request_id = Some(RequestWireId(uuid::Uuid::new_v4()));
    frame
}
/// `observed.presence_generation_view` with the session value on
/// [`SubmitTextInput`](ene_api::v1::round::SubmitTextInput) sends only. Other
/// payloads keep the [`None`] default: the generation view is intake
/// comparison material, not a general envelope claim.
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

/// Builds the pairing frame: display descriptor, pre-pairing sender (no
/// device ID yet — the Host issues it after Owner confirmation).
pub fn pairing_frame(descriptor: &str, incarnation: ClientIncarnationId) -> WireFrame {
    frame_for(
        WirePayload::PairingRequest(PairingRequest {
            device_descriptor: String::from(descriptor),
        }),
        WireSender {
            device_id: None,
            incarnation_id: incarnation,
            connection_id: None,
        },
    )
}

/// Builds the capability frame: speaks [`ProtocolVersion::V1`], claims no
/// optional features (text is the baseline, not a capability), and carries
/// the display platform string.
///
/// `connect` passes the paired device: the paired-sender contract names it
/// on capability and proof frames alike. The Host still attributes through
/// its connection table (paired moments earlier on this same connection)
/// and never trusts the claim — a mismatched claim drops the frame — so the
/// value here satisfies the wire contract without becoming authority.
/// Pre-pairing callers (and tests) pass [`None`].
pub fn capability_frame(
    platform: &str,
    incarnation: ClientIncarnationId,
    device_id: Option<DeviceWireId>,
) -> WireFrame {
    frame_for(
        WirePayload::CapabilityAdvertise(CapabilityAdvertise {
            supported_protocol: vec![ProtocolVersion::V1],
            features: Vec::new(),
            platform: String::from(platform),
        }),
        WireSender {
            device_id,
            incarnation_id: incarnation,
            connection_id: None,
        },
    )
}

/// Wraps `payload` in a [`ProtocolVersion::V1`] envelope for `sender`.
pub fn frame_for(payload: WirePayload, sender: WireSender) -> WireFrame {
    let message_type = message_type_for(&payload);
    let envelope = new_outgoing_envelope(ProtocolVersion::V1, sender, message_type);
    WireFrame { envelope, payload }
}

/// Stamps a fresh command ID on one outgoing request envelope and reports
/// the message ID the Host echoes in `reply_to`.
///
/// One fresh [`CommandWireId`] per send (never reused across retries at this
/// layer): the Host pairs its reply by `reply_to` against the returned
/// message ID, and the command ID keeps every request uniformly pairable as
/// command-side correlation grows. Handshake frames skip this (they rely on
/// message-ID pairing only); fire-and-forget observations skip it too (no
/// reply is ever paired to them). Transport retry of one logical send
/// reuses the ID through [`Client::retry`] instead.
pub(super) fn stamp_request(frame: &mut WireFrame) -> WireMessageId {
    frame.envelope.correlation.command_id = Some(CommandWireId(uuid::Uuid::new_v4()));
    frame.envelope.correlation.request_id = Some(RequestWireId(uuid::Uuid::new_v4()));
    frame.envelope.message_id
}

/// Static kind name of a payload, used in rejection messages (no bodies).
///
/// Delegates to the canonical [`WirePayload::message_type`] vocabulary in
/// `ene-api`: the wire names live in exactly one place, so adding a variant
/// can never leave a second exhaustive list behind.
pub fn payload_kind(payload: &WirePayload) -> &'static str {
    payload.message_type()
}

/// Envelope discriminator for a payload: the variant name, matching the
/// convention the wire tests use (for example `"SubmitTextInput"`).
/// Routing hint only; the Host rejects unknown names, never guesses.
pub fn message_type_for(payload: &WirePayload) -> WireMessageType {
    WireMessageType(String::from(payload_kind(payload)))
}
