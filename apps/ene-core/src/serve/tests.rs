use super::{HostHandle, LiveInput, conn_key, device_client};
use crate::test_support::{live_input, memory_handle};
use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
use ene_api::v1::handshake::{
    AuthChallenge, AuthProof, AuthResult, CapabilityAdvertise, PairingRequest, PairingResult,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{ClientIncarnationId, ConnectionWireId, DeviceWireId, WireMessageType};
use ene_api::v1::refs::{ClientLocalId, CompanionWireRef, TextLangWire};
use ene_api::v1::round::{HistoryRequest, SubmitTextInput, TextBodyWire};
use ene_credential::pairing_proof_hex;
use ene_inference::fake::FakeProviderTransport;
use ene_presence::{ClientId, PresenceRepository};

fn sender() -> WireSender {
    WireSender {
        device_id: None,
        incarnation_id: ClientIncarnationId {
            counter: 1,
            random: 2,
        },
        connection_id: None,
    }
}

/// Stamps a frame with the connection binding its `live` premises carry, as a
/// connection-table-built frame would: direct-handle tests build envelopes by
/// hand.
fn stamped(mut frame: super::WireFrame, live: &LiveInput) -> super::WireFrame {
    frame.envelope.sender.connection_id = Some(live.connection_id);
    frame
}

/// Builds the paired [`LiveInput`] premises for one device on a fresh id.
///
/// Constructed explicitly (never the shared helper): auth-flow tests bind
/// several frames to one connection, so the id must stay fixed across
/// them. `authed` holds: these premises stand in for a connection table
/// entry after its `Accepted`, so post-accept domain frames pass the
/// gate; bypass tests override it explicitly.
fn paired_input(device_wire: &str) -> LiveInput {
    LiveInput {
        client_ref: device_wire.to_string(),
        connection_live: true,
        peer_uid_ok: true,
        paired_device: Some(device_wire.to_string()),
        connection_known: true,
        authed: true,
        connection_id: ConnectionWireId(uuid::Uuid::new_v4()),
        negotiated: None,
    }
}

fn pairing_frame(descriptor: &str) -> super::WireFrame {
    super::WireFrame {
        envelope: new_outgoing_envelope(
            ProtocolVersion::V1,
            sender(),
            WireMessageType(String::from("PairingRequest")),
        ),
        payload: WirePayload::PairingRequest(PairingRequest {
            device_descriptor: descriptor.to_string(),
        }),
    }
}

fn advertise_frame(protocol: ProtocolVersion) -> super::WireFrame {
    super::WireFrame {
        envelope: new_outgoing_envelope(
            ProtocolVersion::V1,
            sender(),
            WireMessageType(String::from("CapabilityAdvertise")),
        ),
        payload: WirePayload::CapabilityAdvertise(CapabilityAdvertise {
            supported_protocol: vec![protocol],
            features: Vec::new(),
            platform: String::from("test"),
        }),
    }
}

fn submit_frame() -> super::WireFrame {
    super::WireFrame {
        envelope: new_outgoing_envelope(
            ProtocolVersion::V1,
            sender(),
            WireMessageType(String::from("SubmitTextInput")),
        ),
        payload: WirePayload::SubmitTextInput(SubmitTextInput {
            companion: CompanionWireRef(String::from("companion-echo")),
            round: None,
            fresh: false,
            local_id: ClientLocalId(String::from("local-1")),
            body: TextBodyWire {
                text: String::from("hello"),
                lang: TextLangWire(String::from("en")),
            },
        }),
    }
}

fn history_frame() -> super::WireFrame {
    super::WireFrame {
        envelope: new_outgoing_envelope(
            ProtocolVersion::V1,
            sender(),
            WireMessageType(String::from("HistoryRequest")),
        ),
        payload: WirePayload::HistoryRequest(HistoryRequest {
            companion: CompanionWireRef(String::from("companion-echo")),
            since: None,
            limit: 10,
        }),
    }
}

/// Premises for a connection that never paired (and so cannot be authed).
fn unpaired_input() -> LiveInput {
    LiveInput {
        paired_device: None,
        connection_known: false,
        authed: false,
        ..live_input("client-a")
    }
}

fn fake_transport() -> FakeProviderTransport {
    FakeProviderTransport::new(String::new(), None)
}

async fn open_handle(tag: &str) -> Option<(HostHandle, tempfile::TempDir)> {
    memory_handle(tag).await
}

#[test]
fn device_mapping_is_deterministic_per_device() {
    let first = device_client("laptop");
    let second = device_client("laptop");
    assert_eq!(first, second, "one device maps to one client");
    let other = device_client("phone");
    assert_ne!(first, other, "different devices map to different clients");
}

#[test]
fn device_mapping_is_not_issuance() {
    let mapped = device_client("laptop");
    assert_ne!(
        mapped,
        ClientId::generate(),
        "the mapping never mints a fresh identity"
    );
}

#[tokio::test]
async fn pairing_denies_an_unauthorized_peer() {
    let (handle, _dir) = open_handle("pair-deny").await.unwrap();
    let denied_input = LiveInput {
        peer_uid_ok: false,
        ..live_input("client-a")
    };
    let transport = fake_transport();
    let responses = handle
        .handle_frame(pairing_frame("laptop"), denied_input, &transport)
        .await;
    assert_eq!(responses.len(), 1, "denial answers exactly one frame");
    let first = responses.first().unwrap();
    assert!(
        matches!(
            &first.payload,
            WirePayload::PairingResult(PairingResult::Denied { .. })
        ),
        "an unauthorized peer is denied"
    );
}

#[tokio::test]
async fn pairing_denies_a_blank_descriptor() {
    let (handle, _dir) = open_handle("pair-blank").await.unwrap();
    let transport = fake_transport();
    for descriptor in ["", "   "] {
        let responses = handle
            .handle_frame(pairing_frame(descriptor), unpaired_input(), &transport)
            .await;
        assert_eq!(responses.len(), 1, "blank denial answers once");
        let Some(first) = responses.first() else {
            continue;
        };
        assert!(
            matches!(
                &first.payload,
                WirePayload::PairingResult(PairingResult::Denied { .. })
            ),
            "a blank descriptor is denied, never pended"
        );
    }
    let pending = handle.pending_devices().await;
    let descriptors = pending.unwrap();
    assert!(
        descriptors.is_empty(),
        "blank descriptors leave no pending entry"
    );
}

#[tokio::test]
async fn pairing_pends_then_pairs_after_owner_approval() {
    let (handle, _dir) = open_handle("pair-flow").await.unwrap();
    let transport = fake_transport();
    let pending = handle
        .handle_frame(pairing_frame("laptop"), unpaired_input(), &transport)
        .await;
    assert_eq!(pending.len(), 1, "the request answers once");
    let first = pending.first().unwrap();
    assert!(
        matches!(
            &first.payload,
            WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation)
        ),
        "a fresh descriptor pends, never auto-approves"
    );
    let listed = handle.pending_devices().await;
    let descriptors = listed.unwrap();
    assert!(
        descriptors.iter().any(|name| name == "laptop"),
        "the pending descriptor lists for the Owner"
    );
    let unknown = handle.approve_device("unknown box").await.unwrap();
    assert!(unknown.is_none(), "an unknown descriptor approves nothing");
    let approved = handle.approve_device("laptop").await.unwrap();
    assert!(approved.is_some(), "owner approval must pair");
    let paired_frame = pairing_frame("laptop");
    let expected_reply = paired_frame.envelope.message_id;
    let paired = handle
        .handle_frame(paired_frame, unpaired_input(), &transport)
        .await;
    let answer = paired.first().unwrap();
    assert!(
        matches!(
            &answer.payload,
            WirePayload::PairingResult(PairingResult::Paired { .. })
        ),
        "an approved descriptor pairs on re-request"
    );
    assert_eq!(
        answer.envelope.correlation.reply_to,
        Some(expected_reply),
        "the reply links back to the request"
    );
}

