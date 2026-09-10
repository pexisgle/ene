//! Unix socket listener: accept, same-user check, frame loop, close.
//!
//! The listener binds `ene.sock` inside the data directory and serves one
//! task per connection. Each task reads length-prefixed
//! [`ene_plugin_ipc::WireFrame`] values, runs them through
//! [`HostHandle::handle_frame`], and writes the responses back. A
//! [`DisconnectNotice`](ene_api::v1::handshake::DisconnectNotice) in the
//! responses is terminal: it is written, then the connection closes.
//!
//! Single instance: [`run`] binds through `bind_singleton`, which tries the
//! bind first and only treats `AddrInUse` as a possible live peer. A
//! short-timeout connect probe then decides: connectable means a live Host
//! already serves this data directory (error, no unlink), while refused (or
//! any other connect failure) means a stale path, which is unlinked before
//! rebinding. A connect timeout fails safe toward live: the path is left
//! alone and the bind reports in use.
//!
//! Per-connection state lives in `ConnectionTable`, owned by this module:
//! [`run`] mints one [`ConnectionWireId`] per accepted connection, pins the
//! first incarnation it sees on that connection (a mismatch later drops the
//! connection without a reply — the frame cannot be attributed to the pinned
//! owner), and marks the connection paired after [`HostHandle::handle_frame`]
//! answers [`Paired`](ene_api::v1::handshake::PairingResult::Paired) on it.
//! A later [`Accepted`](ene_api::v1::handshake::AuthResult::Accepted) answer
//! on the connection marks it authenticated through
//! `ConnectionTable::note_authed`, which also records it as the device's
//! current authed connection: a newer authentication by the same device
//! supersedes the older one, and the older connection goes stale implicitly
//! (its record keeps `authed`, but it is no longer current, so the gate
//! drops its domain frames). Each frame's [`LiveInput`] premises
//! (`paired_device`, `connection_known`, `authed`, `connection_id`) come from
//! this table, never from Client self-reports; the ingress gate in
//! [`HostHandle::handle_frame`] trusts exactly these conn-filled premises,
//! and the envelope connection id must equal the table id on every
//! post-capability frame (the id is revealed to the Client only in
//! `AuthResult::Accepted`, and pre-accept responses carry [`None`], so
//! echoing it is the auth binding). Socket close forgets the entry and, when
//! the closing connection was the last live one holding its paired device,
//! reports the paired device string to
//! [`HostHandle::note_disconnect`](crate::serve::HostHandle::note_disconnect)
//! so presence falls back to `NoActive`. Connection close and presence loss
//! are deliberately separate: one device may hold several live connections,
//! and closing one of them must not clear presence while the others stay
//! live — only the last close for a device observes a disconnect.
//!
//! Same-user proof without new dependencies: after binding, the listener reads
//! the socket file owner through [`MetadataExt::uid`](std::os::unix::fs::MetadataExt)
//! (the file is created by this process inside the `0700` data directory, so
//! its owner is the Host user) and compares it against each peer credential
//! uid from [`tokio::net::UnixStream::peer_cred`]. A mismatch, or an unreadable peer
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
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};

use ene_inference::ProviderTransport;

use crate::serve::{CoreError, HostHandle};

/// Socket filename inside the data directory.
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
use ene_plugin_ipc::{MAX_FRAME_BYTES, decode_frame, encode_frame};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};

#[cfg(unix)]
use crate::serve::LiveInput;

/// Bound on the per-connection transport duplicate-suppression cache.
///
/// Enough to cover any realistic redelivery window on a local socket;
/// beyond it the oldest ids roll off and a very late duplicate would
/// re-process (message ids are sender-minted UUIDs, so a repeat after
/// roll-off means a true transport duplicate, never a fresh send — fresh
/// sends, including transport retries, always mint new ids).
#[cfg(unix)]
const SEEN_MESSAGE_CAP: usize = 128;

