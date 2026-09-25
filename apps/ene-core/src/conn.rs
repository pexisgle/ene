use std::collections::HashMap;
use std::path::PathBuf;

#[cfg(unix)]
use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};

use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::Message;

use ene_inference::{DispatchAbort, ProviderTransport};

use crate::serve::{CoreError, HostHandle, outgoing_fact, outgoing_frame_pre_auth};
use crate::wss::{self, HostSink, HostStream, HostWebSocket};

const DELETION_DRIVE_PERIOD: std::time::Duration = std::time::Duration::from_secs(15);

use crate::serve::LiveInput;
use ene_api::codec::{DecodedFrame, WireFrame, decode_frame, encode_frame};
use ene_api::v1::envelope::WireEnvelope;
use ene_api::v1::handshake::NegotiatedConnection;
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{ClientIncarnationId, ConnectionWireId, WireMessageId};

const SEEN_MESSAGE_CAP: usize = 128;

/// The application capacity is the bounded business queue plus this bounded
/// read-ahead window on top of it: an application frame that arrives when
/// both are full exceeds the connection's capacity and ends it (IPC §22), and
/// raising this number only moves that boundary.
const READ_AHEAD_FRAMES: usize = 8;

struct BusinessJob {
    frame: DecodedFrame,
    live: LiveInput,
}

#[expect(
    clippy::large_enum_variant,
    reason = "responses are bounded by the wire cap and travel a bounded queue; boxing would add an allocation per frame"
)]
enum BusinessOut {
    Response(WireFrame),
    Done,
    Failed(tokio::task::JoinError),
}

#[derive(Debug, Clone)]
pub(crate) enum LiveDecision {
    Ready(LiveInput),
    Duplicate,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransportClass {
    SameMachine,
    #[expect(
        dead_code,
        reason = "the explicit remote listener arrives with Stage 14; the local listener admits SameMachine only"
    )]
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionPhase {
    Accepted,
    Paired,
    Challenged,
    Authenticated,
    Superseded,
    Closed,
}

impl ConnectionPhase {
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
    class: TransportClass,
    phase: ConnectionPhase,
    preauth_deadline: tokio::time::Instant,
    awaiting_owner_confirmation: bool,
    paired_device: Option<String>,
    incarnation: Option<ClientIncarnationId>,
    negotiated: Option<NegotiatedConnection>,
    nonce: Option<String>,
    seen_messages: std::collections::VecDeque<WireMessageId>,
}

#[derive(Debug, Default)]
pub(crate) struct ConnectionTable {
    inner: StdMutex<ConnectionTableInner>,
}

#[derive(Debug, Default)]
struct ConnectionTableInner {
    records: HashMap<ConnectionWireId, ConnectionRecord>,
    device_current: HashMap<String, ConnectionWireId>,
    admission_stopped: bool,
}

impl ConnectionTableInner {
    fn current_authenticated(
        &self,
        id: &ConnectionWireId,
        phase: ConnectionPhase,
        device: Option<&str>,
    ) -> bool {
        !self.admission_stopped
            && phase == ConnectionPhase::Authenticated
            && device.is_some_and(|device| self.device_current.get(device) == Some(id))
    }
}

impl ConnectionTable {
    pub(crate) fn note_accept(&self, class: TransportClass) -> ConnectionWireId {
        let id = ConnectionWireId(uuid::Uuid::new_v4());
        let mut table = crate::lock_unpoison(&self.inner);
        if table.admission_stopped {
            return id;
        }
        table.records.insert(
            id,
            ConnectionRecord {
                class,
                phase: ConnectionPhase::Accepted,
                preauth_deadline: tokio::time::Instant::now() + wss::AUTH_DEADLINE,
                awaiting_owner_confirmation: false,
                paired_device: None,
                incarnation: None,
                negotiated: None,
                nonce: None,
                seen_messages: std::collections::VecDeque::new(),
            },
        );
        id
    }

    pub(crate) fn stop_admission(&self) {
        crate::lock_unpoison(&self.inner).admission_stopped = true;
    }

    pub(crate) fn admission_open(&self) -> bool {
        !crate::lock_unpoison(&self.inner).admission_stopped
    }

    pub(crate) fn note_paired(&self, id: &ConnectionWireId, device_wire: &str) -> bool {
        let mut table = crate::lock_unpoison(&self.inner);
        if table.admission_stopped {
            return false;
        }
        let Some(record) = table.records.get_mut(id) else {
            return false;
        };
        if record.phase != ConnectionPhase::Accepted || record.paired_device.is_some() {
            return false;
        }
        record.paired_device = Some(device_wire.to_string());
        record.phase = ConnectionPhase::Paired;
        // The owner already confirmed; only the machine-controlled
        // capability/auth exchange may still stall.
        record.preauth_deadline = tokio::time::Instant::now() + wss::AUTH_DEADLINE;
        true
    }

