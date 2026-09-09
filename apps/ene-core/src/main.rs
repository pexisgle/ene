//! Host composition root (`Stage 2` entrypoint).
//!
//! `ene-core` is the Host composition root: wiring and lifecycle only. It
//! performs no semantic judgment and owns no domain state beyond the
//! [`ene_core::serve::HostHandle`] it builds in `serve` mode; the domain
//! pipelines live in the library modules, and this binary holds argument
//! parsing plus process setup.
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

/// Command-line failure for the Host entrypoint.
///
/// [`CliError::Usage`] covers argument misuse; [`CliError::Config`] carries a
/// [`ene_config::typed::ConfigError`] from [`Config::load`] unchanged, and
/// [`CliError::Serve`] carries a [`CoreError`] from `serve` mode unchanged.
#[derive(Debug, thiserror::Error)]
enum CliError {
    /// Command-line usage was violated.
    ///
    /// The display always contains the usage line
    /// `usage: ene-core [--config PATH] [serve | approve-device --descriptor EXACT | approve-credential --provider P --label L]`
    /// followed by the detail, so callers can assert on the usage line alone.
    /// The bracketed prefix keeps the `Stage 1` usage line as a substring.
    #[error(
        "usage: ene-core [--config PATH] [serve | approve-device --descriptor EXACT | approve-credential --provider P --label L]: {0}"
    )]
    Usage(String),
    /// Layered configuration loading or validation failed.
    #[error(transparent)]
    Config(#[from] ene_config::typed::ConfigError),
    /// `serve` mode failed.
    #[error(transparent)]
    Serve(#[from] CoreError),
}

/// Parses Host command-line arguments, excluding the program name.
///
/// Accepts exactly one form: `--config PATH`, which selects an explicit JSON
/// configuration file. With no arguments there is no override and the result
/// is [`None`]. A repeated `--config` keeps the last value; later flags
/// override earlier ones, matching the usual override convention. The value
/// following `--config` is consumed verbatim, even when it starts with `--`.
/// A missing value after `--config` and any unknown argument (including
/// `--help` and `--version`) are [`CliError::Usage`] failures whose display
/// contains the usage line.
///
/// The function is pure: it inspects only `args` and never touches the
/// process environment, the filesystem, or `stdout`.
///
/// # Errors
///
/// Returns [`CliError::Usage`] when `--config` has no following value or when
/// any other argument is present.
fn parse_args(args: &[String]) -> Result<Option<PathBuf>, CliError> {
    let mut config: Option<PathBuf> = None;
    let mut pending = args.iter();
    while let Some(arg) = pending.next() {
        if arg.as_str() == "--config" {
            let Some(value) = pending.next() else {
                return Err(CliError::Usage("missing value for --config".to_string()));
            };
            config = Some(PathBuf::from(value));
        } else {
            return Err(CliError::Usage(format!("unknown argument: {arg}")));
        }
    }
    Ok(config)
}

/// Splits the `serve` subcommand off the argument list, order-independent.
///
/// Scans `args` for exactly one `serve` token and returns it separately from
/// the remaining arguments (which [`parse_args`] then parses for `--config`).
/// Both `ene-core serve --config PATH` and `ene-core --config PATH serve`
/// work; a repeated `serve` is a [`CliError::Usage`] failure.
///
/// The function is pure: it inspects only `args` and never touches the
/// process environment, the filesystem, or `stdout`.
///
/// # Errors
///
/// Returns [`CliError::Usage`] when `serve` appears more than once.
fn extract_serve(args: &[String]) -> Result<(bool, Vec<String>), CliError> {
    let mut serve = false;
    let mut rest = Vec::new();
    for arg in args {
        if arg.as_str() == "serve" {
            if serve {
                return Err(CliError::Usage("duplicate subcommand: serve".to_string()));
            }
            serve = true;
        } else {
            rest.push(arg.clone());
        }
    }
    Ok((serve, rest))
}

/// Splits one `--name VALUE` flag out of the argument list, order-independent.
///
/// Later flags override earlier ones. The value following the flag is
/// consumed verbatim, even when it starts with `--`.
///
/// # Errors
///
/// Returns [`CliError::Usage`] when the flag has no following value.
fn extract_named(args: &[String], flag: &str) -> Result<(Option<String>, Vec<String>), CliError> {
    let mut value: Option<String> = None;
    let mut rest = Vec::new();
    let mut pending = args.iter();
    while let Some(arg) = pending.next() {
        if arg.as_str() == flag {
            let Some(next) = pending.next() else {
                return Err(CliError::Usage(format!("missing value for {flag}")));
            };
            value = Some(next.clone());
        } else {
            rest.push(arg.clone());
        }
    }
    Ok((value, rest))
}

/// Splits the `approve-credential` subcommand off the argument list,
/// order-independent, mirroring [`extract_approve_device`].
///
/// # Errors
///
/// Returns [`CliError::Usage`] when `approve-credential` appears more than once.
fn extract_approve_credential(args: &[String]) -> Result<(bool, Vec<String>), CliError> {
    let mut approve = false;
    let mut rest = Vec::new();
    for arg in args {
        if arg.as_str() == "approve-credential" {
            if approve {
                return Err(CliError::Usage(
                    "duplicate subcommand: approve-credential".to_string(),
                ));
            }
            approve = true;
        } else {
            rest.push(arg.clone());
        }
    }
    Ok((approve, rest))
}

/// Splits the `approve-device` subcommand off the argument list, order-independent.
///
/// Scans `args` for exactly one `approve-device` token and returns it
/// separately from the remaining arguments (which [`parse_args`] then parses
/// for `--config` and [`extract_descriptor`] parses for `--descriptor`).
/// Both `ene-core approve-device --descriptor EXACT` and
/// `ene-core --descriptor EXACT approve-device` work; a repeated
/// `approve-device` is a [`CliError::Usage`] failure.
///
/// The function is pure: it inspects only `args` and never touches the
/// process environment, the filesystem, or `stdout`.
///
/// # Errors
///
/// Returns [`CliError::Usage`] when `approve-device` appears more than once.
fn extract_approve_device(args: &[String]) -> Result<(bool, Vec<String>), CliError> {
    let mut approve = false;
    let mut rest = Vec::new();
    for arg in args {
        if arg.as_str() == "approve-device" {
            if approve {
                return Err(CliError::Usage(
                    "duplicate subcommand: approve-device".to_string(),
                ));
            }
            approve = true;
        } else {
            rest.push(arg.clone());
        }
    }
    Ok((approve, rest))
}

/// Splits `--descriptor VALUE` out of the argument list.
///
/// Scans `args` for `--descriptor` flags and returns the last value
/// separately from the remaining arguments (which [`parse_args`] then parses
/// for `--config`). The value following `--descriptor` is consumed verbatim,
/// even when it starts with `--`. Later flags override earlier ones, matching
/// the usual override convention.
///
/// The function is pure: it inspects only `args` and never touches the
/// process environment, the filesystem, or `stdout`.
///
/// # Errors
///
/// Returns [`CliError::Usage`] when `--descriptor` has no following value.
fn extract_descriptor(args: &[String]) -> Result<(Option<String>, Vec<String>), CliError> {
    let mut descriptor: Option<String> = None;
    let mut rest = Vec::new();
    let mut pending = args.iter();
    while let Some(arg) = pending.next() {
        if arg.as_str() == "--descriptor" {
            let Some(value) = pending.next() else {
                return Err(CliError::Usage(
                    "missing value for --descriptor".to_string(),
                ));
            };
            descriptor = Some(value.clone());
        } else {
            rest.push(arg.clone());
        }
    }
    Ok((descriptor, rest))
}

/// `Stage 2` Host entrypoint: parse arguments, load configuration, then stop,
/// serve, or approve.
///
/// Without a subcommand this keeps the `Stage 1` behavior: [`parse_args`],
/// [`Config::load`] (which validates), and [`ene_config::resolve_data_dir`]
/// proof with no effects. With `serve` it resolves the data directory (which
/// must exist as a value: an unresolvable directory is a [`CoreError::Store`]
/// failure, since serving without durable state is meaningless) and blocks on
/// Parsed Host command line: exactly one mode plus its flags.
///
/// Built purely by [`parse_cli`]; [`main`] only executes it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CliCommand {
    /// No subcommand: Stage 1 config proof without effects.
    ShowConfig {
        /// Explicit config file, if `--config PATH` was given.
        config: Option<PathBuf>,
    },
    /// Run the socket listener.
    Serve {
        /// Explicit config file, if `--config PATH` was given.
        config: Option<PathBuf>,
    },
    /// Pairing approval. [`None`] descriptor lists pendings.
    ApproveDevice {
        /// Explicit config file, if `--config PATH` was given.
        config: Option<PathBuf>,
        /// Exact device descriptor, if `--descriptor EXACT` was given.
        descriptor: Option<String>,
    },
    /// Credential approval.
    ApproveCredential {
        /// Explicit config file, if `--config PATH` was given.
        config: Option<PathBuf>,
        /// Credential provider to approve.
        provider: String,
        /// Credential label to approve.
        label: String,
    },
}

