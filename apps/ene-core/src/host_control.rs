use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;

use ene_local_control::{
    ControlOp, ControlOutcome, DeletionOutcome, FromConfirmation, FromHost, PendingDeletionPreview,
    RedactedSecret, RequestState, RequesterOutcome, ToConfirmation, ToHost,
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
const CONTROL_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

const CONTROL_SESSION_TTL: std::time::Duration = std::time::Duration::from_secs(120);

const MAX_PENDING_REQUESTS: usize = 64;

const MAX_SETTLED_REQUESTS: usize = 256;

#[cfg(any(unix, windows))]
const LOADER_ENV_VARS: &[&str] = &[
    "PATH",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "LD_DEBUG",
    "LD_PROFILE",
    "LD_USE_LOAD_BIAS",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "DYLD_FRAMEWORK_PATH",
    "DYLD_FALLBACK_LIBRARY_PATH",
    "DYLD_FALLBACK_FRAMEWORK_PATH",
];

#[cfg(unix)]
const CONTROL_SOCKET_NAME: &str = "host-control.sock";

struct AcceptedRequest {
    state: RequestState,
    seq: u64,
}

enum PendingOp {
    DeviceApprove {
        pending_id: String,
    },
    CredentialPut {
        provider: String,
        label: String,
        mutation_id: String,
        secret: Option<RedactedSecret>,
    },
    DeletionConfirm {
        request_id: String,
    },
}

struct ConfirmationSession {
    nonce: String,
    seat_generation: u64,
    deadline: std::time::Instant,
    request_id: String,
    pending: PendingOp,
}

pub(crate) struct SeatedGui {
    pub(crate) generation: u64,
}

#[derive(Default)]
struct SeatInner {
    holder: Option<SeatedGui>,
    next_generation: u64,
    sessions: HashMap<Uuid, ConfirmationSession>,
    requests: HashMap<String, AcceptedRequest>,
    next_request_seq: u64,
    outbound: Option<std::sync::mpsc::Sender<ene_local_control::channel::ChannelEvent>>,
}

#[derive(Default)]
pub(crate) struct FirstPartyControlSeat {
    inner: StdMutex<SeatInner>,
}

impl FirstPartyControlSeat {
    fn invalidate_sessions(inner: &mut SeatInner, state: RequestState) {
        let request_ids = inner
            .sessions
            .drain()
            .map(|(_, session)| session.request_id)
            .collect::<Vec<_>>();
        for request_id in request_ids {
            if let Some(request) = inner.requests.get_mut(&request_id) {
                request.state = state.clone();
            }
        }
    }

    pub(crate) fn seat_spawned_gui(
        &self,
        outbound: std::sync::mpsc::Sender<ene_local_control::channel::ChannelEvent>,
    ) -> u64 {
        let mut inner = lock_unpoison(&self.inner);
        inner.next_generation = inner.next_generation.saturating_add(1);
        let generation = inner.next_generation;
        inner.holder = Some(SeatedGui { generation });
        Self::invalidate_sessions(&mut inner, RequestState::ConfirmationUnavailable);
        inner.outbound = Some(outbound);
        generation
    }

    pub(crate) fn seat_closed(&self, generation: u64) {
        let mut inner = lock_unpoison(&self.inner);
        if inner
            .holder
            .as_ref()
            .is_some_and(|holder| holder.generation == generation)
        {
            inner.holder = None;
            Self::invalidate_sessions(&mut inner, RequestState::ConfirmationUnavailable);
            inner.outbound = None;
        }
    }

    pub(crate) fn holder_generation(&self) -> Option<u64> {
        lock_unpoison(&self.inner)
            .holder
            .as_ref()
            .map(|holder| holder.generation)
    }

    pub(crate) fn has_seat(&self) -> bool {
        lock_unpoison(&self.inner).holder.is_some()
    }

    fn settle_expired_sessions(inner: &mut SeatInner) {
        let now = std::time::Instant::now();
        let expired = inner
            .sessions
            .iter()
            .filter(|(_, session)| session.deadline <= now)
            .map(|(session_id, _)| *session_id)
            .collect::<Vec<_>>();
        for session_id in expired {
            Self::settle_expired_session(inner, session_id);
        }
    }

    fn retire_settled_requests(inner: &mut SeatInner) {
        loop {
            let settled = inner
                .requests
                .values()
                .filter(|request| !matches!(request.state, RequestState::AwaitingOwnerConfirmation))
                .count();
            if settled < MAX_SETTLED_REQUESTS {
                break;
            }
            let Some(oldest) = inner
                .requests
                .iter()
                .filter(|(_, request)| {
                    !matches!(request.state, RequestState::AwaitingOwnerConfirmation)
                })
                .min_by_key(|(_, request)| request.seq)
                .map(|(request_id, _)| request_id.clone())
            else {
                break;
            };
            inner.requests.remove(&oldest);
        }
    }

    fn accept_request(&self) -> Option<(String, RequestState)> {
        let mut inner = lock_unpoison(&self.inner);
        Self::settle_expired_sessions(&mut inner);
        let pending = inner
            .requests
            .values()
            .filter(|request| matches!(request.state, RequestState::AwaitingOwnerConfirmation))
            .count();
        if pending >= MAX_PENDING_REQUESTS {
            return None;
        }
        Self::retire_settled_requests(&mut inner);
        let request_id = Uuid::new_v4().as_hyphenated().to_string();
        let state = if inner.holder.is_some() {
            RequestState::AwaitingOwnerConfirmation
        } else {
            RequestState::ConfirmationUnavailable
        };
        let seq = inner.next_request_seq;
        inner.next_request_seq = inner.next_request_seq.saturating_add(1);
        inner.requests.insert(
            request_id.clone(),
            AcceptedRequest {
                state: state.clone(),
                seq,
            },
        );
        Some((request_id, state))
    }

    fn request_state(&self, request_id: &str) -> Option<RequestState> {
        let mut inner = lock_unpoison(&self.inner);
        Self::settle_expired_sessions(&mut inner);
        inner
            .requests
            .get(request_id)
            .map(|request| request.state.clone())
    }

    fn mint(
        &self,
        request_id: &str,
        op: ControlOp,
        target: String,
        pending: PendingOp,
    ) -> FromConfirmation {
        let mut inner = lock_unpoison(&self.inner);
        let Some(seat_generation) = inner.holder.as_ref().map(|holder| holder.generation) else {
            if let Some(request) = inner.requests.get_mut(request_id) {
                request.state = RequestState::ConfirmationUnavailable;
            }
            return FromConfirmation::DeniedByBoundary;
        };
        let session_id = Uuid::new_v4();
        let nonce = Uuid::new_v4().as_hyphenated().to_string();
        let premise_generation = seat_generation;
        inner.sessions.insert(
            session_id,
            ConfirmationSession {
                nonce: nonce.clone(),
                seat_generation,
                deadline: std::time::Instant::now() + CONTROL_SESSION_TTL,
                request_id: request_id.to_string(),
                pending,
            },
        );
        let challenge = FromConfirmation::ConfirmationChallenge {
            session_id,
            op,
            target,
            premise_generation,
            nonce: RedactedSecret::new(nonce),
        };
        let delivered = inner.outbound.as_ref().is_some_and(|outbound| {
            outbound
                .send(ene_local_control::channel::ChannelEvent::Outbound(
                    challenge.clone(),
                ))
                .is_ok()
        });
        if !delivered {
            inner.sessions.remove(&session_id);
            if let Some(request) = inner.requests.get_mut(request_id) {
                request.state = RequestState::ConfirmationUnavailable;
            }
            return FromConfirmation::Unavailable;
        }
        challenge
    }

    fn settle_expired_session(inner: &mut SeatInner, session_id: Uuid) -> bool {
        let expired = inner
            .sessions
            .get(&session_id)
            .filter(|session| session.deadline <= std::time::Instant::now())
            .map(|session| session.request_id.clone());
        let Some(request_id) = expired else {
            return false;
        };
        inner.sessions.remove(&session_id);
        if let Some(request) = inner.requests.get_mut(&request_id) {
            request.state = RequestState::Rejected;
        }
        true
    }

    fn take(&self, session_id: Uuid, nonce: &str) -> Option<(String, PendingOp)> {
        let mut inner = lock_unpoison(&self.inner);
        if Self::settle_expired_session(&mut inner, session_id) {
            return None;
        }
        let live_generation = inner.holder.as_ref().map(|holder| holder.generation);
        let matches = inner.sessions.get(&session_id).is_some_and(|session| {
            session.nonce == nonce && Some(session.seat_generation) == live_generation
        });
        if !matches {
            return None;
        }
        inner
            .sessions
            .remove(&session_id)
            .map(|session| (session.request_id, session.pending))
    }

    fn reject(&self, session_id: Uuid, nonce: &str) -> Option<String> {
        let mut inner = lock_unpoison(&self.inner);
        if Self::settle_expired_session(&mut inner, session_id) {
            return None;
        }
        let matches = inner
            .sessions
            .get(&session_id)
            .is_some_and(|session| session.nonce == nonce);
        if !matches {
            return None;
        }
        inner
            .sessions
            .remove(&session_id)
            .map(|session| session.request_id)
    }

    fn stage_credential_secret(
        &self,
        session_id: Uuid,
        nonce: &str,
        target: &str,
        secret: RedactedSecret,
    ) -> bool {
        let mut inner = lock_unpoison(&self.inner);
        if Self::settle_expired_session(&mut inner, session_id) {
            return false;
        }
        let live_generation = inner.holder.as_ref().map(|holder| holder.generation);
        let Some(session) = inner.sessions.get_mut(&session_id) else {
            return false;
        };
        if session.nonce != nonce || Some(session.seat_generation) != live_generation {
            return false;
        }
        match &mut session.pending {
            PendingOp::CredentialPut {
                provider,
                label,
                mutation_id: _,
                secret: staged,
            } if format!("{provider}:{label}") == target => {
                *staged = Some(secret);
                true
            }
            _ => false,
        }
    }

    fn settle_request(&self, request_id: &str, outcome: RequesterOutcome) {
        let mut inner = lock_unpoison(&self.inner);
        if let Some(request) = inner.requests.get_mut(request_id) {
            request.state = RequestState::Applied { outcome };
        }
    }

    fn reject_request(&self, request_id: &str) {
        let mut inner = lock_unpoison(&self.inner);
        if let Some(request) = inner.requests.get_mut(request_id) {
            request.state = RequestState::Rejected;
        }
    }

    fn mark_outcome_unavailable(&self, request_id: &str) {
        let mut inner = lock_unpoison(&self.inner);
        if let Some(request) = inner.requests.get_mut(request_id) {
            request.state = RequestState::OutcomeUnavailable;
        }
    }
}

#[cfg(unix)]
#[must_use]
pub fn control_socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CONTROL_SOCKET_NAME)
}