/// What the connection table decides for one inbound frame.
///
/// Separates transport redelivery from terminal violations: a duplicate
/// must drop silently with the connection kept, while an unknown
/// connection, incarnation mismatch, or device-claim mismatch drops the
/// frame and closes the connection. Collapsing both into `None` turned
/// legitimate redelivery into connection loss — a distinct observable
/// effect the §6.2 silent-drop contract forbids.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum LiveDecision {
    /// Process the frame under these premises.
    Ready(LiveInput),
    /// Redelivery of an already-seen message id: drop before any domain
    /// mapping and keep reading.
    Duplicate,
    /// Unknown connection, incarnation mismatch, or device-claim mismatch:
    /// drop and close.
    Invalid,
}

/// Per-connection pairing, incarnation, and authentication record.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ConnectionRecord {
    /// Device wire string paired on this connection, if any.
    paired_device: Option<String>,
    /// First incarnation seen on this connection, pinned on first frame.
    incarnation: Option<ClientIncarnationId>,
    /// Whether this connection completed the challenge/proof exchange.
    ///
    /// Set by [`ConnectionTable::note_authed`] after an `Accepted` answer;
    /// never cleared except by forgetting the record. Staleness after a
    /// superseding authentication is tracked separately in
    /// [`ConnectionTableInner::device_current`]: the old record keeps this
    /// flag, but [`ConnectionTable::live_for`] no longer reports the
    /// connection as authed once it is not current.
    authed: bool,
    /// Host-selected terms recorded when this connection answered
    /// capability. Re-advertising supersedes (latest wins); the ingress
    /// gate enforces the recorded major on every later frame.
    negotiated: Option<NegotiatedConnection>,
    /// Recently seen transport message ids, oldest-first, for duplicate
    /// suppression (IPC §6.1–6.2): a redelivered frame is dropped before
    /// any domain mapping, so transport at-least-once never becomes
    /// domain twice. Bounded by [`SEEN_MESSAGE_CAP`]; never a domain
    /// identity, never consulted for correlation (that is `command_id` /
    /// `request_id` / `reply_to`).
    seen_messages: std::collections::VecDeque<WireMessageId>,
}

/// Per-connection table owned by the listener.
///
/// Every method takes a short section over the inner maps and never awaits
/// while holding it. Poisoning recovers the committed table: sections run
/// plain map operations that never panic while holding the guard.
#[cfg(unix)]
#[derive(Debug, Default)]
struct ConnectionTable {
    /// Live connections by their Host-minted key, plus the per-device live
    /// count, under one section so pairing and close stay atomic.
    inner: StdMutex<ConnectionTableInner>,
}

/// Connection records plus the per-device live-connection count.
///
/// `device_live` counts connections currently holding each paired device
/// string: [`ConnectionTable::note_paired`] increments once when a
/// connection first records its device (the device is immutable per
/// connection thereafter), and [`ConnectionTable::note_closed`] decrements.
/// Presence falls back only when the count for a device reaches zero, which
/// keeps connection close (transport fact) separate from presence loss
/// (domain fact).
#[cfg(unix)]
#[derive(Debug, Default)]
struct ConnectionTableInner {
    /// Live connections by their Host-minted key.
    records: HashMap<ConnectionWireId, ConnectionRecord>,
    /// Live-connection count per paired device string.
    device_live: HashMap<String, usize>,
    /// Current authed connection per paired device wire string.
    ///
    /// Set by [`ConnectionTable::note_authed`]: a newer authentication by the
    /// same device overwrites the entry, so the older connection goes stale
    /// implicitly — [`ConnectionTable::live_for`] reports `authed` only while
    /// the entry still names the connection. Closing the current connection
    /// clears the entry (a newer entry for another connection is never
    /// cleared by an older close); the remaining live connections stay
    /// unauthed until they complete a fresh challenge.
    device_current: HashMap<String, ConnectionWireId>,
}

/// Locks the connection table, recovering from poisoning.
#[cfg(unix)]
fn lock_table(table: &StdMutex<ConnectionTableInner>) -> MutexGuard<'_, ConnectionTableInner> {
    match table.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Decrements the live count for `device`, dropping the entry at zero.
