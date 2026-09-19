//! Process event loop: IPC + overlay + pose/health + optional wgpu.
//!
//! Hide and clean exit do not talk to Host. A broken parent pipe is a
//! disconnect, not Task cancel.

use std::io::ErrorKind;
use std::path::PathBuf;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;

use crate::error::BodyError;
use crate::ipc::{
    AssetRef, BodyToParent, HEALTH_INTERVAL, ParentToBody, ReadyInfo, decode_parent, encode_body,
};
use crate::render::Gpu;
use crate::vrm::VrmSession;
use crate::window::Overlay;

/// How the parent attached the projection channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcEndpoint {
    Stdio,
    #[cfg(unix)]
    InheritedFd(i32),
    #[cfg(unix)]
    UnixPath(PathBuf),
}

/// Run-time switches. Production tries wgpu; IPC unit tests may skip it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOptions {
    pub try_gpu: bool,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self { try_gpu: true }
    }
}

/// Parse argv. Default endpoint is stdio so tests and a dummy parent can
/// attach without a desktop supervisor.
///
/// # Errors
///
/// Unknown flags, combined endpoints, or an unusable fd/path.
pub fn parse_endpoint<I, S>(args: I) -> Result<IpcEndpoint, BodyError>
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString> + Clone,
{
    let command = clap::Command::new("ene-body")
        .about("ene VRM overlay child (no Host / Client protocol)")
        .arg(
            clap::Arg::new("ipc-stdio")
                .long("ipc-stdio")
                .action(clap::ArgAction::SetTrue)
                .help("Parent→body on stdin, body→parent on stdout"),
        );
    #[cfg(unix)]
    let command = command
        .arg(
            clap::Arg::new("ipc-fd")
                .long("ipc-fd")
                .value_name("FD")
                .help("Inherited connected Unix socket (socketpair end)"),
        )
        .arg(
            clap::Arg::new("ipc-unix")
                .long("ipc-unix")
                .value_name("PATH")
                .help("Connect to a Unix domain socket the parent is listening on"),
        );
    let matches = command
        .try_get_matches_from(args)
        .map_err(|error| BodyError::Usage(error.to_string()))?;

    #[cfg(unix)]
    {
        let stdio = matches.get_flag("ipc-stdio");
        let fd = matches.get_one::<String>("ipc-fd").cloned();
        let unix = matches.get_one::<String>("ipc-unix").cloned();
        let selected = usize::from(stdio) + usize::from(fd.is_some()) + usize::from(unix.is_some());
        if selected > 1 {
            return Err(BodyError::Usage(
                "use only one of --ipc-stdio, --ipc-fd, --ipc-unix".into(),
            ));
        }
        if let Some(raw) = fd {
            let fd: i32 = raw
                .parse()
                .map_err(|_| BodyError::Usage("ipc-fd must be an integer".into()))?;
            if fd < 3 {
                return Err(BodyError::Usage(
                    "ipc-fd must be an inherited descriptor other than 0, 1, or 2".into(),
                ));
            }
            return Ok(IpcEndpoint::InheritedFd(fd));
        }
        if let Some(path) = unix {
            if path.is_empty() {
                return Err(BodyError::Usage("ipc-unix path must not be empty".into()));
            }
            return Ok(IpcEndpoint::UnixPath(PathBuf::from(path)));
        }
        Ok(IpcEndpoint::Stdio)
    }
    #[cfg(not(unix))]
    {
        let _ = matches.get_flag("ipc-stdio");
        Ok(IpcEndpoint::Stdio)
    }
}

/// Drive the overlay process on the given endpoint until shutdown or disconnect.
///
/// # Errors
///
/// Transport or encode failures after the channel is already down are mapped
/// to [`BodyError::Transport`]. Codec errors on inbound frames are skipped
/// (unknown payloads are not interpreted) so a confused parent cannot crash
/// the body with conversation text.
pub async fn run(endpoint: IpcEndpoint, options: RunOptions) -> Result<(), BodyError> {
    match endpoint {
        IpcEndpoint::Stdio => run_with_io(tokio::io::stdin(), tokio::io::stdout(), options).await,
        #[cfg(unix)]
        IpcEndpoint::InheritedFd(fd) => {
            let stream = unix_stream_from_fd(fd)?;
            let (reader, writer) = stream.into_split();
            run_with_io(reader, writer, options).await
        }
        #[cfg(unix)]
        IpcEndpoint::UnixPath(path) => {
            let stream = tokio::net::UnixStream::connect(&path)
                .await
                .map_err(|error| BodyError::Transport(std::format!("unix connect: {error}")))?;
            let (reader, writer) = stream.into_split();
            run_with_io(reader, writer, options).await
        }
    }
}