#[tokio::test]
async fn an_already_paired_connection_cannot_pair_again() {
    let (handle, _dir) = open_handle("pair-immutable").await.unwrap();
    let transport = fake_transport();
    let responses = handle
        .handle_frame(
            pairing_frame("other box"),
            paired_input("device-1"),
            &transport,
        )
        .await;
    assert_eq!(responses.len(), 1, "the refusal answers exactly one frame");
    let first = responses.first().unwrap();
    assert!(
        matches!(
            &first.payload,
            WirePayload::PairingResult(PairingResult::Denied { .. })
        ),
        "a paired connection never re-pairs"
    );
    let pending = handle.pending_devices().await;
    let descriptors = pending.unwrap();
    assert!(
        descriptors.is_empty(),
        "the refused descriptor leaves no pending entry"
    );
}

#[tokio::test]
async fn unpaired_domain_frames_close_with_an_unpaired_notice() {
    let (handle, _dir) = open_handle("gate-drop").await.unwrap();
    let transport = fake_transport();
    let responses = handle
        .handle_frame(submit_frame(), unpaired_input(), &transport)
        .await;
    assert_eq!(responses.len(), 1, "the gate answers once");
    let first = responses.first().unwrap();
    assert!(
        matches!(
            &first.payload,
            WirePayload::DisconnectNotice(notice) if notice.reason == "unpaired"
        ),
        "an unpaired submit closes with the unpaired notice"
    );
    let history = handle
        .handle_frame(history_frame(), unpaired_input(), &transport)
        .await;
    let view = history.first().unwrap();
    assert!(
        matches!(
            &view.payload,
            WirePayload::DisconnectNotice(notice) if notice.reason == "unpaired"
        ),
        "an unpaired history request closes with the unpaired notice"
    );
}

