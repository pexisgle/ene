use std::path::{Path, PathBuf};

use ene_config::Config;
use ene_core::serve::{self, CoreError, HostHandle};

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("{0}")]
    Usage(String),
    #[error(transparent)]
    Config(#[from] ene_config::typed::ConfigError),
    #[error(transparent)]
    Serve(#[from] CoreError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliCommand {
    ShowConfig {
        config: Option<PathBuf>,
    },
    Serve {
        config: Option<PathBuf>,
    },
    ApproveDevice {
        config: Option<PathBuf>,
        pending: Option<String>,
    },
    ApproveCredential {
        config: Option<PathBuf>,
        provider: String,
        label: String,
    },
    PendingDeletions {
        config: Option<PathBuf>,
        after: Option<String>,
        limit: u32,
    },
    ConfirmDeletion {
        config: Option<PathBuf>,
        request: String,
    },
    DeletionStatus {
        config: Option<PathBuf>,
        cursor: Option<String>,
        limit: u32,
    },
    WorkspaceEffectWorker,
}

fn ene_core_command() -> clap::Command {
    use clap::{Arg, Command as ClapCommand};

    ClapCommand::new("ene-core")
        .version(env!("CARGO_PKG_VERSION"))
        .about("ene Host")
        .arg(
            Arg::new("config")
                .long("config")
                .value_name("PATH")
                .global(true)
                .overrides_with("config")
                .allow_hyphen_values(true)
                .help("Configuration file path"),
        )
        .subcommand(ClapCommand::new("serve").about("Run the Host listener"))
        .subcommand(
            ClapCommand::new("approve-device")
                .about("Approve one pending pairing by ID, or list pendings")
                .arg(
                    Arg::new("pending")
                        .long("pending")
                        .value_name("ID")
                        .overrides_with("pending")
                        .allow_hyphen_values(true)
                        .help("Exact pending ID to approve; omit to list pendings"),
                ),
        )
        .subcommand(
            ClapCommand::new("approve-credential")
                .about("Approve one pending credential pair")
                .arg(
                    Arg::new("provider")
                        .long("provider")
                        .value_name("P")
                        .required(true)
                        .overrides_with("provider")
                        .allow_hyphen_values(true),
                )
                .arg(
                    Arg::new("label")
                        .long("label")
                        .value_name("L")
                        .required(true)
                        .overrides_with("label")
                        .allow_hyphen_values(true),
                ),
        )
        .subcommand(
            ClapCommand::new("pending-deletions")
                .about("List Targeted Deletion requests awaiting Host-local confirmation")
                .arg(
                    Arg::new("after")
                        .long("after")
                        .value_name("ID")
                        .overrides_with("after")
                        .allow_hyphen_values(true),
                )
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .value_name("N")
                        .value_parser(clap::value_parser!(u32))
                        .default_value("50"),
                ),
        )
        .subcommand(
            ClapCommand::new("confirm-deletion")
                .about(
                    "Confirm one staged Targeted Deletion request through the running serving Host",
                )
                .arg(
                    Arg::new("request")
                        .long("request")
                        .value_name("ID")
                        .required(true)
                        .overrides_with("request")
                        .allow_hyphen_values(true),
                ),
        )
        .subcommand(
            ClapCommand::new("workspace-effect-worker")
                .hide(true)
                .about("Run the isolated workspace-effect worker"),
        )
        .subcommand(
            ClapCommand::new("deletion-status")
                .about("Show the bounded Targeted Deletion operation status")
                .arg(
                    Arg::new("cursor")
                        .long("cursor")
                        .value_name("CURSOR")
                        .overrides_with("cursor")
                        .allow_hyphen_values(true),
                )
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .value_name("N")
                        .value_parser(clap::value_parser!(u32))
                        .default_value("50"),
                ),
        )
}

