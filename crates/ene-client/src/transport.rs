//! Socket transport: connect, handshake, request/response loop.

use std::path::Path;

use crate::pairing::pairing_proof_hex;
use ene_api::v1::deletion::{DeletionDemand, LocalErasureResult};
#[cfg(any(unix, windows))]
use ene_api::v1::envelope::{ProtocolVersion, WireSender};
use ene_api::v1::handshake::AuthChallenge;
use ene_api::v1::payload::WirePayload;
#[cfg(any(unix, windows))]
use ene_api::v1::refs::WireMessageId;
#[cfg(any(unix, windows))]
use ene_plugin_ipc::{CodecError, MAX_FRAME_BYTES, WireFrame, decode_frame, encode_frame};

#[cfg(any(unix, windows))]
use crate::device;
use crate::error::ClientError;

#[cfg(any(unix, windows))]
use super::frames::{
    PreparedRequest, capability_frame, frame_for, missing_secret_guidance, pairing_frame,
    pending_guidance, proof_frame, unreadable_device_file_guidance,
};
#[cfg(any(unix, windows))]
use super::session::{
    AuthDecision, FrameDecision, SessionState, decide_auth, decide_frame, stale_generation_of,
};
#[cfg(unix)]
use super::socket_path;

/// Connected, handshaked Host session: the stream, the sender identity
/// pairing and authentication fill in, and the observed [`SessionState`].
/// Unix dials `ene.sock`; Windows opens the data directory's named pipe
/// (see `pipe_name`). Everything after the dial — pairing poll, capability,
/// challenge authentication, request/response correlation — is shared.
#[cfg(any(unix, windows))]
pub struct Client {
    stream: Stream,
    sender: WireSender,
    state: SessionState,
}

#[cfg(unix)]
type Stream = tokio::net::UnixStream;

#[cfg(windows)]
type Stream = tokio::net::windows::named_pipe::NamedPipeClient;

