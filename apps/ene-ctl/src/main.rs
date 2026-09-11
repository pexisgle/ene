//! `ene-ctl` CLI client entrypoint.
//!
//! The client holds no canonical state and establishes no local authority of
//! its own; round identity stays Host-issued, and every acceptance or outcome
//! is Host-reported.
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

use ene_ctl::errors::CliError;
use ene_ctl::{client, cmds};

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{BaseViewMark, CommandWireId, RoundWireId, StreamWireId};
use ene_api::v1::round::{ConfirmPresentationWire, PresentationStatus, StreamClose};
use ene_config::paths::resolve_data_dir;
use ene_config::typed::Config;

/// The declarative CLI surface: argv syntax, subcommands, flags, help, and
/// version all come from `clap`. Domain judgment (provider allowlist, target
/// grammar, conflicts, required text) stays in this binary's validation and
/// in the Host; the library only maps the parsed words onto `cmds` types.
fn ene_ctl_command() -> clap::Command {
    use clap::{Arg, ArgAction};

    clap::Command::new("ene-ctl")
        .version(env!("CARGO_PKG_VERSION"))
        .about("ene client control surface")
        .subcommand_required(true)
        .arg(
            Arg::new("config")
                .long("config")
                .value_name("PATH")
                .global(true)
                .overrides_with("config")
                .help("Configuration file path"),
        )
        .subcommand(
            clap::Command::new("setup")
                .about("Show the setup view or assign a provider/model route")
                .arg(Arg::new("show").long("show").action(ArgAction::SetTrue))
                .arg(
                    Arg::new("learning")
                        .long("learning")
                        .action(ArgAction::SetTrue),
                )
                .arg(Arg::new("provider").long("provider").value_name("P"))
                .arg(Arg::new("model").long("model").value_name("M")),
        )
        .subcommand(clap::Command::new("status").about("Show the current setup view"))
        .subcommand(
            clap::Command::new("send")
                .about("Send one text input; `--` ends option parsing")
                .arg(Arg::new("new").long("new").action(ArgAction::SetTrue))
                .arg(Arg::new("round").long("round").value_name("ROUND"))
                .arg(
                    Arg::new("text")
                        .value_name("TEXT")
                        .num_args(1..)
                        .trailing_var_arg(true),
                ),
        )
        .subcommand(
            clap::Command::new("watch")
                .about("Print one round's restored History items")
                .arg(
                    Arg::new("round")
                        .long("round")
                        .value_name("ROUND")
                        .required(true),
                ),
        )
        .subcommand(
            clap::Command::new("history")
                .about("Print the restored Conversation History")
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .value_name("N")
                        .value_parser(clap::value_parser!(u64))
                        .default_value("50"),
                ),
        )
        .subcommand(
            clap::Command::new("memory")
                .about("Read the current Memory list or one Memory's revisions")
                .arg(Arg::new("after").long("after").value_name("ID"))
                .arg(Arg::new("revisions").long("revisions").value_name("ID"))
                .arg(
                    Arg::new("after-revision")
                        .long("after-revision")
                        .value_name("N")
                        .value_parser(clap::value_parser!(u64)),
                ),
        )
}

struct Cli {
    /// `--config PATH`, accepted before or after the subcommand.
    config: Option<PathBuf>,
    command: cmds::Command,
}

fn usage_error(message: impl Into<String>) -> CliError {
    CliError::Usage(format!(
        "{}
run `ene-ctl --help`",
        message.into()
    ))
}