    /// The first pairing request on this connection switches the pre-auth
    /// bound from a machine handshake timeout to the owner-confirmation
    /// limit (IPC §10.2); later resends of the same pending never extend it.
    pub(crate) fn note_awaiting_owner_confirmation(&self, id: &ConnectionWireId) {
        let mut table = crate::lock_unpoison(&self.inner);
        let Some(record) = table.records.get_mut(id) else {
            return;
        };
        if record.awaiting_owner_confirmation {
            return;
        }
        record.awaiting_owner_confirmation = true;
        record.preauth_deadline = tokio::time::Instant::now() + wss::OWNER_CONFIRMATION_LIMIT;
    }

    pub(crate) fn preauth_expired(&self, id: &ConnectionWireId) -> bool {
        let table = crate::lock_unpoison(&self.inner);
        let Some(record) = table.records.get(id) else {
            return false;
        };
        matches!(
            record.phase,
            ConnectionPhase::Accepted | ConnectionPhase::Paired | ConnectionPhase::Challenged
        ) && tokio::time::Instant::now() > record.preauth_deadline
    }

    pub(crate) fn note_challenged(
        &self,
        id: &ConnectionWireId,
        bind_device: Option<&str>,
        terms: NegotiatedConnection,
        nonce: String,
    ) -> ChallengeOutcome {
        let mut table = crate::lock_unpoison(&self.inner);
        if table.admission_stopped {
            return ChallengeOutcome::Unknown;
        }
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
        if table.admission_stopped {
            return NonceAdmission::WrongPhase;
        }
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
        if table.admission_stopped {
            return InstallOutcome::WrongPhase;
        }
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

    pub(crate) fn live_for(
        self: &Arc<Self>,
        id: &ConnectionWireId,
        envelope: &WireEnvelope,
    ) -> LiveDecision {
        let mut table = crate::lock_unpoison(&self.inner);
        if table.admission_stopped {
            return LiveDecision::Invalid;
        }
        let (class, device, phase, negotiated) = {
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
                record.class,
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
        let authed = table.current_authenticated(id, phase, device.as_deref());
        LiveDecision::Ready(LiveInput {
            client_ref,
            connection_live: true,
            peer_uid_ok: class == TransportClass::SameMachine,
            paired_device: device,
            connection_known: true,
            authed,
            connection_id: *id,
            negotiated,
            phase,
            authority: Arc::clone(self),
        })
    }

    pub(crate) fn is_current_authenticated(&self, id: &ConnectionWireId) -> bool {
        let table = crate::lock_unpoison(&self.inner);
        let Some(record) = table.records.get(id) else {
            return false;
        };
        table.current_authenticated(id, record.phase, record.paired_device.as_deref())
    }

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
            table
                .current_authenticated(id, record.phase, record.paired_device.as_deref())
                .then_some(*id)
        })
    }

    pub(crate) fn with_current_connection<R>(
        &self,
        id: &ConnectionWireId,
        commit: impl FnOnce() -> R,
    ) -> Option<R> {
        let table = crate::lock_unpoison(&self.inner);
        let record = table.records.get(id)?;
        if !table.current_authenticated(id, record.phase, record.paired_device.as_deref()) {
            return None;
        }
        Some(commit())
    }

    pub(crate) fn snapshot(self: &Arc<Self>, id: &ConnectionWireId) -> Option<LiveInput> {
        let table = crate::lock_unpoison(&self.inner);
        let record = table.records.get(id)?;
        let device = record.paired_device.clone();
        let authed = table.current_authenticated(id, record.phase, device.as_deref());
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
            peer_uid_ok: record.class == TransportClass::SameMachine,
            paired_device: device,
            connection_known: true,
            authed,
            connection_id: *id,
            negotiated: record.negotiated.clone(),
            phase: record.phase,
            authority: Arc::clone(self),
        })
    }
}

#[cfg(unix)]
const SINGLETON_PROBE_MILLIS: u64 = 200;

