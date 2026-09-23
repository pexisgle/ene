//! Host composition root (`Stage 2` entrypoint): wiring and lifecycle only.
//!
//! It performs no semantic judgment and owns no domain state beyond the
//! [`ene_core::serve::HostHandle`] it builds in `serve` mode; the domain
//! pipelines live in the library modules.
//!
//! With no subcommand the entrypoint keeps the `Stage 1` behavior: parse
//! arguments, [`Config::load`] (validation included), and
//! [`ene_config::resolve_data_dir`] proof without effects. With the `serve`
//! subcommand it resolves the data directory and blocks on
//! [`ene_core::serve::serve`]: the Unix socket listener serving the full
//! orchestration pipeline. With the `approve-device` subcommand it resolves
//! the data directory and records one Owner pairing approval through
//! [`HostHandle::approve_device`](ene_core::serve::HostHandle::approve_device):
//! the Host-local trusted inlet for pending device requests. With the
//! `confirm-deletion` subcommand it dials the serving Host's Host-local
//! control inlet ([`ene_core::host_control`]) instead of opening the state
//! offline: the confirmation must execute where the Client delivery tracking
//! lives (lifecycle §8.1).

use std::path::{Path, PathBuf};

use ene_config::Config;
use ene_core::serve::{self, CoreError, HostHandle};

#[derive(Debug, thiserror::Error)]
enum CliError {
    /// Argument misuse. The display carries `clap`'s own usage text plus the
    /// operational detail; domain validation (blank values, unknown
    /// combinations) adds its message here without re-implementing argv
    /// syntax.
    #[error("{0}")]
    Usage(String),
    #[error(transparent)]
    Config(#[from] ene_config::typed::ConfigError),
    #[error(transparent)]
    Serve(#[from] CoreError),
}

/// Parsed Host command line: exactly one mode plus its flags.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CliCommand {
    /// No subcommand: Stage 1 config proof without effects.
    ShowConfig {
        config: Option<PathBuf>,
    },
    Serve {
        config: Option<PathBuf>,
    },
    /// A [`None`] pending id lists pendings instead of approving.
    ApproveDevice {
        config: Option<PathBuf>,
        pending: Option<String>,
    },
    ApproveCredential {
        config: Option<PathBuf>,
        provider: String,
        label: String,
    },
    /// Targeted Deletion requests awaiting the Host-local trusted
    /// confirmation; the exact target is shown here only (IPC §18.1 preview).
    PendingDeletions {
        config: Option<PathBuf>,
        after: Option<String>,
        limit: u32,
    },
    /// The Owner's final confirmation for one staged Targeted Deletion
    /// request: it runs inside the serving Host via the Host-local control
    /// inlet and starts the canonical operation (IPC §18.1, lifecycle
    /// §8.1). A stopped Host cannot confirm: an offline handle cannot name
    /// the Clients that may hold a target-bearing copy.
    ConfirmDeletion {
        config: Option<PathBuf>,
        request: String,
    },
    /// The same bounded deletion status page the wire view renders.
    DeletionStatus {
        config: Option<PathBuf>,
        cursor: Option<String>,
        limit: u32,
    },
}

/// The declarative Host command line: subcommands, flags, help, and version
/// come from `clap`. The mapping below turns parsed words into [`CliCommand`]
/// and keeps domain validation (non-blank values, mode combinations) in this
/// binary.
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
    // A global `--config` after the subcommand lands on the subcommand's
    // matches; either placement selects the same file.
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
        other => Err(CliError::Usage(format!("unknown command: {other}"))),
    }
}

