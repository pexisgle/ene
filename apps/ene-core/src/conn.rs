//! Unix socket listener: accept, same-user check, frame loop, close.
//!
//! The listener binds `ene.sock` inside the data directory and serves one
//! task per connection. Each task reads length-prefixed
//! [`ene_plugin_ipc::WireFrame`] values, runs them through
//! [`HostHandle::handle_frame`], and writes the responses back. A
//! [`DisconnectNotice`](ene_api::v1::handshake::DisconnectNotice) in the
//! responses is terminal: it is written, then the connection closes.
//!
//! Single instance: [`run`] binds through `bind_singleton`, which treats
//! `AddrInUse` as a possible live peer and probes before deciding to unlink a
//! stale path; a probe timeout fails safe toward live.
//!
//! Per-connection state lives in `ConnectionTable`, owned by this module:
//! [`run`] mints one [`ConnectionWireId`] per accepted connection, pins the
//! first incarnation it sees (a mismatch later drops the connection without a
//! reply, since the frame cannot be attributed to the pinned owner), and
//! records pairing/authentication only from [`HostHandle::handle_frame`]
//! answers. A newer authentication by the same device supersedes the older
//! connection, which goes stale implicitly. Each frame's [`LiveInput`]
//! premises come from this table, never from Client self-reports; the ingress
//! gate in [`HostHandle::handle_frame`] trusts exactly these conn-filled
//! premises, and the envelope connection id must equal the table id on every
//! post-capability frame (the id is revealed only in `AuthResult::Accepted`,
//! so echoing it is the auth binding). Socket close forgets the entry and
//! reports the paired device to
//! [`HostHandle::note_disconnect`](crate::serve::HostHandle::note_disconnect)
//! only when it was the device's last live connection: connection close and
//! presence loss are deliberately separate, because one device may hold
//! several live connections.
//!
//! Same-user proof without new dependencies: after binding, the listener reads
//! the socket file owner through [`MetadataExt::uid`](std::os::unix::fs::MetadataExt)
//! (created by this process inside the `0700` data directory, so its owner is
//! the Host user) and compares it against each peer credential uid from
//! [`tokio::net::UnixStream::peer_cred`]. A mismatch, or an unreadable peer
//! credential, closes the connection before any frame is read: an unprovable
//! peer is a trust violation, not a protocol peer, so it receives no bytes
//! (not even a denial, which would be an oracle). [`LiveInput::peer_uid_ok`]
//! still travels into [`HostHandle::handle_frame`] for the pairing decision,
//! as defense in depth for direct handle callers.
//!
//! Corrupt or oversize frames close the connection without a reply: the frame
//! cannot be attributed to a request, so there is nothing honest to answer.
//! An oversize response (only reachable through an unbounded timeline today)
//! likewise ends the connection; paging that path is deferred work.
//!
//! Windows has no listener yet: [`run`] returns
//! [`CoreError::UnsupportedPlatform`] there. The follow-up is a named-pipe
//! listener behind the same [`HostHandle::handle_frame`] seam, which keeps the
//! Windows build green by holding no Unix import outside `cfg(unix)`.
//!
//! [`LiveInput::peer_uid_ok`]: crate::serve::LiveInput::peer_uid_ok
//! [`LiveInput`]: crate::serve::LiveInput
//! [`MetadataExt::uid`](std::os::unix::fs::MetadataExt): <https://doc.rust-lang.org/std/os/unix/fs/trait.MetadataExt.html>

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use ene_inference::ProviderTransport;

use crate::serve::{CoreError, HostHandle};

const SOCKET_NAME: &str = "ene.sock";

/// Connect-probe timeout for the singleton check.
///
/// Short on purpose: a live listener accepts from its backlog immediately, so
/// anything slower is treated as live anyway (fail-safe: never unlink a
/// maybe-live path).
#[cfg(unix)]
const SINGLETON_PROBE_MILLIS: u64 = 200;