#[tokio::test]
async fn unknown_connection_closes_even_with_a_device() {
    let (handle, _dir) = open_handle("gate-unknown").await.unwrap();
    let transport = fake_transport();
    let input = LiveInput {
        paired_device: Some(String::from("laptop")),
        connection_known: false,
        ..live_input("client-a")
    };
    let responses = handle.handle_frame(submit_frame(), input, &transport).await;
    let first = responses.first().unwrap();
    assert!(
        matches!(
            &first.payload,
            WirePayload::DisconnectNotice(notice) if notice.reason == "unpaired"
        ),
        "an unknown connection closes even when it names a device"
    );
}

#[tokio::test]
async fn unsolicited_challenge_and_result_answer_nothing() {
    let (handle, _dir) = open_handle("auth-deferred").await.unwrap();
    let transport = fake_transport();
    let challenge = super::WireFrame {
        envelope: new_outgoing_envelope(
            ProtocolVersion::V1,
            sender(),
            WireMessageType(String::from("AuthChallenge")),
        ),
        payload: WirePayload::AuthChallenge(AuthChallenge {
            nonce: String::from("nonce-1"),
        }),
    };
    let result = super::WireFrame {
        envelope: new_outgoing_envelope(
            ProtocolVersion::V1,
            sender(),
            WireMessageType(String::from("AuthResult")),
        ),
        payload: WirePayload::AuthResult(AuthResult::Rejected {
            reason: String::from("nope"),
        }),
    };
    for frame in [challenge, result] {
        let responses = handle
            .handle_frame(frame, unpaired_input(), &transport)
            .await;
        assert!(
            responses.is_empty(),
            "unsolicited challenge/result frames answer nothing, even unpaired"
        );
    }
}

fn proof_frame(device_id: DeviceWireId, proof: &str) -> super::WireFrame {
    let mut envelope = new_outgoing_envelope(
        ProtocolVersion::V1,
        WireSender {
            device_id: Some(device_id),
            incarnation_id: ClientIncarnationId {
                counter: 5,
                random: 6,
            },
            connection_id: None,
        },
        WireMessageType(String::from("AuthProof")),
    );
    envelope.observed.presence_generation_view = None;
    super::WireFrame {
        envelope,
        payload: WirePayload::AuthProof(AuthProof {
            proof: proof.to_string(),
        }),
    }
}