fn cli_from_matches(matches: clap::ArgMatches) -> Result<CliCommand, CliError> {
    let config = matches.get_one::<String>("config").map(PathBuf::from);
    let Some((name, sub)) = matches.subcommand() else {
        return Ok(CliCommand::ShowConfig { config });
    };
    let config = config.or_else(|| sub.get_one::<String>("config").map(PathBuf::from));
    match name {
        "serve" => Ok(CliCommand::Serve { config }),
        "approve-device" => Ok(CliCommand::ApproveDevice {
            config,
            pending: sub.get_one::<String>("pending").cloned(),
        }),
        "approve-credential" => {
            let provider = sub
                .get_one::<String>("provider")
                .cloned()
                .unwrap_or_default();
            let label = sub.get_one::<String>("label").cloned().unwrap_or_default();
            if provider.trim().is_empty() || label.trim().is_empty() {
                return Err(CliError::Usage(String::from(
                    "approve-credential requires non-blank --provider and --label",
                )));
            }
            Ok(CliCommand::ApproveCredential {
                config,
                provider,
                label,
            })
        }
        "pending-deletions" => Ok(CliCommand::PendingDeletions {
            config,
            after: sub.get_one::<String>("after").cloned(),
            limit: sub.get_one::<u32>("limit").copied().unwrap_or(50),
        }),
        "confirm-deletion" => Ok(CliCommand::ConfirmDeletion {
            config,
            request: sub.get_one::<String>("request").cloned().ok_or_else(|| {
                CliError::Usage(String::from("confirm-deletion requires --request ID"))
            })?,
        }),
        "deletion-status" => Ok(CliCommand::DeletionStatus {
            config,
            cursor: sub.get_one::<String>("cursor").cloned(),
            limit: sub.get_one::<u32>("limit").copied().unwrap_or(50),
        }),
        "workspace-effect-worker" => Ok(CliCommand::WorkspaceEffectWorker),
        other => Err(CliError::Usage(format!("unknown command: {other}"))),
    }
}

fn main() -> Result<(), CliError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let matches = match ene_core_command()
        .try_get_matches_from(std::iter::once(String::from("ene-core")).chain(args))
    {
        Ok(matches) => matches,
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            error.print().map_err(|error| {
                CliError::Usage(format!("help could not be shown: {}", error.kind()))
            })?;
            return Ok(());
        }
        Err(error) => return Err(CliError::Usage(error.to_string())),
    };
    match cli_from_matches(matches)? {
        CliCommand::ApproveCredential {
            config,
            provider,
            label,
        } => {
            let data_dir = load_data_dir(config.as_deref())?;
            run_approve_credential(&data_dir, provider.trim(), label.trim())?;
            Ok(())
        }
        CliCommand::ApproveDevice { config, pending } => {
            let data_dir = load_data_dir(config.as_deref())?;
            let Some(pending) = pending else {
                list_pending_devices(&data_dir)?;
                return Ok(());
            };
            if pending.trim().is_empty() {
                return Err(CliError::Usage(
                    "approve-device requires a non-blank --pending ID".to_string(),
                ));
            }
            run_approve_device(&data_dir, pending.trim())?;
            Ok(())
        }
        CliCommand::Serve { config } => {
            let data_dir = load_data_dir(config.as_deref())?;
            run_serve(&data_dir)?;
            Ok(())
        }
        CliCommand::PendingDeletions {
            config,
            after,
            limit,
        } => {
            let data_dir = load_data_dir(config.as_deref())?;
            run_pending_deletions(&data_dir, after.as_deref(), limit)?;
            Ok(())
        }
        CliCommand::ConfirmDeletion { config, request } => {
            let data_dir = load_data_dir(config.as_deref())?;
            let request = request.trim();
            if request.is_empty() {
                return Err(CliError::Usage(
                    "confirm-deletion requires a non-blank --request ID".to_string(),
                ));
            }
            run_confirm_deletion(&data_dir, request)?;
            Ok(())
        }
        CliCommand::DeletionStatus {
            config,
            cursor,
            limit,
        } => {
            let data_dir = load_data_dir(config.as_deref())?;
            run_deletion_status(&data_dir, cursor.as_deref(), limit)?;
            Ok(())
        }
        CliCommand::WorkspaceEffectWorker => {
            ene_action::run_workspace_effect_worker();
            Ok(())
        }
        CliCommand::ShowConfig { config } => {
            let cfg = Config::load(config.as_deref())?;
            let _data_dir = ene_config::resolve_data_dir(&cfg);
            Ok(())
        }
    }
}

fn load_data_dir(config: Option<&Path>) -> Result<PathBuf, CliError> {
    let cfg = Config::load(config)?;
    ene_config::resolve_data_dir(&cfg).ok_or_else(|| {
        CliError::Serve(CoreError::Store(String::from("no data directory resolved")))
    })
}

fn block_on<F>(task: F) -> Result<(), CoreError>
where
    F: std::future::Future<Output = Result<(), CoreError>>,
{
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| CoreError::Store("tokio runtime unavailable".to_string()))?
        .block_on(task)
}