/// Resolves the listener socket path for a data directory.
///
/// Public so the sibling Client dialer and integration tests derive the same
/// path the Host binds: there is exactly one socket name and it lives here.
#[must_use]
pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SOCKET_NAME)
}

#[cfg(unix)]
use ene_api::v1::envelope::WireEnvelope;
#[cfg(unix)]
use ene_api::v1::handshake::{AuthResult, NegotiatedConnection, PairingResult};
#[cfg(unix)]
use ene_api::v1::payload::WirePayload;
#[cfg(unix)]
use ene_api::v1::refs::{ClientIncarnationId, ConnectionWireId, WireMessageId};
#[cfg(unix)]
use ene_plugin_ipc::{MAX_FRAME_BYTES, WireFrame, decode_frame, encode_frame};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};

#[cfg(unix)]
use crate::serve::LiveInput;

/// Bound on the per-connection transport duplicate-suppression cache, enough
/// for any realistic redelivery window on a local socket. Beyond it the
/// oldest ids roll off and a very late duplicate re-processes: message ids
/// are sender-minted UUIDs, so a repeat after roll-off is a true transport
/// duplicate, never a fresh send (fresh sends, including transport retries,
/// always mint new ids).
#[cfg(unix)]
const SEEN_MESSAGE_CAP: usize = 128;

/// Separates transport redelivery from terminal violations: a duplicate
/// drops silently with the connection kept, while an unknown connection,
/// incarnation mismatch, or device-claim mismatch drops the frame and closes
/// the connection. Collapsing both into `None` would turn legitimate
/// redelivery into connection loss, a distinct observable effect the §6.2
/// silent-drop contract forbids.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum LiveDecision {
    Ready(LiveInput),
    Duplicate,
    Invalid,
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ConnectionRecord {
    paired_device: Option<String>,
    incarnation: Option<ClientIncarnationId>,
    /// Set by [`ConnectionTable::note_authed`] and never cleared; currency
    /// after a superseding authentication lives in
    /// [`ConnectionTableInner::device_current`].
    authed: bool,
    /// Host-selected terms from the capability answer; re-advertising
    /// supersedes (latest wins) and the ingress gate enforces the recorded
    /// major on later frames.
    negotiated: Option<NegotiatedConnection>,
    /// Recently seen transport message ids, oldest-first, bounded by
    /// [`SEEN_MESSAGE_CAP`]. Transport duplicate suppression only (IPC
    /// §6.1–6.2): never a domain identity, never consulted for correlation
    /// (that is `command_id` / `request_id` / `reply_to`).
    seen_messages: std::collections::VecDeque<WireMessageId>,
}

/// Every method takes a short section over the inner maps and never awaits
/// while holding it. Poisoning recovers the committed table: sections run
/// plain map operations that never panic while holding the guard.
#[cfg(unix)]
#[derive(Debug, Default)]
struct ConnectionTable {
    /// The per-device live count shares this section so pairing and close
    /// stay atomic.
    inner: StdMutex<ConnectionTableInner>,
}

/// `device_live` counts connections currently holding each paired device
/// string: [`ConnectionTable::note_paired`] increments once when a connection
/// first records its device (immutable per connection thereafter), and
/// [`ConnectionTable::note_closed`] decrements. Presence falls back only at
/// zero, keeping connection close (transport fact) separate from presence
/// loss (domain fact).
#[cfg(unix)]
#[derive(Debug, Default)]
struct ConnectionTableInner {
    records: HashMap<ConnectionWireId, ConnectionRecord>,
    device_live: HashMap<String, usize>,
    /// Set by [`ConnectionTable::note_authed`]: a newer authentication by the
    /// same device overwrites the entry, so the older connection goes stale
    /// implicitly — [`ConnectionTable::live_for`] reports `authed` only while
    /// the entry still names the connection. Closing the current connection
    /// clears the entry (a newer entry is never cleared by an older close);
    /// survivors stay unauthed until they complete a fresh challenge.
    device_current: HashMap<String, ConnectionWireId>,
}

