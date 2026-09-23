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

#[cfg(any(unix, windows))]
use ene_api::v1::envelope::WireEnvelope;
use ene_api::v1::handshake::NegotiatedConnection;
#[cfg(any(unix, windows))]
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{ClientIncarnationId, ConnectionWireId, WireMessageId};
#[cfg(any(unix, windows))]
use ene_plugin_ipc::{MAX_FRAME_BYTES, WireFrame, decode_frame, encode_frame};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};

#[cfg(any(unix, windows))]
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
    phase: ConnectionPhase,
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
}

impl ConnectionTableInner {
    fn current_authenticated(
        &self,
        id: &ConnectionWireId,
        phase: ConnectionPhase,
        device: Option<&str>,
    ) -> bool {
        phase == ConnectionPhase::Authenticated
            && device.is_some_and(|device| self.device_current.get(device) == Some(id))
    }
}

impl ConnectionTable {
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
        let authed = table.current_authenticated(id, phase, device.as_deref());
        LiveDecision::Ready(LiveInput {
            client_ref,
            connection_live: true,
            peer_uid_ok: true,
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

    #[cfg(any(unix, windows))]
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
            peer_uid_ok: true,
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
    async fn join(mut self) -> Result<(), tokio::task::JoinError> {
        let Some(task) = self.task.as_mut() else {
            return Ok(());
        };
        let result = task.await;
        self.task.take();
        result.map(|_| ())
    }
}

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
    drop(shutdown.wait_for(|stop| *stop).await);
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
            _task_owner: task_owner,
        }
    }

    async fn quiesce(
        mut self,
        handle: &HostHandle,
        result: Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let handler_result = self.handlers.stop_and_join().await;
        handle.join_confirmation_tasks().await;
        let task_result = self.launcher.shutdown_and_join().await;
        let driver_result = self.driver.stop_and_join().await;
        result
            .and(handler_result)
            .and(task_result)
            .and(driver_result)
    }
}

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
        let mut bytes = vec![0_u8; prefix.len() + claimed];
        bytes[..prefix.len()].copy_from_slice(&prefix);
        if read.read_exact(&mut bytes[prefix.len()..]).await.is_err() {
            break;
        }
        let Ok((frame, _)) = decode_frame(&bytes) else {
            break;
        };
        if frames.send(frame).await.is_err() {
            break;
        }
    }
}

#[cfg(any(unix, windows))]
#[derive(Clone, Copy)]
enum PendingEmission {
    Subscription,
    ClientDemand,
}

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
                        break 'connection;
                    };
                    let live = match table.live_for(&connection, &frame.envelope) {
                        LiveDecision::Ready(live) => live,
                        LiveDecision::Duplicate => continue,
                        LiveDecision::Invalid => break 'connection,
                    };
                    let frame_template = frame.clone();
                    let live_template = live.clone();
                    let (frame_tx, mut frame_rx) =
                        tokio::sync::mpsc::channel::<WireFrame>(STREAM_BUFFER_FRAMES);
                    let mut host = std::pin::pin!(handle.handle_frame_to(
                        frame,
                        live,
                        transport.as_ref(),
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
                    if !host_done {
                        drop(frame_rx);
                        host.await;
                    }
                    if handle.has_pending_learning() {
                        let worker_handle = Arc::clone(&handle);
                        let worker_transport = Arc::clone(&transport);
                        learning.spawn(async move {
                            worker_handle
                                .run_pending_learning(worker_transport.as_ref())
                                .await;
                        });
                    }
                    if failed || terminal {
                        break 'connection;
                    }
                    template = Some((frame_template, live_template));
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
    if let Some(payload) = panicked {
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
