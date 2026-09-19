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
//! stale path; a probe timeout fails safe toward live. The same process also
//! binds the Host-local first-party control endpoint
//! ([`crate::host_control`]) in its accept loop: the Owner's Targeted
//! Deletion confirmation must execute against the live connection table and
//! Client delivery tracking, so it is served from this process and never
//! from an offline state open (lifecycle §8.1, PR §6.4).
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
//! synchronously inside `spawn_blocking`. `ConnectionTable::is_current_authenticated`
//! is the synchronous currentness predicate for observation-only callers
//! (notably the stream gate's final pre-publication check), while
//! `ConnectionTable::with_current_connection` is the commit primitive every
//! Client-dependent mutation uses. Lock order: connection table →
//! presentation memory → Task execution registry → SQLite.
//!
//! Same-user proof without new dependencies: after binding, the listener reads
//! the socket file owner through [`MetadataExt::uid`]
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
//! [`MetadataExt::uid`]: https://doc.rust-lang.org/std/os/unix/fs/trait.MetadataExt.html#tymethod.uid
//! [`tokio::net::UnixStream::peer_cred`]: https://docs.rs/tokio/latest/tokio/net/struct.UnixStream.html#method.peer_cred

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use ene_inference::ProviderTransport;

use crate::serve::{CoreError, HostHandle, outgoing_frame};

const SOCKET_NAME: &str = "ene.sock";

/// Connect-probe timeout for the singleton check.
///
/// Short on purpose: a live listener accepts from its backlog immediately, so
/// anything slower is treated as live anyway (fail-safe: never unlink a
/// maybe-live path).
#[cfg(unix)]
const SINGLETON_PROBE_MILLIS: u64 = 200;

/// Period between serving-time Targeted Deletion ticks (lifecycle §14).
///
/// One tick runs at most one bounded fan-out pass plus at most one backed-off
/// resume of a retryable hold, so the period bounds the retry rate of a held
/// operation. A confirmation kicks its operation immediately; this driver
/// continues whatever remains — multi-pass sweeps, a hold whose holder became
/// reachable, or a kick that failed technically.
#[cfg(any(unix, windows))]
const DELETION_DRIVE_PERIOD: std::time::Duration = std::time::Duration::from_secs(15);

/// Resolves the listener socket path for a data directory.
///
/// Public so the sibling Client dialer and integration tests derive the same
/// path the Host binds: there is exactly one socket name and it lives here.
#[must_use]
pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SOCKET_NAME)
}

#[cfg(any(unix, windows, test))]
use ene_api::v1::envelope::WireEnvelope;
use ene_api::v1::handshake::NegotiatedConnection;
#[cfg(any(unix, windows))]
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{ClientIncarnationId, ConnectionWireId, WireMessageId};
#[cfg(any(unix, windows))]
use ene_plugin_ipc::{MAX_FRAME_BYTES, WireFrame, decode_frame, encode_frame};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};

#[cfg(any(unix, windows, test))]
use crate::serve::LiveInput;

/// Bound on the per-connection transport duplicate-suppression cache, enough
/// for any realistic redelivery window on a local socket. Beyond it the
/// oldest ids roll off and a very late duplicate re-processes: message ids
/// are sender-minted UUIDs, so a repeat after roll-off is a true transport
/// duplicate, never a fresh send (fresh sends, including transport retries,
/// always mint new ids).
#[cfg(any(unix, windows))]
const SEEN_MESSAGE_CAP: usize = 128;