#[tokio::test]
async fn challenge_proof_accepts_and_binds_the_connection() {
    let (handle, _dir) = open_handle("auth-flow").await.unwrap();
    let transport = fake_transport();
    let pending = handle
        .handle_frame(pairing_frame("laptop"), unpaired_input(), &transport)
        .await;
    assert!(
        pending.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation)
        )),
        "a fresh descriptor pends, got {pending:?}"
    );
    for response in &pending {
        assert_eq!(
            response.envelope.sender.connection_id, None,
            "a pre-accept pairing answer hides the connection id"
        );
    }
    let approved = handle.approve_device("laptop").await;
    assert!(
        matches!(approved, Ok(Some(_))),
        "owner approval must pair, got {approved:?}"
    );
    let (record, secret) = approved.unwrap().unwrap();
    let device_wire = record.wire.clone();
    let paired = handle
        .handle_frame(pairing_frame("laptop"), unpaired_input(), &transport)
        .await;
    let answer = paired.first().unwrap();
    let WirePayload::PairingResult(PairingResult::Paired { device_id }) = &answer.payload else {
        return;
    };
    let device_id = *device_id;
    assert_eq!(
        answer.envelope.sender.connection_id, None,
        "even the Paired answer hides the connection id"
    );
    assert_eq!(
        device_id.0.as_hyphenated().to_string(),
        device_wire,
        "the issued device key is the stored opaque projection"
    );
    assert_ne!(
        device_wire,
        record.id.0.as_uuid().as_hyphenated().to_string(),
        "the issued key never renders the domain identity"
    );
    let live = paired_input(&device_wire);
    let challenged = handle
        .handle_frame(
            advertise_frame(ProtocolVersion::V1),
            live.clone(),
            &transport,
        )
        .await;
    assert_eq!(
        challenged.len(),
        2,
        "capability answers terms plus challenge"
    );
    for response in &challenged {
        assert_eq!(
            response.envelope.sender.connection_id, None,
            "negotiation and challenge hide the connection id"
        );
    }
    let challenge_frame = challenged.get(1).unwrap();
    let WirePayload::AuthChallenge(challenge) = &challenge_frame.payload else {
        return;
    };
    let nonce = challenge.nonce.clone();
    assert!(!nonce.is_empty(), "the challenge carries a fresh nonce");
    let proof = pairing_proof_hex(&secret, &nonce);
    let attempt = proof_frame(device_id, &proof);
    let expected_reply = attempt.envelope.message_id;
    let answered = handle.handle_frame(attempt, live.clone(), &transport).await;
    assert_eq!(answered.len(), 2, "a proof answers result plus fact");
    let accepted = answered.first().unwrap();
    assert!(
        matches!(
            &accepted.payload,
            WirePayload::AuthResult(AuthResult::Accepted { connection_id })
            if *connection_id == live.connection_id
        ),
        "a valid proof is accepted with this connection id, got {:?}",
        accepted.payload
    );
    assert_eq!(
        accepted.envelope.correlation.reply_to,
        Some(expected_reply),
        "the result links back to the proof"
    );
    assert_eq!(
        accepted.envelope.sender.device_id,
        Some(device_id),
        "the result names the paired target"
    );
    assert_eq!(
        accepted.envelope.sender.incarnation_id,
        ClientIncarnationId {
            counter: 5,
            random: 6,
        },
        "the result echoes the inbound incarnation"
    );
    assert_eq!(
        accepted.envelope.sender.connection_id,
        Some(live.connection_id),
        "the result echoes the connection"
    );
    let fact = answered.get(1).unwrap();
    assert!(
        matches!(&fact.payload, WirePayload::PresenceAttribution(_)),
        "acceptance carries the attribution fact, got {:?}",
        fact.payload
    );
    assert_eq!(
        fact.envelope.sender.connection_id,
        Some(live.connection_id),
        "the piggybacked fact rides the acceptance, so it reveals too"
    );
    let replayed = handle
        .handle_frame(proof_frame(device_id, &proof), live.clone(), &transport)
        .await;
    assert!(
        replayed.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::AuthResult(AuthResult::Rejected { .. })
        )),
        "the consumed nonce never answers twice, got {replayed:?}"
    );
    for response in &replayed {
        assert_eq!(
            response.envelope.sender.connection_id, None,
            "a rejection hides the connection id even post-accept"
        );
    }
    let rechallenged = handle
        .handle_frame(
            advertise_frame(ProtocolVersion::V1),
            live.clone(),
            &transport,
        )
        .await;
    let fresh = rechallenged.get(1).unwrap();
    let WirePayload::AuthChallenge(fresh_challenge) = &fresh.payload else {
        return;
    };
    assert_ne!(
        fresh_challenge.nonce, nonce,
        "re-advertising mints a fresh nonce"
    );
    let wrong = handle
        .handle_frame(proof_frame(device_id, "deadbeef"), live.clone(), &transport)
        .await;
    assert!(
        wrong.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::AuthResult(AuthResult::Rejected { reason })
            if reason == "invalid proof"
        )),
        "a bad proof is rejected, got {wrong:?}"
    );
    let rechallenged = handle
        .handle_frame(
            advertise_frame(ProtocolVersion::V1),
            live.clone(),
            &transport,
        )
        .await;
    let fresh = rechallenged.get(1).unwrap();
    assert!(
        matches!(&fresh.payload, WirePayload::AuthChallenge(_)),
        "re-advertising challenges again, got {:?}",
        fresh.payload
    );
    let unknown_device = DeviceWireId(uuid::Uuid::new_v4());
    let unknown = handle
        .handle_frame(
            proof_frame(unknown_device, "deadbeef"),
            live.clone(),
            &transport,
        )
        .await;
    assert!(
        unknown.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::AuthResult(AuthResult::Rejected { .. })
        )),
        "an unknown device is rejected, got {unknown:?}"
    );
    let bound = handle
        .handle_frame(stamped(submit_frame(), &live), live.clone(), &transport)
        .await;
    assert!(
        !bound
            .first()
            .is_some_and(|first| matches!(&first.payload, WirePayload::DisconnectNotice(_))),
        "a connection-bound frame passes the gate, got {bound:?}"
    );
    let mut stray = submit_frame();
    stray.envelope.sender.connection_id = Some(ConnectionWireId(uuid::Uuid::new_v4()));
    let dropped = handle.handle_frame(stray, live.clone(), &transport).await;
    assert!(
        dropped.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::DisconnectNotice(notice) if notice.reason == "unpaired"
        )),
        "a foreign connection id closes with the unpaired notice, got {dropped:?}"
    );
}

