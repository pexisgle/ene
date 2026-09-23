#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "test helpers outside #[test] functions need the fixture allowances clippy.toml grants only to test functions"
)]

use std::sync::Arc;

use super::{
    ChallengeOutcome, ConnectionPhase, ConnectionTable, InstallOutcome, LiveDecision,
    NonceAdmission, bind_singleton, socket_path,
};
use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
use ene_api::v1::handshake::NegotiatedConnection;
use ene_api::v1::refs::{ClientIncarnationId, ConnectionWireId, DeviceWireId, WireMessageType};

fn envelope(incarnation: ClientIncarnationId) -> ene_api::v1::envelope::WireEnvelope {
    new_outgoing_envelope(
        ProtocolVersion::V1,
        WireSender {
            device_id: None,
            incarnation_id: incarnation,
            connection_id: None,
        },
        WireMessageType(String::from("CapabilityAdvertise")),
    )
}

/// Stable test device wire: hyphenated UUID text, matching claim rendering.
fn device_one() -> DeviceWireId {
    DeviceWireId(uuid::Uuid::from_u128(1))
}

/// Builds a post-pairing envelope whose sender claims `device`.
fn paired_envelope(
    incarnation: ClientIncarnationId,
    device: DeviceWireId,
) -> ene_api::v1::envelope::WireEnvelope {
    new_outgoing_envelope(
        ProtocolVersion::V1,
        WireSender {
            device_id: Some(device),
            incarnation_id: incarnation,
            connection_id: None,
        },
        WireMessageType(String::from("CapabilityAdvertise")),
    )
}

fn incarnation(counter: u64, random: u64) -> ClientIncarnationId {
    ClientIncarnationId { counter, random }
}

fn terms() -> NegotiatedConnection {
    NegotiatedConnection {
        version: ProtocolVersion::V1,
    }
}

/// Drives `id` to the challenged phase on `device`.
fn challenge(table: &ConnectionTable, id: &ConnectionWireId, device: &str) -> String {
    assert!(
        table.note_paired(id, device),
        "the connection must be accepted and unpaired"
    );
    let nonce = format!("nonce-{}-{}", device, id.0);
    assert_eq!(
        table.note_challenged(id, None, terms(), nonce.clone()),
        ChallengeOutcome::Challenged,
        "the paired connection must accept one challenge"
    );
    nonce
}

#[test]
fn connection_table_pins_the_first_incarnation() {
    let table = Arc::new(ConnectionTable::new());
    let id = table.note_accept();
    let first = table.live_for(&id, &envelope(incarnation(1, 2)));
    assert!(
        matches!(first, LiveDecision::Ready(ref live) if live.connection_known
                && live.paired_device.is_none()
                && !live.authed
                && live.phase == ConnectionPhase::Accepted
                && live.connection_id == id),
        "the first frame pins and yields table-bound unknown-but-unpaired premises"
    );
    let same = table.live_for(&id, &envelope(incarnation(1, 2)));
    assert!(
        matches!(same, LiveDecision::Ready(live) if live.connection_id == id),
        "the pinned incarnation keeps yielding the same table id"
    );
    let other = table.live_for(&id, &envelope(incarnation(1, 3)));
    assert_eq!(
        other,
        LiveDecision::Invalid,
        "an incarnation mismatch closes so the caller drops"
    );
}

#[test]
fn connection_table_suppresses_redelivered_message_ids() {
    let table = Arc::new(ConnectionTable::new());
    let id = table.note_accept();
    let frame = envelope(incarnation(1, 2));
    assert!(
        matches!(table.live_for(&id, &frame), LiveDecision::Ready(_)),
        "the first delivery passes"
    );
    assert_eq!(
        table.live_for(&id, &frame),
        LiveDecision::Duplicate,
        "a redelivered message id drops before any mapping"
    );
    assert!(
        matches!(
            table.live_for(&id, &envelope(incarnation(1, 2))),
            LiveDecision::Ready(_)
        ),
        "fresh message ids still pass"
    );
}