/// `Stage 2` Host entrypoint: parse arguments, load configuration, then stop,
/// serve, or approve.
///
/// Without a subcommand this keeps the `Stage 1` behavior: [`Config::load`]
/// (which validates), and [`ene_config::resolve_data_dir`] proof with no
/// effects. With `serve` it resolves the data directory (which must exist
/// as a value: an unresolvable directory is a [`CoreError::Store`] failure,
/// since serving without durable state is meaningless) and blocks on
/// [`serve::serve`] under a multi-threaded `Tokio` runtime. With
/// `approve-device` it resolves the data directory the same way and records
/// one Owner pairing approval for the exact `--descriptor` value (surrounding
/// whitespace trimmed, matching wire ingress normalization).
///
/// `--help` and `--version` are standard successful exits handled by `clap`
/// before configuration is loaded, so they have no side effects.
///
/// # Errors
///
/// Returns [`CliError::Usage`] for argument misuse, [`CliError::Config`] when
/// [`Config::load`] fails, and [`CliError::Serve`] when `serve` mode fails.
fn main() -> Result<(), CliError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let matches = match ene_core_command()
        .try_get_matches_from(std::iter::once(String::from("ene-core")).chain(args))
    {
        Ok(matches) => matches,
        // `--help` / `--version` are standard successful exits, never errors
        // and never reach configuration or the store.
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
            let cfg = Config::load(config.as_deref())?;
            let Some(data_dir) = ene_config::resolve_data_dir(&cfg) else {
                return Err(CoreError::Store("no data directory resolved".to_string()).into());
            };
            run_approve_credential(&data_dir, provider.trim(), label.trim())?;
            Ok(())
        }
        CliCommand::ApproveDevice { config, pending } => {
            let cfg = Config::load(config.as_deref())?;
            let Some(data_dir) = ene_config::resolve_data_dir(&cfg) else {
                return Err(CoreError::Store("no data directory resolved".to_string()).into());
            };
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
            let cfg = Config::load(config.as_deref())?;
            let Some(data_dir) = ene_config::resolve_data_dir(&cfg) else {
                return Err(CoreError::Store("no data directory resolved".to_string()).into());
            };
            run_serve(&data_dir)?;
            Ok(())
        }
        CliCommand::PendingDeletions {
            config,
            after,
            limit,
        } => {
            let cfg = Config::load(config.as_deref())?;
            let Some(data_dir) = ene_config::resolve_data_dir(&cfg) else {
                return Err(CoreError::Store("no data directory resolved".to_string()).into());
            };
            run_pending_deletions(&data_dir, after.as_deref(), limit)?;
            Ok(())
        }
        CliCommand::ConfirmDeletion { config, request } => {
            let cfg = Config::load(config.as_deref())?;
            let Some(data_dir) = ene_config::resolve_data_dir(&cfg) else {
                return Err(CoreError::Store("no data directory resolved".to_string()).into());
            };
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
            let cfg = Config::load(config.as_deref())?;
            let Some(data_dir) = ene_config::resolve_data_dir(&cfg) else {
                return Err(CoreError::Store("no data directory resolved".to_string()).into());
            };
            run_deletion_status(&data_dir, cursor.as_deref(), limit)?;
            Ok(())
        }
        CliCommand::ShowConfig { config } => {
            let cfg = Config::load(config.as_deref())?;
            let _data_dir = ene_config::resolve_data_dir(&cfg);
            Ok(())
        }
    }
}

/// Builds the multi-threaded `Tokio` runtime the store-backed tasks run on
/// and blocks on `task`.
///
/// A runtime that cannot be built is a [`CoreError::Store`] failure: the
/// runtime is the async substrate of the store-backed Host, and no narrower
/// variant names it.
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

/// Prints what `approve-device --pending` would accept, one
/// `<pending-id> <descriptor>` line per pending (the descriptor is display
/// only; approval names the id). Empty output (exit 0) means nothing is
/// pending.
///
/// # Errors
///
/// Returns [`CoreError::Store`] when the runtime cannot be built or the
/// state cannot be opened.
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

/// `Stage 2` listener entry: binds the Host on the resolved data directory.
///
/// # Errors
///
/// Returns [`CoreError::Store`] when the runtime cannot be built and the
/// [`serve::serve`] error otherwise.
fn run_serve(data_dir: &Path) -> Result<(), CoreError> {
    block_on(serve::serve(data_dir))
}

