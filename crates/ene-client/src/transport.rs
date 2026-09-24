use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use ene_api::codec::{
    CodecError, DecodedFrame, MAX_FRAME_BYTES, WireFrame, decode_frame, encode_frame,
};
use ene_api::runtime::HostRuntimeInfo;
use ene_api::v1::deletion::{DeletionDemand, LocalErasureResult};
use ene_api::v1::envelope::{ProtocolVersion, WireEnvelope, WireSender};
use ene_api::v1::handshake::{AuthChallenge, PairingProvisionSecret, PairingResult};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::WireMessageId;
use ene_api::v1::reject::IncompatibleProtocol;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::device;
use crate::error::ClientError;

use super::frames::{
    PreparedRequest, capability_frame, frame_for, pairing_frame, proof_frame,
    unreadable_device_file_guidance,
};
use super::session::{
    AuthDecision, FrameDecision, SessionState, decide_auth, decide_frame, stale_generation_of,
};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);
const UPGRADE_TIMEOUT: Duration = Duration::from_secs(20);
/// Mirrors the Host's per-connection frame buffer; both directions stay
/// bounded so a slow peer applies backpressure instead of buffering. The
/// application capacity is this queue plus `READ_AHEAD_FRAMES` on top: a
/// frame past that boundary fails the transport instead of buffering more.
const TRANSPORT_QUEUE: usize = 32;
const CONTROL_QUEUE: usize = 8;
/// Mirrors the Host's read-ahead window: while the application queue is full
/// the reader keeps consuming transport control frames out of this bounded
/// window instead of stalling the socket, and a frame that arrives when both
/// the queue and this window are full exceeds the connection's capacity.
const READ_AHEAD_FRAMES: usize = 8;
/// IPC §10.2 bounds the write wait; a Host that stops reading must not own
/// the pump task indefinitely, and a write past the bound fails the
/// connection because the sink may be left mid-frame.
const WRITE_WAIT: Duration = Duration::from_secs(30);

type WsClient =
    tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>;

fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .read_buffer_size(16 * 1024)
        .write_buffer_size(16 * 1024)
        .max_write_buffer_size(MAX_FRAME_BYTES)
        .max_message_size(Some(MAX_FRAME_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES))
}

/// A terminal failure the pump stored for the application: `read_ahead`
/// holds the bounded frames that were buffered before the failure, so
/// `Transport::read()` surfaces that data first and the reason last without
/// the reader ever waiting for an application drain.
struct Terminal {
    read_ahead: VecDeque<DecodedFrame>,
    reason: Failure,
}

enum Failure {
    Codec(String),
    Transport(String),
}

impl Failure {
    fn into_client_error(self) -> ClientError {
        match self {
            Self::Codec(reason) => ClientError::Codec(reason),
            Self::Transport(reason) => ClientError::Transport(reason),
        }
    }
}

/// Transport-control observations of the pump: host pings and pongs are
/// recorded as they pass through, and a test can request a transport ping
/// through the writer. The application never drains through this to make a
/// failure surface.
#[derive(Clone)]
pub struct TransportProbe {
    inner: Arc<TransportProbeInner>,
}

struct TransportProbeInner {
    host_pings: AtomicU64,
    pongs_sent: AtomicU64,
    pongs: StdMutex<VecDeque<Vec<u8>>>,
    pings: StdMutex<VecDeque<Vec<u8>>>,
    wake: tokio::sync::Notify,
}

impl TransportProbe {
    fn new() -> Self {
        Self {
            inner: Arc::new(TransportProbeInner {
                host_pings: AtomicU64::new(0),
                pongs_sent: AtomicU64::new(0),
                pongs: StdMutex::new(VecDeque::new()),
                pings: StdMutex::new(VecDeque::new()),
                wake: tokio::sync::Notify::new(),
            }),
        }
    }

    fn note_host_ping(&self) {
        self.inner.host_pings.fetch_add(1, Ordering::Relaxed);
    }

    fn note_pong_sent(&self) {
        self.inner.pongs_sent.fetch_add(1, Ordering::Relaxed);
    }

    fn note_pong(&self, payload: &[u8]) {
        let mut pongs = lock_probe(&self.inner.pongs);
        while pongs.len() >= 32 {
            pongs.pop_front();
        }
        pongs.push_back(payload.to_vec());
    }

    async fn ping_ready(&self) {
        self.inner.wake.notified().await;
    }

