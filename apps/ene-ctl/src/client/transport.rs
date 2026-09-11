//! Socket transport: connect, handshake, request/response loop.

use std::path::Path;

use ene_api::v1::envelope::{ProtocolVersion, WireSender};
use ene_api::v1::handshake::AuthChallenge;
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::CommandWireId;
use ene_credential::pairing_proof_hex;
use ene_plugin_ipc::{CodecError, MAX_FRAME_BYTES, WireFrame, decode_frame, encode_frame};

use crate::device;
use crate::errors::CliError;

use super::frames::{
    capability_frame, frame_for, frame_for_session, missing_secret_guidance, new_incarnation,
    pairing_frame, pending_guidance, proof_frame, retry_frame, stamp_request,
};
use super::session::{
    AuthDecision, FrameDecision, SessionState, decide_auth, decide_frame, stale_generation_of,
};
use super::socket_path;

/// Connected, handshaked Host session (Unix): the stream, the sender identity
/// pairing and authentication fill in, and the observed [`SessionState`].
#[cfg(unix)]
pub struct Client {
    stream: tokio::net::UnixStream,
    sender: WireSender,
    state: SessionState,
}

#[cfg(unix)]
impl Client {
    /// Dials `ene.sock` under `data_dir` and runs the full handshake: pairing,
    /// capability advertisement, challenge authentication, and the first
    /// presence fact.
    ///
    /// Pairing runs on every connect; already-paired descriptors re-pair
    /// idempotently to the same device key. Secret resolution is
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
    /// Returns [`CliError::Transport`] when the socket cannot be reached, a
    /// frame cannot be moved, or the device file cannot be persisted;
    /// [`CliError::Codec`] when a frame cannot be encoded or decoded;
    /// [`CliError::ServerOutcome`] when pairing is still pending Owner
    /// confirmation or was denied (exit code 2: approve on the Host-local
    /// trusted surface, provision the shown secret once, then re-run — no
    /// auto-retry loop); and [`CliError::ServerRejected`] when the Host
    /// negotiates an incompatible version or answers with an unexpected
    /// payload kind.
    pub async fn connect(
        data_dir: &Path,
        descriptor: &str,
        platform: &str,
    ) -> Result<Self, CliError> {
        let path = socket_path(data_dir);
        let mut stream = tokio::net::UnixStream::connect(&path)
            .await
            .map_err(|error| {
                CliError::Transport(format!(
                    "connect to {} failed: {}",
                    path.display(),
                    error.kind()
                ))
            })?;
        let incarnation = new_incarnation();
        let stored = device::load_stored_device(data_dir);
        let (secret, source) = device::resolve_device_secret(
            stored
                .as_ref()
                .and_then(|known| known.secret().map(str::to_string)),
            device::read_bootstrap_secret(),
        );
        let device_id = {
            write_frame(&mut stream, &pairing_frame(descriptor, incarnation)).await?;
            match read_frame(&mut stream).await?.payload {
                WirePayload::PairingResult(result) => match result {
                    ene_api::v1::handshake::PairingResult::Paired { device_id } => device_id,
                    ene_api::v1::handshake::PairingResult::PendingOwnerConfirmation => {
                        return Err(CliError::ServerOutcome(pending_guidance()));
                    }
                    ene_api::v1::handshake::PairingResult::Denied { reason } => {
                        return Err(CliError::ServerOutcome(format!(
                            "pairing denied: {reason}; approve the device on the \\
                             Host-local trusted surface, then re-run ene-ctl"
                        )));
                    }
                },
                unexpected => {
                    return Err(CliError::ServerRejected(format!(
                        "unexpected {} during pairing; expected PairingResult",
                        unexpected.message_type()
                    )));
                }
            }
        };
        // Persist whenever a secret is effective: first provision and
        // one-shot rotation both overwrite the `0600` file (a `Stored` secret
        // still rewrites alongside the fresh pairing device key; a `Rotated`
        // secret replaces the file secret). `Missing` holds no secret, so
        // there is nothing to persist.
        match source {
            device::SecretSource::Stored | device::SecretSource::Rotated => {
                if let Some(secret_value) = secret.as_deref() {
                    device::store_device(
                        data_dir,
                        &device::StoredDevice::new(device_id, secret_value.to_string()),
                    )?;
                }
            }
            device::SecretSource::Missing => {}
        }
        write_frame(
            &mut stream,
            &capability_frame(platform, incarnation, Some(device_id)),
        )
        .await?;
        match read_frame(&mut stream).await?.payload {
            WirePayload::NegotiatedConnection(negotiated) => {
                if !negotiated.version.shares_major_with(&ProtocolVersion::V1) {
                    return Err(CliError::ServerRejected(format!(
                        "negotiated incompatible version {}.{}; expected major 1",
                        negotiated.version.major, negotiated.version.minor
                    )));
                }
            }
            unexpected => {
                return Err(CliError::ServerRejected(format!(
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
            return Err(CliError::ServerRejected(format!(
                "unexpected {} after negotiation; expected AuthChallenge",
                challenge.message_type()
            )));
        };
        session.authenticate(&challenge).await?;
        let fact = session.next_frame().await?;
        if !matches!(fact, WirePayload::PresenceAttribution(_)) {
            return Err(CliError::ServerRejected(format!(
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
    /// Returns [`CliError::Transport`] or [`CliError::Codec`] when the
    /// exchange cannot be moved or framed; [`CliError::ServerOutcome`] when no
    /// secret is provisioned (approve and provision, then re-run) or the Host
    /// rejects the proof (exit code 2: re-approve for a fresh secret and
    /// retry); and [`CliError::ServerRejected`] when the Host answers with an
    /// unexpected payload kind.
    pub async fn authenticate(&mut self, challenge: &AuthChallenge) -> Result<(), CliError> {
        let Some(secret) = self.state.pairing_secret().map(str::to_string) else {
            return Err(CliError::ServerOutcome(missing_secret_guidance()));
        };
        let proof = pairing_proof_hex(&secret, &challenge.nonce);
        let Some(device) = self.sender.device_id else {
            return Err(CliError::ServerRejected(String::from(
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
            AuthDecision::Guidance { message } => Err(CliError::ServerOutcome(message)),
            AuthDecision::Unexpected { message } => Err(CliError::ServerRejected(message)),
        }
    }

    pub fn companion_ref(&self) -> String {
        self.state.companion_ref()
    }

    /// Sends one payload and returns the answer correlated by `reply_to`,
    /// absorbing pipelined presence facts and deferring other out-of-order
    /// frames on the way. The deferred queue is consulted first, so a queued
    /// answer costs no socket I/O; otherwise this loops per
    /// [`super::session::decide_frame`] until the correlated answer arrives. A
    /// [`StaleRound`](ene_api::v1::round::RoundIntakeOutcomeWire::StaleRound)
    /// answer refreshes the session generation; mismatches are never returned
    /// as answers and never silently dropped.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Transport`] or [`CliError::Codec`] when the
    /// exchange cannot be moved or framed. Payload semantics are the caller's
    /// job: this helper never interprets the answer beyond the generation
    /// bookkeeping.
    pub async fn request(&mut self, payload: WirePayload) -> Result<WirePayload, CliError> {
        let mut frame = frame_for_session(payload, self.sender, self.state.generation());
        let _ = stamp_request(&mut frame);
        self.roundtrip(frame).await
    }

    /// Retries one logical send after a lost reply: the command ID travels
    /// (durable idempotency key on the Host) while message and request ids go
    /// fresh for this attempt. Never to change what the command means, and
    /// only within one sender incarnation — the Host binds the key to its
    /// sender epoch, so a retry under a new incarnation is a conflict, not a
    /// replay; a new epoch mints a fresh command instead.
    ///
    /// # Errors
    ///
    /// Same as [`Client::request`].
    pub async fn retry(
        &mut self,
        payload: WirePayload,
        command: CommandWireId,
    ) -> Result<WirePayload, CliError> {
        self.roundtrip(retry_frame(
            payload,
            self.sender,
            self.state.generation(),
            command,
        ))
        .await
    }

    async fn roundtrip(&mut self, frame: WireFrame) -> Result<WirePayload, CliError> {
        let own_message_id = frame.envelope.message_id;
        write_frame(&mut self.stream, &frame).await?;
        if let Some(queued) = self.state.take_deferred_reply(own_message_id) {
            if let Some(current) = stale_generation_of(&queued) {
                self.state.note_stale_generation(current);
            }
            return Ok(queued);
        }
        loop {
            let incoming = read_frame(&mut self.stream).await?;
            match decide_frame(own_message_id, &incoming) {
                FrameDecision::AbsorbPresence(fact) => self.state.observe_presence(&fact),
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

    /// Fire-and-forget: the Host applies presentation confirmations silently
    /// and answers nothing.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Transport`] or [`CliError::Codec`] when the frame
    /// cannot be moved or encoded.
    pub async fn notify(&mut self, payload: WirePayload) -> Result<(), CliError> {
        write_frame(&mut self.stream, &frame_for(payload, self.sender)).await
    }

    /// Stream follower for `send`: an authoritative
    /// [`PresenceAttribution`](ene_api::v1::payload::WirePayload::PresenceAttribution)
    /// fact refreshes the session generation and is still returned, so the
    /// caller decides what to display.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Transport`] or [`CliError::Codec`] when the next
    /// frame cannot be read or decoded.
    pub async fn next_frame(&mut self) -> Result<WirePayload, CliError> {
        let payload = read_frame(&mut self.stream).await?.payload;
        if let WirePayload::PresenceAttribution(fact) = &payload {
            self.state.observe_presence(fact);
        }
        Ok(payload)
    }
}

#[cfg(unix)]
async fn write_frame(
    stream: &mut tokio::net::UnixStream,
    frame: &WireFrame,
) -> Result<(), CliError> {
    use tokio::io::AsyncWriteExt as _;
    let bytes = encode_frame(frame)
        .map_err(|error: CodecError| CliError::Codec(format!("encode failed: {error}")))?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|error| CliError::Transport(format!("socket write failed: {}", error.kind())))?;
    Ok(())
}

/// 4-byte big-endian length prefix, then the body; the cap is checked before
/// any body-sized allocation, so a hostile prefix cannot drive unbounded
/// allocation.
#[cfg(unix)]
async fn read_frame(stream: &mut tokio::net::UnixStream) -> Result<WireFrame, CliError> {
    use tokio::io::AsyncReadExt as _;
    let mut prefix = [0_u8; 4];
    stream
        .read_exact(&mut prefix)
        .await
        .map_err(|error| CliError::Transport(format!("socket read failed: {}", error.kind())))?;
    let claimed = u32::from_be_bytes(prefix) as usize;
    if claimed > MAX_FRAME_BYTES {
        return Err(CliError::Codec(format!(
            "frame body of {claimed} bytes exceeds the 256 KiB cap"
        )));
    }
    let mut body = vec![0_u8; claimed];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|error| CliError::Transport(format!("socket read failed: {}", error.kind())))?;
    let mut bytes = Vec::with_capacity(4 + claimed);
    bytes.extend_from_slice(&prefix);
    bytes.extend_from_slice(&body);
    decode_frame(&bytes)
        .map(|(frame, _consumed)| frame)
        .map_err(|error: CodecError| CliError::Codec(format!("decode failed: {error}")))
}

/// Non-Unix placeholder: same surface, always unsupported. Every method
/// returns [`CliError::UnsupportedPlatform`]: transport needs a Unix-domain
/// socket.
#[cfg(windows)]
pub struct Client {
    _sealed: (),
}

#[cfg(windows)]
impl Client {
    pub async fn connect(
        _data_dir: &Path,
        _descriptor: &str,
        _platform: &str,
    ) -> Result<Self, CliError> {
        Err(CliError::UnsupportedPlatform("unix socket transport"))
    }

    pub async fn request(&mut self, _payload: WirePayload) -> Result<WirePayload, CliError> {
        Err(CliError::UnsupportedPlatform("unix socket transport"))
    }

    pub async fn authenticate(
        &mut self,
        _challenge: &ene_api::v1::handshake::AuthChallenge,
    ) -> Result<(), CliError> {
        Err(CliError::UnsupportedPlatform("unix socket transport"))
    }

    pub async fn next_frame(&mut self) -> Result<WirePayload, CliError> {
        Err(CliError::UnsupportedPlatform("unix socket transport"))
    }

    pub async fn notify(&mut self, _payload: WirePayload) -> Result<(), CliError> {
        Err(CliError::UnsupportedPlatform("unix socket transport"))
    }

    /// No session ever observes presence on this platform, so every request
    /// carries the bootstrap fallback; the Host revalidates it rather than
    /// attributing through it.
    pub fn companion_ref(&self) -> String {
        String::from(crate::cmds::DEFAULT_COMPANION_REF)
    }
}
