//! Unix socket listener: accept, same-user check, frame loop, close.
//!
//! The listener binds `ene.sock` inside the data directory and serves one
//! task per connection. Each task reads length-prefixed
//! [`ene_plugin_ipc::WireFrame`] values, runs them through
//! [`HostHandle::handle_frame`], and writes the responses back. A terminal
//! refusal — a
//! [`DisconnectNotice`](ene_api::v1::handshake::DisconnectNotice) or the
//! negotiation-level
//! [`IncompatibleProtocol`](ene_api::v1::reject::IncompatibleProtocol) — is
//! written, then the connection closes.
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

use crate::serve::{CoreError, HostHandle, outgoing_fact, outgoing_frame_pre_auth};

const SOCKET_NAME: &str = "ene.sock";

#[cfg(unix)]
const SINGLETON_PROBE_MILLIS: u64 = 200;

#[cfg(any(unix, windows))]
const DELETION_DRIVE_PERIOD: std::time::Duration = std::time::Duration::from_secs(15);

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

#[cfg(any(unix, windows))]
const SEEN_MESSAGE_CAP: usize = 128;

#[cfg(any(unix, windows))]
#[derive(Debug, Clone)]
pub(crate) enum LiveDecision {
    Ready(LiveInput),
    Duplicate,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionPhase {
    Accepted,
    /// A `PairingProvision` issued a device key on this connection. A
    /// reconnect capability frame binds an existing device and moves straight
    /// to `Challenged`, never through this phase.
    Paired,
    Challenged,
    Authenticated,
    Superseded,
    Closed,
}

impl ConnectionPhase {
    /// Whether this connection was replaced by a newer authentication.
    #[must_use]
    pub fn is_superseded(self) -> bool {
        self == Self::Superseded
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChallengeOutcome {
    Challenged,
    Superseded,
    WrongPhase,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NonceAdmission {
    Nonce(String),
    Missing,
    Superseded,
    WrongPhase,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallOutcome {
    Installed {
        superseded: Option<ConnectionWireId>,
    },
    Superseded,
    WrongPhase,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConnectionRecord {
    phase: ConnectionPhase,
    paired_device: Option<String>,
    incarnation: Option<ClientIncarnationId>,
    negotiated: Option<NegotiatedConnection>,
    nonce: Option<String>,
    seen_messages: std::collections::VecDeque<WireMessageId>,
}

/// Every method takes a short section over the inner maps and never awaits
/// while holding it. Poisoning recovers the committed table: the table's own
/// map mutations cannot panic, but `note_closed`'s `on_fallback` and
/// `with_current_connection`'s `commit` run caller code under the guard, so a
/// panic there poisons the mutex and `lock_unpoison` recovers the maps as
/// committed before the callback ran.
#[derive(Debug, Default)]
pub(crate) struct ConnectionTable {
    inner: StdMutex<ConnectionTableInner>,
}

#[derive(Debug, Default)]
struct ConnectionTableInner {
    records: HashMap<ConnectionWireId, ConnectionRecord>,
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

    pub(crate) fn note_auth_failed(&self, id: &ConnectionWireId) {
        let mut table = crate::lock_unpoison(&self.inner);
        if let Some(record) = table.records.get_mut(id)
            && record.phase == ConnectionPhase::Challenged
        {
            record.phase = ConnectionPhase::Closed;
        }
    }

    #[cfg(any(unix, windows))]
    pub(crate) fn live_for(
        self: &Arc<Self>,
        id: &ConnectionWireId,
        envelope: &WireEnvelope,
    ) -> LiveDecision {
        let mut table = crate::lock_unpoison(&self.inner);
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

    pub(crate) fn phase_of(&self, id: &ConnectionWireId) -> Option<ConnectionPhase> {
        crate::lock_unpoison(&self.inner)
            .records
            .get(id)
            .map(|record| record.phase)
    }

    pub(crate) fn incarnation_of(&self, id: &ConnectionWireId) -> Option<(u64, u64)> {
        let table = crate::lock_unpoison(&self.inner);
        let record = table.records.get(id)?;
        record
            .incarnation
            .map(|incarnation| (incarnation.counter, incarnation.random))
    }

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
            client_ref: device.clone().unwrap_or_else(|| {
                record
                    .incarnation
                    .map(|incarnation| {
                        format!("incarnation-{}-{}", incarnation.counter, incarnation.random)
                    })
                    .unwrap_or_else(|| String::from("unpaired"))
            }),
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

#[must_use = "dropping this guard aborts the owned task"]
#[cfg(any(unix, windows))]
struct AbortOnDrop {
    task: Option<tokio::task::JoinHandle<()>>,
}

#[cfg(any(unix, windows))]
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(any(unix, windows))]
impl AbortOnDrop {
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

/// Awaits an inner future, turning a panic in it into `Err(payload)`.
#[cfg(any(unix, windows))]
struct CatchUnwind<F>(F);

#[cfg(any(unix, windows))]
impl<F: std::future::Future> std::future::Future for CatchUnwind<F> {
    type Output = Result<F::Output, Box<dyn std::any::Any + Send>>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        // SAFETY: `CatchUnwind` is structurally pinned; the inner future is
        // never moved after construction, so projecting the pin is sound.
        let inner = unsafe { self.map_unchecked_mut(|this| &mut this.0) };
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| inner.poll(cx))) {
            Ok(std::task::Poll::Ready(value)) => std::task::Poll::Ready(Ok(value)),
            Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
            Err(payload) => std::task::Poll::Ready(Err(payload)),
        }
    }
}

/// A serving-owned stop signal, independent of how the accept loop ends.
#[cfg(any(unix, windows))]
struct DeletionDriver {
    stop: tokio::sync::watch::Sender<bool>,
    task: AbortOnDrop,
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

#[cfg(any(unix, windows))]
struct ServingHandlers {
    stop: tokio::sync::watch::Sender<bool>,
    tasks: tokio::task::JoinSet<()>,
    failure: Option<CoreError>,
}

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
            drop(handle.run_targeted_deletion_tick().await);
        }
    });
    DeletionDriver {
        stop,
        task: AbortOnDrop { task: Some(task) },
    }
}

/// The transport-generic serving composition both listeners own.
///
/// The launcher, the connection table, the Targeted Deletion driver, and the
/// handler set are transport-independent; only the bind and the accept loop
/// differ between the Unix socket and the Windows named pipe. The Task Agent
/// owner releases the launcher's runners on drop, including emergency abort.
#[cfg(any(unix, windows))]
struct ServingComposition<T> {
    launcher: Arc<crate::task_run::BackgroundTaskAgent<T>>,
    table: Arc<ConnectionTable>,
    driver: DeletionDriver,
    handlers: ServingHandlers,
    _task_owner: TaskAgentOwner<T>,
}

#[cfg(any(unix, windows))]
impl<T> ServingComposition<T>
where
    T: ProviderTransport + Send + Sync + 'static,
{
    /// Starts the composition in the order the accept loop depends on it:
    /// the production Task Agent launcher, the Client-incarnation
    /// reachability table, then the Targeted Deletion driver.
    fn start(handle: &Arc<HostHandle>, transport: &Arc<T>) -> Self {
        let launcher = Arc::new(crate::task_run::BackgroundTaskAgent::new(
            Arc::clone(handle),
            Arc::clone(transport),
        ));
        let _ = handle.install_task_launcher(launcher.clone());
        let task_owner = TaskAgentOwner(Arc::clone(&launcher));
        let table = Arc::new(ConnectionTable::new());
        handle.install_client_connection_table(Arc::clone(&table));
        let driver = spawn_targeted_deletion_driver(Arc::clone(handle));
        let handlers = ServingHandlers::new();
        Self {
            launcher,
            table,
            driver,
            handlers,
            _task_owner: task_owner,
        }
    }

    /// Quiesces the composition: handlers (including admitted requests, close
    /// cleanup, and post-response Learning), then the Owner's confirmation
    /// surface, then the Task Agent executions, then the deletion driver's
    /// bounded tick and started Store work. An earlier failure wins the
    /// `.and` fold. Forced drop remains an emergency abort.
    async fn quiesce(
        mut self,
        handle: &HostHandle,
        result: Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        #[cfg(test)]
        handle.serving_test.shutdown_started.notify_one();
        let handler_result = self.handlers.stop_and_join().await;
        // The Owner's confirmation surface may have an admitted operation still
        // running; it finishes before this process stops owning the authority.
        handle.join_confirmation_tasks().await;
        let task_result = self.launcher.shutdown_and_join().await;
        let driver_result = self.driver.stop_and_join().await;
        result
            .and(handler_result)
            .and(task_result)
            .and(driver_result)
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
    // The Host-local first-party control inlet is bound before the device
    // listener accepts: the Owner's Targeted Deletion confirmation must run
    // in this serving process, where the Client delivery tracking and the
    // connection table are alive (lifecycle §8.1, PR §6.4).
    let control = crate::host_control::ControlListener::bind(&data_dir).await?;
    let mut composition = ServingComposition::start(&handle, &transport);
    let table = Arc::clone(&composition.table);
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
                let stop = composition.handlers.stop.subscribe();
                composition.handlers.tasks.spawn(async move {
                    serve_connection(stream, connection, handle, transport, table, stop).await;
                });
            }
            accepted = control.accept() => {
                match accepted {
                    Ok(Some(stream)) => {
                        let stop = composition.handlers.stop.subscribe();
                        composition.handlers.tasks.spawn(crate::host_control::serve_requester(
                            stream, Arc::clone(&handle), stop,
                        ));
                    }
                    Ok(None) => {}
                    Err(error) => break Err(error),
                }
            }
            joined = composition.handlers.tasks.join_next(), if !composition.handlers.tasks.is_empty() => {
                composition.handlers.record(joined);
            }
        }
    };
    composition.quiesce(&handle, result).await
}