#[cfg(unix)]
fn unix_stream_from_fd(fd: i32) -> Result<tokio::net::UnixStream, BodyError> {
    use std::os::fd::{FromRawFd, OwnedFd};

    // SAFETY: `fd` is the inherited connected Unix socket the parent passed
    // exclusively to this process (`--ipc-fd`). The CLI rejects 0/1/2 so this
    // does not alias stdio. Ownership is taken once; no other code in this
    // process retains the raw descriptor.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let std_stream = std::os::unix::net::UnixStream::from(owned);
    std_stream
        .set_nonblocking(true)
        .map_err(|error| BodyError::Transport(std::format!("ipc-fd nonblocking: {error}")))?;
    tokio::net::UnixStream::from_std(std_stream)
        .map_err(|error| BodyError::Transport(std::format!("ipc-fd tokio: {error}")))
}

/// In-process entry for tests that supply their own reader/writer.
///
/// # Errors
///
/// Same as [`run`].
pub async fn run_with_io<R, W>(reader: R, writer: W, options: RunOptions) -> Result<(), BodyError>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    match run_until_disconnect(reader, writer, options).await {
        Ok(()) | Err(BodyError::Disconnected) => Ok(()),
        Err(error) => Err(error),
    }
}

async fn run_until_disconnect<R, W>(
    mut reader: R,
    writer: W,
    options: RunOptions,
) -> Result<(), BodyError>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    let writer = Mutex::new(writer);
    let gpu = if options.try_gpu {
        Gpu::init().await
    } else {
        Gpu::skipped()
    };
    let mut overlay = Overlay::open();
    let mut vrm = VrmSession::new();

    send(
        &writer,
        &BodyToParent::Ready(ReadyInfo {
            overlay: overlay.kind(),
            gpu: gpu.status(),
            expressions: vrm.expressions(),
            spring_bone: vrm.spring_bone(),
        }),
    )
    .await?;
    if let Some(info) = gpu.fail_info() {
        send(&writer, &BodyToParent::GpuFail(info)).await?;
    }

    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let mut health = tokio::time::interval(HEALTH_INTERVAL);
    health.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut seq: u64 = 0;

    loop {
        tokio::select! {
            n = reader.read(&mut tmp) => {
                match n {
                    Ok(0) => {
                        drop(send(&writer, &BodyToParent::CleanExit).await);
                        return Ok(());
                    }
                    Ok(count) => {
                        buf.extend_from_slice(&tmp[..count]);
                        if drain_commands(
                            &mut buf,
                            &mut overlay,
                            &mut vrm,
                            &gpu,
                            &writer,
                        )
                        .await?
                        {
                            return Ok(());
                        }
                    }
                    Err(error) if peer_gone(&error) => return Err(BodyError::Disconnected),
                    Err(error) => {
                        return Err(BodyError::Transport(std::format!("ipc read: {error}")));
                    }
                }
            }
            _ = health.tick() => {
                seq = seq.saturating_add(1);
                gpu.present_tick(overlay.visible());
                if let Some(fact) = overlay.take_local_ui() {
                    send(&writer, &BodyToParent::LocalUi(fact)).await?;
                }
                send(
                    &writer,
                    &BodyToParent::HealthTick(crate::ipc::HealthTick {
                        seq,
                        visible: overlay.visible(),
                        pose: vrm.pose(),
                        gpu_ok: gpu.ok(),
                        overlay: overlay.kind(),
                        expressions: vrm.expressions(),
                        spring_bone: vrm.spring_bone(),
                    }),
                )
                .await?;
            }
        }
    }
}

async fn drain_commands<W>(
    buf: &mut Vec<u8>,
    overlay: &mut Overlay,
    vrm: &mut VrmSession,
    gpu: &Gpu,
    writer: &Mutex<W>,
) -> Result<bool, BodyError>
where
    W: AsyncWrite + Unpin,
{
    loop {
        match decode_parent(buf) {
            Ok((command, used)) => {
                buf.drain(..used);
                if handle_command(command, overlay, vrm, gpu, writer).await? {
                    return Ok(true);
                }
            }
            Err(crate::ipc::IpcError::Truncated { .. }) => return Ok(false),
            Err(_) => {
                // Unknown / corrupt body: drop the claimed frame if we can,
                // otherwise drop the prefix so we do not spin. Never decode
                // leftover bytes as conversation text.
                if buf.len() >= 4 {
                    let claimed = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
                    let need = 4usize.saturating_add(claimed);
                    if claimed > crate::ipc::MAX_FRAME_BYTES {
                        buf.drain(..4);
                    } else if buf.len() >= need {
                        buf.drain(..need);
                    } else {
                        return Ok(false);
                    }
                } else {
                    return Ok(false);
                }
            }
        }
    }
}