/// Separates transport redelivery from terminal violations: a duplicate
/// drops silently with the connection kept, while an unknown connection,
/// incarnation mismatch, or device-claim mismatch drops the frame and closes
/// the connection. Collapsing both into `None` would turn legitimate
/// redelivery into connection loss, a distinct observable effect the §6.2
/// silent-drop contract forbids.
#[cfg(any(unix, windows))]
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
    /// This connection is now authenticated and current; the previous
    /// current connection, if any, is superseded and named here so the
    /// caller can drop its connection-owned presentation state.
    Installed {
        superseded: Option<ConnectionWireId>,
    },
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
    #[cfg(any(unix, windows))]
    pub(crate) fn new() -> Self {
        Self {
            inner: StdMutex::new(ConnectionTableInner::default()),
        }
    }

    #[cfg(any(unix, windows))]
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
        let mut superseded = None;
        if let Some(previous) = previous
            && previous != *id
            && let Some(record) = table.records.get_mut(&previous)
            && record.phase == ConnectionPhase::Authenticated
        {
            record.phase = ConnectionPhase::Superseded;
            superseded = Some(previous);
        }
        InstallOutcome::Installed { superseded }
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
    #[cfg(any(unix, windows))]
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

    /// Whether `id` is still its device's current authenticated connection.
    ///
    /// The synchronous currentness predicate for operations that only need
    /// to observe the connection lifecycle at the point of a publication or
    /// a guarded install: a superseded, closed, unauthenticated, unknown, or
    /// replaced connection is `false`. Also a stream gate's final
    /// pre-publication check (the caller pairs it with the ownership section
    /// for actual mutations; this predicate is observation only).
    pub(crate) fn is_current_authenticated(&self, id: &ConnectionWireId) -> bool {
        let table = crate::lock_unpoison(&self.inner);
        let Some(record) = table.records.get(id) else {
            return false;
        };
        if record.phase != ConnectionPhase::Authenticated {
            return false;
        }
        let Some(device) = record.paired_device.as_ref() else {
            return false;
        };
        table.device_current.get(device) == Some(id)
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
    #[cfg(any(unix, windows))]
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

    /// The Client incarnation pinned to one connection, if a frame pinned it.
    ///
    /// The pin is written by the first admitted frame only; later frames must
    /// agree or the connection is dropped, so this is Host-observed identity,
    /// never a Client claim.
    pub(crate) fn incarnation_of(&self, id: &ConnectionWireId) -> Option<(u64, u64)> {
        let table = crate::lock_unpoison(&self.inner);
        let record = table.records.get(id)?;
        record
            .incarnation
            .map(|incarnation| (incarnation.counter, incarnation.random))
    }

    /// The current authenticated connection of one Client incarnation, if any.
    ///
    /// Only an authenticated, device-current record is reachability evidence:
    /// a superseded, closed, or unauthenticated socket is never used to
    /// deliver a demand, and its absence is an explicit unreachable hold.
    pub(crate) fn current_connection_for_incarnation(
        &self,
        counter: u64,
        random: u64,
    ) -> Option<ConnectionWireId> {
        let table = crate::lock_unpoison(&self.inner);
        table.records.iter().find_map(|(id, record)| {
            let incarnation = record.incarnation?;
            if incarnation.counter != counter || incarnation.random != random {
                return None;
            }
            if record.phase != ConnectionPhase::Authenticated {
                return None;
            }
            let device = record.paired_device.as_ref()?;
            (table.device_current.get(device) == Some(id)).then_some(*id)
        })
    }

    /// Runs one short synchronous commit under the connection-ownership
    /// section (CCT §10.4).
    ///
    /// The section verifies that `id` is still its device's current
    /// authenticated connection and holds the table for the whole closure,
    /// so a competing authentication install, supersede, or close cannot
    /// interleave between the currentness check and `commit`. Returns
    /// [`None`] — running nothing — when the connection was superseded,
    /// closed, unauthenticated, unknown, or replaced. The closure runs on
    /// the caller's blocking thread; it must not call back into this table
    /// and must not await (it returns a plain value).
    pub(crate) fn with_current_connection<R>(
        &self,
        id: &ConnectionWireId,
        commit: impl FnOnce() -> R,
    ) -> Option<R> {
        let table = crate::lock_unpoison(&self.inner);
        let record = table.records.get(id)?;
        if record.phase != ConnectionPhase::Authenticated {
            return None;
        }
        let device = record.paired_device.as_ref()?;
        if table.device_current.get(device) != Some(id) {
            return None;
        }
        Some(commit())
    }

    /// Test-only: pins one incarnation on an accepted record exactly as the
    /// first admitted frame would, so Client-lifecycle tests need no transport.
    #[cfg(test)]
    pub(crate) fn pin_incarnation_for_tests(
        &self,
        id: &ConnectionWireId,
        counter: u64,
        random: u64,
    ) -> bool {
        let mut table = crate::lock_unpoison(&self.inner);
        let Some(record) = table.records.get_mut(id) else {
            return false;
        };
        if record.incarnation.is_some() {
            return false;
        }
        record.incarnation = Some(ClientIncarnationId { counter, random });
        true
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

    /// [`LiveInput`] snapshot for a connection, without an envelope.
    ///
    /// Mirrors [`ConnectionTable::live_for`]'s premise derivation so the
    /// connection-owned subscription loop and direct handle tests can read
    /// the current premises without fabricating an inbound frame.
    #[cfg(any(unix, windows, test))]
    pub(crate) fn snapshot(self: &Arc<Self>, id: &ConnectionWireId) -> Option<LiveInput> {
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
/// Shared by the device socket and the Host-local control socket: both are
/// single-instance per data directory (the `host.lock` owns that premise) and
/// both must clear a stale path left by a crashed process.
///
/// # Errors
///
/// Returns [`CoreError::Bind`] when a live Host already serves the path, when
/// a stale path cannot be cleared, or when the (re)bind fails.
#[cfg(unix)]
pub(crate) async fn bind_singleton(socket: &Path) -> Result<UnixListener, CoreError> {
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

/// Cancels a Tokio task when dropped.
///
/// `conn::run` keeps the Targeted Deletion driver JoinHandle in this guard
/// for the accept-loop lifetime. Unexpected drop or panic of the listener
/// aborts the driver as a best-effort emergency stop. Graceful shutdown
/// retains this guard throughout [`AbortOnDrop::join`] while a running tick
/// finishes its started Store work. Dropping the join remains an emergency
/// abort. The transport-only reader uses the same guard and can be cancelled
/// safely.
#[must_use = "dropping this guard aborts the owned task"]
#[cfg(any(unix, windows))]
struct AbortOnDrop<T> {
    task: Option<tokio::task::JoinHandle<T>>,
}

#[cfg(any(unix, windows))]
impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(any(unix, windows))]
impl<T> AbortOnDrop<T> {
    /// Awaits the task without aborting it. Used after a shutdown signal so
    /// an already-started bounded tick (and its `spawn_blocking` Store work)
    /// can finish.
    async fn join(mut self) -> Result<(), tokio::task::JoinError> {
        let Some(task) = self.task.as_mut() else {
            return Ok(());
        };
        let result = task.await;
        self.task.take();
        result.map(|_| ())
    }
}

/// A serving-owned stop signal, independent of how the accept loop ends.
#[cfg(any(unix, windows))]
struct DeletionDriver {
    stop: tokio::sync::watch::Sender<bool>,
    task: AbortOnDrop<()>,
}

#[cfg(any(unix, windows))]
impl DeletionDriver {
    async fn stop_and_join(self) -> Result<(), CoreError> {
        self.stop.send_replace(true);
        self.task
            .join()
            .await
            .map_err(|_| CoreError::Serving("deletion driver panicked or was cancelled".into()))
    }
}

#[cfg(any(unix, windows))]
pub(crate) async fn wait_for_shutdown(shutdown: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if *shutdown.borrow_and_update() {
            return;
        }
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(any(unix, windows))]
async fn serving_failure(_handle: &HostHandle) -> CoreError {
    #[cfg(test)]
    {
        _handle.serving_test.fail.notified().await;
        CoreError::Bind("injected serving-loop failure".into())
    }
    #[cfg(not(test))]
    std::future::pending().await
}

/// Owns the handlers of one serving composition. Drop is an emergency abort;
/// normal/error loop exits close admission and drain every started mutation.
#[cfg(any(unix, windows))]
struct ServingHandlers {
    stop: tokio::sync::watch::Sender<bool>,
    tasks: tokio::task::JoinSet<()>,
    failure: Option<CoreError>,
}

/// The Host stores a launcher Arc; only the serving owner may release its
/// runners on emergency drop, including their temporary strong Host Arcs.
#[cfg(any(unix, windows))]
struct TaskAgentOwner<T>(Arc<crate::task_run::BackgroundTaskAgent<T>>);

#[cfg(any(unix, windows))]
impl<T> Drop for TaskAgentOwner<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(any(unix, windows))]
impl ServingHandlers {
    fn new() -> Self {
        let (stop, _) = tokio::sync::watch::channel(false);
        Self {
            stop,
            tasks: tokio::task::JoinSet::new(),
            failure: None,
        }
    }

    fn record(&mut self, result: Option<Result<(), tokio::task::JoinError>>) {
        if let Some(Err(_)) = result {
            // JoinError's panic text can contain provider or target material.
            self.failure.get_or_insert_with(|| {
                CoreError::Serving("serving handler panicked or was cancelled".into())
            });
        }
    }

    async fn stop_and_join(&mut self) -> Result<(), CoreError> {
        self.stop.send_replace(true);
        while let Some(result) = self.tasks.join_next().await {
            self.record(Some(result));
        }
        self.failure.take().map_or(Ok(()), Err)
    }
}

/// Publishes this handle's serving-composition driver liveness from first
/// poll until the driver task is dropped, including abort.
///
/// Liveness is recorded here rather than at `tokio::spawn` so a never-polled
/// aborted task does not look alive, and so Drop of the task future is what
/// clears the count.
#[must_use = "the driver liveness ends when this guard is dropped"]
#[cfg(any(unix, windows))]
struct DeletionDriverLive {
    handle: Arc<HostHandle>,
}

#[cfg(any(unix, windows))]
impl DeletionDriverLive {
    fn enter(handle: Arc<HostHandle>) -> Self {
        handle.begin_deletion_driver();
        Self { handle }
    }
}

#[cfg(any(unix, windows))]
impl Drop for DeletionDriverLive {
    fn drop(&mut self) {
        self.handle.end_deletion_driver();
    }
}

/// Starts the serving-composition Targeted Deletion driver (lifecycle §14).
///
/// The returned guard is the driver's lifetime: `conn::run` must keep it
/// across the accept loop. Graceful shutdown joins it so a running tick
/// finishes; unexpected drop still aborts as an emergency stop. Aborting
/// the driver does not cancel durable operations; a successor Host resumes
/// them from SQLite through startup recovery.
///
/// The driver is the production caller of the bounded fan-out tick
/// ([`HostHandle::run_targeted_deletion_tick`]): each period it runs at most
/// one pass plus at most one backed-off resume of a retryable hold. It is a
/// task rather than `select!` arm of the accept loop because a bounded pass
/// can wait on a Client local-erasure bound
/// ([`crate::transient_erasure`]): accepting a connection must never stall
/// behind erasure work. A failed tick stops nothing and infers no outcome —
/// there is no logging subsystem, the durable operation state stays
/// authoritative, and the next period re-derives it.
#[must_use = "the driver is cancelled when this guard is dropped; bind it for the listener lifetime"]
#[cfg(any(unix, windows))]
fn spawn_targeted_deletion_driver(handle: Arc<HostHandle>) -> DeletionDriver {
    let (stop, mut shutdown) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        let _live = DeletionDriverLive::enter(Arc::clone(&handle));
        #[cfg(test)]
        handle.serving_test.ready.notify_one();
        let mut period = tokio::time::interval_at(
            tokio::time::Instant::now() + DELETION_DRIVE_PERIOD,
            DELETION_DRIVE_PERIOD,
        );
        period.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            if *shutdown.borrow() {
                break;
            }
            tokio::select! {
                biased;
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                    continue;
                }
                () = handle.deletion_driver_wake.notified() => {}
                _ = period.tick() => {}
            }
            if *shutdown.borrow() {
                break;
            }
            // Once this tick starts, graceful shutdown waits for it — including
            // any `spawn_blocking` Store work it already entered.
            drop(handle.run_targeted_deletion_tick().await);
        }
    });
    DeletionDriver {
        stop,
        task: AbortOnDrop { task: Some(task) },
    }
}

