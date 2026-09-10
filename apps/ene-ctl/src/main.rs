//! `ene-ctl` CLI client entrypoint.
//!
//! Thin client: parses `--config PATH` plus one subcommand, loads
//! [`Config`], resolves the effective data directory, dials the Host socket,
//! and renders Host-filtered answers. The client holds no canonical state
//! and establishes no local authority of its own; round identity stays
//! Host-issued, and every acceptance or outcome is Host-reported.
//!
//! Presentation output avoids the `print!` family (workspace-denied): all
//! output goes through `writeln!`/`write!` on locked stdio handles with
//! explicit flushes, so each write site states its destination and its
//! failure becomes a [`CliError::Transport`].
//!
//! Stdout contract: view and history commands print their rendered lines (or
//! nothing when empty). `send` prints `AcceptedForRound <round>`, then the
//! stream deltas concatenated as they arrive (flushed per frame), then a
//! trailing newline on [`TextStreamClose`](ene_api::v1::round::TextStreamClose),
//! and finally sends one `Presented` observation for the round (no reply is
//! expected; nothing is sent when stdio failed mid-stream). Deltas on stdout
//! are the user's own conversation text by design; error paths (stderr, exit
//! codes) never carry bodies or secrets.
//!
//! Exit codes: `0` on success; `1` for usage and technical failures
//! (transport, codec, terminal server refusals); `2` for retryable
//! server-side domain outcomes (stale rounds, held transitions, stale base
//! views, pending confirmations, and similar Ok-side declines).

use ene_ctl::errors::{CliError, USAGE};
use ene_ctl::{client, cmds};

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{BaseViewMark, CommandWireId, RoundWireId, StreamWireId};
use ene_api::v1::round::{ConfirmPresentationWire, PresentationStatus, StreamClose};
use ene_config::paths::resolve_data_dir;
use ene_config::typed::Config;

/// Parses `--config PATH` from `args` (excluding the program name).
///
/// Returns [`None`] when no arguments are given. A missing `--config` value
/// or an unknown argument is a [`CliError::Usage`] whose message ends with
/// [`USAGE`]. `--help` and `--version` are deferred to a later stage, so they
/// are reported as unknown arguments for now.
fn parse_args(args: &[String]) -> Result<Option<PathBuf>, CliError> {
    let mut config: Option<PathBuf> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--config" {
            let Some(value) = iter.next() else {
                return Err(CliError::Usage(format!(
                    "missing value for --config\n{USAGE}"
                )));
            };
            config = Some(PathBuf::from(value));
        } else {
            return Err(CliError::Usage(format!("unknown argument: {arg}\n{USAGE}")));
        }
    }
    Ok(config)
}

/// Parsed command line: global config selection plus one subcommand.
struct Cli {
    /// `--config PATH` selection, when given (must precede the subcommand).
    config: Option<PathBuf>,
    /// Subcommand with its operands.
    command: cmds::Command,
}

/// Parses the full command line: leading `--config PATH` pairs (via
/// [`parse_args`], preserving its behavior), then exactly one subcommand.
///
/// A `--config` after the subcommand word belongs to the subcommand and is
/// rejected as an unknown subcommand argument.
fn parse_cli(args: &[String]) -> Result<Cli, CliError> {
    let mut split = 0;
    while split < args.len() {
        if args[split] == "--config" {
            split += 1;
            if args.get(split).is_none() {
                return Err(CliError::Usage(format!(
                    "missing value for --config\n{USAGE}"
                )));
            }
            split += 1;
        } else {
            break;
        }
    }
    let config = parse_args(&args[..split])?;
    if args.len() == split {
        return Err(CliError::Usage(format!("missing command\n{USAGE}")));
    }
    let command = cmds::parse_command(&args[split..])?;
    Ok(Cli { config, command })
}

/// Client entrypoint: parse, load configuration, dial the Host, run.
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let code = error.exit_code();
            let mut stderr = std::io::stderr();
            if writeln!(stderr, "ene-ctl: {error}").is_err() {
                // Stderr is gone; there is nowhere left to report to.
            }
            code
        }
    }
}

