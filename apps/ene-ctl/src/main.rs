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
use ene_api::v1::undelivered::{UndeliveredResponse, UndeliveredSummary};
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
        .subcommand(
            clap::Command::new("tasks")
                .about("List Tasks (stored lifecycle plus execution flag)")
                .arg(Arg::new("cursor").long("cursor").value_name("CURSOR"))
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .value_name("N")
                        .value_parser(clap::value_parser!(u32)),
                ),
        )
        .subcommand(
            clap::Command::new("report")
                .about("Show one Task's paged report (no bodies)")
                .arg(
                    Arg::new("task")
                        .long("task")
                        .value_name("REF")
                        .required(true),
                )
                .arg(Arg::new("cursor").long("cursor").value_name("CURSOR"))
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .value_name("N")
                        .value_parser(clap::value_parser!(u32)),
                ),
        )
        .subcommand(
            clap::Command::new("source")
                .about("Show one bounded body page of a report source")
                .arg(
                    Arg::new("source")
                        .long("source")
                        .value_name("REF")
                        .required(true),
                )
                .arg(
                    Arg::new("cursor")
                        .long("cursor")
                        .value_name("N")
                        .value_parser(clap::value_parser!(u64)),
                )
                .arg(
                    Arg::new("limit-bytes")
                        .long("limit-bytes")
                        .value_name("N")
                        .value_parser(clap::value_parser!(u32)),
                ),
        )
        .subcommand(
            clap::Command::new("select-task")
                .about("Select the Owner-confirmed Task for this conversation")
                .arg(
                    Arg::new("task")
                        .long("task")
                        .value_name("REF")
                        .required(true),
                ),
        )
        .subcommand(
            clap::Command::new("resume-task")
                .about("Explicitly resume one interrupted Task")
                .arg(
                    Arg::new("task")
                        .long("task")
                        .value_name("REF")
                        .required(true),
                )
                .arg(
                    Arg::new("revision")
                        .long("revision")
                        .value_name("N")
                        .value_parser(clap::value_parser!(u64))
                        .required(true),
                )
                .arg(
                    Arg::new("purpose")
                        .long("purpose")
                        .value_name("REF")
                        .required(true),
                )
                .arg(
                    Arg::new("instruction")
                        .long("instruction")
                        .value_name("TEXT")
                        .required(true),
                ),
        )
        .subcommand(
            clap::Command::new("undelivered")
                .about("Fetch the undelivered backlog, paint it, and ACK what was painted")
                .arg(Arg::new("cursor").long("cursor").value_name("CURSOR"))
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .value_name("N")
                        .value_parser(clap::value_parser!(u32)),
                )
                .arg(
                    Arg::new("redisplay")
                        .long("redisplay")
                        .action(ArgAction::SetTrue),
                ),
        )
        .subcommand(
            clap::Command::new("deletion")
                .about("Request a Targeted Deletion (the Host PC still confirms it)")
                .arg(
                    Arg::new("text")
                        .long("text")
                        .value_name("TEXT")
                        .required(true)
                        .allow_hyphen_values(true),
                )
                .arg(
                    Arg::new("purpose")
                        .long("purpose")
                        .value_name("privacy|security")
                        .default_value("privacy"),
                ),
        )
        .subcommand(
            clap::Command::new("deletion-status")
                .about("Show the bounded Targeted Deletion operation status")
                .arg(Arg::new("cursor").long("cursor").value_name("CURSOR"))
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .value_name("N")
                        .value_parser(clap::value_parser!(u32)),
                ),
        )
        .subcommand(
            clap::Command::new("usage")
                .about("Read one bounded page of usage / cost and the current caps")
                .arg(Arg::new("from").long("from").value_name("RFC3339"))
                .arg(Arg::new("to").long("to").value_name("RFC3339"))
                .arg(Arg::new("provider").long("provider").value_name("NAME"))
                .arg(Arg::new("model").long("model").value_name("NAME"))
                .arg(Arg::new("consumer").long("consumer").value_name("NAME"))
                .arg(Arg::new("purpose").long("purpose").value_name("NAME"))
                .arg(
                    Arg::new("status")
                        .long("status")
                        .value_name("reported|unknown|reserved"),
                )
                .arg(Arg::new("cursor").long("cursor").value_name("CURSOR"))
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .value_name("N")
                        .value_parser(clap::value_parser!(u32)),
                ),
        )
        .subcommand(
            clap::Command::new("usage-cap")
                .about("Set or update one system/provider daily/monthly usage cap")
                .arg(
                    Arg::new("scope")
                        .long("scope")
                        .value_name("system|provider")
                        .default_value("system"),
                )
                .arg(Arg::new("provider").long("provider").value_name("NAME"))
                .arg(
                    Arg::new("window")
                        .long("window")
                        .value_name("daily_utc|monthly_utc")
                        .default_value("daily_utc"),
                )
                .arg(
                    Arg::new("currency")
                        .long("currency")
                        .value_name("CODE")
                        .default_value("USD"),
                )
                .arg(
                    Arg::new("limit-micros")
                        .long("limit-micros")
                        .value_name("N")
                        .value_parser(clap::value_parser!(u64))
                        .required(true),
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
        "tasks" => cmds::Command::Tasks {
            cursor: sub.get_one::<String>("cursor").cloned(),
            limit: sub.get_one::<u32>("limit").copied(),
        },
        "report" => cmds::Command::Report {
            task: sub
                .get_one::<String>("task")
                .cloned()
                .ok_or_else(|| usage_error("report requires --task REF"))?,
            cursor: sub.get_one::<String>("cursor").cloned(),
            limit: sub.get_one::<u32>("limit").copied(),
        },
        "source" => cmds::Command::Source {
            source: sub
                .get_one::<String>("source")
                .cloned()
                .ok_or_else(|| usage_error("source requires --source REF"))?,
            cursor: sub.get_one::<u64>("cursor").copied(),
            limit_bytes: sub.get_one::<u32>("limit-bytes").copied(),
        },
        "select-task" => cmds::Command::SelectTask {
            task: sub
                .get_one::<String>("task")
                .cloned()
                .ok_or_else(|| usage_error("select-task requires --task REF"))?,
        },
        "resume-task" => cmds::Command::ResumeTask {
            task: sub
                .get_one::<String>("task")
                .cloned()
                .ok_or_else(|| usage_error("resume-task requires --task REF"))?,
            revision: *sub
                .get_one::<u64>("revision")
                .ok_or_else(|| usage_error("resume-task requires --revision N"))?,
            purpose: sub
                .get_one::<String>("purpose")
                .cloned()
                .ok_or_else(|| usage_error("resume-task requires --purpose REF"))?,
            instruction: sub
                .get_one::<String>("instruction")
                .cloned()
                .ok_or_else(|| usage_error("resume-task requires --instruction TEXT"))?,
        },
        "undelivered" => cmds::Command::Undelivered {
            cursor: sub.get_one::<String>("cursor").cloned(),
            limit: sub.get_one::<u32>("limit").copied(),
            redisplay: sub.get_flag("redisplay"),
        },
        "deletion" => {
            let purpose = sub
                .get_one::<String>("purpose")
                .cloned()
                .unwrap_or_else(|| String::from("privacy"));
            let Some(purpose) = cmds::deletion_purpose(&purpose) else {
                return Err(usage_error("--purpose must be privacy or security"));
            };
            cmds::Command::Deletion {
                text: sub
                    .get_one::<String>("text")
                    .cloned()
                    .ok_or_else(|| usage_error("deletion requires --text TEXT"))?,
                purpose,
            }
        }
        "deletion-status" => cmds::Command::DeletionStatus {
            cursor: sub.get_one::<String>("cursor").cloned(),
            limit: sub.get_one::<u32>("limit").copied(),
        },
        "usage" => cmds::Command::Usage(cmds::UsageArgs {
            from: sub.get_one::<String>("from").cloned(),
            to: sub.get_one::<String>("to").cloned(),
            provider: sub.get_one::<String>("provider").cloned(),
            model: sub.get_one::<String>("model").cloned(),
            consumer: sub.get_one::<String>("consumer").cloned(),
            purpose: sub.get_one::<String>("purpose").cloned(),
            status: sub.get_one::<String>("status").cloned(),
            cursor: sub.get_one::<String>("cursor").cloned(),
            limit: sub.get_one::<u32>("limit").copied(),
        }),
        "usage-cap" => {
            let scope = sub
                .get_one::<String>("scope")
                .cloned()
                .unwrap_or_else(|| String::from("system"));
            let provider = sub.get_one::<String>("provider").cloned();
            match (scope.as_str(), provider.as_deref()) {
                ("system", None) => {}
                ("provider", Some(_)) => {}
                ("system", Some(_)) => {
                    return Err(usage_error(
                        "--provider is only valid with --scope provider",
                    ));
                }
                ("provider", None) => {
                    return Err(usage_error("--scope provider requires --provider NAME"));
                }
                _ => return Err(usage_error("--scope must be system or provider")),
            }
            cmds::Command::UsageCap {
                scope,
                provider,
                window: sub
                    .get_one::<String>("window")
                    .cloned()
                    .unwrap_or_else(|| String::from("daily_utc")),
                currency: sub
                    .get_one::<String>("currency")
                    .cloned()
                    .unwrap_or_else(|| String::from("USD")),
                limit_micros: *sub
                    .get_one::<u64>("limit-micros")
                    .ok_or_else(|| usage_error("usage-cap requires --limit-micros N"))?,
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
    let mut session = match client::Client::begin_connect(data_dir, &platform, &platform).await? {
        client::ConnectProgress::Connected(session) => session,
        client::ConnectProgress::Pending(pending) => {
            emit(&format!(
                "pairing pending: approve {} on the Host-local trusted surface; waiting on this connection",
                pending.pending_id()
            ))?;
            pending.complete().await?
        }
    };
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
        cmds::Command::Tasks { cursor, limit } => {
            let page = request_task_list(&mut session, cursor.as_deref(), limit).await?;
            emit(&cmds::render_task_list(&page))
        }
        cmds::Command::Report {
            task,
            cursor,
            limit,
        } => {
            let page = request_task_report(&mut session, &task, cursor.as_deref(), limit).await?;
            emit(&cmds::render_report_page(&page))
        }
        cmds::Command::Source {
            source,
            cursor,
            limit_bytes,
        } => {
            let page = request_report_source(&mut session, &source, cursor, limit_bytes).await?;
            emit(&cmds::render_source_page(&page))
        }
        cmds::Command::SelectTask { task } => {
            let selected = request_select_task(&mut session, &task).await?;
            emit(&format!(
                "selected {} rev {} {}",
                selected.task.0, selected.revision, selected.progress
            ))
        }
        cmds::Command::ResumeTask {
            task,
            revision,
            purpose,
            instruction,
        } => run_resume_task(&mut session, &task, revision, &purpose, instruction).await,
        cmds::Command::Undelivered {
            cursor,
            limit,
            redisplay,
        } => run_undelivered(&mut session, cursor.as_deref(), limit, redisplay).await,
        cmds::Command::Deletion { text, purpose } => {
            run_deletion(&mut session, &text, purpose).await
        }
        cmds::Command::DeletionStatus { cursor, limit } => {
            let response = request_deletion_status(&mut session, cursor.as_deref(), limit).await?;
            emit(&cmds::render_deletion_status(&response))
        }
        cmds::Command::Usage(args) => {
            let response = request_usage(&mut session, &args).await?;
            emit(&cmds::render_usage_page(&response))
        }
        cmds::Command::UsageCap {
            scope,
            provider,
            window,
            currency,
            limit_micros,
        } => {
            run_usage_cap(
                &mut session,
                &scope,
                provider.as_deref(),
                &window,
                &currency,
                limit_micros,
            )
            .await
        }
    }
}

/// One bounded usage summary read. `Reject` (a malformed filter the Host
/// refuses) stays distinct from `Unavailable` (the read could not answer) and
/// from a stale cursor.
async fn request_usage(
    session: &mut client::Client,
    args: &cmds::UsageArgs,
) -> Result<ene_api::v1::usage::UsageSummaryResponse, CliError> {
    match session
        .request(WirePayload::UsageSummaryRequest(cmds::usage_request(args)))
        .await?
    {
        WirePayload::UsageSummaryResponse(response) => Ok(response),
        WirePayload::Reject(notice) => Err(CliError::ServerRejected(format!(
            "usage request rejected: {}",
            notice.detail
        ))),
        unexpected => Err(CliError::ServerRejected(format!(
            "unexpected {} while reading usage; expected UsageSummaryResponse",
            unexpected.message_type()
        ))),
    }
}

/// Sets or updates one usage cap through the permission-owned command.
///
/// The Client only proposes: it first reads the bounded usage page so the
/// intent's base view is the mark the Host issued for exactly this slot, then
/// sends the shared-grammar cap target. The Host re-checks the current
/// authenticated connection, the mark, and the cap revision; a stale answer
/// means the Owner re-reads instead of overwriting.
async fn run_usage_cap(
    session: &mut client::Client,
    scope: &str,
    provider: Option<&str>,
    window: &str,
    currency: &str,
    limit_micros: u64,
) -> Result<(), CliError> {
    let args = cmds::UsageArgs {
        from: None,
        to: None,
        provider: provider.map(str::to_owned),
        model: None,
        consumer: None,
        purpose: None,
        status: None,
        cursor: None,
        limit: Some(1),
    };
    let response = request_usage(session, &args).await?;
    let ene_api::v1::usage::UsageSummaryResponse::Page(page) = response else {
        return Err(CliError::ServerOutcome(String::from(
            "usage is unavailable; cannot build a cap base view",
        )));
    };
    let Some(base) = cmds::usage_cap_mark_for(&page, scope, provider, window) else {
        return Err(CliError::ServerOutcome(String::from(
            "the usage read did not name this cap slot; retry later",
        )));
    };
    let intent = cmds::usage_cap_intent(
        CommandWireId(uuid::Uuid::new_v4()),
        base,
        scope,
        provider,
        window,
        currency,
        limit_micros,
    );
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
        cmds::ManagementAction::Applied { detail } => emit(&detail),
        cmds::ManagementAction::Retryable { message } => Err(CliError::ServerOutcome(message)),
        cmds::ManagementAction::Terminal { message } => Err(CliError::ServerRejected(message)),
    }
}

/// Sends one advisory Targeted Deletion request.
///
/// The Client first reads the live deletion surface (a pure read) so the
/// intent carries the current mark; the Host re-checks it and stages at most
/// one durable request. The exact text travels in the typed target, never in
/// the rationale quote, and the answer is printed through the shared
/// management description — the command itself never claims a deletion
/// happened: only the Host PC confirmation and the status page can say that.
async fn run_deletion(
    session: &mut client::Client,
    text: &str,
    purpose: ene_api::v1::deletion::DeletionPurposeWire,
) -> Result<(), CliError> {
    if text.trim().is_empty() {
        return Err(CliError::Usage(String::from(
            "deletion requires a non-blank --text",
        )));
    }
    let status = request_deletion_status(session, None, Some(1)).await?;
    let ene_api::v1::deletion::DeletionStatusResponse::Page(page) = status else {
        return Err(CliError::ServerOutcome(String::from(
            "deletion status is unavailable; retry later",
        )));
    };
    let intent = cmds::deletion_intent(
        CommandWireId(uuid::Uuid::new_v4()),
        &page.mark.0,
        purpose,
        text,
    );
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
        cmds::ManagementAction::Applied { detail } => emit(&format!(
            "{detail}; confirm it on the Host PC (`ene-core pending-deletions`)"
        )),
        cmds::ManagementAction::Retryable { message } => Err(CliError::ServerOutcome(format!(
            "{message}; confirm it on the Host PC (`ene-core pending-deletions`)"
        ))),
        cmds::ManagementAction::Terminal { message } => Err(CliError::ServerRejected(message)),
    }
}