/// Serves the Unix socket listener until the process ends.
///
/// Binds [`socket_path`] through the singleton check, proves each peer
/// against the socket owner, and spawns one frame-loop task per authorized
/// connection, driving the fake-friendly [`HostHandle::handle_frame`] seam.
/// The Host-local first-party control endpoint is bound in the same task and
/// served by the same accept loop, so it lives and dies with this listener
/// (one abort releases both). Production keeps an unsignalled shutdown
/// sender, so the 15s Targeted Deletion driver keeps running until the
/// process is killed. Dropping or aborting this future still aborts that
/// driver as an emergency stop; graceful restart uses
/// [`run_until_shutdown`] so a running tick can finish its started Store
/// work. The handle is shared by reference (`Arc` with `&self` methods), so
/// no handle-wide lock spans provider I/O.
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
    let (_keep_alive, shutdown) = tokio::sync::watch::channel(false);
    run_until_shutdown(data_dir, handle, transport, shutdown).await
}

/// Serves the Unix socket listener until `shutdown` is set to `true`.
///
/// Every normal or serving-loop error return is quiescent: accepts have
/// stopped, control and device handlers (including admitted requests, close
/// cleanup and post-response Learning) have joined, Task Agent executions
/// have drained, and the deletion driver has finished its bounded tick and
/// all started deletion Store work. Shutdown interrupts transport waits,
/// never an admitted mutation. An accept error takes precedence over a
/// secondary handler error. Forced drop is an emergency abort, not a
/// graceful restart boundary.
///
/// # Errors
///
/// Returns [`CoreError::Bind`] when the socket cannot be bound (including a
/// live peer) or the socket metadata cannot be read.
#[cfg(unix)]
pub async fn run_until_shutdown<T>(
    data_dir: PathBuf,
    handle: Arc<HostHandle>,
    transport: Arc<T>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
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
    let _ = handle.install_task_launcher(launcher.clone());
    let _task_owner = TaskAgentOwner(Arc::clone(&launcher));
    let table = Arc::new(ConnectionTable::new());
    // The serving composition owns the reachability authority for Client
    // incarnations: without it a Client demand would be an unreachable hold.
    handle.install_client_connection_table(Arc::clone(&table));
    // The Host-local first-party control inlet is bound before the device
    // listener accepts: the Owner's Targeted Deletion confirmation must run
    // in this serving process, where the Client delivery tracking and the
    // connection table are alive (lifecycle §8.1, PR §6.4).
    let control = crate::host_control::ControlListener::bind(&data_dir).await?;
    // Bound to this future: graceful shutdown joins the driver after the
    // current tick; unexpected drop still aborts as an emergency stop.
    let deletion_driver = spawn_targeted_deletion_driver(Arc::clone(&handle));
    let mut handlers = ServingHandlers::new();
    let result = loop {
        if *shutdown.borrow() {
            break Ok(());
        }
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break Ok(());
                }
            }
            error = serving_failure(&handle) => break Err(error),
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(accepted) => accepted,
                    Err(error) => break Err(CoreError::Bind(format!("accept: {error}"))),
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
                let stop = handlers.stop.subscribe();
                handlers.tasks.spawn(async move {
                    serve_connection(stream, connection, handle, transport, table, stop).await;
                });
            }
            accepted = control.accept() => {
                match accepted {
                    Ok(Some(stream)) => {
                        handlers.tasks.spawn(crate::host_control::serve_connection(
                            stream, Arc::clone(&handle), handlers.stop.subscribe(),
                        ));
                    }
                    Ok(None) => {}
                    Err(error) => break Err(error),
                }
            }
            joined = handlers.tasks.join_next(), if !handlers.tasks.is_empty() => {
                handlers.record(joined);
            }
        }
    };
    #[cfg(test)]
    handle.serving_test.shutdown_started.notify_one();
    let handler_result = handlers.stop_and_join().await;
    let task_result = launcher.shutdown_and_join().await;
    let driver_result = deletion_driver.stop_and_join().await;
    result
        .and(handler_result)
        .and(task_result)
        .and(driver_result)
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
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
) -> bool {
    use tokio::io::AsyncWriteExt as _;

    if matches!(response.payload, WirePayload::DisconnectNotice(_)) {
        *terminal = true;
    }
    let Ok(encoded) = encode_frame(&response) else {
        return false;
    };
    tokio::select! {
        biased;
        () = wait_for_shutdown(shutdown) => false,
        result = stream.write_all(&encoded) => result.is_ok(),
    }
}