    fn take_ping(&self) -> Option<Vec<u8>> {
        lock_probe(&self.inner.pings).pop_front()
    }

    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn host_pings(&self) -> u64 {
        self.inner.host_pings.load(Ordering::Relaxed)
    }

    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn pongs_sent(&self) -> u64 {
        self.inner.pongs_sent.load(Ordering::Relaxed)
    }

    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn take_pongs(&self) -> Vec<Vec<u8>> {
        lock_probe(&self.inner.pongs).drain(..).collect()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn send_ping(&self, payload: &[u8]) {
        lock_probe(&self.inner.pings).push_back(payload.to_vec());
        self.inner.wake.notify_one();
    }
}

fn lock_probe<T>(mutex: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Owns the WebSocket through a background pump, so Ping/Pong and inbound
/// frames are processed whenever the socket has them — independent of
/// whether an upper layer is inside a request or `next_frame()`.
struct Transport {
    inbound: mpsc::Receiver<DecodedFrame>,
    terminal: mpsc::Receiver<Terminal>,
    outbound: mpsc::Sender<Vec<u8>>,
    /// Frames the ended reader handed over before its terminal reason; they
    /// surface once the bounded inbound queue is drained.
    staged: VecDeque<DecodedFrame>,
    pending_failure: Option<Failure>,
    #[cfg(any(test, feature = "test-support"))]
    probe: TransportProbe,
    #[cfg(test)]
    reader_task: tokio::task::JoinHandle<()>,
    #[cfg(test)]
    writer_task: tokio::task::JoinHandle<()>,
}

impl Transport {
    fn spawn(socket: WsClient) -> Self {
        Self::spawn_stream(socket)
    }

    fn spawn_stream<S>(socket: WebSocketStream<S>) -> Self
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (inbound_tx, inbound) = mpsc::channel(TRANSPORT_QUEUE);
        let (outbound, outbound_rx) = mpsc::channel(TRANSPORT_QUEUE);
        let (control, control_rx) = mpsc::channel(CONTROL_QUEUE);
        // One terminal per connection: the reader and the writer store their
        // failure here without awaiting the application, and this capacity
        // bounds that hand-over.
        let (terminal, terminal_rx) = mpsc::channel(1);
        let (writer_gone, writer_gone_rx) = tokio::sync::watch::channel(());
        let probe = TransportProbe::new();
        let (sink, stream) = socket.split();
        // The task handles only exist for test observation; in production the
        // pump is detached as soon as it is spawned.
        let _writer_task = tokio::spawn(write_half(
            sink,
            outbound_rx,
            control_rx,
            terminal.clone(),
            probe.clone(),
            writer_gone,
        ));
        let _reader_task = tokio::spawn(read_half(
            stream,
            inbound_tx,
            control,
            terminal,
            probe.clone(),
            writer_gone_rx,
        ));
        Self {
            inbound,
            terminal: terminal_rx,
            outbound,
            staged: VecDeque::new(),
            pending_failure: None,
            #[cfg(any(test, feature = "test-support"))]
            probe,
            #[cfg(test)]
            reader_task: _reader_task,
            #[cfg(test)]
            writer_task: _writer_task,
        }
    }

    #[cfg(test)]
    fn outbound_for_tests(&self) -> mpsc::Sender<Vec<u8>> {
        self.outbound.clone()
    }

    async fn write(&self, frame: &WireFrame) -> Result<(), ClientError> {
        let body = encode_frame(frame)
            .map_err(|error: CodecError| ClientError::Codec(format!("encode failed: {error}")))?;
        self.outbound
            .send(body)
            .await
            .map_err(|_| ClientError::Transport(String::from("the connection to the Host ended")))
    }

    async fn read(&mut self) -> Result<DecodedFrame, ClientError> {
        loop {
            if let Some(frame) = self.staged.pop_front() {
                return Ok(frame);
            }
            if let Some(failure) = self.pending_failure.take() {
                return Err(failure.into_client_error());
            }
            if let Some(frame) = self.inbound.recv().await {
                return Ok(frame);
            }
            // The reader has already ended without waiting for this consumer;
            // serve what it buffered first and its terminal reason last.
            match self.terminal.recv().await {
                Some(terminal) => {
                    self.staged = terminal.read_ahead;
                    self.pending_failure = Some(terminal.reason);
                }
                None => {
                    return Err(ClientError::Transport(String::from(
                        "the connection to the Host ended",
                    )));
                }
            }
        }
    }

    async fn read_known(&mut self, stage: &str) -> Result<WireFrame, ClientError> {
        match self.read().await? {
            DecodedFrame::Known(frame) => Ok(frame),
            DecodedFrame::Unsupported { reason, .. } => Err(ClientError::ServerRejected(format!(
                "unsupported message during {stage}: {}",
                reason.notice().detail
            ))),
        }
    }

    async fn read_handshake(&mut self, stage: &str) -> Result<WireFrame, ClientError> {
        tokio::time::timeout(HANDSHAKE_TIMEOUT, self.read_known(stage))
            .await
            .map_err(|_| {
                ClientError::Transport(format!("the Host did not answer the {stage} step in time"))
            })?
    }
}

/// Owns the write half: transport Pongs, requested transport Pings, and
/// application frames are written here under one bounded write wait, so
/// neither a Host that stops reading nor a stalled application can own this
/// task forever. A write failure is stored in the terminal slot without
/// waiting for the application, and `_writer_gone` is dropped when this
/// writer ends; the reader observes the closed watch and stops with it.
async fn write_half<S>(
    sink: futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
    mut outbound: mpsc::Receiver<Vec<u8>>,
    mut control: mpsc::Receiver<Vec<u8>>,
    terminal: mpsc::Sender<Terminal>,
    probe: TransportProbe,
    _writer_gone: tokio::sync::watch::Sender<()>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + 'static,
{
    let mut sink = sink;
    loop {
        tokio::select! {
            biased;
            pong = control.recv() => {
                let Some(payload) = pong else {
                    // The reader ended; it surfaced why.
                    return;
                };
                probe.note_pong_sent();
                if !send_bounded(&mut sink, Message::Pong(payload.into())).await {
                    surface_write_failure(&terminal);
                    return;
                }
            }
            () = probe.ping_ready() => {
                while let Some(payload) = probe.take_ping() {
                    if !send_bounded(&mut sink, Message::Ping(payload.into())).await {
                        surface_write_failure(&terminal);
                        return;
                    }
                }
            }
            next = outbound.recv() => {
                let Some(body) = next else {
                    // The application is gone; the reader closes the socket.
                    return;
                };
                if !send_bounded(&mut sink, Message::Binary(body.into())).await {
                    surface_write_failure(&terminal);
                    return;
                }
            },
        }
    }
}

async fn send_bounded<S>(
    sink: &mut futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
    message: Message,
) -> bool
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    matches!(
        tokio::time::timeout(WRITE_WAIT, sink.send(message)).await,
        Ok(Ok(()))
    )
}

fn surface_write_failure(terminal: &mpsc::Sender<Terminal>) {
    let reason = Failure::Transport(String::from(
        "websocket write failed: the Host stopped accepting data",
    ));
    fail_with(VecDeque::new(), terminal, reason);
}

/// Owns the read half: Ping → Pong, Close, EOF, and read failures are
/// handled here immediately, and application frames are handed to the
/// inbound queue through the bounded `READ_AHEAD_FRAMES` window, so a slow
/// application consumer applies backpressure to the socket instead of
/// stopping the transport control plane. A frame that arrives with the
/// queue and the window both full exceeds the connection's capacity and
/// ends the transport instead of waiting forever or being dropped in
/// silence. Terminal conditions hand their buffered frames and the reason to
/// the terminal slot in one non-blocking step and end this task, so reaching
/// the terminal state never depends on the application draining anything.
async fn read_half<S>(
    stream: futures_util::stream::SplitStream<WebSocketStream<S>>,
    inbound: mpsc::Sender<DecodedFrame>,
    control: mpsc::Sender<Vec<u8>>,
    terminal: mpsc::Sender<Terminal>,
    probe: TransportProbe,
    mut writer_gone: tokio::sync::watch::Receiver<()>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::sync::mpsc::error::TrySendError;

    let mut stream = stream;
    let mut read_ahead: VecDeque<DecodedFrame> = VecDeque::new();
    loop {
        // The socket is always polled together with the drain of the
        // read-ahead window, so no backlog can hide control frames.
        let next = tokio::select! {
            biased;
            gone = writer_gone.changed() => {
                drop(gone);
                return;
            }
            permit = inbound.reserve(), if !read_ahead.is_empty() => match permit {
                Ok(permit) => {
                    if let Some(frame) = read_ahead.pop_front() {
                        permit.send(frame);
                    }
                    continue;
                }
                Err(_) => return,
            },
            next = stream.next() => next,
        };
        match next {
            Some(Ok(Message::Binary(body))) => {
                let frame = match decode_frame(&body) {
                    Ok(frame) => frame,
                    Err(error) => {
                        let reason = format!("decode failed: {error}");
                        fail_with(read_ahead, &terminal, Failure::Codec(reason));
                        return;
                    }
                };
                match inbound.try_send(frame) {
                    Ok(()) => {}
                    Err(TrySendError::Full(frame)) if read_ahead.len() < READ_AHEAD_FRAMES => {
                        read_ahead.push_back(frame)
                    }
                    Err(TrySendError::Full(_)) => {
                        // The application is over the bounded capacity: end
                        // the transport with an explicit failure instead of
                        // stalling the pump or dropping the frame in silence.
                        fail_with(
                            read_ahead,
                            &terminal,
                            Failure::Transport(String::from(
                                "the application backlog exceeded the bounded inbound queue and read-ahead window",
                            )),
                        );
                        return;
                    }
                    Err(TrySendError::Closed(_)) => return,
                }
            }
            Some(Ok(Message::Ping(payload))) => {
                probe.note_host_ping();
                if control.send(payload.to_vec()).await.is_err() {
                    return;
                }
            }
            Some(Ok(Message::Pong(payload))) => probe.note_pong(&payload),
            Some(Ok(Message::Text(_))) => {
                fail_with(
                    read_ahead,
                    &terminal,
                    Failure::Transport(String::from(
                        "text frames are not part of the wire protocol",
                    )),
                );
                return;
            }
            Some(Ok(Message::Close(_))) => {
                fail_with(
                    read_ahead,
                    &terminal,
                    Failure::Transport(String::from("the Host closed the connection")),
                );
                return;
            }
            Some(Ok(Message::Frame(_))) | None => {
                fail_with(
                    read_ahead,
                    &terminal,
                    Failure::Transport(String::from("the connection to the Host ended")),
                );
                return;
            }
            Some(Err(error)) => {
                let reason = format!("websocket read failed: {error}");
                fail_with(read_ahead, &terminal, Failure::Transport(reason));
                return;
            }
        }
    }
}

/// Stores the buffered frames together with the terminal reason so
/// `Transport::read()` can surface the data first and the failure last.
/// Nothing here awaits the application. Only one terminal fits the slot; a
/// losing race keeps the already-stored terminal, so a failure is never
/// dropped in silence.
fn fail_with(
    read_ahead: VecDeque<DecodedFrame>,
    terminal: &mpsc::Sender<Terminal>,
    reason: Failure,
) {
    drop(terminal.try_send(Terminal { read_ahead, reason }));
}

async fn connect_ws(runtime: &HostRuntimeInfo, pin: &str) -> Result<Transport, ClientError> {
    let port = runtime.local_port().ok_or_else(|| {
        ClientError::Transport(String::from(
            "the Host runtime file does not describe a local wss listener",
        ))
    })?;
    let tcp = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|error| {
            ClientError::Transport(format!(
                "connect to the local Host failed: {}",
                error.kind()
            ))
        })?;
    let tls_config = crate::host_pin::pinned_tls_config(pin)
        .map_err(|error| ClientError::Transport(format!("TLS configuration failed: {error}")))?;
    let connector = tokio_rustls::TlsConnector::from(Arc::new(tls_config));
    let server_name = rustls::pki_types::ServerName::IpAddress(
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST).into(),
    );
    let tls = tokio::time::timeout(UPGRADE_TIMEOUT, connector.connect(server_name, tcp))
        .await
        .map_err(|_| ClientError::Transport(String::from("TLS handshake with the Host timed out")))?
        .map_err(|error| {
            ClientError::Transport(format!("TLS handshake with the Host failed: {error}"))
        })?;
    let mut request = format!("wss://127.0.0.1:{port}/")
        .into_client_request()
        .map_err(|error| {
            ClientError::Transport(format!("build the local Host request: {error}"))
        })?;
    let authorization = format!("Bearer {}", runtime.local_token)
        .parse::<tokio_tungstenite::tungstenite::http::HeaderValue>()
        .map_err(|_| ClientError::Transport(String::from("the local token is not header-safe")))?;
    request.headers_mut().insert("authorization", authorization);
    let generation = runtime
        .startup_generation
        .parse::<tokio_tungstenite::tungstenite::http::HeaderValue>()
        .map_err(|_| {
            ClientError::Transport(String::from("the startup generation is not header-safe"))
        })?;
    request
        .headers_mut()
        .insert("x-ene-startup-generation", generation);
    let (socket, _response) = tokio::time::timeout(
        UPGRADE_TIMEOUT,
        tokio_tungstenite::client_async_with_config(request, tls, Some(ws_config())),
    )
    .await
    .map_err(|_| {
        ClientError::Transport(String::from(
            "the Host did not complete the WebSocket upgrade in time",
        ))
    })?
    .map_err(|error| {
        ClientError::Transport(format!("the Host refused the WebSocket upgrade: {error}"))
    })?;
    Ok(Transport::spawn(socket))
}