/// Parses Host command-line arguments, excluding the program name.
///
/// Option flags (with their values) are extracted FIRST and subcommand
/// tokens are scanned only in the remainder, so a legal value is never
/// mistaken for a subcommand: `ene-core --config serve` selects config
/// path `serve`, and `approve-device --descriptor serve` approves the
/// `serve` descriptor. Flags for a different mode are rejected rather
/// than silently ignored, keeping the previous strictness. The value
/// following a flag is still consumed verbatim, even when it starts with
/// `--` (see [`extract_named`]).
///
/// The function is pure: it inspects only `args` and never touches the
/// process environment, the filesystem, or `stdout`.
///
/// # Errors
///
/// Returns [`CliError::Usage`] for argument misuse (duplicate or combined
/// subcommands, missing or blank required values, flags belonging to
/// another mode, or unknown trailing arguments).
fn parse_cli(args: &[String]) -> Result<CliCommand, CliError> {
    let (config, rest) = extract_named(args, "--config")?;
    let (descriptor, rest) = extract_descriptor(&rest)?;
    let (provider, rest) = extract_named(&rest, "--provider")?;
    let (label, rest) = extract_named(&rest, "--label")?;
    let (serve_mode, rest) = extract_serve(&rest)?;
    let (approve_mode, rest) = extract_approve_device(&rest)?;
    let (approve_cred_mode, rest) = extract_approve_credential(&rest)?;
    let modes = [serve_mode, approve_mode, approve_cred_mode]
        .iter()
        .filter(|selected| **selected)
        .count();
    if modes > 1 {
        return Err(CliError::Usage(
            "serve, approve-device, and approve-credential are mutually exclusive".to_string(),
        ));
    }
    // Anything left is an unknown argument: option values traveled with
    // their flags above, so the remainder holds no legal values.
    let path = parse_args(&rest)?;
    let config = config.map(PathBuf::from).or(path);
    if approve_cred_mode {
        let (Some(provider), Some(label)) = (provider, label) else {
            return Err(CliError::Usage(
                "approve-credential requires --provider P and --label L".to_string(),
            ));
        };
        if provider.trim().is_empty() || label.trim().is_empty() {
            return Err(CliError::Usage(
                "approve-credential requires non-blank --provider and --label".to_string(),
            ));
        }
        if descriptor.is_some() {
            return Err(CliError::Usage(
                "approve-credential takes no --descriptor".to_string(),
            ));
        }
        return Ok(CliCommand::ApproveCredential {
            config,
            provider,
            label,
        });
    }
    if approve_mode {
        if provider.is_some() || label.is_some() {
            return Err(CliError::Usage(
                "approve-device takes no --provider or --label".to_string(),
            ));
        }
        return Ok(CliCommand::ApproveDevice { config, descriptor });
    }
    if serve_mode {
        if descriptor.is_some() || provider.is_some() || label.is_some() {
            return Err(CliError::Usage(
                "serve takes no --descriptor, --provider, or --label".to_string(),
            ));
        }
        return Ok(CliCommand::Serve { config });
    }
    Ok(CliCommand::ShowConfig { config })
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
/// whitespace trimmed, matching wire ingress normalization); an unknown
/// descriptor fails with the pending set so the Owner can retry exactly.
///
/// There is deliberately no `--help` or `--version` handling yet: there is no
/// `stdout` mechanism under the workspace `print_stdout` deny, so they
/// currently report [`CliError::Usage`].
///
/// # Errors
///
/// Returns [`CliError::Usage`] for argument misuse, [`CliError::Config`] when
/// [`Config::load`] fails, and [`CliError::Serve`] when `serve` mode fails.
fn main() -> Result<(), CliError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_cli(&args)? {
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

/// Lists pending pairing descriptors on stdout, one per line.
///
/// Opens the Host state and prints what `approve-device --descriptor`
/// would accept. Empty output (exit 0) means nothing is pending.
///
/// # Errors
///
/// Returns [`CoreError::Store`] when the runtime cannot be built or the
/// state cannot be opened.
fn list_pending_devices(data_dir: &Path) -> Result<(), CoreError> {
    use std::io::Write as _;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| CoreError::Store("tokio runtime unavailable".to_string()))?;
    runtime.block_on(async {
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

/// Blocks the calling thread on [`serve::serve`] for `data_dir`.
///
/// Builds the multi-threaded `Tokio` runtime the listener and the store tasks
/// run on. A runtime that cannot be built is a [`CoreError::Store`] failure:
/// the runtime is the async substrate of the store-backed Host, and no
/// narrower variant names it.
///
/// # Errors
///
/// Returns [`CoreError::Store`] when the runtime cannot be built and the
/// [`serve::serve`] error otherwise.
fn run_serve(data_dir: &Path) -> Result<(), CoreError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| CoreError::Store("tokio runtime unavailable".to_string()))?;
    runtime.block_on(serve::serve(data_dir))
}

/// Records one Owner pairing approval for `descriptor` under `data_dir`.
///
/// Opens the Host state, records the decision through
/// [`HostHandle::approve_device`], and succeeds silently on approval. An
/// unknown descriptor fails with the pending descriptor set so the Owner can
/// retry with the exact value; descriptors are display strings only.
///
/// # Errors
///
/// Returns [`CoreError::Store`] when the runtime cannot be built or the state
/// cannot be opened, and [`CoreError::Approve`] when the descriptor is
/// unknown (listing the pending descriptors) or the approval write fails.
fn run_approve_device(data_dir: &Path, descriptor: &str) -> Result<(), CoreError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| CoreError::Store("tokio runtime unavailable".to_string()))?;
    runtime.block_on(approve_device_async(data_dir, descriptor))
}

