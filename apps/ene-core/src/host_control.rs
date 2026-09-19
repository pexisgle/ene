//! Host-local first-party control (Stage 7 A1).
//!
//! Two channels with different authority, never one endpoint:
//!
//! - The **requester listener** (`host-control.sock` / `<device pipe>-control`)
//!   is a local endpoint any same-user process may dial. It accepts requests
//!   and answers non-secret request state. It issues no seat, carries no
//!   secret, and cannot complete a confirmation session: its frame types are
//!   [`ToHost`] / [`FromHost`], which have no challenge or completion shape.
//! - The **inherited confirmation channel** is a private socket pair / pipe
//!   the Host creates while spawning the official GUI and hands only to that
//!   child. It is the only place [`ConfirmationChallenge`], secret intake, and
//!   [`SessionComplete`](ToConfirmation::SessionComplete) exist.
//!
//! Seat authority comes from the spawn: the Host records the child process
//! handle, the private endpoint, the Host incarnation, and a seat generation
//! together. Empty-seat first-come, a declared PID, a copied nonce, and the
//! ability to open the requester listener are all non-authority.
//!
//! Frames are [`ene_local_control`] JSON, not `ene-api`. Secrets use
//! [`RedactedSecret`]: `Debug` never prints the raw value.

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

/// Upper bound on one control frame. Credential intake carries one secret;
/// anything larger is not this protocol.
#[cfg(any(unix, windows))]
const MAX_CONTROL_FRAME_BYTES: u32 = 16 * 1024;

/// Bound on the first (hello) read from a control peer.
#[cfg(any(unix, windows))]
const CONTROL_HELLO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Unix requester-listener socket name inside the `0700` data directory.
#[cfg(unix)]
const CONTROL_SOCKET_NAME: &str = "host-control.sock";

/// One accepted requester request: what the Owner's surface is being asked to
/// confirm, and what it has decided so far.
struct AcceptedRequest {
    state: RequestState,
}

/// One pending high-privilege operation waiting for session complete.
enum PendingOp {
    DeviceApprove {
        pending_id: String,
    },
    CredentialPut {
        provider: String,
        label: String,
        /// Stable identity of this registration attempt, so a retry observes
        /// the original mutation instead of starting a second one.
        mutation_id: String,
        /// Staged candidate, filled by the confirmation channel's secret
        /// intake. Empty means the Owner never entered a value, which is a
        /// refusal, never a stored credential.
        secret: Option<RedactedSecret>,
    },
    DeletionConfirm {
        request_id: String,
    },
}

struct ConfirmationSession {
    nonce: String,
    seat_generation: u64,
    request_id: String,
    pending: PendingOp,
}

/// Identity of the GUI the Host itself started.
///
/// Recorded at spawn, not declared by a peer. The child identity is the OS
/// process handle's id plus the live child record; the seat generation moves
/// whenever a new child takes the seat.
pub(crate) struct SeatedGui {
    pub(crate) child_id: u32,
    pub(crate) generation: u64,
}

struct SeatInner {
    /// The GUI the Host started, while its private channel is live.
    holder: Option<SeatedGui>,
    next_generation: u64,
    sessions: HashMap<Uuid, ConfirmationSession>,
    requests: HashMap<String, AcceptedRequest>,
    /// Outbound side of the live GUI's private channel. Present exactly while
    /// a Host-spawned GUI holds the seat.
    outbound: Option<std::sync::mpsc::Sender<ene_local_control::channel::ChannelEvent>>,
}

/// Exclusive first-party control seat owned by one serving Host.
///
/// The seat is issued to the Host-spawned GUI alone. Nothing in this type can
/// be reached from the requester listener.
pub(crate) struct FirstPartyControlSeat {
    inner: StdMutex<SeatInner>,
}

impl Default for FirstPartyControlSeat {
    fn default() -> Self {
        Self {
            inner: StdMutex::new(SeatInner {
                holder: None,
                next_generation: 0,
                sessions: HashMap::new(),
                requests: HashMap::new(),
                outbound: None,
            }),
        }
    }
}