fn cli_from_matches(matches: clap::ArgMatches) -> Result<Cli, CliError> {
    let config = matches.get_one::<String>("config").map(PathBuf::from);
    let Some((name, sub)) = matches.subcommand() else {
        return Err(usage_error("missing command"));
    };
    // A global `--config` after the subcommand lands on the subcommand's
    // matches; either placement selects the same file.
    let config = config.or_else(|| sub.get_one::<String>("config").map(PathBuf::from));
    let command = match name {
        "setup" => cmds::Command::Setup(setup_mode(sub)?),
        "status" => cmds::Command::Status,
        "send" => cmds::Command::Send(send_args(sub)?),
        "watch" => cmds::Command::Watch {
            round: sub
                .get_one::<String>("round")
                .cloned()
                .ok_or_else(|| usage_error("watch requires --round ROUND"))?,
        },
        "history" => cmds::Command::History {
            limit: *sub
                .get_one::<u64>("limit")
                .ok_or_else(|| usage_error("history limit has no default"))?,
        },
        "memory" => {
            let after = sub.get_one::<String>("after").cloned();
            let revisions = sub.get_one::<String>("revisions").cloned();
            let after_revision = sub.get_one::<u64>("after-revision").copied();
            if after.is_some() && revisions.is_some() {
                return Err(usage_error(
                    "--after and --revisions select different pages",
                ));
            }
            if after_revision.is_some() && revisions.is_none() {
                return Err(usage_error("--after-revision requires --revisions ID"));
            }
            cmds::Command::Memory {
                after,
                revisions,
                after_revision,
            }
        }
        other => return Err(usage_error(format!("unknown command: {other}"))),
    };
    Ok(Cli { config, command })
}

fn setup_mode(sub: &clap::ArgMatches) -> Result<cmds::SetupMode, CliError> {
    let show = sub.get_flag("show");
    let learning = sub.get_flag("learning");
    let provider = sub.get_one::<String>("provider");
    let model = sub.get_one::<String>("model");
    if show {
        if provider.is_some() || model.is_some() || learning {
            return Err(usage_error("setup --show takes no other flags"));
        }
        return Ok(cmds::SetupMode::Show);
    }
    match (provider, model) {
        (Some(provider), Some(model)) => {
            if provider != cmds::SETUP_PROVIDER_OPENAI {
                return Err(usage_error(format!(
                    "unsupported provider: {provider} (only {})",
                    cmds::SETUP_PROVIDER_OPENAI
                )));
            }
            if model.is_empty() {
                return Err(usage_error("model must not be empty"));
            }
            Ok(cmds::SetupMode::Assign {
                provider: provider.clone(),
                model: model.clone(),
                learning,
            })
        }
        (None, None) => Err(usage_error(
            "setup requires --show or --provider openai --model MODEL",
        )),
        _ => Err(usage_error("--provider and --model must be given together")),
    }
}

fn send_args(sub: &clap::ArgMatches) -> Result<cmds::SendArgs, CliError> {
    let fresh = sub.get_flag("new");
    let round = sub.get_one::<String>("round").cloned();
    let words: Vec<&str> = sub
        .get_many::<String>("text")
        .map(|values| values.map(String::as_str).collect())
        .unwrap_or_default();
    if fresh && round.is_some() {
        return Err(usage_error("--new and --round must not be combined"));
    }
    if words.is_empty() {
        return Err(usage_error("send requires message text"));
    }
    Ok(cmds::SendArgs {
        round,
        fresh,
        text: words.join(" "),
    })
}

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

/// Runs on a single-threaded Tokio runtime; the client is Unix-socket only
/// (no network).
fn run() -> Result<(), CliError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let matches = match ene_ctl_command()
        .try_get_matches_from(std::iter::once(String::from("ene-ctl")).chain(args))
    {
        Ok(matches) => matches,
        // `--help` / `--version` are standard successful exits, never errors
        // and never reach configuration or the Host.
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            error.print().map_err(|err| {
                CliError::Transport(format!("stdout write failed: {}", err.kind()))
            })?;
            return Ok(());
        }
        Err(error) => return Err(CliError::Usage(error.to_string())),
    };
    let cli = cli_from_matches(matches)?;
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
            let view = request_view(&mut session, cmds::setup_view_request()).await?;
            emit(&cmds::render_view(&view))
        }
        cmds::Command::Send(send) => run_send(&mut session, language, send).await,
        cmds::Command::Watch { round } => {
            let items =
                request_history(&mut session, Some(&round), cmds::DEFAULT_HISTORY_LIMIT).await?;
            emit(&cmds::render_history(&items))
        }
        cmds::Command::History { limit } => {
            let items = request_history(&mut session, None, limit).await?;
            emit(&cmds::render_history(&items))
        }
        cmds::Command::Memory {
            after,
            revisions,
            after_revision,
        } => {
            let view = request_view(
                &mut session,
                cmds::memory_view_request(after.as_deref(), revisions.as_deref(), after_revision),
            )
            .await?;
            emit(&cmds::render_view(&view))
        }
    }
}