#[test]
fn connection_table_marks_paired_and_forgets_on_close() {
    let table = Arc::new(ConnectionTable::new());
    let id = table.note_accept();
    let before = table.live_for(&id, &envelope(incarnation(7, 7)));
    assert!(
        matches!(before, LiveDecision::Ready(live) if live.paired_device.is_none()),
        "a fresh connection pairs nothing"
    );
    let device = device_one();
    let device_wire = device.0.as_hyphenated().to_string();
    assert!(
        table.note_paired(&id, &device_wire),
        "the accepted connection accepts one pairing"
    );
    let after = table.live_for(&id, &paired_envelope(incarnation(7, 7), device));
    assert!(
        matches!(
            after,
            LiveDecision::Ready(live)
                if live.paired_device == Some(device_wire.clone())
                    && !live.authed
                    && live.phase == ConnectionPhase::Paired
        ),
        "a paired-but-never-challenged connection stays unauthed"
    );
    let closed = table.note_closed(&id, |_| {});
    assert_eq!(
        closed,
        Some(device_wire),
        "close reports the paired device for disconnect"
    );
    let again = table.note_closed(&id, |_| {});
    assert_eq!(again, None, "forgetting is idempotent");
    let unknown = ConnectionWireId(uuid::Uuid::new_v4());
    assert_eq!(
        table.live_for(&unknown, &envelope(incarnation(7, 7))),
        LiveDecision::Invalid,
        "an unknown connection closes"
    );
}

#[test]
fn challenge_is_accepted_exactly_once_and_keeps_terms() {
    let table = ConnectionTable::new();
    let id = table.note_accept();
    let nonce = challenge(&table, &id, "device-1");
    assert_eq!(table.phase_of(&id), Some(ConnectionPhase::Challenged));
    assert_eq!(table.challenge_nonce_of(&id), Some(nonce.clone()));
    assert_eq!(table.negotiated_of(&id), Some(terms()));

    // A repeat capability frame is refused and changes neither the nonce nor
    // the terms (IPC §9.3), and the unbound reconnect bind cannot repair it.
    assert_eq!(
        table.note_challenged(&id, None, terms(), String::from("other-nonce")),
        ChallengeOutcome::WrongPhase
    );
    assert_eq!(
        table.note_challenged(&id, None, terms(), String::from("other-nonce")),
        ChallengeOutcome::WrongPhase
    );
    assert_eq!(table.challenge_nonce_of(&id), Some(nonce));
    assert_eq!(table.negotiated_of(&id), Some(terms()));

    // A reconnect bind on an accepted connection is the one phase operation.
    let reconnect = table.note_accept();
    assert_eq!(
        table.note_challenged(&reconnect, Some("device-2"), terms(), String::from("n2")),
        ChallengeOutcome::Challenged
    );
    assert_eq!(
        table.note_challenged(&reconnect, Some("device-2"), terms(), String::from("n3")),
        ChallengeOutcome::WrongPhase,
        "the reconnect challenge is also accepted exactly once"
    );
}

#[test]
fn authenticated_install_supersedes_and_is_irreversible() {
    let table = Arc::new(ConnectionTable::new());
    let first = table.note_accept();
    let second = table.note_accept();
    let device = device_one();
    let device_wire = device.0.as_hyphenated().to_string();
    let first_nonce = challenge(&table, &first, &device_wire);
    challenge(&table, &second, &device_wire);
    assert_eq!(table.take_nonce(&first), NonceAdmission::Nonce(first_nonce));

    assert!(matches!(
        table.install_authenticated(&first),
        InstallOutcome::Installed { .. }
    ));
    assert!(table.current_authenticated(&device_wire));
    let current = table.live_for(&first, &paired_envelope(incarnation(9, 9), device));
    assert!(
        matches!(current, LiveDecision::Ready(live) if live.authed),
        "the freshly authenticated connection reports authed"
    );

    assert!(matches!(
        table.install_authenticated(&second),
        InstallOutcome::Installed { superseded: Some(previous) } if previous == first
    ));
    let stale = table.live_for(&first, &paired_envelope(incarnation(9, 9), device));
    assert!(
        matches!(stale, LiveDecision::Ready(live) if !live.authed && live.phase == ConnectionPhase::Superseded),
        "the newer authentication supersedes the old connection irreversibly"
    );
    let now_current = table.live_for(&second, &paired_envelope(incarnation(9, 9), device));
    assert!(
        matches!(now_current, LiveDecision::Ready(live) if live.authed),
        "the newest authentication is the current one"
    );

    // A superseded connection can never race back: retry, re-challenge, and
    // failure recording all leave it superseded and never touch the current.
    assert_eq!(
        table.install_authenticated(&first),
        InstallOutcome::Superseded
    );
    assert_eq!(
        table.note_challenged(&first, Some(&device_wire), terms(), String::from("n")),
        ChallengeOutcome::Superseded
    );
    assert_eq!(table.take_nonce(&first), NonceAdmission::Superseded);
    table.note_auth_failed(&first);
    assert!(
        !table.note_paired(&first, &device_wire),
        "a superseded connection cannot re-pair"
    );
    assert_eq!(
        table.phase_of(&first),
        Some(ConnectionPhase::Superseded),
        "supersede is terminal"
    );
    assert!(table.current_authenticated(&device_wire));
    assert_eq!(
        table.install_authenticated(&first),
        InstallOutcome::Superseded
    );
}

