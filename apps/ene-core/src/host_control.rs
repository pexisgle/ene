//! Host-local first-party control inlet for Targeted Deletion (Stage 6,
//! lifecycle §8.1/§15; PR §6.4).
//!
//! The Owner's final Targeted Deletion confirmation must run inside the
//! serving Host process. The Host decides which Client incarnations are
//! required participants from what it actually handed to each incarnation;
//! that evidence is durable, but only the serving process can deliver the
//! local-erasure demand and resolve the incarnation's current reachability
//! from its connection table (lifecycle §8.1). An offline `HostHandle::open`
//! has no connection table, so admitting a confirmation there would hold
//! every Client participant without ever reaching one — and PR §6.4 requires
//! a mutation while serving to go over IPC, never through a second offline
//! writer.
//!
//! This module is that IPC boundary: a Host-local control endpoint
//! (`host-control.sock` beside the device socket on Unix; a second named pipe
//! beside the device pipe on Windows) reachable only by the same OS user
//! (protected `0700` data directory plus the same peer-credential check the
//! device transport runs). The `ene-core confirm-deletion` console dials it
//! and the serving Host executes `HostHandle::confirm_targeted_deletion`
//! against its live registry and connection table. No Client payload, LLM
//! output, or Task Agent can reach this inlet, and the wire request carries
//! only the Host-minted request identity — never the target body or search
//! material.
//!
//! The endpoint is deliberately narrow: it forwards the one Host-local
//! mutation that cannot be replayed soundly offline. Read-only preview and
//! status stay direct store reads in the console. A missing endpoint (no
//! serving Host) is an explicit refusal, never a fallback to an offline
//! admission.

use std::path::Path;

use ene_preservation::ConfirmTargetedDeletionOutcome;

use crate::serve::CoreError;
#[cfg(any(unix, windows))]
use std::sync::Arc;
#[cfg(any(unix, windows))]
use std::time::Duration;
#[cfg(any(unix, windows))]
use {crate::serve::HostHandle, ene_preservation::DeletionOperationRef, ene_primitive::RawId};

/// Upper bound on one control frame. A confirmation request is a request
/// UUID plus a small JSON envelope; anything larger is not this protocol.
#[cfg(any(unix, windows))]
const MAX_CONTROL_FRAME_BYTES: u32 = 16 * 1024;

/// Bound on reading one request from a control peer. A console writes its
/// request immediately; a silent or dead peer is dropped instead of pinning
/// a task.
#[cfg(any(unix, windows))]
const CONTROL_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Unix control socket file name inside the `0700` data directory.
#[cfg(unix)]
const CONTROL_SOCKET_NAME: &str = "host-control.sock";

/// The Host-local control endpoint of one data directory.
#[cfg(unix)]
#[must_use]
fn control_socket_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join(CONTROL_SOCKET_NAME)
}

/// The Host-local control pipe of one data directory: the device pipe name
/// plus a control suffix, so both endpoints share the same data-directory
/// derivation and DACL machinery.
#[cfg(windows)]
#[must_use]
fn control_pipe_name(data_dir: &Path) -> String {
    format!("{}-control", crate::conn_pipe::pipe_name(data_dir))
}

/// One request the first-party console may send.
#[cfg(any(unix, windows))]
#[derive(Debug, serde::Serialize, serde::Deserialize)]
enum HostControlRequest {
    /// Record the Owner's confirmation for one staged request and admit it.
    /// The string is the Host-minted request identity, never target text.
    ConfirmTargetedDeletion { request: String },
}

/// One typed answer.
#[cfg(any(unix, windows))]
#[derive(Debug, serde::Serialize, serde::Deserialize)]
enum HostControlResponse {
    Confirmed(HostControlConfirmation),
    /// The serving Host could not answer (technical failure or an
    /// unreadable store). Never a completion claim and never a fallback.
    Unavailable,
}

/// The canonical confirmation outcome, in transport-neutral form.
#[cfg(any(unix, windows))]
#[derive(Debug, serde::Serialize, serde::Deserialize)]
enum HostControlConfirmation {
    Started { operation: String, sweep: u64 },
    AlreadyCoveredBy { operation: String, sweep: u64 },
    HeldByOperation { operation: String, sweep: u64 },
    NeedsClarification,
    Missing,
}

