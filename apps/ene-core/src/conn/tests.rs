#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "test helpers outside #[test] functions need the fixture allowances clippy.toml grants only to test functions"
)]

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use super::{
    ChallengeOutcome, ConnectionPhase, ConnectionTable, InstallOutcome, LiveDecision,
    NonceAdmission, bind_singleton, socket_path,
};
use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
use ene_api::v1::handshake::NegotiatedConnection;
use ene_api::v1::refs::{ClientIncarnationId, ConnectionWireId, DeviceWireId, WireMessageType};

#[test]
fn socket_path_appends_the_socket_name() {
    let dir = std::path::Path::new("/tmp/ene-probe-data");
    assert_eq!(
        socket_path(dir),
        std::path::Path::new("/tmp/ene-probe-data/ene.sock"),
        "the socket lives inside the data directory"
    );
}

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
fn duplicate_cache_is_bounded_and_rolls_off() {
    use super::SEEN_MESSAGE_CAP;

    let table = Arc::new(ConnectionTable::new());
    let id = table.note_accept();
    let first = envelope(incarnation(3, 3));
    assert!(
        matches!(table.live_for(&id, &first), LiveDecision::Ready(_)),
        "the first delivery passes"
    );
    for _ in 0..SEEN_MESSAGE_CAP {
        let _ = table.live_for(&id, &envelope(incarnation(3, 3)));
    }
    assert!(
        matches!(table.live_for(&id, &first), LiveDecision::Ready(_)),
        "a rolled-off id processes again instead of growing memory without limit"
    );
    assert_eq!(
        table.live_for(&id, &first),
        LiveDecision::Duplicate,
        "but the replay is still a duplicate once seen again"
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
fn re_pairing_a_connection_keeps_the_first_device() {
    let table = Arc::new(ConnectionTable::new());
    let id = table.note_accept();
    assert!(table.note_paired(&id, "device-1"));
    assert!(
        !table.note_paired(&id, "device-2"),
        "a paired connection can never move to another device"
    );
    assert_eq!(
        table.phase_of(&id),
        Some(ConnectionPhase::Paired),
        "the refused re-pair leaves the phase alone"
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
fn challenge_requires_a_bound_device() {
    let table = ConnectionTable::new();
    let id = table.note_accept();
    assert_eq!(
        table.note_challenged(&id, None, terms(), String::from("nonce")),
        ChallengeOutcome::WrongPhase,
        "an unbound connection cannot challenge"
    );
    assert_eq!(table.phase_of(&id), Some(ConnectionPhase::Accepted));
}

#[test]
fn nonce_is_consumed_once_and_only_in_the_challenged_phase() {
    let table = ConnectionTable::new();
    let id = table.note_accept();
    let nonce = challenge(&table, &id, "device-1");
    assert_eq!(
        table.take_nonce(&id),
        NonceAdmission::Nonce(nonce),
        "the challenge nonce is consumed by the first proof"
    );
    assert_eq!(
        table.take_nonce(&id),
        NonceAdmission::Missing,
        "a second consumption in the challenged phase finds nothing"
    );
    let accepted = table.note_accept();
    assert_eq!(
        table.take_nonce(&accepted),
        NonceAdmission::WrongPhase,
        "an unchallenged connection has no nonce to consume"
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

#[test]
fn failed_authentication_closes_the_challenged_connection() {
    let table = ConnectionTable::new();
    let id = table.note_accept();
    challenge(&table, &id, "device-1");
    table.note_auth_failed(&id);
    assert_eq!(table.phase_of(&id), Some(ConnectionPhase::Closed));
    assert_eq!(
        table.install_authenticated(&id),
        InstallOutcome::WrongPhase,
        "a closed connection cannot install"
    );
}

#[test]
fn install_requires_the_challenged_phase() {
    let table = ConnectionTable::new();
    let accepted = table.note_accept();
    assert_eq!(
        table.install_authenticated(&accepted),
        InstallOutcome::WrongPhase,
        "an accepted connection can never become authenticated"
    );
    let paired = table.note_accept();
    assert!(table.note_paired(&paired, "device-1"));
    assert_eq!(
        table.install_authenticated(&paired),
        InstallOutcome::WrongPhase,
        "pairing alone is not authentication"
    );
}

#[test]
fn lingering_superseded_close_never_triggers_the_fallback() {
    let table = Arc::new(ConnectionTable::new());
    let first = table.note_accept();
    let second = table.note_accept();
    let device = "device-1";
    for id in [first, second] {
        challenge(&table, &id, device);
        let _ = table.take_nonce(&id);
        assert!(matches!(
            table.install_authenticated(&id),
            InstallOutcome::Installed { .. }
        ));
    }
    let fallbacks = Mutex::new(Vec::new());
    let closed = table.note_closed(&first, |device| {
        fallbacks
            .lock()
            .expect("fallback list")
            .push(device.to_string());
    });
    assert_eq!(closed, Some(String::from(device)));
    assert!(
        fallbacks.lock().expect("fallback list").is_empty(),
        "closing a superseded connection never runs the presence fallback"
    );
    assert!(
        table.current_authenticated(device),
        "the superseded close never clears the newer current"
    );
    let closed = table.note_closed(&second, |device| {
        fallbacks
            .lock()
            .expect("fallback list")
            .push(device.to_string());
    });
    assert_eq!(closed, Some(String::from(device)));
    assert_eq!(
        fallbacks.lock().expect("fallback list").as_slice(),
        [String::from(device)],
        "closing the current connection runs the fallback exactly once"
    );
    assert!(
        !table.current_authenticated(device),
        "the current close clears the current slot"
    );
    let third = table.note_accept();
    challenge(&table, &third, device);
    let _ = table.take_nonce(&third);
    assert!(matches!(
        table.install_authenticated(&third),
        InstallOutcome::Installed { .. }
    ));
    assert!(
        table.current_authenticated(device),
        "a fresh authentication installs after the current close"
    );
}

#[test]
fn closing_the_current_connection_leaves_survivors_unauthed() {
    let table = Arc::new(ConnectionTable::new());
    let first = table.note_accept();
    let second = table.note_accept();
    let device = device_one();
    let device_wire = device.0.as_hyphenated().to_string();
    challenge(&table, &first, &device_wire);
    let _ = table.take_nonce(&first);
    assert!(matches!(
        table.install_authenticated(&first),
        InstallOutcome::Installed { .. }
    ));
    challenge(&table, &second, &device_wire);

    let closed = table.note_closed(&first, |_| {});
    assert_eq!(closed, Some(device_wire.clone()));
    let survivor = table.live_for(&second, &paired_envelope(incarnation(6, 6), device));
    assert!(
        matches!(survivor, LiveDecision::Ready(live) if !live.authed),
        "the survivor stays unauthed until it completes a fresh challenge"
    );
    assert!(!table.current_authenticated(&device_wire));
}

#[tokio::test]
async fn stale_regular_file_blocks_bind_until_removed() {
    let dir = tempfile::tempdir().expect("test scratch directory must be creatable");
    let socket = socket_path(dir.path());
    assert!(
        std::fs::write(&socket, b"stale").is_ok(),
        "the stale probe file must be writable"
    );
    let bound = tokio::net::UnixListener::bind(&socket);
    // A stale regular file blocks the bind: this documents why `run`
    // removes the path first (the test only proves the premise, the
    // removal itself runs inside `run`).
    assert!(
        bound.is_err(),
        "a stale regular file must block a fresh bind: {bound:?}"
    );
    assert!(
        std::fs::remove_file(&socket).is_ok(),
        "stale removal must clear the path"
    );
    let rebound = tokio::net::UnixListener::bind(&socket);
    assert!(rebound.is_ok(), "the cleared path must bind: {rebound:?}");
}

#[tokio::test]
async fn live_listener_blocks_a_second_bind() {
    let dir = tempfile::tempdir().expect("test scratch directory must be creatable");
    let socket = socket_path(dir.path());
    let first = bind_singleton(&socket).await;
    let live = first.unwrap();
    let second = bind_singleton(&socket).await;
    assert!(
        second.is_err(),
        "a live listener must block a second bind: {second:?}"
    );
    drop(live);
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
    ) -> Option<Vec<u8>> {
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
        ene_plugin_ipc::encode_frame(&WireFrame { envelope, payload }).ok()
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
    let Some(pairing) = pairing else {
        worker.abort();
        return;
    };
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
    let Some(capability) = capability else {
        worker.abort();
        return;
    };
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

/// IPC §5 / §9.2: a paired connection drops a claim-less frame without a
/// reply.
#[tokio::test]
async fn paired_connection_drops_a_frame_without_a_device_claim() {
    use std::time::Duration;

    use ene_api::v1::handshake::CapabilityAdvertise;
    use ene_api::v1::payload::WirePayload;
    use ene_inference::fake::FakeProviderTransport;
    use ene_plugin_ipc::WireFrame;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use crate::test_support::memory_handle_with;

    let (handle, _dir) = memory_handle_with("claim-drop", |_| {}).await.unwrap();
    let table = Arc::new(ConnectionTable::new());
    let id = table.note_accept();
    let device = device_one();
    let device_wire = device.0.as_hyphenated().to_string();
    assert!(
        matches!(
            table.live_for(&id, &envelope(incarnation(5, 6))),
            LiveDecision::Ready(_)
        ),
        "the pre-pairing frame pins the connection"
    );
    assert!(table.note_paired(&id, &device_wire));

    let pair = tokio::net::UnixStream::pair();
    let (mut client, server) = pair.unwrap();
    let (_stop, shutdown) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(super::serve_connection(
        server,
        id,
        Arc::new(handle),
        Arc::new(FakeProviderTransport::new(String::new(), None)),
        Arc::clone(&table),
        shutdown.clone(),
    ));
    let frame = WireFrame {
        envelope: new_outgoing_envelope(
            ProtocolVersion::V1,
            WireSender {
                device_id: None,
                incarnation_id: incarnation(5, 6),
                connection_id: None,
            },
            WireMessageType(String::from("CapabilityAdvertise")),
        ),
        payload: WirePayload::CapabilityAdvertise(CapabilityAdvertise {
            supported_protocol: vec![ProtocolVersion::V1],
            platform: String::from("test"),
        }),
    };
    let encoded = ene_plugin_ipc::encode_frame(&frame).expect("test frame must encode");
    assert!(
        client.write_all(&encoded).await.is_ok(),
        "the claim-less frame must send"
    );
    // Terminal: EOF, never a response frame.
    let mut byte = [0_u8; 1];
    let closed = tokio::time::timeout(Duration::from_secs(5), client.read(&mut byte)).await;
    assert!(
        matches!(closed, Ok(Ok(0))),
        "a paired connection must close without answering a claim-less frame, got {closed:?}"
    );
    assert!(worker.await.is_ok(), "the connection task must finish");
}

/// Builds one authenticated domain frame for the socket-loop tests: the
/// envelope claims the bound device and echoes the connection id.
fn domain_frame(
    incarnation: ClientIncarnationId,
    device: DeviceWireId,
    connection: ConnectionWireId,
    payload: ene_api::v1::payload::WirePayload,
) -> super::WireFrame {
    let mut envelope = paired_envelope(incarnation, device);
    envelope.sender.connection_id = Some(connection);
    envelope.message_type = WireMessageType(payload.message_type().to_string());
    super::WireFrame { envelope, payload }
}

async fn read_domain(
    stream: &mut tokio::net::UnixStream,
) -> Option<ene_api::v1::payload::WirePayload> {
    use std::time::Duration;

    use tokio::io::AsyncReadExt as _;

    let timed = tokio::time::timeout(Duration::from_secs(5), async {
        let mut prefix = [0_u8; 4];
        stream.read_exact(&mut prefix).await.ok()?;
        let claimed = u32::from_be_bytes(prefix) as usize;
        if claimed > 256 * 1024 {
            return None;
        }
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

async fn write_domain(stream: &mut tokio::net::UnixStream, frame: &super::WireFrame) -> bool {
    use tokio::io::AsyncWriteExt as _;

    let Ok(bytes) = ene_plugin_ipc::encode_frame(frame) else {
        return false;
    };
    stream.write_all(&bytes).await.is_ok()
}

/// Drives the fixture companion to `Present` for `device`.
async fn attach_present(handle: &crate::serve::HostHandle, device: &str) {
    use ene_companion::CompanionRepository as _;
    use ene_presence::PresenceRepository as _;

    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must ensure");
    let current = handle
        .store
        .load_attribution(companion.as_raw())
        .await
        .expect("the attribution must load")
        .expect("the attribution must exist");
    let outcome = handle
        .attach_presence(
            &crate::test_support::live_input(device),
            device,
            true,
            ene_presence::PresenceState::NoActive,
            current.generation,
        )
        .await;
    assert!(
        matches!(outcome, crate::dialogue::AttachOutcome::Attached(_)),
        "the fresh presence must attach"
    );
}

/// Commits one reply with its undelivered registration.
async fn append_registered(
    handle: &crate::serve::HostHandle,
    text: &str,
) -> ene_companion::UndeliveredRef {
    use ene_companion::{CompanionRepository as _, HistoryRepository as _};
    use ene_presence::PresenceRepository as _;

    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must ensure");
    let current = handle
        .store
        .load_attribution(companion.as_raw())
        .await
        .expect("the attribution must load")
        .expect("the attribution must exist");
    let (outcome, registered) = handle
        .store
        .append_reply_with_undelivered(
            ene_companion::AppendHistoryCommand {
                companion,
                round: ene_primitive::RawId::new(),
                role: ene_companion::HistoryRole::Companion,
                text: text.to_string(),
                lang: String::from("en"),
                at: ene_primitive::WallClockWithTz::now(),
                expected_generation: current.generation,
                expected_consent: None,
                expected_credential_set: None,
                expected_owner_message: None,
                command_id: None,
                round_wire: Some(uuid::Uuid::new_v4().as_hyphenated().to_string()),
                round_intent: None,
                incarnation: None,
                local_id: None,
            },
            true,
            None,
        )
        .await
        .expect("the append must commit");
    assert!(matches!(
        outcome,
        ene_companion::HistoryAppendOutcome::CommittedAs { .. }
    ));
    registered.expect("the registration must ride the commit")
}

/// One subscription drain request under the connection's identity.
fn undelivered_request(
    incarnation: ClientIncarnationId,
    device: DeviceWireId,
    connection: ConnectionWireId,
    limit: Option<u32>,
) -> super::WireFrame {
    domain_frame(
        incarnation,
        device,
        connection,
        ene_api::v1::payload::WirePayload::UndeliveredRequest(
            ene_api::v1::undelivered::UndeliveredRequest {
                companion: None,
                cursor: None,
                limit,
                redisplay: false,
            },
        ),
    )
}

fn summary_of(
    payload: ene_api::v1::payload::WirePayload,
) -> ene_api::v1::undelivered::UndeliveredSummary {
    match payload {
        ene_api::v1::payload::WirePayload::UndeliveredResponse(
            ene_api::v1::undelivered::UndeliveredResponse::Summary(summary),
        ) => summary,
        other => panic!("expected a pushed summary, got {other:?}"),
    }
}

/// S5 subscription: a new durable arrival is pushed to an idle connection
/// without the Client sending another request.
#[tokio::test]
async fn subscription_pushes_a_new_arrival_without_a_request() {
    use crate::test_support::{authenticate, memory_handle_with};

    let (handle, _dir) = memory_handle_with("push-arrival", |_| {}).await.unwrap();
    let handle = Arc::new(handle);
    let device = device_one();
    let device_wire = device.0.as_hyphenated().to_string();
    attach_present(&handle, &device_wire).await;

    let table = Arc::new(ConnectionTable::new());
    let id = table.note_accept();
    authenticate(&table, &id, &device_wire);

    let pair = tokio::net::UnixStream::pair().unwrap();
    let (mut client, server) = pair;
    let (_stop, shutdown) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(super::serve_connection(
        server,
        id,
        Arc::clone(&handle),
        Arc::new(ene_inference::fake::FakeProviderTransport::new(
            String::new(),
            None,
        )),
        Arc::clone(&table),
        shutdown.clone(),
    ));

    // Drain the (empty) backlog: this is the only request the Client sends.
    assert!(
        write_domain(
            &mut client,
            &undelivered_request(incarnation(5, 6), device, id, None)
        )
        .await
    );
    let _drain = summary_of(
        read_domain(&mut client)
            .await
            .expect("the drain must answer"),
    );

    // A new durable fact commits after the drain; no request follows.
    append_registered(&handle, "pushed arrival").await;

    let pushed = summary_of(
        read_domain(&mut client)
            .await
            .expect("the arrival must push"),
    );
    assert_eq!(pushed.items.len(), 1, "exactly the new arrival is pushed");
    assert_eq!(pushed.items[0].excerpt, "pushed arrival");
    assert!(!pushed.receipt.0.is_empty(), "the push carries its receipt");
    worker.abort();
}

/// S5 receipt timeout: without an ACK and without another Client request, the
/// subscription releases the expired receipt and advances to the next page.
#[tokio::test]
async fn subscription_advances_on_receipt_timeout_without_a_request() {
    use crate::test_support::{authenticate, memory_handle_with};
    use ene_companion::{CompanionRepository as _, UndeliveredRepository as _};

    let (handle, _dir) = memory_handle_with("push-timeout", |_| {}).await.unwrap();
    let handle = Arc::new(handle);
    let device = device_one();
    let device_wire = device.0.as_hyphenated().to_string();
    attach_present(&handle, &device_wire).await;
    append_registered(&handle, "timeout row one").await;
    append_registered(&handle, "timeout row two").await;
    handle.set_receipt_ttl_for_test(std::time::Duration::from_millis(150));

    let table = Arc::new(ConnectionTable::new());
    let id = table.note_accept();
    authenticate(&table, &id, &device_wire);

    let pair = tokio::net::UnixStream::pair().unwrap();
    let (mut client, server) = pair;
    let (_stop, shutdown) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(super::serve_connection(
        server,
        id,
        Arc::clone(&handle),
        Arc::new(ene_inference::fake::FakeProviderTransport::new(
            String::new(),
            None,
        )),
        Arc::clone(&table),
        shutdown.clone(),
    ));

    // One request with a page bound of one: the pass continues with a
    // cursor, and no ACK will follow.
    assert!(
        write_domain(
            &mut client,
            &undelivered_request(incarnation(5, 7), device, id, Some(1))
        )
        .await
    );
    let first = summary_of(
        read_domain(&mut client)
            .await
            .expect("the first page must answer"),
    );
    assert_eq!(first.items.len(), 1);
    assert!(first.next_cursor.is_some(), "the pass has a continuation");

    // The receipt expires; the subscription pushes the next page on its own.
    let second = summary_of(read_domain(&mut client).await.expect("expiry must advance"));
    assert_ne!(
        second.receipt, first.receipt,
        "a new receipt covers the page"
    );
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].excerpt, "timeout row two");

    // Nothing was falsely presented: both rows stay unpresented (Unknown).
    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must ensure");
    let page = handle
        .store
        .list_unpresented(companion, None, 50)
        .await
        .expect("the listing must read");
    assert_eq!(page.entries.len(), 2);
    assert!(
        page.entries
            .iter()
            .all(|entry| { entry.status == ene_companion::ReportStatus::PresentationUnknown })
    );
    worker.abort();
}

/// A byte stream whose read side stays open while writes fail on demand.
///
/// The connection loop must keep draining inbound frames after a failed
/// push write; only real transport EOF ends it.
struct FlakyWrite<S> {
    inner: S,
    fail_writes: Arc<AtomicBool>,
}

impl<S: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for FlakyWrite<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for FlakyWrite<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.fail_writes.load(Ordering::SeqCst) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "test write failure",
            )));
        }
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.fail_writes.load(Ordering::SeqCst) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "test write failure",
            )));
        }
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.fail_writes.load(Ordering::SeqCst) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "test write failure",
            )));
        }
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// S5 subscription: after a push write failure the connection keeps its
/// reader, and the receipt deadline still releases the expired receipt. A
/// released deadline never keeps firing (no timer busy-loop), and inbound
/// frames stay processable.
#[tokio::test]
async fn receipt_expiry_advances_after_a_push_write_failure() {
    use std::time::Duration;

    use crate::test_support::{authenticate, memory_handle_with};

    let (handle, _dir) = memory_handle_with("push-expiry-blocked", |_| {})
        .await
        .unwrap();
    let handle = Arc::new(handle);
    let device = device_one();
    let device_wire = device.0.as_hyphenated().to_string();
    attach_present(&handle, &device_wire).await;
    // Short TTL: deterministic expiry without the production 30 s.
    handle.set_receipt_ttl_for_test(Duration::from_millis(120));

    let table = Arc::new(ConnectionTable::new());
    let id = table.note_accept();
    authenticate(&table, &id, &device_wire);

    let fail_writes = Arc::new(AtomicBool::new(false));
    let pair = tokio::net::UnixStream::pair().unwrap();
    let (mut client, server) = pair;
    let (_stop, shutdown) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(super::serve_connection(
        FlakyWrite {
            inner: server,
            fail_writes: Arc::clone(&fail_writes),
        },
        id,
        Arc::clone(&handle),
        Arc::new(ene_inference::fake::FakeProviderTransport::new(
            String::new(),
            None,
        )),
        Arc::clone(&table),
        shutdown.clone(),
    ));

    // Drain the empty backlog over a healthy write side.
    assert!(
        write_domain(
            &mut client,
            &undelivered_request(incarnation(5, 9), device, id, Some(1))
        )
        .await
    );
    let drained = summary_of(
        read_domain(&mut client)
            .await
            .expect("the drain must answer"),
    );
    assert!(drained.items.is_empty());

    // A new fact commits while writes fail: the unsolicited push installs
    // its receipt durably, then its write fails and disables further pushes.
    fail_writes.store(true, Ordering::SeqCst);
    append_registered(&handle, "expiry row").await;
    // No unsolicited frame reaches the client: the push write failed.
    let pushed = tokio::time::timeout(Duration::from_millis(250), read_domain(&mut client)).await;
    assert!(
        pushed.is_err(),
        "the push write must fail, got {:?}",
        pushed.ok().flatten()
    );
    // The push carried its item (the receipt was installed durably) before
    // the failed write; nothing else removes the receipt while pushes are
    // blocked.
    assert!(
        handle.presentation_counts_for_test(&id).carried > 0,
        "the push must have carried its item"
    );

    // The deadline releases the expired receipt even though pushes are
    // blocked: the observable state transition is the release itself.
    let release_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while handle.receipt_deadline_for(&id).is_some() {
        assert!(
            tokio::time::Instant::now() < release_deadline,
            "the expired receipt must be released"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // No spin: once released, the deadline does not keep firing.
    let released = handle.receipt_expiry_runs_for_test();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        handle.receipt_expiry_runs_for_test(),
        released,
        "a released receipt must not keep the timer firing"
    );

    // The reader is alive and writes recover: an explicit request still
    // answers and re-presents the durable row.
    fail_writes.store(false, Ordering::SeqCst);
    assert!(
        write_domain(
            &mut client,
            &undelivered_request(incarnation(5, 9), device, id, None)
        )
        .await
    );
    let recovered = summary_of(
        read_domain(&mut client)
            .await
            .expect("the request must answer"),
    );
    assert_eq!(recovered.items.len(), 1);
    assert_eq!(recovered.items[0].excerpt, "expiry row");
    let _ = &handle.store;
    worker.abort();
}
