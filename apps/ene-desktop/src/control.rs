use std::path::{Path, PathBuf};
use std::time::Duration;

use ene_local_control::{
    ControlOp, FromConfirmation, FromHost, GuiChannel, PendingDeletionPreview, ToConfirmation,
    ToHost,
};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::ui::DesktopError;

const CONFIRMATION_WAIT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct PendingChallenge {
    pub session_id: Uuid,
    pub op: ControlOp,
    pub target: String,
    nonce: String,
}

impl PendingChallenge {
    #[must_use]
    pub fn display_target(&self) -> &str {
        &self.target
    }
}

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

    pub async fn request(&self, message: &ToHost) -> Result<FromHost, DesktopError> {
        let mut stream = connect_requester(&self.data_dir).await?;
        write_requester_frame(&mut stream, message).await?;
        read_requester_frame(&mut stream).await
    }

    pub async fn open_desktop(&self) -> Result<bool, DesktopError> {
        match self.request(&ToHost::OpenDesktop).await? {
            FromHost::DesktopOpened => Ok(true),
            FromHost::DesktopUnavailable => Ok(false),
            other => Err(DesktopError::Control(format!(
                "OpenDesktop answered {other:?}"
            ))),
        }
    }

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

    async fn request_accepted(&self, message: &ToHost) -> Result<String, DesktopError> {
        match self.request(message).await? {
            FromHost::RequestAccepted { request_id } => Ok(request_id),
            FromHost::DeniedByBoundary => Err(DesktopError::DeniedByBoundary),
            other => Err(DesktopError::Control(format!(
                "the request was not accepted: {other:?}"
            ))),
        }
    }

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

pub struct ConfirmationClient {
    requester: RequesterClient,
    channel: GuiChannel,
    incoming: mpsc::Receiver<FromConfirmation>,
    closed: bool,
    challenge: Option<PendingChallenge>,
}

impl ConfirmationClient {
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

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    #[must_use]
    pub fn pending_challenge(&self) -> Option<&PendingChallenge> {
        self.challenge.as_ref()
    }

    pub async fn list_pending_deletions(
        &self,
    ) -> Result<Vec<PendingDeletionPreview>, DesktopError> {
        self.requester.list_pending_deletions().await
    }

    pub async fn request_deletion_resume(
        &mut self,
        operation: &str,
        sweep: u64,
    ) -> Result<FromConfirmation, DesktopError> {
        self.send(&ToConfirmation::DeletionResume {
            operation: operation.to_string(),
            sweep,
        })?;
        self.await_outcome().await
    }

    pub(crate) fn discard_pending(&mut self) {
        self.challenge = None;
    }

    pub async fn request_device_approve(&mut self, pending_id: &str) -> Result<(), DesktopError> {
        self.requester
            .request_accepted(&ToHost::RequestDeviceApprove {
                pending_id: pending_id.to_string(),
            })
            .await?;
        self.await_challenge(ControlOp::DeviceApprove).await
    }

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

    pub async fn request_deletion_confirm(&mut self, request_id: &str) -> Result<(), DesktopError> {
        self.requester
            .request_accepted(&ToHost::RequestDeletionConfirm {
                request_id: request_id.to_string(),
            })
            .await?;
        self.await_challenge(ControlOp::DeletionConfirm).await
    }

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

    pub async fn send_confirmed_true(&mut self) -> Result<FromConfirmation, DesktopError> {
        self.send(&ToConfirmation::ConfirmedTrue)?;
        self.await_outcome().await
    }

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

    async fn await_outcome(&mut self) -> Result<FromConfirmation, DesktopError> {
        loop {
            let frame = self.next_frame().await?;
            match frame {
                FromConfirmation::Outcome(_) => return Ok(frame),
                FromConfirmation::DeniedByBoundary => return Ok(frame),
                FromConfirmation::Unavailable => return Ok(frame),
                FromConfirmation::ConfirmationChallenge {
                    session_id,
                    op,
                    target,
                    nonce,
                    ..
                } => {
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
