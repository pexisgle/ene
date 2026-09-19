//! Host-local first-party control inlet (Stage 7 A1).
//!
//! A separate listener from the Client `ene.sock` / named pipe. Linux: data
//! directory + `SO_PEERCRED` same UID. Windows: narrower DACL named pipe +
//! client PID. The exclusive `FirstPartyControlSeat` admits at most one
//! speaker; a second connection is [`FromHost::SeatOccupied`]. Completion
//! requires the mint-time connection and the same peer PID. Reconnect
//! invalidates outstanding sessions.
//!
//! Nonce is freshness only. Empty-seat first-come occupancy is accident
//! prevention, not official GUI authenticity. Peer PID is not attestation.
//!
//! `ene-core approve-*` / `confirm-deletion` speak this inlet while the Host
//! is serving. Occupied seat is a hard fail — never a Client-channel
//! fallback. Targeted Deletion still refuses offline admission (lifecycle
//! §8.1). Device/credential approval with no serving Host keeps the existing
//! [`crate::host_lock::HostLock`] path.
//!
//! Frames are [`ene_local_control`] JSON, not `ene-api`. Secrets use
//! [`RedactedSecret`]: `Debug` never prints the raw value.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex as StdMutex;

use ene_local_control::{
    ControlOp, ControlOutcome, FromHost, PendingDeletionPreview, RedactedSecret, ToHost,
};
use ene_preservation::{ConfirmTargetedDeletionOutcome, DeletionOperationRef};
use ene_primitive::RawId;
use uuid::Uuid;

use crate::lock_unpoison;
use crate::serve::CoreError;
#[cfg(any(unix, windows))]
use crate::serve::HostHandle;
#[cfg(any(unix, windows))]
use std::sync::Arc;
#[cfg(any(unix, windows))]
use std::time::Duration;

/// Upper bound on one control frame. Credential put carries one secret;
/// anything larger is not this protocol.
#[cfg(any(unix, windows))]
const MAX_CONTROL_FRAME_BYTES: u32 = 16 * 1024;

/// Bound on the first (hello) read from a control peer.
#[cfg(any(unix, windows))]
const CONTROL_HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// Unix control socket file name inside the `0700` data directory.
#[cfg(unix)]
const CONTROL_SOCKET_NAME: &str = "host-control.sock";

/// One pending high-privilege operation waiting for session complete.
enum PendingOp {
    DeviceApprove {
        pending_id: String,
    },
    CredentialPut {
        provider: String,
        label: String,
        secret: RedactedSecret,
    },
    DeletionConfirm {
        request_id: String,
    },
}

struct SeatOccupant {
    connection: u64,
    peer_pid: u32,
}

struct ConfirmationSession {
    nonce: String,
    connection: u64,
    peer_pid: u32,
    pending: PendingOp,
}

struct SeatInner {
    holder: Option<SeatOccupant>,
    next_connection: u64,
    generation: u64,
    sessions: HashMap<Uuid, ConfirmationSession>,
}

/// Exclusive first-party control seat owned by one serving Host.
///
/// Occupancy is process-local. Empty-seat first-come is not authenticity.
pub(crate) struct FirstPartyControlSeat {
    inner: StdMutex<SeatInner>,
}

impl Default for FirstPartyControlSeat {
    fn default() -> Self {
        Self {
            inner: StdMutex::new(SeatInner {
                holder: None,
                next_connection: 0,
                generation: 0,
                sessions: HashMap::new(),
            }),
        }
    }
}

impl FirstPartyControlSeat {
    pub(crate) fn allocate_connection(&self) -> u64 {
        let mut inner = lock_unpoison(&self.inner);
        inner.next_connection = inner.next_connection.saturating_add(1);
        inner.next_connection
    }