impl FirstPartyControlSeat {
    /// Records the Host-spawned GUI as the seat holder, advancing the seat
    /// generation. Any older holder is replaced and its sessions are dropped:
    /// a new child is a new seat, never a continuation.
    pub(crate) fn seat_spawned_gui(
        &self,
        child_id: u32,
        outbound: std::sync::mpsc::Sender<ene_local_control::channel::ChannelEvent>,
    ) -> u64 {
        let mut inner = lock_unpoison(&self.inner);
        inner.next_generation = inner.next_generation.saturating_add(1);
        let generation = inner.next_generation;
        inner.holder = Some(SeatedGui {
            child_id,
            generation,
        });
        inner.sessions.clear();
        inner.outbound = Some(outbound);
        generation
    }

    /// Clears the seat when the spawned GUI's private channel ends. Sessions
    /// die with it: an unconfirmed request never survives its surface.
    pub(crate) fn seat_closed(&self, child_id: u32) {
        let mut inner = lock_unpoison(&self.inner);
        if inner
            .holder
            .as_ref()
            .is_some_and(|holder| holder.child_id == child_id)
        {
            inner.holder = None;
            inner.sessions.clear();
            inner.outbound = None;
        }
    }

    /// True while a Host-spawned GUI holds the seat.
    pub(crate) fn has_seat(&self) -> bool {
        lock_unpoison(&self.inner).holder.is_some()
    }

    /// Registers one accepted requester request under a Host-issued id.
    ///
    /// The request carries no authority of its own: it only records what the
    /// Owner's surface should show, and its state is read back by the same id.
    fn accept_request(&self) -> (String, RequestState) {
        let mut inner = lock_unpoison(&self.inner);
        let request_id = Uuid::new_v4().as_hyphenated().to_string();
        let state = if inner.holder.is_some() {
            RequestState::AwaitingOwnerConfirmation
        } else {
            RequestState::ConfirmationUnavailable
        };
        inner.requests.insert(
            request_id.clone(),
            AcceptedRequest {
                state: state.clone(),
            },
        );
        (request_id, state)
    }

    fn request_state(&self, request_id: &str) -> Option<RequestState> {
        lock_unpoison(&self.inner)
            .requests
            .get(request_id)
            .map(|request| request.state.clone())
    }

    /// Mints a one-shot challenge for an accepted request, bound to the live
    /// seat generation. Refused when no Host-spawned GUI holds the seat.
    fn mint(
        &self,
        request_id: &str,
        op: ControlOp,
        target: String,
        pending: PendingOp,
    ) -> FromConfirmation {
        let mut inner = lock_unpoison(&self.inner);
        let Some(holder) = inner.holder.as_ref() else {
            return FromConfirmation::DeniedByBoundary;
        };
        let seat_generation = holder.generation;
        let session_id = Uuid::new_v4();
        let nonce = Uuid::new_v4().as_hyphenated().to_string();
        let premise_generation = seat_generation;
        inner.sessions.insert(
            session_id,
            ConfirmationSession {
                nonce: nonce.clone(),
                seat_generation,
                request_id: request_id.to_string(),
                pending,
            },
        );
        let challenge = FromConfirmation::ConfirmationChallenge {
            session_id,
            op,
            target,
            premise_generation,
            nonce,
        };
        // Deliver it to the Host-spawned GUI's thread. A channel that cannot
        // take the frame has already ended; the session stays unconsumed and
        // the next spawn invalidates it.
        if let Some(outbound) = inner.outbound.as_ref() {
            match outbound.send(ene_local_control::channel::ChannelEvent::Outbound(
                challenge.clone(),
            )) {
                Ok(()) | Err(_) => {}
            }
        }
        challenge
    }

