//! JSON shape checks for the Stage 1 wire-neutral DTOs.
//!
//! Each module contributes at least one serialization roundtrip, unknown
//! fields are shown to be tolerated, and [`ProtocolVersion`] compatibility is
//! pinned to equal-major matching. Failures return early instead of unwrapping
//! so the workspace deny on `unwrap`/`expect` stays green.

use ene_api::envelope::{
    ObservedMarks, ProtocolVersion, WireCorrelation, WireEnvelope, WireSender,
};
use ene_api::handshake::{
    AuthChallenge, AuthProof, AuthResult, CapabilityAdvertise, ClientFeature, ClientFeatureKind,
    DisconnectNotice, NegotiatedConnection, PairingRequest, PairingResult, ReconnectHello,
    RecoveryInvite,
};
use ene_api::management::{
    ManagementIntent, ManagementOutcome, ManagementView, ManagementViewRequest, SetupIntentKind,
    ViewSection,
};
use ene_api::presence::{PresenceAttribution, PresenceState};
use ene_api::round::{
    ConfirmPresentation, HistoryItem, HistoryRequest, HistoryRole, HistoryView, PresentationStatus,
    RoundIntakeOutcome, StreamClose, SubmitTextInput, TextStreamClose, TextStreamFrame,
    TextStreamOpen,
};
use uuid::Uuid;

/// Serializes `value` to JSON and back, returning [`None`] on either failure.
fn roundtrip<T>(value: &T) -> Option<T>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let json = serde_json::to_string(value).ok()?;
    serde_json::from_str(&json).ok()
}

/// Builds a representative envelope for roundtrip and tolerance checks.
fn sample_envelope() -> WireEnvelope {
    WireEnvelope {
        protocol: ProtocolVersion::V1,
        message_id: Uuid::new_v4(),
        correlation: WireCorrelation {
            request_id: Some(Uuid::new_v4()),
            command_id: None,
            stream_id: None,
            reply_to: None,
        },
        sender: WireSender {
            device_id: Uuid::new_v4(),
            incarnation: 7,
            connection_id: None,
        },
        observed: ObservedMarks {
            presence_generation_view: Some(3),
            round_view: Some("round-9".to_string()),
        },
        message_type: "SubmitTextInput".to_string(),
    }
}

#[test]
fn envelope_roundtrips_through_json() {
    let original = sample_envelope();
    let back = roundtrip(&original);
    assert!(back.is_some(), "WireEnvelope must survive a JSON roundtrip");
    let Some(back) = back else {
        return;
    };
    assert!(
        original == back,
        "a JSON roundtrip must preserve WireEnvelope"
    );
}