    fn try_acquire(&self, connection: u64, peer_pid: u32) -> bool {
        let mut inner = lock_unpoison(&self.inner);
        match inner.holder {
            Some(SeatOccupant {
                connection: held_connection,
                peer_pid: held_pid,
            }) if held_connection == connection && held_pid == peer_pid => true,
            Some(_) => false,
            None => {
                inner.generation = inner.generation.saturating_add(1);
                inner.holder = Some(SeatOccupant {
                    connection,
                    peer_pid,
                });
                true
            }
        }
    }

    fn release(&self, connection: u64) {
        let mut inner = lock_unpoison(&self.inner);
        if inner
            .holder
            .as_ref()
            .is_some_and(|holder| holder.connection == connection)
        {
            inner.holder = None;
            inner.sessions.clear();
        }
    }

    fn mint(
        &self,
        connection: u64,
        peer_pid: u32,
        op: ControlOp,
        target: String,
        pending: PendingOp,
    ) -> FromHost {
        let mut inner = lock_unpoison(&self.inner);
        let seated = inner
            .holder
            .as_ref()
            .is_some_and(|holder| holder.connection == connection && holder.peer_pid == peer_pid);
        if !seated {
            return FromHost::DeniedByBoundary;
        }
        let session_id = Uuid::new_v4();
        let nonce = Uuid::new_v4().as_hyphenated().to_string();
        let premise_generation = inner.generation;
        inner.sessions.insert(
            session_id,
            ConfirmationSession {
                nonce: nonce.clone(),
                connection,
                peer_pid,
                pending,
            },
        );
        FromHost::ConfirmationChallenge {
            session_id,
            op,
            target,
            premise_generation,
            nonce,
        }
    }

    fn take(
        &self,
        connection: u64,
        peer_pid: u32,
        session_id: Uuid,
        nonce: &str,
    ) -> Option<PendingOp> {
        let mut inner = lock_unpoison(&self.inner);
        let matches = inner.sessions.get(&session_id).is_some_and(|session| {
            session.connection == connection
                && session.peer_pid == peer_pid
                && session.nonce == nonce
        });
        if !matches {
            return None;
        }
        inner
            .sessions
            .remove(&session_id)
            .map(|session| session.pending)
    }
}

/// The Host-local control endpoint of one data directory.
#[cfg(unix)]
#[must_use]
pub fn control_socket_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join(CONTROL_SOCKET_NAME)
}

/// The Host-local control pipe of one data directory: the device pipe name
/// plus a control suffix, so both endpoints share the same data-directory
/// derivation and DACL machinery.
#[cfg(windows)]
#[must_use]
pub fn control_pipe_name(data_dir: &Path) -> String {
    format!("{}-control", crate::conn_pipe::pipe_name(data_dir))
}

/// Console / test speaker for the Host-local control inlet.
#[cfg(unix)]
pub struct ControlClient {
    stream: tokio::net::UnixStream,
}

/// Console / test speaker for the Host-local control inlet.
#[cfg(windows)]
pub struct ControlClient {
    stream: tokio::net::windows::named_pipe::NamedPipeClient,
}

#[cfg(any(unix, windows))]
impl ControlClient {
    /// Dials the serving Host's control endpoint.
    ///
    /// # Errors
    ///
    /// [`CoreError::Deletion`] when no serving Host answers (the same
    /// recovery guidance Targeted Deletion already used).
    pub async fn connect(data_dir: &Path) -> Result<Self, CoreError> {
        Ok(Self {
            stream: connect(data_dir).await?,
        })
    }