/// Loads configuration, resolves the data directory, and runs the subcommand
/// on a single-threaded Tokio runtime (network-free: Unix socket only).
fn run() -> Result<(), CliError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = parse_cli(&args)?;
    let cfg = Config::load(cli.config.as_deref())?;
    let language = cfg.language.clone();
    let Some(data_dir) = resolve_data_dir(&cfg) else {
        return Err(CliError::Transport(String::from(
            "no data directory: set data_dir or the OS default",
        )));
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::Transport(format!("runtime start failed: {error}")))?;
    runtime.block_on(run_command(&data_dir, &language, cli.command))
}

/// Dials the Host and dispatches the subcommand over the session.
async fn run_command(
    data_dir: &Path,
    language: &str,
    command: cmds::Command,
) -> Result<(), CliError> {
    let platform = client::platform_display();
    let mut session = client::Client::connect(data_dir, &platform, &platform).await?;
    match command {
        cmds::Command::Setup(mode) => run_setup(&mut session, mode).await,
        cmds::Command::Status => {
            let view = request_view(&mut session, cmds::status_view_request()).await?;
            emit(&cmds::render_view(&view))
        }
        cmds::Command::Send(send) => run_send(&mut session, language, send).await,
        cmds::Command::Watch { round } => {
            // No presentation observation here: watch prints already
            // presented-or-unknown restored facts, and viewing them is not
            // presenting a stream.
            let view = request_history(&mut session, cmds::DEFAULT_HISTORY_LIMIT).await?;
            emit(&cmds::render_round_history(&view, &round))
        }
        cmds::Command::History { limit } => {
            let view = request_history(&mut session, limit).await?;
            emit(&cmds::render_history(&view))
        }
    }
}

/// Writes one rendered block plus a newline to stdout, or nothing when the
/// block is empty. Flushes explicitly so piped output is complete on return.
fn emit(text: &str) -> Result<(), CliError> {
    if text.is_empty() {
        return Ok(());
    }
    let mut stdout = std::io::stdout();
    writeln!(stdout, "{text}")
        .map_err(|error| CliError::Transport(format!("stdout write failed: {}", error.kind())))?;
    stdout
        .flush()
        .map_err(|error| CliError::Transport(format!("stdout flush failed: {}", error.kind())))?;
    Ok(())
}

/// Sends a view request and expects the filtered view back.
async fn request_view(
    session: &mut client::Client,
    request: ene_api::v1::management::ManagementViewRequest,
) -> Result<ene_api::v1::management::ManagementView, CliError> {
    match session
        .request(WirePayload::ManagementViewRequest(request))
        .await?
    {
        WirePayload::ManagementView(view) => Ok(view),
        unexpected => Err(CliError::ServerRejected(format!(
            "unexpected {} while reading a view; expected ManagementView",
            client::payload_kind(&unexpected)
        ))),
    }
}

/// Sends a timeline request and expects the filtered items back.
async fn request_history(
    session: &mut client::Client,
    limit: u64,
) -> Result<ene_api::v1::round::HistoryView, CliError> {
    let companion = session.companion_ref();
    match session
        .request(WirePayload::HistoryRequest(cmds::history_request(
            &companion, limit,
        )))
        .await?
    {
        WirePayload::HistoryView(view) => Ok(view),
        unexpected => Err(CliError::ServerRejected(format!(
            "unexpected {} while reading history; expected HistoryView",
            client::payload_kind(&unexpected)
        ))),
    }
}

/// Sends one management intent and maps its outcome: applied lines succeed,
/// retryable declines become [`CliError::ServerOutcome`] (exit 2), terminal
/// declines become [`CliError::ServerRejected`] (exit 1).
async fn apply_intent(
    session: &mut client::Client,
    intent: ene_api::v1::management::ManagementIntent,
) -> Result<String, CliError> {
    let outcome = match session
        .request(WirePayload::ManagementIntent(intent))
        .await?
    {
        WirePayload::ManagementOutcome(outcome) => outcome,
        unexpected => {
            return Err(CliError::ServerRejected(format!(
                "unexpected {} while applying an intent; expected ManagementOutcome",
                client::payload_kind(&unexpected)
            )));
        }
    };
    match cmds::describe_management(&outcome) {
        cmds::ManagementAction::Applied { detail } => Ok(detail),
        cmds::ManagementAction::Retryable { message } => Err(CliError::ServerOutcome(message)),
        cmds::ManagementAction::Terminal { message } => Err(CliError::ServerRejected(message)),
    }
}

