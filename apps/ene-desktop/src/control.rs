//! Host-local control for the first-party GUI (Stage 7 A1).
//!
//! Two channels, matching the Host's split:
//!
//! - [`RequesterClient`] dials the **requester listener**. It carries
//!   requests and non-secret request state, exactly like the console's
//!   `ene-core approve-*`. Opening it grants nothing.
//! - [`ConfirmationClient`] speaks the **inherited private channel** the Host
//!   handed to this process when it spawned it. Only here do challenges,
//!   secret intake, and session completion exist.
//!
//! The GUI is both: it asks for its own pairing, credential intake, and
//! deletion confirmation through the requester listener, and answers the
//! challenge the Host pushes to it on the private channel.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ene_local_control::{
    ControlOp, FromConfirmation, FromHost, GuiChannel, PendingDeletionPreview, ToConfirmation,
    ToHost,
};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::ui::DesktopError;

/// How long the Owner's surface waits for the Host to push a challenge it
/// just asked for, and for the boundary to answer a completion.
const CONFIRMATION_WAIT: Duration = Duration::from_secs(30);

/// One challenge waiting for the Owner's direct gesture.
#[derive(Debug, Clone)]
pub struct PendingChallenge {
    pub session_id: Uuid,
    pub op: ControlOp,
    pub target: String,
    nonce: String,
}

impl PendingChallenge {
    /// The normalized target the Owner's surface displays.
    #[must_use]
    pub fn display_target(&self) -> &str {
        &self.target
    }
}

/// Requester-side client for the serving Host's local listener.
#[derive(Debug, Clone)]
pub struct RequesterClient {
    data_dir: PathBuf,
}

impl RequesterClient {
    #[must_use]
    pub fn new(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
        }
    }

    /// Sends one request and reads its answer.
    ///
    /// # Errors
    ///
    /// [`DesktopError::Transport`] when the requester listener is unreachable,
    /// [`DesktopError::Protocol`] when the answer cannot be decoded.
    pub async fn request(&self, message: &ToHost) -> Result<FromHost, DesktopError> {
        let mut stream = connect_requester(&self.data_dir).await?;
        write_requester_frame(&mut stream, message).await?;
        read_requester_frame(&mut stream).await
    }

    /// Asks the Host to open (or raise) the official GUI.
    ///
    /// # Errors
    ///
    /// As [`RequesterClient::request`]; `Ok(false)` means no GUI could be
    /// started, which is a state, not a fault.
    pub async fn open_desktop(&self) -> Result<bool, DesktopError> {
        match self.request(&ToHost::OpenDesktop).await? {
            FromHost::DesktopOpened => Ok(true),
            FromHost::DesktopUnavailable => Ok(false),
            other => Err(DesktopError::Control(format!(
                "OpenDesktop answered {other:?}"
            ))),
        }
    }

    /// Reads the staged Targeted Deletion request identities.
    ///
    /// # Errors
    ///
    /// As [`RequesterClient::request`].
    pub async fn list_pending_deletions(
        &self,
    ) -> Result<Vec<PendingDeletionPreview>, DesktopError> {
        match self.request(&ToHost::PendingDeletions).await? {
            FromHost::PendingDeletions { requests } => Ok(requests),
            FromHost::DeniedByBoundary => Err(DesktopError::DeniedByBoundary),
            other => Err(DesktopError::Control(format!(
                "expected pending deletions, got {other:?}"
            ))),
        }
    }

    /// Requests one high-privilege object under a Host-issued request id.
    ///
    /// # Errors
    ///
    /// As [`RequesterClient::request`].
    async fn request_accepted(&self, message: &ToHost) -> Result<String, DesktopError> {
        match self.request(message).await? {
            FromHost::RequestAccepted { request_id } => Ok(request_id),
            FromHost::DeniedByBoundary => Err(DesktopError::DeniedByBoundary),
            other => Err(DesktopError::Control(format!(
                "the request was not accepted: {other:?}"
            ))),
        }
    }

    /// Reads one accepted request's non-secret state by its Host-issued id.
    ///
    /// # Errors
    ///
    /// As [`RequesterClient::request`].
    pub async fn request_status(
        &self,
        request_id: &str,
    ) -> Result<ene_local_control::RequestState, DesktopError> {
        match self
            .request(&ToHost::RequestStatus {
                request_id: request_id.to_string(),
            })
            .await?
        {
            FromHost::RequestStatus { state, .. } => Ok(state),
            FromHost::DeniedByBoundary => Err(DesktopError::DeniedByBoundary),
            other => Err(DesktopError::Control(format!(
                "expected request status, got {other:?}"
            ))),
        }
    }
}