    /// Writes one frame and reads one answer.
    ///
    /// # Errors
    ///
    /// Transport or decode failures. Domain outcomes arrive as [`FromHost`].
    pub async fn exchange(&mut self, message: &ToHost) -> Result<FromHost, CoreError> {
        write_message(&mut self.stream, message)
            .await
            .map_err(|error| control_failure(&format!("send failed: {error}")))?;
        read_message::<_, FromHost>(&mut self.stream)
            .await
            .map_err(|error| control_failure(&format!("answer failed: {error}")))?
            .ok_or_else(|| control_failure("the serving Host closed without an answer"))
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
}

#[cfg(unix)]
impl ControlListener {
    pub(crate) async fn bind(data_dir: &Path) -> Result<Self, CoreError> {
        use std::os::unix::fs::MetadataExt as _;

        let path = control_socket_path(data_dir);
        let listener = crate::conn::bind_singleton(&path).await?;
        let owner_uid = std::fs::metadata(&path)
            .map_err(|error| CoreError::Bind(format!("read control socket metadata: {error}")))?
            .uid();
        Ok(Self {
            listener,
            owner_uid,
        })
    }

    /// Accepts at most one control connection, checks the peer credential,
    /// and returns it with the OS peer pid. An unprovable peer is dropped
    /// without a byte.
    pub(crate) async fn accept(&self) -> Result<Option<(tokio::net::UnixStream, u32)>, CoreError> {
        let (stream, _) = self
            .listener
            .accept()
            .await
            .map_err(|error| CoreError::Bind(format!("accept control socket: {error}")))?;
        let Ok(credential) = stream.peer_cred() else {
            return Ok(None);
        };
        if credential.uid() != self.owner_uid {
            return Ok(None);
        }
        let Some(pid) = credential.pid().filter(|pid| *pid > 0) else {
            return Ok(None);
        };
        Ok(Some((stream, pid as u32)))
    }
}

/// The Windows control listener: the exclusive first instance of the control
/// pipe (a live peer fails creation, like the device pipe) with the same
/// logon-SID DACL and OS peer-token check.
#[cfg(windows)]
pub(crate) struct ControlListener {
    server: tokio::net::windows::named_pipe::NamedPipeServer,
    pipe: String,
}

#[cfg(windows)]
impl ControlListener {
    pub(crate) fn bind(data_dir: &Path) -> Result<Self, CoreError> {
        let pipe = control_pipe_name(data_dir);
        let server = crate::conn_pipe::create_first_server(&pipe)?;
        Ok(Self { server, pipe })
    }