/// Named-pipe name for one Host data directory, matching the Host listener.
///
/// Named pipes live in a flat per-machine namespace, so the data directory is
/// folded into the name: FNV-1a (64-bit, fixed offsets, so the name is stable
/// across processes) over its string form, rendered as hex. This duplicates
/// the Host listener's `pipe_name` (`ene-core/src/conn_pipe.rs`) on purpose:
/// `ene-ctl` must not depend on `ene-core`, and the algorithm is pinned by
/// the shared test vector below rather than by shared code. Pure (no OS
/// calls), so the `test` gate keeps it compiled for the Linux-runnable
/// vector test; only the dial site is Windows-only.
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
    /// Dials `ene.sock` under `data_dir` and runs the full handshake: pairing,
    /// capability advertisement, challenge authentication, and the first
    /// presence fact.
    ///
    /// Pairing runs only without a stored device: a first run opens or polls
    /// a pending request by its opaque pending id (remembered in the
    /// `client-pending.json` progress file), while a stored device skips
    /// pairing and resolves its DeviceWireId at capability time, never by
    /// descriptor (#1389). Secret resolution is
    /// [`device::resolve_device_secret`]'s. When pairing succeeds while this
    /// process holds a secret, the `{device_id, secret}` pair is persisted to
    /// the `0600` device file before capability runs (fail-closed: a store
    /// failure aborts the connect rather than running with an unpersisted
    /// secret). With no secret anywhere, the connect proceeds into capability
    /// and then fails closed at the mandatory post-negotiation challenge with
    /// provisioning guidance: an unauthenticated session never reaches domain
    /// service.
    ///
    /// Capability advertises with the paired device ID; exactly one frame is
    /// read back here and must contain the negotiated terms.
    ///
    /// There is no Host "unknown device" outcome on capability — an ID the
    /// Host no longer knows fails later at the domain gate (close plus
    /// `DisconnectNotice`).
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Transport`] when the socket cannot be reached, a
    /// frame cannot be moved, or the device file cannot be persisted;
    /// [`ClientError::Codec`] when a frame cannot be encoded or decoded;
    /// [`ClientError::ServerOutcome`] when pairing is still pending Owner
    /// confirmation or was denied (exit code 2: approve on the Host-local
    /// trusted surface, provision the shown secret once, then re-run — no
    /// auto-retry loop); and [`ClientError::ServerRejected`] when the Host
    /// negotiates an incompatible version or answers with an unexpected
    /// payload kind.
    pub async fn connect(
        data_dir: &Path,
        descriptor: &str,
        platform: &str,
    ) -> Result<Self, ClientError> {
        Self::connect_with_bootstrap(
            data_dir,
            descriptor,
            platform,
            device::read_bootstrap_secret(),
        )
        .await
    }

    /// Same handshake as [`Self::connect`], with an in-process pairing
    /// bootstrap instead of `ENE_PAIRING_SECRET`.
    ///
    /// First-party GUI uses this after a seated `DeviceApproved` so the
    /// pairing secret never enters the process environment. The value is
    /// the env-side input of [`device::resolve_device_secret`] and is not
    /// logged.
    pub async fn connect_with_bootstrap(
        data_dir: &Path,
        descriptor: &str,
        platform: &str,
        bootstrap_secret: Option<String>,
    ) -> Result<Self, ClientError> {
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
        let file_state = device::load_stored_device(data_dir);
        let stored_device = file_state.stored().cloned();
        let (secret, source) = device::resolve_device_secret(
            stored_device
                .as_ref()
                .and_then(|known| known.secret().map(str::to_string)),
            bootstrap_secret,
        );
        // A degraded file (unreadable/corrupt/blank secret) with no bootstrap
        // secret cannot authenticate with anything: report the degraded state
        // for recovery instead of taking the first-run provisioning path.
        if secret.is_none()
            && matches!(
                file_state,
                device::DeviceFileState::Unreadable | device::DeviceFileState::Malformed
            )
        {
            return Err(ClientError::ServerOutcome(unreadable_device_file_guidance()));
        }
        // Pairing runs only without a stored device: a first run (or a run
        // whose device file is gone) opens or polls a pending request by its
        // opaque id, remembered in the `client-pending.json` progress file,
        // while a stored device skips pairing and resolves its DeviceWireId
        // at capability time, never by descriptor (#1389). The remembered id
        // is kept after success too: it is the durable poll key that lets a
        // later run with a lost device file re-resolve the same approval
        // instead of opening a duplicate pending. A stale id (restart clear,
        // wiped Host) converges on a fresh pending Host-side, so the file is
        // self-healing and only a denial drops it.
        let device_id = match (
            stored_device.as_ref().map(|known| known.device_id),
            secret.is_some(),
        ) {
            (Some(known), true) => known,
            _ => {
                let poll = device::load_pending_id(data_dir);
                write_frame(&mut stream, &pairing_frame(descriptor, incarnation, poll)).await?;
                match read_frame(&mut stream).await?.payload {
                    WirePayload::PairingResult(result) => match result {
                        ene_api::v1::handshake::PairingResult::Paired { device_id } => device_id,
                        ene_api::v1::handshake::PairingResult::PendingOwnerConfirmation {
                            pending_id,
                        } => {
                            device::store_pending_id(data_dir, &pending_id)?;
                            return Err(ClientError::ServerOutcome(pending_guidance()));
                        }
                        ene_api::v1::handshake::PairingResult::Denied { reason } => {
                            device::clear_pending_id(data_dir);
                            return Err(ClientError::ServerOutcome(format!(
                                "pairing denied: {reason}; approve the pending ID on the \
                                 Host-local trusted surface, then re-run ene-ctl"
                            )));
                        }
                    },
                    unexpected => {
                        return Err(ClientError::ServerRejected(format!(
                            "unexpected {} during pairing; expected PairingResult",
                            unexpected.message_type()
                        )));
                    }
                }
            }
        };
        // Whether the effective secret must become durable client state is
        // decided before authentication but executed only after the Host
        // accepted the ownership proof: a first provision, a rotation, or a
        // changed device identity writes; a plain `Stored` reconnect does not.
        let must_persist = device::must_persist_after_acceptance(
            source,
            stored_device.as_ref().map(|known| known.device_id),
            device_id,
        );
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
            unexpected => {
                return Err(ClientError::ServerRejected(format!(
                    "unexpected {} during capability negotiation; expected NegotiatedConnection",
                    unexpected.message_type()
                )));
            }
        }
        let mut state = SessionState::default();
        if let Some(secret_value) = secret {
            state.set_pairing_secret(secret_value);
        }
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
        // The ownership proof was Accepted: only now does a newly
        // provisioned or rotated secret become the current durable client
        // identity. A rejected proof returned above, so a wrong bootstrap
        // value can never replace a working file.
        if must_persist {
            match session.state.pairing_secret() {
                Some(secret_value) => device::store_device(
                    data_dir,
                    &device::StoredDevice::new(device_id, secret_value.to_owned()),
                )?,
                None => {
                    return Err(ClientError::Transport(String::from(
                        "accepted authentication lost the client device secret",
                    )));
                }
            }
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
    /// the accepted connection key into the sender (for all later frames) and
    /// the session mirror. [`Client::connect`] calls this for the
    /// post-negotiation challenge; call it only with a Host-minted
    /// [`AuthChallenge`].
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Transport`] or [`ClientError::Codec`] when the
    /// exchange cannot be moved or framed; [`ClientError::ServerOutcome`] when no
    /// secret is provisioned (approve and provision, then re-run) or the Host
    /// rejects the proof (exit code 2: re-approve for a fresh secret and
    /// retry); and [`ClientError::ServerRejected`] when the Host answers with an
    /// unexpected payload kind.
    pub async fn authenticate(&mut self, challenge: &AuthChallenge) -> Result<(), ClientError> {
        let Some(secret) = self.state.pairing_secret().map(str::to_string) else {
            return Err(ClientError::ServerOutcome(missing_secret_guidance()));
        };
        let proof = pairing_proof_hex(&secret, &challenge.nonce);
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
                self.state.set_connection(connection_id);
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

    /// Prepares one logical send before I/O: the returned handle carries the
    /// payload and the command identity a transport retry must reuse.
    ///
    /// Use this with [`Client::execute`] instead of [`Client::request`]
    /// whenever a lost reply must be retryable: the prepared
    /// [`PreparedRequest`] keeps the command identity caller-side, and a
    /// command payload is prepared with its canonical identity (a
    /// [`ManagementIntent`](ene_api::v1::management::ManagementIntent) keeps
    /// its `intent_id`; a pure request carries no command identity at all).
    #[must_use]
    pub fn prepare(&self, payload: WirePayload) -> PreparedRequest {
        PreparedRequest::new(payload)
    }

    /// Sends one prepared request and returns the answer correlated by
    /// `reply_to`, absorbing pipelined presence facts and deferring other
    /// out-of-order frames on the way. The deferred queue is consulted first,
    /// so a queued answer costs no socket I/O; otherwise this loops until the
    /// correlated answer arrives (the streaming form of
    /// [`super::session::decide_frame`]). A
    /// [`StaleRound`](ene_api::v1::round::RoundIntakeOutcomeWire::StaleRound)
    /// answer refreshes the session generation; mismatches are never returned
    /// as answers and never silently dropped.
    ///
    /// Message and request ids go fresh per attempt while the prepared command
    /// identity travels unchanged, so calling this again through
    /// [`Client::retry`] replays one logical command rather than minting a
    /// second one.
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
        self.roundtrip(prepared.frame(self.sender, self.state.generation()))
            .await
    }

    /// Re-sends one prepared command after a lost reply: the command ID
    /// travels (durable idempotency key on the Host) while message and request
    /// ids go fresh for this attempt. Never to change what the command means,
    /// and only within one sender incarnation — the Host binds the key to its
    /// sender epoch, so a retry under a new incarnation is a conflict, not a
    /// replay; re-prepare a fresh command under the new incarnation instead.
    ///
    /// A prepared pure request has no command identity; re-executing it is a
    /// fresh request/response attempt.
    ///
    /// # Errors
    ///
    /// Same as [`Client::execute`].
    pub async fn retry(&mut self, prepared: &PreparedRequest) -> Result<WirePayload, ClientError> {
        self.execute(prepared).await
    }

    /// One-shot convenience for [`Client::prepare`] plus
    /// [`Client::execute`]. Prefer that pair when the caller must retain the
    /// command identity to [`Client::retry`] a lost reply; this form mints or
    /// takes the identity but never exposes it.
    ///
    /// # Errors
    ///
    /// Same as [`Client::execute`].
    pub async fn request(&mut self, payload: WirePayload) -> Result<WirePayload, ClientError> {
        let prepared = self.prepare(payload);
        self.execute(&prepared).await
    }

    /// Request/response carrying presentation observed marks: the caller
    /// echoes the round and generation a summary showed so the Host can
    /// compare them against the receipt. Used for `UndeliveredAck`, which
    /// answers its typed outcome.
    ///
    /// # Errors
    ///
    /// Same as [`Client::execute`].
    pub async fn request_observed(
        &mut self,
        payload: WirePayload,
        round: Option<ene_api::v1::refs::RoundWireId>,
    ) -> Result<WirePayload, ClientError> {
        use super::frames::observed_frame;
        use ene_api::v1::refs::RequestWireId;

        let mut frame = observed_frame(payload, self.sender, self.state.generation(), round);
        let own_message_id = frame.envelope.message_id;
        frame.envelope.correlation.request_id = Some(RequestWireId(uuid::Uuid::new_v4()));
        self.pump(frame, own_message_id).await
    }

    async fn roundtrip(&mut self, frame: WireFrame) -> Result<WirePayload, ClientError> {
        let own_message_id = frame.envelope.message_id;
        self.pump(frame, own_message_id).await
    }

    async fn pump(
        &mut self,
        frame: WireFrame,
        own_message_id: WireMessageId,
    ) -> Result<WirePayload, ClientError> {
        write_frame(&mut self.stream, &frame).await?;
        if let Some(queued) = self.state.take_deferred_reply(own_message_id) {
            if let Some(current) = stale_generation_of(&queued) {
                self.state.note_stale_generation(current);
            }
            return Ok(queued);
        }
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
    /// whether an answer was written.
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
        let result = self.local_erasure_result(demand);
        write_frame(
            &mut self.stream,
            &frame_for(WirePayload::LocalErasureResult(result), self.sender),
        )
        .await?;
        Ok(true)
    }

    /// Builds this session's local-erasure report for one demand.
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

    /// Fire-and-forget: the Host applies presentation confirmations silently
    /// and answers nothing.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Transport`] or [`ClientError::Codec`] when the frame
    /// cannot be moved or encoded.
    pub async fn notify(&mut self, payload: WirePayload) -> Result<(), ClientError> {
        write_frame(&mut self.stream, &frame_for(payload, self.sender)).await
    }

    /// Stream follower for `send`: an authoritative
    /// [`PresenceAttribution`](ene_api::v1::payload::WirePayload::PresenceAttribution)
    /// fact refreshes the session generation and is still returned, so the
    /// caller decides what to display. A Host local-erasure demand is answered
    /// inline and never surfaces as a stream frame.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Transport`] or [`ClientError::Codec`] when the next
    /// frame cannot be read or decoded.
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
    let bytes = encode_frame(frame)
        .map_err(|error: CodecError| ClientError::Codec(format!("encode failed: {error}")))?;
    stream.write_all(&bytes).await.map_err(|error| {
        ClientError::Transport(format!("socket write failed: {}", error.kind()))
    })?;
    Ok(())
}

