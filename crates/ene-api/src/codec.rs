use crate::v1::envelope::WireEnvelope;
use crate::v1::payload::WirePayload;
use crate::v1::reject::{RejectKind, RejectNotice};
use serde::{Deserialize, Serialize};

const MESSAGE_TYPE_TOKEN_CHARS: usize = 64;

pub const MAX_FRAME_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireFrame {
    pub envelope: WireEnvelope,
    pub payload: WirePayload,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecodedFrame {
    Known(WireFrame),
    Unsupported {
        envelope: WireEnvelope,
        reason: UnsupportedReason,
    },
}

impl DecodedFrame {
    #[must_use]
    pub fn envelope(&self) -> &WireEnvelope {
        match self {
            Self::Known(frame) => &frame.envelope,
            Self::Unsupported { envelope, .. } => envelope,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnsupportedReason {
    UnknownMessageType { message_type: String },
    UnknownFieldValue { message_type: String },
    MissingRequiredField { message_type: String },
}

impl UnsupportedReason {
    #[must_use]
    pub fn reject_kind(&self) -> RejectKind {
        match self {
            Self::UnknownMessageType { .. } => RejectKind::UnsupportedMessage,
            Self::UnknownFieldValue { .. } => RejectKind::UnsupportedFieldValue,
            Self::MissingRequiredField { .. } => RejectKind::MissingRequiredField,
        }
    }

    #[must_use]
    pub fn notice(&self) -> RejectNotice {
        let message_type = match self {
            Self::UnknownMessageType { message_type }
            | Self::UnknownFieldValue { message_type }
            | Self::MissingRequiredField { message_type } => message_type,
        };
        let detail = match self {
            Self::UnknownMessageType { .. } => format!("unknown message type {message_type:?}"),
            Self::UnknownFieldValue { .. } => {
                format!("unsupported field value in {message_type:?}")
            }
            Self::MissingRequiredField { .. } => {
                format!("missing required field in {message_type:?}")
            }
        };
        RejectNotice {
            kind: self.reject_kind(),
            detail,
        }
    }
}

fn bound_token(raw: &str) -> String {
    let mut chars = raw.chars();
    let bounded: String = chars.by_ref().take(MESSAGE_TYPE_TOKEN_CHARS).collect();
    if chars.next().is_some() {
        format!("{bounded}…")
    } else {
        bounded
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    #[error("frame body of {len} bytes exceeds the 256 KiB cap")]
    FrameTooLarge { len: usize },
    #[error("frame body failed to decode: {reason}")]
    DecodeFailed { reason: String },
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
    Ok(body)
}

pub fn decode_frame(body: &[u8]) -> Result<DecodedFrame, CodecError> {
    if body.len() > MAX_FRAME_BYTES {
        return Err(CodecError::FrameTooLarge { len: body.len() });
    }
    let probe: FrameProbe =
        rmp_serde::from_slice(body).map_err(|error| CodecError::DecodeFailed {
            reason: decode_reason(&error),
        })?;
    let known_envelope =
        WirePayload::KNOWN_MESSAGE_TYPES.contains(&probe.envelope.message_type.0.as_str());
    let known_payload = WirePayload::KNOWN_MESSAGE_TYPES.contains(&probe.payload.key.as_str());
    if !known_envelope || !known_payload {
        let message_type = if !known_payload {
            probe.payload.key.clone()
        } else {
            probe.envelope.message_type.0.clone()
        };
        return Ok(DecodedFrame::Unsupported {
            envelope: probe.envelope,
            reason: UnsupportedReason::UnknownMessageType {
                message_type: bound_token(&message_type),
            },
        });
    }
    match rmp_serde::from_slice::<WireFrame>(body) {
        Ok(frame) => Ok(DecodedFrame::Known(frame)),
        Err(error) => {
            let message_type = bound_token(&probe.payload.key);
            let reason = match &error {
                rmp_serde::decode::Error::Syntax(message)
                    if message.starts_with("unknown variant") =>
                {
                    Some(UnsupportedReason::UnknownFieldValue { message_type })
                }
                rmp_serde::decode::Error::Syntax(message)
                    if message.starts_with("missing field") =>
                {
                    Some(UnsupportedReason::MissingRequiredField { message_type })
                }
                _ => None,
            };
            match reason {
                Some(reason) => Ok(DecodedFrame::Unsupported {
                    envelope: probe.envelope,
                    reason,
                }),
                None => Err(CodecError::DecodeFailed {
                    reason: decode_reason(&error),
                }),
            }
        }
    }
}

#[derive(Deserialize)]
struct FrameProbe {
    envelope: WireEnvelope,
    payload: PayloadProbe,
}

struct PayloadProbe {
    key: String,
}

impl<'de> Deserialize<'de> for PayloadProbe {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct KeyVisitor;

        impl<'de> serde::de::Visitor<'de> for KeyVisitor {
            type Value = PayloadProbe;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a payload naming its message type")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(PayloadProbe {
                    key: value.to_owned(),
                })
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                use serde::de;

                let key: String = map
                    .next_key()?
                    .ok_or_else(|| de::Error::custom("payload map carries no message type key"))?;
                map.next_value::<de::IgnoredAny>()?;
                while map.next_key::<de::IgnoredAny>()?.is_some() {
                    map.next_value::<de::IgnoredAny>()?;
                }
                Ok(PayloadProbe { key })
            }
        }

        deserializer.deserialize_any(KeyVisitor)
    }
}

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
    use super::{CodecError, DecodedFrame, MAX_FRAME_BYTES, WireFrame, decode_frame, encode_frame};
    use crate::v1::envelope::{ProtocolVersion, WireEnvelope, WireSender, new_outgoing_envelope};
    use crate::v1::payload::WirePayload;
    use crate::v1::refs::{
        ClientIncarnationId, ClientLocalId, CompanionWireRef, StreamWireId, TextLangWire,
        WireMessageType,
    };
    use crate::v1::reject::RejectKind;
    use crate::v1::round::{RoundTarget, SubmitTextInput, TextBodyWire};
    use serde::Serialize;

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
            WireMessageType(String::from("SubmitTextInput")),
        );
        let payload = WirePayload::SubmitTextInput(SubmitTextInput {
            companion: CompanionWireRef(String::from("companion-1")),
            target: RoundTarget::New,
            local_id: ClientLocalId(String::from("local-1")),
            body: TextBodyWire {
                text: String::from("hello"),
                lang: TextLangWire(String::from("en")),
            },
        });
        WireFrame { envelope, payload }
    }

    fn known(frame: DecodedFrame) -> WireFrame {
        match frame {
            DecodedFrame::Known(frame) => frame,
            other => panic!("expected a known frame, got {other:?}"),
        }
    }

    fn envelope_of(message_type: &str) -> WireEnvelope {
        new_outgoing_envelope(
            ProtocolVersion::V1,
            WireSender {
                device_id: None,
                incarnation_id: ClientIncarnationId {
                    counter: 7,
                    random: 42,
                },
                connection_id: None,
            },
            WireMessageType(String::from(message_type)),
        )
    }

    #[derive(Serialize)]
    struct CraftedFrame {
        envelope: WireEnvelope,
        payload: CraftedPayload,
    }

    #[derive(Serialize)]
    enum CraftedPayload {
        FutureThing(FutureBody),
        SubmitTextInput(PartialSubmit),
        TextStreamClose(CraftedClose),
    }

    #[derive(Serialize)]
    struct FutureBody {
        note: String,
    }

    #[derive(Serialize)]
    struct PartialSubmit {
        companion: CompanionWireRef,
        round: Option<crate::v1::refs::RoundWireId>,
        body: TextBodyWire,
    }

    #[derive(Serialize)]
    struct CraftedClose {
        stream: StreamWireId,
        status: String,
    }

    fn decode_crafted(frame: &CraftedFrame) -> DecodedFrame {
        let body = rmp_serde::to_vec_named(frame).expect("crafted frame encodes");
        decode_frame(&body).expect("crafted frame decodes")
    }

    fn unsupported_reason(frame: &CraftedFrame) -> super::UnsupportedReason {
        match decode_crafted(frame) {
            DecodedFrame::Unsupported { reason, .. } => reason,
            other => panic!("expected an unsupported frame, got {other:?}"),
        }
    }

    #[test]
    fn roundtrip_preserves_envelope_and_payload() {
        let frame = sample_frame();
        let body = encode_frame(&frame).expect("encode frame");
        let decoded = decode_frame(&body).expect("decode frame");
        assert_eq!(known(decoded), frame, "codec must preserve the frame");
    }

    #[test]
    fn corrupt_body_is_decode_failed_without_payload_echo() {
        let mut body = vec![0xC1_u8];
        body.extend_from_slice(b"secret-body-marker-xyz");
        let error = decode_frame(&body).expect_err("decode corrupt body");
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

    #[test]
    fn oversize_body_rejected_on_decode_before_the_structure_is_read() {
        let body = vec![0_u8; MAX_FRAME_BYTES + 1];
        let error = decode_frame(&body).expect_err("decode oversize body");
        assert!(
            matches!(error, CodecError::FrameTooLarge { .. }),
            "an oversize body is refused by its size: {error:?}"
        );
    }

    #[test]
    fn an_unknown_payload_message_type_is_unsupported_and_keeps_the_envelope() {
        let crafted = CraftedFrame {
            envelope: envelope_of("FutureThing"),
            payload: CraftedPayload::FutureThing(FutureBody {
                note: String::from("from a newer peer"),
            }),
        };
        let DecodedFrame::Unsupported { envelope, reason } = decode_crafted(&crafted) else {
            panic!("an unknown message type must not fail the decode");
        };
        assert_eq!(envelope, crafted.envelope, "the envelope stays available");
        assert_eq!(
            reason,
            super::UnsupportedReason::UnknownMessageType {
                message_type: String::from("FutureThing")
            }
        );
        assert_eq!(reason.reject_kind(), RejectKind::UnsupportedMessage);
    }

    #[test]
    fn a_known_envelope_type_still_rejects_an_unknown_payload_type() {
        let crafted = CraftedFrame {
            envelope: envelope_of("SubmitTextInput"),
            payload: CraftedPayload::FutureThing(FutureBody {
                note: String::from("payload disagrees"),
            }),
        };
        let reason = unsupported_reason(&crafted);
        assert_eq!(
            reason,
            super::UnsupportedReason::UnknownMessageType {
                message_type: String::from("FutureThing")
            },
            "the payload discriminator decides, not the envelope hint"
        );
    }

    #[test]
    fn an_unknown_envelope_type_never_decodes_the_payload_body() {
        let crafted = CraftedFrame {
            envelope: envelope_of("FuturePing"),
            payload: CraftedPayload::SubmitTextInput(PartialSubmit {
                companion: CompanionWireRef(String::from("companion-1")),
                round: None,
                body: TextBodyWire {
                    text: String::from("body never inspected"),
                    lang: TextLangWire(String::from("en")),
                },
            }),
        };
        let reason = unsupported_reason(&crafted);
        assert_eq!(
            reason,
            super::UnsupportedReason::UnknownMessageType {
                message_type: String::from("FuturePing")
            },
            "an unknown message type is rejected without inferring its body"
        );
    }

    #[test]
    fn an_unknown_enum_value_is_unsupported_field_value() {
        let crafted = CraftedFrame {
            envelope: envelope_of("TextStreamClose"),
            payload: CraftedPayload::TextStreamClose(CraftedClose {
                stream: StreamWireId(uuid::Uuid::new_v4()),
                status: String::from("Suspended"),
            }),
        };
        let reason = unsupported_reason(&crafted);
        assert_eq!(
            reason,
            super::UnsupportedReason::UnknownFieldValue {
                message_type: String::from("TextStreamClose")
            }
        );
        assert_eq!(reason.reject_kind(), RejectKind::UnsupportedFieldValue);
    }

    #[test]
    fn a_missing_required_field_is_missing_required_field() {
        let crafted = CraftedFrame {
            envelope: envelope_of("SubmitTextInput"),
            payload: CraftedPayload::SubmitTextInput(PartialSubmit {
                companion: CompanionWireRef(String::from("companion-1")),
                round: None,
                body: TextBodyWire {
                    text: String::from("local_id deliberately absent"),
                    lang: TextLangWire(String::from("en")),
                },
            }),
        };
        let reason = unsupported_reason(&crafted);
        assert_eq!(
            reason,
            super::UnsupportedReason::MissingRequiredField {
                message_type: String::from("SubmitTextInput")
            }
        );
        assert_eq!(reason.reject_kind(), RejectKind::MissingRequiredField);
    }

    #[test]
    fn a_non_map_payload_is_a_structural_decode_failure() {
        #[derive(Serialize)]
        struct ScalarPayloadFrame {
            envelope: WireEnvelope,
            payload: u32,
        }
        let crafted = ScalarPayloadFrame {
            envelope: envelope_of("SubmitTextInput"),
            payload: 7,
        };
        let body = rmp_serde::to_vec_named(&crafted).expect("encode scalar payload");
        let error = decode_frame(&body).expect_err("a scalar payload is not a message");
        assert!(
            matches!(error, CodecError::DecodeFailed { .. }),
            "a malformed payload fails the frame, got {error:?}"
        );
    }

    #[test]
    fn reject_details_bound_and_escape_the_message_type_token() {
        let long = format!("evil\n{}", "a".repeat(400));
        let crafted = CraftedFrame {
            envelope: envelope_of(&long),
            payload: CraftedPayload::SubmitTextInput(PartialSubmit {
                companion: CompanionWireRef(String::from("companion-1")),
                round: None,
                body: TextBodyWire {
                    text: String::from("payload type is known"),
                    lang: TextLangWire(String::from("en")),
                },
            }),
        };
        let DecodedFrame::Unsupported { envelope, reason } = decode_crafted(&crafted) else {
            panic!("the long token must still reject as unsupported");
        };
        assert_eq!(
            envelope.message_type.0, long,
            "the envelope keeps the raw token for the owner of this connection"
        );
        let notice = reason.notice();
        assert!(
            notice.detail.len() <= 96,
            "the echoed token must stay bounded: {}",
            notice.detail
        );
        assert!(
            !notice.detail.contains('\n'),
            "the detail must not carry raw control characters: {:?}",
            notice.detail
        );
        assert!(
            notice.detail.contains('…'),
            "truncation must be visible: {:?}",
            notice.detail
        );
    }
}