    pub(crate) async fn accept(
        &mut self,
    ) -> Result<Option<(tokio::net::windows::named_pipe::NamedPipeServer, u32)>, CoreError> {
        use std::os::windows::io::AsRawHandle as _;

        if self.server.connect().await.is_err() {
            // A failed wait leaves this instance unusable; replace it rather
            // than serving half-open state.
            self.server = crate::conn_pipe::create_next_server(&self.pipe)?;
            return Ok(None);
        }
        let next = crate::conn_pipe::create_next_server(&self.pipe)?;
        let current = std::mem::replace(&mut self.server, next);
        let handle = current.as_raw_handle();
        if !crate::conn_pipe::peer_same_user(handle) {
            return Ok(None);
        }
        let Some(pid) = crate::conn_pipe::peer_process_id(handle) else {
            return Ok(None);
        };
        Ok(Some((current, pid)))
    }
}

/// Serves one control connection as a persistent exclusive-seat speaker.
///
/// Shutdown cancels only transport I/O. Once a [`ToHost::SessionComplete`]
/// is admitted, the bound operation runs to completion; the serving loop
/// must join this handler before releasing the Host's single-writer
/// authority.
#[cfg(any(unix, windows))]
pub(crate) async fn serve_connection<S>(
    mut stream: S,
    handle: Arc<HostHandle>,
    peer_pid: u32,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let connection = handle.control_seat.allocate_connection();
    let mut seated = false;
    loop {
        let request = if seated {
            tokio::select! {
                biased;
                () = crate::conn::wait_for_shutdown(&mut shutdown) => break,
                result = read_message::<_, ToHost>(&mut stream) => match result {
                    Ok(Some(request)) => request,
                    Ok(None) | Err(_) => break,
                },
            }
        } else {
            tokio::select! {
                biased;
                () = crate::conn::wait_for_shutdown(&mut shutdown) => break,
                result = tokio::time::timeout(
                    CONTROL_HELLO_TIMEOUT,
                    read_message::<_, ToHost>(&mut stream),
                ) => match result {
                    Ok(Ok(Some(request))) => request,
                    Ok(Ok(None) | Err(_)) | Err(_) => break,
                },
            }
        };
        let reply = if seated {
            dispatch_seated(&handle, connection, peer_pid, request).await
        } else {
            match request {
                ToHost::SeatHello => {
                    if handle.control_seat.try_acquire(connection, peer_pid) {
                        seated = true;
                        FromHost::SeatGranted
                    } else {
                        FromHost::SeatOccupied
                    }
                }
                ToHost::ConfirmedTrue => FromHost::DeniedByBoundary,
                _ => FromHost::DeniedByBoundary,
            }
        };
        let occupied = matches!(reply, FromHost::SeatOccupied);
        if write_message(&mut stream, &reply).await.is_err() {
            break;
        }
        if occupied {
            break;
        }
    }
    if seated {
        handle.control_seat.release(connection);
    }
}

#[cfg(any(unix, windows))]
async fn dispatch_seated(
    handle: &HostHandle,
    connection: u64,
    peer_pid: u32,
    request: ToHost,
) -> FromHost {
    match request {
        ToHost::SeatHello => {
            if handle.control_seat.try_acquire(connection, peer_pid) {
                FromHost::SeatGranted
            } else {
                FromHost::SeatOccupied
            }
        }
        ToHost::ConfirmedTrue => FromHost::DeniedByBoundary,
        ToHost::DeviceApprove { pending_id } => handle.control_seat.mint(
            connection,
            peer_pid,
            ControlOp::DeviceApprove,
            pending_id.clone(),
            PendingOp::DeviceApprove { pending_id },
        ),
        ToHost::CredentialPut {
            provider,
            label,
            secret,
        } => handle.control_seat.mint(
            connection,
            peer_pid,
            ControlOp::CredentialPut,
            format!("{provider}:{label}"),
            PendingOp::CredentialPut {
                provider,
                label,
                secret,
            },
        ),
        ToHost::DeletionConfirm { request_id } => handle.control_seat.mint(
            connection,
            peer_pid,
            ControlOp::DeletionConfirm,
            request_id.clone(),
            PendingOp::DeletionConfirm { request_id },
        ),
        ToHost::PendingDeletions => match handle.pending_targeted_deletions(None, 50).await {
            Ok(list) => FromHost::PendingDeletions {
                requests: list
                    .iter()
                    .map(|request| PendingDeletionPreview {
                        request_id: request
                            .request()
                            .as_raw()
                            .as_uuid()
                            .as_hyphenated()
                            .to_string(),
                        purpose: request.purpose().as_str().to_string(),
                    })
                    .collect(),
            },
            Err(_) => FromHost::Unavailable,
        },
        ToHost::DeletionResume { operation, sweep } => {
            match handle.resume_targeted_deletion(&operation, sweep).await {
                Ok(ene_preservation::DeletionLifecycleOutcome::Applied(current)) => {
                    FromHost::Outcome(ControlOutcome::DeletionResumed {
                        operation: current
                            .operation
                            .as_raw()
                            .as_uuid()
                            .as_hyphenated()
                            .to_string(),
                        sweep: current.sweep.as_u64(),
                    })
                }
                Ok(ene_preservation::DeletionLifecycleOutcome::Missing) => {
                    FromHost::Outcome(ControlOutcome::DeletionMissing)
                }
                Ok(ene_preservation::DeletionLifecycleOutcome::Held(_)) => {
                    FromHost::Outcome(ControlOutcome::DeletionHeldByOperation { operation, sweep })
                }
                Ok(_) => FromHost::DeniedByBoundary,
                Err(_) => FromHost::Unavailable,
            }
        }
        ToHost::SessionComplete { session_id, nonce } => {
            match handle
                .control_seat
                .take(connection, peer_pid, session_id, &nonce)
            {
                Some(pending) => execute_pending(handle, pending).await,
                None => FromHost::DeniedByBoundary,
            }
        }
    }
}

#[cfg(any(unix, windows))]
async fn execute_pending(handle: &HostHandle, pending: PendingOp) -> FromHost {
    match pending {
        PendingOp::DeviceApprove { pending_id } => match handle.approve_device(&pending_id).await {
            Ok(Some((_, secret))) => FromHost::Outcome(ControlOutcome::DeviceApproved {
                pending_id,
                pairing_secret: RedactedSecret::new(secret),
            }),
            Ok(None) => FromHost::Outcome(ControlOutcome::DeviceUnknown { pending_id }),
            Err(_) => FromHost::Unavailable,
        },
        PendingOp::CredentialPut {
            provider,
            label,
            secret,
        } => match handle
            .put_credential(&provider, &label, secret.expose())
            .await
        {
            Ok(true) => {
                // The OS item exists, but registration is complete only once
                // the approval sweep and the usable reference commit together.
                // A commit that did not happen is reported as its own state:
                // neither a stored credential nor an untouched one.
                match handle.approve_credential(&provider, &label).await {
                    Ok(true) => {
                        FromHost::Outcome(ControlOutcome::CredentialStored { provider, label })
                    }
                    Ok(false) | Err(_) => {
                        FromHost::Outcome(ControlOutcome::CredentialUncommitted { provider, label })
                    }
                }
            }
            Ok(false) | Err(_) => {
                FromHost::Outcome(ControlOutcome::CredentialRefused { provider, label })
            }
        },
        PendingOp::DeletionConfirm { request_id } => {
            #[cfg(test)]
            if let Some(gate) = handle.host_control_confirm_gate() {
                gate.pause().await;
            }
            match handle.confirm_targeted_deletion(&request_id).await {
                Ok(outcome) => FromHost::Outcome(control_from_deletion(outcome)),
                Err(_) => FromHost::Unavailable,
            }
        }
    }
}

fn control_from_deletion(outcome: ConfirmTargetedDeletionOutcome) -> ControlOutcome {
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
            ControlOutcome::DeletionStarted { operation, sweep }
        }
        ConfirmTargetedDeletionOutcome::AlreadyCoveredBy(current) => {
            let (operation, sweep) = operation(current);
            ControlOutcome::DeletionAlreadyCoveredBy { operation, sweep }
        }
        ConfirmTargetedDeletionOutcome::HeldByOperation(current) => {
            let (operation, sweep) = operation(current);
            ControlOutcome::DeletionHeldByOperation { operation, sweep }
        }
        ConfirmTargetedDeletionOutcome::NeedsClarification => {
            ControlOutcome::DeletionNeedsClarification
        }
        ConfirmTargetedDeletionOutcome::Missing => ControlOutcome::DeletionMissing,
    }
}