#[cfg(any(unix, windows))]
impl From<ConfirmTargetedDeletionOutcome> for HostControlConfirmation {
    fn from(outcome: ConfirmTargetedDeletionOutcome) -> Self {
        let operation = |current: DeletionOperationRef| {
            (
                current
                    .operation
                    .as_raw()
                    .as_uuid()
                    .as_hyphenated()
                    .to_string(),
                current.sweep.as_u64(),
            )
        };
        match outcome {
            ConfirmTargetedDeletionOutcome::Started(current) => {
                let (operation, sweep) = operation(current);
                Self::Started { operation, sweep }
            }
            ConfirmTargetedDeletionOutcome::AlreadyCoveredBy(current) => {
                let (operation, sweep) = operation(current);
                Self::AlreadyCoveredBy { operation, sweep }
            }
            ConfirmTargetedDeletionOutcome::HeldByOperation(current) => {
                let (operation, sweep) = operation(current);
                Self::HeldByOperation { operation, sweep }
            }
            ConfirmTargetedDeletionOutcome::NeedsClarification => Self::NeedsClarification,
            ConfirmTargetedDeletionOutcome::Missing => Self::Missing,
        }
    }
}

#[cfg(any(unix, windows))]
impl HostControlConfirmation {
    /// Rebuilds the canonical outcome, or [`None`] when a rendered operation
    /// identity is not a canonical UUID (a malformed answer is refused, never
    /// guessed).
    fn into_outcome(self) -> Option<ConfirmTargetedDeletionOutcome> {
        fn parse(operation: &str, sweep: u64) -> Option<DeletionOperationRef> {
            let uuid = uuid::Uuid::parse_str(operation).ok()?;
            Some(DeletionOperationRef {
                operation: ene_preservation::DeletionOperationId::from_raw(RawId::from_uuid(uuid)),
                sweep: ene_preservation::DeletionSweepGeneration::from_u64(sweep),
            })
        }
        Some(match self {
            Self::Started { operation, sweep } => {
                ConfirmTargetedDeletionOutcome::Started(parse(&operation, sweep)?)
            }
            Self::AlreadyCoveredBy { operation, sweep } => {
                ConfirmTargetedDeletionOutcome::AlreadyCoveredBy(parse(&operation, sweep)?)
            }
            Self::HeldByOperation { operation, sweep } => {
                ConfirmTargetedDeletionOutcome::HeldByOperation(parse(&operation, sweep)?)
            }
            Self::NeedsClarification => ConfirmTargetedDeletionOutcome::NeedsClarification,
            Self::Missing => ConfirmTargetedDeletionOutcome::Missing,
        })
    }
}

/// The serving process's control listener, bound before the device listener
/// accepts so no confirmation can race startup.
#[cfg(unix)]
pub(crate) struct ControlListener {
    listener: tokio::net::UnixListener,
    /// Owner recorded when this process created the socket (inside the
    /// `0700` directory), compared against every peer credential.
    owner_uid: u32,
    handle: Arc<HostHandle>,
}

#[cfg(unix)]
impl ControlListener {
    pub(crate) async fn bind(data_dir: &Path, handle: Arc<HostHandle>) -> Result<Self, CoreError> {
        use std::os::unix::fs::MetadataExt as _;

        let path = control_socket_path(data_dir);
        let listener = crate::conn::bind_singleton(&path).await?;
        let owner_uid = std::fs::metadata(&path)
            .map_err(|error| CoreError::Bind(format!("read control socket metadata: {error}")))?
            .uid();
        Ok(Self {
            listener,
            owner_uid,
            handle,
        })
    }

    /// Accepts at most one control connection, checks the peer credential,
    /// and spawns its one-request handler. An unprovable peer is dropped
    /// without a byte.
    pub(crate) async fn accept(&self) {
        let Ok((stream, _)) = self.listener.accept().await else {
            return;
        };
        if !stream
            .peer_cred()
            .is_ok_and(|credential| credential.uid() == self.owner_uid)
        {
            return;
        }
        let handle = Arc::clone(&self.handle);
        tokio::spawn(async move {
            serve_connection(stream, handle).await;
        });
    }
}

/// The Windows control listener: the exclusive first instance of the control
/// pipe (a live peer fails creation, like the device pipe) with the same
/// logon-SID DACL and OS peer-token check.
#[cfg(windows)]
pub(crate) struct ControlListener {
    server: tokio::net::windows::named_pipe::NamedPipeServer,
    pipe: String,
    handle: Arc<HostHandle>,
}

