//! Seated Host-local control speaker. Not `ene-api`. Secrets stay redacted.

use std::path::{Path, PathBuf};

use ene_local_control::{ControlOp, FromHost, PendingDeletionPreview, RedactedSecret, ToHost};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use uuid::Uuid;

use crate::ui::DesktopError;

const MAX_CONTROL_FRAME_BYTES: u32 = 16 * 1024;
const CONTROL_SOCKET_NAME: &str = "host-control.sock";

/// Unix control socket inside the `0700` data directory (same name as Host).
#[must_use]
pub fn control_socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CONTROL_SOCKET_NAME)
}

/// Windows control pipe: Client pipe name plus `-control`, matching Host A1.
#[cfg(any(windows, test))]
#[must_use]
pub fn windows_client_pipe(data_dir: &Path) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;
    let mut tag = FNV_OFFSET;
    for byte in data_dir.as_os_str().as_encoded_bytes() {
        tag ^= u64::from(*byte);
        tag = tag.wrapping_mul(FNV_PRIME);
    }
    format!(r"\\.\pipe\ene-{tag:016x}-control")
}

#[cfg(unix)]
struct ControlStream {
    inner: tokio::net::UnixStream,
}

#[cfg(windows)]
struct ControlStream {
    inner: tokio::net::windows::named_pipe::NamedPipeClient,
}

/// One seated speaker. Reconnect invalidates Host sessions (Host-side).
pub struct ControlSeat {
    stream: ControlStream,
    challenge: Option<PendingChallenge>,
}

/// Challenge kept off the GUI snapshot except for the display target.
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