fn list_pending_devices(data_dir: &Path) -> Result<(), CoreError> {
    use std::io::Write as _;
    block_on(async {
        let handle = HostHandle::open(data_dir).await?;
        let mut pending = handle.pending_devices().await?;
        pending.sort_by(|first, second| first.pending_id.cmp(&second.pending_id));
        let mut stdout = std::io::stdout().lock();
        for entry in &pending {
            writeln!(stdout, "{} {}", entry.pending_id, entry.descriptor).map_err(|error| {
                CoreError::Store(format!("pending list could not be shown: {error}"))
            })?;
        }
        stdout.flush().map_err(|error| {
            CoreError::Store(format!("pending list could not be shown: {error}"))
        })?;
        Ok(())
    })
}

fn run_serve(data_dir: &Path) -> Result<(), CoreError> {
    block_on(serve::serve(data_dir))
}

fn run_approve_device(data_dir: &Path, pending_id: &str) -> Result<(), CoreError> {
    run_requester(
        data_dir,
        "device approval",
        ene_core::host_control::request_device_approve(data_dir, pending_id),
    )
}

fn run_requester(
    data_dir: &Path,
    what: &str,
    request: impl std::future::Future<Output = Result<ene_local_control::RequestState, CoreError>>,
) -> Result<(), CoreError> {
    use ene_core::host_lock::HostLock;

    block_on(async move {
        match HostLock::acquire(data_dir) {
            Ok(_lock) => Err(CoreError::HostUnavailable),
            Err(CoreError::AlreadyRunning) => {
                let state = request.await?;
                show_requester_state(what, &state)
            }
            Err(error) => Err(error),
        }
    })
}

fn show_requester_state(
    what: &str,
    state: &ene_local_control::RequestState,
) -> Result<(), CoreError> {
    use std::io::Write as _;

    use ene_local_control::{RequestState, RequesterOutcome};

    let line = match state {
        RequestState::AwaitingOwnerConfirmation => format!(
            "{what}: the Owner's confirmation surface has not decided yet; no change was applied"
        ),
        RequestState::ConfirmationUnavailable => {
            format!("{what}: no confirmation surface is available; no change was applied")
        }
        RequestState::Rejected => format!("{what}: the Owner declined; no change was applied"),
        RequestState::StalePremise => format!(
            "{what}: the target or its premise moved; request again against the current state"
        ),
        RequestState::OutcomeUnavailable => format!(
            "{what}: the outcome could not be read; check the current state before retrying"
        ),
        RequestState::Applied { outcome } => match outcome {
            RequesterOutcome::DeviceApproved {
                pending_id,
                device_id,
            } => format!(
                "{what}: approved pending {pending_id} as device {device_id}; the pairing \
                 client receives its own provision"
            ),
            RequesterOutcome::DeviceUnknown { pending_id } => {
                format!("{what}: pending {pending_id} is unknown to the serving Host")
            }
            RequesterOutcome::CredentialStored { provider, label } => {
                format!("{what}: {provider}:{label} is registered and active")
            }
            RequesterOutcome::CredentialRefused { provider, label } => format!(
                "{what}: {provider}:{label} was not stored; the value is entered on the \
                 confirmation surface, never on this command line"
            ),
            RequesterOutcome::CredentialUncommitted { provider, label } => format!(
                "{what}: {provider}:{label} reached the OS store, but registration did not \
                 commit; inspect the pending state before retrying, and do not re-send the value"
            ),
            RequesterOutcome::Deletion(outcome) => {
                format!("{what}: the deletion request settled as {outcome:?}")
            }
        },
    };
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{line}")
        .and_then(|()| stdout.flush())
        .map_err(|error| CoreError::Approve(format!("the outcome could not be shown: {error}")))
}

fn run_approve_credential(data_dir: &Path, provider: &str, label: &str) -> Result<(), CoreError> {
    run_requester(
        data_dir,
        "credential registration",
        ene_core::host_control::request_credential_put(data_dir, provider, label),
    )
}

fn run_pending_deletions(
    data_dir: &Path,
    after: Option<&str>,
    limit: u32,
) -> Result<(), CoreError> {
    use std::io::Write as _;

    if !(1..=100).contains(&limit) {
        return Err(CoreError::Deletion(String::from(
            "pending-deletions limit must be 1..=100",
        )));
    }
    let after = match after {
        None => None,
        Some(raw) => Some(parse_deletion_request_id(raw)?),
    };
    block_on(async move {
        let handle = HostHandle::open(data_dir).await?;
        let pending = handle.pending_targeted_deletions(after, limit).await?;
        let mut stdout = std::io::stdout().lock();
        for request in &pending {
            writeln!(
                stdout,
                "{} {} {}",
                raw_id_text(request.request().as_raw()),
                request.purpose().as_str(),
                request.owner_review_text()
            )
            .map_err(|error| {
                CoreError::Store(format!("pending deletions could not be shown: {error}"))
            })?;
        }
        stdout.flush().map_err(|error| {
            CoreError::Store(format!("pending deletions could not be shown: {error}"))
        })?;
        Ok(())
    })
}