/// Records one Owner credential approval under `data_dir`.
///
/// Opens the Host state and flips the pending credential request usable.
/// Unknown pairs fail with the pending set so the Owner can retry exactly.
///
/// # Errors
///
/// Returns [`CoreError::Store`] when the runtime cannot be built or the
/// state cannot be opened, and [`CoreError::Approve`] when the pair is
/// unknown.
fn run_approve_credential(data_dir: &Path, provider: &str, label: &str) -> Result<(), CoreError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| CoreError::Store("tokio runtime unavailable".to_string()))?;
    runtime.block_on(approve_credential_async(data_dir, provider, label))
}

/// Opens the Host state and records one Owner credential approval.
///
/// Split from [`run_approve_credential`] so the async body stays runtime-free.
async fn approve_credential_async(
    data_dir: &Path,
    provider: &str,
    label: &str,
) -> Result<(), CoreError> {
    let handle = HostHandle::open(data_dir).await?;
    if handle.approve_credential(provider, label).await? {
        return Ok(());
    }
    let pending = handle.pending_credentials().await?;
    Err(CoreError::Approve(format!(
        "unknown credential {provider}:{label}; pending: [{pending}]",
        pending = pending.join(", ")
    )))
}

/// Opens the Host state and records one Owner pairing approval.
///
/// Split from [`run_approve_device`] so the async body stays runtime-free.
/// The one-time pairing secret prints once to this Host-local console: that
/// console is the trusted inlet, so displaying here (and nowhere else) is
/// the distribution channel. The operator provisions it into the client's
/// protected device file.
async fn approve_device_async(data_dir: &Path, descriptor: &str) -> Result<(), CoreError> {
    use std::io::Write as _;
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
}