#[cfg(windows)]
#[must_use]
pub fn control_pipe_name(data_dir: &Path) -> String {
    format!("{}-control", ene_plugin_ipc::pipe_name(data_dir))
}

#[cfg(unix)]
pub struct ControlClient {
    stream: tokio::net::UnixStream,
}

#[cfg(windows)]
pub struct ControlClient {
    stream: tokio::net::windows::named_pipe::NamedPipeClient,
}

#[cfg(any(unix, windows))]
impl ControlClient {
    pub async fn connect(data_dir: &Path) -> Result<Self, CoreError> {
        Ok(Self {
            stream: connect(data_dir).await?,
        })
    }

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

#[cfg(unix)]
pub(crate) struct ControlListener {
    listener: tokio::net::UnixListener,
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

    pub(crate) async fn accept(&self) -> Result<Option<tokio::net::UnixStream>, CoreError> {
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
        Ok(Some(stream))
    }
}

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
    ) -> Result<Option<tokio::net::windows::named_pipe::NamedPipeServer>, CoreError> {
        use std::os::windows::io::AsRawHandle as _;

        if self.server.connect().await.is_err() {
            self.server = crate::conn_pipe::create_next_server(&self.pipe)?;
            return Ok(None);
        }
        let next = crate::conn_pipe::create_next_server(&self.pipe)?;
        let current = std::mem::replace(&mut self.server, next);
        if !crate::conn_pipe::peer_same_user(current.as_raw_handle()) {
            return Ok(None);
        }
        Ok(Some(current))
    }
}

