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
//! [`run`] mints one [`ConnectionWireId`] per accepted connection, and every
//! connection advances through the one-way [`ConnectionPhase`] machine
//! (IPC §9.3): `Accepted → Paired → Challenged → Authenticated → Superseded |
//! Closed`. `authenticated` means the ownership proof succeeded on this
//! connection; `current` means the Host's per-device current slot still points
//! at it. Domain ingress and presence reachability use only a connection that
//! is authenticated, current, open, and bound to a valid device — never
//! paired-socket counts or past authentication successes (#1384). A newer
//! authentication for the same device supersedes the previous connection
//! irreversibly; superseded sockets answer typed `StaleConnection` rejections
//! while staying open (IPC §11.3) and can never become current again. Close
//! admission compares connection identity before clearing the current slot:
//! closing a superseded or unauthenticated connection never clears the newer
//! current, and the presence fallback condition is the absence of a current
//! authenticated connection, not a zero live-socket count.
//!
//! Each frame's [`LiveInput`] premises come from this table, never from Client
//! self-reports; the ingress gate in [`HostHandle::handle_frame`] trusts
//! exactly these conn-filled premises. [`LiveInput`] also carries the table
//! handle, so the handshake phase operations (device bind, nonce consumption,
//! auth install) and the close admission that runs the presence
//! compare/commit (CCT §10.4) are short connection-ownership sections: the
//! table section is never held across an `.await`, and the SQLite work runs
//! synchronously inside `spawn_blocking`. Lock order: connection table → Task
//! execution registry → SQLite.
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
//! Windows serves the same loop over a named pipe: [`run`] creates the
//! exclusive first server instance for the data directory's pipe name (a
//! second Host fails to create, like the Unix singleton probe), checks the OS
//! peer token before any frame is read, and drives the same
//! [`HostHandle::handle_frame`] seam, so authentication, currentness, and the
//! connection phase machine are identical on both transports. Other platforms
//! have no listener and [`run`] returns
//! [`CoreError::UnsupportedPlatform`] there.
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

#[cfg(any(unix, test))]
use ene_api::v1::envelope::WireEnvelope;
use ene_api::v1::handshake::NegotiatedConnection;
#[cfg(any(unix, windows))]
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{ClientIncarnationId, ConnectionWireId, WireMessageId};
#[cfg(any(unix, windows))]
use ene_plugin_ipc::{MAX_FRAME_BYTES, WireFrame, decode_frame, encode_frame};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};

#[cfg(any(unix, test))]
use crate::serve::LiveInput;

/// Bound on the per-connection transport duplicate-suppression cache, enough
/// for any realistic redelivery window on a local socket. Beyond it the
/// oldest ids roll off and a very late duplicate re-processes: message ids
/// are sender-minted UUIDs, so a repeat after roll-off is a true transport
/// duplicate, never a fresh send (fresh sends, including transport retries,
/// always mint new ids).
#[cfg(any(unix, test))]
const SEEN_MESSAGE_CAP: usize = 128;

/// Separates transport redelivery from terminal violations: a duplicate
/// drops silently with the connection kept, while an unknown connection,
/// incarnation mismatch, or device-claim mismatch drops the frame and closes
/// the connection. Collapsing both into `None` would turn legitimate
/// redelivery into connection loss, a distinct observable effect the §6.2
/// silent-drop contract forbids.
#[cfg(any(unix, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LiveDecision {
    Ready(LiveInput),
    Duplicate,
    Invalid,
}

/// One-way connection phase (IPC §9.3).
///
/// `Accepted → Paired → Challenged → Authenticated → Superseded | Closed`.
/// A failed authentication ends in `Closed`, as does the transport ending in
/// any phase. `Superseded` and `Closed` are terminal: a superseded connection
/// never returns to a serviceable phase (IPC §11.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionPhase {
    /// The connection exists; it has not paired or presented a device.
    Accepted,
    /// A `PairingResult::Paired` issued a device key on this connection, or a
    /// reconnect capability frame bound an existing device.
    Paired,
    /// Capability terms and a single-use challenge nonce are pending proof.
    Challenged,
    /// The ownership proof succeeded on this connection.
    Authenticated,
    /// A newer authentication for the same device replaced this connection as
    /// the device's current one. Irreversible.
    Superseded,
    /// The connection cannot proceed: authentication failed, or the transport
    /// ended. Irreversible.
    Closed,
}