/// Zero-count entries are removed (rather than kept at zero) so that
/// "still live" reads as map membership with no separate bookkeeping.
#[cfg(unix)]
fn decrement_live(counts: &mut HashMap<String, usize>, device: &str) {
    if let Some(count) = counts.get_mut(device) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            counts.remove(device);
        }
    }
}

#[cfg(unix)]
impl ConnectionTable {
    fn new() -> Self {
        Self {
            inner: StdMutex::new(ConnectionTableInner::default()),
        }
    }

    fn note_accept(&self) -> ConnectionWireId {
        let id = ConnectionWireId(uuid::Uuid::new_v4());
        crate::lock_unpoison(&self.inner).records.insert(
            id,
            ConnectionRecord {
                paired_device: None,
                incarnation: None,
                authed: false,
                negotiated: None,
                seen_messages: std::collections::VecDeque::new(),
            },
        );
        id
    }

    fn note_negotiated(&self, id: &ConnectionWireId, terms: NegotiatedConnection) {
        if let Some(record) = crate::lock_unpoison(&self.inner).records.get_mut(id) {
            record.negotiated = Some(terms);
        }
    }

    /// Builds the [`LiveInput`] premises for one inbound envelope.
    ///
    /// Returns [`LiveDecision::Invalid`] when the connection is unknown (the
    /// table forgot it), when the envelope incarnation mismatches the pinned
    /// first incarnation, or when the envelope device claim is missing or
    /// disagrees with the table: the caller drops the connection without a
    /// reply in all three cases (a missing or mismatched claim is a
    /// protocol violation or theft attempt, and answering it would be an
    /// oracle). The `client_ref` is table-derived (paired device) or
    /// the pinned incarnation pair — never the envelope claim, which is
    /// only equality-checked; authority travels in `paired_device` plus
    /// `connection_known` plus `authed`, all table-filled, and
    /// `connection_id` carries the table key the gate requires envelopes
    /// to echo on post-capability frames. `authed` holds only while the
    /// record completed the challenge AND is still the device's current
    /// authed connection: a superseded connection reports unauthed even
    /// though its record keeps the flag.
    fn live_for(&self, id: &ConnectionWireId, envelope: &WireEnvelope) -> LiveDecision {
        let mut table = crate::lock_unpoison(&self.inner);
        // Transport duplicate suppression first: a redelivered message id is
        // dropped before it can pin incarnation, pair, or touch any domain
        // mapping. Fresh sends — including transport retries, which always
        // mint new ids — pass through and are recorded bounded-oldest-first.
        let (device, record_authed) = {
            let Some(record) = table.records.get_mut(id) else {
                return LiveDecision::Invalid;
            };
            if record.seen_messages.contains(&envelope.message_id) {
                return LiveDecision::Duplicate;
            }
            record.seen_messages.push_back(envelope.message_id);
            while record.seen_messages.len() > SEEN_MESSAGE_CAP {
                record.seen_messages.pop_front();
            }
            let seen = envelope.sender.incarnation_id;
            match record.incarnation {
                None => record.incarnation = Some(seen),
                Some(pinned) if pinned != seen => return LiveDecision::Invalid,
                Some(_) => {}
            }
            (record.paired_device.clone(), record.authed)
        };
        let claimed = envelope
            .sender
            .device_id
            .as_ref()
            .map(|id| id.0.as_hyphenated().to_string());
        let client_ref = match (&device, claimed) {
            (Some(paired), Some(claim)) if paired == &claim => paired.clone(),
            (None, None) => {
                let incarnation = envelope.sender.incarnation_id;
                format!("incarnation-{}-{}", incarnation.counter, incarnation.random)
            }
            _ => return LiveDecision::Invalid,
        };
        let current = device
            .as_ref()
            .is_some_and(|paired| table.device_current.get(paired) == Some(id));
        let negotiated = table
            .records
            .get(id)
            .and_then(|record| record.negotiated.clone());
        LiveDecision::Ready(LiveInput {
            client_ref,
            connection_live: true,
            peer_uid_ok: true,
            paired_device: device,
            connection_known: true,
            authed: record_authed && current,
            connection_id: *id,
            negotiated,
        })
    }