pub struct Client {
    transport: Transport,
    sender: WireSender,
    state: SessionState,
}

pub enum ConnectProgress {
    Connected(Client),
    Pending(PendingPairingClient),
}

pub struct PendingPairingClient {
    transport: Transport,
    incarnation: ene_api::v1::refs::ClientIncarnationId,
    data_dir: PathBuf,
    platform: String,
    pending_id: String,
    pairing_message_id: WireMessageId,
}

impl PendingPairingClient {
    #[must_use]
    pub fn pending_id(&self) -> &str {
        &self.pending_id
    }

    pub async fn complete(self) -> Result<Client, ClientError> {
        let Self {
            mut transport,
            incarnation,
            data_dir,
            platform,
            pending_id: _,
            pairing_message_id,
        } = self;
        let provision_frame = transport.read_known("pairing provision").await?;
        require_reply_to(&provision_frame, pairing_message_id, "pairing provision")?;
        let WirePayload::PairingProvision(provision) = provision_frame.payload else {
            return Err(ClientError::ServerRejected(format!(
                "unexpected {} while awaiting pairing provision; start a fresh pairing request",
                provision_frame.payload.message_type()
            )));
        };
        let device_id = provision.device_id;
        let secret = provision.pairing_secret;
        finish_connect(
            transport,
            incarnation,
            &data_dir,
            &platform,
            device_id,
            secret,
            true,
        )
        .await
    }
}