fn deletion_from_control(outcome: ControlOutcome) -> Option<ConfirmTargetedDeletionOutcome> {
    fn parse(operation: &str, sweep: u64) -> Option<DeletionOperationRef> {
        let uuid = uuid::Uuid::parse_str(operation).ok()?;
        Some(DeletionOperationRef {
            operation: ene_preservation::DeletionOperationId::from_raw(RawId::from_uuid(uuid)),
            sweep: ene_preservation::DeletionSweepGeneration::from_u64(sweep),
        })
    }
    Some(match outcome {
        ControlOutcome::DeletionStarted { operation, sweep } => {
            ConfirmTargetedDeletionOutcome::Started(parse(&operation, sweep)?)
        }
        ControlOutcome::DeletionAlreadyCoveredBy { operation, sweep } => {
            ConfirmTargetedDeletionOutcome::AlreadyCoveredBy(parse(&operation, sweep)?)
        }
        ControlOutcome::DeletionHeldByOperation { operation, sweep } => {
            ConfirmTargetedDeletionOutcome::HeldByOperation(parse(&operation, sweep)?)
        }
        ControlOutcome::DeletionNeedsClarification => {
            ConfirmTargetedDeletionOutcome::NeedsClarification
        }
        ControlOutcome::DeletionMissing => ConfirmTargetedDeletionOutcome::Missing,
        _ => return None,
    })
}