#[cfg(any(unix, windows))]
pub(crate) async fn serve_requester<S>(
    mut stream: S,
    handle: Arc<HostHandle>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let request = tokio::select! {
            biased;
            () = crate::conn::wait_for_shutdown(&mut shutdown) => break,
            result = tokio::time::timeout(
                CONTROL_READ_TIMEOUT,
                read_message::<_, ToHost>(&mut stream),
            ) => match result {
                Ok(Ok(Some(request))) => request,
                Ok(Ok(None) | Err(_)) | Err(_) => break,
            },
        };
        if *shutdown.borrow() {
            break;
        }
        let reply = dispatch_requester(&handle, request).await;
        if write_message(&mut stream, &reply).await.is_err() {
            break;
        }
    }
}

#[cfg(any(unix, windows))]
async fn admit_requester(
    handle: &Arc<HostHandle>,
    op: ControlOp,
    target: String,
    pending: PendingOp,
) -> FromHost {
    drop(handle.open_desktop().await);
    let Some((request_id, state)) = handle.control_seat.accept_request() else {
        return FromHost::BackpressureHold;
    };
    if matches!(state, RequestState::AwaitingOwnerConfirmation) {
        handle.control_seat.mint(&request_id, op, target, pending);
    }
    FromHost::RequestAccepted { request_id }
}

#[cfg(any(unix, windows))]
async fn dispatch_requester(handle: &Arc<HostHandle>, request: ToHost) -> FromHost {
    match request {
        ToHost::OpenDesktop => match handle.open_desktop().await {
            Ok(true) => FromHost::DesktopOpened,
            Ok(false) => FromHost::DesktopUnavailable,
            Err(_) => FromHost::Unavailable,
        },
        ToHost::RequestDeviceApprove { pending_id } => {
            admit_requester(
                handle,
                ControlOp::DeviceApprove,
                pending_id.clone(),
                PendingOp::DeviceApprove { pending_id },
            )
            .await
        }
        ToHost::RequestCredentialPut { provider, label } => {
            let target = format!("{provider}:{label}");
            admit_requester(
                handle,
                ControlOp::CredentialPut,
                target,
                PendingOp::CredentialPut {
                    provider,
                    label,
                    mutation_id: Uuid::new_v4().as_hyphenated().to_string(),
                    secret: None,
                },
            )
            .await
        }
        ToHost::RequestDeletionConfirm { request_id } => {
            admit_requester(
                handle,
                ControlOp::DeletionConfirm,
                request_id.clone(),
                PendingOp::DeletionConfirm { request_id },
            )
            .await
        }
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
        ToHost::RequestStatus { request_id } => {
            match handle.control_seat.request_state(&request_id) {
                Some(state) => FromHost::RequestStatus { request_id, state },
                None => FromHost::DeniedByBoundary,
            }
        }
        ToHost::ConfirmedTrue => FromHost::DeniedByBoundary,
    }
}