#[tokio::test]
async fn pre_accept_denials_and_closes_hide_the_connection_id() {
    let (handle, _dir) = open_handle("auth-hidden").await.unwrap();
    let transport = fake_transport();
    let live = live_input("client-a");
    let denied = handle
        .handle_frame(pairing_frame("laptop"), unpaired_input(), &transport)
        .await;
    // The fresh descriptor pends rather than denying; the peer-mismatch
    // denial and the two closes below are the hiding cases.
    assert!(
        denied.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation)
        )),
        "a fresh descriptor pends, got {denied:?}"
    );
    let mismatched = LiveInput {
        peer_uid_ok: false,
        ..unpaired_input()
    };
    let refused = handle
        .handle_frame(pairing_frame("laptop"), mismatched, &transport)
        .await;
    assert!(
        refused.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::PairingResult(PairingResult::Denied { .. })
        )),
        "an unauthorized peer is denied, got {refused:?}"
    );
    let mismatch = handle
        .handle_frame(
            advertise_frame(ProtocolVersion { major: 9, minor: 0 }),
            live.clone(),
            &transport,
        )
        .await;
    assert!(
        mismatch
            .first()
            .is_some_and(|first| matches!(&first.payload, WirePayload::DisconnectNotice(_))),
        "a major mismatch disconnects, got {mismatch:?}"
    );
    let dropped = handle
        .handle_frame(submit_frame(), unpaired_input(), &transport)
        .await;
    assert!(
        dropped.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::DisconnectNotice(notice) if notice.reason == "unpaired"
        )),
        "an unpaired submit closes, got {dropped:?}"
    );
    for response in denied
        .iter()
        .chain(&refused)
        .chain(&mismatch)
        .chain(&dropped)
    {
        assert_eq!(
            response.envelope.sender.connection_id, None,
            "no pre-accept response may reveal the connection id, got {:?}",
            response.payload
        );
        assert!(
            response.envelope.correlation.reply_to.is_some(),
            "hiding never drops the reply link, got {:?}",
            response.payload
        );
    }
}