/// Flushes explicitly so piped output is complete on return.
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
            unexpected.message_type()
        ))),
    }
}

/// One explicit History read. A successful empty result is distinct from an
/// invalid request, an unreadable store, and a rotated companion projection;
/// each failure keeps its own meaning and exit class instead of being shown
/// as an empty timeline.
fn history_items(
    response: ene_api::v1::round::HistoryResponse,
) -> Result<Vec<ene_api::v1::round::HistoryItem>, CliError> {
    use ene_api::v1::round::HistoryResponse;
    match response {
        HistoryResponse::Items(items) => Ok(items),
        HistoryResponse::InvalidRequest => Err(CliError::ServerRejected(String::from(
            "invalid history request; correct the request fields and retry",
        ))),
        HistoryResponse::Unavailable => Err(CliError::ServerOutcome(String::from(
            "history is unavailable; retry later",
        ))),
        HistoryResponse::StaleCompanion => Err(CliError::ServerOutcome(String::from(
            "companion projection is stale; re-sync presence and retry",
        ))),
    }
}

async fn request_history(
    session: &mut client::Client,
    round: Option<&str>,
    limit: u64,
) -> Result<Vec<ene_api::v1::round::HistoryItem>, CliError> {
    let companion = session.companion_ref();
    let request = match round {
        Some(round) => cmds::round_history_request(&companion, round, limit),
        None => cmds::history_request(&companion, limit),
    };
    match session
        .request(WirePayload::HistoryRequest(request))
        .await?
    {
        WirePayload::HistoryResponse(response) => history_items(response),
        unexpected => Err(CliError::ServerRejected(format!(
            "unexpected {} while reading history; expected HistoryResponse",
            unexpected.message_type()
        ))),
    }
}

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
                unexpected.message_type()
            )));
        }
    };
    match cmds::describe_management(&outcome) {
        cmds::ManagementAction::Applied { detail } => Ok(detail),
        cmds::ManagementAction::Retryable { message } => Err(CliError::ServerOutcome(message)),
        cmds::ManagementAction::Terminal { message } => Err(CliError::ServerRejected(message)),
    }
}

async fn run_setup(session: &mut client::Client, mode: cmds::SetupMode) -> Result<(), CliError> {
    match mode {
        cmds::SetupMode::Show => {
            let view = request_view(session, cmds::setup_view_request()).await?;
            emit(&cmds::render_view(&view))
        }
        cmds::SetupMode::Assign {
            provider,
            model,
            learning,
        } => {
            let view = request_view(session, cmds::setup_view_request()).await?;
            let base = BaseViewMark(view.mark.0.clone());
            let credential =
                cmds::credential_intent(CommandWireId(uuid::Uuid::new_v4()), &base, &provider);
            apply_intent(session, credential).await?;
            let capability = if learning {
                cmds::CAPABILITY_LEARNING
            } else {
                cmds::CAPABILITY_DIALOGUE
            };
            let assignment = cmds::assignment_intent(
                CommandWireId(uuid::Uuid::new_v4()),
                &base,
                capability,
                &provider,
                &model,
            );
            apply_intent(session, assignment).await?;
            if learning {
                emit(&format!(
                    "learning assignment stored: provider={provider} model={model}"
                ))
            } else {
                emit(&format!(
                    "setup complete: provider={provider} model={model}"
                ))
            }
        }
    }
}

/// Any stdio failure before the close frame and the buffered frames are
/// flushed returns early and sends no presentation observation, so the Host
/// keeps the stream `Pending`/`Unknown` instead of recording a presentation
/// the operator never saw.
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
                unexpected.message_type()
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
                    unexpected.message_type()
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