/// 4-byte big-endian length prefix, then the body; the cap is checked before
/// any body-sized allocation, so a hostile prefix cannot drive unbounded
/// allocation. Shared by the Unix socket and the Windows named pipe: both
/// transports carry the same length-prefixed frames.
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
    let mut body = vec![0_u8; claimed];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|error| ClientError::Transport(format!("socket read failed: {}", error.kind())))?;
    let mut bytes = Vec::with_capacity(4 + claimed);
    bytes.extend_from_slice(&prefix);
    bytes.extend_from_slice(&body);
    decode_frame(&bytes)
        .map(|(frame, _consumed)| frame)
        .map_err(|error: CodecError| ClientError::Codec(format!("decode failed: {error}")))
}

/// Unsupported-platform placeholder: same surface, always unsupported.
/// Every method returns [`ClientError::UnsupportedPlatform`]: transport needs a
/// Unix-domain socket or a Windows named pipe.
#[cfg(not(any(unix, windows)))]
pub struct Client {
    _sealed: (),
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

    pub async fn connect_with_bootstrap(
        _data_dir: &Path,
        _descriptor: &str,
        _platform: &str,
        _bootstrap_secret: Option<String>,
    ) -> Result<Self, ClientError> {
        Err(ClientError::UnsupportedPlatform("no supported transport"))
    }

