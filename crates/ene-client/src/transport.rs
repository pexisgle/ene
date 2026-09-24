use std::path::{Path, PathBuf};

use crate::pairing::pairing_proof_hex;
#[cfg(any(unix, windows))]
use ene_api::codec::{
    CodecError, DecodedFrame, MAX_FRAME_BYTES, UnsupportedReason, WireFrame, decode_frame,
    encode_frame,
};
use ene_api::v1::deletion::{DeletionDemand, LocalErasureResult};
#[cfg(any(unix, windows))]
use ene_api::v1::envelope::{ProtocolVersion, WireEnvelope, WireSender};
use ene_api::v1::handshake::{AuthChallenge, PairingProvisionSecret, PairingResult};
use ene_api::v1::payload::WirePayload;
#[cfg(any(unix, windows))]
use ene_api::v1::refs::WireMessageId;
#[cfg(any(unix, windows))]
use ene_api::v1::reject::IncompatibleProtocol;

#[cfg(any(unix, windows))]
use crate::device;
use crate::error::ClientError;

#[cfg(any(unix, windows))]
use super::frames::{
    PreparedRequest, capability_frame, frame_for, pairing_frame, proof_frame,
    unreadable_device_file_guidance,
};
#[cfg(any(unix, windows))]
use super::session::{
    AuthDecision, FrameDecision, SessionState, decide_auth, decide_frame, stale_generation_of,
};
#[cfg(unix)]
use super::socket_path;

#[cfg(any(unix, windows))]
pub struct Client {
    stream: Stream,
    sender: WireSender,
    state: SessionState,
}

#[cfg(any(unix, windows))]
pub enum ConnectProgress {
    Connected(Client),
    Pending(PendingPairingClient),
}

#[cfg(any(unix, windows))]
pub struct PendingPairingClient {
    stream: Stream,
    incarnation: ene_api::v1::refs::ClientIncarnationId,
    data_dir: PathBuf,
    platform: String,
    pending_id: String,
    pairing_message_id: WireMessageId,
}

#[cfg(any(unix, windows))]
impl PendingPairingClient {
    #[must_use]
    pub fn pending_id(&self) -> &str {
        &self.pending_id
    }

    pub async fn complete(self) -> Result<Client, ClientError> {
        let Self {
            mut stream,
            incarnation,
            data_dir,
            platform,
            pending_id: _,
            pairing_message_id,
        } = self;
        let provision_frame = read_known(&mut stream, "pairing provision").await?;
        require_reply_to(&provision_frame, pairing_message_id, "pairing provision")?;
        let WirePayload::PairingProvision(provision) = provision_frame.payload else {
            return Err(ClientError::ServerRejected(format!(
                "unexpected {} while awaiting pairing provision; start a fresh pairing request",
                provision_frame.payload.message_type()
            )));
        };
        Client::finish_connect(
            stream,
            incarnation,
            &data_dir,
            &platform,
            provision.device_id,
            provision.pairing_secret,
            true,
        )
        .await
    }
}

#[cfg(unix)]
type Stream = tokio::net::UnixStream;

#[cfg(windows)]
type Stream = tokio::net::windows::named_pipe::NamedPipeClient;