impl ControlSeat {
    /// Dials the control inlet and sends [`ToHost::SeatHello`].
    ///
    /// Occupying an empty seat is accident prevention, not authenticity.
    pub async fn occupy(data_dir: &Path) -> Result<Self, DesktopError> {
        let mut attempts = 0_u8;
        loop {
            match connect(data_dir).await {
                Ok(mut stream) => {
                    write_message(&mut stream.inner, &ToHost::SeatHello).await?;
                    return match read_message(&mut stream.inner).await? {
                        FromHost::SeatGranted => Ok(Self {
                            stream,
                            challenge: None,
                        }),
                        FromHost::SeatOccupied => Err(DesktopError::SeatOccupied),
                        other => Err(DesktopError::Control(format!(
                            "hello answered {}",
                            control_kind(&other)
                        ))),
                    };
                }
                Err(error) => {
                    attempts = attempts.saturating_add(1);
                    if attempts >= 80 {
                        return Err(error);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
    }

    #[must_use]
    pub fn pending_challenge(&self) -> Option<&PendingChallenge> {
        self.challenge.as_ref()
    }

    pub async fn request_device_approve(&mut self, pending_id: &str) -> Result<(), DesktopError> {
        self.exchange_for_challenge(ToHost::DeviceApprove {
            pending_id: pending_id.to_string(),
        })
        .await
    }

    pub async fn request_credential_put(
        &mut self,
        provider: &str,
        label: &str,
        secret: String,
    ) -> Result<(), DesktopError> {
        self.exchange_for_challenge(ToHost::CredentialPut {
            provider: provider.to_string(),
            label: label.to_string(),
            secret: RedactedSecret::new(secret),
        })
        .await
    }

    pub async fn list_pending_deletions(
        &mut self,
    ) -> Result<Vec<PendingDeletionPreview>, DesktopError> {
        write_message(&mut self.stream.inner, &ToHost::PendingDeletions).await?;
        match read_message(&mut self.stream.inner).await? {
            FromHost::PendingDeletions { requests } => Ok(requests),
            FromHost::DeniedByBoundary => Err(DesktopError::DeniedByBoundary),
            other => Err(DesktopError::Control(format!(
                "expected pending deletions, got {}",
                control_kind(&other)
            ))),
        }
    }

    pub async fn request_deletion_confirm(&mut self, request_id: &str) -> Result<(), DesktopError> {
        self.exchange_for_challenge(ToHost::DeletionConfirm {
            request_id: request_id.to_string(),
        })
        .await
    }

    pub async fn request_deletion_resume(
        &mut self,
        operation: &str,
        sweep: u64,
    ) -> Result<FromHost, DesktopError> {
        write_message(
            &mut self.stream.inner,
            &ToHost::DeletionResume {
                operation: operation.to_string(),
                sweep,
            },
        )
        .await?;
        read_message(&mut self.stream.inner).await
    }

    pub async fn complete_pending(&mut self) -> Result<FromHost, DesktopError> {
        let Some(challenge) = self.challenge.take() else {
            return Err(DesktopError::Protocol(String::from(
                "no live confirmation session",
            )));
        };
        write_message(
            &mut self.stream.inner,
            &ToHost::SessionComplete {
                session_id: challenge.session_id,
                nonce: challenge.nonce,
            },
        )
        .await?;
        read_message(&mut self.stream.inner).await
    }

    pub async fn send_confirmed_true(&mut self) -> Result<FromHost, DesktopError> {
        write_message(&mut self.stream.inner, &ToHost::ConfirmedTrue).await?;
        read_message(&mut self.stream.inner).await
    }

    async fn exchange_for_challenge(&mut self, message: ToHost) -> Result<(), DesktopError> {
        write_message(&mut self.stream.inner, &message).await?;
        match read_message(&mut self.stream.inner).await? {
            FromHost::ConfirmationChallenge {
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
                Ok(())
            }
            FromHost::DeniedByBoundary => Err(DesktopError::DeniedByBoundary),
            other => Err(DesktopError::Control(format!(
                "expected challenge, got {}",
                control_kind(&other)
            ))),
        }
    }
}

fn control_kind(from: &FromHost) -> &'static str {
    match from {
        FromHost::SeatGranted => "SeatGranted",
        FromHost::SeatOccupied => "SeatOccupied",
        FromHost::DeniedByBoundary => "DeniedByBoundary",
        FromHost::ConfirmationChallenge { .. } => "ConfirmationChallenge",
        FromHost::Outcome(_) => "Outcome",
        FromHost::Unavailable => "Unavailable",
        FromHost::PendingDeletions { .. } => "PendingDeletions",
    }
}

#[cfg(unix)]
async fn connect(data_dir: &Path) -> Result<ControlStream, DesktopError> {
    let path = control_socket_path(data_dir);
    let inner = tokio::net::UnixStream::connect(&path)
        .await
        .map_err(|error| DesktopError::Transport(error.kind().to_string()))?;
    Ok(ControlStream { inner })
}

#[cfg(windows)]
async fn connect(data_dir: &Path) -> Result<ControlStream, DesktopError> {
    let pipe = windows_client_pipe(data_dir);
    let inner = tokio::net::windows::named_pipe::ClientOptions::new()
        .open(&pipe)
        .map_err(|error| DesktopError::Transport(error.kind().to_string()))?;
    Ok(ControlStream { inner })
}

async fn write_message<W>(stream: &mut W, message: &ToHost) -> Result<(), DesktopError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let body = serde_json::to_vec(message)
        .map_err(|error| DesktopError::Protocol(format!("control encode: {error}")))?;
    if body.len() > MAX_CONTROL_FRAME_BYTES as usize {
        return Err(DesktopError::Protocol(String::from(
            "control frame exceeds the bound",
        )));
    }
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .await
        .map_err(|error| DesktopError::Transport(error.kind().to_string()))?;
    stream
        .write_all(&body)
        .await
        .map_err(|error| DesktopError::Transport(error.kind().to_string()))?;
    stream
        .flush()
        .await
        .map_err(|error| DesktopError::Transport(error.kind().to_string()))
}

async fn read_message<R>(stream: &mut R) -> Result<FromHost, DesktopError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut prefix = [0_u8; 4];
    stream
        .read_exact(&mut prefix)
        .await
        .map_err(|error| DesktopError::Transport(error.kind().to_string()))?;
    let length = u32::from_be_bytes(prefix);
    if length == 0 || length > MAX_CONTROL_FRAME_BYTES {
        return Err(DesktopError::Protocol(String::from(
            "control frame length is out of bounds",
        )));
    }
    let mut body = vec![0_u8; length as usize];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|error| DesktopError::Transport(error.kind().to_string()))?;
    serde_json::from_slice(&body)
        .map_err(|error| DesktopError::Protocol(format!("control decode: {error}")))
}

#[cfg(test)]
mod tests {
    use super::windows_client_pipe;
    use std::path::Path;

    #[test]
    fn windows_control_pipe_is_directory_scoped() {
        let first = windows_client_pipe(Path::new("/tmp/ene-data"));
        let second = windows_client_pipe(Path::new("/tmp/ene-other"));
        assert!(first.starts_with(r"\\.\pipe\ene-"));
        assert!(first.ends_with("-control"));
        assert_ne!(first, second);
    }
}
