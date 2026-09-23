use std::path::{Path, PathBuf};

use crate::pairing::pairing_proof_hex;
use ene_api::v1::deletion::{DeletionDemand, LocalErasureResult};
#[cfg(any(unix, windows))]
use ene_api::v1::envelope::{ProtocolVersion, WireSender};
use ene_api::v1::handshake::{AuthChallenge, PairingProvisionSecret, PairingResult};
use ene_api::v1::payload::WirePayload;
#[cfg(any(unix, windows))]
use ene_api::v1::refs::WireMessageId;
#[cfg(any(unix, windows))]
use ene_api::v1::reject::IncompatibleProtocol;
#[cfg(any(unix, windows))]
use ene_plugin_ipc::{CodecError, MAX_FRAME_BYTES, WireFrame, decode_frame, encode_frame};

#[cfg(any(unix, windows))]
use crate::device;
use crate::error::ClientError;

#[cfg(any(unix, windows))]
use super::frames::{
    PreparedRequest, capability_frame, frame_for, missing_secret_guidance, pairing_frame,
    proof_frame, unreadable_device_file_guidance,
};
#[cfg(any(unix, windows))]
use super::session::{
    AuthDecision, FrameDecision, SessionState, decide_auth, decide_frame, stale_generation_of,
};
#[cfg(unix)]
use super::socket_path;

/// Connected, handshaked Host session: the stream, the sender identity
/// pairing and authentication fill in, and the observed session state.
/// Unix dials `ene.sock`; Windows opens the data directory's named pipe
/// (see `pipe_name`). Everything after the dial — pairing poll, capability,
/// provision, capability, challenge authentication, and request/response
/// correlation — is shared.
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
        let provision_frame = read_frame(&mut stream).await?;
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