#[tokio::test]
async fn stale_socket_file_rebinds_after_close() {
    let dir = tempfile::tempdir().expect("test scratch directory must be creatable");
    let socket = socket_path(dir.path());
    let first = bind_singleton(&socket).await;
    let live = first.unwrap();
    drop(live);
    let rebound = bind_singleton(&socket).await;
    assert!(
        rebound.is_ok(),
        "a closed listener leaves a stale path that rebinds: {rebound:?}"
    );
    drop(rebound);
}

/// A redelivered frame must drop silently WITHOUT closing the
/// connection: transport at-least-once must never become domain twice,
/// and a duplicate is not a terminal violation.
#[tokio::test]
async fn redelivery_keeps_the_connection_serving() {
    use std::time::Duration;

    use ene_api::v1::handshake::{CapabilityAdvertise, PairingRequest};
    use ene_api::v1::payload::WirePayload;
    use ene_api::v1::refs::WireMessageId;
    use ene_inference::fake::FakeProviderTransport;
    use ene_plugin_ipc::WireFrame;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use crate::test_support::memory_handle_with;

    fn framed(
        payload: WirePayload,
        incarnation: ClientIncarnationId,
        message: WireMessageId,
    ) -> Vec<u8> {
        let message_type = payload.message_type().to_string();
        let mut envelope = new_outgoing_envelope(
            ProtocolVersion::V1,
            WireSender {
                device_id: None,
                incarnation_id: incarnation,
                connection_id: None,
            },
            WireMessageType(message_type),
        );
        envelope.message_id = message;
        ene_plugin_ipc::encode_frame(&WireFrame { envelope, payload }).expect("frame must encode")
    }

    async fn read_answer(stream: &mut tokio::net::UnixStream) -> Option<WirePayload> {
        let timed = tokio::time::timeout(Duration::from_secs(5), async {
            let mut prefix = [0_u8; 4];
            stream.read_exact(&mut prefix).await.ok()?;
            let claimed = u32::from_be_bytes(prefix) as usize;
            let mut body = vec![0_u8; claimed];
            stream.read_exact(&mut body).await.ok()?;
            let mut bytes = prefix.to_vec();
            bytes.extend_from_slice(&body);
            let (frame, _) = ene_plugin_ipc::decode_frame(&bytes).ok()?;
            Some(frame.payload)
        })
        .await;
        timed.ok().flatten()
    }

    let (handle, _dir) = memory_handle_with("dup-serving", |_| {}).await.unwrap();
    let table = Arc::new(ConnectionTable::new());
    let id = table.note_accept();
    let pair = tokio::net::UnixStream::pair();
    let (mut client, server) = pair.unwrap();
    let (_stop, shutdown) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(super::serve_connection(
        server,
        id,
        Arc::new(handle),
        Arc::new(FakeProviderTransport::new(String::from("hi"), None)),
        Arc::clone(&table),
        shutdown.clone(),
    ));
    let incarnation = incarnation(5, 6);
    let duplicate = WireMessageId(uuid::Uuid::new_v4());
    let pairing = framed(
        WirePayload::PairingRequest(PairingRequest {
            device_descriptor: String::from("dup-device"),
        }),
        incarnation,
        duplicate,
    );
    assert!(
        client.write_all(&pairing).await.is_ok(),
        "first delivery must send"
    );
    assert!(
        matches!(
            read_answer(&mut client).await,
            Some(WirePayload::PairingResult(_))
        ),
        "first delivery must answer"
    );
    assert!(
        client.write_all(&pairing).await.is_ok(),
        "redelivery must send"
    );
    let capability = framed(
        WirePayload::CapabilityAdvertise(CapabilityAdvertise {
            supported_protocol: vec![ProtocolVersion::V1],
            platform: String::from("test"),
        }),
        incarnation,
        WireMessageId(uuid::Uuid::new_v4()),
    );
    assert!(
        client.write_all(&capability).await.is_ok(),
        "post-duplicate send must send"
    );
    assert!(
        matches!(
            read_answer(&mut client).await,
            Some(WirePayload::DisconnectNotice(_))
        ),
        "the connection must still serve after a duplicate (unpaired capability closes with notice)"
    );
    drop(client);
    worker.abort();
}