/// The GUI's end of the Host's private confirmation channel.
///
/// A dedicated thread owns the channel's blocking reads and forwards every
/// frame here, so neither the Slint event loop nor the worker ever blocks on
/// the pipe.
pub struct ConfirmationClient {
    requester: RequesterClient,
    channel: GuiChannel,
    incoming: mpsc::Receiver<FromConfirmation>,
    closed: bool,
    challenge: Option<PendingChallenge>,
}

impl ConfirmationClient {
    /// Adopts one private channel and starts its reader thread.
    ///
    /// # Errors
    ///
    /// [`DesktopError::Transport`] when the reader thread cannot start.
    pub fn adopt(data_dir: &Path, channel: GuiChannel) -> Result<Self, DesktopError> {
        let (sender, incoming) = mpsc::channel::<FromConfirmation>(16);
        let mut reader = channel.try_clone().map_err(|error| {
            DesktopError::Transport(format!("confirmation channel clone: {error}"))
        })?;
        std::thread::Builder::new()
            .name(String::from("ene-confirmation-read"))
            .spawn(move || {
                loop {
                    match reader.recv() {
                        Ok(Some(frame)) => {
                            if sender.blocking_send(frame).is_err() {
                                return;
                            }
                        }
                        Ok(None) | Err(_) => return,
                    }
                }
            })
            .map_err(|error| DesktopError::Transport(format!("confirmation reader: {error}")))?;
        Ok(Self {
            requester: RequesterClient::new(data_dir),
            channel,
            incoming,
            closed: false,
            challenge: None,
        })
    }

    /// True once the private channel ended. The GUI must then discard its
    /// sessions and close: it is no longer the Host's confirmation surface.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    #[must_use]
    pub fn pending_challenge(&self) -> Option<&PendingChallenge> {
        self.challenge.as_ref()
    }

    /// Reads the staged Targeted Deletion request identities.
    ///
    /// # Errors
    ///
    /// As [`RequesterClient::request`].
    pub async fn list_pending_deletions(
        &self,
    ) -> Result<Vec<PendingDeletionPreview>, DesktopError> {
        self.requester.list_pending_deletions().await
    }

    /// Asks the Owner's surface to resume one Held deletion operation. The
    /// operation was already admitted; this is not a second destructive
    /// confirmation.
    ///
    /// # Errors
    ///
    /// As [`ConfirmationClient::await_outcome`].
    pub async fn request_deletion_resume(
        &mut self,
        operation: &str,
        sweep: u64,
    ) -> Result<FromConfirmation, DesktopError> {
        // Resume is not a destructive admission, so the Host answers it on the
        // channel directly instead of minting a challenge: the Owner's gesture
        // is the request itself.
        self.send(&ToConfirmation::DeletionResume {
            operation: operation.to_string(),
            sweep,
        })?;
        self.await_outcome().await
    }

    /// Forgets the local authority reference. This never rolls back an
    /// operation the Owner already sent.
    pub(crate) fn discard_pending(&mut self) {
        self.challenge = None;
    }

    /// Asks for one device approval and waits for its challenge.
    ///
    /// # Errors
    ///
    /// As [`ConfirmationClient::await_challenge`].
    pub async fn request_device_approve(&mut self, pending_id: &str) -> Result<(), DesktopError> {
        self.requester
            .request_accepted(&ToHost::RequestDeviceApprove {
                pending_id: pending_id.to_string(),
            })
            .await?;
        self.await_challenge(ControlOp::DeviceApprove).await
    }

    /// Asks to register a credential pair and waits for its intake challenge.
    ///
    /// The value is not part of the request: the Owner types it on the
    /// surface this challenge opens.
    ///
    /// # Errors
    ///
    /// As [`ConfirmationClient::await_challenge`].
    pub async fn request_credential_put(
        &mut self,
        provider: &str,
        label: &str,
    ) -> Result<(), DesktopError> {
        self.requester
            .request_accepted(&ToHost::RequestCredentialPut {
                provider: provider.to_string(),
                label: label.to_string(),
            })
            .await?;
        self.await_challenge(ControlOp::CredentialPut).await
    }

    /// Asks for one Targeted Deletion confirmation and waits for its challenge.
    ///
    /// # Errors
    ///
    /// As [`ConfirmationClient::await_challenge`].
    pub async fn request_deletion_confirm(&mut self, request_id: &str) -> Result<(), DesktopError> {
        self.requester
            .request_accepted(&ToHost::RequestDeletionConfirm {
                request_id: request_id.to_string(),
            })
            .await?;
        self.await_challenge(ControlOp::DeletionConfirm).await
    }

