use std::io::{BufRead as _, Seek as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use ene_desktop::measure::{
    InteractionTraceLine, MeasurementError, PresentationRecord, ProcessRole, ProcessTarget,
    WaylandFeedbackTraceLine, import_presentmon_csv, sample_idle, wayland_presentation_record,
};

#[derive(Debug)]
struct Args {
    targets: Vec<ProcessTarget>,
    sha: String,
    kernel: String,
    duration: Duration,
    interval: Duration,
    presentation: Option<PathBuf>,
    wayland_feedback: Option<PathBuf>,
    presentmon_csv: Option<PathBuf>,
    presentmon_exe: Option<PathBuf>,
    swap_chain: Option<String>,
    fps_warmup_secs: f64,
    fps_wall_secs: f64,
    interactions: Option<PathBuf>,
    interaction_trace: Option<PathBuf>,
    click_through: Option<PathBuf>,
    environment: Option<PathBuf>,
    output_json: PathBuf,
    output_report: PathBuf,
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(
        "usage: ene-measure --host PID --desktop PID --body PID [--other NAME:PID]... --sha SHA --kernel BUILD [--duration-secs 300] [--interval-ms 1000] [--wayland-feedback-jsonl PATH | --presentmon-csv PATH --swap-chain ID [--presentmon-exe PATH] | --presentation-json PATH] [--fps-warmup-secs 5] [--fps-wall-secs 10] [--interaction-json PATH | --interaction-jsonl PATH] [--click-through-json PATH] [--environment-json PATH] --output-json PATH --output-report PATH"
    )]
    Usage,
    #[error("invalid {name}: {value}")]
    InvalidValue { name: &'static str, value: String },
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to decode {path}: {reason}")]
    Decode { path: PathBuf, reason: String },
    #[error(transparent)]
    Measurement(#[from] MeasurementError),
    #[error("failed to start PresentMon {path}: {source}")]
    PresentMonStart {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to reap PresentMon {path}: {source}")]
    PresentMonWait {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("PresentMon exited unsuccessfully: {0}")]
    PresentMonExit(std::process::ExitStatus),
}

fn main() -> ExitCode {
    match run() {
        Ok(pass) => {
            if pass {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(2)
            }
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<bool, CliError> {
    let args = parse_args(std::env::args().skip(1))?;
    let body_pid = args
        .targets
        .iter()
        .find(|target| target.role == ProcessRole::Body)
        .map(|target| target.pid)
        .ok_or(CliError::Usage)?;
    let wayland_offset = args
        .wayland_feedback
        .as_deref()
        .map(trace_offset)
        .transpose()?;
    let interaction_offset = args
        .interaction_trace
        .as_deref()
        .map(trace_offset)
        .transpose()?;
    let mut presentmon = start_presentmon(&args, body_pid)?;
    let sampled = sample_idle(
        &args.targets,
        args.duration,
        args.interval,
        args.sha,
        args.kernel,
    );
    if sampled.is_err()
        && let Some(child) = &mut presentmon
    {
        drop(child.kill());
        drop(child.wait());
    }
    let mut record = sampled?;
    if let Some(child) = &mut presentmon {
        let status = child.wait().map_err(|source| CliError::PresentMonWait {
            path: args
                .presentmon_exe
                .clone()
                .unwrap_or_else(|| PathBuf::from("PresentMon")),
            source,
        })?;
        if !status.success() {
            return Err(CliError::PresentMonExit(status));
        }
    }
    if let Some(path) = args.presentation {
        let supplied: PresentationRecord = read_json(&path)?;
        record.fps = Some(PresentationRecord::from_events(
            supplied.source,
            supplied.warmup_secs,
            supplied.wall_secs,
            supplied.events,
        )?);
    } else if let Some(path) = args.presentmon_csv {
        let fps = import_presentmon_csv(
            &path,
            body_pid,
            args.swap_chain.as_deref().ok_or(CliError::Usage)?,
            args.fps_warmup_secs,
            args.fps_wall_secs,
        )?;
        if !fps.events.is_empty() {
            record.fps = Some(fps);
        }
    } else if let Some(path) = args.wayland_feedback {
        let feedback = read_wayland_feedback(
            &path,
            wayland_offset.unwrap_or(0),
            record.started_unix_ms,
            args.fps_warmup_secs,
            args.fps_wall_secs,
        )?;
        if !feedback.is_empty() {
            match wayland_presentation_record(
                body_pid,
                args.fps_warmup_secs,
                args.fps_wall_secs,
                feedback,
            ) {
                Ok(fps) => record.fps = Some(fps),
                Err(MeasurementError::PresentationTrace(_))
                | Err(MeasurementError::DuplicatePresentation(_)) => {}
                Err(other) => return Err(other.into()),
            }
        }
    }
    if let Some(path) = args.interactions {
        record.interactions = read_json(&path)?;
    } else if let Some(path) = args.interaction_trace {
        record.interactions = read_interaction_trace(&path, interaction_offset.unwrap_or(0))?;
    }
    if let Some(path) = args.click_through {
        record.click_through = Some(read_json(&path)?);
    }
    if let Some(path) = args.environment {
        record.environment = Some(read_json(&path)?);
    }
    record.evaluate();
    record.write_outputs(&args.output_json, &args.output_report)?;
    print!("{}", record.human_report());
    Ok(record.claims_pass())
}

fn start_presentmon(args: &Args, body_pid: u32) -> Result<Option<std::process::Child>, CliError> {
    let Some(executable) = &args.presentmon_exe else {
        return Ok(None);
    };
    if !cfg!(target_os = "windows") {
        return Err(CliError::InvalidValue {
            name: "presentmon-exe",
            value: String::from("PresentMon capture is Windows-only"),
        });
    }
    let output = args.presentmon_csv.as_ref().ok_or(CliError::Usage)?;
    let child = std::process::Command::new(executable)
        .arg("--process_id")
        .arg(body_pid.to_string())
        .arg("--output_file")
        .arg(output)
        .arg("--v2_metrics")
        .arg("--timed")
        .arg(args.duration.as_secs().to_string())
        .arg("--terminate_after_timed")
        .arg("--no_console_stats")
        .spawn()
        .map_err(|source| CliError::PresentMonStart {
            path: executable.clone(),
            source,
        })?;
    Ok(Some(child))
}

fn open_trace(
    path: &Path,
    offset: u64,
) -> Result<Option<std::io::BufReader<std::fs::File>>, CliError> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(CliError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    file.seek(std::io::SeekFrom::Start(offset))
        .map_err(|source| CliError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(Some(std::io::BufReader::new(file)))
}

fn read_interaction_trace(
    path: &Path,
    offset: u64,
) -> Result<Vec<ene_desktop::measure::InteractionSample>, CliError> {
    let Some(reader) = open_trace(path, offset)? else {
        return Ok(Vec::new());
    };
    reader
        .lines()
        .map(|line| {
            let line = line.map_err(|source| CliError::Read {
                path: path.to_path_buf(),
                source,
            })?;
            let trace: InteractionTraceLine =
                serde_json::from_str(&line).map_err(|error| CliError::Decode {
                    path: path.to_path_buf(),
                    reason: error.to_string(),
                })?;
            Ok(trace.sample)
        })
        .collect()
}

fn trace_offset(path: &Path) -> Result<u64, CliError> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(source) => Err(CliError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn read_wayland_feedback(
    path: &Path,
    offset: u64,
    started_unix_ms: u128,
    warmup_secs: f64,
    wall_secs: f64,
) -> Result<Vec<ene_body::ipc::PresentationFeedback>, CliError> {
    let Some(reader) = open_trace(path, offset)? else {
        return Ok(Vec::new());
    };
    let window_start = started_unix_ms
        .saturating_mul(1_000_000)
        .saturating_add((warmup_secs * 1_000_000_000.0).round() as u128);
    let window_end = window_start.saturating_add((wall_secs * 1_000_000_000.0).round() as u128);
    let mut selected = Vec::new();
    let mut submitted = std::collections::BTreeSet::new();
    for line in reader.lines() {
        let line = line.map_err(|source| CliError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let trace: WaylandFeedbackTraceLine =
            serde_json::from_str(&line).map_err(|error| CliError::Decode {
                path: path.to_path_buf(),
                reason: error.to_string(),
            })?;
        let observed = u128::from(trace.observed_unix_ns);
        match &trace.feedback.outcome {
            ene_body::ipc::PresentationOutcome::Submitted
                if observed >= window_start && observed < window_end =>
            {
                submitted.insert(trace.feedback.correlation_id);
                selected.push(trace.feedback);
            }
            ene_body::ipc::PresentationOutcome::Submitted => {}
            _ if submitted.contains(&trace.feedback.correlation_id) => {
                selected.push(trace.feedback);
            }
            _ => {}
        }
    }
    Ok(selected)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, CliError> {
    let bytes = std::fs::read(path).map_err(|source| CliError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|error| CliError::Decode {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Args, CliError> {
    let mut args = args;
    let mut targets = Vec::new();
    let mut sha = None;
    let mut kernel = None;
    let mut duration = Duration::from_secs(300);
    let mut interval = Duration::from_secs(1);
    let mut presentation = None;
    let mut wayland_feedback = None;
    let mut presentmon_csv = None;
    let mut presentmon_exe = None;
    let mut swap_chain = None;
    let mut fps_warmup_secs = 5.0;
    let mut fps_wall_secs = 10.0;
    let mut interactions = None;
    let mut interaction_trace = None;
    let mut click_through = None;
    let mut environment = None;
    let mut output_json = None;
    let mut output_report = None;
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(CliError::Usage)?;
        match flag.as_str() {
            "--host" => targets.push(target(ProcessRole::Host, "ene-core", &value)?),
            "--desktop" => {
                targets.push(target(ProcessRole::Desktop, "ene-desktop", &value)?);
            }
            "--body" => targets.push(target(ProcessRole::Body, "ene-body", &value)?),
            "--other" => {
                let (name, pid) = value
                    .split_once(':')
                    .ok_or_else(|| CliError::InvalidValue {
                        name: "other NAME:PID",
                        value: value.clone(),
                    })?;
                targets.push(target(ProcessRole::Other, name, pid)?);
            }
            "--sha" => sha = Some(value),
            "--kernel" => kernel = Some(value),
            "--duration-secs" => {
                duration = Duration::from_secs(parse_u64("duration-secs", &value)?);
            }
            "--interval-ms" => {
                interval = Duration::from_millis(parse_u64("interval-ms", &value)?);
            }
            "--presentation-json" => presentation = Some(PathBuf::from(value)),
            "--wayland-feedback-jsonl" => wayland_feedback = Some(PathBuf::from(value)),
            "--presentmon-csv" => presentmon_csv = Some(PathBuf::from(value)),
            "--presentmon-exe" => presentmon_exe = Some(PathBuf::from(value)),
            "--swap-chain" => swap_chain = Some(value),
            "--fps-warmup-secs" => {
                fps_warmup_secs = parse_f64("fps-warmup-secs", &value)?;
            }
            "--fps-wall-secs" => fps_wall_secs = parse_f64("fps-wall-secs", &value)?,
            "--interaction-json" => interactions = Some(PathBuf::from(value)),
            "--interaction-jsonl" => interaction_trace = Some(PathBuf::from(value)),
            "--click-through-json" => click_through = Some(PathBuf::from(value)),
            "--environment-json" => environment = Some(PathBuf::from(value)),
            "--output-json" => output_json = Some(PathBuf::from(value)),
            "--output-report" => output_report = Some(PathBuf::from(value)),
            _ => return Err(CliError::Usage),
        }
    }
    for role in [ProcessRole::Host, ProcessRole::Desktop, ProcessRole::Body] {
        if targets.iter().filter(|target| target.role == role).count() != 1 {
            return Err(CliError::Usage);
        }
    }
    let unique_pids = targets
        .iter()
        .map(|target| target.pid)
        .collect::<std::collections::BTreeSet<_>>();
    let presentation_sources = usize::from(presentation.is_some())
        + usize::from(presentmon_csv.is_some())
        + usize::from(wayland_feedback.is_some());
    if unique_pids.len() != targets.len()
        || presentation_sources > 1
        || presentmon_csv.is_some() != swap_chain.is_some()
        || (presentmon_exe.is_some() && presentmon_csv.is_none())
        || (interactions.is_some() && interaction_trace.is_some())
        || !fps_warmup_secs.is_finite()
        || fps_warmup_secs < 0.0
        || !fps_wall_secs.is_finite()
        || fps_wall_secs <= 0.0
        || duration.as_secs_f64() < fps_warmup_secs + fps_wall_secs
    {
        return Err(CliError::Usage);
    }
    Ok(Args {
        targets,
        sha: sha.ok_or(CliError::Usage)?,
        kernel: kernel.ok_or(CliError::Usage)?,
        duration,
        interval,
        presentation,
        wayland_feedback,
        presentmon_csv,
        presentmon_exe,
        swap_chain,
        fps_warmup_secs,
        fps_wall_secs,
        interactions,
        interaction_trace,
        click_through,
        environment,
        output_json: output_json.ok_or(CliError::Usage)?,
        output_report: output_report.ok_or(CliError::Usage)?,
    })
}

fn target(role: ProcessRole, name: &str, value: &str) -> Result<ProcessTarget, CliError> {
    let pid = value.parse().map_err(|_| CliError::InvalidValue {
        name: "PID",
        value: value.to_string(),
    })?;
    Ok(ProcessTarget {
        role,
        name: name.to_string(),
        pid,
    })
}

fn parse_u64(name: &'static str, value: &str) -> Result<u64, CliError> {
    value.parse().map_err(|_| CliError::InvalidValue {
        name,
        value: value.to_string(),
    })
}

fn parse_f64(name: &'static str, value: &str) -> Result<f64, CliError> {
    value.parse().map_err(|_| CliError::InvalidValue {
        name,
        value: value.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{parse_args, read_wayland_feedback};

    use ene_body::ipc::{PresentationFeedback, PresentationOutcome};
    use ene_desktop::measure::{WaylandFeedbackTraceLine, wayland_presentation_record};

    fn write_trace(lines: &[WaylandFeedbackTraceLine]) -> tempfile::NamedTempFile {
        use std::io::Write as _;
        let mut file = tempfile::NamedTempFile::new().expect("temp trace");
        for line in lines {
            let encoded = serde_json::to_string(line).expect("encode");
            file.write_all(encoded.as_bytes()).expect("write");
            file.write_all(b"\n").expect("newline");
        }
        file.flush().expect("flush");
        file
    }

    fn line(
        observed_unix_ns: u64,
        correlation_id: u64,
        outcome: PresentationOutcome,
    ) -> WaylandFeedbackTraceLine {
        WaylandFeedbackTraceLine {
            observed_unix_ns,
            feedback: PresentationFeedback {
                surface_id: String::from("wl_surface@1"),
                correlation_id,
                outcome,
            },
        }
    }

    #[test]
    fn mandatory_processes_and_outputs_parse() {
        let args = parse_args(
            [
                "--host",
                "1",
                "--desktop",
                "2",
                "--body",
                "3",
                "--sha",
                "abc",
                "--kernel",
                "build",
                "--output-json",
                "out.json",
                "--output-report",
                "out.txt",
            ]
            .into_iter()
            .map(String::from),
        )
        .expect("args");
        assert_eq!(args.targets.len(), 3);
        assert_eq!(args.duration.as_secs(), 300);
    }

    #[test]
    fn terminal_feedback_after_the_window_end_still_counts() {
        let trace = write_trace(&[
            line(5_000_000_000, 7, PresentationOutcome::Submitted),
            line(
                12_000_000_000,
                7,
                PresentationOutcome::Presented {
                    timestamp_ns: 9_000_000_000,
                    clock_id: 1,
                    output: String::from("DP-1"),
                },
            ),
        ]);
        let feedback =
            read_wayland_feedback(trace.path(), 0, 1_000, 0.0, 10.0).expect("read trace");
        let record = wayland_presentation_record(1, 0.0, 10.0, feedback).expect("record");
        assert_eq!(record.presented, 1);
        assert_eq!(record.missing, 0);
        assert_eq!(record.discarded, 0);
    }

    #[test]
    fn unresolved_submission_inside_the_window_is_missing() {
        let trace = write_trace(&[line(5_000_000_000, 9, PresentationOutcome::Submitted)]);
        let feedback =
            read_wayland_feedback(trace.path(), 0, 1_000, 0.0, 10.0).expect("read trace");
        let record = wayland_presentation_record(1, 0.0, 10.0, feedback).expect("record");
        assert_eq!(record.presented, 0);
        assert_eq!(record.missing, 1);
    }

    #[test]
    fn submissions_outside_the_window_are_ignored() {
        let trace = write_trace(&[
            line(500_000_000, 1, PresentationOutcome::Submitted),
            line(20_000_000_000, 2, PresentationOutcome::Submitted),
        ]);
        let feedback =
            read_wayland_feedback(trace.path(), 0, 1_000, 0.0, 10.0).expect("read trace");
        assert!(feedback.is_empty());
    }
}