impl ConnectionPhase {
    /// Whether the phase is terminal and can never become serviceable again.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Superseded | Self::Closed)
    }

    /// Whether this connection was replaced by a newer authentication.
    #[must_use]
    pub fn is_superseded(self) -> bool {
        self == Self::Superseded
    }
}

/// How one capability phase operation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChallengeOutcome {
    /// The device is bound (reconnect) or was already bound, the terms were
    /// recorded, and a fresh nonce is pending proof.
    Challenged,
    /// The connection was superseded by a newer authentication.
    Superseded,
    /// The connection is not in a phase that admits a capability frame.
    WrongPhase,
    /// The table no longer holds the connection.
    Unknown,
}

/// How consuming a challenge nonce ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NonceAdmission {
    /// The single-use nonce for this challenge.
    Nonce(String),
    /// The connection is challenged but the nonce was already consumed.
    Missing,
    /// The connection was superseded by a newer authentication.
    Superseded,
    /// The connection is not in the challenged phase.
    WrongPhase,
    /// The table no longer holds the connection.
    Unknown,
}

/// How installing an authenticated connection ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallOutcome {
    /// This connection is now authenticated and current; the previous current
    /// connection, if any, is superseded.
    Installed,
    /// The connection was superseded before this install: the proof no longer
    /// counts and the newer current is untouched.
    Superseded,
    /// The connection is not in the challenged phase.
    WrongPhase,
    /// The table no longer holds the connection.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConnectionRecord {
    phase: ConnectionPhase,
    /// Host-issued device wire string bound to this connection: set once by a
    /// `Paired` answer or by the reconnect device bind, never moved to
    /// another device afterwards.
    paired_device: Option<String>,
    incarnation: Option<ClientIncarnationId>,
    /// Host-selected terms from the capability answer. Written exactly once,
    /// when the challenge is issued; a later capability frame is refused
    /// without touching them.
    negotiated: Option<NegotiatedConnection>,
    /// The single-use challenge nonce, consumed by the first `AuthProof` in
    /// the challenged phase.
    nonce: Option<String>,
    /// Recently seen transport message ids, oldest-first, bounded by
    /// [`SEEN_MESSAGE_CAP`]. Transport duplicate suppression only (IPC
    /// §6.1–6.2): never a domain identity, never consulted for correlation
    /// (that is `command_id` / `request_id` / `reply_to`).
    seen_messages: std::collections::VecDeque<WireMessageId>,
}

/// Every method takes a short section over the inner maps and never awaits
/// while holding it. Poisoning recovers the committed table: sections run
/// plain map operations that never panic while holding the guard.
#[derive(Debug, Default)]
pub(crate) struct ConnectionTable {
    inner: StdMutex<ConnectionTableInner>,
}

#[derive(Debug, Default)]
struct ConnectionTableInner {
    records: HashMap<ConnectionWireId, ConnectionRecord>,
    /// The per-device current connection slot. [`ConnectionTable::install_authenticated`]
    /// overwrites it (superseding the previous record), and
    /// [`ConnectionTable::note_closed`] clears it only when the closing
    /// connection still owns the slot. Paired-socket counts are deliberately
    /// not tracked: currentness, not liveness, decides reachability (#1384).
    device_current: HashMap<String, ConnectionWireId>,
}

impl ConnectionTable {
    #[cfg(any(unix, test))]
    pub(crate) fn new() -> Self {
        Self {
            inner: StdMutex::new(ConnectionTableInner::default()),
        }
    }

    #[cfg(any(unix, test))]
    pub(crate) fn note_accept(&self) -> ConnectionWireId {
        let id = ConnectionWireId(uuid::Uuid::new_v4());
        crate::lock_unpoison(&self.inner).records.insert(
            id,
            ConnectionRecord {
                phase: ConnectionPhase::Accepted,
                paired_device: None,
                incarnation: None,
                negotiated: None,
                nonce: None,
                seen_messages: std::collections::VecDeque::new(),
            },
        );
        id
    }