#[cfg(any(unix, windows))]
/// Reads and decodes frames until the stream ends or a frame is invalid.
///
/// The reader runs as its own task so the connection loop can wait on the
/// inbound frames, the undelivered wakeup hint, and the receipt deadline at
/// the same time: `read_exact` is not cancellation-safe, so the read cannot
/// sit directly in a `select!`. A full channel backpressures the reader (and
/// with it the socket), never the Task runner.
#[cfg(any(unix, windows))]
async fn read_frames<R>(mut read: R, frames: tokio::sync::mpsc::Sender<WireFrame>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    use tokio::io::AsyncReadExt as _;

    let mut prefix = [0_u8; 4];
    loop {
        if read.read_exact(&mut prefix).await.is_err() {
            break;
        }
        let claimed = u32::from_be_bytes(prefix) as usize;
        if claimed > MAX_FRAME_BYTES {
            break;
        }
        let mut body = vec![0_u8; claimed];
        if read.read_exact(&mut body).await.is_err() {
            break;
        }
        let mut bytes = Vec::with_capacity(prefix.len() + body.len());
        bytes.extend_from_slice(&prefix);
        bytes.extend_from_slice(&body);
        let Ok((frame, _)) = decode_frame(&bytes) else {
            break;
        };
        if frames.send(frame).await.is_err() {
            // The connection loop is gone; stop reading.
            break;
        }
    }
}