impl Client {
    pub async fn connect(
        data_dir: &Path,
        descriptor: &str,
        platform: &str,
    ) -> Result<Self, ClientError> {
        match Self::begin_connect(data_dir, descriptor, platform).await? {
            ConnectProgress::Connected(client) => Ok(client),
            ConnectProgress::Pending(pending) => pending.complete().await,
        }
    }

    pub async fn begin_connect(
        data_dir: &Path,
        descriptor: &str,
        platform: &str,
    ) -> Result<ConnectProgress, ClientError> {
        let incarnation = crate::incarnation::boot_incarnation(data_dir)?;
        let runtime = crate::runtime_info::load_host_runtime(data_dir)?;
        let pin = crate::host_pin::establish_host_pin(data_dir, &runtime)?;
        let mut transport = connect_ws(&runtime, &pin).await?;
        match device::load_stored_device(data_dir) {
            device::DeviceFileState::Loaded(stored) => {
                let device_id = stored.device_id;
                let secret = stored.pairing_secret;
                finish_connect(
                    transport,
                    incarnation,
                    data_dir,
                    platform,
                    device_id,
                    secret,
                    false,
                )
                .await
                .map(ConnectProgress::Connected)
            }
            device::DeviceFileState::Missing => {
                let request = pairing_frame(descriptor, incarnation);
                let pairing_message_id = request.envelope.message_id;
                transport.write(&request).await?;
                let answer = transport.read_handshake("pairing request").await?;
                require_reply_to(&answer, pairing_message_id, "pairing request")?;
                match answer.payload {
                    WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation {
                        pending_id,
                    }) => Ok(ConnectProgress::Pending(PendingPairingClient {
                        transport,
                        incarnation,
                        data_dir: data_dir.to_path_buf(),
                        platform: platform.to_owned(),
                        pending_id,
                        pairing_message_id,
                    })),
                    WirePayload::PairingResult(PairingResult::Denied { reason }) => {
                        Err(ClientError::ServerOutcome(format!(
                            "pairing denied: {reason}; start a fresh pairing request"
                        )))
                    }
                    WirePayload::IncompatibleProtocol(notice) => {
                        Err(incompatible_protocol_error(&notice))
                    }
                    unexpected => Err(ClientError::ServerRejected(format!(
                        "unexpected {} during pairing; expected PairingResult",
                        unexpected.message_type()
                    ))),
                }
            }
            device::DeviceFileState::Unreadable | device::DeviceFileState::Malformed => {
                Err(ClientError::ServerOutcome(unreadable_device_file_guidance()))
            }
        }
    }

    pub fn companion_ref(&self) -> String {
        self.state.companion_ref()
    }

    #[must_use]
    pub fn open_round(&self) -> Option<ene_api::v1::refs::RoundWireId> {
        self.state.open_round().cloned()
    }

    #[must_use]
    pub fn round_target(&self) -> ene_api::v1::round::RoundTarget {
        self.state.round_target()
    }

    #[must_use]
    pub fn prepare(&self, payload: WirePayload) -> PreparedRequest {
        PreparedRequest::new(payload)
    }

    pub async fn execute(
        &mut self,
        prepared: &PreparedRequest,
    ) -> Result<WirePayload, ClientError> {
        self.pump(prepared.frame(self.sender, self.state.generation()))
            .await
    }

    pub async fn request(&mut self, payload: WirePayload) -> Result<WirePayload, ClientError> {
        let prepared = self.prepare(payload);
        self.execute(&prepared).await
    }

    pub async fn request_observed(
        &mut self,
        payload: WirePayload,
        round: Option<ene_api::v1::refs::RoundWireId>,
        generation: Option<u64>,
    ) -> Result<WirePayload, ClientError> {
        use super::frames::observed_frame;
        use ene_api::v1::refs::RequestWireId;

        let mut frame = observed_frame(payload, self.sender, generation, round);
        frame.envelope.correlation.request_id = Some(RequestWireId(uuid::Uuid::new_v4()));
        self.pump(frame).await
    }

    async fn pump(&mut self, frame: WireFrame) -> Result<WirePayload, ClientError> {
        let own_message_id = frame.envelope.message_id;
        self.transport.write(&frame).await?;
        loop {
            let incoming = match self.transport.read().await? {
                DecodedFrame::Known(frame) => frame,
                DecodedFrame::Unsupported { envelope, reason } => {
                    self.reject_unsupported(&envelope, &reason).await?;
                    continue;
                }
            };
            if self
                .answer_deletion_demand_if_any(&incoming.payload)
                .await?
            {
                continue;
            }
            match decide_frame(own_message_id, &incoming) {
                FrameDecision::AbsorbPresence(fact) => self.state.observe_presence(&fact),
                FrameDecision::AbsorbBodyHint => {}
                FrameDecision::Answer(payload) => {
                    if let Some(current) = stale_generation_of(&payload) {
                        self.state.note_stale_generation(current);
                    }
                    self.state.observe_intake(&payload);
                    return Ok(payload);
                }
                FrameDecision::Defer => self.state.push_deferred(incoming),
            }
        }
    }

    async fn reject_unsupported(
        &mut self,
        envelope: &WireEnvelope,
        reason: &ene_api::codec::UnsupportedReason,
    ) -> Result<(), ClientError> {
        let mut reply = frame_for(WirePayload::Reject(reason.notice()), self.sender);
        reply.envelope.correlation.reply_to = Some(envelope.message_id);
        self.transport.write(&reply).await
    }

    async fn answer_deletion_demand_if_any(
        &mut self,
        payload: &WirePayload,
    ) -> Result<bool, ClientError> {
        let WirePayload::DeletionDemand(demand) = payload else {
            return Ok(false);
        };
        if self.state.defer_erasure() {
            self.state.clear_deferred_frames();
            self.state.push_pending_erasure(demand.clone());
            return Ok(true);
        }
        let result = self.local_erasure_result(demand);
        self.transport
            .write(&frame_for(
                WirePayload::LocalErasureResult(result),
                self.sender,
            ))
            .await?;
        Ok(true)
    }

    pub fn defer_erasure(&mut self) {
        self.state.set_defer_erasure(true);
    }

    pub fn take_pending_erasure(&mut self) -> Option<DeletionDemand> {
        self.state.take_pending_erasure()
    }

    pub async fn report_local_erasure(
        &mut self,
        result: LocalErasureResult,
    ) -> Result<(), ClientError> {
        self.notify(WirePayload::LocalErasureResult(result)).await
    }

    fn local_erasure_result(&mut self, demand: &DeletionDemand) -> LocalErasureResult {
        let wiped = self.state.wipe_transient();
        LocalErasureResult {
            demand: demand.demand.clone(),
            operation: demand.operation.clone(),
            sweep: demand.sweep,
            wiped,
            unverified: Vec::new(),
        }
    }

    pub async fn notify(&mut self, payload: WirePayload) -> Result<(), ClientError> {
        self.transport.write(&frame_for(payload, self.sender)).await
    }

    pub async fn next_frame(&mut self) -> Result<WirePayload, ClientError> {
        loop {
            let payload = match self.transport.read().await? {
                DecodedFrame::Known(frame) => frame.payload,
                DecodedFrame::Unsupported { envelope, reason } => {
                    self.reject_unsupported(&envelope, &reason).await?;
                    continue;
                }
            };
            if self.answer_deletion_demand_if_any(&payload).await? {
                continue;
            }
            if let WirePayload::PresenceAttribution(fact) = &payload {
                self.state.observe_presence(fact);
            }
            return Ok(payload);
        }
    }

    pub fn take_undelivered(&mut self) -> Vec<WireFrame> {
        self.state.take_undelivered()
    }

    /// Hands the pump a transport-level Ping to send. Test-only: the
    /// application's own traffic never needs to inject transport control.
    #[cfg(any(test, feature = "test-support"))]
    pub fn send_transport_ping_for_tests(&self, payload: &[u8]) {
        self.transport.probe.send_ping(payload);
    }

    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn transport_probe_for_tests(&self) -> TransportProbe {
        self.transport.probe.clone()
    }

    async fn authenticate(
        &mut self,
        challenge: &AuthChallenge,
        secret: &str,
        device: ene_api::v1::refs::DeviceWireId,
    ) -> Result<(), ClientError> {
        let proof = crate::pairing::pairing_proof_hex(secret, &challenge.nonce);
        self.transport
            .write(&proof_frame(&proof, self.sender.incarnation_id, device))
            .await?;
        let answer = self.transport.read_handshake("auth result").await?.payload;
        match decide_auth(&answer) {
            AuthDecision::Accepted { connection_id } => {
                self.sender.connection_id = Some(connection_id);
                Ok(())
            }
            AuthDecision::Guidance { message } => Err(ClientError::ServerOutcome(message)),
            AuthDecision::Unexpected { message } => Err(ClientError::ServerRejected(message)),
        }
    }
}