    /// Records a Host-issued device key: `Accepted → Paired`, set-once.
    ///
    /// Called only after the pairing repository answered `Paired` on this
    /// connection, so the table records issuance, never a Client claim. Any
    /// other phase, or a second device, changes nothing.
    pub(crate) fn note_paired(&self, id: &ConnectionWireId, device_wire: &str) -> bool {
        let mut table = crate::lock_unpoison(&self.inner);
        let Some(record) = table.records.get_mut(id) else {
            return false;
        };
        if record.phase != ConnectionPhase::Accepted || record.paired_device.is_some() {
            return false;
        }
        record.paired_device = Some(device_wire.to_string());
        record.phase = ConnectionPhase::Paired;
        true
    }

    /// Records one capability frame as the connection's single challenge.
    ///
    /// This is one phase operation (IPC §9.3): on `Accepted` it binds
    /// `bind_device` (the reconnect device the Host already resolved in the
    /// device store) and moves straight to `Challenged`; on `Paired` (the
    /// fresh pairing completed) it requires no new bind. Terms and the nonce
    /// are written only here, so a repeat or out-of-phase capability frame
    /// cannot change them.
    pub(crate) fn note_challenged(
        &self,
        id: &ConnectionWireId,
        bind_device: Option<&str>,
        terms: NegotiatedConnection,
        nonce: String,
    ) -> ChallengeOutcome {
        let mut table = crate::lock_unpoison(&self.inner);
        let Some(record) = table.records.get_mut(id) else {
            return ChallengeOutcome::Unknown;
        };
        match record.phase {
            ConnectionPhase::Accepted => {
                let Some(device) = bind_device else {
                    return ChallengeOutcome::WrongPhase;
                };
                record.paired_device = Some(device.to_string());
            }
            ConnectionPhase::Paired => {
                if let Some(bind) = bind_device
                    && record.paired_device.as_deref() != Some(bind)
                {
                    return ChallengeOutcome::WrongPhase;
                }
            }
            ConnectionPhase::Superseded => return ChallengeOutcome::Superseded,
            ConnectionPhase::Challenged
            | ConnectionPhase::Authenticated
            | ConnectionPhase::Closed => return ChallengeOutcome::WrongPhase,
        }
        record.phase = ConnectionPhase::Challenged;
        record.negotiated = Some(terms);
        record.nonce = Some(nonce);
        ChallengeOutcome::Challenged
    }

    /// Consumes the challenge nonce if this connection is still challenged.
    pub(crate) fn take_nonce(&self, id: &ConnectionWireId) -> NonceAdmission {
        let mut table = crate::lock_unpoison(&self.inner);
        let Some(record) = table.records.get_mut(id) else {
            return NonceAdmission::Unknown;
        };
        match record.phase {
            ConnectionPhase::Challenged => match record.nonce.take() {
                Some(nonce) => NonceAdmission::Nonce(nonce),
                None => NonceAdmission::Missing,
            },
            ConnectionPhase::Superseded => NonceAdmission::Superseded,
            ConnectionPhase::Accepted
            | ConnectionPhase::Paired
            | ConnectionPhase::Authenticated
            | ConnectionPhase::Closed => NonceAdmission::WrongPhase,
        }
    }

    /// Installs this connection as authenticated and current in one section.
    ///
    /// The phase and the bound device are re-checked here, so a proof that
    /// raced a newer authentication cannot install over it: a superseded
    /// record answers [`InstallOutcome::Superseded`] and leaves the newer
    /// current alone. Installation supersedes the device's previous current
    /// record irreversibly; a lost `AuthResult::Accepted` response never rolls
    /// the install back.
    pub(crate) fn install_authenticated(&self, id: &ConnectionWireId) -> InstallOutcome {
        let mut table = crate::lock_unpoison(&self.inner);
        let previous = {
            let Some(record) = table.records.get_mut(id) else {
                return InstallOutcome::Unknown;
            };
            match record.phase {
                ConnectionPhase::Challenged => {}
                ConnectionPhase::Superseded => return InstallOutcome::Superseded,
                ConnectionPhase::Accepted
                | ConnectionPhase::Paired
                | ConnectionPhase::Authenticated
                | ConnectionPhase::Closed => return InstallOutcome::WrongPhase,
            }
            let Some(device) = record.paired_device.clone() else {
                return InstallOutcome::WrongPhase;
            };
            record.phase = ConnectionPhase::Authenticated;
            table.device_current.insert(device, *id)
        };
        if let Some(previous) = previous
            && previous != *id
            && let Some(record) = table.records.get_mut(&previous)
            && record.phase == ConnectionPhase::Authenticated
        {
            record.phase = ConnectionPhase::Superseded;
        }
        InstallOutcome::Installed
    }