#[cfg(test)]
mod tests {
    use super::{
        CliCommand, extract_approve_device, extract_descriptor, extract_serve, parse_args,
        parse_cli,
    };
    use std::path::PathBuf;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn no_args_yields_no_override() {
        let args: Vec<String> = Vec::new();
        let path = parse_args(&args).expect("no args must succeed");
        assert!(path.is_none(), "no args must yield no override");
    }

    #[test]
    fn config_flag_captures_its_value() {
        let args = [String::from("--config"), String::from("/tmp/ene.json")];
        let path = parse_args(&args).expect("--config with a value must succeed");
        assert!(
            path == Some(PathBuf::from("/tmp/ene.json")),
            "the --config value must become the override"
        );
    }

    #[test]
    fn missing_config_value_is_a_usage_error() {
        let args = [String::from("--config")];
        let error = parse_args(&args).expect_err("a missing --config value must fail");
        let rendered = format!("{error}");
        assert!(
            rendered.contains("usage: ene-core [--config PATH]"),
            "a missing value must render the usage line: {rendered}"
        );
    }

    #[test]
    fn unknown_argument_is_a_usage_error() {
        let args = [String::from("--verbose")];
        let error = parse_args(&args).expect_err("an unknown argument must fail");
        let rendered = format!("{error}");
        assert!(
            rendered.contains("usage: ene-core [--config PATH]"),
            "an unknown argument must render the usage line: {rendered}"
        );
    }

