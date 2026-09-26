use crate::v1::envelope::WireEnvelope;
use crate::v1::payload::WirePayload;
use crate::v1::reject::{RejectKind, RejectNotice};
use serde::{Deserialize, Serialize};

const MESSAGE_TYPE_TOKEN_CHARS: usize = 64;

pub const MAX_FRAME_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    // The externally tagged payload map admits a trailing key only through the
    // probe: serde's newtype variant path never inspects it, so the rule is
    // enforced here rather than left to a container attribute that cannot see it.
    if probe.payload.trailing_keys {
        return Ok(DecodedFrame::Unsupported {
            envelope: probe.envelope,
            reason: UnsupportedReason::UnknownFieldValue {
                message_type: bound_token(&probe.payload.key),
            },
        });
    }
    match rmp_serde::from_slice::<WireFrame>(body) {
        Ok(frame) => Ok(DecodedFrame::Known(frame)),
        Err(error) => {
            let message_type = bound_token(&probe.payload.key);
            let reason = match &error {
                rmp_serde::decode::Error::Syntax(message)
                    if message.starts_with("unknown variant")
                        || message.starts_with("unknown field") =>
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
    trailing_keys: bool,
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
                    trailing_keys: false,
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
                let mut trailing_keys = false;
                while map.next_key::<de::IgnoredAny>()?.is_some() {
                    map.next_value::<de::IgnoredAny>()?;
                    trailing_keys = true;
                }
                Ok(PayloadProbe { key, trailing_keys })
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
    use std::collections::BTreeMap;

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
        target: crate::v1::round::RoundTarget,
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

    #[test]
    fn malformed_frames_fail_structurally_without_echo() {
        let secret = "secret-body-marker-xyz";
        let mut body = vec![0xC1_u8];
        body.extend_from_slice(secret.as_bytes());
        let error = decode_frame(&body).expect_err("decode corrupt body");
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains(secret));
        let CodecError::DecodeFailed { reason } = error else {
            panic!("corrupt body must fail decode");
        };
        assert!(!reason.contains(secret));

        #[derive(Serialize)]
        struct ScalarPayloadFrame {
            envelope: WireEnvelope,
            payload: u32,
        }
        let scalar = ScalarPayloadFrame {
            envelope: envelope_of("SubmitTextInput"),
            payload: 7,
        };
        let body = rmp_serde::to_vec_named(&scalar).expect("encode scalar payload");
        assert!(matches!(
            decode_frame(&body),
            Err(CodecError::DecodeFailed { .. })
        ));
    }

    #[test]
    fn an_unknown_field_in_a_known_payload_is_rejected_rather_than_ignored() {
        #[derive(Serialize)]
        struct SubmitWithExtra {
            companion: CompanionWireRef,
            target: crate::v1::round::RoundTarget,
            local_id: crate::v1::refs::ClientLocalId,
            body: TextBodyWire,
            future_optional: u32,
        }

        #[derive(Serialize)]
        enum Payload {
            SubmitTextInput(SubmitWithExtra),
        }

        #[derive(Serialize)]
        struct Frame {
            envelope: WireEnvelope,
            payload: Payload,
        }

        let well_formed = SubmitWithExtra {
            companion: CompanionWireRef(String::from("companion-1")),
            target: crate::v1::round::RoundTarget::New,
            local_id: crate::v1::refs::ClientLocalId(String::from("local-1")),
            body: TextBodyWire {
                text: String::from("hello"),
                lang: TextLangWire(String::from("en")),
            },
            future_optional: 2,
        };
        let body = rmp_serde::to_vec_named(&Frame {
            envelope: envelope_of("SubmitTextInput"),
            payload: Payload::SubmitTextInput(well_formed),
        })
        .expect("the crafted frame encodes");
        let DecodedFrame::Unsupported { reason, .. } =
            decode_frame(&body).expect("the crafted frame decodes to a typed rejection")
        else {
            panic!(
                "an unknown field must reject the frame instead of decoding it as the old shape"
            );
        };
        assert_eq!(
            reason,
            super::UnsupportedReason::UnknownFieldValue {
                message_type: String::from("SubmitTextInput"),
            }
        );
        assert_eq!(reason.reject_kind(), RejectKind::UnsupportedFieldValue);
    }

    #[test]
    fn a_trailing_key_in_the_payload_map_is_rejected_not_ignored() {
        #[derive(Serialize)]
        struct TrailingPayload {
            envelope: WireEnvelope,
            payload: BTreeMap<String, PayloadBody>,
        }

        #[derive(Serialize)]
        enum PayloadBody {
            SubmitTextInput(TrailingSubmit),
        }

        #[derive(Serialize)]
        struct TrailingSubmit {
            companion: CompanionWireRef,
            target: crate::v1::round::RoundTarget,
            local_id: crate::v1::refs::ClientLocalId,
            body: TextBodyWire,
        }

        let mut payload = BTreeMap::new();
        payload.insert(
            String::from("SubmitTextInput"),
            PayloadBody::SubmitTextInput(TrailingSubmit {
                companion: CompanionWireRef(String::from("companion-1")),
                target: crate::v1::round::RoundTarget::New,
                local_id: crate::v1::refs::ClientLocalId(String::from("local-1")),
                body: TextBodyWire {
                    text: String::from("hello"),
                    lang: TextLangWire(String::from("en")),
                },
            }),
        );
        payload.insert(
            String::from("future_payload"),
            PayloadBody::SubmitTextInput(TrailingSubmit {
                companion: CompanionWireRef(String::from("companion-1")),
                target: crate::v1::round::RoundTarget::New,
                local_id: crate::v1::refs::ClientLocalId(String::from("local-2")),
                body: TextBodyWire {
                    text: String::from("second key"),
                    lang: TextLangWire(String::from("en")),
                },
            }),
        );
        let body = rmp_serde::to_vec_named(&TrailingPayload {
            envelope: envelope_of("SubmitTextInput"),
            payload,
        })
        .expect("the crafted frame encodes");
        let DecodedFrame::Unsupported { reason, .. } =
            decode_frame(&body).expect("the crafted frame decodes to a typed rejection")
        else {
            panic!("a trailing payload key must reject the frame instead of being ignored");
        };
        assert_eq!(
            reason,
            super::UnsupportedReason::UnknownFieldValue {
                message_type: String::from("SubmitTextInput"),
            }
        );
    }

    #[test]
    fn frame_size_cap_is_enforced_before_decode_or_after_encode() {
        let mut frame = sample_frame();
        let WirePayload::SubmitTextInput(input) = &mut frame.payload else {
            panic!("sample payload must be text input");
        };
        input.body.text = "x".repeat(MAX_FRAME_BYTES);
        let CodecError::FrameTooLarge { len } =
            encode_frame(&frame).expect_err("encode oversize body")
        else {
            panic!("oversize body must report length");
        };
        assert!(len > MAX_FRAME_BYTES);

        let body = vec![0_u8; MAX_FRAME_BYTES + 1];
        let error = decode_frame(&body).expect_err("decode oversize body");
        assert!(matches!(error, CodecError::FrameTooLarge { .. }));
    }

    #[test]
    fn message_type_discriminator_is_authoritative_and_preserves_typed_rejects() {
        let cases = [
            (
                CraftedFrame {
                    envelope: envelope_of("FutureThing"),
                    payload: CraftedPayload::FutureThing(FutureBody {
                        note: String::from("from a newer peer"),
                    }),
                },
                super::UnsupportedReason::UnknownMessageType {
                    message_type: String::from("FutureThing"),
                },
                RejectKind::UnsupportedMessage,
            ),
            (
                CraftedFrame {
                    envelope: envelope_of("SubmitTextInput"),
                    payload: CraftedPayload::FutureThing(FutureBody {
                        note: String::from("payload disagrees"),
                    }),
                },
                super::UnsupportedReason::UnknownMessageType {
                    message_type: String::from("FutureThing"),
                },
                RejectKind::UnsupportedMessage,
            ),
            (
                CraftedFrame {
                    envelope: envelope_of("FuturePing"),
                    payload: CraftedPayload::SubmitTextInput(PartialSubmit {
                        companion: CompanionWireRef(String::from("companion-1")),
                        target: crate::v1::round::RoundTarget::New,
                        body: TextBodyWire {
                            text: String::from("body never inspected"),
                            lang: TextLangWire(String::from("en")),
                        },
                    }),
                },
                super::UnsupportedReason::UnknownMessageType {
                    message_type: String::from("FuturePing"),
                },
                RejectKind::UnsupportedMessage,
            ),
            (
                CraftedFrame {
                    envelope: envelope_of("TextStreamClose"),
                    payload: CraftedPayload::TextStreamClose(CraftedClose {
                        stream: StreamWireId(uuid::Uuid::new_v4()),
                        status: String::from("Suspended"),
                    }),
                },
                super::UnsupportedReason::UnknownFieldValue {
                    message_type: String::from("TextStreamClose"),
                },
                RejectKind::UnsupportedFieldValue,
            ),
            (
                CraftedFrame {
                    envelope: envelope_of("SubmitTextInput"),
                    payload: CraftedPayload::SubmitTextInput(PartialSubmit {
                        companion: CompanionWireRef(String::from("companion-1")),
                        target: crate::v1::round::RoundTarget::New,
                        body: TextBodyWire {
                            text: String::from("local_id deliberately absent"),
                            lang: TextLangWire(String::from("en")),
                        },
                    }),
                },
                super::UnsupportedReason::MissingRequiredField {
                    message_type: String::from("SubmitTextInput"),
                },
                RejectKind::MissingRequiredField,
            ),
        ];

        for (crafted, expected_reason, expected_kind) in cases {
            let DecodedFrame::Unsupported { envelope, reason } = decode_crafted(&crafted) else {
                panic!("the crafted frame must be a typed unsupported result");
            };
            assert_eq!(envelope, crafted.envelope);
            assert_eq!(reason.reject_kind(), expected_kind);
            assert_eq!(reason, expected_reason);
        }
    }

    #[test]
    fn reject_details_bound_and_escape_the_message_type_token() {
        for (long, bounded_notice) in [
            (format!("evil\n{}", "a".repeat(400)), true),
            (format!("evil\r\t\u{1b}\0{}", "a".repeat(400)), false),
        ] {
            let crafted = CraftedFrame {
                envelope: envelope_of(&long),
                payload: CraftedPayload::SubmitTextInput(PartialSubmit {
                    companion: CompanionWireRef(String::from("companion-1")),
                    target: crate::v1::round::RoundTarget::New,
                    body: TextBodyWire {
                        text: String::from("payload type is known"),
                        lang: TextLangWire(String::from("en")),
                    },
                }),
            };
            let DecodedFrame::Unsupported { envelope, reason } = decode_crafted(&crafted) else {
                panic!("the long token must still reject as unsupported");
            };
            assert_eq!(envelope.message_type.0, long);
            let notice = reason.notice();
            if bounded_notice {
                assert!(notice.detail.len() <= 96);
            }
            assert_eq!(notice.kind, RejectKind::UnsupportedMessage);
            assert!(!notice.detail.chars().any(char::is_control));
            assert!(notice.detail.contains('…'));
        }
    }
}
