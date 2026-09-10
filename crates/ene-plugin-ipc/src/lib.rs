//! Length-prefixed `MessagePack` transport frames (IPC §10.1).
//!
//! This crate is the retained transport-frames crate shared by the Host
//! listener and the Client dialer. It is a pure byte codec: it frames one
//! domain message ([`WireFrame`]) as a 4-byte big-endian exclusive length
//! prefix followed by the canonical `MessagePack` body (IPC §7), and parses
//! such bytes back. It performs no I/O, owns no sockets, and runs no async
//! tasks; socket read/write loops live in the applications that embed it.
//!
//! One frame carries exactly one domain message. Text streaming chunking
//! happens at the DTO level ([`ene_api::v1::round::TextStreamFrameWire`]),
//! never here: this layer never splits, merges, or otherwise interprets
//! payloads. It never inspects envelope or payload semantics either; domain
//! meaning (routing, validation, authority) stays in `ene-api` and the
//! Host. Unknown-field tolerance therefore comes free from the `ene-api`
//! DTOs, not from any logic here.

use ene_api::v1::envelope::WireEnvelope;
use ene_api::v1::payload::WirePayload;
use serde::{Deserialize, Serialize};

/// Length of the big-endian frame-length prefix in bytes.
const LEN_PREFIX_LEN: usize = 4;

/// Maximum `MessagePack` body length in bytes, exclusive of the prefix.
///
/// Bodies longer than this are rejected on encode and on decode. The bound
/// keeps a single hostile or corrupt length prefix from driving unbounded
/// allocation while comfortably fitting text round-trip traffic.
pub const MAX_FRAME_BYTES: usize = 256 * 1024;

/// One domain message on the wire: routing envelope plus typed payload.
///
/// Serialization order is the field order (`envelope`, then `payload`) under
/// the crate-wide `MessagePack` configuration used by [`encode_frame`] and
/// [`decode_frame`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireFrame {
    /// Routing envelope. Routes only; never authorizes.
    pub envelope: WireEnvelope,
    /// Typed body named by the envelope's `message_type`.
    pub payload: WirePayload,
}

/// Transport framing failure.
///
/// Display strings carry lengths and decoder reasons only. They never echo
/// frame bytes: a corrupt body may contain conversation text, so neither
/// the reason nor any other field may include raw payload material.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    /// A body length exceeded [`MAX_FRAME_BYTES`]: on encode, the encoded
    /// body; on decode, the length the prefix claims.
    #[error("frame body of {len} bytes exceeds the 256 KiB cap")]
    FrameTooLarge {
        /// Offending body length in bytes.
        len: usize,
    },
    /// Fewer bytes than a complete frame arrived. `need` is the total byte
    /// count required: 4 bytes when the prefix itself is short,
    /// prefix plus claimed body length otherwise.
    #[error("truncated frame: have {have} bytes, need {need}")]
    Truncated {
        /// Bytes available in the input slice.
        have: usize,
        /// Total bytes required for the frame.
        need: usize,
    },
    /// The length prefix was well-formed but the body bytes did not decode
    /// as a [`WireFrame`]. The reason is the decoder's short diagnostic,
    /// which never echoes input bytes.
    #[error("frame body failed to decode: {reason}")]
    DecodeFailed {
        /// Short decoder diagnostic, free of raw payload bytes.
        reason: String,
    },
}

/// Encodes one frame as a 4-byte big-endian body length plus body.
///
/// The length is exclusive: it counts the `MessagePack` body only, not the
/// prefix (IPC §10.1). Bodies longer than [`MAX_FRAME_BYTES`] are rejected
/// with [`CodecError::FrameTooLarge`].
///
/// The cap is enforced encode-then-check: the body is serialized first and
/// its length compared before the output buffer is built. This allocates up
/// to the true body size even for oversize inputs. That is acceptable at the
/// 256 KiB scale on a same-machine socket between mutually authenticated
/// peers, but a future hardening step (a length-bounded streaming encoder)
/// should precede any use over larger-payload or less-trusted transports.
pub fn encode_frame(frame: &WireFrame) -> Result<Vec<u8>, CodecError> {
    let body = rmp_serde::to_vec(frame).map_err(|error| CodecError::DecodeFailed {
        // `rmp-serde` writing into a `Vec` cannot fail in practice; there is
        // no encode-dedicated variant because the failure is uninhabited for
        // these types, so the single codec error carries it with the stage
        // named in the reason.
        reason: std::format!("encode: {error}"),
    })?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(CodecError::FrameTooLarge { len: body.len() });
    }
    // The cap check above keeps this conversion exact: `MAX_FRAME_BYTES`
    // (256 KiB) fits in `u32`.
    let len_prefix = body.len() as u32;
    let mut out = Vec::with_capacity(LEN_PREFIX_LEN + body.len());
    out.extend_from_slice(&len_prefix.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decodes the first frame in `bytes`, returning it with its consumed length.
///
/// Consumed length is prefix plus claimed body length, so trailing bytes are
/// the next frame: callers advance past the consumed count and call again.
/// Inputs shorter than the prefix fail with [`CodecError::Truncated`]
/// needing 4 bytes; a prefix claiming more than [`MAX_FRAME_BYTES`] fails
/// with [`CodecError::FrameTooLarge`] before any body-sized work (no large
/// allocation, no large read); a short body fails with
/// [`CodecError::Truncated`] needing the frame total; an undecodable body
/// fails with [`CodecError::DecodeFailed`] whose reason carries no raw
/// payload bytes.
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
            // `rmp-serde` diagnostics describe the structural failure and
            // never echo input bytes, so this passthrough cannot leak body
            // text into logs.
            reason: std::format!("{error}"),
        }
    })?;
    Ok((frame, need))
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

    /// Minimal envelope plus a text DTO payload for codec tests.
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
        let body = rmp_serde::to_vec(&frame).expect("encode body");
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
        // Claims 1 GiB but only 10 body bytes arrive: the cap must fire
        // before any body-sized allocation or read.
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
        // 0xC1 is never a valid `MessagePack` marker; the trailing ASCII is
        // planted body-like text that must not leak into the diagnostics.
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