#[tokio::test]
async fn unauthed_domain_frame_closes_even_without_a_connection_id() {
    let (handle, _dir) = open_handle("gate-bypass").await.unwrap();
    let transport = fake_transport();
    // A paired device that skipped the proof: the bypass attempt carries
    // no connection id because pre-accept responses never reveal it.
    let bypass = LiveInput {
        paired_device: Some(String::from("laptop")),
        connection_known: true,
        authed: false,
        ..live_input("client-a")
    };
    let without_id = submit_frame();
    assert_eq!(
        without_id.envelope.sender.connection_id, None,
        "the bypass frame carries no connection id"
    );
    let dropped = handle
        .handle_frame(without_id, bypass.clone(), &transport)
        .await;
    assert!(
        dropped.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::DisconnectNotice(notice) if notice.reason == "unpaired"
        )),
        "an unauthed domain frame closes, got {dropped:?}"
    );
    for response in &dropped {
        assert_eq!(
            response.envelope.sender.connection_id, None,
            "the drop itself reveals nothing"
        );
    }
    // Even a fully authed connection must still echo the id: `None` is
    // never a valid binding.
    let authed = LiveInput {
        authed: true,
        ..bypass
    };
    let dropped = handle
        .handle_frame(submit_frame(), authed, &transport)
        .await;
    assert!(
        dropped.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::DisconnectNotice(notice) if notice.reason == "unpaired"
        )),
        "an id-less frame on an authed connection still closes, got {dropped:?}"
    );
}

#[tokio::test]
async fn superseded_connection_replay_closes_despite_a_known_id() {
    let (handle, _dir) = open_handle("gate-superseded").await.unwrap();
    let transport = fake_transport();
    // The connection table reports a superseded connection as unauthed
    // (see the conn-level supersede test): the envelope echoes the table
    // id exactly, yet the gate must still drop the frame because the
    // device authenticated anew elsewhere.
    let stale = LiveInput {
        paired_device: Some(String::from("laptop")),
        connection_known: true,
        authed: false,
        ..live_input("client-a")
    };
    let replay = stamped(submit_frame(), &stale);
    assert_eq!(
        replay.envelope.sender.connection_id,
        Some(stale.connection_id),
        "the replay echoes the table id exactly"
    );
    let dropped = handle.handle_frame(replay, stale, &transport).await;
    assert!(
        dropped.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::DisconnectNotice(notice) if notice.reason == "unpaired"
        )),
        "a known-id replay on a superseded connection closes, got {dropped:?}"
    );
}

#[tokio::test]
async fn approved_secret_verifies_from_a_fresh_handle_on_the_same_dir() {
    use ene_credential::MemoryCredentialStore;

    use super::CredStore;

    let dir = tempfile::tempdir().expect("test scratch directory must be creatable");
    let transport = fake_transport();
    let opened = HostHandle::open_with_cred_store(
        dir.path(),
        CredStore::Memory(MemoryCredentialStore::new()),
    )
    .await;
    let first = opened.unwrap();
    let pending = first
        .handle_frame(pairing_frame("laptop"), unpaired_input(), &transport)
        .await;
    assert!(
        pending.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation)
        )),
        "a fresh descriptor pends, got {pending:?}"
    );
    let approved = first.approve_device("laptop").await;
    assert!(
        matches!(approved, Ok(Some(_))),
        "owner approval must pair, got {approved:?}"
    );
    let (record, secret) = approved.unwrap().unwrap();
    assert!(
        dir.path().join("device-auth.json").exists(),
        "approval persists the secret to the device-auth file"
    );
    drop(first);
    // A fresh handle holds no secret map at all: if verification reads
    // only memory, this proof must fail. It must pass from the file.
    let reopened = HostHandle::open_with_cred_store(
        dir.path(),
        CredStore::Memory(MemoryCredentialStore::new()),
    )
    .await;
    let second = reopened.unwrap();
    let device_wire = record.wire.clone();
    let live = paired_input(&device_wire);
    let challenged = second
        .handle_frame(
            advertise_frame(ProtocolVersion::V1),
            live.clone(),
            &transport,
        )
        .await;
    let challenge_frame = challenged.get(1).unwrap();
    let WirePayload::AuthChallenge(challenge) = &challenge_frame.payload else {
        return;
    };
    let proof = pairing_proof_hex(&secret, &challenge.nonce);
    let device_uuid = uuid::Uuid::parse_str(&device_wire).unwrap();
    let answered = second
        .handle_frame(
            proof_frame(DeviceWireId(device_uuid), &proof),
            live,
            &transport,
        )
        .await;
    assert!(
        answered.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::AuthResult(AuthResult::Accepted { .. })
        )),
        "the file-backed secret verifies with no re-approval, got {answered:?}"
    );
}