async fn handle_command<W>(
    command: ParentToBody,
    overlay: &mut Overlay,
    vrm: &mut VrmSession,
    gpu: &Gpu,
    writer: &Mutex<W>,
) -> Result<bool, BodyError>
where
    W: AsyncWrite + Unpin,
{
    match command {
        ParentToBody::Show => overlay.set_visible(true),
        ParentToBody::Hide => overlay.set_visible(false),
        ParentToBody::Placement(placement) => {
            if placement.is_valid() {
                overlay.set_placement(placement);
            }
        }
        ParentToBody::PoseHint(pose) => vrm.set_pose(pose),
        ParentToBody::AssetRef(asset) => apply_asset(asset, vrm, writer).await?,
        ParentToBody::Shutdown => {
            gpu.present_tick(false);
            send(writer, &BodyToParent::CleanExit).await?;
            return Ok(true);
        }
    }
    Ok(false)
}

async fn apply_asset<W>(
    asset: AssetRef,
    vrm: &mut VrmSession,
    writer: &Mutex<W>,
) -> Result<(), BodyError>
where
    W: AsyncWrite + Unpin,
{
    match vrm.set_asset(asset) {
        Ok(()) => Ok(()),
        Err(info) => send(writer, &BodyToParent::AssetFail(info)).await,
    }
}

async fn send<W>(writer: &Mutex<W>, message: &BodyToParent) -> Result<(), BodyError>
where
    W: AsyncWrite + Unpin,
{
    let bytes = encode_body(message)?;
    let mut guard = writer.lock().await;
    guard
        .write_all(&bytes)
        .await
        .map_err(|error| map_write_error("write", error))?;
    guard
        .flush()
        .await
        .map_err(|error| map_write_error("flush", error))?;
    Ok(())
}

fn map_write_error(op: &'static str, error: std::io::Error) -> BodyError {
    if peer_gone(&error) {
        BodyError::Disconnected
    } else {
        BodyError::Transport(std::format!("ipc {op}: {error}"))
    }
}

fn peer_gone(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::BrokenPipe | ErrorKind::ConnectionReset | ErrorKind::UnexpectedEof
    )
}

#[cfg(test)]
mod tests {
    use super::{IpcEndpoint, RunOptions, parse_endpoint, run_with_io};
    use crate::ipc::{
        AssetFailReason, AssetRef, BodyToParent, FeatureSupport, OverlayKind, ParentToBody,
        PoseHint, decode_body, encode_parent,
    };
    use std::io::Write as _;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn default_endpoint_is_stdio() {
        let endpoint = parse_endpoint(["ene-body"]).expect("parse");
        assert_eq!(endpoint, IpcEndpoint::Stdio);
    }

    #[test]
    fn stdio_flag_is_accepted() {
        let endpoint = parse_endpoint(["ene-body", "--ipc-stdio"]).expect("parse");
        assert_eq!(endpoint, IpcEndpoint::Stdio);
    }

    #[cfg(unix)]
    #[test]
    fn inherited_stdio_fds_are_rejected() {
        let err = parse_endpoint(["ene-body", "--ipc-fd", "1"]).expect_err("fd 1");
        let text = err.to_string();
        assert!(text.contains("ipc-fd"), "{text}");
    }

    #[cfg(unix)]
    #[test]
    fn combined_endpoints_are_rejected() {
        let err = parse_endpoint(["ene-body", "--ipc-stdio", "--ipc-unix", "/tmp/x"])
            .expect_err("combined");
        let text = err.to_string();
        assert!(text.contains("only one"), "{text}");
    }

    async fn read_event(
        reader: &mut tokio::io::DuplexStream,
        leftover: &mut Vec<u8>,
    ) -> BodyToParent {
        loop {
            if let Ok((event, used)) = decode_body(leftover) {
                leftover.drain(..used);
                return event;
            }
            let mut tmp = [0u8; 256];
            let n = reader.read(&mut tmp).await.expect("read");
            assert!(n > 0, "parent pipe closed before event");
            leftover.extend_from_slice(&tmp[..n]);
        }
    }

