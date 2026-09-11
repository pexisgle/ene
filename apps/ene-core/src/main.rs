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
//! the Host-local trusted inlet for pending device requests.

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
    /// A [`None`] descriptor lists pendings instead of approving.
    ApproveDevice {
        config: Option<PathBuf>,
        descriptor: Option<String>,
    },
    ApproveCredential {
        config: Option<PathBuf>,
        provider: String,
        label: String,
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
                .about("Approve one pending device descriptor, or list pendings")
                .arg(
                    Arg::new("descriptor")
                        .long("descriptor")
                        .value_name("EXACT")
                        .overrides_with("descriptor")
                        .allow_hyphen_values(true)
                        .help("Exact pending descriptor to approve; omit to list pendings"),
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
            descriptor: sub.get_one::<String>("descriptor").cloned(),
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
        CliCommand::ApproveDevice { config, descriptor } => {
            let cfg = Config::load(config.as_deref())?;
            let Some(data_dir) = ene_config::resolve_data_dir(&cfg) else {
                return Err(CoreError::Store("no data directory resolved".to_string()).into());
            };
            let Some(descriptor) = descriptor else {
                list_pending_devices(&data_dir)?;
                return Ok(());
            };
            if descriptor.trim().is_empty() {
                return Err(CliError::Usage(
                    "approve-device requires a non-blank --descriptor".to_string(),
                ));
            }
            run_approve_device(&data_dir, descriptor.trim())?;
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

/// Prints what `approve-device --descriptor` would accept, one descriptor per
/// line. Empty output (exit 0) means nothing is pending.
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
        pending.sort();
        let mut stdout = std::io::stdout().lock();
        for descriptor in &pending {
            writeln!(stdout, "{descriptor}").map_err(|error| {
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

/// An unknown descriptor fails with the pending descriptor set so the Owner
/// can retry with the exact value; descriptors are display strings only.
///
/// The one-time pairing secret prints once to this Host-local console, the
/// trusted inlet, and nowhere else; the operator provisions it into the
/// client's protected device file.
///
/// # Errors
///
/// Returns [`CoreError::Store`] when the runtime cannot be built or the state
/// cannot be opened, and [`CoreError::Approve`] when the descriptor is
/// unknown (listing the pending descriptors) or the approval write fails.
fn run_approve_device(data_dir: &Path, descriptor: &str) -> Result<(), CoreError> {
    use std::io::Write as _;
    block_on(async {
        let handle = HostHandle::open(data_dir).await?;
        if let Some((_, secret)) = handle.approve_device(descriptor).await? {
            let mut stdout = std::io::stdout().lock();
            writeln!(stdout, "pairing secret (show once): {secret}").map_err(|error| {
                CoreError::Store(format!(
                    "approved, but the secret could not be shown: {error}"
                ))
            })?;
            stdout.flush().map_err(|error| {
                CoreError::Store(format!(
                    "approved, but the secret could not be shown: {error}"
                ))
            })?;
            return Ok(());
        }
        let pending = handle.pending_devices().await?;
        Err(CoreError::Approve(format!(
            "unknown device descriptor {descriptor:?}; pending: [{pending}]",
            pending = pending.join(", ")
        )))
    })
}

/// Unknown pairs fail with the pending set so the Owner can retry exactly.
///
/// # Errors
///
/// Returns [`CoreError::Store`] when the runtime cannot be built or the
/// state cannot be opened, and [`CoreError::Approve`] when the pair is
/// unknown.
fn run_approve_credential(data_dir: &Path, provider: &str, label: &str) -> Result<(), CoreError> {
    block_on(async {
        let handle = HostHandle::open(data_dir).await?;
        if handle.approve_credential(provider, label).await? {
            return Ok(());
        }
        let pending = handle.pending_credentials().await?;
        Err(CoreError::Approve(format!(
            "unknown credential {provider}:{label}; pending: [{pending}]",
            pending = pending.join(", ")
        )))
    })
}

#[cfg(test)]
mod tests {
    use super::{CliCommand, cli_from_matches, ene_core_command};
    use std::path::PathBuf;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    fn parse(words: &[&str]) -> Result<CliCommand, super::CliError> {
        let matches = ene_core_command()
            .try_get_matches_from(std::iter::once(String::from("ene-core")).chain(args(words)))
            .map_err(|error| super::CliError::Usage(error.to_string()))?;
        cli_from_matches(matches)
    }

    fn clap_error(words: &[&str]) -> clap::error::ErrorKind {
        match ene_core_command()
            .try_get_matches_from(std::iter::once(String::from("ene-core")).chain(args(words)))
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
        assert!(matches!(
            clap_error(&["approve-device", "--help"]),
            clap::error::ErrorKind::DisplayHelp
        ));
    }

    #[test]
    fn no_args_yields_no_override() {
        let parsed = parse(&[]).expect("no args must succeed");
        assert_eq!(parsed, CliCommand::ShowConfig { config: None });
    }

    #[test]
    fn config_flag_captures_its_value_verbatim() {
        let parsed = parse(&["--config", "/tmp/ene.json"]).expect("--config must succeed");
        assert_eq!(
            parsed,
            CliCommand::ShowConfig {
                config: Some(PathBuf::from("/tmp/ene.json"))
            }
        );
        let hyphen = parse(&["--config", "--odd"]).expect("a hyphen value must be consumed");
        assert_eq!(
            hyphen,
            CliCommand::ShowConfig {
                config: Some(PathBuf::from("--odd"))
            }
        );
    }

    #[test]
    fn missing_config_value_is_a_usage_error() {
        assert!(matches!(
            parse(&["--config"]),
            Err(super::CliError::Usage(_))
        ));
    }

    #[test]
    fn unknown_argument_is_a_usage_error() {
        assert!(matches!(
            parse(&["--verbose"]),
            Err(super::CliError::Usage(_))
        ));
    }

    #[test]
    fn repeated_config_keeps_the_last_value() {
        let parsed = parse(&[
            "--config",
            "/tmp/first.json",
            "--config",
            "/tmp/second.json",
        ])
        .expect("a repeated --config must succeed");
        assert_eq!(
            parsed,
            CliCommand::ShowConfig {
                config: Some(PathBuf::from("/tmp/second.json"))
            }
        );
    }

    #[test]
    fn serve_parses_in_any_position() {
        for words in [
            &["serve"][..],
            &["serve", "--config", "/tmp/e.json"][..],
            &["--config", "/tmp/e.json", "serve"][..],
        ] {
            let parsed = parse(words).expect("serve must parse");
            let config = match parsed {
                CliCommand::Serve { config } => config,
                other => panic!("expected serve, got {other:?}"),
            };
            if words.contains(&"--config") {
                assert_eq!(config, Some(PathBuf::from("/tmp/e.json")));
            } else {
                assert_eq!(config, None);
            }
        }
    }

    #[test]
    fn repeated_serve_is_a_usage_error() {
        assert!(matches!(
            parse(&["serve", "serve"]),
            Err(super::CliError::Usage(_))
        ));
    }

    #[test]
    fn combined_subcommands_are_rejected() {
        assert!(matches!(
            parse(&["serve", "approve-device"]),
            Err(super::CliError::Usage(_))
        ));
    }

    #[test]
    fn config_value_named_serve_is_not_a_subcommand() {
        let parsed = parse(&["--config", "serve"]).expect("the value is data");
        assert_eq!(
            parsed,
            CliCommand::ShowConfig {
                config: Some(PathBuf::from("serve"))
            }
        );
    }

    #[test]
    fn descriptor_value_named_serve_is_not_a_subcommand() {
        let parsed =
            parse(&["approve-device", "--descriptor", "serve"]).expect("the value is data");
        assert_eq!(
            parsed,
            CliCommand::ApproveDevice {
                config: None,
                descriptor: Some(String::from("serve"))
            }
        );
    }

    #[test]
    fn descriptor_flag_captures_its_value_verbatim() {
        let parsed = parse(&["approve-device", "--descriptor", "--odd-value"])
            .expect("--descriptor with a value must parse");
        assert_eq!(
            parsed,
            CliCommand::ApproveDevice {
                config: None,
                descriptor: Some(String::from("--odd-value"))
            },
            "the value is consumed verbatim, even with a leading --"
        );
    }

    #[test]
    fn repeated_descriptor_keeps_the_last_value() {
        let parsed = parse(&[
            "approve-device",
            "--descriptor",
            "first",
            "--descriptor",
            "second",
        ])
        .expect("a repeated --descriptor must parse");
        assert_eq!(
            parsed,
            CliCommand::ApproveDevice {
                config: None,
                descriptor: Some(String::from("second"))
            }
        );
    }

    #[test]
    fn missing_descriptor_value_is_a_usage_error() {
        assert!(matches!(
            parse(&["approve-device", "--descriptor"]),
            Err(super::CliError::Usage(_))
        ));
    }

    #[test]
    fn approve_device_without_descriptor_lists_pendings() {
        let parsed = parse(&["approve-device"]).expect("approve-device must parse");
        assert_eq!(
            parsed,
            CliCommand::ApproveDevice {
                config: None,
                descriptor: None
            }
        );
    }

    #[test]
    fn approve_credential_requires_a_non_blank_pair() {
        let parsed = parse(&[
            "approve-credential",
            "--provider",
            "openai",
            "--label",
            "main",
        ])
        .expect("a complete pair must parse");
        assert_eq!(
            parsed,
            CliCommand::ApproveCredential {
                config: None,
                provider: String::from("openai"),
                label: String::from("main"),
            }
        );
        assert!(matches!(
            parse(&[
                "approve-credential",
                "--provider",
                "openai",
                "--label",
                "  "
            ]),
            Err(super::CliError::Usage(_))
        ));
        assert!(matches!(
            parse(&["approve-credential", "--provider", "openai"]),
            Err(super::CliError::Usage(_))
        ));
        assert!(matches!(
            parse(&[
                "approve-credential",
                "--provider",
                "openai",
                "--label",
                "main",
                "--descriptor",
                "x"
            ]),
            Err(super::CliError::Usage(_))
        ));
    }

    #[test]
    fn flags_before_the_subcommand_still_parse() {
        let parsed = parse(&[
            "--config",
            "/tmp/e.json",
            "approve-device",
            "--descriptor",
            "laptop",
        ])
        .expect("global flags and subcommand options must parse");
        assert_eq!(
            parsed,
            CliCommand::ApproveDevice {
                config: Some(PathBuf::from("/tmp/e.json")),
                descriptor: Some(String::from("laptop"))
            }
        );
    }

    #[test]
    fn stray_mode_flags_are_rejected() {
        assert!(matches!(
            parse(&["serve", "--descriptor", "laptop"]),
            Err(super::CliError::Usage(_))
        ));
        assert!(matches!(
            parse(&["approve-device", "--provider", "openai"]),
            Err(super::CliError::Usage(_))
        ));
    }

    #[test]
    fn unknown_trailing_arguments_are_rejected() {
        assert!(matches!(
            parse(&["serve", "extra"]),
            Err(super::CliError::Usage(_))
        ));
    }
}
