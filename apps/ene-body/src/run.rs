use std::io::ErrorKind;
#[cfg(unix)]
use std::path::PathBuf;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;

use crate::error::BodyError;
use crate::ipc::{
    AssetReadyInfo, AssetRef, BodyToParent, HEALTH_INTERVAL, MotionSetInfo, ParentToBody,
    ReadyInfo, decode_parent, encode_body,
};
use crate::vrm::VrmSession;
use crate::window::Overlay;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcEndpoint {
    Stdio,
    #[cfg(unix)]
    InheritedFd(i32),
    #[cfg(unix)]
    UnixPath(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOptions {
    pub try_gpu: bool,
    pub try_native_overlay: bool,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            try_gpu: true,
            try_native_overlay: true,
        }
    }
}

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
    let mut overlay = if options.try_native_overlay {
        Overlay::open(options.try_gpu)
    } else {
        Overlay::unavailable("native overlay disabled by test options")
    };
    let mut vrm = VrmSession::new();

    send(
        &writer,
        &BodyToParent::Ready(ReadyInfo {
            overlay: overlay.kind(),
            gpu: overlay.gpu_status(),
            expressions: vrm.expressions(),
            spring_bone: vrm.spring_bone(),
        }),
    )
    .await?;
    let mut reported_gpu_failure = overlay.gpu_failure();
    if let Some(info) = reported_gpu_failure {
        send(&writer, &BodyToParent::GpuFail(info)).await?;
    }
    if let Some(info) = overlay.unavailable_info() {
        send(&writer, &BodyToParent::OverlayUnavailable(info)).await?;
    }

    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let mut health = tokio::time::interval(HEALTH_INTERVAL);
    health.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    const RUNTIME_HZ: f32 = 60.0;
    let mut runtime_tick = tokio::time::interval(std::time::Duration::from_nanos(16_666_667));
    runtime_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut render_paused_until = None;
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
                while let Some(fact) = overlay.take_local_ui() {
                    send(&writer, &BodyToParent::LocalUi(fact)).await?;
                }
                while let Some(feedback) = overlay.take_presentation() {
                    send(&writer, &BodyToParent::Presentation(feedback)).await?;
                }
                let current_gpu_failure = overlay.gpu_failure();
                if current_gpu_failure != reported_gpu_failure {
                    if let Some(info) = current_gpu_failure {
                        send(&writer, &BodyToParent::GpuFail(info)).await?;
                    }
                    reported_gpu_failure = current_gpu_failure;
                }
                send(
                    &writer,
                    &BodyToParent::HealthTick(crate::ipc::HealthTick {
                        seq,
                        visible: overlay.visible(),
                        pose: vrm.pose(),
                        gpu_ok: overlay.gpu_status() == crate::ipc::GpuInitStatus::Ok,
                        overlay: overlay.kind(),
                        expressions: vrm.expressions(),
                        spring_bone: vrm.spring_bone(),
                        motion: vrm.motion(),
                    }),
                )
                .await?;
            }
            _ = runtime_tick.tick() => {
                let now = tokio::time::Instant::now();
                let high_load_paused = render_paused_until.is_some_and(|until| now < until);
                overlay.pump();
                if overlay.ready_to_render() && !high_load_paused {
                    let started = std::time::Instant::now();
                    match vrm.update(1.0 / RUNTIME_HZ) {
                        Ok(meshes) => overlay.render(&meshes),
                        Err(info) => send(&writer, &BodyToParent::AssetFail(info)).await?,
                    }
                    if started.elapsed() >= std::time::Duration::from_millis(100) {
                        render_paused_until = Some(now + std::time::Duration::from_secs(1));
                    } else {
                        render_paused_until = None;
                    }
                }
                while let Some(feedback) = overlay.take_presentation() {
                    send(&writer, &BodyToParent::Presentation(feedback)).await?;
                }
            }
        }
    }
}

async fn drain_commands<W>(
    buf: &mut Vec<u8>,
    overlay: &mut Overlay,
    vrm: &mut VrmSession,
    writer: &Mutex<W>,
) -> Result<bool, BodyError>
where
    W: AsyncWrite + Unpin,
{
    loop {
        match decode_parent(buf) {
            Ok((command, used)) => {
                buf.drain(..used);
                if handle_command(command, overlay, vrm, writer).await? {
                    return Ok(true);
                }
            }
            Err(crate::ipc::IpcError::Truncated { .. }) => return Ok(false),
            Err(_) => {
                // Unknown / corrupt body: drop the framed bytes whole if the
                // length is readable, otherwise drop the prefix so we do not
                // spin. Never decode leftover bytes as conversation text.
                match crate::ipc::frame_len(buf) {
                    Ok(need) => {
                        buf.drain(..need);
                    }
                    Err(crate::ipc::IpcError::Truncated { .. }) => return Ok(false),
                    Err(_) => {
                        buf.drain(..4);
                    }
                }
            }
        }
    }
}

async fn handle_command<W>(
    command: ParentToBody,
    overlay: &mut Overlay,
    vrm: &mut VrmSession,
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
        ParentToBody::MotionSet(set) => apply_motions(&set, vrm, writer).await?,
        ParentToBody::Shutdown => {
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
        Ok(()) => {
            let stats = vrm.stats().ok_or_else(|| {
                BodyError::Runtime(String::from("loaded VRM has no retained statistics"))
            })?;
            send(
                writer,
                &BodyToParent::AssetReady(AssetReadyInfo {
                    primitives: stats.primitives,
                    expressions: stats.expressions,
                    spring_chains: stats.spring_chains,
                }),
            )
            .await
        }
        Err(info) => send(writer, &BodyToParent::AssetFail(info)).await,
    }
}

async fn apply_motions<W>(
    set: &MotionSetInfo,
    vrm: &mut VrmSession,
    writer: &Mutex<W>,
) -> Result<(), BodyError>
where
    W: AsyncWrite + Unpin,
{
    match vrm.set_motions(set) {
        Ok(()) => Ok(()),
        Err(info) => send(writer, &BodyToParent::MotionFail(info)).await,
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