///
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
    /// Creates an empty table.
    fn new() -> Self {
        Self {
            inner: StdMutex::new(ConnectionTableInner::default()),
        }
    }

    /// Records an accepted connection under a freshly minted key.
    fn note_accept(&self) -> ConnectionWireId {
        let id = ConnectionWireId(uuid::Uuid::new_v4());
        lock_table(&self.inner).records.insert(
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

    /// Records the Host-selected terms answered on this connection,
    /// superseding any earlier negotiation.
    fn note_negotiated(&self, id: &ConnectionWireId, terms: NegotiatedConnection) {
        if let Some(record) = lock_table(&self.inner).records.get_mut(id) {
            record.negotiated = Some(terms);
        }
    }

    /// Builds the [`LiveInput`] premises for one inbound envelope.
    ///
    /// Returns [`None`] when the connection is unknown (the table forgot
    /// it), when the envelope incarnation mismatches the pinned first
    /// incarnation, or when the envelope device claim disagrees with the
    /// table: the caller drops the connection without a reply in all three
    /// cases (a mismatched claim is a theft attempt, and answering it would
    /// be an oracle). The `client_ref` is table-derived (paired device) or
    /// the pinned incarnation pair — never the envelope claim, which is
    /// only equality-checked; authority travels in `paired_device` plus
    /// `connection_known` plus `authed`, all table-filled, and
    /// `connection_id` carries the table key the gate requires envelopes
    /// to echo on post-capability frames. `authed` holds only while the
    /// record completed the challenge AND is still the device's current
    /// authed connection: a superseded connection reports unauthed even
    /// though its record keeps the flag.
    fn live_for(&self, id: &ConnectionWireId, envelope: &WireEnvelope) -> LiveDecision {
        let mut table = lock_table(&self.inner);
        // Transport duplicate suppression first: a redelivered message id is
        // dropped before it can pin incarnation, pair, or touch any domain
        // mapping. Fresh sends — including transport retries, which always
        // mint new ids — pass through and are recorded bounded-oldest-first.
        // The record borrow ends before the currency read below: both go
        // through the table guard, so they cannot overlap.
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
        // The envelope device claim is verified, never trusted: on a paired
        // connection it must be absent or equal the table value, otherwise
        // the frame is dropped (a mismatched claim is a theft attempt, and
        // answering it would be an oracle). On an unpaired connection any
        // device claim is a protocol violation with the same treatment: the
        // pairing/capability/proof frames that legitimately precede pairing
        // carry none by contract.
        let claimed = envelope
            .sender
            .device_id
            .as_ref()
            .map(|id| id.0.as_hyphenated().to_string());
        let client_ref = match (&device, claimed) {
            (Some(paired), Some(claim)) if paired != &claim => {
                return LiveDecision::Invalid;
            }
            (Some(paired), _) => paired.clone(),
            (None, Some(_)) => return LiveDecision::Invalid,
            (None, None) => {
                let incarnation = envelope.sender.incarnation_id;
                format!("incarnation-{}-{}", incarnation.counter, incarnation.random)
            }
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
    /// The paired device is immutable once set: the first `Paired` answer on
    /// a connection is the only one recorded, so a connection can never move
    /// its live count (or any other per-device bookkeeping) to another
    /// device, and closing it can never strand an earlier device's liveness.
    /// Host ingress denies a second pairing request on the same connection;
    /// this set-once check keeps the invariant even if such a response is
    /// ever emitted.
    fn note_paired(&self, id: &ConnectionWireId, device_wire: &str) {
        let mut table = lock_table(&self.inner);
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
    /// claim. The connection becomes the device's current authed connection:
    /// a newer authentication by the same device overwrites the entry and
    /// the older connection goes stale implicitly. An unpaired connection
    /// records nothing: acceptance without a paired device cannot happen,
    /// and failing closed here keeps it that way.
    fn note_authed(&self, id: &ConnectionWireId) {
        let mut table = lock_table(&self.inner);
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
    /// The caller reports a returned device string to
    /// [`HostHandle::note_disconnect`](crate::serve::HostHandle::note_disconnect)
    /// only when `device_still_live` is false: closing one of several live
    /// connections for a device is a transport fact, not presence loss. A
    /// second close for the same key returns [`None`] with false: forgetting
    /// is idempotent. When the closing connection is the device's current
    /// authed connection, the currency entry goes with it; an entry naming a
    /// different (newer) connection is left alone, and the survivors stay
    /// unauthed until they complete a fresh challenge.
    fn note_closed(&self, id: &ConnectionWireId) -> (Option<String>, bool) {
        let mut table = lock_table(&self.inner);
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

/// Binds the singleton listener for `socket`.
///
/// Tries the bind first: success means no live peer and no stale path. An
/// `AddrInUse` bind runs the connect probe — connectable means a live Host
/// already serves this path (error, no unlink), while a refused probe means a
/// stale path, which is unlinked before rebinding. Any other bind failure
/// reports directly.
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
    // Only a refused (or otherwise failed) connect means stale. A timeout
    // fails safe toward live: the path is left alone.
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
/// connection. The socket path is assembled here from `data_dir`: callers
/// pass the data directory, never the socket path. The transport is the
/// production `OpenAI` transport; the fake-friendly seam is
/// [`HostHandle::handle_frame`], which this loop drives. There is no shutdown
/// signal in `Stage 2`: the future resolves only on bind failure; otherwise it
/// runs until killed. The handle is shared by reference (`Arc` with `&self`
/// methods): no handle-wide lock spans provider I/O.
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

/// Runs one connection frame loop: read, handle, write, close on terminal.
///
/// An incarnation mismatch, a corrupt or oversize frame, or a terminal
/// [`DisconnectNotice`](ene_api::v1::handshake::DisconnectNotice) in the
/// responses ends the connection; close always forgets the table entry and
/// reports a paired device to
/// [`HostHandle::note_disconnect`](crate::serve::HostHandle::note_disconnect)
/// only when no other live connection still holds that device (see
/// [`ConnectionTable::note_closed`]).
#[cfg(unix)]
async fn serve_connection<T>(
    mut stream: tokio::net::UnixStream,
    connection: ConnectionWireId,
    handle: Arc<HostHandle>,
    transport: Arc<T>,
    table: Arc<ConnectionTable>,
) where
    T: ProviderTransport + Send + Sync + 'static,
{
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

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
            // Transport redelivery: silent drop, connection kept.
            LiveDecision::Duplicate => continue,
            // Unknown connection, incarnation mismatch, or device-claim
            // mismatch: terminal.
            LiveDecision::Invalid => break,
        };
        let responses = handle.handle_frame(frame, live, transport.as_ref()).await;
        for response in &responses {
            if let WirePayload::PairingResult(PairingResult::Paired { device_id }) =
                &response.payload
            {
                table.note_paired(&connection, &device_id.0.as_hyphenated().to_string());
            }
            // The accepted id is the table key the handle echoed back: only
            // an exact match authenticates this connection.
            if let WirePayload::AuthResult(AuthResult::Accepted { connection_id }) =
                &response.payload
                && *connection_id == connection
            {
                table.note_authed(&connection);
            }
            if let WirePayload::NegotiatedConnection(negotiated) = &response.payload {
                table.note_negotiated(&connection, negotiated.clone());
            }
        }
        let mut failed = false;
        let mut terminal = false;
        for response in &responses {
            if matches!(response.payload, WirePayload::DisconnectNotice(_)) {
                terminal = true;
            }
            let Ok(encoded) = encode_frame(response) else {
                failed = true;
                break;
            };
            if stream.write_all(&encoded).await.is_err() {
                failed = true;
                break;
            }
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
/// The follow-up is a named-pipe listener behind the same
/// [`HostHandle::handle_frame`] seam.
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