    /// Waits for the Host to push the challenge of the operation just
    /// requested.
    ///
    /// # Errors
    ///
    /// [`DesktopError::Protocol`] when a challenge for another operation
    /// arrives first, and [`DesktopError::Transport`] when none does.
    async fn await_challenge(&mut self, expected: ControlOp) -> Result<(), DesktopError> {
        let frame = self.next_frame().await?;
        match frame {
            FromConfirmation::ConfirmationChallenge {
                session_id,
                op,
                target,
                nonce,
                ..
            } if op == expected => {
                self.challenge = Some(PendingChallenge {
                    session_id,
                    op,
                    target,
                    nonce,
                });
                Ok(())
            }
            FromConfirmation::ConfirmationChallenge { op, .. } => Err(DesktopError::Protocol(
                format!("expected a {expected:?} challenge, got {op:?}"),
            )),
            other => Err(DesktopError::Control(format!(
                "expected a challenge, got {other:?}"
            ))),
        }
    }

    /// The Owner's direct confirmation on a non-secret challenge surface.
    ///
    /// # Errors
    ///
    /// [`DesktopError::Protocol`] when no challenge is live, and
    /// [`DesktopError::Transport`] when the boundary does not answer.
    pub async fn complete_pending(&mut self) -> Result<FromConfirmation, DesktopError> {
        let Some(challenge) = self.challenge.take() else {
            return Err(DesktopError::Protocol(String::from(
                "no live confirmation session",
            )));
        };
        self.send(&ToConfirmation::SessionComplete {
            session_id: challenge.session_id,
            nonce: challenge.nonce,
        })?;
        self.await_outcome().await
    }

    /// The Owner's direct confirmation of a credential registration: the
    /// value enters the private channel that presented the challenge, then
    /// the session completes.
    ///
    /// # Errors
    ///
    /// [`DesktopError::Protocol`] when no credential challenge is live, and
    /// [`DesktopError::Transport`] when the boundary does not answer.
    pub async fn complete_credential(
        &mut self,
        secret: String,
    ) -> Result<FromConfirmation, DesktopError> {
        let Some(challenge) = self.challenge.take() else {
            return Err(DesktopError::Protocol(String::from(
                "no live confirmation session",
            )));
        };
        if challenge.op != ControlOp::CredentialPut {
            return Err(DesktopError::Protocol(String::from(
                "the live challenge is not a credential intake",
            )));
        }
        let (provider, label) = challenge
            .target
            .split_once(':')
            .map(|(provider, label)| (provider.to_string(), label.to_string()))
            .ok_or_else(|| {
                DesktopError::Protocol(String::from("the credential target is malformed"))
            })?;
        self.send(&ToConfirmation::CredentialSecret {
            session_id: challenge.session_id,
            nonce: challenge.nonce.clone(),
            provider,
            label,
            secret: ene_local_control::RedactedSecret::new(secret),
        })?;
        match self.await_outcome().await? {
            FromConfirmation::Outcome(ene_local_control::ControlOutcome::CredentialStaged {
                ..
            }) => {}
            other => {
                return Ok(other);
            }
        }
        self.send(&ToConfirmation::SessionComplete {
            session_id: challenge.session_id,
            nonce: challenge.nonce,
        })?;
        self.await_outcome().await
    }

    /// The Owner's refusal on the challenge surface. Applies nothing.
    ///
    /// # Errors
    ///
    /// As [`ConfirmationClient::complete_pending`].
    pub async fn reject_pending(&mut self) -> Result<FromConfirmation, DesktopError> {
        let Some(challenge) = self.challenge.take() else {
            return Err(DesktopError::Protocol(String::from(
                "no live confirmation session",
            )));
        };
        self.send(&ToConfirmation::SessionReject {
            session_id: challenge.session_id,
            nonce: challenge.nonce,
        })?;
        self.await_outcome().await
    }

    /// Session-less self-declaration: always refused by the boundary, and a
    /// regression guard that the private channel does not widen it.
    ///
    /// # Errors
    ///
    /// As [`ConfirmationClient::await_outcome`].
    pub async fn send_confirmed_true(&mut self) -> Result<FromConfirmation, DesktopError> {
        self.send(&ToConfirmation::ConfirmedTrue)?;
        self.await_outcome().await
    }