/// Emits one subscription push for a connection with a captured template.
///
/// Returns `false` when the write failed: further pushes stop, but the
/// connection loop keeps draining inbound frames, because a frame the peer
/// sent before closing (for example a stream's `ConfirmPresentation`) still
/// carries a durable observation that must be applied. The reader's EOF ends
/// the connection.
#[cfg(any(unix, windows))]
async fn emit_push<W>(
    write_half: &mut W,
    handle: &HostHandle,
    table: &Arc<ConnectionTable>,
    connection: &ConnectionWireId,
    template: &Option<(WireFrame, LiveInput)>,
    terminal: &mut bool,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
) -> bool
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let Some((frame, _)) = template else {
        return true;
    };
    let Some(live) = table.snapshot(connection) else {
        return true;
    };
    // A replacement may have landed between the caller's snapshot and this
    // push: never write transient presentation onto a connection that is no
    // longer the current authenticated one. Suppression is safe — the new
    // connection's own subscription serves the row.
    if !table.is_current_authenticated(connection) {
        return true;
    }
    let Some(pushed) = handle.push_undelivered(frame, &live).await else {
        return true;
    };
    write_response(write_half, pushed, terminal, shutdown).await
}

/// Emits one pending Client local-erasure demand on this connection.
///
/// Mirrors [`emit_push`]: the demand is addressed to the connection's pinned
/// incarnation and written under the connection's last admitted frame as the
/// envelope template. The demand is marked delivered before the write; a write
/// failure ends further pushes, and the participant wait reports an explicit
/// hold (never a completion) for this pass, so a lost demand is re-driven
/// idempotently by a later pass.
#[cfg(any(unix, windows))]
async fn emit_client_demand<W>(
    write_half: &mut W,
    handle: &HostHandle,
    table: &Arc<ConnectionTable>,
    connection: &ConnectionWireId,
    template: &Option<(WireFrame, LiveInput)>,
    terminal: &mut bool,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
) -> bool
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let Some((frame, _)) = template else {
        return true;
    };
    let Some(live) = table.snapshot(connection) else {
        return true;
    };
    if !table.is_current_authenticated(connection) {
        return true;
    }
    let Some(payload) = handle.take_client_demand(&live).await else {
        return true;
    };
    write_response(
        write_half,
        outgoing_frame(frame, &live, payload),
        terminal,
        shutdown,
    )
    .await
}