#[cfg(any(unix, windows))]
use crate::serve::STREAM_BUFFER_FRAMES;

#[cfg(any(unix, windows))]
async fn write_response(
    stream: &mut (impl tokio::io::AsyncWrite + Unpin),
    response: WireFrame,
    terminal: &mut bool,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
) -> bool {
    use tokio::io::AsyncWriteExt as _;

    if matches!(
        response.payload,
        WirePayload::DisconnectNotice(_) | WirePayload::IncompatibleProtocol(_)
    ) {
        *terminal = true;
    }
    let Ok(encoded) = encode_frame(&response).map(zeroize::Zeroizing::new) else {
        return false;
    };
    tokio::select! {
        biased;
        () = wait_for_shutdown(shutdown) => false,
        result = stream.write_all(&encoded) => result.is_ok(),
    }
}

#[cfg(any(unix, windows))]
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
            break;
        }
    }
}

/// The unsolicited frame kinds one connection may emit outside a request.
#[cfg(any(unix, windows))]
#[derive(Clone, Copy)]
enum PendingEmission {
    /// One subscription push from the presentation continuation.
    Subscription,
    /// One pending Client local-erasure demand for the pinned incarnation.
    ClientDemand,
}

/// Emits one unsolicited frame of `kind` for a connection with a captured
/// template.
///
/// Returns `false` when the write failed: further pushes stop, but the
/// connection loop keeps draining inbound frames, because a frame the peer
/// sent before closing (for example a stream's `ConfirmPresentation`) still
/// carries a durable observation that must be applied. The reader's EOF ends
/// the connection. A Client demand is marked delivered before the write, so a
/// lost demand is re-driven idempotently (as an explicit hold, never a
/// completion) by a later pass.
#[cfg(any(unix, windows))]
#[expect(
    clippy::too_many_arguments,
    reason = "connection output state is intentionally explicit"
)]
async fn emit_unsolicited<W>(
    write_half: &mut W,
    handle: &HostHandle,
    table: &Arc<ConnectionTable>,
    connection: &ConnectionWireId,
    template: &Option<(WireFrame, LiveInput)>,
    terminal: &mut bool,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    kind: PendingEmission,
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
    let frame = match kind {
        PendingEmission::Subscription => {
            let Some(pushed) = handle.push_undelivered(frame, &live).await else {
                return true;
            };
            pushed
        }
        PendingEmission::ClientDemand => {
            let Some(payload) = handle.take_client_demand(&live).await else {
                return true;
            };
            outgoing_fact(frame, &live, payload)
        }
    };
    write_response(write_half, frame, terminal, shutdown).await
}