    #[tokio::test]
    async fn in_process_ipc_show_hide_pose_asset_and_shutdown() {
        let (mut parent, child) = tokio::io::duplex(4096);
        let (child_read, child_write) = tokio::io::split(child);
        let body = tokio::spawn(run_with_io(
            child_read,
            child_write,
            RunOptions { try_gpu: false },
        ));

        let mut leftover = Vec::new();
        let ready = read_event(&mut parent, &mut leftover).await;
        match ready {
            BodyToParent::Ready(info) => {
                assert_eq!(info.overlay, OverlayKind::Headless);
                assert_eq!(info.expressions, FeatureSupport::Unsupported);
                assert_eq!(info.spring_bone, FeatureSupport::Unsupported);
            }
            other => panic!("expected Ready, got {other:?}"),
        }
        let gpu = read_event(&mut parent, &mut leftover).await;
        assert!(matches!(gpu, BodyToParent::GpuFail(_)), "{gpu:?}");

        parent
            .write_all(&encode_parent(&ParentToBody::Show).expect("show"))
            .await
            .expect("write show");
        parent
            .write_all(&encode_parent(&ParentToBody::PoseHint(PoseHint::Listening)).expect("pose"))
            .await
            .expect("write pose");

        let mut saw_listening = false;
        let mut saw_visible = false;
        for _ in 0..12 {
            match read_event(&mut parent, &mut leftover).await {
                BodyToParent::HealthTick(tick) => {
                    if tick.visible {
                        saw_visible = true;
                    }
                    if tick.pose == PoseHint::Listening {
                        saw_listening = true;
                    }
                    assert_eq!(tick.expressions, FeatureSupport::Unsupported);
                    assert_eq!(tick.spring_bone, FeatureSupport::Unsupported);
                    if saw_visible && saw_listening {
                        break;
                    }
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(saw_visible, "show must be visible on health");
        assert!(saw_listening, "pose hint must appear on health");

        parent
            .write_all(
                &encode_parent(&ParentToBody::AssetRef(AssetRef::Path {
                    path: "/no/such/ene-body.vrm".into(),
                }))
                .expect("asset"),
            )
            .await
            .expect("write asset");

        let mut saw_asset_fail = false;
        for _ in 0..12 {
            match read_event(&mut parent, &mut leftover).await {
                BodyToParent::AssetFail(info) => {
                    assert_eq!(info.reason, AssetFailReason::Missing);
                    saw_asset_fail = true;
                    break;
                }
                BodyToParent::HealthTick(_) => {}
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(saw_asset_fail);

        let dir = tempfile::tempdir().expect("temp");
        let asset_path = dir.path().join("ref.vrm");
        std::fs::File::create(&asset_path)
            .expect("create")
            .write_all(b"placeholder")
            .expect("write");
        parent
            .write_all(
                &encode_parent(&ParentToBody::AssetRef(AssetRef::BytesTemp {
                    path: asset_path.to_string_lossy().into_owned(),
                }))
                .expect("temp ref"),
            )
            .await
            .expect("write temp ref");

        parent
            .write_all(&encode_parent(&ParentToBody::Hide).expect("hide"))
            .await
            .expect("write hide");

        let mut saw_hidden_health = false;
        for _ in 0..12 {
            match read_event(&mut parent, &mut leftover).await {
                BodyToParent::HealthTick(tick) => {
                    if !tick.visible {
                        saw_hidden_health = true;
                        break;
                    }
                }
                BodyToParent::AssetFail(_) => panic!("placeholder ref must not AssetFail"),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(
            saw_hidden_health,
            "hide keeps the process alive and still ticks"
        );

        parent
            .write_all(&encode_parent(&ParentToBody::Shutdown).expect("shutdown"))
            .await
            .expect("write shutdown");
        let mut saw_exit = false;
        for _ in 0..8 {
            match read_event(&mut parent, &mut leftover).await {
                BodyToParent::CleanExit => {
                    saw_exit = true;
                    break;
                }
                BodyToParent::HealthTick(_) => {}
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(saw_exit);
        body.await.expect("join").expect("run");
    }

    #[tokio::test]
    async fn parent_disconnect_ends_without_host_commands() {
        let (parent, child) = tokio::io::duplex(1024);
        let (child_read, child_write) = tokio::io::split(child);
        let body = tokio::spawn(run_with_io(
            child_read,
            child_write,
            RunOptions { try_gpu: false },
        ));
        drop(parent);
        body.await.expect("join").expect("clean disconnect");
    }
}