/// An incarnation mismatch, a corrupt or oversize frame, or a terminal
/// [`DisconnectNotice`](ene_api::v1::handshake::DisconnectNotice) in the
/// responses ends the connection; close always forgets the table entry and
/// runs the presence fallback through
/// [`HostHandle::close_connection`](crate::serve::HostHandle::close_connection)
/// when this was the device's current authenticated connection.
///
/// Transport-generic over the byte stream so the Unix socket and the Windows
/// named pipe share this loop, the duplicate suppression, and the phase gate.
///
/// The loop owns the connection lifetime: it processes inbound requests and,
/// on the same lifetime, advances the undelivered subscription on a coalesced
/// registration hint or on the nearest receipt deadline (CCT §10.5). Pushes
/// run the same durable pass machinery as explicit requests, are bounded to
/// one frame each, and cannot park the Task runner; a disconnect or
/// supersession ends the loop and drops the connection-owned state.
async fn serve_connection<S, T>(
    stream: S,
    connection: ConnectionWireId,
    handle: Arc<HostHandle>,
    transport: Arc<T>,
    table: Arc<ConnectionTable>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    T: ProviderTransport + Send + Sync + 'static,
{
    #[cfg(test)]
    handle.serving_test.device_started.notify_one();
    let (read_half, mut write_half) = tokio::io::split(stream);
    let (frames_tx, mut frames_rx) = tokio::sync::mpsc::channel::<WireFrame>(STREAM_BUFFER_FRAMES);
    let reader = AbortOnDrop {
        task: Some(tokio::spawn(read_frames(read_half, frames_tx))),
    };
    let mut learning = tokio::task::JoinSet::new();
    let mut learning_failure = None;
    // Subscribe before the first read: a registration that commits while the
    // loop starts still changes the epoch, so the waiter cannot miss it.
    let mut wake = handle.undelivered_wakeup();
    // The most recent admitted inbound frame and its premises: pushes carry
    // no reply correlation, so they reuse the connection's incarnation and
    // device binding from the last real frame. No frame yet means the
    // connection is still pre-auth, where nothing may be presented.
    let mut template: Option<(WireFrame, LiveInput)> = None;
    let mut terminal = false;
    // A failed push write stops further pushes but never discards inbound
    // frames the peer already sent; the reader's EOF ends the connection.
    let mut push_blocked = false;

    'connection: loop {
        let deadline = handle.receipt_deadline_for(&connection);
        let timer = async {
            match deadline {
                Some(at) => tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            biased;
            () = wait_for_shutdown(&mut shutdown) => break 'connection,
            joined = learning.join_next(), if !learning.is_empty() => {
                if let Some(Err(error)) = joined {
                    learning_failure = Some(error);
                    break 'connection;
                }
            }
            maybe = frames_rx.recv() => {
                let Some(frame) = maybe else {
                    // Reader ended (EOF, invalid frame, or oversize): the
                    // connection is over.
                    break 'connection;
                };
                let live = match table.live_for(&connection, &frame.envelope) {
                    LiveDecision::Ready(live) => live,
                    LiveDecision::Duplicate => continue,
                    LiveDecision::Invalid => break 'connection,
                };
                let frame_template = frame.clone();
                let live_template = live.clone();
                // The handle emits each response as it is decided; this loop
                // writes them while the host future is still running, so an
                // early accept and provider deltas reach the socket before
                // provider completion. The channel is bounded: stream deltas
                // backpressure the provider when the client falls behind
                // instead of queueing without limit.
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
                let mut host_done = false;
                loop {
                    if host_done {
                        while let Ok(response) = frame_rx.try_recv() {
                            if !write_response(&mut write_half, response, &mut terminal, &mut shutdown).await {
                                failed = true;
                                break;
                            }
                        }
                        break;
                    }
                    tokio::select! {
                        biased;
                        () = wait_for_shutdown(&mut shutdown) => {
                            failed = true;
                            break;
                        }
                        () = &mut host => {
                            host_done = true;
                        }
                        maybe = frame_rx.recv() => {
                            match maybe {
                                Some(response) => {
                                    if !write_response(&mut write_half, response, &mut terminal, &mut shutdown).await {
                                        failed = true;
                                        break;
                                    }
                                }
                                None => host_done = true,
                            }
                        }
                    }
                }
                // Stop output backpressure, but never drop an admitted request
                // while its Store work or Learning handoff can still mutate.
                if !host_done {
                    drop(frame_rx);
                    host.await;
                }
                // After request completion, Learning formation runs in its
                // own owned task even if transport output was interrupted.
                // The handler joins it before exiting. `run_pending_learning`
                // serializes and drains, so a second spawn that finds an
                // emptied queue is a cheap no-op.
                if handle.has_pending_learning() {
                    let worker_handle = Arc::clone(&handle);
                    let worker_transport = Arc::clone(&transport);
                    learning.spawn(async move {
                        #[cfg(test)]
                        if worker_handle.serving_test.park_learning.swap(false, std::sync::atomic::Ordering::SeqCst) {
                            worker_handle.serving_test.learning_entered.notify_one();
                            worker_handle.serving_test.learning_release.notified().await;
                        }
                        worker_handle
                            .run_pending_learning(worker_transport.as_ref())
                            .await;
                    });
                }
                if failed || terminal {
                    break 'connection;
                }
                template = Some((frame_template, live_template));
                // A registration hint that fired before this first admitted
                // frame was not lost: advance the subscription now that the
                // connection has an envelope to push under. A push write
                // failure disables pushes but does not end the connection.
                if !push_blocked
                    && !emit_push(
                        &mut write_half,
                        &handle,
                        &table,
                        &connection,
                        &template,
                        &mut terminal,
                        &mut shutdown,
                    )
                    .await
                {
                    push_blocked = true;
                }
                if !push_blocked
                    && !emit_client_demand(
                        &mut write_half,
                        &handle,
                        &table,
                        &connection,
                        &template,
                        &mut terminal,
                        &mut shutdown,
                    )
                    .await
                {
                    push_blocked = true;
                }
            }
            changed = wake.changed() => {
                if changed.is_err() {
                    // The store is gone with the handle; the connection ends.
                    break 'connection;
                }
                if !push_blocked
                    && !emit_push(
                        &mut write_half,
                        &handle,
                        &table,
                        &connection,
                        &template,
                        &mut terminal,
                        &mut shutdown,
                    )
                    .await
                {
                    push_blocked = true;
                }
                if !push_blocked
                    && !emit_client_demand(
                        &mut write_half,
                        &handle,
                        &table,
                        &connection,
                        &template,
                        &mut terminal,
                        &mut shutdown,
                    )
                    .await
                {
                    push_blocked = true;
                }
            }
            () = handle.client_demand_wakeup().notified() => {
                if !push_blocked
                    && !emit_client_demand(
                        &mut write_half,
                        &handle,
                        &table,
                        &connection,
                        &template,
                        &mut terminal,
                        &mut shutdown,
                    )
                    .await
                {
                    push_blocked = true;
                }
            }
            () = timer => {
                // Expiry is durable-state progression, not a socket write:
                // release due receipts even after a push write failure, or
                // the same elapsed deadline would stay readable and the loop
                // would spin. A blocked push only skips the unsolicited
                // write; inbound frames keep their own path below.
                handle.expire_due_receipts(&connection);
                if !push_blocked
                    && !emit_push(
                        &mut write_half,
                        &handle,
                        &table,
                        &connection,
                        &template,
                        &mut terminal,
                        &mut shutdown,
                    )
                    .await
                {
                    push_blocked = true;
                }
                if !push_blocked
                    && !emit_client_demand(
                        &mut write_half,
                        &handle,
                        &table,
                        &connection,
                        &template,
                        &mut terminal,
                        &mut shutdown,
                    )
                    .await
                {
                    push_blocked = true;
                }
            }
        }
    }
    if let Some(task) = &reader.task {
        task.abort();
    }
    let reader_failure = reader
        .join()
        .await
        .err()
        .filter(|error| !error.is_cancelled());
    // Losing transport reachability must not wait for post-response inference.
    // The handler still owns and joins that Learning work before it exits.
    handle.close_connection(&table, connection).await;
    let learning_failure = drain_learning(&mut learning, learning_failure).await;
    #[cfg(test)]
    handle.serving_test.device_finished.notify_one();
    if let Some(error) = learning_failure.or(reader_failure) {
        // Only after all mutation-capable siblings and close cleanup ended
        // may the serving supervisor observe this child failure.
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        std::panic::resume_unwind(Box::new("Learning worker was cancelled"));
    }
}

#[cfg(any(unix, windows))]
async fn drain_learning(
    tasks: &mut tokio::task::JoinSet<()>,
    mut failure: Option<tokio::task::JoinError>,
) -> Option<tokio::task::JoinError> {
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            failure.get_or_insert(error);
        }
    }
    failure
}