/// One bounded Targeted Deletion status read; no body crosses this path.
async fn request_deletion_status(
    session: &mut client::Client,
    cursor: Option<&str>,
    limit: Option<u32>,
) -> Result<ene_api::v1::deletion::DeletionStatusResponse, CliError> {
    use ene_api::v1::deletion::DeletionStatusRequest;
    use ene_api::v1::refs::DeletionStatusCursorWire;

    match session
        .request(WirePayload::DeletionStatusRequest(DeletionStatusRequest {
            cursor: cursor.map(|cursor| DeletionStatusCursorWire(cursor.to_string())),
            limit,
        }))
        .await?
    {
        WirePayload::DeletionStatusResponse(response) => Ok(response),
        WirePayload::Reject(notice) => Err(CliError::ServerRejected(format!(
            "deletion status rejected: {}",
            notice.detail
        ))),
        unexpected => Err(CliError::ServerRejected(format!(
            "unexpected {} while reading the deletion status; expected DeletionStatusResponse",
            unexpected.message_type()
        ))),
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

/// One explicit Task-list read. Stale cursors and rejections keep their own
/// exit classes instead of rendering as an empty list.
async fn request_task_list(
    session: &mut client::Client,
    cursor: Option<&str>,
    limit: Option<u32>,
) -> Result<ene_api::v1::undelivered::TaskListPage, CliError> {
    use ene_api::v1::undelivered::TaskListResponse;
    match session
        .request(WirePayload::ListTasks(cmds::list_tasks_request(
            cursor.map(str::to_owned),
            limit,
        )))
        .await?
    {
        WirePayload::TaskListResponse(TaskListResponse::Page(page)) => Ok(page),
        WirePayload::TaskListResponse(TaskListResponse::StaleBaseView { .. }) => {
            Err(CliError::ServerOutcome(String::from(
                "stale task-list cursor; re-query from the head",
            )))
        }
        unexpected => Err(CliError::ServerRejected(format!(
            "unexpected {} while listing tasks; expected TaskListResponse",
            unexpected.message_type()
        ))),
    }
}

/// One explicit Task-report read (paged identities, never bodies).
async fn request_task_report(
    session: &mut client::Client,
    task: &str,
    cursor: Option<&str>,
    limit: Option<u32>,
) -> Result<ene_api::v1::undelivered::TaskReportPage, CliError> {
    use ene_api::v1::undelivered::TaskReportResponse;
    let page = match session
        .request(WirePayload::GetTaskReport(cmds::task_report_request(
            task,
            cursor.map(str::to_owned),
            limit,
        )))
        .await?
    {
        WirePayload::TaskReportResponse(response) => response,
        unexpected => {
            return Err(CliError::ServerRejected(format!(
                "unexpected {} while reading a task report; expected TaskReportResponse",
                unexpected.message_type()
            )));
        }
    };
    match cmds::describe_report(&page) {
        cmds::ReportAction::Show => match page {
            TaskReportResponse::Page(page) => Ok(page),
            TaskReportResponse::UnknownRef | TaskReportResponse::StaleBaseView { .. } => Err(
                CliError::ServerOutcome(String::from("task report moved underneath the request")),
            ),
        },
        cmds::ReportAction::Retryable { message } => Err(CliError::ServerOutcome(message)),
    }
}

/// One bounded source-body page. `InputUnavailable` is retryable (exit 2):
/// the body exists but cannot be projected safely right now.
async fn request_report_source(
    session: &mut client::Client,
    source: &str,
    cursor: Option<u64>,
    limit_bytes: Option<u32>,
) -> Result<ene_api::v1::undelivered::ReportSourcePageView, CliError> {
    use ene_api::v1::undelivered::ReportSourceResponse;
    match session
        .request(WirePayload::GetReportSource(cmds::report_source_request(
            source,
            cursor,
            limit_bytes,
        )))
        .await?
    {
        WirePayload::ReportSourceResponse(ReportSourceResponse::Page(page)) => Ok(page),
        WirePayload::ReportSourceResponse(ReportSourceResponse::UnknownRef) => {
            Err(CliError::ServerOutcome(String::from(
                "unknown report source; re-read the report and retry",
            )))
        }
        WirePayload::ReportSourceResponse(ReportSourceResponse::InputUnavailable) => Err(
            CliError::ServerOutcome(String::from("report source is unavailable; retry later")),
        ),
        unexpected => Err(CliError::ServerRejected(format!(
            "unexpected {} while reading a report source; expected ReportSourceResponse",
            unexpected.message_type()
        ))),
    }
}

/// First-party Task selection: in-memory display selection, never an
/// execution start.
async fn request_select_task(
    session: &mut client::Client,
    task: &str,
) -> Result<ene_api::v1::undelivered::TaskSelected, CliError> {
    use ene_api::v1::undelivered::SelectTaskResponse;
    match session
        .request(WirePayload::SelectTask(cmds::select_task_request(task)))
        .await?
    {
        WirePayload::SelectTaskResponse(SelectTaskResponse::Selected(selected)) => Ok(selected),
        WirePayload::SelectTaskResponse(SelectTaskResponse::UnknownRef) => Err(
            CliError::ServerOutcome(String::from("unknown task reference; re-list and retry")),
        ),
        unexpected => Err(CliError::ServerRejected(format!(
            "unexpected {} while selecting a task; expected SelectTaskResponse",
            unexpected.message_type()
        ))),
    }
}

/// Explicit first-party resume through the wire command (retryable identity:
/// a lost reply replays through `retry`, never a second command).
async fn run_resume_task(
    session: &mut client::Client,
    task: &str,
    revision: u64,
    purpose: &str,
    instruction: String,
) -> Result<(), CliError> {
    let prepared = session.prepare(WirePayload::ResumeTask(cmds::resume_task_request(
        task,
        revision,
        purpose,
        instruction,
    )));
    let outcome = match session.execute(&prepared).await? {
        WirePayload::ResumeTaskOutcome(outcome) => outcome,
        unexpected => {
            return Err(CliError::ServerRejected(format!(
                "unexpected {} while resuming a task; expected ResumeTaskOutcome",
                unexpected.message_type()
            )));
        }
    };
    match cmds::describe_resume(&outcome) {
        cmds::ResumeAction::Resumed { detail } => emit(&detail),
        cmds::ResumeAction::Refused { message } => Err(CliError::ServerRejected(message)),
        cmds::ResumeAction::Retryable { message } => Err(CliError::ServerOutcome(message)),
    }
}

/// Fetches one undelivered page, paints it, and ACKs the receipts that fully
/// painted. Any stdio failure before the flush returns early and sends no
/// ACK, so the Host keeps the batch `Unknown` instead of recording a
/// presentation the operator never saw.
async fn run_undelivered(
    session: &mut client::Client,
    cursor: Option<&str>,
    limit: Option<u32>,
    redisplay: bool,
) -> Result<(), CliError> {
    use ene_api::v1::undelivered::UndeliveredResponse;
    let response = match session
        .request(WirePayload::UndeliveredRequest(cmds::undelivered_request(
            cursor.map(str::to_owned),
            limit,
            redisplay,
        )))
        .await?
    {
        WirePayload::UndeliveredResponse(response) => response,
        unexpected => {
            return Err(CliError::ServerRejected(format!(
                "unexpected {} while fetching undelivered items; expected UndeliveredResponse",
                unexpected.message_type()
            )));
        }
    };
    match cmds::describe_fetch(&response) {
        cmds::FetchAction::Paint => {}
        cmds::FetchAction::Retryable { message } => {
            return Err(CliError::ServerOutcome(message));
        }
    }
    let UndeliveredResponse::Summary(summary) = response else {
        return Err(CliError::ServerOutcome(String::from(
            "undelivered fetch moved underneath the request",
        )));
    };
    emit(&cmds::render_summary(&summary))?;
    ack_summary(session, &summary).await
}

/// ACKs one fully painted summary as `Presented`, echoing the round and
/// generation the summary showed for the Host's receipt comparison.
async fn ack_summary(
    session: &mut client::Client,
    summary: &ene_api::v1::undelivered::UndeliveredSummary,
) -> Result<(), CliError> {
    if summary.items.is_empty() {
        return Ok(());
    }
    let ack = cmds::undelivered_ack(&summary.receipt.0, PresentationStatus::Presented);
    let outcome = match session
        .request_observed(
            WirePayload::UndeliveredAck(ack),
            Some(summary.round.clone()),
        )
        .await?
    {
        WirePayload::UndeliveredAckOutcome(outcome) => outcome,
        unexpected => {
            return Err(CliError::ServerRejected(format!(
                "unexpected {} while confirming presentation; expected UndeliveredAckOutcome",
                unexpected.message_type()
            )));
        }
    };
    match cmds::describe_ack(&outcome) {
        cmds::AckAction::Confirmed { .. } => Ok(()),
        cmds::AckAction::Retryable { message } => Err(CliError::ServerOutcome(message)),
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
    // Backlog the Host auto-presented at attach (recovery/summon, no Owner
    // query): paint it before the new reply and ACK it with the stream's
    // presentation observation below. A stdio failure here sends no ACK, so
    // the Host keeps the batch Unknown.
    let mut auto: Vec<UndeliveredSummary> = Vec::new();
    for frame in session.take_undelivered() {
        if let WirePayload::UndeliveredResponse(UndeliveredResponse::Summary(summary)) =
            frame.payload
        {
            let text = cmds::render_summary(&summary);
            if !text.is_empty() {
                writeln!(stdout, "{text}").map_err(|error| {
                    CliError::Transport(format!("stdout write failed: {}", error.kind()))
                })?;
                stdout.flush().map_err(|error| {
                    CliError::Transport(format!("stdout flush failed: {}", error.kind()))
                })?;
            }
            auto.push(summary);
        }
    }
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
            WirePayload::UndeliveredResponse(UndeliveredResponse::Summary(summary)) => {
                // Auto-presented backlog interleaved with the stream: paint
                // inline and remember the receipt for the end-of-send ACK.
                let text = cmds::render_summary(&summary);
                if !text.is_empty() {
                    writeln!(stdout, "{text}").map_err(|error| {
                        CliError::Transport(format!("stdout write failed: {}", error.kind()))
                    })?;
                    stdout.flush().map_err(|error| {
                        CliError::Transport(format!("stdout flush failed: {}", error.kind()))
                    })?;
                }
                auto.push(summary);
            }
            WirePayload::UndeliveredResponse(_) | WirePayload::UndeliveredAckOutcome(_) => {
                // Non-summary fetch answers and stray ACK outcomes never
                // route here; absorb them instead of failing the stream.
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
    // The backlog painted above (attach-time and in-stream auto-presents)
    // is ACKed only now, after its final frame painted: a partial batch
    // would have returned early above with no ACK, keeping it Unknown.
    for summary in &auto {
        ack_summary(session, summary).await?;
    }
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
    fn deletion_forms_parse() {
        let request = parse(&["deletion", "--text", "leaked key"]).expect("deletion default");
        assert!(
            request.command
                == super::cmds::Command::Deletion {
                    text: String::from("leaked key"),
                    purpose: ene_api::v1::deletion::DeletionPurposeWire::Privacy,
                }
        );
        let security = parse(&["deletion", "--text", "-secret-", "--purpose", "security"])
            .expect("deletion security");
        assert!(
            security.command
                == super::cmds::Command::Deletion {
                    text: String::from("-secret-"),
                    purpose: ene_api::v1::deletion::DeletionPurposeWire::Security,
                }
        );
        let status = parse(&[
            "deletion-status",
            "--cursor",
            "deletion-status:x",
            "--limit",
            "7",
        ])
        .expect("deletion status");
        assert!(
            status.command
                == super::cmds::Command::DeletionStatus {
                    cursor: Some(String::from("deletion-status:x")),
                    limit: Some(7),
                }
        );
        for words in [
            &["deletion"][..],
            &["deletion", "--text", "x", "--purpose", "everything"][..],
        ] {
            assert!(
                matches!(parse(words), Err(CliError::Usage(_))),
                "{words:?} must be a usage error"
            );
        }
    }

    #[test]
    fn task_and_undelivered_forms_parse() {
        let tasks = parse(&["tasks"]).expect("tasks default");
        assert!(
            tasks.command
                == super::cmds::Command::Tasks {
                    cursor: None,
                    limit: None,
                }
        );
        let paged = parse(&["tasks", "--cursor", "c1", "--limit", "7"]).expect("tasks page");
        assert!(
            paged.command
                == super::cmds::Command::Tasks {
                    cursor: Some(String::from("c1")),
                    limit: Some(7),
                }
        );
        let report = parse(&["report", "--task", "task-1"]).expect("report");
        assert!(
            report.command
                == super::cmds::Command::Report {
                    task: String::from("task-1"),
                    cursor: None,
                    limit: None,
                }
        );
        assert!(matches!(parse(&["report"]), Err(CliError::Usage(_))));
        let source = parse(&["source", "--source", "s1", "--cursor", "9"]).expect("source");
        assert!(
            source.command
                == super::cmds::Command::Source {
                    source: String::from("s1"),
                    cursor: Some(9),
                    limit_bytes: None,
                }
        );
        assert!(matches!(parse(&["source"]), Err(CliError::Usage(_))));
        let select = parse(&["select-task", "--task", "task-2"]).expect("select-task");
        assert!(
            select.command
                == super::cmds::Command::SelectTask {
                    task: String::from("task-2")
                }
        );
        let resume = parse(&[
            "resume-task",
            "--task",
            "task-3",
            "--revision",
            "4",
            "--purpose",
            "task-3:4",
            "--instruction",
            "go on",
        ])
        .expect("resume-task");
        assert!(
            resume.command
                == super::cmds::Command::ResumeTask {
                    task: String::from("task-3"),
                    revision: 4,
                    purpose: String::from("task-3:4"),
                    instruction: String::from("go on"),
                }
        );
        assert!(matches!(
            parse(&["resume-task", "--task", "t"]),
            Err(CliError::Usage(_))
        ));
        let undelivered = parse(&["undelivered"]).expect("undelivered default");
        assert!(
            undelivered.command
                == super::cmds::Command::Undelivered {
                    cursor: None,
                    limit: None,
                    redisplay: false,
                }
        );
        let rescan = parse(&["undelivered", "--redisplay", "--limit", "3"]).expect("redisplay");
        assert!(
            rescan.command
                == super::cmds::Command::Undelivered {
                    cursor: None,
                    limit: Some(3),
                    redisplay: true,
                }
        );
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

    #[test]
    fn usage_forms_parse_into_filters_and_reject_misuse() {
        use super::cmds::{Command as Cmd, UsageArgs};

        let cli = parse(&[
            "usage",
            "--from",
            "2026-09-01T00:00:00Z",
            "--to",
            "2026-09-02T00:00:00Z",
            "--provider",
            "openai",
            "--model",
            "gpt-x",
            "--consumer",
            "companion_dialogue",
            "--purpose",
            "dialogue_response",
            "--status",
            "reported",
            "--cursor",
            "cursor-1",
            "--limit",
            "10",
        ])
        .expect("the full usage form parses");
        assert!(
            cli.command
                == Cmd::Usage(UsageArgs {
                    from: Some(String::from("2026-09-01T00:00:00Z")),
                    to: Some(String::from("2026-09-02T00:00:00Z")),
                    provider: Some(String::from("openai")),
                    model: Some(String::from("gpt-x")),
                    consumer: Some(String::from("companion_dialogue")),
                    purpose: Some(String::from("dialogue_response")),
                    status: Some(String::from("reported")),
                    cursor: Some(String::from("cursor-1")),
                    limit: Some(10),
                })
        );
        let bare = parse(&["usage"]).expect("a filter-less usage read parses");
        assert!(
            bare.command
                == Cmd::Usage(UsageArgs {
                    from: None,
                    to: None,
                    provider: None,
                    model: None,
                    consumer: None,
                    purpose: None,
                    status: None,
                    cursor: None,
                    limit: None,
                })
        );
        assert!(matches!(
            parse(&["usage", "--limit", "many"]),
            Err(CliError::Usage(_))
        ));
    }

    #[test]
    fn usage_cap_forms_parse_and_validate_scope_pairing() {
        use super::cmds::Command as Cmd;

        let system = parse(&["usage-cap", "--limit-micros", "1000"])
            .expect("the default system daily form parses");
        assert!(
            system.command
                == Cmd::UsageCap {
                    scope: String::from("system"),
                    provider: None,
                    window: String::from("daily_utc"),
                    currency: String::from("USD"),
                    limit_micros: 1_000,
                }
        );
        let provider = parse(&[
            "usage-cap",
            "--scope",
            "provider",
            "--provider",
            "openai",
            "--window",
            "monthly_utc",
            "--currency",
            "USD",
            "--limit-micros",
            "42",
        ])
        .expect("the provider monthly form parses");
        assert!(
            provider.command
                == Cmd::UsageCap {
                    scope: String::from("provider"),
                    provider: Some(String::from("openai")),
                    window: String::from("monthly_utc"),
                    currency: String::from("USD"),
                    limit_micros: 42,
                }
        );
        // A scope/provider disagreement is a usage error, never a guessed
        // slot.
        assert!(matches!(
            parse(&["usage-cap", "--scope", "provider", "--limit-micros", "1"]),
            Err(CliError::Usage(_))
        ));
        assert!(matches!(
            parse(&[
                "usage-cap",
                "--scope",
                "system",
                "--provider",
                "openai",
                "--limit-micros",
                "1"
            ]),
            Err(CliError::Usage(_))
        ));
        assert!(matches!(
            parse(&["usage-cap", "--scope", "global", "--limit-micros", "1"]),
            Err(CliError::Usage(_))
        ));
        assert!(matches!(
            parse(&["usage-cap", "--limit-micros", "many"]),
            Err(CliError::Usage(_))
        ));
    }
}
