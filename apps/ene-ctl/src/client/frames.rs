//! Pure outbound frame builders: pairing, capability, auth proof, requests.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
use ene_api::v1::handshake::{AuthProof, CapabilityAdvertise, PairingRequest};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{
    ClientIncarnationId, CommandWireId, DeviceWireId, RequestWireId, WireMessageType,
};
use ene_plugin_ipc::WireFrame;

use crate::device;

static INCARNATION_SEQ: AtomicU64 = AtomicU64::new(0);

static INCARNATION_START: OnceLock<u64> = OnceLock::new();

/// Uniqueness needs are modest (disambiguating restarts of one device) and a
/// collision only risks a duplicate-suppression alias, never a privilege
/// change: pid plus process-local counter plus start-time nanoseconds from
/// `std` only, no OS RNG dependency. Distinct envelope dimension from
/// connection identity and presence generation.
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

/// The proof is the pairing-secret HMAC over the single-use challenge nonce;
/// the sender names the paired device and hides the connection id (still
/// undisclosed pre-accept).
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
pub fn pending_guidance() -> String {
    format!(
        "pairing is pending owner confirmation; approve the device on the \
         Host-local trusted surface, then re-run ene-ctl once with {} set \
         to the shown secret (it is stored to the 0600 client device file)",
        device::BOOTSTRAP_SECRET_ENV,
    )
}

#[must_use]
pub fn missing_secret_guidance() -> String {
    format!(
        "no pairing secret stored for this device; approve the device on \
         the Host-local trusted surface, then re-run ene-ctl once with {} \
         set to the shown secret",
        device::BOOTSTRAP_SECRET_ENV,
    )
}

/// The stored device file exists but is unusable. Distinct from
/// [`missing_secret_guidance`]: re-running the first-run flow without
/// reprovisioning cannot help, because the file must be replaced by a fresh
/// proven secret.
#[must_use]
pub fn unreadable_device_file_guidance() -> String {
    format!(
        "the stored client device file is unreadable or malformed; approve \
         the device again on the Host-local trusted surface, then re-run \
         ene-ctl once with {} set to the fresh secret (the file is replaced \
         only after the proof succeeds)",
        device::BOOTSTRAP_SECRET_ENV,
    )
}

/// Echoing the Host reason is safe: it is operational by DTO contract, never
/// a secret or body copy.
#[must_use]
pub fn auth_rejected_guidance(reason: &str) -> String {
    format!(
        "authentication rejected: {reason}; approve the device again on the \
         Host-local trusted surface and re-run ene-ctl once with {} set to \
         the fresh secret",
        device::BOOTSTRAP_SECRET_ENV,
    )
}

/// Reuses the caller's command id while message and request ids go fresh for
/// this attempt. Same-incarnation retries only (see [`super::Client::retry`]):
/// sender, generation view, and payload travel untouched.
pub fn retry_frame(
    payload: WirePayload,
    sender: WireSender,
    generation: Option<u64>,
    command: CommandWireId,
) -> WireFrame {
    PreparedRequest {
        command_id: Some(command),
        payload,
    }
    .frame(sender, generation)
}

/// A logical send prepared before I/O: the payload plus the command identity a
/// transport retry must reuse. Prepare through [`super::Client::prepare`] and
/// keep the handle; [`super::Client::execute`] and [`super::Client::retry`]
/// send it without rebuilding the identity.
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
    /// Captures one payload's canonical command identity before I/O: a
    /// [`ManagementIntent`](ene_api::v1::management::ManagementIntent) keeps
    /// its `intent_id` (the Host's management idempotency key, never a second
    /// minted id), a
    /// [`SubmitTextInput`](ene_api::v1::round::SubmitTextInput) mints a fresh
    /// [`CommandWireId`], and a pure request carries none.
    #[must_use]
    pub fn new(payload: WirePayload) -> Self {
        let command_id = match &payload {
            WirePayload::ManagementIntent(intent) => Some(intent.intent_id),
            WirePayload::SubmitTextInput(_) => Some(CommandWireId(uuid::Uuid::new_v4())),
            _ => None,
        };
        Self {
            command_id,
            payload,
        }
    }

    /// One transport attempt: the caller-owned command identity travels
    /// unchanged while message and request ids go fresh, so re-sending the
    /// same handle replays one logical command (IPC §6.2) instead of minting
    /// a new one.
    pub(super) fn frame(&self, sender: WireSender, generation: Option<u64>) -> WireFrame {
        let mut frame = frame_for_session(self.payload.clone(), sender, generation);
        frame.envelope.correlation.command_id = self.command_id;
        frame.envelope.correlation.request_id = Some(RequestWireId(uuid::Uuid::new_v4()));
        frame
    }
}

/// Stamps `observed.presence_generation_view` on
/// [`SubmitTextInput`](ene_api::v1::round::SubmitTextInput) sends only: the
/// generation view is intake comparison material, not a general envelope
/// claim.
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

/// Pre-pairing sender: the Host issues the device ID after Owner
/// confirmation.
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

/// Speaks [`ProtocolVersion::V1`] and carries the display platform string.
/// `connect` passes the paired device the paired-sender contract requires;
/// pre-pairing callers (and tests) pass [`None`].
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