#[cfg(windows)]
impl ControlListener {
    pub(crate) fn bind(data_dir: &Path, handle: Arc<HostHandle>) -> Result<Self, CoreError> {
        let pipe = control_pipe_name(data_dir);
        let server = crate::conn_pipe::create_first_server(&pipe)?;
        Ok(Self {
            server,
            pipe,
            handle,
        })
    }

    pub(crate) async fn accept(&mut self) -> Result<(), CoreError> {
        use std::os::windows::io::AsRawHandle as _;

        if self.server.connect().await.is_err() {
            // A failed wait leaves this instance unusable; replace it rather
            // than serving half-open state.
            self.server = crate::conn_pipe::create_next_server(&self.pipe)?;
            return Ok(());
        }
        let next = crate::conn_pipe::create_next_server(&self.pipe)?;
        let current = std::mem::replace(&mut self.server, next);
        if !crate::conn_pipe::peer_same_user(current.as_raw_handle()) {
            return Ok(());
        }
        let handle = Arc::clone(&self.handle);
        tokio::spawn(async move {
            serve_connection(current, handle).await;
        });
        Ok(())
    }
}

/// Serves one control connection: at most one bounded request, the
/// canonical confirmation executed in this process, and the typed answer.
///
/// A silent peer, a malformed frame, or a write failure changes no durable
/// state beyond what the confirmation itself committed; the console learns
/// the transport failure and can retry the same request identity
/// idempotently.
#[cfg(any(unix, windows))]
async fn serve_connection<S>(mut stream: S, handle: Arc<HostHandle>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let request = match tokio::time::timeout(
        CONTROL_READ_TIMEOUT,
        read_message::<_, HostControlRequest>(&mut stream),
    )
    .await
    {
        Ok(Ok(Some(request))) => request,
        // EOF, malformed frame, or a silent peer: nothing honest to answer.
        Ok(Ok(None) | Err(_)) | Err(_) => return,
    };
    let response = match request {
        HostControlRequest::ConfirmTargetedDeletion { request } => {
            match handle.confirm_targeted_deletion(&request).await {
                Ok(outcome) => HostControlResponse::Confirmed(outcome.into()),
                Err(_) => HostControlResponse::Unavailable,
            }
        }
    };
    // The durable confirmation state already committed; a lost answer only
    // means the console reports the transport failure.
    drop(write_message(&mut stream, &response).await);
}

/// Dials the serving Host's control endpoint and records the Owner
/// confirmation inside it.
///
/// This is the production confirmation path behind `ene-core
/// confirm-deletion`. The confirmation is admitted by the serving process
/// with its durable delivery evidence and live connection table; the typed
/// outcome is the canonical [`ConfirmTargetedDeletionOutcome`], never a
/// completion claim.
///
/// # Errors
///
/// Returns [`CoreError::Deletion`] with recovery guidance when no serving
/// Host answers the control endpoint (the offline fallback is deliberately
/// absent: only the serving process can reach the Client incarnations that
/// may hold a target-bearing copy), when the exchange cannot complete, or
/// when the serving Host refuses the request technically (the typed
/// `Unavailable` answer). A domain outcome such as
/// `Missing` or `NeedsClarification` is a successful answer and is returned
/// as itself.
pub async fn confirm_targeted_deletion(
    data_dir: &Path,
    request: &str,
) -> Result<ConfirmTargetedDeletionOutcome, CoreError> {
    #[cfg(any(unix, windows))]
    {
        let mut stream = connect(data_dir).await?;
        write_message(
            &mut stream,
            &HostControlRequest::ConfirmTargetedDeletion {
                request: request.to_string(),
            },
        )
        .await
        .map_err(|error| control_failure(&format!("send failed: {error}")))?;
        let response = read_message::<_, HostControlResponse>(&mut stream)
            .await
            .map_err(|error| control_failure(&format!("answer failed: {error}")))?
            .ok_or_else(|| control_failure("the serving Host closed without an answer"))?;
        match response {
            HostControlResponse::Confirmed(confirmation) => confirmation
                .into_outcome()
                .ok_or_else(|| control_failure("the serving Host answered a malformed operation")),
            HostControlResponse::Unavailable => Err(control_failure(
                "the serving Host could not answer the confirmation",
            )),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (data_dir, request);
        Err(CoreError::UnsupportedPlatform(
            "no Host-local control transport",
        ))
    }
}

/// The explicit refusal when the serving Host cannot be reached: recovery
/// guidance, never a silent offline fallback.
#[cfg(any(unix, windows))]
fn control_failure(detail: &str) -> CoreError {
    CoreError::Deletion(format!(
        "the serving Host is not reachable on the Host-local control inlet \
         ({detail}); start `ene-core serve` and retry — only the serving process \
         can reach every Client that may hold a target-bearing copy"
    ))
}

#[cfg(unix)]
async fn connect(data_dir: &Path) -> Result<tokio::net::UnixStream, CoreError> {
    let path = control_socket_path(data_dir);
    tokio::net::UnixStream::connect(&path)
        .await
        .map_err(|error| {
            control_failure(&format!(
                "connect to {} failed: {}",
                path.display(),
                error.kind()
            ))
        })
}

#[cfg(windows)]
async fn connect(
    data_dir: &Path,
) -> Result<tokio::net::windows::named_pipe::NamedPipeClient, CoreError> {
    let pipe = control_pipe_name(data_dir);
    tokio::net::windows::named_pipe::ClientOptions::new()
        .open(&pipe)
        .map_err(|error| control_failure(&format!("connect to {pipe} failed: {}", error.kind())))
}

/// Writes one length-prefixed JSON message.
#[cfg(any(unix, windows))]
async fn write_message<W, T>(stream: &mut W, message: &T) -> std::io::Result<()>
where
    W: tokio::io::AsyncWrite + Unpin,
    T: serde::Serialize,
{
    use tokio::io::AsyncWriteExt as _;

    let body = serde_json::to_vec(message)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if body.len() > MAX_CONTROL_FRAME_BYTES as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "control frame exceeds the bound",
        ));
    }
    stream.write_all(&(body.len() as u32).to_be_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await
}