/// Serves the Windows named-pipe listener until the process ends.
///
/// Creates the exclusive first server instance for the data directory's pipe
/// name (a live peer fails creation, like the Unix singleton probe — pipe
/// instances vanish with their process, so there is no stale path to unlink),
/// proves each peer with the OS token check, and spawns one frame-loop task
/// per authorized connection over the shared [`HostHandle::handle_frame`]
/// seam. Production keeps an unsignalled shutdown sender, matching Unix:
/// the 15s Targeted Deletion driver keeps running until the process is
/// killed. Dropping or aborting this future still aborts that driver as an
/// emergency stop; graceful restart uses [`run_until_shutdown`] so a running
/// tick can finish its started Store work. The same shutdown regressions
/// exercise the Unix socket and Windows named-pipe transports.
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
    let (_keep_alive, shutdown) = tokio::sync::watch::channel(false);
    run_until_shutdown(data_dir, handle, transport, shutdown).await
}

/// Serves the Windows named-pipe listener until `shutdown` is set to `true`.
///
/// Every normal or serving-loop error return is quiescent: accepts have
/// stopped, control and device handlers (including admitted requests, close
/// cleanup and post-response Learning) have joined, Task Agent executions
/// have drained, and the deletion driver has finished its bounded tick and
/// all started deletion Store work. Shutdown interrupts transport waits,
/// never an admitted mutation. An accept error takes precedence over a
/// secondary handler error. Forced drop is an emergency abort, not a
/// graceful restart boundary.
///
/// # Errors
///
/// Returns [`CoreError::Bind`] when the first pipe instance cannot be created
/// (including a live peer) or a follow-up instance cannot be created.
#[cfg(windows)]
pub async fn run_until_shutdown<T>(
    data_dir: PathBuf,
    handle: Arc<HostHandle>,
    transport: Arc<T>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
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
    let _ = handle.install_task_launcher(launcher.clone());
    let _task_owner = TaskAgentOwner(Arc::clone(&launcher));
    let table = Arc::new(ConnectionTable::new());
    handle.install_client_connection_table(Arc::clone(&table));
    // The Host-local first-party control inlet: same ownership and peer
    // check as the Unix path; the Owner's Targeted Deletion confirmation
    // must run in this serving process (lifecycle §8.1, PR §6.4).
    let mut control = crate::host_control::ControlListener::bind(&data_dir)?;
    // Same serving-composition driver lifetime as the Unix listener.
    let deletion_driver = spawn_targeted_deletion_driver(Arc::clone(&handle));
    let mut handlers = ServingHandlers::new();
    let result: Result<(), CoreError> = async {
        loop {
            if *shutdown.borrow() {
                return Ok(());
            }
            tokio::select! {
                biased;
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return Ok(());
                    }
                }
                error = serving_failure(&handle) => return Err(error),
                connected = server.connect() => {
                    if connected.is_err() {
                        // A failed wait leaves this instance unusable; replace it
                        // rather than serving half-open state.
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
                    let stop = handlers.stop.subscribe();
                    handlers.tasks.spawn(async move {
                        serve_connection(current, connection, handle, transport, table, stop).await;
                    });
                }
                accepted = control.accept() => {
                    if let Some(stream) = accepted? {
                        handlers.tasks.spawn(crate::host_control::serve_connection(
                            stream, Arc::clone(&handle), handlers.stop.subscribe(),
                        ));
                    }
                }
                joined = handlers.tasks.join_next(), if !handlers.tasks.is_empty() => {
                    handlers.record(joined);
                }
            }
        }
    }
    .await;
    #[cfg(test)]
    handle.serving_test.shutdown_started.notify_one();
    let handler_result = handlers.stop_and_join().await;
    let task_result = launcher.shutdown_and_join().await;
    let driver_result = deletion_driver.stop_and_join().await;
    result
        .and(handler_result)
        .and(task_result)
        .and(driver_result)
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

/// # Errors
///
/// Always returns [`CoreError::UnsupportedPlatform`].
#[cfg(not(any(unix, windows)))]
#[expect(
    clippy::unused_async,
    reason = "stub mirrors the async listener signature; no transport exists here"
)]
pub async fn run_until_shutdown(
    _data_dir: PathBuf,
    _handle: Arc<HostHandle>,
    _transport: Arc<impl ProviderTransport>,
    _shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<(), CoreError> {
    Err(CoreError::UnsupportedPlatform("no supported listener"))
}

#[cfg(all(test, unix))]
mod tests;

#[cfg(all(test, any(unix, windows)))]
pub(crate) mod shutdown_tests;
