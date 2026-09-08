//! Wire contract checks: shapes survive JSON, unknown fields are tolerated,
//! redactions hold, and identifiers stay separated by type.

use ene_api::v1::envelope::WireSender;
use ene_api::v1::envelope::{ProtocolVersion, WireEnvelope, new_outgoing_envelope};
use ene_api::v1::handshake::{CapabilityAdvertise, PairingRequest};
use ene_api::v1::management::{ManagementIntent, SetupIntentKind};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::presence::{PresenceAttributionWire, PresenceStateWire};
use ene_api::v1::refs::{
    ClientIncarnationId, ClientWireRef, CompanionWireRef, RoundWireId, WireMessageType,
};
use ene_api::v1::refs::{ClientLocalId, StreamWireId, TextLangWire};
use ene_api::v1::round::{HistoryRole, HistoryView, RoundIntakeOutcomeWire, SubmitTextInput};
use ene_api::v1::round::{TextBodyWire, TextStreamFrameWire};
use uuid::Uuid;

fn sender() -> WireSender {
    WireSender {
        device_id: None,
        incarnation_id: ClientIncarnationId {
            counter: 0,
            random: 7,
        },
        connection_id: None,
    }
}

fn envelope() -> WireEnvelope {
    new_outgoing_envelope(
        ProtocolVersion::V1,
        sender(),
        WireMessageType(String::from("SubmitTextInput")),
    )
}

/// Encodes to JSON and decodes back, asserting the value survives.
fn roundtrip<T>(value: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + core::fmt::Debug,
{
    let text = serde_json::to_string(value);
    assert!(text.is_ok(), "must serialize to JSON");
    if let Ok(text) = text {
        let back: Result<T, _> = serde_json::from_str(&text);
        assert!(back.is_ok(), "must deserialize from its own JSON");
        if let Ok(back) = back {
            assert!(back == *value, "a JSON roundtrip must preserve the value");
        }
    }
}

#[test]
fn envelope_roundtrip() {
    roundtrip(&envelope());
}

#[test]
fn handshake_roundtrip() {
    roundtrip(&PairingRequest {
        device_descriptor: String::from("Owner laptop"),
    });
    roundtrip(&CapabilityAdvertise {
        supported_protocol: [ProtocolVersion::V1].to_vec(),
        features: [].to_vec(),
        platform: String::from("linux"),
    });
}

#[test]
fn round_trip_roundtrip() {
    roundtrip(&SubmitTextInput {
        companion: CompanionWireRef(String::from("companion-1")),
        round: None,
        local_id: ClientLocalId(String::from("local-1")),
        body: TextBodyWire {
            text: String::from("hello"),
            lang: TextLangWire(String::from("en")),
        },
    });
    roundtrip(&RoundIntakeOutcomeWire::StaleRound {
        current_round: None,
        current_generation: 4,
    });
    roundtrip(&TextStreamFrameWire {
        stream: StreamWireId(Uuid::new_v4()),
        seq: 0,
        delta: String::from("hi"),
        is_final: true,
    });
}

#[test]
fn presence_and_management_roundtrip() {
    roundtrip(&PresenceAttributionWire {
        companion: CompanionWireRef(String::from("companion-1")),
        state: PresenceStateWire::NoActive,
        active_client: Some(ClientWireRef(String::from("client-1"))),
        generation: 2,
        move_reason: None,
    });
    roundtrip(&ManagementIntent {
        intent_id: Uuid::new_v4(),
        kind: SetupIntentKind::CompleteSetup,
        target: String::from("setup"),
        base_view: None,
        rationale: None,
    });
}

#[test]
fn payload_enum_roundtrip() {
    roundtrip(&WirePayload::HistoryView(HistoryView {
        items: [{
            use ene_api::v1::round::HistoryItem;
            HistoryItem {
                round: RoundWireId(String::from("round-1")),
                role: HistoryRole::Owner,
                text: String::from("hello"),
                at: String::from("2026-09-08T12:00:00+09:00"),
            }
        }]
        .to_vec(),
    }));
}

#[test]
fn unknown_fields_are_ignored_not_rejected() {
    let text = r#"{"protocol":{"major":1,"minor":0},"message_id":"12345678-1234-5678-1234-567812345678","correlation":{"request_id":null,"command_id":null,"stream_id":null,"reply_to":null,"causation_span":null},"sender":{"device_id":null,"incarnation_id":{"counter":0,"random":0},"connection_id":null},"observed":{"presence_generation_view":null,"round_view":null,"ticket_view":null},"message_type":"Ping","from_future_version_field":"ignored"}"#;
    let parsed: Result<WireEnvelope, _> = serde_json::from_str(text);
    assert!(parsed.is_ok(), "unknown fields must be ignored: {parsed:?}");
}

#[test]
fn major_match_admits_negotiation_only() {
    assert!(ProtocolVersion::V1.shares_major_with(&ProtocolVersion { major: 1, minor: 5 }));
    assert!(!ProtocolVersion::V1.shares_major_with(&ProtocolVersion { major: 2, minor: 0 }));
}