/// Reads one length-prefixed JSON message; [`None`] when the peer closed or
/// the frame is malformed or oversize.
#[cfg(any(unix, windows))]
async fn read_message<R, T>(stream: &mut R) -> std::io::Result<Option<T>>
where
    R: tokio::io::AsyncRead + Unpin,
    T: serde::de::DeserializeOwned,
{
    use tokio::io::AsyncReadExt as _;

    let mut prefix = [0_u8; 4];
    match stream.read_exact(&mut prefix).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = u32::from_be_bytes(prefix);
    if length == 0 || length > MAX_CONTROL_FRAME_BYTES {
        return Ok(None);
    }
    let mut body = vec![0_u8; length as usize];
    stream.read_exact(&mut body).await?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_preservation::{DeletionOperationId, DeletionSweepGeneration};

    #[test]
    fn confirmation_round_trips_every_canonical_outcome() {
        let current = DeletionOperationRef {
            operation: DeletionOperationId::from_raw(RawId::new()),
            sweep: DeletionSweepGeneration::from_u64(4),
        };
        for outcome in [
            ConfirmTargetedDeletionOutcome::Started(current),
            ConfirmTargetedDeletionOutcome::AlreadyCoveredBy(current),
            ConfirmTargetedDeletionOutcome::HeldByOperation(current),
            ConfirmTargetedDeletionOutcome::NeedsClarification,
            ConfirmTargetedDeletionOutcome::Missing,
        ] {
            let rendered = HostControlConfirmation::from(outcome.clone());
            let json = serde_json::to_string(&rendered).expect("the answer must serialize");
            let parsed: HostControlConfirmation =
                serde_json::from_str(&json).expect("the answer must parse");
            assert_eq!(
                parsed.into_outcome(),
                Some(outcome),
                "every canonical outcome must round-trip"
            );
        }
    }

    #[test]
    fn a_malformed_operation_identity_is_refused() {
        let malformed = HostControlConfirmation::Started {
            operation: String::from("not-a-uuid"),
            sweep: 1,
        };
        assert_eq!(malformed.into_outcome(), None);
    }

    /// The offline fallback is deliberately absent: with no serving Host the
    /// console gets an explicit refusal with recovery guidance, never a
    /// confirmation admitted from an empty delivery history (lifecycle §8.1).
    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn a_stopped_host_refuses_instead_of_admitting_offline() {
        let dir = tempfile::tempdir().expect("the scratch directory must create");
        let error = confirm_targeted_deletion(dir.path(), "00000000-0000-0000-0000-000000000000")
            .await
            .expect_err("no serving Host must be an explicit refusal");
        assert!(
            error.to_string().contains("serving Host"),
            "the refusal must carry recovery guidance: {error}"
        );
    }
}