    pub async fn request(&mut self, _payload: WirePayload) -> Result<WirePayload, ClientError> {
        Err(ClientError::UnsupportedPlatform("no supported transport"))
    }

    pub async fn authenticate(&mut self, _challenge: &AuthChallenge) -> Result<(), ClientError> {
        Err(ClientError::UnsupportedPlatform("no supported transport"))
    }

    pub async fn next_frame(&mut self) -> Result<WirePayload, ClientError> {
        Err(ClientError::UnsupportedPlatform("no supported transport"))
    }

    pub async fn notify(&mut self, _payload: WirePayload) -> Result<(), ClientError> {
        Err(ClientError::UnsupportedPlatform("no supported transport"))
    }

    /// No session ever observes presence on this platform, so every request
    /// carries the bootstrap fallback; the Host revalidates it rather than
    /// attributing through it.
    pub fn companion_ref(&self) -> String {
        String::from(crate::DEFAULT_COMPANION_REF)
    }
}

#[cfg(test)]
mod pipe_tests {
    use super::pipe_name;

    /// The client and the Host listener must derive the same pipe name from
    /// one data directory; the vector pins the FNV-1a algorithm they share
    /// without sharing code (mirrored in `ene-core`'s `conn_pipe` test, which
    /// only runs on Windows).
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