#[cfg(any(unix, windows))]
async fn dispatch_confirmation(handle: &HostHandle, request: ToConfirmation) -> FromConfirmation {
    match request {
        ToConfirmation::SessionComplete { session_id, nonce } => {
            match handle.control_seat.take(session_id, nonce.expose()) {
                Some((request_id, pending)) => {
                    let (reply, outcome) = execute_pending(handle, pending).await;
                    if let Some(outcome) = outcome {
                        handle.control_seat.settle_request(&request_id, outcome);
                    } else {
                        handle.control_seat.mark_outcome_unavailable(&request_id);
                    }
                    reply
                }
                None => FromConfirmation::DeniedByBoundary,
            }
        }
        ToConfirmation::SessionReject { session_id, nonce } => {
            match handle.control_seat.reject(session_id, nonce.expose()) {
                Some(request_id) => {
                    handle.control_seat.reject_request(&request_id);
                    FromConfirmation::Outcome(ControlOutcome::Rejected { session_id })
                }
                None => FromConfirmation::DeniedByBoundary,
            }
        }
        ToConfirmation::CredentialSecret {
            session_id,
            nonce,
            provider,
            label,
            secret,
        } => {
            if !handle.control_seat.stage_credential_secret(
                session_id,
                nonce.expose(),
                &format!("{provider}:{label}"),
                secret,
            ) {
                return FromConfirmation::DeniedByBoundary;
            }
            FromConfirmation::Outcome(ControlOutcome::CredentialStaged { provider, label })
        }
        ToConfirmation::DeletionResume { operation, sweep } => {
            match handle.resume_targeted_deletion(&operation, sweep).await {
                Ok(ene_preservation::DeletionLifecycleOutcome::Applied(current)) => {
                    FromConfirmation::Outcome(ControlOutcome::Deletion(DeletionOutcome::Resumed {
                        operation: current
                            .operation
                            .as_raw()
                            .as_uuid()
                            .as_hyphenated()
                            .to_string(),
                        sweep: current.sweep.as_u64(),
                    }))
                }
                Ok(ene_preservation::DeletionLifecycleOutcome::Held(_)) => {
                    FromConfirmation::Outcome(ControlOutcome::Deletion(
                        DeletionOutcome::HeldByOperation { operation, sweep },
                    ))
                }
                Ok(ene_preservation::DeletionLifecycleOutcome::StaleSweep) => {
                    FromConfirmation::Outcome(ControlOutcome::Deletion(
                        DeletionOutcome::StaleSweep { operation, sweep },
                    ))
                }
                Ok(ene_preservation::DeletionLifecycleOutcome::Completed) => {
                    FromConfirmation::Outcome(ControlOutcome::Deletion(
                        DeletionOutcome::Completed { operation, sweep },
                    ))
                }
                Ok(ene_preservation::DeletionLifecycleOutcome::Finalizing) => {
                    FromConfirmation::Outcome(ControlOutcome::Deletion(
                        DeletionOutcome::Finalizing { operation, sweep },
                    ))
                }
                Ok(ene_preservation::DeletionLifecycleOutcome::Missing) => {
                    FromConfirmation::Outcome(ControlOutcome::Deletion(DeletionOutcome::Missing))
                }
                Err(_) => FromConfirmation::Unavailable,
            }
        }
        ToConfirmation::ConfirmedTrue => FromConfirmation::DeniedByBoundary,
    }
}

#[cfg(any(unix, windows))]
async fn execute_pending(
    handle: &HostHandle,
    pending: PendingOp,
) -> (FromConfirmation, Option<RequesterOutcome>) {
    match pending {
        PendingOp::DeviceApprove { pending_id } => match handle.approve_device(&pending_id).await {
            Ok(Some(record)) => (
                FromConfirmation::Outcome(ControlOutcome::DeviceApproved {
                    pending_id: pending_id.clone(),
                    device_id: record.wire.clone(),
                }),
                Some(RequesterOutcome::DeviceApproved {
                    pending_id,
                    device_id: record.wire,
                }),
            ),
            Ok(None) => (
                FromConfirmation::Outcome(ControlOutcome::DeviceUnknown {
                    pending_id: pending_id.clone(),
                }),
                Some(RequesterOutcome::DeviceUnknown { pending_id }),
            ),
            Err(_) => (FromConfirmation::Unavailable, None),
        },
        PendingOp::CredentialPut {
            provider,
            label,
            mutation_id,
            secret,
        } => {
            let Some(secret) = secret else {
                return (
                    FromConfirmation::Outcome(ControlOutcome::CredentialRefused {
                        provider: provider.clone(),
                        label: label.clone(),
                    }),
                    Some(RequesterOutcome::CredentialRefused { provider, label }),
                );
            };
            let outcome = handle
                .publish_credential(&provider, &label, &mutation_id, secret.expose())
                .await
                .unwrap_or(ene_credential::MutationOutcome::Unknown);
            credential_outcome(outcome, provider, label)
        }
        PendingOp::DeletionConfirm { request_id } => {
            #[cfg(test)]
            if let Some(gate) = handle.host_control_confirm_gate() {
                gate.pause().await;
            }
            match handle.confirm_targeted_deletion(&request_id).await {
                Ok(outcome) => {
                    let deletion = deletion_outcome(&outcome);
                    (
                        FromConfirmation::Outcome(ControlOutcome::Deletion(deletion.clone())),
                        Some(RequesterOutcome::Deletion(deletion)),
                    )
                }
                Err(_) => (FromConfirmation::Unavailable, None),
            }
        }
    }
}

#[cfg(any(unix, windows))]
fn credential_outcome(
    outcome: ene_credential::MutationOutcome,
    provider: String,
    label: String,
) -> (FromConfirmation, Option<RequesterOutcome>) {
    use ene_credential::MutationOutcome;
    match outcome {
        MutationOutcome::Activated { .. } => (
            FromConfirmation::Outcome(ControlOutcome::CredentialStored {
                provider: provider.clone(),
                label: label.clone(),
            }),
            Some(RequesterOutcome::CredentialStored { provider, label }),
        ),
        MutationOutcome::Stale => (
            FromConfirmation::Outcome(ControlOutcome::CredentialUncommitted {
                provider: provider.clone(),
                label: label.clone(),
            }),
            Some(RequesterOutcome::CredentialUncommitted { provider, label }),
        ),
        MutationOutcome::Revoked { .. } | MutationOutcome::Rejected | MutationOutcome::Refused => (
            FromConfirmation::Outcome(ControlOutcome::CredentialRefused {
                provider: provider.clone(),
                label: label.clone(),
            }),
            Some(RequesterOutcome::CredentialRefused { provider, label }),
        ),
        MutationOutcome::Unknown => (FromConfirmation::Unavailable, None),
    }
}