/// An unknown pending id fails with the pending id set so the Owner
/// can retry with the exact value; descriptors are display strings only.
///
/// The one-time pairing secret prints once to this Host-local console, the
/// trusted inlet, and nowhere else; the operator provisions it into the
/// client's protected device file.
///
/// This is an offline mutation when no Host is serving, so it takes the
/// single-writer [`HostLock`](ene_core::host_lock::HostLock) before opening
/// the store (PR §6.4). While a Host is serving, the command speaks the
/// Host-local control inlet instead. An occupied seat fails; the command
/// never falls through to the Client channel.
///
/// # Errors
///
/// Returns [`CoreError::AlreadyRunning`] only when the lock is held and the
/// control inlet is not this path's concern; serving occupancy is
/// [`CoreError::SeatOccupied`]. [`CoreError::Store`] when the runtime cannot
/// be built or the state cannot be opened, and [`CoreError::Approve`] when
/// the pending id is unknown.
fn run_approve_device(data_dir: &Path, pending_id: &str) -> Result<(), CoreError> {
    use ene_core::host_lock::HostLock;

    block_on(async {
        match HostLock::acquire(data_dir) {
            Ok(_lock) => Err(host_not_serving()),
            Err(CoreError::AlreadyRunning) => {
                let state =
                    ene_core::host_control::request_device_approve(data_dir, pending_id).await?;
                show_requester_state("device approval", &state)
            }
            Err(error) => Err(error),
        }
    })
}

/// The requester-only refusal when no Host is serving.
///
/// The Owner's confirmation surface lives in the serving process, so an
/// offline command can never record a confirmation; the old offline mutation
/// fallback is deliberately gone (first-party-desktop §5.1.5).
fn host_not_serving() -> CoreError {
    CoreError::Approve(String::from(
        "the Host is not serving; start `ene-core serve` and retry — the Owner's \
         confirmation surface runs there, and an offline command cannot record one",
    ))
}

/// Shows one requester request's settled state. Secrets never appear here: the
/// pairing provision and the credential value belong to their own channels.
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

/// Unknown pairs fail with the pending set so the Owner can retry exactly.
///
/// Offline (no serving Host) this takes [`HostLock`] before opening the
/// store. While serving, the command puts the bearer over the control inlet
/// (`ENE_OPENAI_API_KEY`); an occupied seat fails and never falls through
/// to the Client channel.
///
/// # Errors
///
/// [`CoreError::SeatOccupied`] when the control seat is held,
/// [`CoreError::Store`] when the runtime cannot be built or the state cannot
/// be opened, and [`CoreError::Approve`] when the pair is unknown or the
/// serving-time secret is missing.
fn run_approve_credential(data_dir: &Path, provider: &str, label: &str) -> Result<(), CoreError> {
    use ene_core::host_lock::HostLock;

    block_on(async {
        match HostLock::acquire(data_dir) {
            Ok(_lock) => Err(host_not_serving()),
            Err(CoreError::AlreadyRunning) => {
                let state =
                    ene_core::host_control::request_credential_put(data_dir, provider, label)
                        .await?;
                show_requester_state("credential registration", &state)
            }
            Err(error) => Err(error),
        }
    })
}