    /// Marks the connection paired with the Host-issued device wire string.
    ///
    /// Called only after [`HostHandle::handle_frame`] answers `Paired` on
    /// this connection, so the table records issuance, never a Client claim.
    /// The paired device is immutable once set: a connection can never move
    /// its live count to another device, and closing it can never strand an
    /// earlier device's liveness. Host ingress denies a second pairing
    /// request; this set-once check keeps the invariant even if such a
    /// response is emitted.
    fn note_paired(&self, id: &ConnectionWireId, device_wire: &str) {
        let mut table = crate::lock_unpoison(&self.inner);
        let Some(record) = table.records.get_mut(id) else {
            return;
        };
        if record.paired_device.is_some() {
            return;
        }
        record.paired_device = Some(device_wire.to_string());
        *table
            .device_live
            .entry(device_wire.to_string())
            .or_insert(0) += 1;
    }

    /// Marks the connection authenticated after an `Accepted` answer.
    ///
    /// Called only after [`HostHandle::handle_frame`] answers `Accepted` on
    /// this connection, so the table records authentication, never a Client
    /// claim. An unpaired connection records nothing: acceptance without a
    /// paired device cannot happen, and failing closed here keeps it that way.
    fn note_authed(&self, id: &ConnectionWireId) {
        let mut table = crate::lock_unpoison(&self.inner);
        let Some(record) = table.records.get_mut(id) else {
            return;
        };
        let Some(device) = record.paired_device.clone() else {
            return;
        };
        record.authed = true;
        table.device_current.insert(device, *id);
    }

    /// Forgets a closed connection, returning its paired device, if any, and
    /// whether that device still holds another live connection.
    ///
    /// The caller reports the device to
    /// [`HostHandle::note_disconnect`](crate::serve::HostHandle::note_disconnect)
    /// only when `device_still_live` is false: closing one of several live
    /// connections is a transport fact, not presence loss. Forgetting is
    /// idempotent, and closing the current authed connection clears its
    /// currency entry; an entry naming a newer connection is left alone.
    fn note_closed(&self, id: &ConnectionWireId) -> (Option<String>, bool) {
        let mut table = crate::lock_unpoison(&self.inner);
        let Some(record) = table.records.remove(id) else {
            return (None, false);
        };
        let Some(device) = record.paired_device else {
            return (None, false);
        };
        decrement_live(&mut table.device_live, &device);
        if table.device_current.get(&device) == Some(id) {
            table.device_current.remove(&device);
        }
        let still_live = table.device_live.contains_key(&device);
        (Some(device), still_live)
    }
}

/// Binds the singleton listener for `socket`, treating `AddrInUse` as a
/// possible live peer and probing via [`probe_and_rebind`].
///
/// # Errors
///
/// Returns [`CoreError::Bind`] when a live Host already serves the path, when
/// a stale path cannot be cleared, or when the (re)bind fails.
#[cfg(unix)]
async fn bind_singleton(socket: &Path) -> Result<UnixListener, CoreError> {
    match UnixListener::bind(socket) {
        Ok(listener) => Ok(listener),
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            probe_and_rebind(socket).await
        }
        Err(error) => Err(CoreError::Bind(format!("bind: {error}"))),
    }
}

/// Probes an in-use socket path and rebinds when it is stale.
///
/// A connectable path belongs to a live Host: error without unlinking. A
/// refused (or otherwise failed) connect means nothing listens, so the stale
/// path is unlinked and bound fresh. A probe timeout fails safe toward live.
#[cfg(unix)]
async fn probe_and_rebind(socket: &Path) -> Result<UnixListener, CoreError> {
    let probe = tokio::time::timeout(
        std::time::Duration::from_millis(SINGLETON_PROBE_MILLIS),
        UnixStream::connect(socket),
    )
    .await;
    let live = !matches!(probe, Ok(Err(_)));
    if live {
        return Err(CoreError::Bind(String::from(
            "another Host is already serving this data directory",
        )));
    }
    match std::fs::remove_file(socket) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CoreError::Bind(format!("remove stale socket: {error}")));
        }
    }
    UnixListener::bind(socket).map_err(|error| CoreError::Bind(format!("bind: {error}")))
}