#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};

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
struct AbortOnDrop {
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl AbortOnDrop {
    async fn join(mut self) -> Result<(), tokio::task::JoinError> {
        let Some(task) = self.task.as_mut() else {
            return Ok(());
        };
        let result = task.await;
        self.task.take();
        result.map(|_| ())
    }
}

struct CatchUnwind<F>(F);

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

struct DeletionDriver {
    stop: tokio::sync::watch::Sender<bool>,
    task: AbortOnDrop,
}

impl DeletionDriver {
    async fn stop_and_join(self) -> Result<(), CoreError> {
        self.stop.send_replace(true);
        self.task
            .join()
            .await
            .map_err(|_| CoreError::Serving("deletion driver panicked or was cancelled".into()))
    }
}

pub(crate) async fn wait_for_shutdown(shutdown: &mut tokio::sync::watch::Receiver<bool>) {
    drop(shutdown.wait_for(|stop| *stop).await);
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub(crate) struct ShutdownTestBarrier {
    armed: std::sync::atomic::AtomicBool,
    signal_state: tokio::sync::watch::Sender<bool>,
    connection_state: tokio::sync::watch::Sender<bool>,
    release_state: tokio::sync::watch::Sender<bool>,
}

#[cfg(any(test, feature = "test-support"))]
impl Default for ShutdownTestBarrier {
    fn default() -> Self {
        Self {
            armed: std::sync::atomic::AtomicBool::new(false),
            signal_state: tokio::sync::watch::channel(false).0,
            connection_state: tokio::sync::watch::channel(false).0,
            release_state: tokio::sync::watch::channel(false).0,
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ShutdownTestBarrier {
    fn arm(&self) {
        self.armed.store(true, std::sync::atomic::Ordering::SeqCst);
        self.signal_state.send_replace(false);
        self.connection_state.send_replace(false);
        self.release_state.send_replace(false);
    }

    fn note_signal(&self) {
        if self.armed.load(std::sync::atomic::Ordering::SeqCst) {
            self.signal_state.send_replace(true);
        }
    }

    async fn wait_for_signal(&self) {
        wait_for_test_flag(&self.signal_state).await;
    }

    async fn wait_for_connection(&self) {
        wait_for_test_flag(&self.connection_state).await;
    }

    fn note_connection(&self) {
        if self.armed.load(std::sync::atomic::Ordering::SeqCst) {
            self.connection_state.send_replace(true);
        }
    }

    async fn wait_for_release(&self) {
        if self.armed.load(std::sync::atomic::Ordering::SeqCst) {
            wait_for_test_flag(&self.release_state).await;
        }
    }

    fn release(&self) {
        self.release_state.send_replace(true);
    }
}

#[cfg(any(test, feature = "test-support"))]
async fn wait_for_test_flag(state: &tokio::sync::watch::Sender<bool>) {
    let mut state = state.subscribe();
    while !*state.borrow_and_update() {
        if state.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl HostHandle {
    #[doc(hidden)]
    pub fn arm_shutdown_boundary_for_tests(&self) {
        self.shutdown_test_barrier.arm();
    }

    #[doc(hidden)]
    pub async fn wait_shutdown_boundary_signal_for_tests(&self) {
        self.shutdown_test_barrier.wait_for_signal().await;
    }

    #[doc(hidden)]
    pub async fn wait_shutdown_boundary_connection_for_tests(&self) {
        self.shutdown_test_barrier.wait_for_connection().await;
    }

    #[doc(hidden)]
    pub fn release_shutdown_boundary_for_tests(&self) {
        self.shutdown_test_barrier.release();
    }

    pub(crate) fn note_shutdown_admission_signal(&self) {
        self.shutdown_test_barrier.note_signal();
    }

    pub(crate) async fn pause_shutdown_before_abort_for_tests(&self) {
        self.shutdown_test_barrier.wait_for_release().await;
    }

    pub(crate) async fn pause_connection_at_shutdown_for_tests(&self) {
        self.shutdown_test_barrier.note_connection();
        self.shutdown_test_barrier.wait_for_release().await;
    }
}

struct ServingHandlers {
    stop: tokio::sync::watch::Sender<bool>,
    tasks: tokio::task::JoinSet<()>,
    failure: Option<CoreError>,
}

struct TaskAgentOwner<T>(Arc<crate::task_run::BackgroundTaskAgent<T>>);

impl<T> Drop for TaskAgentOwner<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

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

    fn stop_admission(&mut self, table: &ConnectionTable) {
        // The table gate is the synchronous admission linearization. The
        // watch signal then wakes serving connections and requesters so they
        // leave their receive loops before any Host dispatch is aborted.
        table.stop_admission();
        self.stop.send_replace(true);
    }

    async fn join(&mut self) -> Result<(), CoreError> {
        while let Some(result) = self.tasks.join_next().await {
            self.record(Some(result));
        }
        self.failure.take().map_or(Ok(()), Err)
    }
}

#[must_use = "the driver liveness ends when this guard is dropped"]
struct DeletionDriverLive {
    handle: Arc<HostHandle>,
}

impl DeletionDriverLive {
    fn enter(handle: Arc<HostHandle>) -> Self {
        handle.begin_deletion_driver();
        Self { handle }
    }
}

impl Drop for DeletionDriverLive {
    fn drop(&mut self) {
        self.handle.end_deletion_driver();
    }
}

#[must_use = "the driver is cancelled when this guard is dropped; bind it for the listener lifetime"]
fn spawn_targeted_deletion_driver(handle: Arc<HostHandle>) -> DeletionDriver {
    let (stop, mut shutdown) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        let _live = DeletionDriverLive::enter(Arc::clone(&handle));
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

struct ServingComposition<T> {
    launcher: Arc<crate::task_run::BackgroundTaskAgent<T>>,
    table: Arc<ConnectionTable>,
    driver: DeletionDriver,
    handlers: ServingHandlers,
    /// The Host-lifecycle cooperative stop for running dialogue and learning
    /// dispatches: fired once the Host stops admitting work, before the
    /// connection tasks are joined, so a parked provider wait ends through
    /// the inference boundary's abort contract instead of outliving the Host.
    abort: DispatchAbort,
    _task_owner: TaskAgentOwner<T>,
}

impl<T> ServingComposition<T>
where
    T: ProviderTransport + Send + Sync + 'static,
{
    fn start(handle: &Arc<HostHandle>, transport: &Arc<T>) -> Self {
        let launcher = Arc::new(crate::task_run::BackgroundTaskAgent::new(
            Arc::clone(handle),
            Arc::clone(transport),
        ));
        let _ = handle.install_task_launcher(launcher.clone());
        let task_owner = TaskAgentOwner(Arc::clone(&launcher));
        let table = Arc::new(ConnectionTable::default());
        handle.install_client_connection_table(Arc::clone(&table));
        let driver = spawn_targeted_deletion_driver(Arc::clone(handle));
        let handlers = ServingHandlers::new();
        Self {
            launcher,
            table,
            driver,
            handlers,
            abort: DispatchAbort::default(),
            _task_owner: task_owner,
        }
    }

    async fn quiesce(
        mut self,
        handle: &HostHandle,
        result: Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        // The top-level accept loop has already stopped accepting new work.
        // First close the synchronous admission boundary for existing
        // connections and requesters, then cooperatively abort dispatches,
        // and only then join handlers and their owned business work.
        self.handlers.stop_admission(&self.table);
        #[cfg(any(test, feature = "test-support"))]
        handle.note_shutdown_admission_signal();
        #[cfg(any(test, feature = "test-support"))]
        handle.pause_shutdown_before_abort_for_tests().await;
        self.abort.abort();
        let (handler_result, task_result) =
            tokio::join!(self.handlers.join(), self.launcher.shutdown_and_join());
        handle.join_confirmation_tasks().await;
        let driver_result = self.driver.stop_and_join().await;
        result
            .and(handler_result)
            .and(task_result)
            .and(driver_result)
    }
}

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

pub async fn run_until_shutdown<T>(
    data_dir: PathBuf,
    handle: Arc<HostHandle>,
    transport: Arc<T>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<(), CoreError>
where
    T: ProviderTransport + Send + Sync + 'static,
{
    let wss = wss::WssListener::prepare(&data_dir, &handle.cred_store).await?;
    #[cfg(unix)]
    let control = crate::host_control::ControlListener::bind(&data_dir).await?;
    #[cfg(windows)]
    let control = crate::host_control::ControlListener::bind(&data_dir)?;
    #[cfg(unix)]
    let control = control;
    #[cfg(windows)]
    let mut control = control;
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
            accepted = wss.accept() => {
                match accepted {
                    Err(error) => break Err(error),
                    Ok(None) => {}
                    Ok(Some(pending)) => {
                        let mut stop = composition.handlers.stop.subscribe();
                        let abort = composition.abort.clone();
                        let handle = Arc::clone(&handle);
                        let transport = Arc::clone(&transport);
                        let table = Arc::clone(&table);
                        composition.handlers.tasks.spawn(async move {
                            let upgraded = tokio::select! {
                                biased;
                                () = wait_for_shutdown(&mut stop) => None,
                                upgraded = pending.finish() => upgraded,
                            };
                            let Some((socket, permit)) = upgraded else {
                                return;
                            };
                            let connection = table.note_accept(TransportClass::SameMachine);
                            // The business task is owned by this serving task
                            // from its spawn: once the connection ends no new
                            // job can arrive, and the join below keeps the
                            // Host lifecycle in charge of the started
                            // operation, so graceful shutdown quiesces it
                            // instead of detaching it.
                            let (jobs_tx, jobs_rx) =
                                tokio::sync::mpsc::channel::<BusinessJob>(1);
                            let (business_out_tx, business_out_rx) =
                                tokio::sync::mpsc::channel::<BusinessOut>(STREAM_BUFFER_FRAMES);
                            let business_stop = stop.clone();
                            let business = tokio::spawn(run_business(
                                Arc::clone(&handle),
                                transport,
                                jobs_rx,
                                business_out_tx,
                                abort,
                                business_stop,
                            ));
                            let served = CatchUnwind(serve_connection(
                                socket,
                                connection,
                                handle,
                                table,
                                stop,
                                permit,
                                jobs_tx,
                                business_out_rx,
                            ))
                            .await;
                            // Join before any unwind so a business task is
                            // never detached from the Host lifecycle.
                            let joined = business.await;
                            if let Err(payload) = served {
                                std::panic::resume_unwind(payload);
                            }
                            if let Err(error) = joined {
                                if error.is_panic() {
                                    std::panic::resume_unwind(error.into_panic());
                                }
                                std::panic::resume_unwind(Box::new(
                                    "the business task was cancelled",
                                ));
                            }
                        });
                    }
                }
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
    drop(control);
    drop(wss);
    let quiesced = composition.quiesce(&handle, result).await;
    let runtime_cleanup = wss::remove_runtime(&data_dir);
    runtime_cleanup.and(quiesced)
}

use crate::serve::STREAM_BUFFER_FRAMES;

async fn write_response(
    sink: &mut HostSink,
    response: WireFrame,
    terminal: &mut bool,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
) -> bool {
    if matches!(
        response.payload,
        WirePayload::DisconnectNotice(_) | WirePayload::IncompatibleProtocol(_)
    ) {
        *terminal = true;
    }
    let Ok(body) = encode_frame(&response) else {
        return false;
    };
    send_transport_message(sink, Message::Binary(body.into()), shutdown).await
}

async fn send_transport_message(
    sink: &mut HostSink,
    message: Message,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
) -> bool {
    // IPC §10.2 bounds the write wait: a peer that stops reading may not own
    // the connection task forever, and a write past the bound leaves the
    // sink mid-frame, so the connection ends with it.
    tokio::select! {
        biased;
        () = wait_for_shutdown(shutdown) => false,
        result = tokio::time::timeout(wss::WRITE_WAIT, sink.send(message)) => {
            matches!(result, Ok(Ok(())))
        }
    }
}

/// Owns the read half of the connection. Transport control (Ping / Pong),
/// EOF, Close, and read failures are handled here immediately, no matter how
/// far behind the application consumer is: the socket is always polled
/// together with the drain of the bounded `READ_AHEAD_FRAMES` window, so an
/// application backlog can never hide them (IPC §10.2 / §22). Application
/// frames fill the bounded business queue first and this bounded window on
/// top; a frame that arrives when both are full exceeds the connection's
/// capacity and ends the connection as an explicit failure instead of
/// waiting forever or being dropped in silence. `_ended` is dropped when
/// this reader ends; the connection loop observes the closed watch and
/// closes the connection promptly.
async fn read_ws_frames(
    mut stream: HostStream,
    frames: tokio::sync::mpsc::Sender<DecodedFrame>,
    control: tokio::sync::mpsc::Sender<Vec<u8>>,
    last_activity: Arc<StdMutex<tokio::time::Instant>>,
    _ended: tokio::sync::watch::Sender<()>,
) {
    use tokio::sync::mpsc::error::TrySendError;

    let mut read_ahead: std::collections::VecDeque<DecodedFrame> =
        std::collections::VecDeque::new();
    loop {
        let message = tokio::select! {
            biased;
            permit = frames.reserve(), if !read_ahead.is_empty() => match permit {
                Ok(permit) => {
                    if let Some(frame) = read_ahead.pop_front() {
                        permit.send(frame);
                    }
                    continue;
                }
                Err(_) => return,
            },
            message = stream.next() => message,
        };
        match message {
            Some(Ok(Message::Binary(body))) => {
                touch_activity(&last_activity);
                let Ok(frame) = decode_frame(&body) else {
                    return;
                };
                match frames.try_send(frame) {
                    Ok(()) => {}
                    Err(TrySendError::Full(frame)) if read_ahead.len() < READ_AHEAD_FRAMES => {
                        read_ahead.push_back(frame)
                    }
                    Err(TrySendError::Full(_)) | Err(TrySendError::Closed(_)) => return,
                }
            }
            Some(Ok(Message::Ping(payload))) => {
                touch_activity(&last_activity);
                if control.send(payload.to_vec()).await.is_err() {
                    return;
                }
            }
            Some(Ok(Message::Pong(_))) => touch_activity(&last_activity),
            Some(Ok(Message::Text(_)))
            | Some(Ok(Message::Close(_)))
            | Some(Ok(Message::Frame(_)))
            | Some(Err(_))
            | None => return,
        }
    }
}

/// Liveness uses the tokio clock so the monitor and paused-time tests read
/// the same timeline; a transport pong only refreshes this stamp and never
/// counts as presence or a presentation acknowledgement.
fn touch_activity(last_activity: &StdMutex<tokio::time::Instant>) {
    let mut stamp = crate::lock_unpoison(last_activity);
    *stamp = tokio::time::Instant::now();
}

fn activity_idle(last_activity: &StdMutex<tokio::time::Instant>) -> std::time::Duration {
    let stamp = *crate::lock_unpoison(last_activity);
    tokio::time::Instant::now().saturating_duration_since(stamp)
}

#[derive(Clone, Copy)]
enum PendingEmission {
    Subscription,
    ClientDemand,
}

#[expect(
    clippy::too_many_arguments,
    reason = "connection output state is intentionally explicit"
)]
async fn emit_unsolicited(
    write_half: &mut HostSink,
    handle: &HostHandle,
    table: &Arc<ConnectionTable>,
    connection: &ConnectionWireId,
    template: &Option<(WireFrame, LiveInput)>,
    terminal: &mut bool,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    kind: PendingEmission,
) -> bool {
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

#[expect(
    clippy::too_many_arguments,
    reason = "connection output state is intentionally explicit"
)]
async fn advance_output(
    write_half: &mut HostSink,
    handle: &HostHandle,
    table: &Arc<ConnectionTable>,
    connection: &ConnectionWireId,
    template: &Option<(WireFrame, LiveInput)>,
    terminal: &mut bool,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    push_blocked: &mut bool,
) {
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

#[expect(
    clippy::too_many_arguments,
    reason = "connection output state is intentionally explicit"
)]
async fn serve_connection(
    socket: HostWebSocket,
    connection: ConnectionWireId,
    handle: Arc<HostHandle>,
    table: Arc<ConnectionTable>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    _permit: tokio::sync::OwnedSemaphorePermit,
    jobs_tx: tokio::sync::mpsc::Sender<BusinessJob>,
    mut business_out_rx: tokio::sync::mpsc::Receiver<BusinessOut>,
) {
    let Some(mut pairing_provisions) = handle.pairing_deliveries.register(&connection) else {
        handle.close_connection(&table, connection).await;
        return;
    };
    let (mut write_half, read_half) = socket.split();
    let (frames_tx, mut frames_rx) =
        tokio::sync::mpsc::channel::<DecodedFrame>(STREAM_BUFFER_FRAMES);
    let (control_tx, mut control_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
    let (ended_tx, mut ended_rx) = tokio::sync::watch::channel(());
    let last_activity = Arc::new(StdMutex::new(tokio::time::Instant::now()));
    let reader = AbortOnDrop {
        task: Some(tokio::spawn(read_ws_frames(
            read_half,
            frames_tx,
            control_tx,
            Arc::clone(&last_activity),
            ended_tx,
        ))),
    };
    let mut next_ping = tokio::time::Instant::now() + wss::PING_INTERVAL;
    let mut suspected = false;
    let mut held: std::collections::VecDeque<DecodedFrame> = std::collections::VecDeque::new();
    let mut monitor_at = tokio::time::Instant::now() + wss::MONITOR_TICK;
    let mut wake = handle.undelivered_wakeup();
    let mut template: Option<(WireFrame, LiveInput)> = None;
    let mut pending_dispatch: Option<(DecodedFrame, LiveInput)> = None;
    let mut pairing_delivery_open = true;
    let mut terminal = false;
    let mut push_blocked = false;
    let mut busy = false;
    let mut learning_failure: Option<tokio::task::JoinError> = None;

    let body = async {
        'connection: loop {
            let deadline = handle.receipt_deadline_for(&connection);
            let timer = async {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending::<()>().await,
                }
            };
            // Biased order is the connection's progress guarantee: shutdown,
            // liveness/Ping, EOF/Close, transport control, the monotonic
            // receipt deadline, pairing, and dispatchable held work are
            // polled ahead of application ingress. Continuous application
            // frames therefore cannot starve business dispatch or control;
            // business output remains below ingress so streaming cannot
            // monopolize the loop. Every branch consumes its event when it
            // fires.
            tokio::select! {
                biased;
                () = wait_for_shutdown(&mut shutdown) => {
                    #[cfg(any(test, feature = "test-support"))]
                    handle.pause_connection_at_shutdown_for_tests().await;
                    break 'connection;
                }
                () = tokio::time::sleep_until(monitor_at) => {
                    monitor_at = {
                        let now = tokio::time::Instant::now();
                        let mut next = monitor_at + wss::MONITOR_TICK;
                        if next < now {
                            next = now + wss::MONITOR_TICK;
                        }
                        next
                    };
                    let idle = activity_idle(&last_activity);
                    if idle >= wss::LIVENESS_LIMIT {
                        break 'connection;
                    }
                    // Liveness is transport-level only: it never becomes a
                    // presentation ACK or presence, and a suspected connection
                    // holds new inbound work in `held` instead of dispatching it.
                    suspected = idle >= wss::SUSPECT_AFTER;
                    if table.preauth_expired(&connection) {
                        break 'connection;
                    }
                    if tokio::time::Instant::now() >= next_ping {
                        next_ping = tokio::time::Instant::now() + wss::PING_INTERVAL;
                        if !send_transport_message(
                            &mut write_half,
                            Message::Ping(Default::default()),
                            &mut shutdown,
                        )
                        .await
                        {
                            break 'connection;
                        }
                    }
                }
                ended = ended_rx.changed() => {
                    // EOF, Close, and read failures end here promptly, even
                    // while a business handler is still running: the
                    // connection stops being current without waiting for it.
                    drop(ended);
                    break 'connection;
                }
                payload = control_rx.recv() => {
                    let Some(payload) = payload else {
                        break 'connection;
                    };
                    if !send_transport_message(&mut write_half, Message::Pong(payload.into()), &mut shutdown).await {
                        break 'connection;
                    }
                }
                () = timer => {
                    // The receipt deadline is a state transition on the
                    // monotonic clock (IPC §13.3): expiry runs whether or not
                    // a business handler is running. Only publishing what
                    // comes next waits for the handler, so unsolicited output
                    // keeps its send order.
                    handle.expire_due_receipts(&connection);
                    if !busy {
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
                () = std::future::ready(()), if !busy && !suspected && !held.is_empty() => {
                    if *shutdown.borrow() {
                        break 'connection;
                    }
                    let Some(frame) = held.pop_front() else {
                        continue;
                    };
                    let live = match table.live_for(&connection, frame.envelope()) {
                        LiveDecision::Ready(live) => live,
                        LiveDecision::Duplicate => continue,
                        LiveDecision::Invalid => break 'connection,
                    };
                    pending_dispatch = Some((frame.clone(), live.clone()));
                    busy = true;
                    if jobs_tx.send(BusinessJob { frame, live }).await.is_err() {
                        break 'connection;
                    }
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
                maybe = frames_rx.recv(), if !busy => {
                    let Some(frame) = maybe else {
                        break 'connection;
                    };
                    if held.len() >= STREAM_BUFFER_FRAMES {
                        break 'connection;
                    }
                    held.push_back(frame);
                }
                changed = wake.changed(), if !busy => {
                    if changed.is_err() {
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
                () = handle.client_demand_wakeup().notified(), if !busy => {
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
                outgoing = business_out_rx.recv() => {
                    // The channel closes when the business task ends, so a
                    // failed task still ends this connection promptly.
                    let Some(outgoing) = outgoing else {
                        break 'connection;
                    };
                    match outgoing {
                        BusinessOut::Response(response) => {
                            if !write_response(&mut write_half, response, &mut terminal, &mut shutdown).await {
                                break 'connection;
                            }
                        }
                        BusinessOut::Failed(failure) => {
                            learning_failure = Some(failure);
                            break 'connection;
                        }
                        BusinessOut::Done => {
                            if terminal {
                                break 'connection;
                            }
                            if let Some((DecodedFrame::Known(frame_template), live_template)) =
                                pending_dispatch.take()
                            {
                                template = Some((frame_template, live_template));
                            }
                            busy = false;
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
    // Ending the connection closes both business channels: the business task
    // runs its current operation to completion — a client disconnect alone
    // never infers "not executed" or "failed" for work that already started —
    // drains its learning workers, and exits; this serving task joins it
    // before it reports back. Host shutdown fires the cooperative dispatch
    // abort first, so that same completion (post-claim accounting included)
    // is bounded instead of waiting for a provider that never returns. Only
    // delivery ends here, together with the connection record.
    drop(jobs_tx);
    drop(business_out_rx);
    handle.close_connection(&table, connection).await;
    if let Some(payload) = panicked {
        std::panic::resume_unwind(payload);
    }
    if let Some(error) = reader_failure {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        std::panic::resume_unwind(Box::new("the connection reader was cancelled"));
    }
    if let Some(error) = learning_failure {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        std::panic::resume_unwind(Box::new("Learning worker was cancelled"));
    }
}

async fn run_business<T>(
    handle: Arc<HostHandle>,
    transport: Arc<T>,
    mut jobs: tokio::sync::mpsc::Receiver<BusinessJob>,
    business_out: tokio::sync::mpsc::Sender<BusinessOut>,
    abort: DispatchAbort,
    mut admission_stop: tokio::sync::watch::Receiver<bool>,
) where
    T: ProviderTransport + Send + Sync + 'static,
{
    let mut learning = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            biased;
            () = wait_for_shutdown(&mut admission_stop) => break,
            joined = learning.join_next(), if !learning.is_empty() => {
                if let Some(Err(failure)) = joined
                    && business_out.send(BusinessOut::Failed(failure)).await.is_err()
                {
                    break;
                }
            }
            job = jobs.recv() => {
                let Some(job) = job else {
                    break;
                };
                if *admission_stop.borrow() {
                    break;
                }
                run_business_frame(&handle, transport.as_ref(), job, &business_out, &abort).await;
                if handle.has_pending_learning() {
                    let worker_handle = Arc::clone(&handle);
                    let worker_transport = Arc::clone(&transport);
                    let worker_abort = abort.clone();
                    let mut worker_admission_stop = admission_stop.clone();
                    learning.spawn(async move {
                        worker_handle
                            .run_pending_learning(
                                worker_transport.as_ref(),
                                &worker_abort,
                                &mut worker_admission_stop,
                            )
                            .await;
                    });
                }
                if business_out.send(BusinessOut::Done).await.is_err() {
                    break;
                }
            }
        }
    }
    while let Some(joined) = learning.join_next().await {
        if let Err(failure) = joined {
            drop(business_out.send(BusinessOut::Failed(failure)).await);
        }
    }
}

async fn run_business_frame<T: ProviderTransport>(
    handle: &HostHandle,
    transport: &T,
    job: BusinessJob,
    business_out: &tokio::sync::mpsc::Sender<BusinessOut>,
    abort: &DispatchAbort,
) {
    let (frame_tx, frame_rx) = tokio::sync::mpsc::channel::<WireFrame>(STREAM_BUFFER_FRAMES);
    let mut frame_rx = Some(frame_rx);
    let mut host =
        std::pin::pin!(handle.handle_frame_to(job.frame, job.live, transport, &frame_tx, abort));
    let mut done = false;
    loop {
        if done {
            if let Some(rx) = frame_rx.as_mut() {
                while let Ok(response) = rx.try_recv() {
                    if business_out
                        .send(BusinessOut::Response(response))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
            return;
        }
        let Some(rx) = frame_rx.as_mut() else {
            // The connection loop is gone: the operation still runs to
            // completion, but its responses end undelivered.
            host.await;
            return;
        };
        let mut delivery_ended = false;
        tokio::select! {
            biased;
            () = &mut host => done = true,
            response = rx.recv() => match response {
                Some(response) => {
                    if business_out
                        .send(BusinessOut::Response(response))
                        .await
                        .is_err()
                    {
                        delivery_ended = true;
                    }
                }
                None => delivery_ended = true,
            },
        }
        if delivery_ended {
            drop(frame_rx.take());
        }
    }
}

#[cfg(test)]
mod connection_deadline_tests {
    use std::time::Duration;

    use ene_api::v1::envelope::{ProtocolVersion, WireEnvelope, WireSender, new_outgoing_envelope};
    use ene_api::v1::handshake::NegotiatedConnection;
    use ene_api::v1::refs::{ClientIncarnationId, WireMessageType};

    use super::{ChallengeOutcome, ConnectionTable, InstallOutcome, LiveDecision, TransportClass};
    use crate::wss::{AUTH_DEADLINE, OWNER_CONFIRMATION_LIMIT};

    fn envelope() -> WireEnvelope {
        new_outgoing_envelope(
            ProtocolVersion::V1,
            WireSender {
                device_id: None,
                incarnation_id: ClientIncarnationId {
                    counter: 1,
                    random: 2,
                },
                connection_id: None,
            },
            WireMessageType(String::from("TestMessage")),
        )
    }

    #[tokio::test(start_paused = true)]
    async fn the_machine_deadline_bounds_pre_auth_until_the_owner_confirms() {
        let table = ConnectionTable::default();

        let stalled = table.note_accept(TransportClass::SameMachine);
        assert!(!table.preauth_expired(&stalled));
        tokio::time::advance(AUTH_DEADLINE + Duration::from_millis(1)).await;
        assert!(
            table.preauth_expired(&stalled),
            "a silent machine handshake must expire at the auth deadline"
        );

        let waiting = table.note_accept(TransportClass::SameMachine);
        table.note_awaiting_owner_confirmation(&waiting);
        tokio::time::advance(AUTH_DEADLINE + Duration::from_millis(1)).await;
        assert!(
            !table.preauth_expired(&waiting),
            "owner confirmation outlives the machine deadline"
        );
        table.note_awaiting_owner_confirmation(&waiting);
        tokio::time::advance(OWNER_CONFIRMATION_LIMIT).await;
        assert!(
            table.preauth_expired(&waiting),
            "the owner wait is still bounded, and resends never extend it"
        );

        let approved = table.note_accept(TransportClass::SameMachine);
        table.note_awaiting_owner_confirmation(&approved);
        assert!(table.note_paired(&approved, "device-1"));
        tokio::time::advance(AUTH_DEADLINE + Duration::from_millis(1)).await;
        assert!(
            table.preauth_expired(&approved),
            "after the owner confirms, only the machine exchange may stall"
        );

        let authed = table.note_accept(TransportClass::SameMachine);
        assert!(matches!(
            table.note_challenged(
                &authed,
                Some("device-2"),
                NegotiatedConnection {
                    version: ProtocolVersion::V1
                },
                String::from("nonce")
            ),
            ChallengeOutcome::Challenged
        ));
        assert!(matches!(
            table.install_authenticated(&authed),
            InstallOutcome::Installed { .. }
        ));
        tokio::time::advance(OWNER_CONFIRMATION_LIMIT).await;
        assert!(
            !table.preauth_expired(&authed),
            "authenticated connections leave the pre-auth bound"
        );
    }

    #[test]
    fn a_closed_connection_carries_no_liveness_or_deadline_into_the_next_one() {
        let table = std::sync::Arc::new(ConnectionTable::default());
        let first = table.note_accept(TransportClass::SameMachine);
        assert!(matches!(
            table.live_for(&first, &envelope()),
            LiveDecision::Ready(live) if live.peer_uid_ok
        ));
        assert_eq!(table.note_closed(&first, |_| {}), None);
        assert!(matches!(
            table.live_for(&first, &envelope()),
            LiveDecision::Invalid
        ));
        assert!(!table.preauth_expired(&first));

        let second = table.note_accept(TransportClass::SameMachine);
        assert_ne!(first, second, "every accept is its own connection record");
        assert!(matches!(
            table.live_for(&second, &envelope()),
            LiveDecision::Ready(live) if live.peer_uid_ok
        ));
        assert!(
            !table.preauth_expired(&second),
            "the closing connection's elapsed deadline never leaks to the next one"
        );
    }
}
