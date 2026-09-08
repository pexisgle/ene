//! Host transport: socket path, frame builders, handshake, request/response.
//!
//! The CLI dials the Host over a Unix-domain socket at
//! [`socket_path`] (`ene.sock` inside the resolved data directory; the socket
//! name is Stage-2 provisional). The handshake is pairing plus capability
//! advertisement only: [`PairingRequest`]
//! (display descriptor, pre-pairing sender with no device ID) must answer
//! [`Paired`](ene_api::v1::handshake::PairingResult::Paired), then
//! [`CapabilityAdvertise`]
//! must answer
//! [`NegotiatedConnection`](ene_api::v1::handshake::NegotiatedConnection)
//! with a matching major version. Authentication (`AuthChallenge`/`AuthProof`)
//! and reconnection are later-stage scope; this client sends no connection ID.
//!
//! Framing goes through `ene-plugin-ipc` only ([`encode_frame`]/[`decode_frame`]); this module
//! owns the socket read/write loops. [`CodecError`]
//! displays carry lengths and decoder reasons only and never echo frame
//! bytes, so mapping them into [`CliError::Codec`]
//! cannot leak conversation text. All other error messages carry operations,
//! payload-kind names, refs, or generations — never bodies or secrets.
//!
//! Non-Unix platforms get stubs returning
//! [`CliError::UnsupportedPlatform`];
//! the pure builders below stay shared.

use std::path::{Path, PathBuf};

use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
use ene_api::v1::handshake::{CapabilityAdvertise, PairingRequest};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{ClientIncarnationId, DeviceWireId, WireMessageType};
use ene_plugin_ipc::{CodecError, MAX_FRAME_BYTES, WireFrame, decode_frame, encode_frame};

use crate::CliError;

/// Returns the Host socket path for `data_dir`: `<data_dir>/ene.sock`.
///
/// Pure and side-effect free; the caller decides whether the directory or
/// socket must exist (absence surfaces as [`CliError::Transport`] on dial).
pub(crate) fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ene.sock")
}

/// Display-only platform string for pairing and capability frames, from the
/// compile-time OS and architecture (for example `"linux-x86_64"`). Display
/// only, never permission evidence.
pub(crate) fn platform_display() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Mints this process's incarnation: sequence zero (one boot), randomness
/// from wall-clock nanoseconds folded with the process ID.
///
/// Uniqueness needs are modest — disambiguating restarts of one device —
/// and a collision only risks a duplicate-suppression alias, never a
/// privilege change, so a clock-derived value documented here is enough.
pub(crate) fn new_incarnation() -> ClientIncarnationId {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    ClientIncarnationId {
        counter: 0,
        random: (nanos as u64) ^ u64::from(std::process::id()),
    }
}