/// Serves the Unix socket listener until the process ends.
///
/// Binds [`socket_path`] through the singleton check, proves each peer
/// against the socket owner, and spawns one frame-loop task per authorized
/// connection, driving the fake-friendly [`HostHandle::handle_frame`] seam.
/// There is no shutdown signal in `Stage 2`: the future resolves only on bind
/// failure; otherwise it runs until killed. The handle is shared by reference
/// (`Arc` with `&self` methods), so no handle-wide lock spans provider I/O.
///
/// # Errors
///
/// Returns [`CoreError::Bind`] when the socket cannot be bound (including a
/// live peer) or the socket metadata cannot be read.
#[cfg(unix)]
pub async fn run<T>(
    data_dir: PathBuf,
    handle: Arc<HostHandle>,
    transport: Arc<T>,
) -> Result<(), CoreError>
where
    T: ProviderTransport + Send + Sync + 'static,
{
    use std::os::unix::fs::MetadataExt as _;

    let socket = socket_path(&data_dir);
    let listener = bind_singleton(&socket).await?;
    let owner = std::fs::metadata(&socket)
        .map_err(|error| CoreError::Bind(format!("read socket metadata: {error}")))?
        .uid();
    let table = Arc::new(ConnectionTable::new());
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let peer_ok = match stream.peer_cred() {
            Ok(cred) => cred.uid() == owner,
            Err(_) => false,
        };
        if !peer_ok {
            continue;
        }
        let connection = table.note_accept();
        let handle = Arc::clone(&handle);
        let transport = Arc::clone(&transport);
        let table = Arc::clone(&table);
        tokio::spawn(async move {
            serve_connection(stream, connection, handle, transport, table).await;
        });
    }
}

#[cfg(unix)]
use crate::serve::STREAM_BUFFER_FRAMES;

/// Applies one response's table bookkeeping and writes it to the socket.
///
/// Returns `false` when the connection can no longer carry frames (encode or
/// write failure). The accepted connection id authenticates only when it is
/// the table key the handle echoed back.
#[cfg(unix)]
async fn write_response(
    stream: &mut tokio::net::UnixStream,
    table: &ConnectionTable,
    connection: &ConnectionWireId,
    response: WireFrame,
    terminal: &mut bool,
) -> bool {
    use tokio::io::AsyncWriteExt as _;

    if let WirePayload::PairingResult(PairingResult::Paired { device_id }) = &response.payload {
        table.note_paired(connection, &device_id.0.as_hyphenated().to_string());
    }
    if let WirePayload::AuthResult(AuthResult::Accepted { connection_id }) = &response.payload
        && *connection_id == *connection
    {
        table.note_authed(connection);
    }
    if let WirePayload::NegotiatedConnection(negotiated) = &response.payload {
        table.note_negotiated(connection, negotiated.clone());
    }
    if matches!(response.payload, WirePayload::DisconnectNotice(_)) {
        *terminal = true;
    }
    let Ok(encoded) = encode_frame(&response) else {
        return false;
    };
    stream.write_all(&encoded).await.is_ok()
}