/// Runs `request` under a seated session: hello, challenge, complete.
#[cfg(any(unix, windows))]
async fn seated_complete(data_dir: &Path, request: ToHost) -> Result<FromHost, CoreError> {
    let mut client = ControlClient::connect(data_dir).await?;
    match client.exchange(&ToHost::SeatHello).await? {
        FromHost::SeatGranted => {}
        FromHost::SeatOccupied => return Err(CoreError::SeatOccupied),
        FromHost::DeniedByBoundary => {
            return Err(control_failure("hello denied by boundary"));
        }
        other => {
            return Err(control_failure(&format!(
                "unexpected hello answer {}",
                from_host_kind(&other)
            )));
        }
    }
    let challenge = client.exchange(&request).await?;
    let FromHost::ConfirmationChallenge {
        session_id, nonce, ..
    } = challenge
    else {
        return Ok(challenge);
    };
    client
        .exchange(&ToHost::SessionComplete { session_id, nonce })
        .await
}

/// Owner device approval over the serving control inlet.
///
/// Returns the one-time pairing secret for Host-local display, or [`None`]
/// when the pending id is unknown.
///
/// # Errors
///
/// [`CoreError::SeatOccupied`] when another speaker holds the seat;
/// [`CoreError::Approve`] / [`CoreError::Deletion`] when the inlet is
/// unreachable or refuses technically.
pub async fn approve_device(
    data_dir: &Path,
    pending_id: &str,
) -> Result<Option<String>, CoreError> {
    #[cfg(any(unix, windows))]
    {
        match seated_complete(
            data_dir,
            ToHost::DeviceApprove {
                pending_id: pending_id.to_string(),
            },
        )
        .await?
        {
            FromHost::Outcome(ControlOutcome::DeviceApproved { pairing_secret, .. }) => {
                Ok(Some(pairing_secret.expose().to_string()))
            }
            FromHost::Outcome(ControlOutcome::DeviceUnknown { .. }) => Ok(None),
            FromHost::SeatOccupied => Err(CoreError::SeatOccupied),
            FromHost::Unavailable => Err(control_failure(
                "the serving Host could not answer device approval",
            )),
            other => Err(control_failure(&format!(
                "unexpected device-approval answer {}",
                from_host_kind(&other)
            ))),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (data_dir, pending_id);
        Err(CoreError::UnsupportedPlatform(
            "no Host-local control transport",
        ))
    }
}

/// Serving-time credential put over the control inlet.
///
/// # Errors
///
/// [`CoreError::SeatOccupied`] when another speaker holds the seat.
pub async fn put_credential(
    data_dir: &Path,
    provider: &str,
    label: &str,
    secret: &str,
) -> Result<bool, CoreError> {
    #[cfg(any(unix, windows))]
    {
        match seated_complete(
            data_dir,
            ToHost::CredentialPut {
                provider: provider.to_string(),
                label: label.to_string(),
                secret: RedactedSecret::new(secret),
            },
        )
        .await?
        {
            FromHost::Outcome(ControlOutcome::CredentialStored { .. }) => Ok(true),
            FromHost::Outcome(ControlOutcome::CredentialRefused { .. }) => Ok(false),
            FromHost::Outcome(ControlOutcome::CredentialUncommitted { .. }) => {
                Err(CoreError::Approve(String::from(
                    "the value reached the OS store, but the approval sweep and the usable \
                     reference did not commit; inspect the pending pair before retrying — do \
                     not re-send the secret",
                )))
            }
            FromHost::SeatOccupied => Err(CoreError::SeatOccupied),
            FromHost::Unavailable => Err(control_failure(
                "the serving Host could not answer credential put",
            )),
            other => Err(control_failure(&format!(
                "unexpected credential-put answer {}",
                from_host_kind(&other)
            ))),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (data_dir, provider, label, secret);
        Err(CoreError::UnsupportedPlatform(
            "no Host-local control transport",
        ))
    }
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
/// absent), [`CoreError::SeatOccupied`] when another speaker holds the
/// seat, or when the serving Host refuses technically. A domain outcome such
/// as `Missing` or `NeedsClarification` is a successful answer.
pub async fn confirm_targeted_deletion(
    data_dir: &Path,
    request: &str,
) -> Result<ConfirmTargetedDeletionOutcome, CoreError> {
    #[cfg(any(unix, windows))]
    {
        match seated_complete(
            data_dir,
            ToHost::DeletionConfirm {
                request_id: request.to_string(),
            },
        )
        .await?
        {
            FromHost::Outcome(outcome) => deletion_from_control(outcome)
                .ok_or_else(|| control_failure("the serving Host answered a malformed operation")),
            FromHost::SeatOccupied => Err(CoreError::SeatOccupied),
            FromHost::Unavailable => Err(control_failure(
                "the serving Host could not answer the confirmation",
            )),
            other => Err(control_failure(&format!(
                "unexpected confirmation answer {}",
                from_host_kind(&other)
            ))),
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

fn from_host_kind(message: &FromHost) -> &'static str {
    match message {
        FromHost::SeatGranted => "SeatGranted",
        FromHost::SeatOccupied => "SeatOccupied",
        FromHost::DeniedByBoundary => "DeniedByBoundary",
        FromHost::ConfirmationChallenge { .. } => "ConfirmationChallenge",
        FromHost::Outcome(_) => "Outcome",
        FromHost::Unavailable => "Unavailable",
        FromHost::PendingDeletions { .. } => "PendingDeletions",
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

    #[test]
    fn deletion_outcomes_round_trip_through_control() {
        let current = DeletionOperationRef {
            operation: ene_preservation::DeletionOperationId::from_raw(RawId::new()),
            sweep: ene_preservation::DeletionSweepGeneration::from_u64(4),
        };
        for outcome in [
            ConfirmTargetedDeletionOutcome::Started(current),
            ConfirmTargetedDeletionOutcome::AlreadyCoveredBy(current),
            ConfirmTargetedDeletionOutcome::HeldByOperation(current),
            ConfirmTargetedDeletionOutcome::NeedsClarification,
            ConfirmTargetedDeletionOutcome::Missing,
        ] {
            let rendered = control_from_deletion(outcome.clone());
            assert_eq!(
                deletion_from_control(rendered),
                Some(outcome),
                "every canonical outcome must round-trip"
            );
        }
    }

    #[test]
    fn a_malformed_operation_identity_is_refused() {
        let malformed = ControlOutcome::DeletionStarted {
            operation: String::from("not-a-uuid"),
            sweep: 1,
        };
        assert_eq!(deletion_from_control(malformed), None);
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

    #[test]
    fn empty_seat_occupancy_is_not_authenticity() {
        let seat = FirstPartyControlSeat::default();
        let first = seat.allocate_connection();
        assert!(
            seat.try_acquire(first, 7),
            "empty-seat first-come occupies for accident prevention"
        );
        let second = seat.allocate_connection();
        assert!(
            !seat.try_acquire(second, 7),
            "a second connection is refused even with the same pid"
        );
        // Occupancy is exclusive. It is not evidence that the first speaker
        // is the official GUI.
        seat.release(first);
        assert!(
            seat.try_acquire(second, 9),
            "release must admit a later speaker"
        );
    }
}
