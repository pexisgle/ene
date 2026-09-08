//! Unix socket listener: accept, same-user check, frame loop, close.
//!
//! The listener binds `ene.sock` inside the data directory (removing a stale
//! file first) and serves one task per connection. Each task reads
//! length-prefixed [`ene_plugin_ipc::WireFrame`] values, runs them through
//! [`HostHandle::handle_frame`], and writes the responses back. A
//! [`DisconnectNotice`](ene_api::v1::handshake::DisconnectNotice) in the
//! responses is terminal: it is written, then the connection closes.
//!
//! Same-user proof without new dependencies: after binding, the listener reads
//! the socket file owner through [`MetadataExt::uid`](std::os::unix::fs::MetadataExt)
//! (the file is created by this process inside the `0700` data directory, so
//! its owner is the Host user) and compares it against each peer credential
//! uid from [`tokio::net::UnixStream::peer_cred`]. A mismatch, or an unreadable peer
//! credential, closes the connection before any frame is read: an unprovable
//! peer is a trust violation, not a protocol peer, so it receives no bytes
//! (not even a denial, which would be an oracle). [`LiveInput::peer_uid_ok`]
//! still travels into [`HostHandle::handle_frame`] for the pairing decision,
//! as defense in depth for direct handle callers.
//!
//! Corrupt or oversize frames close the connection without a reply: the frame
//! cannot be attributed to a request, so there is nothing honest to answer.
//! An oversize response (only reachable through an unbounded timeline today)
//! likewise ends the connection; paging that path is deferred work.
//!
//! Windows has no listener yet: [`run`] returns
//! [`CoreError::UnsupportedPlatform`] there. The follow-up is a named-pipe
//! listener behind the same [`HostHandle::handle_frame`] seam, which keeps the
//! Windows build green by holding no Unix import outside `cfg(unix)`.
//!
//! [`LiveInput::peer_uid_ok`]: crate::serve::LiveInput::peer_uid_ok
//! [`MetadataExt::uid`](std::os::unix::fs::MetadataExt): <https://doc.rust-lang.org/std/os/unix/fs/trait.MetadataExt.html>

use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(unix)]
use ene_api::v1::envelope::WireEnvelope;
#[cfg(unix)]
use ene_api::v1::payload::WirePayload;
use ene_credential::EnvCredentialStore;
use ene_inference::provider::OpenAiResponsesTransport;
#[cfg(unix)]
use ene_plugin_ipc::{MAX_FRAME_BYTES, decode_frame, encode_frame};
use tokio::sync::Mutex;

#[cfg(unix)]
use crate::serve::LiveInput;
use crate::serve::{CoreError, HostHandle};

/// Socket filename inside the data directory.
const SOCKET_NAME: &str = "ene.sock";

/// Resolves the listener socket path for a data directory.
///
/// Public so the sibling Client dialer and integration tests derive the same
/// path the Host binds: there is exactly one socket name and it lives here.
#[must_use]
pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SOCKET_NAME)
}

/// Derives the opaque client ref for one inbound envelope.
///
/// The paired device key names the client when present; otherwise the
/// incarnation pair does. Both are routing hints only: the Host pins the ref
/// to a [`ene_presence::ClientId`] on first use and never treats the ref
/// itself as authority.
#[cfg(unix)]
fn client_ref_for(envelope: &WireEnvelope) -> String {
    if let Some(device) = &envelope.sender.device_id {
        device.0.as_hyphenated().to_string()
    } else {
        let incarnation = envelope.sender.incarnation_id;
        format!("incarnation-{}-{}", incarnation.counter, incarnation.random)
    }
}

/// Serves the Unix socket listener until the process ends.
///
/// Binds [`socket_path`], proves each peer against the socket owner, and
/// spawns one frame-loop task per authorized connection. The transport is the
/// production `OpenAI` transport; the fake-friendly seam is
/// [`HostHandle::handle_frame`], which this loop drives. There is no shutdown
/// signal in `Stage 2`: the future resolves only on bind failure; otherwise it
/// runs until killed.
///
/// # Errors
///
/// Returns [`CoreError::Bind`] when the stale socket cannot be cleared, the
/// bind fails, or the socket metadata cannot be read.
#[cfg(unix)]
pub async fn run(
    data_dir: &Path,
    handle: Arc<Mutex<HostHandle>>,
    transport: Arc<OpenAiResponsesTransport<EnvCredentialStore>>,
) -> Result<(), CoreError> {
    use std::os::unix::fs::MetadataExt as _;
    use tokio::net::UnixListener;

    let socket = socket_path(data_dir);
    match std::fs::remove_file(&socket) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CoreError::Bind(format!("remove stale socket: {error}")));
        }
    }
    let listener =
        UnixListener::bind(&socket).map_err(|error| CoreError::Bind(format!("bind: {error}")))?;
    let owner = std::fs::metadata(&socket)
        .map_err(|error| CoreError::Bind(format!("read socket metadata: {error}")))?
        .uid();
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let peer_ok = match stream.peer_cred() {
            Ok(cred) => cred.uid() == owner,
            Err(_) => false,
        };
        if !peer_ok {
            continue;
        }
        let handle = Arc::clone(&handle);
        let transport = Arc::clone(&transport);
        tokio::spawn(async move {
            serve_connection(stream, handle, transport).await;
        });
    }
}