#[cfg(any(unix, windows))]
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
        #[cfg(unix)]
        let mut stream = {
            let path = socket_path(data_dir);
            tokio::net::UnixStream::connect(&path)
                .await
                .map_err(|error| {
                    ClientError::Transport(format!(
                        "connect to {} failed: {}",
                        path.display(),
                        error.kind()
                    ))
                })?
        };
        #[cfg(windows)]
        let mut stream = {
            let pipe = ene_plugin_ipc::pipe_name(data_dir);
            tokio::net::windows::named_pipe::ClientOptions::new()
                .open(&pipe)
                .map_err(|error| {
                    ClientError::Transport(format!("connect to {pipe} failed: {}", error.kind()))
                })?
        };
        match device::load_stored_device(data_dir) {
            device::DeviceFileState::Loaded(stored) => {
                let device_id = stored.device_id;
                let secret = stored.pairing_secret;
                Self::finish_connect(
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
                let request = pairing_frame(descriptor, incarnation);
                let pairing_message_id = request.envelope.message_id;
                write_frame(&mut stream, &request).await?;
                let answer = read_known(&mut stream, "pairing request").await?;
                require_reply_to(&answer, pairing_message_id, "pairing request")?;
                match answer.payload {
                    WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation {
                        pending_id,
                    }) => Ok(ConnectProgress::Pending(PendingPairingClient {
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

    async fn finish_connect(
        mut stream: Stream,
        incarnation: ene_api::v1::refs::ClientIncarnationId,
        data_dir: &Path,
        platform: &str,
        device_id: ene_api::v1::refs::DeviceWireId,
        secret: PairingProvisionSecret,
        persist_after_acceptance: bool,
    ) -> Result<Self, ClientError> {
        write_frame(
            &mut stream,
            &capability_frame(platform, incarnation, device_id),
        )
        .await?;
        match read_known(&mut stream, "negotiated connection")
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
        let mut session = Self {
            stream,
            sender: WireSender {
                device_id: Some(device_id),
                incarnation_id: incarnation,
                connection_id: None,
            },
            state: SessionState::default(),
        };
        let challenge = read_known(&mut session.stream, "auth challenge")
            .await?
            .payload;
        let WirePayload::AuthChallenge(challenge) = challenge else {
            return Err(ClientError::ServerRejected(format!(
                "unexpected {} after negotiation; expected AuthChallenge",
                challenge.message_type()
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

    async fn authenticate(
        &mut self,
        challenge: &AuthChallenge,
        secret: &str,
        device: ene_api::v1::refs::DeviceWireId,
    ) -> Result<(), ClientError> {
        let proof = pairing_proof_hex(secret, &challenge.nonce);
        write_frame(
            &mut self.stream,
            &proof_frame(&proof, self.sender.incarnation_id, device),
        )
        .await?;
        let answer = read_known(&mut self.stream, "auth result").await?.payload;
        match decide_auth(&answer) {
            AuthDecision::Accepted { connection_id } => {
                self.sender.connection_id = Some(connection_id);
                Ok(())
            }
            AuthDecision::Guidance { message } => Err(ClientError::ServerOutcome(message)),
            AuthDecision::Unexpected { message } => Err(ClientError::ServerRejected(message)),
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

    pub fn take_undelivered(&mut self) -> Vec<WireFrame> {
        self.state.take_undelivered()
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
        write_frame(&mut self.stream, &frame).await?;
        loop {
            let incoming = match read_frame(&mut self.stream).await? {
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
            &mut self.stream,
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
        write_frame(&mut self.stream, &frame_for(payload, self.sender)).await
    }

    async fn reject_unsupported(
        &mut self,
        envelope: &WireEnvelope,
        reason: &UnsupportedReason,
    ) -> Result<(), ClientError> {
        let mut reply = frame_for(WirePayload::Reject(reason.notice()), self.sender);
        reply.envelope.correlation.reply_to = Some(envelope.message_id);
        write_frame(&mut self.stream, &reply).await
    }

    pub async fn next_frame(&mut self) -> Result<WirePayload, ClientError> {
        loop {
            let payload = match read_frame(&mut self.stream).await? {
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
}

#[cfg(any(unix, windows))]
async fn write_frame(
    stream: &mut (impl tokio::io::AsyncWrite + Unpin),
    frame: &WireFrame,
) -> Result<(), ClientError> {
    use tokio::io::AsyncWriteExt as _;
    let body = zeroize::Zeroizing::new(
        encode_frame(frame)
            .map_err(|error: CodecError| ClientError::Codec(format!("encode failed: {error}")))?,
    );
    let mut bytes = Vec::with_capacity(4 + body.len());
    bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&body);
    stream.write_all(&bytes).await.map_err(|error| {
        ClientError::Transport(format!("socket write failed: {}", error.kind()))
    })?;
    Ok(())
}

#[cfg(any(unix, windows))]
async fn read_frame(
    stream: &mut (impl tokio::io::AsyncRead + Unpin),
) -> Result<DecodedFrame, ClientError> {
    use tokio::io::AsyncReadExt as _;
    let mut prefix = [0_u8; 4];
    stream
        .read_exact(&mut prefix)
        .await
        .map_err(|error| ClientError::Transport(format!("socket read failed: {}", error.kind())))?;
    let claimed = u32::from_be_bytes(prefix) as usize;
    if claimed > MAX_FRAME_BYTES {
        return Err(ClientError::Codec(format!(
            "frame body of {claimed} bytes exceeds the 256 KiB cap"
        )));
    }
    let mut body = zeroize::Zeroizing::new(vec![0_u8; claimed]);
    stream
        .read_exact(&mut body)
        .await
        .map_err(|error| ClientError::Transport(format!("socket read failed: {}", error.kind())))?;
    decode_frame(&body)
        .map_err(|error: CodecError| ClientError::Codec(format!("decode failed: {error}")))
}

#[cfg(any(unix, windows))]
async fn read_known(
    stream: &mut (impl tokio::io::AsyncRead + Unpin),
    stage: &str,
) -> Result<WireFrame, ClientError> {
    match read_frame(stream).await? {
        DecodedFrame::Known(frame) => Ok(frame),
        DecodedFrame::Unsupported { reason, .. } => Err(ClientError::ServerRejected(format!(
            "unsupported message during {stage}: {}",
            reason.notice().detail
        ))),
    }
}

#[cfg(any(unix, windows))]
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

#[cfg(any(unix, windows))]
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

#[cfg(not(any(unix, windows)))]
pub struct Client {
    _sealed: (),
}

#[cfg(not(any(unix, windows)))]
pub enum ConnectProgress {
    Connected(Client),
    Pending(PendingPairingClient),
}

#[cfg(not(any(unix, windows)))]
pub struct PendingPairingClient {
    _sealed: (),
}

#[cfg(not(any(unix, windows)))]
impl PendingPairingClient {
    #[must_use]
    pub fn pending_id(&self) -> &str {
        ""
    }

    pub async fn complete(self) -> Result<Client, ClientError> {
        Err(ClientError::UnsupportedPlatform("no supported transport"))
    }
}

#[cfg(not(any(unix, windows)))]
impl Client {
    pub async fn connect(
        _data_dir: &Path,
        _descriptor: &str,
        _platform: &str,
    ) -> Result<Self, ClientError> {
        Err(ClientError::UnsupportedPlatform("no supported transport"))
    }

    pub async fn begin_connect(
        _data_dir: &Path,
        _descriptor: &str,
        _platform: &str,
    ) -> Result<ConnectProgress, ClientError> {
        Err(ClientError::UnsupportedPlatform("no supported transport"))
    }

    pub async fn request(&mut self, _payload: WirePayload) -> Result<WirePayload, ClientError> {
        Err(ClientError::UnsupportedPlatform("no supported transport"))
    }

    pub async fn next_frame(&mut self) -> Result<WirePayload, ClientError> {
        Err(ClientError::UnsupportedPlatform("no supported transport"))
    }

    pub async fn notify(&mut self, _payload: WirePayload) -> Result<(), ClientError> {
        Err(ClientError::UnsupportedPlatform("no supported transport"))
    }

    pub fn defer_erasure(&mut self) {}

    pub fn take_pending_erasure(&mut self) -> Option<ene_api::v1::deletion::DeletionDemand> {
        None
    }

    pub async fn report_local_erasure(
        &mut self,
        _result: ene_api::v1::deletion::LocalErasureResult,
    ) -> Result<(), ClientError> {
        Err(ClientError::UnsupportedPlatform("no supported transport"))
    }

    pub fn companion_ref(&self) -> String {
        String::from(crate::DEFAULT_COMPANION_REF)
    }
}