    /// Ends a failed authentication: `Challenged → Closed`.
    ///
    /// Only the challenged phase moves; a superseded record stays superseded
    /// (that phase is irreversible), and any other phase is left as it is.
    pub(crate) fn note_auth_failed(&self, id: &ConnectionWireId) {
        let mut table = crate::lock_unpoison(&self.inner);
        if let Some(record) = table.records.get_mut(id)
            && record.phase == ConnectionPhase::Challenged
        {
            record.phase = ConnectionPhase::Closed;
        }
    }

    /// Builds the [`LiveInput`] premises for one inbound envelope.
    ///
    /// Returns [`LiveDecision::Invalid`] when the connection is unknown (the
    /// table forgot it), when the envelope incarnation mismatches the pinned
    /// first incarnation, or when a paired connection's envelope claim is
    /// missing or disagrees with the table: the caller drops the connection
    /// without a reply in all three cases (a missing or mismatched claim is a
    /// protocol violation or theft attempt, and answering it would be an
    /// oracle). An unbound `Accepted` record tolerates a device claim: the
    /// reconnect path resolves it, and the claim is never authority on its
    /// own. The `client_ref` is table-derived (paired device) or the pinned
    /// incarnation pair — never the envelope claim, which is only
    /// equality-checked; authority travels in `paired_device` plus
    /// `connection_known` plus `authed`, all table-filled. `authed` holds only
    /// while the record completed the challenge AND is still the device's
    /// current connection: a superseded connection reports unauthed even
    /// though its record keeps its phase. `phase` is the snapshot the gate
    /// uses to answer typed stale rejections.
    #[cfg(any(unix, test))]
    pub(crate) fn live_for(
        self: &Arc<Self>,
        id: &ConnectionWireId,
        envelope: &WireEnvelope,
    ) -> LiveDecision {
        let mut table = crate::lock_unpoison(&self.inner);
        // Transport duplicate suppression first: a redelivered message id is
        // dropped before it can pin incarnation, pair, or touch any domain
        // mapping. Fresh sends — including transport retries, which always
        // mint new ids — pass through and are recorded bounded-oldest-first.
        let (device, phase, negotiated) = {
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
            (
                record.paired_device.clone(),
                record.phase,
                record.negotiated.clone(),
            )
        };
        let claimed = envelope
            .sender
            .device_id
            .as_ref()
            .map(|id| id.0.as_hyphenated().to_string());
        let client_ref = match (&device, &claimed) {
            (Some(paired), Some(claim)) if paired == claim => paired.clone(),
            (Some(_), _) => return LiveDecision::Invalid,
            (None, _) => {
                let incarnation = envelope.sender.incarnation_id;
                format!("incarnation-{}-{}", incarnation.counter, incarnation.random)
            }
        };
        let current = device
            .as_ref()
            .is_some_and(|paired| table.device_current.get(paired) == Some(id));
        LiveDecision::Ready(LiveInput {
            client_ref,
            connection_live: true,
            peer_uid_ok: true,
            paired_device: device,
            connection_known: true,
            authed: phase == ConnectionPhase::Authenticated && current,
            connection_id: *id,
            negotiated,
            phase,
            authority: Arc::clone(self),
        })
    }