/// Presentation fact and completion success are separate claims: text the
/// operator saw stays presented even when the stream did not complete, and
/// only frames of this live stream count (the opening frame shows nothing).
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
    use std::path::PathBuf;

    use super::{CliError, cli_from_matches, ene_ctl_command};

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    fn parse(words: &[&str]) -> Result<super::Cli, CliError> {
        let matches = ene_ctl_command()
            .try_get_matches_from(std::iter::once(String::from("ene-ctl")).chain(args(words)))
            .map_err(|error| CliError::Usage(error.to_string()))?;
        cli_from_matches(matches)
    }

    fn clap_error(words: &[&str]) -> clap::error::ErrorKind {
        match ene_ctl_command()
            .try_get_matches_from(std::iter::once(String::from("ene-ctl")).chain(args(words)))
        {
            Ok(_) => panic!("{words:?} must fail"),
            Err(error) => error.kind(),
        }
    }

    #[test]
    fn help_and_version_are_successful_clap_exits() {
        assert!(matches!(
            clap_error(&["--help"]),
            clap::error::ErrorKind::DisplayHelp
        ));
        assert!(matches!(
            clap_error(&["--version"]),
            clap::error::ErrorKind::DisplayVersion
        ));
        // Subcommand help is standard too.
        assert!(matches!(
            clap_error(&["send", "--help"]),
            clap::error::ErrorKind::DisplayHelp
        ));
    }

    #[test]
    fn missing_command_reports_usage() {
        assert!(matches!(parse(&[]), Err(CliError::Usage(_))));
        assert!(matches!(
            parse(&["--config", "/tmp/ene.json"]),
            Err(CliError::Usage(_))
        ));
    }

    #[test]
    fn config_is_global_and_keeps_the_last_value() {
        let cli = parse(&["--config", "/tmp/ene.json", "status"]).expect("config plus status");
        assert!(cli.config == Some(PathBuf::from("/tmp/ene.json")));
        assert!(cli.command == super::cmds::Command::Status);
        // `--config` after the subcommand is accepted as a global option.
        let after = parse(&["status", "--config", "/tmp/ene.json"]).expect("config after status");
        assert!(after.config == Some(PathBuf::from("/tmp/ene.json")));
        let repeated = parse(&[
            "--config",
            "/tmp/a.json",
            "--config",
            "/tmp/b.json",
            "status",
        ])
        .expect("a repeated --config keeps the last value");
        assert!(repeated.config == Some(PathBuf::from("/tmp/b.json")));
    }

    #[test]
    fn config_value_named_serve_stays_data() {
        let cli = parse(&["--config", "serve", "status"]).expect("the value is not a command");
        assert!(cli.config == Some(PathBuf::from("serve")));
    }

    #[test]
    fn setup_forms_parse_and_invalid_combinations_report_usage() {
        let show = parse(&["setup", "--show"]).expect("setup --show");
        assert!(show.command == super::cmds::Command::Setup(super::cmds::SetupMode::Show));
        let assign = parse(&["setup", "--provider", "openai", "--model", "gpt-x"])
            .expect("setup assignment");
        assert!(
            assign.command
                == super::cmds::Command::Setup(super::cmds::SetupMode::Assign {
                    provider: String::from("openai"),
                    model: String::from("gpt-x"),
                    learning: false,
                })
        );
        let learning = parse(&[
            "setup",
            "--provider",
            "openai",
            "--model",
            "gpt-x",
            "--learning",
        ])
        .expect("learning assignment");
        assert!(matches!(
            learning.command,
            super::cmds::Command::Setup(super::cmds::SetupMode::Assign { learning: true, .. })
        ));
        for words in [
            &["setup"][..],
            &["setup", "--provider", "openai"][..],
            &[
                "setup",
                "--show",
                "--provider",
                "openai",
                "--model",
                "gpt-x",
            ][..],
            &["setup", "--provider", "acme", "--model", "gpt-x"][..],
            &["setup", "--provider", "openai", "--model", ""][..],
            &["setup", "--unknown"][..],
        ] {
            assert!(
                matches!(parse(words), Err(CliError::Usage(_))),
                "{words:?} must be a usage error"
            );
        }
    }

    #[test]
    fn send_forms_parse_and_end_of_options_carries_option_like_text() {
        let joined = parse(&["send", "hello", "world"]).expect("send text");
        assert!(
            joined.command
                == super::cmds::Command::Send(super::cmds::SendArgs {
                    round: None,
                    fresh: false,
                    text: String::from("hello world"),
                })
        );
        let literal = parse(&["send", "--", "--foo"]).expect("send -- --foo");
        assert!(
            literal.command
                == super::cmds::Command::Send(super::cmds::SendArgs {
                    round: None,
                    fresh: false,
                    text: String::from("--foo"),
                }),
            "`--` must carry option-like text, got {:?}",
            literal.command
        );
        let round = parse(&["send", "--round", "round-7", "hi"]).expect("send --round");
        assert!(matches!(
            round.command,
            super::cmds::Command::Send(super::cmds::SendArgs {
                round: Some(_),
                fresh: false,
                ..
            })
        ));
        let fresh = parse(&["send", "--new", "hi"]).expect("send --new");
        assert!(matches!(
            fresh.command,
            super::cmds::Command::Send(super::cmds::SendArgs { fresh: true, .. })
        ));
        for words in [
            &["send"][..],
            &["send", "--new", "--round", "r", "hi"][..],
            &["send", "--round"][..],
            &["send", "--unknown", "hi"][..],
        ] {
            assert!(
                matches!(parse(words), Err(CliError::Usage(_))),
                "{words:?} must be a usage error"
            );
        }
    }

    #[test]
    fn watch_history_and_memory_forms_parse() {
        let watch = parse(&["watch", "--round", "round-7"]).expect("watch");
        assert!(
            watch.command
                == super::cmds::Command::Watch {
                    round: String::from("round-7")
                }
        );
        assert!(matches!(parse(&["watch"]), Err(CliError::Usage(_))));
        let default = parse(&["history"]).expect("history default");
        assert!(
            default.command
                == super::cmds::Command::History {
                    limit: super::cmds::DEFAULT_HISTORY_LIMIT
                }
        );
        let limited = parse(&["history", "--limit", "7"]).expect("history --limit");
        assert!(limited.command == super::cmds::Command::History { limit: 7 });
        assert!(matches!(
            parse(&["history", "--limit", "soon"]),
            Err(CliError::Usage(_))
        ));
        let list = parse(&["memory", "--after", "memory-1"]).expect("memory page");
        assert!(
            list.command
                == super::cmds::Command::Memory {
                    after: Some(String::from("memory-1")),
                    revisions: None,
                    after_revision: None,
                }
        );
        let detail = parse(&[
            "memory",
            "--revisions",
            "memory-1",
            "--after-revision",
            "20",
        ])
        .expect("memory revisions");
        assert!(
            detail.command
                == super::cmds::Command::Memory {
                    after: None,
                    revisions: Some(String::from("memory-1")),
                    after_revision: Some(20),
                }
        );
        for words in [
            &["memory", "extra"][..],
            &["memory", "--after", "a", "--revisions", "b"][..],
            &["memory", "--after-revision", "3"][..],
            &["memory", "--after"][..],
        ] {
            assert!(
                matches!(parse(words), Err(CliError::Usage(_))),
                "{words:?} must be a usage error"
            );
        }
    }

    #[test]
    fn unknown_flags_and_positionals_report_usage() {
        assert!(matches!(
            parse(&["status", "extra"]),
            Err(CliError::Usage(_))
        ));
        assert!(matches!(
            parse(&["status", "--verbose"]),
            Err(CliError::Usage(_))
        ));
        assert!(matches!(parse(&["frobnicate"]), Err(CliError::Usage(_))));
    }

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

    #[test]
    fn history_failures_keep_distinct_meanings() {
        use ene_api::v1::round::HistoryResponse;

        assert!(
            super::history_items(HistoryResponse::Items(Vec::new())).is_ok(),
            "an empty read is a success"
        );
        for (response, expected, what) in [
            (
                HistoryResponse::InvalidRequest,
                std::process::ExitCode::FAILURE,
                "invalid request",
            ),
            (
                HistoryResponse::Unavailable,
                std::process::ExitCode::from(2),
                "unavailable",
            ),
            (
                HistoryResponse::StaleCompanion,
                std::process::ExitCode::from(2),
                "stale projection",
            ),
        ] {
            let error = super::history_items(response)
                .expect_err("a failure variant must not answer items");
            assert!(
                error.exit_code() == expected,
                "{what} must keep its exit class, got {error:?}"
            );
        }
    }

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