/// Advances this connection's unsolicited output: one subscription push and
/// one Client demand, suppressed once a write failure latched `push_blocked`.
#[cfg(any(unix, windows))]
#[expect(
    clippy::too_many_arguments,
    reason = "connection output state is intentionally explicit"
)]
async fn advance_output<W>(
    write_half: &mut W,
    handle: &HostHandle,
    table: &Arc<ConnectionTable>,
    connection: &ConnectionWireId,
    template: &Option<(WireFrame, LiveInput)>,
    terminal: &mut bool,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    push_blocked: &mut bool,
) where
    W: tokio::io::AsyncWrite + Unpin,
{
    if *push_blocked {
        return;
    }
    if !emit_unsolicited(
        write_half,
        handle,
        table,
        connection,
        template,
        terminal,
        shutdown,
        PendingEmission::Subscription,
    )
    .await
    {
        *push_blocked = true;
    }
    if !*push_blocked
        && !emit_unsolicited(
            write_half,
            handle,
            table,
            connection,
            template,
            terminal,
            shutdown,
            PendingEmission::ClientDemand,
        )
        .await
    {
        *push_blocked = true;
    }
}

/// An incarnation mismatch, a corrupt or oversize frame, or a terminal
/// refusal — [`DisconnectNotice`](ene_api::v1::handshake::DisconnectNotice) or
/// [`IncompatibleProtocol`](ene_api::v1::reject::IncompatibleProtocol) — in
/// the responses ends the connection; close always forgets the table entry and
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
    let Some(mut pairing_provisions) = handle.pairing_deliveries.register(&connection) else {
        handle.close_connection(&table, connection).await;
        return;
    };
    let (read_half, mut write_half) = tokio::io::split(stream);
    let (frames_tx, mut frames_rx) = tokio::sync::mpsc::channel::<WireFrame>(STREAM_BUFFER_FRAMES);
    let reader = AbortOnDrop {
        task: Some(tokio::spawn(read_frames(read_half, frames_tx))),
    };
    let mut learning = tokio::task::JoinSet::new();
    let mut learning_failure = None;
    let mut wake = handle.undelivered_wakeup();
    let mut template: Option<(WireFrame, LiveInput)> = None;
    let mut pairing_delivery_open = true;
    let mut terminal = false;
    let mut push_blocked = false;

    let body = async {
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
                            Some(response) = frame_rx.recv() => {
                                if !write_response(&mut write_half, response, &mut terminal, &mut shutdown).await {
                                    failed = true;
                                    break;
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
                    advance_output(
                        &mut write_half,
                        &handle,
                        &table,
                        &connection,
                        &template,
                        &mut terminal,
                        &mut shutdown,
                        &mut push_blocked,
                    )
                    .await;
                }
                provision = pairing_provisions.recv(), if pairing_delivery_open => {
                    let Some(provision) = provision else {
                        pairing_delivery_open = false;
                        continue;
                    };
                    let Some((frame, live)) = template.as_ref() else {
                        break 'connection;
                    };
                    if !matches!(frame.payload, WirePayload::PairingRequest(_)) {
                        break 'connection;
                    }
                    let device_wire = provision.device_id.0.as_hyphenated().to_string();
                    if !table.note_paired(&connection, &device_wire) {
                        break 'connection;
                    }
                    let response = outgoing_frame_pre_auth(
                        frame,
                        live,
                        WirePayload::PairingProvision(provision),
                    );
                    if !write_response(&mut write_half, response, &mut terminal, &mut shutdown).await {
                        break 'connection;
                    }
                }
                changed = wake.changed() => {
                    if changed.is_err() {
                        // The store is gone with the handle; the connection ends.
                        break 'connection;
                    }
                    advance_output(
                        &mut write_half,
                        &handle,
                        &table,
                        &connection,
                        &template,
                        &mut terminal,
                        &mut shutdown,
                        &mut push_blocked,
                    )
                    .await;
                }
                () = handle.client_demand_wakeup().notified() => {
                    if !push_blocked
                        && !emit_unsolicited(
                            &mut write_half,
                            &handle,
                            &table,
                            &connection,
                            &template,
                            &mut terminal,
                            &mut shutdown,
                            PendingEmission::ClientDemand,
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
                    advance_output(
                        &mut write_half,
                        &handle,
                        &table,
                        &connection,
                        &template,
                        &mut terminal,
                        &mut shutdown,
                        &mut push_blocked,
                    )
                    .await;
                }
            }
        }
    };
    let panicked = CatchUnwind(body).await.err();
    if let Some(task) = &reader.task {
        task.abort();
    }
    let reader_failure = reader
        .join()
        .await
        .err()
        .filter(|error| !error.is_cancelled());
    handle.close_connection(&table, connection).await;
    let learning_failure = drain_learning(&mut learning, learning_failure).await;
    #[cfg(test)]
    handle.serving_test.device_finished.notify_one();
    if let Some(payload) = panicked {
        // The teardown above always ran; surface the handler panic through
        // the task `JoinError` exactly as before.
        std::panic::resume_unwind(payload);
    }
    if let Some(error) = learning_failure.or(reader_failure) {
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

    let pipe = ene_plugin_ipc::pipe_name(&data_dir);
    let mut server = crate::conn_pipe::create_first_server(&pipe)?;
    // The Host-local first-party control inlet: same ownership and peer
    // check as the Unix path; the Owner's Targeted Deletion confirmation
    // must run in this serving process (lifecycle §8.1, PR §6.4).
    let mut control = crate::host_control::ControlListener::bind(&data_dir)?;
    let mut composition = ServingComposition::start(&handle, &transport);
    let table = Arc::clone(&composition.table);
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
                        server = crate::conn_pipe::create_next_server(&pipe)?;
                        continue;
                    }
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
                    let stop = composition.handlers.stop.subscribe();
                    composition.handlers.tasks.spawn(async move {
                        serve_connection(current, connection, handle, transport, table, stop).await;
                    });
                }
                accepted = control.accept() => {
                    if let Some(stream) = accepted? {
                        let stop = composition.handlers.stop.subscribe();
                        composition.handlers.tasks.spawn(crate::host_control::serve_requester(
                            stream, Arc::clone(&handle), stop,
                        ));
                    }
                }
                joined = composition.handlers.tasks.join_next(), if !composition.handlers.tasks.is_empty() => {
                    composition.handlers.record(joined);
                }
            }
        }
    }
    .await;
    composition.quiesce(&handle, result).await
}

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
