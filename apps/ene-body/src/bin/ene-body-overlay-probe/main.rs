use std::io::Write as _;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::process::ExitCode;

use ene_body::ipc::{
    AssetRef, BodyToParent, ParentToBody, PlacementBox, PoseHint, decode_body, encode_parent,
};
use ene_body::{RunOptions, run_with_io};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};

#[cfg(target_os = "linux")]
mod underlay;

#[derive(Debug, thiserror::Error)]
enum ProbeError {
    #[error("usage: ene-body-overlay-probe PATH.vrm")]
    Usage,
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Ipc(#[from] ene_body::ipc::IpcError),
    #[error("{0}")]
    Body(#[from] ene_body::BodyError),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), ProbeError> {
    let asset = std::env::args().nth(1).ok_or(ProbeError::Usage)?;
    let mut options = RunOptions::default();
    if std::env::var_os("ENE_BODY_SKIP_GPU").is_some() {
        options.try_gpu = false;
    }
    if std::env::var_os("ENE_BODY_HEADLESS").is_some() {
        options.try_native_overlay = false;
    }
    let placement = initial_placement()?;

    let (mut to_body, body_reader) = tokio::io::duplex(64 * 1024);
    let (body_writer, mut from_body) = tokio::io::duplex(64 * 1024);
    let body_future = run_with_io(body_reader, body_writer, options);
    tokio::pin!(body_future);

    #[cfg(target_os = "linux")]
    if let Some(path) = std::env::var_os("ENE_PROBE_UNDERLAY_JSONL").map(PathBuf::from) {
        underlay::spawn(path);
    }

    send(
        &mut to_body,
        &ParentToBody::AssetRef(AssetRef::Path { path: asset }),
    )
    .await?;
    send(&mut to_body, &ParentToBody::Placement(placement)).await?;
    send(&mut to_body, &ParentToBody::PoseHint(PoseHint::Idle)).await?;
    send(&mut to_body, &ParentToBody::Show).await?;

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut frame = Vec::new();
    let mut chunk = [0_u8; 4096];
    let mut body_done = false;
    loop {
        tokio::select! {
            line = lines.next_line() => {
                match line? {
                    None => break,
                    Some(line) => {
                        if !command(&line, &mut to_body).await? {
                            break;
                        }
                    }
                }
            }
            read = from_body.read(&mut chunk) => {
                let read = read?;
                if read == 0 {
                    eprintln!("probe: body closed its event stream");
                    break;
                }
                frame.extend_from_slice(&chunk[..read]);
                while let Ok((event, used)) = decode_body(&frame) {
                    frame.drain(..used);
                    emit(&event)?;
                    if matches!(event, BodyToParent::CleanExit) {
                        frame.clear();
                    }
                }
            }
            result = &mut body_future => {
                body_done = true;
                if let Err(error) = result {
                    return Err(ProbeError::Body(error));
                }
                eprintln!("probe: body exited");
                break;
            }
        }
    }
    if body_done {
        drop(to_body);
        return Ok(());
    }
    if let Err(error) = send(&mut to_body, &ParentToBody::Shutdown).await {
        eprintln!("probe: shutdown send failed: {error}");
    }
    drop(to_body);
    match tokio::time::timeout(std::time::Duration::from_secs(5), &mut body_future).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(ProbeError::Body(error)),
        Err(_) => eprintln!("probe: body did not exit within 5s"),
    }
    Ok(())
}

fn initial_placement() -> Result<PlacementBox, ProbeError> {
    let Some(raw) = std::env::var_os("ENE_PROBE_PLACEMENT") else {
        return Ok(PlacementBox {
            x: 24,
            y: 24,
            width: 420,
            height: 640,
            scale: 1.0,
        });
    };
    let raw = raw.to_string_lossy();
    let parts = raw.split(',').collect::<Vec<_>>();
    if parts.len() != 5 {
        return Err(ProbeError::Usage);
    }
    let x = parts[0].trim().parse().map_err(|_| ProbeError::Usage)?;
    let y = parts[1].trim().parse().map_err(|_| ProbeError::Usage)?;
    let width = parts[2].trim().parse().map_err(|_| ProbeError::Usage)?;
    let height = parts[3].trim().parse().map_err(|_| ProbeError::Usage)?;
    let scale = parts[4].trim().parse().map_err(|_| ProbeError::Usage)?;
    Ok(PlacementBox {
        x,
        y,
        width,
        height,
        scale,
    })
}

async fn send(
    writer: &mut tokio::io::DuplexStream,
    message: &ParentToBody,
) -> Result<(), ProbeError> {
    let bytes = encode_parent(message)?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

async fn command(line: &str, writer: &mut tokio::io::DuplexStream) -> Result<bool, ProbeError> {
    let mut parts = line.split_whitespace();
    let Some(head) = parts.next() else {
        return Ok(true);
    };
    let message = match head {
        "quit" | "shutdown" => return Ok(false),
        "show" => Some(ParentToBody::Show),
        "hide" => Some(ParentToBody::Hide),
        "pose" => match parts.next() {
            Some("idle") => Some(ParentToBody::PoseHint(PoseHint::Idle)),
            Some("listening") => Some(ParentToBody::PoseHint(PoseHint::Listening)),
            Some("speaking") => Some(ParentToBody::PoseHint(PoseHint::Speaking)),
            Some("working") => Some(ParentToBody::PoseHint(PoseHint::Working)),
            Some("attention") => Some(ParentToBody::PoseHint(PoseHint::Attention)),
            _ => {
                eprintln!("probe: pose requires idle|listening|speaking|working|attention");
                None
            }
        },
        "placement" => {
            let values = parts.collect::<Vec<_>>();
            if values.len() == 5 {
                match (
                    values[0].parse::<i32>(),
                    values[1].parse::<i32>(),
                    values[2].parse::<u32>(),
                    values[3].parse::<u32>(),
                    values[4].parse::<f32>(),
                ) {
                    (Ok(x), Ok(y), Ok(width), Ok(height), Ok(scale)) => {
                        Some(ParentToBody::Placement(PlacementBox {
                            x,
                            y,
                            width,
                            height,
                            scale,
                        }))
                    }
                    _ => {
                        eprintln!("probe: placement takes X Y W H SCALE");
                        None
                    }
                }
            } else {
                eprintln!("probe: placement takes X Y W H SCALE");
                None
            }
        }
        "asset" => Some(ParentToBody::AssetRef(AssetRef::Path {
            path: parts.collect::<Vec<_>>().join(" "),
        })),
        "motions" => {
            let dir = parts.collect::<Vec<_>>().join(" ");
            let clips = ene_body::motion::pose_clips_in(std::path::Path::new(&dir));
            if clips.is_empty() {
                eprintln!("probe: no mapped motion clips found in {dir}");
                None
            } else {
                Some(ParentToBody::MotionSet(ene_body::ipc::MotionSetInfo {
                    clips,
                }))
            }
        }
        "help" => {
            eprintln!(
                "probe commands: show | hide | pose NAME | placement X Y W H SCALE | asset PATH | motions DIR | quit"
            );
            None
        }
        other => {
            eprintln!("probe: unknown command {other}");
            None
        }
    };
    if let Some(message) = message {
        send(writer, &message).await?;
    }
    Ok(true)
}

fn emit(event: &BodyToParent) -> Result<(), ProbeError> {
    let observed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let line = serde_json::json!({ "observed_unix_ms": observed, "event": event });
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, &line)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}
