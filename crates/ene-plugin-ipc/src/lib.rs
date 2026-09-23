//! Length-prefixed `MessagePack` transport frames (IPC §10.1), shared by the
//! Host listener and the Client dialer.
//!
//! This is a pure byte codec: it frames one domain message ([`WireFrame`]) as
//! a 4-byte big-endian exclusive length prefix followed by the canonical
//! `MessagePack` body (IPC §7), and parses such bytes back. It performs no
//! I/O, owns no sockets, and runs no async tasks; socket read/write loops
//! live in the applications that embed it.
//!
//! One frame carries exactly one domain message. Text streaming chunking
//! happens at the DTO level ([`ene_api::v1::round::TextStreamFrameWire`]),
//! never here: this layer never splits, merges, or otherwise interprets
//! payloads. It never inspects envelope or payload semantics either; domain
//! meaning (routing, validation, authority) stays in `ene-api` and the
//! Host. Unknown-field tolerance comes from the named `MessagePack`
//! encoding (structs as maps) together with the `ene-api` DTOs, not from
//! any logic here.

use std::path::Path;

use ene_api::v1::envelope::WireEnvelope;
use ene_api::v1::payload::WirePayload;
use serde::{Deserialize, Serialize};

const LEN_PREFIX_LEN: usize = 4;

/// Pipe name for one Host data directory.
///
/// Named pipes live in a flat per-machine namespace, so the data directory
/// is folded into the name: FNV-1a (64-bit, fixed offsets, so the name is
/// stable across processes) over its string form, rendered as hex. Backslash
/// can never appear in the hex tag. One definition: the Host listener, the
/// Client dialer, and the first-party control inlet derive the same name.
#[must_use]
pub fn pipe_name(data_dir: &Path) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;
    let mut tag = FNV_OFFSET;
    for byte in data_dir.as_os_str().as_encoded_bytes() {
        tag ^= u64::from(*byte);
        tag = tag.wrapping_mul(FNV_PRIME);
    }
    format!(r"\\.\pipe\ene-{tag:016x}")
}

/// Maximum `MessagePack` body length in bytes, exclusive of the prefix. The
/// bound keeps a single hostile or corrupt length prefix from driving
/// unbounded allocation while comfortably fitting text round-trip traffic.
pub const MAX_FRAME_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireFrame {
    pub envelope: WireEnvelope,
    pub payload: WirePayload,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    #[error("frame body of {len} bytes exceeds the 256 KiB cap")]
    FrameTooLarge { len: usize },
    #[error("truncated frame: have {have} bytes, need {need}")]
    Truncated { have: usize, need: usize },
    #[error("frame body failed to decode: {reason}")]
    DecodeFailed { reason: String },
    /// The body could not be serialized as a [`WireFrame`].
    #[error("frame body failed to encode: {reason}")]
    EncodeFailed { reason: String },
}