fn run_confirm_deletion(data_dir: &Path, request: &str) -> Result<(), CoreError> {
    use std::io::Write as _;

    use ene_preservation::ConfirmTargetedDeletionOutcome;

    let request_id = parse_deletion_request_id(request)?;
    block_on(async move {
        let outcome = ene_core::host_control::confirm_targeted_deletion(
            data_dir,
            &raw_id_text(request_id.as_raw()),
        )
        .await?;
        let mut stdout = std::io::stdout().lock();
        let line = match outcome {
            ConfirmTargetedDeletionOutcome::Started(operation) => format!(
                "started {} sweep {}",
                raw_id_text(operation.operation.as_raw()),
                operation.sweep.as_u64()
            ),
            ConfirmTargetedDeletionOutcome::AlreadyCoveredBy(operation) => format!(
                "already covered by {} sweep {}",
                raw_id_text(operation.operation.as_raw()),
                operation.sweep.as_u64()
            ),
            ConfirmTargetedDeletionOutcome::HeldByOperation(operation) => format!(
                "held by {} sweep {}",
                raw_id_text(operation.operation.as_raw()),
                operation.sweep.as_u64()
            ),
            ConfirmTargetedDeletionOutcome::NeedsClarification => {
                return Err(CoreError::Deletion(String::from(
                    "the staged target is not admissible",
                )));
            }
            ConfirmTargetedDeletionOutcome::Missing => {
                let handle = HostHandle::open(data_dir).await?;
                let pending = handle.pending_targeted_deletions(None, 100).await?;
                return Err(CoreError::Deletion(format!(
                    "unknown deletion request {request:?}; pending: [{}]",
                    pending
                        .iter()
                        .map(|request| raw_id_text(request.request().as_raw()))
                        .collect::<Vec<String>>()
                        .join(", ")
                )));
            }
        };
        writeln!(stdout, "{line}").map_err(|error| {
            CoreError::Store(format!("the confirmation could not be shown: {error}"))
        })?;
        stdout.flush().map_err(|error| {
            CoreError::Store(format!("the confirmation could not be shown: {error}"))
        })?;
        Ok(())
    })
}

fn run_deletion_status(data_dir: &Path, cursor: Option<&str>, limit: u32) -> Result<(), CoreError> {
    use std::io::Write as _;

    use ene_api::v1::deletion::DeletionParticipantReportWire;

    block_on(async move {
        let handle = HostHandle::open(data_dir).await?;
        let page = handle.deletion_status_page(cursor, limit).await?;
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "mark {}", page.mark.0).map_err(|error| {
            CoreError::Store(format!("deletion status could not be shown: {error}"))
        })?;
        for operation in &page.operations {
            let hold = operation
                .hold
                .map_or_else(|| String::from("-"), |hold| hold.as_str().to_string());
            let participants = match &operation.participants {
                DeletionParticipantReportWire::NotReported => String::from("not-reported"),
                DeletionParticipantReportWire::Reported(entries) => entries.len().to_string(),
            };
            writeln!(
                stdout,
                "{} {} {} sweep={} started={} hold={} participants={}",
                operation.operation.0,
                operation.phase.as_str(),
                operation.purpose.as_str(),
                operation.sweep,
                operation.started_at,
                hold,
                participants
            )
            .map_err(|error| {
                CoreError::Store(format!("deletion status could not be shown: {error}"))
            })?;
        }
        if let Some(next) = &page.next_cursor {
            writeln!(stdout, "next {}", next.0).map_err(|error| {
                CoreError::Store(format!("deletion status could not be shown: {error}"))
            })?;
        }
        stdout.flush().map_err(|error| {
            CoreError::Store(format!("deletion status could not be shown: {error}"))
        })?;
        Ok(())
    })
}

fn parse_deletion_request_id(raw: &str) -> Result<ene_preservation::DeletionRequestId, CoreError> {
    uuid::Uuid::parse_str(raw.trim())
        .map(|id| {
            ene_preservation::DeletionRequestId::from_raw(ene_primitive::RawId::from_uuid(id))
        })
        .map_err(|_| CoreError::Deletion(String::from("request ID is not a canonical UUID")))
}

fn raw_id_text(raw: ene_primitive::RawId) -> String {
    raw.as_uuid().as_hyphenated().to_string()
}