    /// Takes one session for completion, checking the nonce and the seat
    /// generation it was minted under.
    fn take(&self, session_id: Uuid, nonce: &str) -> Option<(String, PendingOp)> {
        let mut inner = lock_unpoison(&self.inner);
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

    /// Drops one session the Owner declined on the surface.
    fn reject(&self, session_id: Uuid, nonce: &str) -> Option<String> {
        let mut inner = lock_unpoison(&self.inner);
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

    /// Stages one secret candidate into the live session it names.
    ///
    /// The value is accepted only for a session that is live under the current
    /// seat generation, whose nonce matches, and whose operation is a
    /// credential put for the same `target`. Anything else is refused without
    /// keeping the bytes.
    fn stage_credential_secret(
        &self,
        session_id: Uuid,
        nonce: &str,
        target: &str,
        secret: RedactedSecret,
    ) -> bool {
        let mut inner = lock_unpoison(&self.inner);
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

    /// Records the settled state of one request so a requester polling the
    /// same id observes the owner's decision.
    fn settle_request(&self, request_id: &str, outcome: RequesterOutcome) {
        let mut inner = lock_unpoison(&self.inner);
        if let Some(request) = inner.requests.get_mut(request_id) {
            request.state = RequestState::Applied { outcome };
        }
    }
}

/// The requester-listener endpoint of one data directory.
#[cfg(unix)]
#[must_use]
pub fn control_socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CONTROL_SOCKET_NAME)
}

/// The requester-listener pipe of one data directory: the device pipe name
/// plus a control suffix, so both endpoints share the same data-directory
/// derivation and DACL machinery.
#[cfg(windows)]
#[must_use]
pub fn control_pipe_name(data_dir: &Path) -> String {
    format!("{}-control", crate::conn_pipe::pipe_name(data_dir))
}

/// Console / test speaker for the requester listener.
#[cfg(unix)]
pub struct ControlClient {
    stream: tokio::net::UnixStream,
}

/// Console / test speaker for the requester listener.
#[cfg(windows)]
pub struct ControlClient {
    stream: tokio::net::windows::named_pipe::NamedPipeClient,
}

#[cfg(any(unix, windows))]
impl ControlClient {
    /// Dials the serving Host's requester listener.
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

    /// Writes one requester frame and reads one answer.
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

    /// Sends raw JSON on the requester listener and reports whether an answer
    /// came back. Test-only: it exists so a regression can prove that a
    /// completion-shaped frame cannot even be decoded here.
    #[doc(hidden)]
    pub async fn exchange_raw(&mut self, raw: &str) -> Result<String, CoreError> {
        let body = raw.as_bytes();
        if body.len() > MAX_CONTROL_FRAME_BYTES as usize {
            return Err(control_failure("frame exceeds the bound"));
        }
        write_bytes(&mut self.stream, body)
            .await
            .map_err(|error| control_failure(&format!("send failed: {error}")))?;
        let answer = read_raw_answer(&mut self.stream)
            .await
            .map_err(|error| control_failure(&format!("answer failed: {error}")))?;
        answer.ok_or_else(|| control_failure("the serving Host closed without an answer"))
    }
}

/// The requester listener, bound before the device listener accepts so a
/// request can never race startup.
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