    /// Sends one frame on the private channel.
    ///
    /// # Errors
    ///
    /// [`DesktopError::Transport`] when the channel ended.
    fn send(&mut self, frame: &ToConfirmation) -> Result<(), DesktopError> {
        match self.channel.send(frame) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.closed = true;
                Err(DesktopError::Transport(format!(
                    "confirmation channel: {error}"
                )))
            }
        }
    }

    /// Waits for the outcome of the operation just completed.
    ///
    /// # Errors
    ///
    /// [`DesktopError::Transport`] when the channel ends or the Host stays
    /// silent past [`CONFIRMATION_WAIT`], and [`DesktopError::Control`] when
    /// a challenge arrives in place of an outcome.
    async fn await_outcome(&mut self) -> Result<FromConfirmation, DesktopError> {
        loop {
            let frame = self.next_frame().await?;
            match frame {
                FromConfirmation::Outcome(_) => return Ok(frame),
                // A boundary refusal is a domain answer, not a fault: the
                // caller renders it and never retries it as transport.
                FromConfirmation::DeniedByBoundary => return Ok(frame),
                FromConfirmation::Unavailable => return Ok(frame),
                FromConfirmation::ConfirmationChallenge {
                    session_id,
                    op,
                    target,
                    nonce,
                    ..
                } => {
                    // A second request's challenge may arrive while the first
                    // is settling; keep it for its own Owner gesture.
                    self.challenge = Some(PendingChallenge {
                        session_id,
                        op,
                        target,
                        nonce,
                    });
                }
            }
        }
    }

    /// Reads the next frame the Host pushed, within the bound.
    async fn next_frame(&mut self) -> Result<FromConfirmation, DesktopError> {
        match tokio::time::timeout(CONFIRMATION_WAIT, self.incoming.recv()).await {
            Ok(Some(frame)) => Ok(frame),
            Ok(None) => {
                self.closed = true;
                Err(DesktopError::Transport(String::from(
                    "the Host closed the confirmation channel",
                )))
            }
            Err(_) => Err(DesktopError::Transport(String::from(
                "the confirmation channel stayed silent",
            ))),
        }
    }
}

#[cfg(unix)]
async fn connect_requester(data_dir: &Path) -> Result<tokio::net::UnixStream, DesktopError> {
    let path = data_dir.join("host-control.sock");
    tokio::net::UnixStream::connect(&path)
        .await
        .map_err(|error| DesktopError::Transport(format!("requester listener: {}", error.kind())))
}

#[cfg(windows)]
async fn connect_requester(
    data_dir: &Path,
) -> Result<tokio::net::windows::named_pipe::NamedPipeClient, DesktopError> {
    // Same derivation as the Host's requester listener: the device pipe name
    // plus the control suffix, folded from the data directory.
    let pipe = format!("{}-control", crate::session::client_pipe_name(data_dir));
    tokio::net::windows::named_pipe::ClientOptions::new()
        .open(&pipe)
        .map_err(|error| DesktopError::Transport(format!("requester listener: {}", error.kind())))
}

const MAX_REQUESTER_FRAME_BYTES: u32 = 16 * 1024;

async fn write_requester_frame<W>(stream: &mut W, message: &ToHost) -> Result<(), DesktopError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt as _;

    let body = serde_json::to_vec(message)
        .map_err(|error| DesktopError::Protocol(format!("requester encode: {error}")))?;
    if body.len() > MAX_REQUESTER_FRAME_BYTES as usize {
        return Err(DesktopError::Protocol(String::from(
            "requester frame exceeds the bound",
        )));
    }
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .await
        .map_err(|error| DesktopError::Transport(format!("requester write: {error}")))?;
    stream
        .write_all(&body)
        .await
        .map_err(|error| DesktopError::Transport(format!("requester write: {error}")))?;
    stream
        .flush()
        .await
        .map_err(|error| DesktopError::Transport(format!("requester write: {error}")))
}

async fn read_requester_frame<R>(stream: &mut R) -> Result<FromHost, DesktopError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt as _;

    let mut prefix = [0_u8; 4];
    stream
        .read_exact(&mut prefix)
        .await
        .map_err(|error| DesktopError::Transport(format!("requester read: {error}")))?;
    let length = u32::from_be_bytes(prefix);
    if length == 0 || length > MAX_REQUESTER_FRAME_BYTES {
        return Err(DesktopError::Protocol(String::from(
            "requester frame length is out of bounds",
        )));
    }
    let mut body = vec![0_u8; length as usize];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|error| DesktopError::Transport(format!("requester read: {error}")))?;
    serde_json::from_slice(&body)
        .map_err(|error| DesktopError::Protocol(format!("requester decode: {error}")))
}

#[cfg(test)]
mod tests {
    use ene_local_control::{CONFIRMATION_MODE_ENV, CONFIRMATION_MODE_STDIO};

    /// The launcher/GUI switch is an environment marker the Host sets, never a
    /// command-line flag a user or a requester can aim.
    #[test]
    fn the_confirmation_mode_marker_is_the_hosts() {
        assert_eq!(CONFIRMATION_MODE_ENV, "ENE_CONFIRMATION_CHANNEL");
        assert_eq!(CONFIRMATION_MODE_STDIO, "stdio");
    }
}