    #[test]
    fn repeated_config_keeps_the_last_value() {
        let args = [
            String::from("--config"),
            String::from("/tmp/first.json"),
            String::from("--config"),
            String::from("/tmp/second.json"),
        ];
        let path = parse_args(&args).expect("a repeated --config must succeed");
        assert!(
            path == Some(PathBuf::from("/tmp/second.json")),
            "a repeated --config must keep the last value"
        );
    }

    #[test]
    fn bare_args_select_no_subcommand() {
        let args = [String::from("--config"), String::from("/tmp/ene.json")];
        let split = extract_serve(&args);
        assert!(split.is_ok(), "args without serve must split");
        let Some((serve, rest)) = split.ok() else {
            return;
        };
        assert!(!serve, "no serve token means no subcommand");
        assert_eq!(rest, args, "the rest must pass through untouched");
    }

    #[test]
    fn serve_token_splits_off_in_any_position() {
        for args in [
            vec![String::from("serve")],
            vec![
                String::from("serve"),
                String::from("--config"),
                String::from("/tmp/e.json"),
            ],
            vec![
                String::from("--config"),
                String::from("/tmp/e.json"),
                String::from("serve"),
            ],
        ] {
            let split = extract_serve(&args);
            assert!(split.is_ok(), "a single serve must split: {args:?}");
            let Some((serve, rest)) = split.ok() else {
                return;
            };
            assert!(serve, "the serve token must select the subcommand");
            assert!(
                !rest.iter().any(|arg| arg == "serve"),
                "the rest must not keep serve: {rest:?}"
            );
        }
    }