/// Runs one connection frame loop: read, handle, write, close on terminal.
#[cfg(unix)]
async fn serve_connection(
    mut stream: tokio::net::UnixStream,
    handle: Arc<Mutex<HostHandle>>,
    transport: Arc<OpenAiResponsesTransport<EnvCredentialStore>>,
) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let mut prefix = [0_u8; 4];
    loop {
        if stream.read_exact(&mut prefix).await.is_err() {
            break;
        }
        let claimed = u32::from_be_bytes(prefix) as usize;
        if claimed > MAX_FRAME_BYTES {
            break;
        }
        let mut body = vec![0_u8; claimed];
        if stream.read_exact(&mut body).await.is_err() {
            break;
        }
        let mut bytes = Vec::with_capacity(prefix.len() + body.len());
        bytes.extend_from_slice(&prefix);
        bytes.extend_from_slice(&body);
        let Ok((frame, _)) = decode_frame(&bytes) else {
            break;
        };
        let live = LiveInput {
            client_ref: client_ref_for(&frame.envelope),
            connection_live: true,
            peer_uid_ok: true,
        };
        let responses = {
            let mut guard = handle.lock().await;
            guard.handle_frame(frame, live, transport.as_ref()).await
        };
        let mut failed = false;
        let mut terminal = false;
        for response in &responses {
            if matches!(response.payload, WirePayload::DisconnectNotice(_)) {
                terminal = true;
            }
            let Ok(encoded) = encode_frame(response) else {
                failed = true;
                break;
            };
            if stream.write_all(&encoded).await.is_err() {
                failed = true;
                break;
            }
        }
        if failed || terminal {
            break;
        }
    }
}

/// Windows stub: no listener yet.
///
/// The follow-up is a named-pipe listener behind the same
/// [`HostHandle::handle_frame`] seam.
///
/// # Errors
///
/// Always returns [`CoreError::UnsupportedPlatform`].
#[cfg(not(unix))]
#[expect(
    clippy::unused_async,
    reason = "stub mirrors the async listener signature; the named-pipe follow-up awaits"
)]
pub async fn run(
    _data_dir: &Path,
    _handle: Arc<Mutex<HostHandle>>,
    _transport: Arc<OpenAiResponsesTransport<EnvCredentialStore>>,
) -> Result<(), CoreError> {
    Err(CoreError::UnsupportedPlatform("named-pipe listener"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::socket_path;

    #[test]
    fn socket_path_appends_the_socket_name() {
        let dir = std::path::Path::new("/tmp/ene-probe-data");
        assert_eq!(
            socket_path(dir),
            std::path::Path::new("/tmp/ene-probe-data/ene.sock"),
            "the socket lives inside the data directory"
        );
    }

    #[tokio::test]
    async fn stale_regular_file_blocks_bind_until_removed() {
        let Some(dir) = crate::test_support::temp_data_dir("conn-bind") else {
            return;
        };
        let socket = socket_path(&dir);
        assert!(
            std::fs::write(&socket, b"stale").is_ok(),
            "the stale probe file must be writable"
        );
        let bound = tokio::net::UnixListener::bind(&socket);
        // A stale regular file blocks the bind: this documents why `run`
        // removes the path first (the test only proves the premise, the
        // removal itself runs inside `run`).
        assert!(
            bound.is_err(),
            "a stale regular file must block a fresh bind: {bound:?}"
        );
        assert!(
            std::fs::remove_file(&socket).is_ok(),
            "stale removal must clear the path"
        );
        let rebound = tokio::net::UnixListener::bind(&socket);
        assert!(rebound.is_ok(), "the cleared path must bind: {rebound:?}");
        crate::test_support::remove_data_dir(&dir);
    }
}