#[test]
fn handshake_roundtrips_through_json() {
    let original = CapabilityAdvertise {
        supported_protocol: vec![ProtocolVersion::V1, ProtocolVersion { major: 1, minor: 1 }],
        features: vec![ClientFeature {
            kind: ClientFeatureKind::Text,
            available: true,
        }],
        platform: "test-client".to_string(),
    };
    let back = roundtrip(&original);
    assert!(
        back.is_some(),
        "CapabilityAdvertise must survive a JSON roundtrip"
    );
    let Some(back) = back else {
        return;
    };
    assert!(
        original == back,
        "a JSON roundtrip must preserve CapabilityAdvertise"
    );

    let pairing = PairingRequest {
        device_descriptor: "owner laptop".to_string(),
    };
    let pairing_back = roundtrip(&pairing);
    assert!(
        pairing_back.is_some(),
        "PairingRequest must survive a JSON roundtrip"
    );
    let Some(pairing_back) = pairing_back else {
        return;
    };
    assert!(
        pairing == pairing_back,
        "a JSON roundtrip must preserve PairingRequest"
    );

    let paired = PairingResult::Paired {
        device_id: Uuid::new_v4(),
    };
    let paired_back: Option<PairingResult> = roundtrip(&paired);
    assert!(
        paired_back.is_some(),
        "PairingResult::Paired must survive a JSON roundtrip"
    );
    let Some(paired_back) = paired_back else {
        return;
    };
    assert!(
        paired == paired_back,
        "a JSON roundtrip must preserve PairingResult"
    );

    let challenge = AuthChallenge {
        nonce: "nonce-1".to_string(),
    };
    let challenge_back = roundtrip(&challenge);
    assert!(
        challenge_back.is_some(),
        "AuthChallenge must survive a JSON roundtrip"
    );
    let Some(challenge_back): Option<AuthChallenge> = challenge_back else {
        return;
    };
    assert!(
        challenge == challenge_back,
        "a JSON roundtrip must preserve AuthChallenge"
    );

    let proof = AuthProof {
        proof: "proof-1".to_string(),
    };
    let proof_back = roundtrip(&proof);
    assert!(
        proof_back.is_some(),
        "AuthProof must survive a JSON roundtrip"
    );
    let Some(proof_back): Option<AuthProof> = proof_back else {
        return;
    };
    assert!(
        proof == proof_back,
        "a JSON roundtrip must preserve AuthProof"
    );

    let accepted = AuthResult::Accepted {
        connection_id: Uuid::new_v4(),
    };
    let accepted_back: Option<AuthResult> = roundtrip(&accepted);
    assert!(
        accepted_back.is_some(),
        "AuthResult::Accepted must survive a JSON roundtrip"
    );
    let Some(accepted_back) = accepted_back else {
        return;
    };
    assert!(
        accepted == accepted_back,
        "a JSON roundtrip must preserve AuthResult"
    );

    let negotiated = NegotiatedConnection {
        version: ProtocolVersion::V1,
        accepted_features: vec![ClientFeatureKind::Text],
    };
    let negotiated_back = roundtrip(&negotiated);
    assert!(
        negotiated_back.is_some(),
        "NegotiatedConnection must survive a JSON roundtrip"
    );
    let Some(negotiated_back) = negotiated_back else {
        return;
    };
    assert!(
        negotiated == negotiated_back,
        "a JSON roundtrip must preserve NegotiatedConnection"
    );

    let hello_back: Option<ReconnectHello> = roundtrip(&ReconnectHello);
    assert!(
        hello_back.is_some(),
        "ReconnectHello must survive a JSON roundtrip"
    );
    let Some(hello_back) = hello_back else {
        return;
    };
    assert!(
        ReconnectHello == hello_back,
        "a JSON roundtrip must preserve ReconnectHello"
    );

    let invite = RecoveryInvite {
        hint: "resume here".to_string(),
    };
    let invite_back = roundtrip(&invite);
    assert!(
        invite_back.is_some(),
        "RecoveryInvite must survive a JSON roundtrip"
    );
    let Some(invite_back): Option<RecoveryInvite> = invite_back else {
        return;
    };
    assert!(
        invite == invite_back,
        "a JSON roundtrip must preserve RecoveryInvite"
    );

    let notice = DisconnectNotice {
        reason: "idle".to_string(),
    };
    let notice_back = roundtrip(&notice);
    assert!(
        notice_back.is_some(),
        "DisconnectNotice must survive a JSON roundtrip"
    );
    let Some(notice_back): Option<DisconnectNotice> = notice_back else {
        return;
    };
    assert!(
        notice == notice_back,
        "a JSON roundtrip must preserve DisconnectNotice"
    );
}