    /// Accepts at most one requester connection, checks the peer credential,
    /// and returns it. An unprovable peer is dropped without a byte.
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

/// The Windows requester listener: the exclusive first instance of the
/// control pipe (a live peer fails creation, like the device pipe) with the
/// same logon-SID DACL and OS peer-token check.
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
            // A failed wait leaves this instance unusable; replace it rather
            // than serving half-open state.
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

/// Serves one requester connection: requests in, non-secret state out.
///
/// A requester never receives a challenge, a secret, or a completion: the
/// frame types it can decode have no such shape, and every completion-shaped
/// request it can express is refused.
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
                CONTROL_HELLO_TIMEOUT,
                read_message::<_, ToHost>(&mut stream),
            ) => match result {
                Ok(Ok(Some(request))) => request,
                Ok(Ok(None) | Err(_)) | Err(_) => break,
            },
        };
        let reply = dispatch_requester(&handle, request).await;
        if write_message(&mut stream, &reply).await.is_err() {
            break;
        }
    }
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
            let (request_id, state) = handle.control_seat.accept_request();
            if matches!(state, RequestState::AwaitingOwnerConfirmation) {
                handle.control_seat.mint(
                    &request_id,
                    ControlOp::DeviceApprove,
                    pending_id.clone(),
                    PendingOp::DeviceApprove { pending_id },
                );
            }
            FromHost::RequestAccepted { request_id }
        }
        ToHost::RequestCredentialPut { provider, label } => {
            let target = format!("{provider}:{label}");
            let (request_id, state) = handle.control_seat.accept_request();
            if matches!(state, RequestState::AwaitingOwnerConfirmation) {
                handle.control_seat.mint(
                    &request_id,
                    ControlOp::CredentialPut,
                    target,
                    PendingOp::CredentialPut {
                        provider,
                        label,
                        mutation_id: Uuid::new_v4().as_hyphenated().to_string(),
                        // The secret arrives on the confirmation channel; the
                        // requester only names the pair.
                        secret: None,
                    },
                );
            }
            FromHost::RequestAccepted { request_id }
        }
        ToHost::RequestDeletionConfirm { request_id } => {
            let (accepted_id, state) = handle.control_seat.accept_request();
            if matches!(state, RequestState::AwaitingOwnerConfirmation) {
                handle.control_seat.mint(
                    &accepted_id,
                    ControlOp::DeletionConfirm,
                    request_id.clone(),
                    PendingOp::DeletionConfirm { request_id },
                );
            }
            FromHost::RequestAccepted {
                request_id: accepted_id,
            }
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

/// Serves the inherited confirmation channel of one Host-spawned GUI.
///
/// Only this path may complete a session or accept a secret. The seat is
/// released when the channel ends, so the GUI's own death cannot leave a
/// confirmable session behind.
#[cfg(any(unix, windows))]
async fn dispatch_confirmation(handle: &HostHandle, request: ToConfirmation) -> FromConfirmation {
    match request {
        ToConfirmation::SessionComplete { session_id, nonce } => {
            match handle.control_seat.take(session_id, &nonce) {
                Some((request_id, pending)) => {
                    let (reply, outcome) = execute_pending(handle, pending).await;
                    if let Some(outcome) = outcome {
                        handle.control_seat.settle_request(&request_id, outcome);
                    }
                    reply
                }
                None => FromConfirmation::DeniedByBoundary,
            }
        }
        ToConfirmation::SessionReject { session_id, nonce } => {
            match handle.control_seat.reject(session_id, &nonce) {
                Some(_) => FromConfirmation::Outcome(ControlOutcome::Rejected { session_id }),
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
            // The secret binds to a live session for the same pair: a value
            // for another session, or one whose session already settled, is
            // refused without storing anything.
            if !handle.control_seat.stage_credential_secret(
                session_id,
                &nonce,
                &format!("{provider}:{label}"),
                secret,
            ) {
                return FromConfirmation::DeniedByBoundary;
            }
            // Intake alone neither stores nor publishes: the Owner's direct
            // confirmation on the same surface consumes the staged value.
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
                Ok(_) | Err(_) => FromConfirmation::Unavailable,
            }
        }
        ToConfirmation::ConfirmedTrue => FromConfirmation::DeniedByBoundary,
    }
}

/// Runs one confirmed operation and renders both its wire answer and the
/// non-secret fact a requester may poll.
#[cfg(any(unix, windows))]
async fn execute_pending(
    handle: &HostHandle,
    pending: PendingOp,
) -> (FromConfirmation, Option<RequesterOutcome>) {
    match pending {
        PendingOp::DeviceApprove { pending_id } => match handle.approve_device(&pending_id).await {
            Ok(Some((record, secret))) => (
                FromConfirmation::Outcome(ControlOutcome::DeviceApproved {
                    pending_id: pending_id.clone(),
                    device_id: record.wire.clone(),
                    pairing_secret: RedactedSecret::new(secret),
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
            // Completion consumes the candidate staged on the confirmation
            // channel. No candidate means the Owner never entered a value: a
            // refusal, never a stored secret.
            let Some(secret) = secret else {
                return (
                    FromConfirmation::Outcome(ControlOutcome::CredentialRefused {
                        provider: provider.clone(),
                        label: label.clone(),
                    }),
                    Some(RequesterOutcome::CredentialRefused { provider, label }),
                );
            };
            // The OS item existing is not registration: the approval sweep and
            // the usable reference must commit together, and a commit that did
            // not happen is its own state rather than success or a clean miss.
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

/// Renders one credential publication outcome for both channels.
///
/// Only a committed activation is `CredentialStored`. A commit that did not
/// happen is its own state, and an undetermined result is neither success nor
/// "nothing happened".
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
        DeletionOutcome::Resumed { .. } => return None,
    })
}

/// One GUI process the Host started, kept for liveness and for reaping.
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

/// Resolves the official GUI executable: the `ene-desktop` binary installed
/// beside this Host binary.
///
/// The path is fixed by the installation. No request, environment variable,
/// `PATH` entry, or current directory selects it, so a requester cannot aim
/// the Host at a different program.
#[cfg(any(unix, windows))]
fn locate_gui_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let sibling = exe.parent()?.join(if cfg!(windows) {
        "ene-desktop.exe"
    } else {
        "ene-desktop"
    });
    sibling.is_file().then_some(sibling)
}

impl HostHandle {
    /// Waits for every in-flight confirmation dispatch to finish.
    ///
    /// The serving composition calls this before releasing its authority: an
    /// operation the Owner's surface already admitted must not be cut in half,
    /// and no dispatch may outlive the handle it borrowed.
    pub(crate) async fn join_confirmation_tasks(&self) {
        loop {
            let next = {
                let mut tasks = self
                    .confirmation_tasks
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
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

    /// Opens the official first-party GUI, or converges on the live one.
    ///
    /// The Host spawns the child itself and hands it the private confirmation
    /// channel as its stdio; the seat is issued from that spawn alone. A live
    /// GUI is never replaced, so a second request cannot take a seat from the
    /// first. Returns `Ok(false)` when no GUI could be started, which is the
    /// `ConfirmationUnavailable` outcome for any requester.
    ///
    /// # Errors
    ///
    /// [`CoreError::Bind`] when the private channel or the child cannot be
    /// created. A missing GUI binary is `Ok(false)`, never an error: an
    /// installation without the GUI has no confirmation surface, not a fault.
    pub async fn open_desktop(self: &Arc<Self>) -> Result<bool, CoreError> {
        #[cfg(any(unix, windows))]
        {
            let _serialized = self.gui_open.lock().await;
            if self.gui_is_live() {
                return Ok(true);
            }
            let Some(binary) = locate_gui_binary() else {
                return Ok(false);
            };
            let (host_channel, mut child_handles) = ene_local_control::HostChannel::pair()
                .map_err(|error| CoreError::Bind(format!("confirmation channel: {error}")))?;
            let mut command = std::process::Command::new(binary);
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
            *lock_unpoison(&self.gui_child) = Some(GuiProcess { child, child_id });
            self.attach_confirmation_channel(host_channel, child_id);
            Ok(true)
        }
        #[cfg(not(any(unix, windows)))]
        {
            Ok(false)
        }
    }

    /// True while the GUI the Host started is alive and holds the seat.
    #[cfg(any(unix, windows))]
    fn gui_is_live(&self) -> bool {
        let mut guard = lock_unpoison(&self.gui_child);
        let Some(process) = guard.as_mut() else {
            return false;
        };
        match process.child.try_wait() {
            Ok(None) => self.control_seat.has_seat(),
            Ok(Some(_)) | Err(_) => {
                let child_id = process.child_id;
                *guard = None;
                self.control_seat.seat_closed(child_id);
                false
            }
        }
    }

    /// Registers one spawned GUI's private channel and serves it.
    ///
    /// Exactly one thread owns the channel: it writes outbound challenges and
    /// replies, dispatches inbound frames against this handle, and clears the
    /// seat when the child's channel ends.
    #[cfg(any(unix, windows))]
    pub(crate) fn attach_confirmation_channel(
        self: &Arc<Self>,
        host_channel: ene_local_control::HostChannel,
        child_id: u32,
    ) {
        use ene_local_control::channel::ChannelEvent;

        // One queue carries both directions. Two queues would need a select
        // that a blocking reader cannot offer: `recv` on the inbound queue
        // would starve the outbound challenges behind it.
        let (events, events_rx) = std::sync::mpsc::channel::<ChannelEvent>();
        let seat_generation = self.control_seat.seat_spawned_gui(child_id, events.clone());
        let reader_channel = match host_channel.try_clone() {
            Ok(clone) => clone,
            Err(_) => {
                self.control_seat.seat_closed(child_id);
                return;
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
            self.control_seat.seat_closed(child_id);
            return;
        }
        // The serving thread must not keep the Host alive: a GUI whose channel
        // stays open would otherwise retain the predecessor past shutdown. It
        // upgrades per frame and reports Unavailable once the Host is gone.
        let handle = Arc::downgrade(self);
        // Replies from dispatched frames come back through the same queue, so
        // the channel keeps one writer and one ordering.
        let replies = events.clone();
        drop(events);
        // Dispatch runs on the runtime, never by blocking this thread on it:
        // a current-thread runtime is driven by its own caller, so a foreign
        // `block_on` here would park instead of progressing. The thread only
        // moves bytes; replies come back through the same queue it drains.
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
                            let replies = replies.clone();
                            let dispatched_handle = Arc::clone(&host);
                            let dispatched = runtime.spawn(async move {
                                let reply = dispatch_confirmation(&dispatched_handle, frame).await;
                                match replies.send(ChannelEvent::Outbound(reply)) {
                                    Ok(()) | Err(_) => {}
                                }
                            });
                            // The Host joins these on shutdown: an operation the
                            // Owner's surface already admitted runs to
                            // completion before the serving authority goes
                            // away, and no task outlives the handle it borrows.
                            host.confirmation_tasks
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .push(dispatched);
                        }
                        ChannelEvent::Closed => break,
                    }
                }
                drop(writer);
                // The seat belongs to the Host, which may already be gone;
                // clearing it is best effort and never resurrects the handle.
                if let Some(handle) = handle.upgrade() {
                    handle.control_seat.seat_closed(child_id);
                }
            });
        if served.is_err() {
            self.control_seat.seat_closed(child_id);
            return;
        }
        let _ = seat_generation;
    }
}

/// Gives one test the GUI end of a private channel registered as the Host's
/// spawned child.
///
/// The design allows a test-only child for protocol and race coverage; it is
/// never a product path and never stands in for the Owner's direct gesture in
/// an acceptance run.
///
/// # Errors
///
/// [`CoreError::Bind`] when the private pair cannot be created.
#[cfg(any(unix, windows))]
#[doc(hidden)]
pub fn seat_test_gui_for_tests(
    handle: &Arc<HostHandle>,
) -> Result<ene_local_control::GuiChannel, CoreError> {
    let (gui, host) = ene_local_control::GuiChannel::pair_for_test()
        .map_err(|error| CoreError::Bind(format!("test confirmation pair: {error}")))?;
    handle.attach_confirmation_channel(host, 0);
    Ok(gui)
}

/// Owner device approval over the requester listener.
///
/// # Errors
///
/// [`CoreError::Deletion`] when the requester listener is unreachable.
/// A request that no Owner surface could confirm is `Ok(None)`.
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

/// Serving-time credential registration request.
///
/// The pair is named, never the value: the raw secret is accepted from the
/// inherited confirmation channel alone, so this call can only ask the Owner's
/// surface to open the intake. Returns the settled requester state.
///
/// # Errors
///
/// [`CoreError::Deletion`] when the requester listener is unreachable.
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

/// Dials the requester listener, sends one request, and follows the accepted
/// request id until the Owner's boundary settles it.
#[cfg(any(unix, windows))]
async fn requester_complete(data_dir: &Path, request: ToHost) -> Result<RequestState, CoreError> {
    let mut client = ControlClient::connect(data_dir).await?;
    let accepted = client.exchange(&request).await?;
    let FromHost::RequestAccepted { request_id } = accepted else {
        return Err(control_failure(&format!(
            "the serving Host refused the request as {}",
            from_host_kind(&accepted)
        )));
    };
    // The Owner confirms out of band; poll the Host-issued id until the
    // boundary settles it. The poll is bounded: an unanswered request is
    // reported as still awaiting, never as applied.
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

/// Dials the serving Host's requester listener and records the Owner's
/// confirmation inside it.
///
/// # Errors
///
/// Returns [`CoreError::Deletion`] with recovery guidance when no serving
/// Host answers (the offline fallback is deliberately absent).
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
            } => deletion_from_control(&outcome)
                .ok_or_else(|| control_failure("the serving Host answered a malformed operation")),
            RequestState::ConfirmationUnavailable | RequestState::Rejected => {
                Err(control_failure("no confirmation surface was available"))
            }
            RequestState::AwaitingOwnerConfirmation => Err(control_failure(
                "the Owner did not confirm before the request timed out",
            )),
            _ => Err(control_failure("the confirmation could not be resolved")),
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
        FromHost::Unavailable => "Unavailable",
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

/// Writes one already-encoded body with its length prefix.
#[cfg(any(unix, windows))]
async fn write_bytes<W>(stream: &mut W, body: &[u8]) -> std::io::Result<()>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt as _;

    stream.write_all(&(body.len() as u32).to_be_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await
}

/// Reads one length-prefixed body verbatim; [`None`] when the peer closed or
/// the frame is malformed or oversize.
#[cfg(any(unix, windows))]
async fn read_raw_answer<R>(stream: &mut R) -> std::io::Result<Option<String>>
where
    R: tokio::io::AsyncRead + Unpin,
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
    Ok(Some(String::from_utf8_lossy(&body).into_owned()))
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
    fn an_unspawned_gui_leaves_no_confirmable_seat() {
        let seat = FirstPartyControlSeat::default();
        assert!(
            !seat.has_seat(),
            "a Host that never spawned a GUI has no seat to hand out"
        );
        let (request_id, state) = seat.accept_request();
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
        let first = seat.seat_spawned_gui(11, std::sync::mpsc::channel().0);
        let (request_id, _) = seat.accept_request();
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
        let second = seat.seat_spawned_gui(22, std::sync::mpsc::channel().0);
        assert_ne!(first, second, "a new child is a new seat generation");
        assert!(
            seat.take(session_id, &nonce).is_none(),
            "the previous GUI's session must not complete under the new seat"
        );
    }

    #[test]
    fn the_gui_channel_ending_clears_the_seat() {
        let seat = FirstPartyControlSeat::default();
        seat.seat_spawned_gui(7, std::sync::mpsc::channel().0);
        seat.seat_closed(7);
        assert!(!seat.has_seat(), "a closed GUI leaves no seat behind");
        let (request_id, state) = seat.accept_request();
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
    fn a_foreign_nonce_cannot_complete_a_session() {
        let seat = FirstPartyControlSeat::default();
        seat.seat_spawned_gui(3, std::sync::mpsc::channel().0);
        let (request_id, _) = seat.accept_request();
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
}