fn deletion_outcome(outcome: &ConfirmTargetedDeletionOutcome) -> DeletionOutcome {
    let operation = |current: &DeletionOperationRef| {
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
            DeletionOutcome::Started { operation, sweep }
        }
        ConfirmTargetedDeletionOutcome::AlreadyCoveredBy(current) => {
            let (operation, sweep) = operation(current);
            DeletionOutcome::AlreadyCoveredBy { operation, sweep }
        }
        ConfirmTargetedDeletionOutcome::HeldByOperation(current) => {
            let (operation, sweep) = operation(current);
            DeletionOutcome::HeldByOperation { operation, sweep }
        }
        ConfirmTargetedDeletionOutcome::NeedsClarification => DeletionOutcome::NeedsClarification,
        ConfirmTargetedDeletionOutcome::Missing => DeletionOutcome::Missing,
    }
}

fn deletion_from_control(outcome: &DeletionOutcome) -> Option<ConfirmTargetedDeletionOutcome> {
    fn parse(operation: &str, sweep: u64) -> Option<DeletionOperationRef> {
        let uuid = uuid::Uuid::parse_str(operation).ok()?;
        Some(DeletionOperationRef {
            operation: ene_preservation::DeletionOperationId::from_raw(RawId::from_uuid(uuid)),
            sweep: ene_preservation::DeletionSweepGeneration::from_u64(sweep),
        })
    }
    Some(match outcome {
        DeletionOutcome::Started { operation, sweep } => {
            ConfirmTargetedDeletionOutcome::Started(parse(operation, *sweep)?)
        }
        DeletionOutcome::AlreadyCoveredBy { operation, sweep } => {
            ConfirmTargetedDeletionOutcome::AlreadyCoveredBy(parse(operation, *sweep)?)
        }
        DeletionOutcome::HeldByOperation { operation, sweep } => {
            ConfirmTargetedDeletionOutcome::HeldByOperation(parse(operation, *sweep)?)
        }
        DeletionOutcome::NeedsClarification => ConfirmTargetedDeletionOutcome::NeedsClarification,
        DeletionOutcome::Missing => ConfirmTargetedDeletionOutcome::Missing,
        DeletionOutcome::Resumed { .. }
        | DeletionOutcome::StaleSweep { .. }
        | DeletionOutcome::Completed { .. }
        | DeletionOutcome::Finalizing { .. } => return None,
    })
}

pub(crate) struct GuiProcess {
    pub(crate) child: std::process::Child,
    pub(crate) child_id: u32,
}

impl std::fmt::Debug for GuiProcess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GuiProcess")
            .field("child_id", &self.child_id)
            .finish_non_exhaustive()
    }
}

#[cfg(any(unix, windows))]
fn reap_gui_child(process: GuiProcess) {
    std::thread::spawn(move || {
        let mut process = process;
        drop(process.child.kill());
        drop(process.child.wait());
    });
}

#[cfg(any(unix, windows))]
fn locate_gui_binary() -> Option<(PathBuf, PathBuf)> {
    let exe = std::env::current_exe().ok()?;
    let install_dir = exe.parent()?.to_path_buf();
    let binary = install_dir.join(if cfg!(windows) {
        "ene-desktop.exe"
    } else {
        "ene-desktop"
    });
    binary.is_file().then_some((binary, install_dir))
}

impl HostHandle {
    pub(crate) async fn join_confirmation_tasks(&self) {
        loop {
            let next = {
                let mut tasks = self
                    .confirmation_tasks
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                self.confirmation_stopping
                    .store(true, std::sync::atomic::Ordering::Release);
                tasks.pop()
            };
            let Some(task) = next else {
                return;
            };
            match task.await {
                Ok(()) | Err(_) => {}
            }
        }
    }

    pub async fn open_desktop(self: &Arc<Self>) -> Result<bool, CoreError> {
        #[cfg(any(unix, windows))]
        {
            let _serialized = self.gui_open.lock().await;
            if self.gui_is_live() {
                return Ok(true);
            }
            let Some((binary, install_dir)) = locate_gui_binary() else {
                return Ok(false);
            };
            let (host_channel, mut child_handles) = ene_local_control::HostChannel::pair()
                .map_err(|error| CoreError::Bind(format!("confirmation channel: {error}")))?;
            let mut command = std::process::Command::new(binary);
            command.current_dir(install_dir);
            for variable in LOADER_ENV_VARS {
                command.env_remove(variable);
            }
            command
                .env(
                    ene_local_control::CONFIRMATION_MODE_ENV,
                    ene_local_control::CONFIRMATION_MODE_STDIO,
                )
                .env("ENE_DATA_DIR", &self.data_dir)
                .stderr(std::process::Stdio::null());
            child_handles.apply(&mut command);
            let child = command
                .spawn()
                .map_err(|error| CoreError::Bind(format!("spawn the official GUI: {error}")))?;
            let child_id = child.id();
            let replaced = lock_unpoison(&self.gui_child).replace(GuiProcess { child, child_id });
            if let Some(previous) = replaced {
                reap_gui_child(previous);
            }
            if !self.attach_confirmation_channel(host_channel) {
                if let Some(process) = lock_unpoison(&self.gui_child).take() {
                    reap_gui_child(process);
                }
                return Ok(false);
            }
            Ok(true)
        }
        #[cfg(not(any(unix, windows)))]
        {
            Ok(false)
        }
    }

