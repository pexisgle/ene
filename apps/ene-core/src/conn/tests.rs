use super::{ConnectionTable, LiveDecision, bind_singleton, socket_path};
use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
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

#[test]
fn connection_table_pins_the_first_incarnation() {
    let table = ConnectionTable::new();
    let id = table.note_accept();
    let first = table.live_for(&id, &envelope(incarnation(1, 2)));
    assert!(
        matches!(first, LiveDecision::Ready(ref live) if live.connection_known
                && live.paired_device.is_none()
                && !live.authed
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
    let table = ConnectionTable::new();
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

    let table = ConnectionTable::new();
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
    let table = ConnectionTable::new();
    let id = table.note_accept();
    let before = table.live_for(&id, &envelope(incarnation(7, 7)));
    assert!(
        matches!(before, LiveDecision::Ready(live) if live.paired_device.is_none()),
        "a fresh connection pairs nothing"
    );
    let device = device_one();
    let device_wire = device.0.as_hyphenated().to_string();
    table.note_paired(&id, &device_wire);
    let after = table.live_for(&id, &paired_envelope(incarnation(7, 7), device));
    assert!(
        matches!(
            after,
            LiveDecision::Ready(live)
                if live.paired_device == Some(device_wire.clone()) && !live.authed
        ),
        "a paired-but-never-challenged connection stays unauthed"
    );
    let (closed, still_live) = table.note_closed(&id);
    assert_eq!(
        closed,
        Some(device_wire),
        "close reports the paired device for disconnect"
    );
    assert!(!still_live, "the last close for a device ends its liveness");
    let (again, again_live) = table.note_closed(&id);
    assert_eq!(again, None, "forgetting is idempotent");
    assert!(!again_live, "a forgotten key holds nothing live");
    let unknown = ConnectionWireId(uuid::Uuid::new_v4());
    assert_eq!(
        table.live_for(&unknown, &envelope(incarnation(7, 7))),
        LiveDecision::Invalid,
        "an unknown connection closes"
    );
}

#[test]
fn auth_marks_current_and_supersedes_the_previous_connection() {
    let table = ConnectionTable::new();
    let first = table.note_accept();
    let second = table.note_accept();
    let device = device_one();
    let device_wire = device.0.as_hyphenated().to_string();
    for id in [first, second] {
        let pinned = table.live_for(&id, &envelope(incarnation(9, 9)));
        assert!(
            matches!(pinned, LiveDecision::Ready(_)),
            "both connections must pin before pairing"
        );
        table.note_paired(&id, &device_wire);
    }
    table.note_authed(&first);
    let current = table.live_for(&first, &paired_envelope(incarnation(9, 9), device));
    assert!(
        matches!(current, LiveDecision::Ready(live) if live.authed),
        "the freshly authenticated connection reports authed"
    );
    let waiting = table.live_for(&second, &paired_envelope(incarnation(9, 9), device));
    assert!(
        matches!(waiting, LiveDecision::Ready(live) if !live.authed),
        "the paired-but-never-challenged connection stays unauthed"
    );
    table.note_authed(&second);
    let stale = table.live_for(&first, &paired_envelope(incarnation(9, 9), device));
    assert!(
        matches!(stale, LiveDecision::Ready(live) if !live.authed),
        "a newer authentication supersedes: the old connection goes stale implicitly"
    );
    let now_current = table.live_for(&second, &paired_envelope(incarnation(9, 9), device));
    assert!(
        matches!(now_current, LiveDecision::Ready(live) if live.authed),
        "the newest authentication is the current one"
    );
}

#[test]
fn closing_an_older_connection_keeps_the_newer_currency() {
    let table = ConnectionTable::new();
    let first = table.note_accept();
    let second = table.note_accept();
    let device = device_one();
    let device_wire = device.0.as_hyphenated().to_string();
    for id in [first, second] {
        let pinned = table.live_for(&id, &envelope(incarnation(4, 4)));
        assert!(
            matches!(pinned, LiveDecision::Ready(_)),
            "both connections must pin before pairing"
        );
        table.note_paired(&id, &device_wire);
    }
    table.note_authed(&first);
    table.note_authed(&second);
    let (closed, still_live) = table.note_closed(&first);
    assert_eq!(
        closed,
        Some(device_wire.clone()),
        "the older close reports its device"
    );
    assert!(still_live, "the surviving connection keeps the device live");
    let survivor = table.live_for(&second, &paired_envelope(incarnation(4, 4), device));
    assert!(
        matches!(survivor, LiveDecision::Ready(live) if live.authed),
        "closing the superseded connection never clears the newer currency"
    );
}

#[test]
fn closing_the_current_connection_clears_currency_without_reviving() {
    let table = ConnectionTable::new();
    let first = table.note_accept();
    let second = table.note_accept();
    let device = device_one();
    let device_wire = device.0.as_hyphenated().to_string();
    for id in [first, second] {
        let pinned = table.live_for(&id, &envelope(incarnation(6, 6)));
        assert!(
            matches!(pinned, LiveDecision::Ready(_)),
            "both connections must pin before pairing"
        );
        table.note_paired(&id, &device_wire);
    }
    table.note_authed(&first);
    let (closed, still_live) = table.note_closed(&first);
    assert_eq!(
        closed,
        Some(device_wire),
        "the current close reports its device"
    );
    assert!(still_live, "the surviving connection keeps the device live");
    let survivor = table.live_for(&second, &paired_envelope(incarnation(6, 6), device));
    assert!(
        matches!(survivor, LiveDecision::Ready(live) if !live.authed),
        "the survivor stays unauthed until it completes a fresh challenge"
    );
}

#[test]
fn auth_without_pairing_records_nothing() {
    let table = ConnectionTable::new();
    let id = table.note_accept();
    let pinned = table.live_for(&id, &envelope(incarnation(1, 1)));
    assert!(
        matches!(pinned, LiveDecision::Ready(_)),
        "the connection must pin first"
    );
    table.note_authed(&id);
    let live = table.live_for(&id, &envelope(incarnation(1, 1)));
    assert!(
        matches!(live, LiveDecision::Ready(live) if !live.authed),
        "an unpaired connection can never become authed"
    );
}

#[test]
fn device_liveness_counts_connections_not_pair_events() {
    let table = ConnectionTable::new();
    let first = table.note_accept();
    let second = table.note_accept();
    for id in [first, second] {
        let pinned = table.live_for(&id, &envelope(incarnation(7, 7)));
        assert!(
            matches!(pinned, LiveDecision::Ready(_)),
            "both connections must pin before pairing"
        );
    }
    table.note_paired(&first, "device-1");
    table.note_paired(&first, "device-1");
    table.note_paired(&second, "device-1");
    let (closed, still_live) = table.note_closed(&first);
    assert_eq!(
        closed,
        Some(String::from("device-1")),
        "the first close reports its device"
    );
    assert!(
        still_live,
        "one remaining live connection keeps the device live"
    );
    let (last, last_live) = table.note_closed(&second);
    assert_eq!(
        last,
        Some(String::from("device-1")),
        "the last close reports its device"
    );
    assert!(
        !last_live,
        "closing the last live connection ends device liveness"
    );
}

#[test]
fn re_pairing_a_connection_keeps_the_first_device() {
    let table = ConnectionTable::new();
    let id = table.note_accept();
    let pinned = table.live_for(&id, &envelope(incarnation(3, 3)));
    assert!(
        matches!(pinned, LiveDecision::Ready(_)),
        "the connection must pin before pairing"
    );
    table.note_paired(&id, "device-1");
    table.note_paired(&id, "device-2");
    let (closed, still_live) = table.note_closed(&id);
    assert_eq!(
        closed,
        Some(String::from("device-1")),
        "the first paired device is immutable"
    );
    assert!(
        !still_live,
        "the connection never counted as live for the second device"
    );
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
    use std::sync::Arc;
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
    let worker = tokio::spawn(super::serve_connection(
        server,
        id,
        Arc::new(handle),
        Arc::new(FakeProviderTransport::new(String::from("hi"), None)),
        Arc::clone(&table),
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
    use std::sync::Arc;
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
    table.note_paired(&id, &device_wire);

    let pair = tokio::net::UnixStream::pair();
    let (mut client, server) = pair.unwrap();
    let worker = tokio::spawn(super::serve_connection(
        server,
        id,
        Arc::new(handle),
        Arc::new(FakeProviderTransport::new(String::new(), None)),
        Arc::clone(&table),
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