/// Builds the pairing frame: display descriptor, pre-pairing sender (no
/// device ID yet — the Host issues it after Owner confirmation).
pub(crate) fn pairing_frame(descriptor: &str, incarnation: ClientIncarnationId) -> WireFrame {
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
pub(crate) fn capability_frame(
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
pub(crate) fn frame_for(payload: WirePayload, sender: WireSender) -> WireFrame {
    let message_type = message_type_for(&payload);
    let envelope = new_outgoing_envelope(ProtocolVersion::V1, sender, message_type);
    WireFrame { envelope, payload }
}

/// Static kind name of a payload, used in rejection messages (no bodies).
pub(crate) fn payload_kind(payload: &WirePayload) -> &'static str {
    match payload {
        WirePayload::PairingRequest(_) => "PairingRequest",
        WirePayload::PairingResult(_) => "PairingResult",
        WirePayload::AuthChallenge(_) => "AuthChallenge",
        WirePayload::AuthProof(_) => "AuthProof",
        WirePayload::AuthResult(_) => "AuthResult",
        WirePayload::CapabilityAdvertise(_) => "CapabilityAdvertise",
        WirePayload::NegotiatedConnection(_) => "NegotiatedConnection",
        WirePayload::ReconnectHello(_) => "ReconnectHello",
        WirePayload::RecoveryInvite(_) => "RecoveryInvite",
        WirePayload::DisconnectNotice(_) => "DisconnectNotice",
        WirePayload::SubmitTextInput(_) => "SubmitTextInput",
        WirePayload::RoundIntakeOutcome(_) => "RoundIntakeOutcome",
        WirePayload::TextStreamOpen(_) => "TextStreamOpen",
        WirePayload::TextStreamFrame(_) => "TextStreamFrame",
        WirePayload::TextStreamClose(_) => "TextStreamClose",
        WirePayload::ConfirmPresentation(_) => "ConfirmPresentation",
        WirePayload::HistoryRequest(_) => "HistoryRequest",
        WirePayload::HistoryView(_) => "HistoryView",
        WirePayload::PresenceAttribution(_) => "PresenceAttribution",
        WirePayload::ManagementIntent(_) => "ManagementIntent",
        WirePayload::ManagementOutcome(_) => "ManagementOutcome",
        WirePayload::ManagementViewRequest(_) => "ManagementViewRequest",
        WirePayload::ManagementView(_) => "ManagementView",
    }
}

/// Envelope discriminator for a payload: the variant name, matching the
/// convention the wire tests use (for example `"SubmitTextInput"`).
/// Routing hint only; the Host rejects unknown names, never guesses.
pub(crate) fn message_type_for(payload: &WirePayload) -> WireMessageType {
    WireMessageType(String::from(payload_kind(payload)))
}

/// Connected, handshaked Host session (Unix): the stream plus the sender
/// identity established by pairing (device ID filled in, no connection ID —
/// authentication is later-stage scope).
#[cfg(unix)]
pub(crate) struct Client {
    /// Framed Host connection.
    stream: tokio::net::UnixStream,
    /// Sender identity for subsequent frames.
    sender: WireSender,
}

#[cfg(unix)]
impl Client {
    /// Dials `ene.sock` under `data_dir` and runs the pairing-plus-capability
    /// handshake.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Transport`] when the socket cannot be reached or
    /// a frame cannot be moved, [`CliError::Codec`] when a frame cannot be
    /// encoded or decoded, [`CliError::ServerOutcome`] when pairing is still
    /// pending Owner confirmation, and [`CliError::ServerRejected`] when the
    /// Host denies pairing, negotiates an incompatible version, or answers
    /// with an unexpected payload kind.
    pub(crate) async fn connect(
        data_dir: &Path,
        descriptor: &str,
        platform: &str,
    ) -> Result<Self, CliError> {
        let path = socket_path(data_dir);
        let mut stream = tokio::net::UnixStream::connect(&path)
            .await
            .map_err(|error| {
                CliError::Transport(format!(
                    "connect to {} failed: {}",
                    path.display(),
                    error.kind()
                ))
            })?;
        let incarnation = new_incarnation();
        write_frame(&mut stream, &pairing_frame(descriptor, incarnation)).await?;
        let device_id = match read_frame(&mut stream).await?.payload {
            WirePayload::PairingResult(result) => match result {
                ene_api::v1::handshake::PairingResult::Paired { device_id } => device_id,
                ene_api::v1::handshake::PairingResult::PendingOwnerConfirmation => {
                    return Err(CliError::ServerOutcome(String::from(
                        "pairing is pending owner confirmation; retry after approval",
                    )));
                }
                ene_api::v1::handshake::PairingResult::Denied { reason } => {
                    // `reason` is operational by DTO contract (never a
                    // secret or body copy), so echoing it is safe.
                    return Err(CliError::ServerRejected(format!(
                        "pairing denied: {reason}"
                    )));
                }
            },
            unexpected => {
                return Err(CliError::ServerRejected(format!(
                    "unexpected {} during pairing; expected PairingResult",
                    payload_kind(&unexpected)
                )));
            }
        };
        write_frame(
            &mut stream,
            &capability_frame(platform, incarnation, Some(device_id)),
        )
        .await?;
        match read_frame(&mut stream).await?.payload {
            WirePayload::NegotiatedConnection(negotiated) => {
                if !negotiated.version.shares_major_with(&ProtocolVersion::V1) {
                    return Err(CliError::ServerRejected(format!(
                        "negotiated incompatible version {}.{}; expected major 1",
                        negotiated.version.major, negotiated.version.minor
                    )));
                }
            }
            unexpected => {
                return Err(CliError::ServerRejected(format!(
                    "unexpected {} during capability negotiation; expected NegotiatedConnection",
                    payload_kind(&unexpected)
                )));
            }
        }
        Ok(Self {
            stream,
            sender: WireSender {
                device_id: Some(device_id),
                incarnation_id: incarnation,
                connection_id: None,
            },
        })
    }

    /// Sends one payload frame and reads the single answering frame.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Transport`] or [`CliError::Codec`] when the
    /// exchange cannot be moved or framed. Payload semantics are the
    /// caller's job: this helper never interprets the answer.
    pub(crate) async fn request(&mut self, payload: WirePayload) -> Result<WirePayload, CliError> {
        write_frame(&mut self.stream, &frame_for(payload, self.sender)).await?;
        Ok(read_frame(&mut self.stream).await?.payload)
    }

    /// Reads the next incoming frame payload (stream follower for `send`).
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Transport`] or [`CliError::Codec`] when the next
    /// frame cannot be read or decoded.
    pub(crate) async fn next_frame(&mut self) -> Result<WirePayload, CliError> {
        Ok(read_frame(&mut self.stream).await?.payload)
    }
}

/// Encodes `frame` and writes it as one length-prefixed unit.
#[cfg(unix)]
async fn write_frame(
    stream: &mut tokio::net::UnixStream,
    frame: &WireFrame,
) -> Result<(), CliError> {
    use tokio::io::AsyncWriteExt as _;
    let bytes = encode_frame(frame)
        .map_err(|error: CodecError| CliError::Codec(format!("encode failed: {error}")))?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|error| CliError::Transport(format!("socket write failed: {}", error.kind())))?;
    Ok(())
}

/// Reads one length-prefixed frame: 4-byte big-endian body length, then the
/// body. The cap is checked before any body-sized allocation, so a hostile
/// prefix cannot drive unbounded allocation.
#[cfg(unix)]
async fn read_frame(stream: &mut tokio::net::UnixStream) -> Result<WireFrame, CliError> {
    use tokio::io::AsyncReadExt as _;
    let mut prefix = [0_u8; 4];
    stream
        .read_exact(&mut prefix)
        .await
        .map_err(|error| CliError::Transport(format!("socket read failed: {}", error.kind())))?;
    let claimed = u32::from_be_bytes(prefix) as usize;
    if claimed > MAX_FRAME_BYTES {
        return Err(CliError::Codec(format!(
            "frame body of {claimed} bytes exceeds the 256 KiB cap"
        )));
    }
    let mut body = vec![0_u8; claimed];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|error| CliError::Transport(format!("socket read failed: {}", error.kind())))?;
    let mut bytes = Vec::with_capacity(4 + claimed);
    bytes.extend_from_slice(&prefix);
    bytes.extend_from_slice(&body);
    decode_frame(&bytes)
        .map(|(frame, _consumed)| frame)
        .map_err(|error: CodecError| CliError::Codec(format!("decode failed: {error}")))
}

/// Non-Unix placeholder: same surface, always unsupported.
#[cfg(windows)]
pub(crate) struct Client {
    /// Unconstructible: there is no socket to hold.
    _sealed: (),
}

#[cfg(windows)]
impl Client {
    /// Always reports unsupported: transport needs a Unix-domain socket.
    ///
    /// # Errors
    ///
    /// Always returns [`CliError::UnsupportedPlatform`].
    pub(crate) async fn connect(
        _data_dir: &Path,
        _descriptor: &str,
        _platform: &str,
    ) -> Result<Self, CliError> {
        Err(CliError::UnsupportedPlatform("unix socket transport"))
    }

    /// Always reports unsupported: transport needs a Unix-domain socket.
    ///
    /// # Errors
    ///
    /// Always returns [`CliError::UnsupportedPlatform`].
    pub(crate) async fn request(&mut self, _payload: WirePayload) -> Result<WirePayload, CliError> {
        Err(CliError::UnsupportedPlatform("unix socket transport"))
    }

    /// Always reports unsupported: transport needs a Unix-domain socket.
    ///
    /// # Errors
    ///
    /// Always returns [`CliError::UnsupportedPlatform`].
    pub(crate) async fn next_frame(&mut self) -> Result<WirePayload, CliError> {
        Err(CliError::UnsupportedPlatform("unix socket transport"))
    }
}

#[cfg(test)]
mod tests {
    //! Builder shapes, socket path, and in-memory codec roundtrips.
    //!
    //! No sockets are opened: frames go through the in-memory codec only.

    use ene_api::v1::envelope::ProtocolVersion;
    use ene_api::v1::payload::WirePayload;

    use super::{ClientIncarnationId, WireSender};
    use super::{
        capability_frame, frame_for, message_type_for, new_incarnation, pairing_frame,
        payload_kind, platform_display, socket_path,
    };

    /// Fixed incarnation so built frames are deterministic.
    fn incarnation() -> ClientIncarnationId {
        ClientIncarnationId {
            counter: 0,
            random: 7,
        }
    }

    /// Yields `Ok` values without `unwrap`/`expect` (both denied): the
    /// `assert!` fails the test first, so the `else` branch is only a
    /// type-level fallback, never a silent pass.
    fn require_ok<T: core::fmt::Debug, E: core::fmt::Debug>(
        result: Result<T, E>,
        what: &str,
    ) -> Option<T> {
        assert!(result.is_ok(), "{what} unexpectedly failed: {result:?}");
        result.ok()
    }

    #[test]
    fn socket_path_appends_ene_sock() {
        let dir = std::path::Path::new("/tmp/ene-data");
        assert!(
            socket_path(dir) == dir.join("ene.sock"),
            "socket path must be ene.sock under the data dir"
        );
    }

    #[test]
    fn platform_display_names_os_and_arch() {
        let display = platform_display();
        assert!(
            display == format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            "platform display must name OS and arch, got {display:?}"
        );
    }

    #[test]
    fn pairing_frame_is_pre_pairing_v1() {
        let frame = pairing_frame("Owner laptop", incarnation());
        let WirePayload::PairingRequest(request) = &frame.payload else {
            return;
        };
        assert!(
            request.device_descriptor == "Owner laptop",
            "pairing keeps the display descriptor"
        );
        assert!(
            frame.envelope.protocol == ProtocolVersion::V1,
            "pairing speaks V1"
        );
        assert!(
            frame.envelope.sender.device_id.is_none(),
            "pre-pairing sender carries no device ID"
        );
        assert!(
            frame.envelope.message_type.0 == "PairingRequest",
            "pairing names its payload shape"
        );
    }

    #[test]
    fn capability_frame_claims_no_features_and_threads_device() {
        let sender_device = ene_api::v1::refs::DeviceWireId(uuid::Uuid::new_v4());
        let frame = capability_frame("linux-x86_64", incarnation(), Some(sender_device));
        let WirePayload::CapabilityAdvertise(advertise) = &frame.payload else {
            return;
        };
        assert!(
            advertise.supported_protocol == vec![ProtocolVersion::V1],
            "capability speaks V1"
        );
        assert!(
            advertise.features.is_empty(),
            "text is the baseline, not a claimed feature"
        );
        assert!(
            advertise.platform == "linux-x86_64",
            "capability carries the display platform"
        );
        assert!(
            frame.envelope.sender.device_id == Some(sender_device),
            "capability threads the paired device ID"
        );
        assert!(
            frame.envelope.message_type.0 == "CapabilityAdvertise",
            "capability names its payload shape"
        );
    }

    #[test]
    fn message_type_names_the_variant() {
        let sender = WireSender {
            device_id: None,
            incarnation_id: incarnation(),
            connection_id: None,
        };
        let frame = frame_for(
            WirePayload::HistoryRequest(super::super::cmds::history_request(3)),
            sender,
        );
        assert!(
            message_type_for(&frame.payload).0 == "HistoryRequest",
            "discriminator must name the variant"
        );
        assert!(
            payload_kind(&frame.payload) == "HistoryRequest",
            "kind name must match the discriminator"
        );
        assert!(
            frame.envelope.protocol == ProtocolVersion::V1,
            "built frames speak V1"
        );
    }

    #[test]
    fn pairing_frame_survives_the_wire_codec() {
        let frame = pairing_frame("Owner laptop", incarnation());
        let Some(encoded) =
            require_ok(ene_plugin_ipc::encode_frame(&frame), "encode pairing frame")
        else {
            return;
        };
        let Some((decoded, consumed)) = require_ok(
            ene_plugin_ipc::decode_frame(&encoded),
            "decode pairing frame",
        ) else {
            return;
        };
        assert!(consumed == encoded.len(), "decode must consume the frame");
        assert!(decoded == frame, "codec must preserve the pairing frame");
    }

    #[test]
    fn capability_frame_survives_the_wire_codec() {
        let frame = capability_frame("linux-x86_64", incarnation(), None);
        let Some(encoded) = require_ok(
            ene_plugin_ipc::encode_frame(&frame),
            "encode capability frame",
        ) else {
            return;
        };
        let Some((decoded, _consumed)) = require_ok(
            ene_plugin_ipc::decode_frame(&encoded),
            "decode capability frame",
        ) else {
            return;
        };
        assert!(decoded == frame, "codec must preserve the capability frame");
    }

    #[test]
    fn incarnation_is_boot_zero() {
        assert!(
            new_incarnation().counter == 0,
            "incarnation sequence starts at boot zero"
        );
    }
}
