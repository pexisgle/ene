use std::path::{Path, PathBuf};
use std::sync::Arc;
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
/// bounded so a slow peer applies backpressure instead of buffering.
const TRANSPORT_QUEUE: usize = 32;
const CONTROL_QUEUE: usize = 8;
/// Mirrors the Host's read-ahead: while the application queue is full the
/// reader keeps consuming transport control frames out of this bounded
/// window instead of stalling the socket.
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

#[expect(
    clippy::large_enum_variant,
    reason = "frames are bounded by the wire cap and travel a bounded queue; boxing would add an allocation per frame"
)]
enum Inbound {
    Frame(DecodedFrame),
    CodecFailed(String),
    TransportFailed(String),
}

/// Owns the WebSocket through a background pump, so Ping/Pong and inbound
/// frames are processed whenever the socket has them — independent of
/// whether an upper layer is inside a request or `next_frame()`.
struct Transport {
    inbound: mpsc::Receiver<Inbound>,
    outbound: mpsc::Sender<Vec<u8>>,
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
        let (writer_gone, writer_gone_rx) = tokio::sync::watch::channel(());
        let (sink, stream) = socket.split();
        tokio::spawn(write_half(
            sink,
            outbound_rx,
            control_rx,
            inbound_tx.clone(),
            writer_gone,
        ));
        tokio::spawn(read_half(stream, inbound_tx, control, writer_gone_rx));
        Self { inbound, outbound }
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
        match self.inbound.recv().await {
            Some(Inbound::Frame(frame)) => Ok(frame),
            Some(Inbound::CodecFailed(reason)) => Err(ClientError::Codec(reason)),
            Some(Inbound::TransportFailed(reason)) => Err(ClientError::Transport(reason)),
            None => Err(ClientError::Transport(String::from(
                "the connection to the Host ended",
            ))),
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

/// Owns the write half: transport Pongs and application frames are written
/// here under one bounded write wait, so neither a Host that stops reading
/// nor a stalled application can own this task forever. `_writer_gone` is
/// dropped when this writer ends; the reader observes the closed watch and
/// stops with it.
async fn write_half<S>(
    sink: futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
    mut outbound: mpsc::Receiver<Vec<u8>>,
    mut control: mpsc::Receiver<Vec<u8>>,
    inbound: mpsc::Sender<Inbound>,
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
                if !send_bounded(&mut sink, Message::Pong(payload.into())).await {
                    surface_write_failure(&inbound).await;
                    return;
                }
            }
            next = outbound.recv() => {
                let Some(body) = next else {
                    // The application is gone; the reader closes the socket.
                    return;
                };
                if !send_bounded(&mut sink, Message::Binary(body.into())).await {
                    surface_write_failure(&inbound).await;
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

async fn surface_write_failure(inbound: &mpsc::Sender<Inbound>) {
    let reason = String::from("websocket write failed: the Host stopped accepting data");
    drop(inbound.send(Inbound::TransportFailed(reason)).await);
}

/// Owns the read half: Ping → Pong, Close, EOF, and read failures are
/// handled here immediately, and application frames are handed to the
/// inbound queue through the bounded `READ_AHEAD_FRAMES` window, so a slow
/// application consumer applies backpressure to the socket instead of
/// stopping the transport control plane. Terminal conditions flush the
/// buffered frames first and then surface the reason to the readers above.
async fn read_half<S>(
    stream: futures_util::stream::SplitStream<WebSocketStream<S>>,
    inbound: mpsc::Sender<Inbound>,
    control: mpsc::Sender<Vec<u8>>,
    mut writer_gone: tokio::sync::watch::Receiver<()>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use std::collections::VecDeque;
    use tokio::sync::mpsc::error::TrySendError;

    let mut stream = stream;
    let mut read_ahead: VecDeque<DecodedFrame> = VecDeque::new();
    loop {
        let next = if read_ahead.len() >= READ_AHEAD_FRAMES {
            // Full backpressure: nothing further is read from the socket
            // until the application makes room.
            tokio::select! {
                biased;
                gone = writer_gone.changed() => {
                    drop(gone);
                    return;
                }
                permit = inbound.reserve() => match permit {
                    Ok(permit) => {
                        if let Some(frame) = read_ahead.pop_front() {
                            permit.send(Inbound::Frame(frame));
                        }
                        continue;
                    }
                    Err(_) => return,
                },
            }
        } else if read_ahead.is_empty() {
            tokio::select! {
                biased;
                gone = writer_gone.changed() => {
                    drop(gone);
                    return;
                }
                next = stream.next() => next,
            }
        } else {
            tokio::select! {
                biased;
                permit = inbound.reserve() => match permit {
                    Ok(permit) => {
                        if let Some(frame) = read_ahead.pop_front() {
                            permit.send(Inbound::Frame(frame));
                        }
                        continue;
                    }
                    Err(_) => return,
                },
                gone = writer_gone.changed() => {
                    drop(gone);
                    return;
                }
                next = stream.next() => next,
            }
        };
        match next {
            Some(Ok(Message::Binary(body))) => {
                let frame = match decode_frame(&body) {
                    Ok(frame) => frame,
                    Err(error) => {
                        let reason = format!("decode failed: {error}");
                        fail_after_flush(&mut read_ahead, &inbound, Inbound::CodecFailed(reason))
                            .await;
                        return;
                    }
                };
                match inbound.try_send(Inbound::Frame(frame)) {
                    Ok(()) => {}
                    Err(TrySendError::Full(Inbound::Frame(frame))) => read_ahead.push_back(frame),
                    Err(TrySendError::Full(_)) | Err(TrySendError::Closed(_)) => return,
                }
            }
            Some(Ok(Message::Ping(payload))) => {
                if control.send(payload.to_vec()).await.is_err() {
                    return;
                }
            }
            Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Text(_))) => {
                fail_after_flush(
                    &mut read_ahead,
                    &inbound,
                    Inbound::TransportFailed(String::from(
                        "text frames are not part of the wire protocol",
                    )),
                )
                .await;
                return;
            }
            Some(Ok(Message::Close(_))) => {
                fail_after_flush(
                    &mut read_ahead,
                    &inbound,
                    Inbound::TransportFailed(String::from("the Host closed the connection")),
                )
                .await;
                return;
            }
            Some(Ok(Message::Frame(_))) | None => {
                fail_after_flush(
                    &mut read_ahead,
                    &inbound,
                    Inbound::TransportFailed(String::from("the connection to the Host ended")),
                )
                .await;
                return;
            }
            Some(Err(error)) => {
                let reason = format!("websocket read failed: {error}");
                fail_after_flush(&mut read_ahead, &inbound, Inbound::TransportFailed(reason)).await;
                return;
            }
        }
    }
}

/// Delivers every buffered frame before the terminal reason, so the
/// application observes its data first and the failure last.
async fn fail_after_flush(
    read_ahead: &mut std::collections::VecDeque<DecodedFrame>,
    inbound: &mpsc::Sender<Inbound>,
    reason: Inbound,
) {
    while let Some(frame) = read_ahead.pop_front() {
        if inbound.send(Inbound::Frame(frame)).await.is_err() {
            return;
        }
    }
    drop(inbound.send(reason).await);
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
        // and one more frame sits in the reader's bounded read-ahead.
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