    #[cfg(any(unix, windows))]
    fn gui_is_live(&self) -> bool {
        let mut guard = lock_unpoison(&self.gui_child);
        let Some(process) = guard.as_mut() else {
            return false;
        };
        match process.child.try_wait() {
            Ok(None) => self.control_seat.has_seat(),
            Ok(Some(_)) | Err(_) => {
                *guard = None;
                if let Some(generation) = self.control_seat.holder_generation() {
                    self.control_seat.seat_closed(generation);
                }
                false
            }
        }
    }

    #[cfg(any(unix, windows))]
    pub(crate) fn attach_confirmation_channel(
        self: &Arc<Self>,
        host_channel: ene_local_control::HostChannel,
    ) -> bool {
        use ene_local_control::channel::ChannelEvent;

        let (events, events_rx) = std::sync::mpsc::channel::<ChannelEvent>();
        let seat_generation = self.control_seat.seat_spawned_gui(events.clone());
        let reader_channel = match host_channel.try_clone() {
            Ok(clone) => clone,
            Err(_) => {
                self.control_seat.seat_closed(seat_generation);
                return false;
            }
        };
        let reader_tx = events.clone();
        let reader = std::thread::Builder::new()
            .name(String::from("ene-confirmation-read"))
            .spawn(move || {
                let mut channel = reader_channel;
                loop {
                    match channel.recv() {
                        Ok(Some(frame)) => {
                            if reader_tx.send(ChannelEvent::Inbound(frame)).is_err() {
                                break;
                            }
                        }
                        Ok(None) | Err(_) => {
                            match reader_tx.send(ChannelEvent::Closed) {
                                Ok(()) | Err(_) => {}
                            }
                            break;
                        }
                    }
                }
            });
        if reader.is_err() {
            self.control_seat.seat_closed(seat_generation);
            return false;
        }
        let handle = Arc::downgrade(self);
        let replies = events.clone();
        drop(events);
        let runtime = tokio::runtime::Handle::current();
        let served = std::thread::Builder::new()
            .name(String::from("ene-confirmation-serve"))
            .spawn(move || {
                let mut writer = host_channel;
                for event in events_rx {
                    match event {
                        ChannelEvent::Outbound(frame) => {
                            if writer.send(&frame).is_err() {
                                break;
                            }
                        }
                        ChannelEvent::Inbound(frame) => {
                            let Some(host) = handle.upgrade() else {
                                break;
                            };
                            let mut tasks = host
                                .confirmation_tasks
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            if host
                                .confirmation_stopping
                                .load(std::sync::atomic::Ordering::Acquire)
                            {
                                drop(tasks);
                                match replies.send(ChannelEvent::Outbound(
                                    FromConfirmation::DeniedByBoundary,
                                )) {
                                    Ok(()) | Err(_) => {}
                                }
                                continue;
                            }
                            let replies = replies.clone();
                            let dispatched_handle = Arc::clone(&host);
                            let dispatched = runtime.spawn(async move {
                                let reply = dispatch_confirmation(&dispatched_handle, frame).await;
                                match replies.send(ChannelEvent::Outbound(reply)) {
                                    Ok(()) | Err(_) => {}
                                }
                            });
                            tasks.retain(|task| !task.is_finished());
                            tasks.push(dispatched);
                        }
                        ChannelEvent::Closed => break,
                    }
                }
                drop(writer);
                if let Some(handle) = handle.upgrade() {
                    handle.control_seat.seat_closed(seat_generation);
                }
            });
        if served.is_err() {
            self.control_seat.seat_closed(seat_generation);
            return false;
        }
        true
    }
}

#[cfg(any(unix, windows))]
#[doc(hidden)]
pub fn seat_test_gui_for_tests(
    handle: &Arc<HostHandle>,
) -> Result<ene_local_control::GuiChannel, CoreError> {
    let (gui, host) = ene_local_control::GuiChannel::pair_for_test()
        .map_err(|error| CoreError::Bind(format!("test confirmation pair: {error}")))?;
    let _ = handle.attach_confirmation_channel(host);
    Ok(gui)
}

pub async fn request_device_approve(
    data_dir: &Path,
    pending_id: &str,
) -> Result<RequestState, CoreError> {
    #[cfg(any(unix, windows))]
    {
        requester_complete(
            data_dir,
            ToHost::RequestDeviceApprove {
                pending_id: pending_id.to_string(),
            },
        )
        .await
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (data_dir, pending_id);
        Err(CoreError::UnsupportedPlatform(
            "no Host-local control transport",
        ))
    }
}

pub async fn request_credential_put(
    data_dir: &Path,
    provider: &str,
    label: &str,
) -> Result<RequestState, CoreError> {
    #[cfg(any(unix, windows))]
    {
        requester_complete(
            data_dir,
            ToHost::RequestCredentialPut {
                provider: provider.to_string(),
                label: label.to_string(),
            },
        )
        .await
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (data_dir, provider, label);
        Err(CoreError::UnsupportedPlatform(
            "no Host-local control transport",
        ))
    }
}