#[tokio::test]
async fn proof_without_challenge_is_rejected() {
    let (handle, _dir) = open_handle("auth-nochallenge").await.unwrap();
    let transport = fake_transport();
    let proof = super::WireFrame {
        envelope: new_outgoing_envelope(
            ProtocolVersion::V1,
            sender(),
            WireMessageType(String::from("AuthProof")),
        ),
        payload: WirePayload::AuthProof(AuthProof {
            proof: String::from("proof-1"),
        }),
    };
    let live = live_input("client-a");
    let responses = handle.handle_frame(proof, live.clone(), &transport).await;
    assert_eq!(responses.len(), 1, "a proof answers exactly one frame");
    let first = responses.first().unwrap();
    assert!(
        matches!(
            &first.payload,
            WirePayload::AuthResult(AuthResult::Rejected { reason })
            if reason == "no pending challenge"
        ),
        "a proof with no pending challenge is rejected, got {:?}",
        first.payload
    );
    assert_eq!(
        first.envelope.sender.connection_id, None,
        "a pre-accept rejection hides the connection id"
    );
    assert_eq!(
        first.envelope.sender.incarnation_id,
        sender().incarnation_id,
        "hiding the connection never drops the incarnation echo"
    );
}

#[tokio::test]
async fn negotiated_version_is_fixed_per_connection() {
    use ene_api::v1::handshake::NegotiatedConnection;

    let (handle, _dir) = open_handle("version-fixed").await.unwrap();
    let transport = fake_transport();
    let negotiated = LiveInput {
        negotiated: Some(NegotiatedConnection {
            version: ProtocolVersion::V1,
            accepted_features: Vec::new(),
        }),
        ..paired_input("device-1")
    };
    let mut mixed = submit_frame();
    mixed.envelope.protocol = ProtocolVersion {
        major: 1,
        minor: 99,
    };
    let rejected = handle
        .handle_frame(mixed, negotiated.clone(), &transport)
        .await;
    assert!(
        rejected.first().is_some_and(|first| matches!(
            &first.payload,
            WirePayload::Reject(notice)
                if notice.kind == ene_api::v1::reject::RejectKind::IncompatibleProtocol
        )),
        "a v1.99 frame on a v1.0 connection must reject, got {rejected:?}"
    );
    assert_eq!(
        rejected
            .first()
            .map(|first| first.envelope.sender.connection_id),
        Some(Some(negotiated.connection_id)),
        "a post-auth protocol reject carries the current connection"
    );
    let agreed = handle
        .handle_frame(submit_frame(), negotiated, &transport)
        .await;
    assert!(
        agreed
            .first()
            .is_none_or(|first| !matches!(&first.payload, WirePayload::Reject(_))),
        "the negotiated version itself must pass the gate, got {agreed:?}"
    );
    let mut early = pairing_frame("laptop");
    early.envelope.protocol = ProtocolVersion { major: 1, minor: 1 };
    let pre = handle
        .handle_frame(early, unpaired_input(), &transport)
        .await;
    assert!(
        pre.first()
            .is_some_and(|first| matches!(&first.payload, WirePayload::Reject(_))),
        "pre-negotiation frames must speak exactly v1.0, got {pre:?}"
    );
    assert_eq!(
        pre.first().map(|first| first.envelope.sender.connection_id),
        Some(None),
        "a pre-auth protocol reject still hides the connection"
    );
}

