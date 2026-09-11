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
    /// The display always contains the usage line
    /// `usage: ene-core [--config PATH] [serve | approve-device --descriptor EXACT | approve-credential --provider P --label L]`
    /// followed by the detail, so callers can assert on the usage line alone.
    /// The bracketed prefix keeps the `Stage 1` usage line as a substring.
    #[error(
        "usage: ene-core [--config PATH] [serve | approve-device --descriptor EXACT | approve-credential --provider P --label L]: {0}"
    )]
    Usage(String),
    #[error(transparent)]
    Config(#[from] ene_config::typed::ConfigError),
    #[error(transparent)]
    Serve(#[from] CoreError),
}

/// Splits a subcommand token out of `args`, wherever it appears.
///
/// Both `ene-core serve --config PATH` and `ene-core --config PATH serve`
/// work, and the remaining arguments are what [`extract_named`] later parses
/// for that mode's flags. A repeated token is a usage error.
fn extract_subcommand(args: &[String], name: &str) -> Result<(bool, Vec<String>), CliError> {
    let mut found = false;
    let mut rest = Vec::new();
    for arg in args {
        if arg.as_str() == name {
            if found {
                return Err(CliError::Usage(format!("duplicate subcommand: {name}")));
            }
            found = true;
        } else {
            rest.push(arg.clone());
        }
    }
    Ok((found, rest))
}

/// Later flags override earlier ones; the value is consumed verbatim, even
/// when it starts with `--`.
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

/// A repeated `--descriptor` keeps the last value; the value is consumed
/// verbatim, even when it starts with `--`.
fn extract_descriptor(args: &[String]) -> Result<(Option<String>, Vec<String>), CliError> {
    extract_named(args, "--descriptor")
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

/// Option flags (with their values) are extracted FIRST and subcommand
/// tokens are scanned only in the remainder, so a legal value is never
/// mistaken for a subcommand: `ene-core --config serve` selects config
/// path `serve`, and `approve-device --descriptor serve` approves the
/// `serve` descriptor. Flags for a different mode are rejected rather
/// than silently ignored.
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
    let (serve_mode, rest) = extract_subcommand(&rest, "serve")?;
    let (approve_mode, rest) = extract_subcommand(&rest, "approve-device")?;
    let (approve_cred_mode, rest) = extract_subcommand(&rest, "approve-credential")?;
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
    if let Some(unknown) = rest.first() {
        return Err(CliError::Usage(format!("unknown argument: {unknown}")));
    }
    let config = config.map(PathBuf::from);
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
/// whitespace trimmed, matching wire ingress normalization).
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

/// Prints what `approve-device --descriptor` would accept, one descriptor per
/// line. Empty output (exit 0) means nothing is pending.
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

/// An unknown descriptor fails with the pending descriptor set so the Owner
/// can retry with the exact value; descriptors are display strings only.
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

/// Split from [`run_approve_device`] so the async body stays runtime-free.
/// The one-time pairing secret prints once to this Host-local console, the
/// trusted inlet, and nowhere else; the operator provisions it into the
/// client's protected device file.
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
    use super::{CliCommand, extract_descriptor, extract_named, extract_subcommand, parse_cli};
    use std::path::PathBuf;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn no_args_yields_no_override() {
        let parsed = parse_cli(&[]).expect("no args must succeed");
        assert_eq!(parsed, CliCommand::ShowConfig { config: None });
    }

    #[test]
    fn config_flag_captures_its_value() {
        let parsed =
            parse_cli(&args(&["--config", "/tmp/ene.json"])).expect("--config must succeed");
        assert_eq!(
            parsed,
            CliCommand::ShowConfig {
                config: Some(PathBuf::from("/tmp/ene.json"))
            }
        );
    }

    #[test]
    fn missing_config_value_is_a_usage_error() {
        let error =
            parse_cli(&args(&["--config"])).expect_err("a missing --config value must fail");
        let rendered = format!("{error}");
        assert!(
            rendered.contains("usage: ene-core [--config PATH]"),
            "a missing value must render the usage line: {rendered}"
        );
    }

    #[test]
    fn unknown_argument_is_a_usage_error() {
        let error = parse_cli(&args(&["--verbose"])).expect_err("an unknown argument must fail");
        let rendered = format!("{error}");
        assert!(
            rendered.contains("usage: ene-core [--config PATH]"),
            "an unknown argument must render the usage line: {rendered}"
        );
    }

    #[test]
    fn repeated_config_keeps_the_last_value() {
        let parsed = parse_cli(&args(&[
            "--config",
            "/tmp/first.json",
            "--config",
            "/tmp/second.json",
        ]))
        .expect("a repeated --config must succeed");
        assert_eq!(
            parsed,
            CliCommand::ShowConfig {
                config: Some(PathBuf::from("/tmp/second.json"))
            }
        );
    }

    #[test]
    fn bare_args_select_no_subcommand() {
        let args = [String::from("--config"), String::from("/tmp/ene.json")];
        let split = extract_subcommand(&args, "serve");
        assert!(split.is_ok(), "args without serve must split");
        let (serve, rest) = split.ok().unwrap();
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
            let split = extract_subcommand(&args, "serve");
            assert!(split.is_ok(), "a single serve must split: {args:?}");
            let (serve, rest) = split.ok().unwrap();
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
        let split = extract_subcommand(&args, "serve");
        assert!(split.is_err(), "a repeated serve must fail");
        let error = split.err().unwrap();
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
            let split = extract_subcommand(&args, "approve-device");
            assert!(
                split.is_ok(),
                "a single approve-device must split: {args:?}"
            );
            let (approve, rest) = split.ok().unwrap();
            assert!(approve, "the token must select the subcommand");
            assert!(
                !rest.iter().any(|arg| arg == "approve-device"),
                "the rest must not keep the token: {rest:?}"
            );
        }
    }

    #[test]
    fn approve_credential_splits_and_parses_named_flags() {
        let args = [
            String::from("approve-credential"),
            String::from("--provider"),
            String::from("openai"),
            String::from("--label"),
            String::from("main"),
        ];
        let split = extract_subcommand(&args, "approve-credential");
        let (approve, rest) = split.unwrap();
        assert!(approve, "the token must select the subcommand");
        let named = extract_named(&rest, "--provider");
        let (provider, rest) = named.unwrap();
        assert_eq!(provider, Some(String::from("openai")));
        let named = extract_named(&rest, "--label");
        let (label, rest) = named.unwrap();
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
        let split = extract_subcommand(&args, "approve-device");
        assert!(split.is_err(), "a repeated approve-device must fail");
        let error = split.err().unwrap();
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
        let (descriptor, rest) = parsed.ok().unwrap();
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
        let (descriptor, _) = parsed.ok().unwrap();
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
        let error = parsed.err().unwrap();
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
        let (descriptor, rest) = parsed.ok().unwrap();
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
        let CliCommand::ShowConfig { config } = parsed.unwrap() else {
            panic!("unexpected variant");
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
        let CliCommand::ApproveDevice { descriptor, .. } = parsed.unwrap() else {
            panic!("unexpected variant");
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