/// Runs `setup`: `--show` renders the four Host sections; `--provider/--model`
/// fetches the setup view for its mark, registers the credential intent
/// (key sourced Host-side), then assigns provider/model over the shared
/// consent target, and reports the recorded assignment.
async fn run_setup(session: &mut client::Client, mode: cmds::SetupMode) -> Result<(), CliError> {
    match mode {
        cmds::SetupMode::Show => {
            let view = request_view(session, cmds::setup_view_request()).await?;
            emit(&cmds::render_view(&view))
        }
        cmds::SetupMode::Assign { provider, model } => {
            let view = request_view(session, cmds::setup_view_request()).await?;
            let base = BaseViewMark(view.mark.0.clone());
            let credential =
                cmds::credential_intent(CommandWireId(uuid::Uuid::new_v4()), &base, &provider);
            apply_intent(session, credential).await?;
            let assignment = cmds::assignment_intent(
                CommandWireId(uuid::Uuid::new_v4()),
                &base,
                &provider,
                &model,
            );
            apply_intent(session, assignment).await?;
            emit(&format!(
                "setup complete: provider={provider} model={model}"
            ))
        }
    }
}

/// Runs `send`: submits the candidate, prints the accepted round, streams
/// deltas as they arrive, and ends with a newline on stream close. Intake
/// declines become [`CliError::ServerOutcome`] (exit 2).
///
/// After the close frame and a successful flush of every buffered frame, the
/// client sends one presentation observation for the round (the Host applies
/// it silently and answers nothing): [`observe_close`] decides its status
/// from the close reason and whether any frame was shown, and a
/// non-completed close additionally fails with a `ServerOutcome` naming the
/// status (exit 2) after the observation is sent. Any stdio failure before
/// that point returns early and sends nothing, so the Host keeps the stream
/// `Pending`/`Unknown` instead of recording a presentation the operator
/// never saw.
async fn run_send(
    session: &mut client::Client,
    language: &str,
    send: cmds::SendArgs,
) -> Result<(), CliError> {
    let input = cmds::submit_input(
        &session.companion_ref(),
        send.round,
        send.fresh,
        send.text,
        String::from(language),
    );
    let outcome = match session.request(WirePayload::SubmitTextInput(input)).await? {
        WirePayload::RoundIntakeOutcome(outcome) => outcome,
        unexpected => {
            return Err(CliError::ServerRejected(format!(
                "unexpected {} while submitting text; expected RoundIntakeOutcome",
                client::payload_kind(&unexpected)
            )));
        }
    };
    let round = match cmds::describe_intake(&outcome) {
        cmds::IntakeAction::Accepted { round } => round,
        cmds::IntakeAction::Declined { message } => {
            return Err(CliError::ServerOutcome(message));
        }
    };
    let mut stdout = std::io::stdout();
    writeln!(stdout, "AcceptedForRound {round}")
        .map_err(|error| CliError::Transport(format!("stdout write failed: {}", error.kind())))?;
    let mut stream: Option<StreamWireId> = None;
    let mut shown = false;
    let close_status = loop {
        match session.next_frame().await? {
            WirePayload::TextStreamOpen(open) => {
                // Routing only (stream key, round, generation); the key is
                // kept for the presentation observation after close. The
                // open alone shows nothing, so it never marks `shown`.
                if stream.is_none() {
                    stream = Some(open.stream);
                }
            }
            WirePayload::TextStreamFrame(frame) => {
                if stream.is_none() {
                    stream = Some(frame.stream);
                }
                write!(stdout, "{}", frame.delta).map_err(|error| {
                    CliError::Transport(format!("stdout write failed: {}", error.kind()))
                })?;
                stdout.flush().map_err(|error| {
                    CliError::Transport(format!("stdout flush failed: {}", error.kind()))
                })?;
                shown = true;
            }
            WirePayload::TextStreamClose(close) => {
                if stream.is_none() {
                    stream = Some(close.stream);
                }
                break close.status;
            }
            WirePayload::PresenceAttribution(_) => {
                // Latest-value fact: the session already recorded its
                // generation in the frame loop; there is nothing to display.
            }
            unexpected => {
                return Err(CliError::ServerRejected(format!(
                    "unexpected {} while streaming text; expected TextStreamFrame",
                    client::payload_kind(&unexpected)
                )));
            }
        }
    };
    writeln!(stdout)
        .map_err(|error| CliError::Transport(format!("stdout write failed: {}", error.kind())))?;
    stdout
        .flush()
        .map_err(|error| CliError::Transport(format!("stdout flush failed: {}", error.kind())))?;
    let (status, success) = observe_close(close_status, shown);
    session
        .notify(WirePayload::ConfirmPresentation(ConfirmPresentationWire {
            round: RoundWireId(round),
            stream,
            status,
            detail: None,
        }))
        .await?;
    if success {
        Ok(())
    } else {
        Err(CliError::ServerOutcome(format!("stream {close_status:?}")))
    }
}