pub fn encode_frame(frame: &WireFrame) -> Result<Vec<u8>, CodecError> {
    let body = rmp_serde::to_vec_named(frame).map_err(|error| CodecError::EncodeFailed {
        reason: std::format!("{error}"),
    })?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(CodecError::FrameTooLarge { len: body.len() });
    }
    let len_prefix = body.len() as u32;
    let mut out = Vec::with_capacity(LEN_PREFIX_LEN + body.len());
    out.extend_from_slice(&len_prefix.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

pub fn decode_frame(bytes: &[u8]) -> Result<(WireFrame, usize), CodecError> {
    if bytes.len() < LEN_PREFIX_LEN {
        return Err(CodecError::Truncated {
            have: bytes.len(),
            need: LEN_PREFIX_LEN,
        });
    }
    let claimed = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if claimed > MAX_FRAME_BYTES {
        return Err(CodecError::FrameTooLarge { len: claimed });
    }
    let need = LEN_PREFIX_LEN + claimed;
    if bytes.len() < need {
        return Err(CodecError::Truncated {
            have: bytes.len(),
            need,
        });
    }
    let frame = rmp_serde::from_slice(&bytes[LEN_PREFIX_LEN..need]).map_err(|error| {
        CodecError::DecodeFailed {
            reason: decode_reason(&error),
        }
    })?;
    Ok((frame, need))
}

/// Structural decode text without any frame-derived value. `Syntax` embeds the
/// unexpected value (serde's `invalid_type`/`unknown variant` text), which a
/// corrupt or cross-version body could have stuffed with conversation content.
fn decode_reason(error: &rmp_serde::decode::Error) -> String {
    match error {
        rmp_serde::decode::Error::Syntax(_) => {
            String::from("frame body does not match the expected structure")
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{CodecError, MAX_FRAME_BYTES, WireFrame, decode_frame, encode_frame};
    use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
    use ene_api::v1::payload::WirePayload;
    use ene_api::v1::refs::{
        ClientIncarnationId, ClientLocalId, CompanionWireRef, TextLangWire, WireMessageType,
    };
    use ene_api::v1::round::{SubmitTextInput, TextBodyWire};

    fn sample_frame() -> WireFrame {
        let sender = WireSender {
            device_id: None,
            incarnation_id: ClientIncarnationId {
                counter: 7,
                random: 42,
            },
            connection_id: None,
        };
        let envelope = new_outgoing_envelope(
            ProtocolVersion::V1,
            sender,
            WireMessageType(String::from("test-message")),
        );
        let payload = WirePayload::SubmitTextInput(SubmitTextInput {
            companion: CompanionWireRef(String::from("companion-1")),
            round: None,
            fresh: false,
            local_id: ClientLocalId(String::from("local-1")),
            body: TextBodyWire {
                text: String::from("hello"),
                lang: TextLangWire(String::from("en")),
            },
        });
        WireFrame { envelope, payload }
    }

    #[test]
    fn pipe_name_is_the_stable_data_directory_vector() {
        assert_eq!(
            super::pipe_name(std::path::Path::new("/tmp/ene-data")),
            String::from(r"\\.\pipe\ene-2c2d8a5218b804b9"),
        );
    }

    #[test]
    fn roundtrip_preserves_envelope_and_payload() {
        let frame = sample_frame();
        let encoded = encode_frame(&frame).expect("encode frame");
        let (decoded, consumed) = decode_frame(&encoded).expect("decode frame");
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded, frame);
    }

    #[test]
    fn prefix_is_big_endian_body_length() {
        let frame = sample_frame();
        let body = rmp_serde::to_vec_named(&frame).expect("encode body");
        let encoded = encode_frame(&frame).expect("encode frame");
        let body_len = u32::try_from(body.len()).expect("sample body fits in u32");
        let mut expected = body_len.to_be_bytes().to_vec();
        expected.extend_from_slice(&body);
        assert_eq!(encoded, expected);
        assert_eq!(encoded.len(), 4 + body.len());
    }

    #[test]
    fn short_prefix_is_truncated_needing_four() {
        let encoded = encode_frame(&sample_frame()).expect("encode frame");
        assert!(encoded.len() > 4, "sample must carry a body");
        for have in 0..4 {
            let error = decode_frame(&encoded[..have]).expect_err("decode short prefix");
            assert_eq!(
                error,
                CodecError::Truncated { have, need: 4 },
                "short prefix reports have/need"
            );
        }
    }

    #[test]
    fn short_body_is_truncated_needing_frame_total() {
        let encoded = encode_frame(&sample_frame()).expect("encode frame");
        assert!(encoded.len() > 4, "sample must carry a body");
        let cut = encoded.len() - 1;
        let error = decode_frame(&encoded[..cut]).expect_err("decode short body");
        assert_eq!(
            error,
            CodecError::Truncated {
                have: cut,
                need: encoded.len()
            },
            "short body reports the frame total"
        );
    }

    #[test]
    fn oversize_prefix_rejected_without_large_read() {
        let mut bytes = 1_073_741_824_u32.to_be_bytes().to_vec();
        bytes.extend_from_slice(&[0_u8; 10]);
        let error = decode_frame(&bytes).expect_err("decode oversize prefix");
        assert_eq!(
            error,
            CodecError::FrameTooLarge { len: 1_073_741_824 },
            "oversize prefix reports the claimed length"
        );
    }

    #[test]
    fn corrupt_body_is_decode_failed_without_payload_echo() {
        let mut body = vec![0xC1_u8];
        body.extend_from_slice(b"secret-body-marker-xyz");
        let mut bytes = (body.len() as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(&body);
        let error = decode_frame(&bytes).expect_err("decode corrupt body");
        let CodecError::DecodeFailed { reason } = error else {
            panic!("corrupt body must fail decode, got {error:?}");
        };
        assert!(
            !reason.contains("secret-body-marker-xyz"),
            "reason must not echo body bytes: {reason}"
        );
        let rendered = std::format!("{}", CodecError::DecodeFailed { reason });
        assert!(
            !rendered.contains("secret-body-marker-xyz"),
            "Display must not echo body bytes: {rendered}"
        );
    }

    #[test]
    fn concatenated_frames_decode_sequentially() {
        let first = sample_frame();
        let second = sample_frame();
        let first_encoded = encode_frame(&first).expect("encode first");
        let second_encoded = encode_frame(&second).expect("encode second");
        let mut both = first_encoded.clone();
        both.extend_from_slice(&second_encoded);
        let (decoded_first, consumed_first) = decode_frame(&both).expect("decode first");
        assert_eq!(consumed_first, first_encoded.len());
        assert_eq!(decoded_first, first);
        let (decoded_second, consumed_second) =
            decode_frame(&both[consumed_first..]).expect("decode second");
        assert_eq!(consumed_first + consumed_second, both.len());
        assert_eq!(decoded_second, second);
    }

    #[test]
    fn oversize_body_rejected_on_encode() {
        let mut frame = sample_frame();
        let WirePayload::SubmitTextInput(input) = &mut frame.payload else {
            panic!("sample payload must be text input");
        };
        input.body.text = "x".repeat(MAX_FRAME_BYTES);
        let error = encode_frame(&frame).expect_err("encode oversize body");
        let CodecError::FrameTooLarge { len } = error else {
            panic!("oversize body must report length, got {error:?}");
        };
        assert!(
            len > MAX_FRAME_BYTES,
            "reported length exceeds the cap: {len}"
        );
    }
}