#[test]
fn round_roundtrips_through_json() {
    let original = SubmitTextInput {
        companion: "companion-1".to_string(),
        round: None,
        local_id: "local-1".to_string(),
        text: "hello".to_string(),
        lang: "en".to_string(),
    };
    let back = roundtrip(&original);
    assert!(
        back.is_some(),
        "SubmitTextInput must survive a JSON roundtrip"
    );
    let Some(back) = back else {
        return;
    };
    assert!(
        original == back,
        "a JSON roundtrip must preserve SubmitTextInput"
    );

    let outcome = RoundIntakeOutcome::StaleRound {
        current_round: Some("round-2".to_string()),
        current_generation: 4,
    };
    let outcome_back: Option<RoundIntakeOutcome> = roundtrip(&outcome);
    assert!(
        outcome_back.is_some(),
        "RoundIntakeOutcome must survive a JSON roundtrip"
    );
    let Some(outcome_back) = outcome_back else {
        return;
    };
    assert!(
        outcome == outcome_back,
        "a JSON roundtrip must preserve RoundIntakeOutcome"
    );

    let open = TextStreamOpen {
        stream: Uuid::new_v4(),
        round: "round-2".to_string(),
        generation: 4,
    };
    let open_back = roundtrip(&open);
    assert!(
        open_back.is_some(),
        "TextStreamOpen must survive a JSON roundtrip"
    );
    let Some(open_back): Option<TextStreamOpen> = open_back else {
        return;
    };
    assert!(
        open == open_back,
        "a JSON roundtrip must preserve TextStreamOpen"
    );

    let frame = TextStreamFrame {
        stream: Uuid::new_v4(),
        seq: 0,
        delta: "hel".to_string(),
        is_final: false,
    };
    let frame_back = roundtrip(&frame);
    assert!(
        frame_back.is_some(),
        "TextStreamFrame must survive a JSON roundtrip"
    );
    let Some(frame_back): Option<TextStreamFrame> = frame_back else {
        return;
    };
    assert!(
        frame == frame_back,
        "a JSON roundtrip must preserve TextStreamFrame"
    );

    let close = TextStreamClose {
        stream: Uuid::new_v4(),
        status: StreamClose::Completed,
    };
    let close_back = roundtrip(&close);
    assert!(
        close_back.is_some(),
        "TextStreamClose must survive a JSON roundtrip"
    );
    let Some(close_back): Option<TextStreamClose> = close_back else {
        return;
    };
    assert!(
        close == close_back,
        "a JSON roundtrip must preserve TextStreamClose"
    );

    let confirm = ConfirmPresentation {
        round: "round-2".to_string(),
        stream: None,
        status: PresentationStatus::Presented,
        detail: None,
    };
    let confirm_back = roundtrip(&confirm);
    assert!(
        confirm_back.is_some(),
        "ConfirmPresentation must survive a JSON roundtrip"
    );
    let Some(confirm_back): Option<ConfirmPresentation> = confirm_back else {
        return;
    };
    assert!(
        confirm == confirm_back,
        "a JSON roundtrip must preserve ConfirmPresentation"
    );

    let view = HistoryView {
        items: vec![HistoryItem {
            round: "round-1".to_string(),
            role: HistoryRole::Owner,
            text: "hi".to_string(),
            at: "2026-09-08T12:00:00+09:00".to_string(),
        }],
    };
    let view_back = roundtrip(&view);
    assert!(
        view_back.is_some(),
        "HistoryView must survive a JSON roundtrip"
    );
    let Some(view_back): Option<HistoryView> = view_back else {
        return;
    };
    assert!(
        view == view_back,
        "a JSON roundtrip must preserve HistoryView"
    );

    let request = HistoryRequest {
        companion: "companion-1".to_string(),
        since: Some("2026-09-08T00:00:00+09:00".to_string()),
        limit: 20,
    };
    let request_back = roundtrip(&request);
    assert!(
        request_back.is_some(),
        "HistoryRequest must survive a JSON roundtrip"
    );
    let Some(request_back): Option<HistoryRequest> = request_back else {
        return;
    };
    assert!(
        request == request_back,
        "a JSON roundtrip must preserve HistoryRequest"
    );
}

#[test]
fn presence_roundtrips_through_json() {
    let original = PresenceAttribution {
        companion: "companion-1".to_string(),
        state: PresenceState::Present,
        active_client: Some("client-1".to_string()),
        generation: 9,
        move_reason: None,
    };
    let back = roundtrip(&original);
    assert!(
        back.is_some(),
        "PresenceAttribution must survive a JSON roundtrip"
    );
    let Some(back) = back else {
        return;
    };
    assert!(
        original == back,
        "a JSON roundtrip must preserve PresenceAttribution"
    );
}