    /// Whether `device` currently has an authenticated connection.
    ///
    /// The presence fallback trigger is this absence, not a zero live-socket
    /// count: a lingering superseded or unauthenticated socket never satisfies
    /// it (#1384).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "presence/Client-dependent admission consumes this predicate in the presence slice; slice A tests it directly"
        )
    )]
    pub(crate) fn current_authenticated(&self, device: &str) -> bool {
        let table = crate::lock_unpoison(&self.inner);
        table.device_current.get(device).is_some_and(|id| {
            table
                .records
                .get(id)
                .is_some_and(|record| record.phase == ConnectionPhase::Authenticated)
        })
    }

    /// Forgets a closed connection and runs the presence fallback in the same
    /// section when it was the device's current authenticated connection.
    ///
    /// The current slot is compared against this connection's identity before
    /// clearing, so a superseded or unauthenticated close never clears a newer
    /// current (S5-05). `on_fallback` runs while the section is still held, so
    /// its synchronous presence compare/commit (CCT §10.4) cannot interleave
    /// with a competing authentication install; it must not call back into
    /// this table. Forgetting is idempotent.
    #[cfg(any(unix, test))]
    pub(crate) fn note_closed(
        &self,
        id: &ConnectionWireId,
        on_fallback: impl FnOnce(&str),
    ) -> Option<String> {
        let mut table = crate::lock_unpoison(&self.inner);
        let record = table.records.remove(id)?;
        let device = record.paired_device?;
        if table.device_current.get(&device) == Some(id) {
            table.device_current.remove(&device);
            on_fallback(&device);
        }
        Some(device)
    }

    /// Current phase of a connection, or [`None`] when the table forgot it.
    pub(crate) fn phase_of(&self, id: &ConnectionWireId) -> Option<ConnectionPhase> {
        crate::lock_unpoison(&self.inner)
            .records
            .get(id)
            .map(|record| record.phase)
    }

    /// Test-only pending-challenge snapshot.
    #[cfg(test)]
    pub(crate) fn challenge_nonce_of(&self, id: &ConnectionWireId) -> Option<String> {
        crate::lock_unpoison(&self.inner)
            .records
            .get(id)
            .and_then(|record| record.nonce.clone())
    }

    /// Test-only negotiated-terms snapshot.
    #[cfg(test)]
    pub(crate) fn negotiated_of(&self, id: &ConnectionWireId) -> Option<NegotiatedConnection> {
        crate::lock_unpoison(&self.inner)
            .records
            .get(id)
            .and_then(|record| record.negotiated.clone())
    }

    /// Test-only [`LiveInput`] snapshot for a connection, without an envelope.
    ///
    /// Mirrors [`ConnectionTable::live_for`]'s premise derivation so direct
    /// handle tests can drive the real table.
    #[cfg(test)]
    pub(crate) fn test_live(self: &Arc<Self>, id: &ConnectionWireId) -> Option<LiveInput> {
        let table = crate::lock_unpoison(&self.inner);
        let record = table.records.get(id)?;
        let device = record.paired_device.clone();
        let current = device
            .as_ref()
            .is_some_and(|paired| table.device_current.get(paired) == Some(id));
        Some(LiveInput {
            client_ref: device.clone().unwrap_or_else(|| String::from("unpaired")),
            connection_live: true,
            peer_uid_ok: true,
            paired_device: device,
            connection_known: true,
            authed: record.phase == ConnectionPhase::Authenticated && current,
            connection_id: *id,
            negotiated: record.negotiated.clone(),
            phase: record.phase,
            authority: Arc::clone(self),
        })
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
    // Production Task Agent launcher: the serving process owns the shared
    // handle and provider transport, so an accepted conversation delegation
    // starts the existing runner in the background without any test-side
    // runner invocation.
    let launcher = std::sync::Arc::new(crate::task_run::BackgroundTaskAgent::new(
        Arc::clone(&handle),
        Arc::clone(&transport),
    ));
    let _ = handle.install_task_launcher(launcher);
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

#[cfg(any(unix, windows))]
use crate::serve::STREAM_BUFFER_FRAMES;

/// Writes one response frame to the connection.
///
/// The phase and current-slot bookkeeping happens where each answer is
/// decided, inside the connection-ownership sections (IPC §9.3); this loop
/// only carries bytes. Returns `false` when the connection can no longer
/// carry frames (encode or write failure).
#[cfg(any(unix, windows))]
async fn write_response(
    stream: &mut (impl tokio::io::AsyncWrite + Unpin),
    response: WireFrame,
    terminal: &mut bool,
) -> bool {
    use tokio::io::AsyncWriteExt as _;

    if matches!(response.payload, WirePayload::DisconnectNotice(_)) {
        *terminal = true;
    }
    let Ok(encoded) = encode_frame(&response) else {
        return false;
    };
    stream.write_all(&encoded).await.is_ok()
}

#[cfg(any(unix, windows))]
/// An incarnation mismatch, a corrupt or oversize frame, or a terminal
/// [`DisconnectNotice`](ene_api::v1::handshake::DisconnectNotice) in the
/// responses ends the connection; close always forgets the table entry and
/// runs the presence fallback through
/// [`HostHandle::close_connection`](crate::serve::HostHandle::close_connection)
/// when this was the device's current authenticated connection.
///
/// Transport-generic over the byte stream so the Unix socket and the Windows
/// named pipe share this loop, the duplicate suppression, and the phase gate.
async fn serve_connection<S, T>(
    mut stream: S,
    connection: ConnectionWireId,
    handle: Arc<HostHandle>,
    transport: Arc<T>,
    table: Arc<ConnectionTable>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
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
                    if !write_response(&mut stream, response, &mut terminal).await {
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
                            if !write_response(&mut stream, response, &mut terminal).await {
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
    handle.close_connection(&table, connection).await;
}

/// Serves the Windows named-pipe listener until the process ends.
///
/// Creates the exclusive first server instance for the data directory's pipe
/// name (a live peer fails creation, like the Unix singleton probe — pipe
/// instances vanish with their process, so there is no stale path to unlink),
/// proves each peer with the OS token check, and spawns one frame-loop task
/// per authorized connection over the shared [`HostHandle::handle_frame`]
/// seam. There is no shutdown signal in `Stage 2`: the future resolves only
/// on creation failure; otherwise it runs until killed. Behavior beyond
/// creation is Windows-unverified on this Linux host (see
/// [`crate::conn_pipe`]).
///
/// # Errors
///
/// Returns [`CoreError::Bind`] when the first pipe instance cannot be created
/// (including a live peer) or a follow-up instance cannot be created.
#[cfg(windows)]
pub async fn run<T>(
    data_dir: PathBuf,
    handle: Arc<HostHandle>,
    transport: Arc<T>,
) -> Result<(), CoreError>
where
    T: ProviderTransport + Send + Sync + 'static,
{
    use std::os::windows::io::AsRawHandle as _;

    let pipe = crate::conn_pipe::pipe_name(&data_dir);
    let mut server = crate::conn_pipe::create_first_server(&pipe)?;
    // Production Task Agent launcher: same ownership as the Unix path, so an
    // accepted conversation delegation starts the existing runner in the
    // background without any test-side runner invocation.
    let launcher = std::sync::Arc::new(crate::task_run::BackgroundTaskAgent::new(
        Arc::clone(&handle),
        Arc::clone(&transport),
    ));
    let _ = handle.install_task_launcher(launcher);
    let table = Arc::new(ConnectionTable::new());
    loop {
        if server.connect().await.is_err() {
            // A failed wait leaves this instance unusable; replace it rather
            // than serving half-open state.
            server = crate::conn_pipe::create_next_server(&pipe)?;
            continue;
        }
        // The OS peer token check runs before any frame is read: an
        // unprovable peer is dropped without a byte, like the Unix
        // uid-mismatch path.
        let peer_ok = crate::conn_pipe::peer_same_user(server.as_raw_handle());
        let next = crate::conn_pipe::create_next_server(&pipe)?;
        let current = std::mem::replace(&mut server, next);
        if !peer_ok {
            continue;
        }
        let connection = table.note_accept();
        let handle = Arc::clone(&handle);
        let transport = Arc::clone(&transport);
        let table = Arc::clone(&table);
        tokio::spawn(async move {
            serve_connection(current, connection, handle, transport, table).await;
        });
    }
}

/// Unsupported platforms have no listener.
///
/// # Errors
///
/// Always returns [`CoreError::UnsupportedPlatform`].
#[cfg(not(any(unix, windows)))]
#[expect(
    clippy::unused_async,
    reason = "stub mirrors the async listener signature; no transport exists here"
)]
pub async fn run(
    _data_dir: PathBuf,
    _handle: Arc<HostHandle>,
    _transport: Arc<impl ProviderTransport>,
) -> Result<(), CoreError> {
    Err(CoreError::UnsupportedPlatform("no supported listener"))
}

#[cfg(all(test, unix))]
mod tests;