/// Maps a stream close to its presentation observation plus completion
/// success: presentation fact and completion success are separate claims.
///
/// A [`Completed`](StreamClose::Completed) stream always observes
/// [`Presented`](PresentationStatus::Presented) and succeeds. Any other close
/// still sends an observation — [`Presented`](PresentationStatus::Presented)
/// when the operator saw at least one frame (shown text stays presented even
/// though the stream did not complete), else sticky
/// [`Unknown`](PresentationStatus::Unknown) — but reports non-success so the
/// caller exits 2 with a `ServerOutcome` naming the close status. Only frames
/// shown on this live stream count: the opening frame alone shows nothing.
fn observe_close(status: StreamClose, frames_shown: bool) -> (PresentationStatus, bool) {
    match status {
        StreamClose::Completed => (PresentationStatus::Presented, true),
        StreamClose::Interrupted | StreamClose::Cancelled | StreamClose::Stale => {
            if frames_shown {
                (PresentationStatus::Presented, false)
            } else {
                (PresentationStatus::Unknown, false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for [`parse_args`](super::parse_args) and
    //! [`parse_cli`](super::parse_cli).
    //!
    //! Both parsers are pure over their input slices, so every case runs
    //! without touching the process environment.

    use std::path::{Path, PathBuf};

    use super::{CliError, USAGE, parse_args, parse_cli};

    /// Builds owned arguments from plain words.
    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    /// No arguments select no config file.
    #[test]
    fn no_args_selects_no_config_file() {
        assert!(
            matches!(parse_args(&args(&[])), Ok(None)),
            "no arguments must select no config file"
        );
    }

    /// `--config PATH` selects that file.
    #[test]
    fn config_flag_selects_a_file() {
        let path = parse_args(&args(&["--config", "/tmp/ene.json"]))
            .expect("--config with a value must succeed")
            .expect("--config must select a file");
        assert!(
            path.as_path() == Path::new("/tmp/ene.json"),
            "--config must select the given file"
        );
    }

    /// A repeated `--config` keeps the last value.
    #[test]
    fn repeated_config_flag_keeps_the_last_value() {
        let path = parse_args(&args(&[
            "--config",
            "/tmp/a.json",
            "--config",
            "/tmp/b.json",
        ]))
        .expect("a repeated --config must succeed")
        .expect("a repeated --config must select a file");
        assert!(
            path.as_path() == Path::new("/tmp/b.json"),
            "a repeated --config must keep the last value"
        );
    }

    /// A missing `--config` value reports usage.
    #[test]
    fn missing_config_value_reports_usage() {
        let super::CliError::Usage(message) = parse_args(&args(&["--config"]))
            .expect_err("a missing --config value must be a usage error")
        else {
            panic!("a missing --config value must be a usage error");
        };
        assert!(
            message.contains(USAGE),
            "a missing --config value must report the usage line: {message:?}"
        );
    }

    /// An unknown argument reports usage.
    #[test]
    fn unknown_argument_reports_usage() {
        let super::CliError::Usage(message) = parse_args(&args(&["--unknown"]))
            .expect_err("an unknown argument must be a usage error")
        else {
            panic!("an unknown argument must be a usage error");
        };
        assert!(
            message.contains(USAGE),
            "an unknown argument must report the usage line: {message:?}"
        );
    }

    /// A stray positional argument reports usage.
    #[test]
    fn positional_argument_reports_usage() {
        let super::CliError::Usage(message) =
            parse_args(&args(&["extra"])).expect_err("a positional argument must be a usage error")
        else {
            panic!("a positional argument must be a usage error");
        };
        assert!(
            message.contains(USAGE),
            "a positional argument must report the usage line: {message:?}"
        );
    }

    /// Deferred flags such as `--help` report usage for now.
    #[test]
    fn help_flag_reports_usage_while_deferred() {
        let super::CliError::Usage(message) = parse_args(&args(&["--help"]))
            .expect_err("--help must be a usage error while deferred")
        else {
            panic!("--help must be a usage error while deferred");
        };
        assert!(
            message.contains(USAGE),
            "--help must report the usage line while deferred: {message:?}"
        );
    }

    /// Asserts a [`parse_cli`] usage error ending with the usage text.
    fn assert_cli_usage(result: Result<super::Cli, CliError>, what: &str) {
        let Err(CliError::Usage(message)) = result else {
            panic!("{what} must be a usage error");
        };
        assert!(
            message.ends_with(USAGE),
            "{what} must end with the usage text: {message:?}"
        );
    }

    /// `--config` before the subcommand selects the file and the command.
    #[test]
    fn config_before_command_selects_both() {
        let parsed = parse_cli(&args(&["--config", "/tmp/ene.json", "status"]));
        assert!(parsed.is_ok(), "--config plus status must succeed");
        let cli = parsed.ok().unwrap();
        assert!(
            cli.config == Some(PathBuf::from("/tmp/ene.json")),
            "--config must select the given file"
        );
        assert!(
            cli.command == super::cmds::Command::Status,
            "the subcommand must be status, got {:?}",
            cli.command
        );
    }

    /// A missing subcommand reports usage.
    #[test]
    fn missing_command_reports_usage() {
        assert_cli_usage(parse_cli(&args(&[])), "no arguments");
        assert_cli_usage(
            parse_cli(&args(&["--config", "/tmp/ene.json"])),
            "--config without a command",
        );
    }

    /// A `--config` after the subcommand belongs to it and is rejected.
    #[test]
    fn config_after_command_reports_usage() {
        assert_cli_usage(
            parse_cli(&args(&["status", "--config", "/tmp/ene.json"])),
            "--config after the subcommand",
        );
    }

    /// A missing `--config` value reports usage at the top level too.
    #[test]
    fn top_level_missing_config_value_reports_usage() {
        assert_cli_usage(parse_cli(&args(&["--config"])), "dangling --config");
    }

    /// Full subcommand lines parse through the top level.
    #[test]
    fn full_command_lines_parse() {
        let parsed = parse_cli(&args(&["send", "hello"]));
        assert!(parsed.is_ok(), "send hello must succeed");
        let cli = parsed.ok().unwrap();
        assert!(cli.config.is_none(), "no --config must select no file");
    }

    /// Exit codes: server outcomes exit 2, everything else exits 1.
    #[test]
    fn exit_codes_split_outcome_from_failures() {
        assert!(
            CliError::ServerOutcome(String::from("stale")).exit_code()
                == std::process::ExitCode::from(2),
            "server outcomes must exit 2"
        );
        for error in [
            CliError::Usage(String::from("u")),
            CliError::Transport(String::from("t")),
            CliError::Codec(String::from("c")),
            CliError::ServerRejected(String::from("r")),
            CliError::UnsupportedPlatform("p"),
        ] {
            assert!(
                error.exit_code() == std::process::ExitCode::FAILURE,
                "usage and technical failures must exit 1, got {error:?}"
            );
        }
    }

    /// Close-status matrix: completion always presents and succeeds; any
    /// other close still observes (presented when frames were shown, unknown
    /// otherwise) but reports non-success.
    #[test]
    fn close_status_maps_presentation_and_success_separately() {
        use ene_api::v1::round::PresentationStatus;
        use ene_api::v1::round::StreamClose;

        use super::observe_close;

        for shown in [false, true] {
            assert!(
                observe_close(StreamClose::Completed, shown)
                    == (PresentationStatus::Presented, true),
                "completion always presents and succeeds, shown={shown}"
            );
        }
        for status in [
            StreamClose::Interrupted,
            StreamClose::Cancelled,
            StreamClose::Stale,
        ] {
            assert!(
                observe_close(status, true) == (PresentationStatus::Presented, false),
                "a shown-but-{status:?} stream observes presented yet fails"
            );
            assert!(
                observe_close(status, false) == (PresentationStatus::Unknown, false),
                "an unshown {status:?} stream observes unknown and fails"
            );
        }
    }
}