#[test]
fn management_roundtrips_through_json() {
    let original = ManagementIntent {
        intent_id: Uuid::new_v4(),
        kind: SetupIntentKind::SelectProvider,
        target: "provider-openai".to_string(),
        base_view: Some("view-3".to_string()),
        rationale: None,
    };
    let back = roundtrip(&original);
    assert!(
        back.is_some(),
        "ManagementIntent must survive a JSON roundtrip"
    );
    let Some(back) = back else {
        return;
    };
    assert!(
        original == back,
        "a JSON roundtrip must preserve ManagementIntent"
    );

    let outcome = ManagementOutcome::StaleBaseView {
        current: "view-4".to_string(),
    };
    let outcome_back: Option<ManagementOutcome> = roundtrip(&outcome);
    assert!(
        outcome_back.is_some(),
        "ManagementOutcome must survive a JSON roundtrip"
    );
    let Some(outcome_back) = outcome_back else {
        return;
    };
    assert!(
        outcome == outcome_back,
        "a JSON roundtrip must preserve ManagementOutcome"
    );

    let request = ManagementViewRequest {
        sections: vec!["setup".to_string()],
    };
    let request_back = roundtrip(&request);
    assert!(
        request_back.is_some(),
        "ManagementViewRequest must survive a JSON roundtrip"
    );
    let Some(request_back): Option<ManagementViewRequest> = request_back else {
        return;
    };
    assert!(
        request == request_back,
        "a JSON roundtrip must preserve ManagementViewRequest"
    );

    let view = ManagementView {
        mark: "view-4".to_string(),
        sections: vec![ViewSection {
            kind: "setup".to_string(),
            title: "Setup".to_string(),
            body: "choose a provider".to_string(),
        }],
    };
    let view_back = roundtrip(&view);
    assert!(
        view_back.is_some(),
        "ManagementView must survive a JSON roundtrip"
    );
    let Some(view_back): Option<ManagementView> = view_back else {
        return;
    };
    assert!(
        view == view_back,
        "a JSON roundtrip must preserve ManagementView"
    );
}

#[test]
fn unknown_fields_are_tolerated() {
    let original = sample_envelope();
    let encoded = serde_json::to_value(&original).ok();
    assert!(
        encoded.is_some(),
        "WireEnvelope must encode to a JSON value"
    );
    let Some(mut encoded) = encoded else {
        return;
    };
    let Some(object) = encoded.as_object_mut() else {
        return;
    };
    object.insert(
        "future_field".to_string(),
        serde_json::Value::String("ignored".to_string()),
    );
    let back: Option<WireEnvelope> = serde_json::from_value(encoded).ok();
    assert!(
        back.is_some(),
        "unknown fields must deserialize without error"
    );
    let Some(back) = back else {
        return;
    };
    assert!(
        original == back,
        "unknown fields must not change the decoded value"
    );

    let input = SubmitTextInput {
        companion: "companion-1".to_string(),
        round: Some("round-1".to_string()),
        local_id: "local-2".to_string(),
        text: "hey".to_string(),
        lang: "en".to_string(),
    };
    let encoded = serde_json::to_value(&input).ok();
    assert!(
        encoded.is_some(),
        "SubmitTextInput must encode to a JSON value"
    );
    let Some(mut encoded) = encoded else {
        return;
    };
    let Some(object) = encoded.as_object_mut() else {
        return;
    };
    object.insert(
        "another_future_field".to_string(),
        serde_json::Value::Bool(true),
    );
    let back: Option<SubmitTextInput> = serde_json::from_value(encoded).ok();
    assert!(
        back.is_some(),
        "unknown payload fields must deserialize without error"
    );
    let Some(back) = back else {
        return;
    };
    assert!(
        input == back,
        "unknown payload fields must not change the decoded value"
    );
}

#[test]
fn protocol_version_compat_requires_equal_major() {
    let minor_bump = ProtocolVersion { major: 1, minor: 7 };
    assert!(
        ProtocolVersion::V1.is_compatible_with(&minor_bump),
        "the same major must stay compatible across minors"
    );
    assert!(
        minor_bump.is_compatible_with(&ProtocolVersion::V1),
        "compatibility must be symmetric within a major"
    );
    let next_major = ProtocolVersion { major: 2, minor: 0 };
    assert!(
        !ProtocolVersion::V1.is_compatible_with(&next_major),
        "a different major must be rejected"
    );
    assert!(
        !next_major.is_compatible_with(&ProtocolVersion::V1),
        "major rejection must be symmetric"
    );
}