async fn finish_connect(
    mut transport: Transport,
    incarnation: ene_api::v1::refs::ClientIncarnationId,
    data_dir: &Path,
    platform: &str,
    device_id: ene_api::v1::refs::DeviceWireId,
    secret: PairingProvisionSecret,
    persist_after_acceptance: bool,
) -> Result<Client, ClientError> {
    transport
        .write(&capability_frame(platform, incarnation, device_id))
        .await?;
    match transport
        .read_handshake("negotiated connection")
        .await?
        .payload
    {
        WirePayload::NegotiatedConnection(negotiated) => {
            if !negotiated.version.shares_major_with(&ProtocolVersion::V1) {
                return Err(ClientError::ServerRejected(format!(
                    "negotiated incompatible version {}.{}; expected major 1",
                    negotiated.version.major, negotiated.version.minor
                )));
            }
        }
        WirePayload::IncompatibleProtocol(notice) => {
            return Err(incompatible_protocol_error(&notice));
        }
        unexpected => {
            return Err(ClientError::ServerRejected(format!(
                "unexpected {} during capability negotiation; expected NegotiatedConnection",
                unexpected.message_type()
            )));
        }
    }
    let mut session = Client {
        transport,
        sender: WireSender {
            device_id: Some(device_id),
            incarnation_id: incarnation,
            connection_id: None,
        },
        state: SessionState::default(),
    };
    let challenge_payload = session
        .transport
        .read_handshake("auth challenge")
        .await?
        .payload;
    let WirePayload::AuthChallenge(challenge) = challenge_payload else {
        return Err(ClientError::ServerRejected(format!(
            "unexpected {} after negotiation; expected AuthChallenge",
            challenge_payload.message_type()
        )));
    };
    session
        .authenticate(&challenge, secret.expose_secret(), device_id)
        .await?;
    if persist_after_acceptance {
        device::store_device(
            data_dir,
            &device::StoredDevice::new(device_id, secret.expose_secret().to_owned()),
        )?;
    }
    let fact = session.next_frame().await?;
    if !matches!(fact, WirePayload::PresenceAttribution(_)) {
        return Err(ClientError::ServerRejected(format!(
            "unexpected {} after authentication; expected PresenceAttribution",
            fact.message_type()
        )));
    }
    Ok(session)
}