    #[test]
    fn repeated_serve_is_a_usage_error() {
        let args = [String::from("serve"), String::from("serve")];
        let split = extract_serve(&args);
        assert!(split.is_err(), "a repeated serve must fail");
        let Some(error) = split.err() else {
            return;
        };
        let rendered = format!("{error}");
        assert!(
            rendered.contains("usage: ene-core [--config PATH]"),
            "a repeated serve must render the usage line: {rendered}"
        );
    }

    #[test]
    fn approve_device_token_splits_off_in_any_position() {
        for args in [
            vec![String::from("approve-device")],
            vec![
                String::from("approve-device"),
                String::from("--descriptor"),
                String::from("laptop"),
            ],
            vec![
                String::from("--descriptor"),
                String::from("laptop"),
                String::from("approve-device"),
            ],
        ] {
            let split = extract_approve_device(&args);
            assert!(
                split.is_ok(),
                "a single approve-device must split: {args:?}"
            );
            let Some((approve, rest)) = split.ok() else {
                return;
            };
            assert!(approve, "the token must select the subcommand");
            assert!(
                !rest.iter().any(|arg| arg == "approve-device"),
                "the rest must not keep the token: {rest:?}"
            );
        }
    }

    #[test]
    fn approve_credential_splits_and_parses_named_flags() {
        use super::{extract_approve_credential, extract_named};
        let args = [
            String::from("approve-credential"),
            String::from("--provider"),
            String::from("openai"),
            String::from("--label"),
            String::from("main"),
        ];
        let split = extract_approve_credential(&args);
        assert!(split.is_ok(), "a single approve-credential must split");
        let Ok((approve, rest)) = split else {
            return;
        };
        assert!(approve, "the token must select the subcommand");
        let named = extract_named(&rest, "--provider");
        assert!(named.is_ok(), "provider flag must parse");
        let Ok((provider, rest)) = named else {
            return;
        };
        assert_eq!(provider, Some(String::from("openai")));
        let named = extract_named(&rest, "--label");
        assert!(named.is_ok(), "label flag must parse");
        let Ok((label, rest)) = named else {
            return;
        };
        assert_eq!(label, Some(String::from("main")));
        assert!(rest.is_empty(), "nothing must remain: {rest:?}");
        let missing = extract_named(&[String::from("--provider")], "--provider");
        assert!(missing.is_err(), "a valueless flag must fail");
    }

    #[test]
    fn repeated_approve_device_is_a_usage_error() {
        let args = [
            String::from("approve-device"),
            String::from("approve-device"),
        ];
        let split = extract_approve_device(&args);
        assert!(split.is_err(), "a repeated approve-device must fail");
        let Some(error) = split.err() else {
            return;
        };
        let rendered = format!("{error}");
        assert!(
            rendered.contains("usage: ene-core [--config PATH]"),
            "a repeated approve-device must render the usage line: {rendered}"
        );
    }

    #[test]
    fn descriptor_flag_captures_its_value_verbatim() {
        let args = [
            String::from("--descriptor"),
            String::from("--odd-value"),
            String::from("--config"),
            String::from("/tmp/e.json"),
        ];
        let parsed = extract_descriptor(&args);
        assert!(parsed.is_ok(), "--descriptor with a value must split");
        let Some((descriptor, rest)) = parsed.ok() else {
            return;
        };
        assert_eq!(
            descriptor,
            Some(String::from("--odd-value")),
            "the value is consumed verbatim, even with a leading --"
        );
        assert_eq!(
            rest,
            vec![String::from("--config"), String::from("/tmp/e.json"),],
            "the rest must pass through untouched"
        );
    }