#[cfg(any(test, windows))]
fn pipe_name(data_dir: &Path) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;
    let mut tag = FNV_OFFSET;
    for byte in data_dir.as_os_str().as_encoded_bytes() {
        tag ^= u64::from(*byte);
        tag = tag.wrapping_mul(FNV_PRIME);
    }
    format!(r"\\.\pipe\ene-{tag:016x}")
}

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
            let pipe = pipe_name(data_dir);
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
                let answer = read_frame(&mut stream).await?;
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
            &capability_frame(platform, incarnation, Some(device_id)),
        )
        .await?;
        match read_frame(&mut stream).await?.payload {
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
        let mut state = SessionState::default();
        state.set_pairing_secret(secret);
        let mut session = Self {
            stream,
            sender: WireSender {
                device_id: Some(device_id),
                incarnation_id: incarnation,
                connection_id: None,
            },
            state,
        };
        let challenge = read_frame(&mut session.stream).await?.payload;
        let WirePayload::AuthChallenge(challenge) = challenge else {
            return Err(ClientError::ServerRejected(format!(
                "unexpected {} after negotiation; expected AuthChallenge",
                challenge.message_type()
            )));
        };
        session.authenticate(&challenge).await?;
        if persist_after_acceptance {
            let Some(secret_value) = session.state.pairing_secret() else {
                return Err(ClientError::Transport(String::from(
                    "accepted authentication lost the client device secret",
                )));
            };
            device::store_device(
                data_dir,
                &device::StoredDevice::new(device_id, secret_value.to_owned()),
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

    /// Answers one authentication challenge using the session secret, storing
    /// the accepted connection key into the sender (for all later frames).
    /// [`Client::connect`] calls this for the
    /// post-negotiation challenge, the only challenge the Host sends on a
    /// connection.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Transport`] or [`ClientError::Codec`] when the
    /// exchange cannot be moved or framed; [`ClientError::ServerOutcome`] when no
    /// secret is available or the Host rejects the proof (both require a fresh
    /// pairing); and [`ClientError::ServerRejected`] when the Host answers
    /// with an unexpected payload kind.
    async fn authenticate(&mut self, challenge: &AuthChallenge) -> Result<(), ClientError> {
        let Some(secret) = self.state.pairing_secret() else {
            return Err(ClientError::ServerOutcome(missing_secret_guidance()));
        };
        let proof = pairing_proof_hex(secret, &challenge.nonce);
        let Some(device) = self.sender.device_id else {
            return Err(ClientError::ServerRejected(String::from(
                "cannot prove ownership without a paired device",
            )));
        };
        write_frame(
            &mut self.stream,
            &proof_frame(&proof, self.sender.incarnation_id, device),
        )
        .await?;
        let answer = read_frame(&mut self.stream).await?.payload;
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

    /// Drains deferred auto-presented summaries the Host pushed without
    /// `reply_to`. The caller paints each and ACKs the receipts it fully
    /// painted.
    pub fn take_undelivered(&mut self) -> Vec<WireFrame> {
        self.state.take_undelivered()
    }

    #[must_use]
    pub fn prepare(&self, payload: WirePayload) -> PreparedRequest {
        PreparedRequest::new(payload)
    }

    /// Sends one prepared request and returns the answer correlated by
    /// `reply_to`, absorbing pipelined presence facts and deferring other
    /// out-of-order frames on the way. The deferred queue only buffers
    /// auto-presented summaries drained by the session's `take_undelivered`;
    /// the answer itself
    /// is read from the socket, so this loops until the correlated answer
    /// arrives (the streaming form of `session::decide_frame`). A
    /// [`StaleRound`](ene_api::v1::round::RoundIntakeOutcomeWire::StaleRound)
    /// answer refreshes the session generation; mismatches are never returned
    /// as answers and never silently dropped.
    ///
    /// Message and request ids go fresh per attempt while the prepared command
    /// identity travels unchanged, so calling this again on the same retained
    /// handle replays one logical command rather than minting a second one.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Transport`] or [`ClientError::Codec`] when the
    /// exchange cannot be moved or framed. Payload semantics are the caller's
    /// job: this helper never interprets the answer beyond the generation
    /// bookkeeping.
    pub async fn execute(
        &mut self,
        prepared: &PreparedRequest,
    ) -> Result<WirePayload, ClientError> {
        self.pump(prepared.frame(self.sender, self.state.generation()))
            .await
    }

    /// One-shot convenience for [`Client::prepare`] plus
    /// [`Client::execute`]. Prefer that pair when the caller must retain the
    /// command identity to re-execute a lost reply; this form mints or takes
    /// the identity but never exposes it.
    ///
    /// # Errors
    ///
    /// Same as [`Client::execute`].
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
            let incoming = read_frame(&mut self.stream).await?;
            if self
                .answer_deletion_demand_if_any(&incoming.payload)
                .await?
            {
                continue;
            }
            match decide_frame(own_message_id, &incoming) {
                FrameDecision::AbsorbPresence(fact) => self.state.observe_presence(&fact),
                FrameDecision::AbsorbBodyHint(_) => {}
                FrameDecision::Answer(payload) => {
                    if let Some(current) = stale_generation_of(&payload) {
                        self.state.note_stale_generation(current);
                    }
                    return Ok(payload);
                }
                FrameDecision::Defer => self.state.push_deferred(incoming),
            }
        }
    }

    /// Answers one unsolicited Host local-erasure demand inline, returning
    /// whether the frame was handled: `true` means either an answer was
    /// written or, in the deferred (GUI participant) mode, the demand was
    /// stashed for later local wiping — never that a reply reached the Host.
    ///
    /// The demand is a control fact, never the reply this session is waiting
    /// for: it is handled and the read continues. The reply carries only class
    /// names and correlation — never a target body — and claims nothing beyond
    /// this process's own local wiping (IPC §17, lifecycle §10).
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

    pub async fn next_frame(&mut self) -> Result<WirePayload, ClientError> {
        loop {
            let payload = read_frame(&mut self.stream).await?.payload;
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
    let bytes = zeroize::Zeroizing::new(
        encode_frame(frame)
            .map_err(|error: CodecError| ClientError::Codec(format!("encode failed: {error}")))?,
    );
    stream.write_all(&bytes).await.map_err(|error| {
        ClientError::Transport(format!("socket write failed: {}", error.kind()))
    })?;
    Ok(())
}

#[cfg(any(unix, windows))]
async fn read_frame(
    stream: &mut (impl tokio::io::AsyncRead + Unpin),
) -> Result<WireFrame, ClientError> {
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
    let mut bytes = zeroize::Zeroizing::new(Vec::with_capacity(4 + claimed));
    bytes.extend_from_slice(&prefix);
    bytes.extend_from_slice(&body);
    decode_frame(&bytes)
        .map(|(frame, _consumed)| frame)
        .map_err(|error: CodecError| ClientError::Codec(format!("decode failed: {error}")))
}

/// Terminal connect refusal: no common protocol major (IPC §7.2, V-11). Names
/// both sides' maxima and the Host's upgrade hint so the operator can move the
/// older side; operational text only, never a secret or body copy. Retrying
/// the same build cannot intersect majors, so it is [`ServerRejected`], not a
/// retryable [`ServerOutcome`].
///
/// [`ServerRejected`]: ClientError::ServerRejected
/// [`ServerOutcome`]: ClientError::ServerOutcome
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

/// Unsupported-platform placeholder: connection and I/O methods return
/// [`ClientError::UnsupportedPlatform`] (transport needs a Unix-domain socket
/// or a Windows named pipe); state-only accessors report the empty/default
/// value. The supported-only helpers (`prepare`/`execute`/
/// `request_observed`/`take_undelivered`) are not available on this platform.
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

#[cfg(test)]
mod pipe_tests {
    use super::pipe_name;

    #[test]
    fn pipe_name_is_stable_and_directory_scoped() {
        assert_eq!(
            pipe_name(std::path::Path::new("/tmp/ene-data")),
            String::from(r"\\.\pipe\ene-2c2d8a5218b804b9"),
            "the pinned vector pins the shared algorithm"
        );
        let first = pipe_name(std::path::Path::new("/tmp/ene-data"));
        assert!(
            first.starts_with(r"\\.\pipe\ene-"),
            "the pipe lives in the machine namespace: {first:?}"
        );
        assert!(
            first == pipe_name(std::path::Path::new("/tmp/ene-data")),
            "the name is stable across processes: {first:?}"
        );
        assert!(
            first != pipe_name(std::path::Path::new("/tmp/other-data")),
            "distinct directories use distinct pipes"
        );
    }
}