pub(crate) fn incompatible_protocol_error(notice: &IncompatibleProtocol) -> ClientError {
    ClientError::ServerRejected(format!(
        "incompatible protocol: host max {}.{}, client max {}.{}; {}",
        notice.host_max.major,
        notice.host_max.minor,
        notice.client_max.major,
        notice.client_max.minor,
        notice.hint
    ))
}

fn require_reply_to(
    frame: &WireFrame,
    expected: WireMessageId,
    stage: &str,
) -> Result<(), ClientError> {
    if frame.envelope.correlation.reply_to == Some(expected) {
        Ok(())
    } else {
        Err(ClientError::ServerRejected(format!(
            "uncorrelated {stage} frame"
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ene_api::v1::envelope::WireSender;
    use ene_api::v1::handshake::PairingRequest;
    use ene_api::v1::refs::ClientIncarnationId;

    use super::*;

    type PlainWs = WebSocketStream<tokio::net::TcpStream>;

    /// A loopback WebSocket pair; with `small_buffers` the server side stops
    /// absorbing data quickly so a writer stalls against it.
    async fn ws_pair(small_buffers: bool) -> (PlainWs, PlainWs) {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.expect("accept");
            tokio_tungstenite::accept_async(tcp).await.expect("upgrade")
        });
        let socket = tokio::net::TcpSocket::new_v4().expect("socket");
        if small_buffers {
            // A small fixed send window makes a writer stall quickly against
            // a Host that stops reading.
            socket
                .set_send_buffer_size(4096)
                .expect("client send buffer");
        }
        let tcp = socket.connect(address).await.expect("connect");
        let (client, _) = tokio_tungstenite::client_async("ws://localhost/", tcp)
            .await
            .expect("client upgrade");
        (client, server.await.expect("server task"))
    }

    fn bulky_frame(padding: usize) -> Vec<u8> {
        let frame = frame_for(
            WirePayload::PairingRequest(PairingRequest {
                device_descriptor: "x".repeat(padding),
            }),
            WireSender {
                device_id: None,
                incarnation_id: ClientIncarnationId {
                    counter: 1,
                    random: 2,
                },
                connection_id: None,
            },
        );
        encode_frame(&frame).expect("frame encodes")
    }

    #[tokio::test]
    async fn an_application_backlog_still_answers_pings_and_surfaces_close() {
        let (client, mut server) = ws_pair(false).await;
        let mut transport = Transport::spawn_stream(client);
        let body = bulky_frame(64);

        // One frame more than the inbound queue capacity: the queue is full
        // and one more frame sits in the read-ahead window, which still has
        // room — below the overload boundary of queue + read-ahead.
        for _ in 0..=TRANSPORT_QUEUE {
            server
                .send(Message::Binary(body.clone().into()))
                .await
                .expect("application frame must send");
        }

        // The transport control plane keeps moving behind that backlog.
        let expected = vec![0x7_u8; 32];
        server
            .send(Message::Ping(expected.clone().into()))
            .await
            .expect("ping must send");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match server.next().await {
                    Some(Ok(Message::Pong(payload))) if payload.as_ref() == expected => return,
                    Some(Ok(_)) => continue,
                    other => panic!("expected a pong, got {other:?}"),
                }
            }
        })
        .await
        .expect("a ping must be answered while the application queue is full");

        server
            .send(Message::Close(None))
            .await
            .expect("close must send");
        let mut frames = 0;
        loop {
            match tokio::time::timeout(Duration::from_secs(10), transport.read()).await {
                Ok(Ok(_)) => frames += 1,
                Ok(Err(error)) => {
                    assert_eq!(
                        frames,
                        TRANSPORT_QUEUE + 1,
                        "every buffered frame is delivered before the close"
                    );
                    assert!(
                        format!("{error:?}").contains("Host closed the connection"),
                        "the close reason must surface, got {error:?}"
                    );
                    break;
                }
                Err(_) => panic!("the close must surface behind the backlog"),
            }
        }
    }

    #[tokio::test]
    async fn an_application_frame_past_the_read_ahead_window_fails_the_transport() {
        let (client, mut server) = ws_pair(false).await;
        let mut transport = Transport::spawn_stream(client);
        let body = bulky_frame(64);

        // Saturation, not the queue alone: the full inbound queue plus the
        // full read-ahead window. The transport is still usable at exactly
        // this capacity.
        for _ in 0..(TRANSPORT_QUEUE + READ_AHEAD_FRAMES) {
            server
                .send(Message::Binary(body.clone().into()))
                .await
                .expect("the saturating frame must send");
        }

        // One application frame past the boundary must not stall the pump
        // forever and must not be dropped in silence: it ends the transport
        // with an explicit, bounded failure after the buffered frames.
        server
            .send(Message::Binary(body.clone().into()))
            .await
            .expect("the over-capacity frame must reach the socket");

        // Let the reader reach the boundary while the application is not
        // consuming: the queue and the read-ahead window must both be full
        // when the over-capacity frame is read.
        tokio::time::sleep(Duration::from_millis(300)).await;

        let mut frames = 0;
        loop {
            match tokio::time::timeout(Duration::from_secs(10), transport.read()).await {
                Ok(Ok(_)) => frames += 1,
                Ok(Err(error)) => {
                    assert_eq!(
                        frames,
                        TRANSPORT_QUEUE + READ_AHEAD_FRAMES,
                        "every buffered frame is delivered before the overload failure"
                    );
                    let rendered = format!("{error:?}");
                    assert!(
                        rendered.contains("backlog"),
                        "the over-capacity frame must surface as an explicit backlog failure, got {rendered}"
                    );
                    break;
                }
                Err(_) => panic!(
                    "the overload must surface instead of hanging the pump; got {frames} frames"
                ),
            }
        }
    }

    /// The over-capacity overload with no application consumer at all: the
    /// reader and writer must reach their terminal state on their own, and a
    /// later reader still gets the buffered data in order followed by the
    /// explicit backlog failure — the reader is not kept alive to surface it.
    #[tokio::test]
    async fn an_overload_ends_the_pump_without_an_application_drain() {
        let (client, mut server) = ws_pair(false).await;
        let mut transport = Transport::spawn_stream(client);
        let total = TRANSPORT_QUEUE + READ_AHEAD_FRAMES;

        // Distinct frames, so their order is observable when they surface.
        for padding in 0..total {
            server
                .send(Message::Binary(bulky_frame(padding).into()))
                .await
                .expect("the saturating frame must send");
        }
        server
            .send(Message::Binary(bulky_frame(total).into()))
            .await
            .expect("the over-capacity frame must reach the socket");

        // The application never calls read(): the pump still ends in bounded
        // time instead of waiting for a drain that can never come.
        tokio::time::timeout(Duration::from_secs(10), async {
            while !transport.reader_task.is_finished() || !transport.writer_task.is_finished() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the pump must reach its terminal state without an application drain");

        let mut paddings = Vec::with_capacity(total);
        for _ in 0..total {
            let frame = tokio::time::timeout(Duration::from_secs(10), transport.read())
                .await
                .expect("the buffered frames must surface")
                .expect("the buffered frames must decode");
            let DecodedFrame::Known(frame) = frame else {
                panic!("the buffered frame must be known, got {frame:?}");
            };
            let WirePayload::PairingRequest(request) = frame.payload else {
                panic!(
                    "expected buffered application frames, got {}",
                    frame.payload.message_type()
                );
            };
            paddings.push(request.device_descriptor.len());
        }
        assert_eq!(
            paddings,
            (0..total).collect::<Vec<_>>(),
            "the buffered frames must surface in their arrival order"
        );

        let failure = tokio::time::timeout(Duration::from_secs(10), transport.read())
            .await
            .expect("the overload failure must surface behind the buffered frames")
            .expect_err("the over-capacity frame must fail the transport");
        let rendered = format!("{failure:?}");
        assert!(
            rendered.contains("backlog"),
            "the overload must stay an explicit backlog failure, got {rendered}"
        );
        assert!(
            transport.reader_task.is_finished(),
            "the reader must not stay alive to surface the failure"
        );
    }

    #[tokio::test]
    async fn eof_surfaces_after_the_buffered_frames() {
        let (client, mut server) = ws_pair(false).await;
        let mut transport = Transport::spawn_stream(client);
        server
            .send(Message::Binary(bulky_frame(16).into()))
            .await
            .expect("frame must send");
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(server);
        let first = tokio::time::timeout(Duration::from_secs(10), transport.read())
            .await
            .expect("the buffered frame must arrive")
            .expect("the buffered frame must decode");
        drop(first);
        let eof = tokio::time::timeout(Duration::from_secs(10), transport.read())
            .await
            .expect("eof must surface")
            .expect_err("the connection ended");
        let rendered = format!("{eof:?}");
        assert!(
            rendered.contains("the connection to the Host ended")
                || rendered.contains("websocket read failed"),
            "the transport end must surface as a transport failure, got {eof:?}"
        );
    }

    #[tokio::test]
    async fn a_stalled_host_read_is_bounded_and_surfaces_a_transport_failure() {
        let (client, _server) = ws_pair(true).await;
        let mut transport = Transport::spawn_stream(client);
        let outbound = transport.outbound_for_tests();
        let body = bulky_frame(8 * 1024);

        // The application keeps writing while the Host never reads: the
        // writer stalls against the closed window and the outbound queue
        // fills behind it.
        let writer = tokio::spawn(async move {
            for _ in 0..64 {
                if outbound.send(body.clone()).await.is_err() {
                    break;
                }
            }
        });
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }

        // The write wait bound must release the pump instead of owning it.
        tokio::time::pause();
        tokio::time::advance(WRITE_WAIT + Duration::from_secs(2)).await;
        tokio::time::resume();

        let failure = tokio::time::timeout(Duration::from_secs(10), transport.read())
            .await
            .expect("the stalled write must surface instead of hanging")
            .expect_err("the stalled write must fail the connection");
        assert!(
            format!("{failure:?}").contains("websocket write failed"),
            "got {failure:?}"
        );
        tokio::time::timeout(Duration::from_secs(10), writer)
            .await
            .expect("the application writer must stop once the transport is gone")
            .expect("the writer task must not panic");
    }
}