#[cfg(any(unix, windows))]
async fn requester_complete(data_dir: &Path, request: ToHost) -> Result<RequestState, CoreError> {
    let mut client = ControlClient::connect(data_dir).await?;
    let accepted = client.exchange(&request).await?;
    if matches!(accepted, FromHost::BackpressureHold) {
        return Err(CoreError::Control(String::from(
            "the host-local requester queue is saturated; retry once it drains",
        )));
    }
    let FromHost::RequestAccepted { request_id } = accepted else {
        return Err(control_failure(&format!(
            "the serving Host refused the request as {}",
            from_host_kind(&accepted)
        )));
    };
    for _ in 0..600 {
        let state = client
            .exchange(&ToHost::RequestStatus {
                request_id: request_id.clone(),
            })
            .await?;
        match state {
            FromHost::RequestStatus { state, .. } => match state {
                RequestState::AwaitingOwnerConfirmation => {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                settled => return Ok(settled),
            },
            FromHost::DeniedByBoundary => return Ok(RequestState::OutcomeUnavailable),
            other => {
                return Err(control_failure(&format!(
                    "the serving Host answered {}",
                    from_host_kind(&other)
                )));
            }
        }
    }
    Ok(RequestState::AwaitingOwnerConfirmation)
}

pub async fn confirm_targeted_deletion(
    data_dir: &Path,
    request: &str,
) -> Result<ConfirmTargetedDeletionOutcome, CoreError> {
    #[cfg(any(unix, windows))]
    {
        let state = requester_complete(
            data_dir,
            ToHost::RequestDeletionConfirm {
                request_id: request.to_string(),
            },
        )
        .await?;
        match state {
            RequestState::Applied {
                outcome: RequesterOutcome::Deletion(outcome),
            } => deletion_from_control(&outcome).ok_or_else(|| {
                CoreError::Control(String::from(
                    "the serving Host answered a malformed operation",
                ))
            }),
            RequestState::ConfirmationUnavailable => Err(CoreError::Control(String::from(
                "no confirmation surface was available",
            ))),
            RequestState::Rejected => Err(CoreError::Control(String::from("the Owner declined"))),
            RequestState::AwaitingOwnerConfirmation => Err(CoreError::Control(String::from(
                "the request is still awaiting the Owner",
            ))),
            _ => Err(CoreError::Control(String::from(
                "the confirmation could not be resolved",
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
        FromHost::DesktopOpened => "DesktopOpened",
        FromHost::DesktopUnavailable => "DesktopUnavailable",
        FromHost::RequestAccepted { .. } => "RequestAccepted",
        FromHost::RequestStatus { .. } => "RequestStatus",
        FromHost::PendingDeletions { .. } => "PendingDeletions",
        FromHost::DeniedByBoundary => "DeniedByBoundary",
        FromHost::BackpressureHold => "BackpressureHold",
        FromHost::Unavailable => "Unavailable",
    }
}

#[cfg(any(unix, windows))]
fn control_failure(detail: &str) -> CoreError {
    CoreError::Control(format!(
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

#[cfg(any(unix, windows))]
async fn write_message<W, T>(stream: &mut W, message: &T) -> std::io::Result<()>
where
    W: tokio::io::AsyncWrite + Unpin,
    T: serde::Serialize,
{
    use tokio::io::AsyncWriteExt as _;

    let body = ene_local_control::channel::encode_body(message)?;
    stream.write_all(&(body.len() as u32).to_be_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await
}

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
    if length == 0 || length > ene_local_control::channel::MAX_CONTROL_FRAME_BYTES {
        return Ok(None);
    }
    let mut body = vec![0_u8; length as usize];
    stream.read_exact(&mut body).await?;
    ene_local_control::channel::decode_body(&body).map(Some)
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
            let rendered = deletion_outcome(&outcome);
            assert_eq!(
                deletion_from_control(&rendered),
                Some(outcome),
                "every canonical outcome must round-trip"
            );
        }
    }

    #[test]
    fn a_malformed_operation_identity_is_refused() {
        let malformed = DeletionOutcome::Started {
            operation: String::from("not-a-uuid"),
            sweep: 1,
        };
        assert_eq!(deletion_from_control(&malformed), None);
    }

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
    fn an_unspawned_gui_leaves_no_confirmable_seat() {
        let seat = FirstPartyControlSeat::default();
        assert!(
            !seat.has_seat(),
            "a Host that never spawned a GUI has no seat to hand out"
        );
        let (request_id, state) = seat
            .accept_request()
            .expect("an empty queue admits a request");
        assert!(
            matches!(state, RequestState::ConfirmationUnavailable),
            "a request with no confirmation surface must say so, got {state:?}"
        );
        let refused = seat.mint(
            &request_id,
            ControlOp::DeviceApprove,
            String::from("p"),
            PendingOp::DeviceApprove {
                pending_id: String::from("p"),
            },
        );
        assert!(
            matches!(refused, FromConfirmation::DeniedByBoundary),
            "no challenge may be minted without a Host-spawned GUI"
        );
    }

    #[test]
    fn a_new_spawned_gui_invalidates_the_previous_seat_and_its_sessions() {
        let seat = FirstPartyControlSeat::default();
        let (first_outbound, _first_inbound) = std::sync::mpsc::channel();
        let first = seat.seat_spawned_gui(first_outbound);
        let (request_id, _) = seat
            .accept_request()
            .expect("an empty queue admits a request");
        let FromConfirmation::ConfirmationChallenge {
            session_id, nonce, ..
        } = seat.mint(
            &request_id,
            ControlOp::DeletionConfirm,
            String::from("r"),
            PendingOp::DeletionConfirm {
                request_id: String::from("r"),
            },
        )
        else {
            panic!("a live seat must mint a challenge");
        };
        let (second_outbound, _second_inbound) = std::sync::mpsc::channel();
        let second = seat.seat_spawned_gui(second_outbound);
        assert_ne!(first, second, "a new child is a new seat generation");
        assert!(
            seat.take(session_id, nonce.expose()).is_none(),
            "the previous GUI's session must not complete under the new seat"
        );
    }

    #[test]
    fn the_gui_channel_ending_clears_the_seat() {
        let seat = FirstPartyControlSeat::default();
        let generation = seat.seat_spawned_gui(std::sync::mpsc::channel().0);
        seat.seat_closed(generation);
        assert!(!seat.has_seat(), "a closed GUI leaves no seat behind");
        let (request_id, state) = seat
            .accept_request()
            .expect("an empty queue admits a request");
        assert!(matches!(state, RequestState::ConfirmationUnavailable));
        assert!(matches!(
            seat.mint(
                &request_id,
                ControlOp::CredentialPut,
                String::from("p"),
                PendingOp::CredentialPut {
                    provider: String::from("openai"),
                    label: String::from("main"),
                    mutation_id: String::from("m-test"),
                    secret: None,
                },
            ),
            FromConfirmation::DeniedByBoundary
        ));
    }

    #[test]
    fn a_closing_older_channel_does_not_clear_the_newer_seat() {
        let seat = FirstPartyControlSeat::default();
        let (first_outbound, _first_inbound) = std::sync::mpsc::channel();
        let first = seat.seat_spawned_gui(first_outbound);
        let (second_outbound, _second_inbound) = std::sync::mpsc::channel();
        let second = seat.seat_spawned_gui(second_outbound);
        seat.seat_closed(first);
        assert!(
            seat.has_seat(),
            "an older channel closing must not clear the newer live seat"
        );
        assert_eq!(seat.holder_generation(), Some(second));
    }

    #[test]
    fn an_expired_session_cannot_complete_and_settles_rejected() {
        let seat = FirstPartyControlSeat::default();
        let (outbound, _inbound) = std::sync::mpsc::channel();
        seat.seat_spawned_gui(outbound);
        let (request_id, _) = seat
            .accept_request()
            .expect("an empty queue admits a request");
        let FromConfirmation::ConfirmationChallenge {
            session_id, nonce, ..
        } = seat.mint(
            &request_id,
            ControlOp::DeletionConfirm,
            String::from("r"),
            PendingOp::DeletionConfirm {
                request_id: String::from("r"),
            },
        )
        else {
            panic!("a live seat must mint a challenge");
        };
        {
            let mut inner = lock_unpoison(&seat.inner);
            let session = inner
                .sessions
                .get_mut(&session_id)
                .expect("the minted session must exist");
            session.deadline = std::time::Instant::now() - CONTROL_SESSION_TTL;
        }
        assert!(
            seat.take(session_id, nonce.expose()).is_none(),
            "an expired session must not complete"
        );
        assert_eq!(
            seat.request_state(&request_id),
            Some(RequestState::Rejected),
            "an expired session must settle its request as rejected"
        );
    }

    #[test]
    fn a_foreign_nonce_cannot_complete_a_session() {
        let seat = FirstPartyControlSeat::default();
        let (outbound, _inbound) = std::sync::mpsc::channel();
        seat.seat_spawned_gui(outbound);
        let (request_id, _) = seat
            .accept_request()
            .expect("an empty queue admits a request");
        let FromConfirmation::ConfirmationChallenge { session_id, .. } = seat.mint(
            &request_id,
            ControlOp::DeviceApprove,
            String::from("p"),
            PendingOp::DeviceApprove {
                pending_id: String::from("p"),
            },
        ) else {
            panic!("a live seat must mint a challenge");
        };
        assert!(
            seat.take(session_id, "not-the-minted-nonce").is_none(),
            "a guessed nonce is not authority"
        );
    }

    #[test]
    fn the_pending_request_cap_holds_admission_instead_of_growing() {
        let seat = FirstPartyControlSeat::default();
        let (outbound, _inbound) = std::sync::mpsc::channel();
        seat.seat_spawned_gui(outbound);
        let mut admitted = Vec::new();
        for _ in 0..MAX_PENDING_REQUESTS {
            let (request_id, _) = seat
                .accept_request()
                .expect("admission is available below the cap");
            admitted.push(request_id);
        }
        assert!(
            seat.accept_request().is_none(),
            "a full pending set must be held, never admitted"
        );
        let first = admitted.remove(0);
        seat.reject_request(&first);
        assert!(
            seat.accept_request().is_some(),
            "a settled request must free one pending slot"
        );
    }

    #[test]
    fn expired_sessions_are_retired_before_a_new_admission() {
        let seat = FirstPartyControlSeat::default();
        let (outbound, _inbound) = std::sync::mpsc::channel();
        seat.seat_spawned_gui(outbound);
        let (request_id, _) = seat
            .accept_request()
            .expect("an empty queue admits a request");
        let FromConfirmation::ConfirmationChallenge { session_id, .. } = seat.mint(
            &request_id,
            ControlOp::DeletionConfirm,
            String::from("r"),
            PendingOp::DeletionConfirm {
                request_id: String::from("r"),
            },
        ) else {
            panic!("a live seat must mint a challenge");
        };
        {
            let mut inner = lock_unpoison(&seat.inner);
            let session = inner
                .sessions
                .get_mut(&session_id)
                .expect("the minted session must exist");
            session.deadline = std::time::Instant::now() - CONTROL_SESSION_TTL;
        }
        let admitted = seat.accept_request();
        assert!(
            admitted.is_some(),
            "the reclaimed slot admits a new request"
        );
        assert_eq!(
            seat.request_state(&request_id),
            Some(RequestState::Rejected),
            "the retired request reads back rejected"
        );
        let inner = lock_unpoison(&seat.inner);
        assert!(
            inner.sessions.is_empty(),
            "the expired session is reclaimed, not retained: {} remain",
            inner.sessions.len()
        );
    }
}
