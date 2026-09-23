use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ene_api::codec::{CodecError, DecodedFrame, WireFrame, decode_frame, encode_frame};
use ene_api::runtime::HostRuntimeInfo;
use ene_api::v1::deletion::{DeletionDemand, LocalErasureResult};
use ene_api::v1::envelope::{ProtocolVersion, WireEnvelope, WireSender};
use ene_api::v1::handshake::{AuthChallenge, PairingProvisionSecret, PairingResult};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::WireMessageId;
use ene_api::v1::reject::IncompatibleProtocol;
use futures_util::{SinkExt as _, StreamExt as _};
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

type WsClient =
    tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>;
type ClientSink = futures_util::stream::SplitSink<WsClient, Message>;
type ClientStream = futures_util::stream::SplitStream<WsClient>;

fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .read_buffer_size(16 * 1024)
        .write_buffer_size(16 * 1024)
        .max_write_buffer_size(ene_api::codec::MAX_FRAME_BYTES)
        .max_message_size(Some(ene_api::codec::MAX_FRAME_BYTES))
        .max_frame_size(Some(ene_api::codec::MAX_FRAME_BYTES))
}

async fn connect_ws(
    runtime: &HostRuntimeInfo,
    pin: &str,
) -> Result<(ClientSink, ClientStream), ClientError> {
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
    Ok(socket.split())
}

async fn write_frame(sink: &mut ClientSink, frame: &WireFrame) -> Result<(), ClientError> {
    let body = encode_frame(frame)
        .map_err(|error: CodecError| ClientError::Codec(format!("encode failed: {error}")))?;
    sink.send(Message::Binary(body.into()))
        .await
        .map_err(|error| ClientError::Transport(format!("websocket write failed: {error}")))
}

async fn read_decoded(
    sink: &mut ClientSink,
    stream: &mut ClientStream,
) -> Result<DecodedFrame, ClientError> {
    while let Some(message) = stream.next().await {
        let message = message
            .map_err(|error| ClientError::Transport(format!("websocket read failed: {error}")))?;
        match message {
            Message::Binary(body) => {
                return decode_frame(&body).map_err(|error: CodecError| {
                    ClientError::Codec(format!("decode failed: {error}"))
                });
            }
            Message::Ping(payload) => {
                sink.send(Message::Pong(payload)).await.map_err(|error| {
                    ClientError::Transport(format!("websocket pong failed: {error}"))
                })?;
            }
            Message::Pong(_) => {}
            Message::Text(_) => {
                return Err(ClientError::Transport(String::from(
                    "text frames are not part of the wire protocol",
                )));
            }
            Message::Close(_) => {
                return Err(ClientError::Transport(String::from(
                    "the Host closed the connection",
                )));
            }
            Message::Frame(_) => {
                return Err(ClientError::Transport(String::from(
                    "unexpected raw frame from the Host",
                )));
            }
        }
    }
    Err(ClientError::Transport(String::from(
        "the connection to the Host ended",
    )))
}

async fn read_known(
    sink: &mut ClientSink,
    stream: &mut ClientStream,
    stage: &str,
) -> Result<WireFrame, ClientError> {
    match read_decoded(sink, stream).await? {
        DecodedFrame::Known(frame) => Ok(frame),
        DecodedFrame::Unsupported { reason, .. } => Err(ClientError::ServerRejected(format!(
            "unsupported message during {stage}: {}",
            reason.notice().detail
        ))),
    }
}

async fn handshake_read(
    sink: &mut ClientSink,
    stream: &mut ClientStream,
    stage: &str,
) -> Result<WireFrame, ClientError> {
    tokio::time::timeout(HANDSHAKE_TIMEOUT, read_known(sink, stream, stage))
        .await
        .map_err(|_| {
            ClientError::Transport(format!("the Host did not answer the {stage} step in time"))
        })?
}

pub struct Client {
    sink: ClientSink,
    stream: ClientStream,
    sender: WireSender,
    state: SessionState,
}

pub enum ConnectProgress {
    Connected(Client),
    Pending(PendingPairingClient),
}

pub struct PendingPairingClient {
    sink: ClientSink,
    stream: ClientStream,
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
            mut sink,
            mut stream,
            incarnation,
            data_dir,
            platform,
            pending_id: _,
            pairing_message_id,
        } = self;
        let provision_frame = read_known(&mut sink, &mut stream, "pairing provision").await?;
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
            sink,
            stream,
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
        let (sink, stream) = connect_ws(&runtime, &pin).await?;
        match device::load_stored_device(data_dir) {
            device::DeviceFileState::Loaded(stored) => {
                let device_id = stored.device_id;
                let secret = stored.pairing_secret;
                finish_connect(
                    sink,
                    stream,
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
                let mut sink = sink;
                let mut stream = stream;
                let request = pairing_frame(descriptor, incarnation);
                let pairing_message_id = request.envelope.message_id;
                write_frame(&mut sink, &request).await?;
                let answer = handshake_read(&mut sink, &mut stream, "pairing request").await?;
                require_reply_to(&answer, pairing_message_id, "pairing request")?;
                match answer.payload {
                    WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation {
                        pending_id,
                    }) => Ok(ConnectProgress::Pending(PendingPairingClient {
                        sink,
                        stream,
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
        write_frame(&mut self.sink, &frame).await?;
        loop {
            let incoming = match read_decoded(&mut self.sink, &mut self.stream).await? {
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
        write_frame(&mut self.sink, &reply).await
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
        write_frame(
            &mut self.sink,
            &frame_for(WirePayload::LocalErasureResult(result), self.sender),
        )
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
        write_frame(&mut self.sink, &frame_for(payload, self.sender)).await
    }

    pub async fn next_frame(&mut self) -> Result<WirePayload, ClientError> {
        loop {
            let payload = match read_decoded(&mut self.sink, &mut self.stream).await? {
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
        write_frame(
            &mut self.sink,
            &proof_frame(&proof, self.sender.incarnation_id, device),
        )
        .await?;
        let answer = handshake_read(&mut self.sink, &mut self.stream, "auth result")
            .await?
            .payload;
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

#[expect(
    clippy::too_many_arguments,
    reason = "the handshake state is intentionally explicit"
)]
async fn finish_connect(
    mut sink: ClientSink,
    mut stream: ClientStream,
    incarnation: ene_api::v1::refs::ClientIncarnationId,
    data_dir: &Path,
    platform: &str,
    device_id: ene_api::v1::refs::DeviceWireId,
    secret: PairingProvisionSecret,
    persist_after_acceptance: bool,
) -> Result<Client, ClientError> {
    write_frame(
        &mut sink,
        &capability_frame(platform, incarnation, device_id),
    )
    .await?;
    match handshake_read(&mut sink, &mut stream, "negotiated connection")
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
        sink,
        stream,
        sender: WireSender {
            device_id: Some(device_id),
            incarnation_id: incarnation,
            connection_id: None,
        },
        state: SessionState::default(),
    };
    let challenge_payload =
        handshake_read(&mut session.sink, &mut session.stream, "auth challenge")
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