#[cfg(unix)]
/// An incarnation mismatch, a corrupt or oversize frame, or a terminal
/// [`DisconnectNotice`](ene_api::v1::handshake::DisconnectNotice) in the
/// responses ends the connection; close always forgets the table entry and
/// reports a paired device to
/// [`HostHandle::note_disconnect`](crate::serve::HostHandle::note_disconnect)
/// only when no other live connection still holds that device (see
/// [`ConnectionTable::note_closed`]).
async fn serve_connection<T>(
    mut stream: tokio::net::UnixStream,
    connection: ConnectionWireId,
    handle: Arc<HostHandle>,
    transport: Arc<T>,
    table: Arc<ConnectionTable>,
) where
    T: ProviderTransport + Send + Sync + 'static,
{
    use tokio::io::AsyncReadExt as _;

    let mut prefix = [0_u8; 4];
    loop {
        if stream.read_exact(&mut prefix).await.is_err() {
            break;
        }
        let claimed = u32::from_be_bytes(prefix) as usize;
        if claimed > MAX_FRAME_BYTES {
            break;
        }
        let mut body = vec![0_u8; claimed];
        if stream.read_exact(&mut body).await.is_err() {
            break;
        }
        let mut bytes = Vec::with_capacity(prefix.len() + body.len());
        bytes.extend_from_slice(&prefix);
        bytes.extend_from_slice(&body);
        let Ok((frame, _)) = decode_frame(&bytes) else {
            break;
        };
        let live = match table.live_for(&connection, &frame.envelope) {
            LiveDecision::Ready(live) => live,
            LiveDecision::Duplicate => continue,
            LiveDecision::Invalid => break,
        };
        // The handle emits each response as it is decided; this loop writes
        // them while the host future is still running, so an early accept and
        // provider deltas reach the socket before provider completion. The
        // channel is bounded: stream deltas backpressure the provider when
        // the client falls behind instead of queueing without limit.
        let (frame_tx, mut frame_rx) =
            tokio::sync::mpsc::channel::<WireFrame>(STREAM_BUFFER_FRAMES);
        let mut sink = frame_tx.clone();
        let mut host = std::pin::pin!(handle.handle_frame_to(
            frame,
            live,
            transport.as_ref(),
            &mut sink,
            &frame_tx,
        ));
        let mut failed = false;
        let mut terminal = false;
        let mut host_done = false;
        loop {
            if host_done {
                while let Ok(response) = frame_rx.try_recv() {
                    if !write_response(&mut stream, &table, &connection, response, &mut terminal)
                        .await
                    {
                        failed = true;
                        break;
                    }
                }
                break;
            }
            tokio::select! {
                biased;
                () = &mut host => {
                    host_done = true;
                }
                maybe = frame_rx.recv() => {
                    match maybe {
                        Some(response) => {
                            if !write_response(
                                &mut stream,
                                &table,
                                &connection,
                                response,
                                &mut terminal,
                            )
                            .await
                            {
                                failed = true;
                                break;
                            }
                        }
                        None => host_done = true,
                    }
                }
            }
        }
        // The response above is already on the wire: post-response Learning
        // formation runs in its own task, never as part of the request's
        // completion. `run_pending_learning` serializes and drains, so a
        // second spawn that finds an emptied queue is a cheap no-op.
        if handle.has_pending_learning() {
            let worker_handle = Arc::clone(&handle);
            let worker_transport = Arc::clone(&transport);
            tokio::spawn(async move {
                worker_handle
                    .run_pending_learning(worker_transport.as_ref())
                    .await;
            });
        }
        if failed || terminal {
            break;
        }
    }
    let (device, still_live) = table.note_closed(&connection);
    if let Some(device) = device
        && !still_live
    {
        handle.note_disconnect(&device).await;
    }
}

/// Windows stub: no listener yet.
///
/// # Errors
///
/// Always returns [`CoreError::UnsupportedPlatform`].
#[cfg(not(unix))]
#[expect(
    clippy::unused_async,
    reason = "stub mirrors the async listener signature; the named-pipe follow-up awaits"
)]
pub async fn run(
    _data_dir: PathBuf,
    _handle: Arc<HostHandle>,
    _transport: Arc<impl ProviderTransport>,
) -> Result<(), CoreError> {
    Err(CoreError::UnsupportedPlatform("named-pipe listener"))
}

#[cfg(all(test, unix))]
mod tests;