/// Prints the Targeted Deletion requests awaiting the Owner's confirmation,
/// one `<request-id> <purpose> <exact-text>` line each.
///
/// This is the Host-local trusted preview (IPC §18.1): the exact target text is
/// shown here, on the Owner's own console, and nowhere else. The request
/// identity is Host-minted and never travels the wire, so no Client can name —
/// let alone confirm — one.
///
/// # Errors
///
/// Returns [`CoreError::Store`] when the runtime cannot be built or the state
/// cannot be opened, and [`CoreError::Deletion`] for a malformed `--after`
/// identity.
fn run_pending_deletions(
    data_dir: &Path,
    after: Option<&str>,
    limit: u32,
) -> Result<(), CoreError> {
    use std::io::Write as _;

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
                deletion_request_id_text(request),
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

/// Records one Owner confirmation and starts the canonical Targeted Deletion
/// operation (IPC §18.1) through the serving Host's Host-local first-party
/// control inlet, then prints the operation identity the status view reports.
///
/// The confirmation must run in the serving process. The required
/// participant snapshot includes every Client incarnation with durable
/// body-delivery evidence, and only the serving process can reach those
/// incarnations through its live connection table (lifecycle §8.1); an
/// offline state open could name them but could never complete their local
/// erasure, so this command never admits from an offline handle. It dials
/// [`ene_core::host_control`] and reports the serving Host's typed outcome;
/// when no Host is serving it fails with recovery guidance instead of
/// confirming.
///
/// An unknown request id fails with the pending id set (never their target
/// text, which stays on the `pending-deletions` preview).
///
/// # Errors
///
/// Returns [`CoreError::Deletion`] when the serving Host is not reachable on
/// the control inlet (or refuses technically), for an unknown or
/// inadmissible request, and [`CoreError::Store`] when the pending-id
/// fallback cannot be read.
fn run_confirm_deletion(data_dir: &Path, request: &str) -> Result<(), CoreError> {
    use std::io::Write as _;

    use ene_preservation::ConfirmTargetedDeletionOutcome;

    let request_id = parse_deletion_request_id(request)?;
    block_on(async move {
        let outcome = ene_core::host_control::confirm_targeted_deletion(
            data_dir,
            &deletion_request_id_text_of(request_id),
        )
        .await?;
        let mut stdout = std::io::stdout().lock();
        let line = match outcome {
            ConfirmTargetedDeletionOutcome::Started(operation) => format!(
                "started {} sweep {}",
                deletion_operation_text(operation),
                operation.sweep.as_u64()
            ),
            ConfirmTargetedDeletionOutcome::AlreadyCoveredBy(operation) => format!(
                "already covered by {} sweep {}",
                deletion_operation_text(operation),
                operation.sweep.as_u64()
            ),
            ConfirmTargetedDeletionOutcome::HeldByOperation(operation) => format!(
                "held by {} sweep {}",
                deletion_operation_text(operation),
                operation.sweep.as_u64()
            ),
            ConfirmTargetedDeletionOutcome::NeedsClarification => {
                return Err(CoreError::Deletion(String::from(
                    "the staged target is not admissible",
                )));
            }
            ConfirmTargetedDeletionOutcome::Missing => {
                // A read-only state open is safe while serving; the pending
                // preview never admits anything.
                let handle = HostHandle::open(data_dir).await?;
                let pending = handle.pending_targeted_deletions(None, 100).await?;
                return Err(CoreError::Deletion(format!(
                    "unknown deletion request {request:?}; pending: [{}]",
                    pending
                        .iter()
                        .map(deletion_request_id_text)
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

/// Prints the same bounded deletion status page the wire view renders: the
/// surface mark, then one line per operation, then the next cursor while a
/// later page exists. No target body, search material, or credential appears
/// here.
///
/// # Errors
///
/// Returns [`CoreError::Store`] when the runtime cannot be built or the state
/// cannot be opened, and [`CoreError::Deletion`] for a malformed cursor or an
/// unreadable surface.
fn run_deletion_status(data_dir: &Path, cursor: Option<&str>, limit: u32) -> Result<(), CoreError> {
    use std::io::Write as _;

    use ene_api::v1::deletion::{DeletionParticipantReportWire, DeletionStatusResponse};

    block_on(async move {
        let handle = HostHandle::open(data_dir).await?;
        let response = handle.deletion_status_page(cursor, limit).await?;
        let DeletionStatusResponse::Page(page) = response else {
            return Err(CoreError::Deletion(String::from(
                "deletion status is unavailable",
            )));
        };
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

/// Parses one Host-minted deletion request identity from its rendered form.
///
/// # Errors
///
/// Returns [`CoreError::Deletion`] for anything that is not a canonical UUID
/// rendering; a request id is never guessed or defaulted.
fn parse_deletion_request_id(raw: &str) -> Result<ene_preservation::DeletionRequestId, CoreError> {
    uuid::Uuid::parse_str(raw.trim())
        .map(|id| {
            ene_preservation::DeletionRequestId::from_raw(ene_primitive::RawId::from_uuid(id))
        })
        .map_err(|_| CoreError::Deletion(String::from("request ID is not a canonical UUID")))
}

fn deletion_request_id_text(request: &ene_preservation::TargetedDeletionRequest) -> String {
    deletion_request_id_text_of(request.request())
}

fn deletion_request_id_text_of(request: ene_preservation::DeletionRequestId) -> String {
    request.as_raw().as_uuid().as_hyphenated().to_string()
}

fn deletion_operation_text(operation: ene_preservation::DeletionOperationRef) -> String {
    operation
        .operation
        .as_raw()
        .as_uuid()
        .as_hyphenated()
        .to_string()
}