    #[test]
    fn repeated_descriptor_keeps_the_last_value() {
        let args = [
            String::from("--descriptor"),
            String::from("first"),
            String::from("--descriptor"),
            String::from("second"),
        ];
        let parsed = extract_descriptor(&args);
        assert!(parsed.is_ok(), "a repeated --descriptor must split");
        let Some((descriptor, _)) = parsed.ok() else {
            return;
        };
        assert_eq!(
            descriptor,
            Some(String::from("second")),
            "a repeated --descriptor must keep the last value"
        );
    }

    #[test]
    fn missing_descriptor_value_is_a_usage_error() {
        let args = [String::from("--descriptor")];
        let parsed = extract_descriptor(&args);
        assert!(parsed.is_err(), "a missing --descriptor value must fail");
        let Some(error) = parsed.err() else {
            return;
        };
        let rendered = format!("{error}");
        assert!(
            rendered.contains("usage: ene-core [--config PATH]"),
            "a missing value must render the usage line: {rendered}"
        );
    }

    #[test]
    fn absent_descriptor_yields_no_value() {
        let args = [String::from("--config"), String::from("/tmp/e.json")];
        let parsed = extract_descriptor(&args);
        assert!(parsed.is_ok(), "args without --descriptor must split");
        let Some((descriptor, rest)) = parsed.ok() else {
            return;
        };
        assert!(descriptor.is_none(), "no flag means no descriptor");
        assert_eq!(rest, args, "the rest must pass through untouched");
    }

    #[test]
    fn config_value_named_serve_is_not_a_subcommand() {
        let parsed = parse_cli(&args(&["--config", "serve"]));
        assert!(
            matches!(
                parsed,
                Ok(CliCommand::ShowConfig {
                    config: Some(_),
                    ..
                })
            ),
            "a --config value is data, never a subcommand, got {parsed:?}"
        );
        let Ok(CliCommand::ShowConfig { config }) = parsed else {
            return;
        };
        assert_eq!(
            config,
            Some(PathBuf::from("serve")),
            "the value must survive verbatim"
        );
    }

    #[test]
    fn descriptor_value_named_serve_is_not_a_subcommand() {
        let parsed = parse_cli(&args(&["approve-device", "--descriptor", "serve"]));
        assert!(
            matches!(
                parsed,
                Ok(CliCommand::ApproveDevice {
                    descriptor: Some(_),
                    ..
                })
            ),
            "a --descriptor value is data, got {parsed:?}"
        );
        let Ok(CliCommand::ApproveDevice { descriptor, .. }) = parsed else {
            return;
        };
        assert_eq!(
            descriptor.as_deref(),
            Some("serve"),
            "the descriptor value must survive verbatim"
        );
    }

    #[test]
    fn flags_before_the_subcommand_still_parse() {
        let parsed = parse_cli(&args(&[
            "--config",
            "/tmp/e.json",
            "--descriptor",
            "laptop",
            "approve-device",
        ]));
        assert!(
            matches!(
                parsed,
                Ok(CliCommand::ApproveDevice {
                    config: Some(_),
                    descriptor: Some(_),
                })
            ),
            "order-independent flags must parse, got {parsed:?}"
        );
    }

    #[test]
    fn stray_mode_flags_are_rejected() {
        let parsed = parse_cli(&args(&["serve", "--descriptor", "laptop"]));
        assert!(
            parsed.is_err(),
            "serve must not silently swallow --descriptor, got {parsed:?}"
        );
        let parsed = parse_cli(&args(&["approve-device", "--provider", "openai"]));
        assert!(
            parsed.is_err(),
            "approve-device must not silently swallow --provider, got {parsed:?}"
        );
    }

    #[test]
    fn unknown_trailing_arguments_are_rejected() {
        let parsed = parse_cli(&args(&["serve", "extra"]));
        assert!(
            parsed.is_err(),
            "trailing garbage must fail, got {parsed:?}"
        );
    }

    #[test]
    fn combined_subcommands_are_rejected() {
        let parsed = parse_cli(&args(&["serve", "approve-device"]));
        assert!(parsed.is_err(), "combined modes must fail, got {parsed:?}");
    }
}