/// The typed reject sender follows the auth boundary (IPC §5).
#[tokio::test]
async fn reject_sender_follows_the_auth_boundary() {
    let (handle, _dir) = open_handle("reject-sender").await.unwrap();
    let transport = fake_transport();
    let mut mismatch = submit_frame();
    mismatch.envelope.message_type = WireMessageType(String::from("HistoryRequest"));

    let live = paired_input("device-1");
    let post = handle
        .handle_frame(mismatch.clone(), live.clone(), &transport)
        .await;
    let first = post.first().unwrap();
    assert!(
        matches!(
            &first.payload,
            WirePayload::Reject(notice)
                if notice.kind == ene_api::v1::reject::RejectKind::UnsupportedMessage
        ),
        "a discriminator mismatch must reject, got {post:?}"
    );
    assert_eq!(
        first.envelope.sender.connection_id,
        Some(live.connection_id),
        "a post-auth message reject carries the current connection"
    );

    let pre = handle
        .handle_frame(mismatch, unpaired_input(), &transport)
        .await;
    let first = pre.first().unwrap();
    assert!(
        matches!(&first.payload, WirePayload::Reject(_)),
        "the same mismatch must reject pre-auth, got {pre:?}"
    );
    assert_eq!(
        first.envelope.sender.connection_id, None,
        "a pre-auth message reject still hides the connection"
    );
}

#[tokio::test]
async fn capability_mismatch_ends_with_a_disconnect_notice() {
    let (handle, _dir) = open_handle("caps-mismatch").await.unwrap();
    let frame = advertise_frame(ProtocolVersion { major: 9, minor: 0 });
    let transport = fake_transport();
    let responses = handle
        .handle_frame(frame, live_input("client-a"), &transport)
        .await;
    assert_eq!(responses.len(), 1, "mismatch answers exactly one frame");
    let first = responses.first().unwrap();
    assert!(
        matches!(&first.payload, WirePayload::DisconnectNotice(_)),
        "a major mismatch disconnects"
    );
}

#[tokio::test]
async fn capability_match_negotiates_and_challenges_without_attaching() {
    use ene_companion::CompanionRepository as _;

    let (handle, _dir) = open_handle("caps-ok").await.unwrap();
    let live = live_input("client-a");
    let frame = advertise_frame(ProtocolVersion::V1);
    let expected_reply = frame.envelope.message_id;
    let transport = fake_transport();
    let responses = handle.handle_frame(frame, live.clone(), &transport).await;
    assert_eq!(
        responses.len(),
        2,
        "negotiation answers terms plus the auth challenge"
    );
    let first = responses.first().unwrap();
    assert!(
        matches!(&first.payload, WirePayload::NegotiatedConnection(_)),
        "a major match negotiates, got {:?}",
        first.payload
    );
    assert_eq!(
        first.envelope.correlation.reply_to,
        Some(expected_reply),
        "the reply links back to the request"
    );
    let second = responses.get(1).unwrap();
    let WirePayload::AuthChallenge(challenge) = &second.payload else {
        return;
    };
    assert!(
        !challenge.nonce.is_empty(),
        "negotiation opens authentication with a fresh nonce"
    );
    for response in &responses {
        assert_eq!(
            response.envelope.sender.connection_id, None,
            "pre-accept negotiation hides the connection id"
        );
    }
    let recorded = match handle.pending_nonces.lock() {
        Ok(map) => map.get(&conn_key(&live.connection_id)).cloned(),
        Err(_) => None,
    };
    assert_eq!(
        recorded.as_ref(),
        Some(&challenge.nonce),
        "the challenge nonce is pending for this connection"
    );
    let companion = handle.store.ensure_running_companion().await;
    let companion = companion.unwrap();
    let attribution = handle.store.load_attribution(companion.as_raw()).await;
    let current = attribution.unwrap().unwrap();
    assert!(
        current.active_client.is_none(),
        "capability negotiation never attaches presence"
    );
}

#[tokio::test]
async fn disconnect_without_presence_is_a_no_op() {
    use ene_companion::CompanionRepository as _;

    let (handle, _dir) = open_handle("disc-noop").await.unwrap();
    handle.note_disconnect("never-attached").await;
    let companion = handle.store.ensure_running_companion().await;
    let companion = companion.unwrap();
    let attribution = handle.store.load_attribution(companion.as_raw()).await;
    let current = attribution.unwrap().unwrap();
    assert_eq!(
        current.state,
        ene_presence::PresenceState::NoActive,
        "a disconnect with nothing attached changes nothing"
    );
}
